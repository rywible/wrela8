//! The typed tree (plans/M3.md item A, decision 1): sema's product, not
//! an IR. A plain-enum mirror of `syntax::ast`'s `Expr`/`Stmt`/
//! declaration shapes — no lowering, no CFG, no SSA, no node IDs, no
//! arena, no `Rc`. Produced by `bodies.rs`'s expression/statement checker
//! (`check_expr`/`check_stmt` now return a typed node instead of a bare
//! `Type`/`()`); `access.rs`/`flow.rs`/`matches.rs` are **not** retrofitted
//! onto this in M3 — they keep consuming the ast, peeking at a typed
//! node's `.ty` only where they already asked `bodies::check_expr` for a
//! `Type` (a recorded non-goal, plans/M3.md).
//!
//! Every expression node carries its resolved `Type` (`types::Type`,
//! reused directly — no duplicate type representation). A call carries a
//! by-value callee key (`CalleeKey`): a dotted path for a plain fn/method,
//! or `generics::canonical_key`'s own "kind:name[args]" spelling for an
//! instantiated generic (so a `Call`'s key and `TypedProgram::instantiations`'s
//! own map key are the identical string for the generic-fn/generic-struct-
//! method cases — no second lookup scheme). Desugared operators
//! (a user (`Named`) type's `+`/`-`/`*`/`/`/`%`/`<`, 05-library.md §8) are
//! already method-call-shaped (`OpCall`); a builtin scalar/`bool`/`char`
//! op stays the primitive `Binary` node. Literals carry their settled
//! scalar type (the i64/u64/f64 defaulting already resolved by
//! `bodies.rs`); the literal's own source text is kept verbatim (clone
//! freely) rather than pre-parsed to a value — decoding it is the
//! evaluator's job (plans/M3.md item B). `?` stays one node carrying its
//! conversion-target key (`None` when the error type already matches,
//! `Some(key)` naming the `<Target>.from` conversion — the same key
//! whether the target's `From` came from an explicit `from` fn or
//! `deriving(From)`, since both desugar to the identical shape).  Enum
//! constructions (`Some`/`Ok`/`Err`/leading-dot/`Enum.Variant`) carry
//! `(enum name, variant name)` resolved. A closure carries its structural
//! `fn` type (on the wrapping node) plus its own params/body.
//!
//! A fn/method/init's own parameter defaults are typed **once**, on the
//! declaration (`TypedParam::default`) — not re-typed at every call site
//! (which would be wrong wherever a default references the callee's own
//! `self`, and pointless duplication otherwise). A `Call` node's argument
//! list therefore aligns 1:1 with the callee's declared parameters,
//! `None` for a slot the call site left to that stored default. A struct
//! literal's un-supplied, defaulted fields are elided the same way
//! (`TypedStruct::field_defaults` carries the field's own default once).

use std::collections::BTreeMap;

use crate::sema::types::{self, Type};
use crate::syntax::ast::{AccessMode, BinOp};

// --- callee keys (decision 1) --------------------------------------------

/// A call's by-value target, resolved at check time. `Fn`/`Method` are a
/// plain (never-generic) dotted path; `FnInstance`/`MethodInstance` carry
/// `generics::canonical_key`'s own "kind:name[args]" spelling for the
/// instantiated generic fn/struct — identical to
/// `TypedProgram::instantiations`'s own map key for the fn case, and to
/// that key plus `.method` for a method reached through an instantiated
/// struct.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CalleeKey {
    /// A plain top-level fn, never generic: its bare name.
    Fn(String),
    /// An instantiated top-level generic fn: `generics::canonical_key`'s
    /// `"fn:name[args]"` spelling.
    FnInstance(String),
    /// A method/associated fn/`init` on a plain (non-generic) struct:
    /// `(struct name, member name)`.
    Method(String, String),
    /// A method/associated fn/`init` reached through an instantiated
    /// generic struct: (`generics::canonical_key`'s `"struct:name[args]"`
    /// spelling, member name).
    MethodInstance(String, String),
}

impl CalleeKey {
    pub fn spelling(&self) -> String {
        match self {
            CalleeKey::Fn(name) => name.clone(),
            CalleeKey::FnInstance(key) => key.clone(),
            CalleeKey::Method(ty, member) => format!("{ty}.{member}"),
            CalleeKey::MethodInstance(key, member) => format!("{key}.{member}"),
        }
    }
}

// --- expressions -----------------------------------------------------------

#[derive(Debug, Clone, PartialEq)]
pub struct TypedExpr {
    pub ty: Type,
    pub kind: TypedExprKind,
}

#[derive(Debug, Clone, PartialEq)]
pub enum TypedExprKind {
    /// An integer literal's source text, unparsed (the evaluator, item B,
    /// decodes it); `ty` above already carries its settled scalar type.
    Int(String),
    Float(String),
    /// Always `Static[Str]` (see `ty`).
    Str(String),
    /// Always `Static[Bytes[N]]` (see `ty`).
    BStr(String),
    Char(String),
    Bool(bool),
    Unit,
    /// A local variable/parameter/`self` reference.
    Local(String),
    /// A module-level `const` reference.
    Const(String),
    /// A bare fn/method value reference, never called directly here (the
    /// call-shaped forms below cover every direct call).
    FnRef(CalleeKey),
    /// A struct field read. `unwrap_own` has already stripped any `own[P]`
    /// wrapper before matching the base's type, but the base node itself
    /// keeps its own true (possibly `own`-wrapped) type.
    Field(Box<TypedExpr>, String),
    /// `base[index]` — a fixed array or `Bytes` element read.
    Index(Box<TypedExpr>, Box<TypedExpr>),
    /// A resolved call: `receiver` is `Some` for a method/`init` call,
    /// `None` for a plain fn/associated-fn call; `args` aligns 1:1 with
    /// the callee's declared parameters, `None` for a slot left to the
    /// callee's own stored default (`TypedParam::default`).
    Call {
        callee: CalleeKey,
        receiver: Option<Box<TypedExpr>>,
        args: Vec<Option<TypedExpr>>,
    },
    /// Calling a `fn(...)`-typed value (a closure, or a plain fn/method
    /// referenced as a value): positional only, no defaults possible (a
    /// raw `fn` type carries none).
    CallValue(Box<TypedExpr>, Vec<TypedExpr>),
    /// `x.to[T]()` — a scalar-to-scalar conversion (`checked_to`/
    /// `truncate_to` still fail closed in sema, so they never reach here).
    ToScalar(Box<TypedExpr>),
    Neg(Box<TypedExpr>),
    BitNot(Box<TypedExpr>),
    /// Unary `take`: same type/value as `inner`, moved.
    Take(Box<TypedExpr>),
    /// Postfix `?`. `conv` is `None` when the error type already matches
    /// (or `Option`'s case, which never converts); `Some(key)` names the
    /// `<Target>.from`-shaped conversion (`explicit `from``or
    /// `deriving(From)` — same key either way).
    Try(Box<TypedExpr>, Option<CalleeKey>),
    /// A builtin scalar/`bool`/`char` binary op — never desugared.
    Binary(BinOp, Box<TypedExpr>, Box<TypedExpr>),
    /// A user (`Named`) type's desugared operator method
    /// (05-library.md §8): `self`, then the right-hand operand.
    OpCall(CalleeKey, Box<TypedExpr>, Box<TypedExpr>),
    Is(Box<TypedExpr>, Box<TypedPattern>),
    Not(Box<TypedExpr>),
    And(Box<TypedExpr>, Box<TypedExpr>),
    Or(Box<TypedExpr>, Box<TypedExpr>),
    /// `Some`/`Ok`/`Err`/leading-dot/`Enum.Variant` construction, resolved
    /// to `(enum name, variant name)` — `enum_name` is `"Option"` /
    /// `"Result"` for the two builtin sums, else a user enum's own name.
    EnumConstruct {
        enum_name: String,
        variant: String,
        args: Vec<TypedExpr>,
    },
    Closure {
        params: Vec<TypedClosureParam>,
        body: TypedClosureBody,
    },
    Tuple(Vec<TypedExpr>),
    /// A fixed-array literal.
    List(Vec<TypedExpr>),
    /// A struct literal (no `init`): only the explicitly supplied fields,
    /// declaration order — an omitted, defaulted field is elided (its
    /// default lives once on `TypedStruct::field_defaults`).
    StructLiteral {
        name: String,
        fields: Vec<(String, TypedExpr)>,
    },
    /// `panic(msg)` — comptime abandonment (plans/M3.md item B); `ty` is
    /// always `never`.
    Panic(Box<TypedExpr>),
}

#[derive(Debug, Clone, PartialEq)]
pub struct TypedClosureParam {
    pub mode: AccessMode,
    pub name: String,
    pub ty: Type,
}

#[derive(Debug, Clone, PartialEq)]
pub enum TypedClosureBody {
    Expr(Box<TypedExpr>),
    Suite(Vec<TypedStmt>),
}

// --- patterns ----------------------------------------------------------

#[derive(Debug, Clone, PartialEq)]
pub struct TypedPattern {
    /// The type this pattern matches against (its scrutinee's type at
    /// this position) — not a "type of the pattern" in any other sense.
    pub ty: Type,
    pub kind: TypedPatternKind,
}

#[derive(Debug, Clone, PartialEq)]
pub enum TypedPatternKind {
    Wildcard,
    Literal(Box<TypedExpr>),
    Binding(String),
    Take(Box<TypedPattern>),
    /// `enum_name` is `"Option"`/`"Result"` for the two builtin sums,
    /// else a user enum's own name — resolved the same way as
    /// `TypedExprKind::EnumConstruct`.
    Variant {
        enum_name: String,
        variant: String,
        payload: Vec<TypedPattern>,
    },
    Tuple(Vec<TypedPattern>),
    Array(Vec<TypedPattern>),
    Or(Vec<TypedPattern>),
}

// --- statements --------------------------------------------------------

#[derive(Debug, Clone, PartialEq)]
pub struct TypedStmt {
    pub kind: TypedStmtKind,
}

#[derive(Debug, Clone, PartialEq)]
pub struct TypedElif {
    pub cond: TypedExpr,
    pub body: Vec<TypedStmt>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct TypedMatchArm {
    pub pattern: TypedPattern,
    pub guard: Option<TypedExpr>,
    pub body: Vec<TypedStmt>,
}

/// A `for` loop's iterable, resolved: either a range (its two endpoints,
/// already same-typed, plus inclusivity) or a fixed-array-valued
/// expression — mirrors `bodies::check_for`'s own two-shape derivation.
#[derive(Debug, Clone, PartialEq)]
pub enum TypedForIter {
    Range(TypedExpr, TypedExpr, bool),
    Expr(TypedExpr),
}

#[derive(Debug, Clone, PartialEq)]
pub enum TypedDeferBody {
    Expr(Box<TypedExpr>),
    Suite(Vec<TypedStmt>),
}

#[derive(Debug, Clone, PartialEq)]
pub enum TypedStmtKind {
    /// A fresh local binding: `name = value` where `name` was not already
    /// bound in the current scope.
    Let {
        name: String,
        ty: Type,
        value: TypedExpr,
    },
    /// A reassignment or place assignment (`target = value`); a compound
    /// assignment (`+=`, ...) has already been desugared into `value`
    /// (a `Binary`/`OpCall` re-reading `target`) by the checker, exactly
    /// like the language's own `a += b` == `a = a.add(b)` rule
    /// (02-language.md §7.4) — there is no separate compound-assign node.
    Assign {
        target: TypedExpr,
        value: TypedExpr,
    },
    If {
        cond: TypedExpr,
        then_branch: Vec<TypedStmt>,
        elifs: Vec<TypedElif>,
        else_branch: Option<Vec<TypedStmt>>,
    },
    Match {
        scrutinee: TypedExpr,
        arms: Vec<TypedMatchArm>,
    },
    For {
        name: String,
        elem_ty: Type,
        take_binding: bool,
        iter: TypedForIter,
        body: Vec<TypedStmt>,
    },
    While {
        cond: TypedExpr,
        body: Vec<TypedStmt>,
    },
    Break,
    Continue,
    Pass,
    Return(Option<TypedExpr>),
    Assert {
        cond: TypedExpr,
        message: Option<TypedExpr>,
    },
    Defer(TypedDeferBody),
    ExprStmt(TypedExpr),
}

// --- declarations --------------------------------------------------------

#[derive(Debug, Clone, PartialEq)]
pub struct TypedParam {
    pub mode: AccessMode,
    pub name: String,
    pub ty: Type,
    pub default: Option<TypedExpr>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct TypedFn {
    /// `Some((mode, self type))` for a method/`init`; `None` for a plain
    /// fn or an associated fn (no receiver).
    pub receiver: Option<(AccessMode, Type)>,
    pub params: Vec<TypedParam>,
    pub ret: Type,
    pub body: Vec<TypedStmt>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct TypedConst {
    pub ty: Type,
    pub value: TypedExpr,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct TypedStruct {
    pub name: String,
    /// Declared field order (plans/M3.md item B's own addition — a
    /// producer gap found evaluating struct construction/field access:
    /// decision 5 stores a struct value as a field-ordered `Vec<Value>`,
    /// but nothing before this recorded that order anywhere in the typed
    /// tree; `bodies::check_struct_members` already walks the fields in
    /// declaration order to build `field_defaults`, so this is that same
    /// walk's field names, kept). Not rendered by `dump` below — it is
    /// evaluator-only bookkeeping, not part of the pinned dump grammar,
    /// so no existing golden's text changes.
    pub fields: Vec<String>,
    /// Every field that declared a default, typed once (with `self`
    /// bound, since a default may reference it) — a struct literal that
    /// omits the field elides it from its own `fields` list instead of
    /// re-typing the same default at every construction site.
    pub field_defaults: BTreeMap<String, TypedExpr>,
    pub methods: BTreeMap<String, TypedFn>,
    pub assoc_fns: BTreeMap<String, TypedFn>,
    pub init: Option<TypedFn>,
}

/// One generic instantiation's checked body (plans/M2.md item H, carried
/// into M3): keyed in `TypedProgram::instantiations` by
/// `generics::canonical_key`'s own spelling. An enum instantiation has no
/// body to check (02-language.md §7.2: enums carry no methods) — nothing
/// beyond confirming it was checked/reclassified, so it carries no
/// payload.
#[derive(Debug, Clone, PartialEq)]
pub enum TypedInstantiation {
    Fn(TypedFn),
    Struct(TypedStruct),
    Enum,
}

/// `@test` attribute kind (plans/M3.md item E, decision 9): `Comptime`
/// for a bare `@test` — `wrela test`'s own job, run in the build
/// evaluator under a fresh quota; `Runtime` for `@test(runtime)`
/// (02-language.md §12.2: booted on the wrela machine runner, a
/// generated image test — M5, fail-closed here, decision 10's own named
/// gap).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TestKind {
    Comptime,
    Runtime,
}

/// One `@test`-attributed module-level fn (`sema::bodies::test_attr_kind`
/// validates the attribute's own shape — zero params, its lone optional
/// argument the bare name `runtime`), recorded in declaration order
/// (`TypedProgram::tests`'s own doc comment — the typed tree's `fns` map
/// is `BTreeMap`-keyed for determinism everywhere else, decision 1's own
/// convention, but `wrela test` (decision 9) must run tests in source
/// order, not name order, so this is a second, order-preserving record
/// of the exact same fns, not a replacement for the map). Not rendered
/// by `dump` below — same reasoning as `TypedStruct::fields`/
/// `TypedProgram::enums`: `wrela test`'s own report is the pinned
/// surface for this data, not `--stage=typed`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TestDecl {
    pub name: String,
    pub kind: TestKind,
}

/// Sema's product (plans/M3.md decision 1): every top-level `const`/
/// non-generic `fn`/non-generic `struct`'s checked body, plus every
/// concrete generic instantiation any of them (or `access`/`flow`/
/// `matches`'s own re-derivation) discovered — `BTreeMap`-keyed
/// throughout for determinism (CLAUDE.md).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct TypedProgram {
    pub consts: BTreeMap<String, TypedConst>,
    pub fns: BTreeMap<String, TypedFn>,
    pub structs: BTreeMap<String, TypedStruct>,
    /// Every `@test`-attributed module-level fn, in declaration order
    /// (plans/M3.md item E's own producer addition — `TestDecl`'s own
    /// doc comment).
    pub tests: Vec<TestDecl>,
    /// Every top-level (non-generic) user enum's own variant names, in
    /// declaration order (plans/M3.md item B's own addition — the same
    /// producer gap as `TypedStruct::fields`: an enum has no body to
    /// check, so nothing before this recorded its declaration anywhere
    /// in the typed tree, yet decision 5's `Value::Enum(variant index,
    /// payload)` representation needs exactly this order to construct or
    /// match a user enum's value). Not rendered by `dump` below — same
    /// reasoning as `TypedStruct::fields`.
    pub enums: BTreeMap<String, Vec<String>>,
    pub instantiations: BTreeMap<String, TypedInstantiation>,
}

// --- the `--stage=typed` dump (decision 2) --------------------------------
//
// One node per line, two-space indent per nesting level, `Kind
// key=value ty=<type>` in the M1 dump style (`syntax::parser`'s own
// `dump`/`dump_expr` conventions) — no spans anywhere (a typed node has
// none of its own; the ast's spans are exactly what this tree does not
// carry, decision 1). `Program`'s four maps are already `BTreeMap`s, so
// iterating each in order is deterministic.

fn push_line(out: &mut String, depth: usize, line: &str) {
    out.push_str(&"  ".repeat(depth));
    out.push_str(line);
    out.push('\n');
}

fn ty(t: &Type) -> String {
    types::render_type(t)
}

pub fn dump(program: &TypedProgram) -> String {
    let mut out = String::new();
    out.push_str("Program\n");
    for (name, c) in &program.consts {
        push_line(&mut out, 1, &format!("Const name={name} ty={}", ty(&c.ty)));
        dump_expr(&c.value, 2, &mut out);
    }
    for (name, f) in &program.fns {
        push_line(&mut out, 1, &format!("Fn name={name} ret={}", ty(&f.ret)));
        dump_fn_body(f, 2, &mut out);
    }
    for (name, s) in &program.structs {
        push_line(&mut out, 1, &format!("Struct name={name}"));
        dump_struct_body(s, 2, &mut out);
    }
    for (key, inst) in &program.instantiations {
        push_line(&mut out, 1, &format!("Instantiation key={key}"));
        match inst {
            TypedInstantiation::Fn(f) => {
                push_line(&mut out, 2, &format!("Fn ret={}", ty(&f.ret)));
                dump_fn_body(f, 3, &mut out);
            }
            TypedInstantiation::Struct(s) => {
                push_line(&mut out, 2, "Struct");
                dump_struct_body(s, 3, &mut out);
            }
            TypedInstantiation::Enum => push_line(&mut out, 2, "Enum"),
        }
    }
    out
}

fn dump_param(p: &TypedParam, depth: usize, out: &mut String) {
    push_line(
        out,
        depth,
        &format!(
            "Param name={} mode={} ty={}",
            p.name,
            p.mode.as_str(),
            ty(&p.ty)
        ),
    );
    if let Some(def) = &p.default {
        push_line(out, depth + 1, "Default");
        dump_expr(def, depth + 2, out);
    }
}

fn dump_fn_body(f: &TypedFn, depth: usize, out: &mut String) {
    if let Some((mode, self_ty)) = &f.receiver {
        push_line(
            out,
            depth,
            &format!("Receiver mode={} ty={}", mode.as_str(), ty(self_ty)),
        );
    }
    for p in &f.params {
        dump_param(p, depth, out);
    }
    push_line(out, depth, "Body");
    dump_stmts(&f.body, depth + 1, out);
}

fn dump_struct_body(s: &TypedStruct, depth: usize, out: &mut String) {
    for (name, def) in &s.field_defaults {
        push_line(out, depth, &format!("FieldDefault name={name}"));
        dump_expr(def, depth + 1, out);
    }
    for (name, f) in &s.methods {
        push_line(
            out,
            depth,
            &format!("Method name={name} ret={}", ty(&f.ret)),
        );
        dump_fn_body(f, depth + 1, out);
    }
    for (name, f) in &s.assoc_fns {
        push_line(
            out,
            depth,
            &format!("AssocFn name={name} ret={}", ty(&f.ret)),
        );
        dump_fn_body(f, depth + 1, out);
    }
    if let Some(f) = &s.init {
        push_line(out, depth, &format!("Init ret={}", ty(&f.ret)));
        dump_fn_body(f, depth + 1, out);
    }
}

fn dump_stmts(stmts: &[TypedStmt], depth: usize, out: &mut String) {
    for s in stmts {
        dump_stmt(s, depth, out);
    }
}

fn dump_stmt(stmt: &TypedStmt, depth: usize, out: &mut String) {
    match &stmt.kind {
        TypedStmtKind::Let { name, ty: t, value } => {
            push_line(out, depth, &format!("Let name={name} ty={}", ty(t)));
            dump_expr(value, depth + 1, out);
        }
        TypedStmtKind::Assign { target, value } => {
            push_line(out, depth, "Assign");
            dump_expr(target, depth + 1, out);
            dump_expr(value, depth + 1, out);
        }
        TypedStmtKind::If {
            cond,
            then_branch,
            elifs,
            else_branch,
        } => {
            push_line(out, depth, "If");
            dump_expr(cond, depth + 1, out);
            push_line(out, depth + 1, "Then");
            dump_stmts(then_branch, depth + 2, out);
            for elif in elifs {
                push_line(out, depth + 1, "Elif");
                dump_expr(&elif.cond, depth + 2, out);
                dump_stmts(&elif.body, depth + 2, out);
            }
            if let Some(b) = else_branch {
                push_line(out, depth + 1, "Else");
                dump_stmts(b, depth + 2, out);
            }
        }
        TypedStmtKind::Match { scrutinee, arms } => {
            push_line(out, depth, "Match");
            dump_expr(scrutinee, depth + 1, out);
            for arm in arms {
                push_line(out, depth + 1, "Case");
                dump_pattern(&arm.pattern, depth + 2, out);
                if let Some(g) = &arm.guard {
                    push_line(out, depth + 2, "Guard");
                    dump_expr(g, depth + 3, out);
                }
                dump_stmts(&arm.body, depth + 2, out);
            }
        }
        TypedStmtKind::For {
            name,
            elem_ty,
            take_binding,
            iter,
            body,
        } => {
            let mut header = format!("For name={name} elem_ty={}", ty(elem_ty));
            if *take_binding {
                header.push_str(" take=true");
            }
            push_line(out, depth, &header);
            match iter {
                TypedForIter::Range(from, to, incl) => {
                    push_line(out, depth + 1, &format!("Range inclusive={incl}"));
                    dump_expr(from, depth + 2, out);
                    dump_expr(to, depth + 2, out);
                }
                TypedForIter::Expr(e) => dump_expr(e, depth + 1, out),
            }
            push_line(out, depth + 1, "Body");
            dump_stmts(body, depth + 2, out);
        }
        TypedStmtKind::While { cond, body } => {
            push_line(out, depth, "While");
            dump_expr(cond, depth + 1, out);
            push_line(out, depth + 1, "Body");
            dump_stmts(body, depth + 2, out);
        }
        TypedStmtKind::Break => push_line(out, depth, "Break"),
        TypedStmtKind::Continue => push_line(out, depth, "Continue"),
        TypedStmtKind::Pass => push_line(out, depth, "Pass"),
        TypedStmtKind::Return(value) => {
            push_line(out, depth, "Return");
            if let Some(v) = value {
                dump_expr(v, depth + 1, out);
            }
        }
        TypedStmtKind::Assert { cond, message } => {
            push_line(out, depth, "Assert");
            dump_expr(cond, depth + 1, out);
            if let Some(m) = message {
                dump_expr(m, depth + 1, out);
            }
        }
        TypedStmtKind::Defer(body) => {
            push_line(out, depth, "Defer");
            match body {
                TypedDeferBody::Expr(e) => dump_expr(e, depth + 1, out),
                TypedDeferBody::Suite(stmts) => {
                    push_line(out, depth + 1, "Body");
                    dump_stmts(stmts, depth + 2, out);
                }
            }
        }
        TypedStmtKind::ExprStmt(e) => dump_expr(e, depth, out),
    }
}

fn dump_pattern(p: &TypedPattern, depth: usize, out: &mut String) {
    match &p.kind {
        TypedPatternKind::Wildcard => push_line(out, depth, &format!("Wildcard ty={}", ty(&p.ty))),
        TypedPatternKind::Literal(e) => {
            push_line(out, depth, &format!("PatternLiteral ty={}", ty(&p.ty)));
            dump_expr(e.as_ref(), depth + 1, out);
        }
        TypedPatternKind::Binding(name) => {
            push_line(out, depth, &format!("Binding name={name} ty={}", ty(&p.ty)))
        }
        TypedPatternKind::Take(inner) => {
            push_line(out, depth, &format!("TakePattern ty={}", ty(&p.ty)));
            dump_pattern(inner, depth + 1, out);
        }
        TypedPatternKind::Variant {
            enum_name,
            variant,
            payload,
        } => {
            push_line(
                out,
                depth,
                &format!(
                    "VariantPattern enum={enum_name} variant={variant} ty={}",
                    ty(&p.ty)
                ),
            );
            for pat in payload {
                dump_pattern(pat, depth + 1, out);
            }
        }
        TypedPatternKind::Tuple(elems) => {
            push_line(out, depth, &format!("TuplePattern ty={}", ty(&p.ty)));
            for e in elems {
                dump_pattern(e, depth + 1, out);
            }
        }
        TypedPatternKind::Array(elems) => {
            push_line(out, depth, &format!("ArrayPattern ty={}", ty(&p.ty)));
            for e in elems {
                dump_pattern(e, depth + 1, out);
            }
        }
        TypedPatternKind::Or(alts) => {
            push_line(out, depth, &format!("OrPattern ty={}", ty(&p.ty)));
            for a in alts {
                dump_pattern(a, depth + 1, out);
            }
        }
    }
}

fn dump_call_args(
    args: &[Option<TypedExpr>],
    ty_for_default: &Type,
    depth: usize,
    out: &mut String,
) {
    for a in args {
        match a {
            Some(e) => dump_expr(e, depth, out),
            None => push_line(out, depth, &format!("DefaultArg ty={}", ty(ty_for_default))),
        }
    }
}

fn dump_expr(e: &TypedExpr, depth: usize, out: &mut String) {
    let t = ty(&e.ty);
    match &e.kind {
        TypedExprKind::Int(text) => push_line(out, depth, &format!("Int text={text} ty={t}")),
        TypedExprKind::Float(text) => push_line(out, depth, &format!("Float text={text} ty={t}")),
        TypedExprKind::Str(text) => push_line(out, depth, &format!("Str text={text} ty={t}")),
        TypedExprKind::BStr(text) => push_line(out, depth, &format!("BStr text={text} ty={t}")),
        TypedExprKind::Char(text) => push_line(out, depth, &format!("Char text={text} ty={t}")),
        TypedExprKind::Bool(v) => push_line(out, depth, &format!("Bool value={v} ty={t}")),
        TypedExprKind::Unit => push_line(out, depth, &format!("Unit ty={t}")),
        TypedExprKind::Local(name) => push_line(out, depth, &format!("Local name={name} ty={t}")),
        TypedExprKind::Const(name) => push_line(out, depth, &format!("Const name={name} ty={t}")),
        TypedExprKind::FnRef(key) => {
            push_line(out, depth, &format!("FnRef key={} ty={t}", key.spelling()))
        }
        TypedExprKind::Field(base, name) => {
            push_line(out, depth, &format!("Field name={name} ty={t}"));
            dump_expr(base, depth + 1, out);
        }
        TypedExprKind::Index(base, idx) => {
            push_line(out, depth, &format!("Index ty={t}"));
            dump_expr(base, depth + 1, out);
            dump_expr(idx, depth + 1, out);
        }
        TypedExprKind::Call {
            callee,
            receiver,
            args,
        } => {
            push_line(
                out,
                depth,
                &format!("Call key={} ty={t}", callee.spelling()),
            );
            if let Some(r) = receiver {
                push_line(out, depth + 1, "Receiver");
                dump_expr(r, depth + 2, out);
            }
            // A defaulted slot's own declared type is unknown at this
            // dump site by design (the default lives on the callee's own
            // declaration, not here) — `unit` is printed as a harmless,
            // deterministic placeholder; the real type is on
            // `TypedParam::default`'s own node wherever that fn/method is
            // itself dumped.
            dump_call_args(args, &Type::Unit, depth + 1, out);
        }
        TypedExprKind::CallValue(callee, args) => {
            push_line(out, depth, &format!("CallValue ty={t}"));
            push_line(out, depth + 1, "Callee");
            dump_expr(callee, depth + 2, out);
            for a in args {
                dump_expr(a, depth + 1, out);
            }
        }
        TypedExprKind::ToScalar(inner) => {
            push_line(out, depth, &format!("ToScalar ty={t}"));
            dump_expr(inner, depth + 1, out);
        }
        TypedExprKind::Neg(inner) => {
            push_line(out, depth, &format!("Neg ty={t}"));
            dump_expr(inner, depth + 1, out);
        }
        TypedExprKind::BitNot(inner) => {
            push_line(out, depth, &format!("BitNot ty={t}"));
            dump_expr(inner, depth + 1, out);
        }
        TypedExprKind::Take(inner) => {
            push_line(out, depth, &format!("Take ty={t}"));
            dump_expr(inner, depth + 1, out);
        }
        TypedExprKind::Try(inner, conv) => {
            let mut header = format!("Try ty={t}");
            if let Some(key) = conv {
                header.push_str(&format!(" conv={}", key.spelling()));
            }
            push_line(out, depth, &header);
            dump_expr(inner, depth + 1, out);
        }
        TypedExprKind::Binary(op, l, r) => {
            push_line(out, depth, &format!("Binary op={} ty={t}", op.as_str()));
            dump_expr(l, depth + 1, out);
            dump_expr(r, depth + 1, out);
        }
        TypedExprKind::OpCall(key, l, r) => {
            push_line(out, depth, &format!("OpCall key={} ty={t}", key.spelling()));
            dump_expr(l, depth + 1, out);
            dump_expr(r, depth + 1, out);
        }
        TypedExprKind::Is(inner, pat) => {
            push_line(out, depth, &format!("Is ty={t}"));
            dump_expr(inner, depth + 1, out);
            dump_pattern(pat.as_ref(), depth + 1, out);
        }
        TypedExprKind::Not(inner) => {
            push_line(out, depth, &format!("Not ty={t}"));
            dump_expr(inner, depth + 1, out);
        }
        TypedExprKind::And(l, r) => {
            push_line(out, depth, &format!("And ty={t}"));
            dump_expr(l, depth + 1, out);
            dump_expr(r, depth + 1, out);
        }
        TypedExprKind::Or(l, r) => {
            push_line(out, depth, &format!("Or ty={t}"));
            dump_expr(l, depth + 1, out);
            dump_expr(r, depth + 1, out);
        }
        TypedExprKind::EnumConstruct {
            enum_name,
            variant,
            args,
        } => {
            push_line(
                out,
                depth,
                &format!("EnumConstruct enum={enum_name} variant={variant} ty={t}"),
            );
            for a in args {
                dump_expr(a, depth + 1, out);
            }
        }
        TypedExprKind::Closure { params, body } => {
            push_line(out, depth, &format!("Closure ty={t}"));
            for p in params {
                push_line(
                    out,
                    depth + 1,
                    &format!(
                        "ClosureParam name={} mode={} ty={}",
                        p.name,
                        p.mode.as_str(),
                        ty(&p.ty)
                    ),
                );
            }
            match body {
                TypedClosureBody::Expr(e) => dump_expr(e, depth + 1, out),
                TypedClosureBody::Suite(stmts) => {
                    push_line(out, depth + 1, "Body");
                    dump_stmts(stmts, depth + 2, out);
                }
            }
        }
        TypedExprKind::Tuple(items) => {
            push_line(out, depth, &format!("Tuple ty={t}"));
            for i in items {
                dump_expr(i, depth + 1, out);
            }
        }
        TypedExprKind::List(items) => {
            push_line(out, depth, &format!("List ty={t}"));
            for i in items {
                dump_expr(i, depth + 1, out);
            }
        }
        TypedExprKind::StructLiteral { name, fields } => {
            push_line(out, depth, &format!("StructLiteral name={name} ty={t}"));
            for (fname, fval) in fields {
                push_line(out, depth + 1, &format!("Field name={fname}"));
                dump_expr(fval, depth + 2, out);
            }
        }
        TypedExprKind::Panic(msg) => {
            push_line(out, depth, &format!("Panic ty={t}"));
            dump_expr(msg, depth + 1, out);
        }
    }
}
