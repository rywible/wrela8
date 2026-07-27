//! Structural generic instantiation (plans/M2.md item H): typing a
//! `Generic[Args]` use enqueues the concrete instantiation (keyed in a
//! `BTreeMap` by a canonical `"<kind>:<name>[<args>]"` string — decision
//! 1's "BTreeSet-keyed by name + resolved args", realized this way so
//! `Type`/`Expr` never need an `Ord` impl of their own; see
//! `bodies.rs`'s `InstKind`/`QueuedInstantiation`/`ModuleCtx::generics_queue`
//! doc comments for the queue itself and the "instantiated at" chain
//! mechanism). Each unique instantiation is checked exactly once: this
//! file builds a substituted copy of the declaration (`Subst`,
//! `subst_*` below — decision 4's "clone freely", applied to `Type`
//! trees instead of source text) and feeds it back through the *same*
//! per-declaration check functions `bodies.rs`/`access.rs`/`matches.rs`
//! already run for a non-generic declaration (each widened `pub(crate)`
//! for exactly this reuse — decision 10's minimal-footprint rule, no
//! restructuring).
//!
//! A missing requirement discovered while checking an instantiation
//! reports the multi-line chain diagnostic pinned verbatim in
//! 02-language.md §7.3 (decision 2) — see `finalize_diagnostic` below for
//! the exact, deliberately narrow shape this recognizes (documented
//! there, and in the session report).
//!
//! Const arguments (02-language.md §6.3, decision 4) evaluate only as a
//! literal, a `const` name (recursively, to another literal/`const`/
//! variant path), or a fieldless-enum variant path — `eval_const_expr`
//! below; anything else fails closed via the existing `unimplemented_at`
//! helper.
//!
//! Method-owned generic parameters (plans/M13.md item Q / 02 §8.3): a
//! method's (or associated fn's) own `[T, const N]` list instantiates
//! under the same worklist, keyed `method:{ReceiverType}.{name}[{args}]`.
//! Type args are inferred at the call site exactly as for free functions
//! — including `R` from a `fn(...) -> R` argument when a closure or named
//! fn is passed — then the substituted body is re-checked and the
//! monomorphized `TypedFn` lands in `TypedProgram::instantiations`.

use std::collections::{BTreeMap, BTreeSet};

use crate::sema::bodies::{self, FnInfo, InstKind, ModuleCtx, QueuedInstantiation, StructInfo};
use crate::sema::typed::{TypedEnum, TypedInstantiation};
use crate::sema::types::{
    self, Classification, DeclEnum, DeclField, DeclFn, DeclGenericKind, DeclGenericParam,
    DeclMember, DeclParam, DeclStruct, DeclVariant, DeclVariantPayload, Type, TypeArg,
};
use crate::sema::{SemaError, access, flow, matches, unimplemented_at};
use crate::syntax::ast::{self, Arg, BinOp, ClosureBody, Expr, Member, Module, Span, Stmt};
use crate::syntax::printer;

// --- canonical keys ---------------------------------------------------

/// `"<kind>:<name>[<args>]"`, span-insensitive (`types::render_type_arg`
/// already ignores spans for the shapes M2 supports — same reasoning as
/// `bodies::types_eq`'s own `same_len_expr`). This is both the
/// `BTreeMap` key `bodies::enqueue_instantiation` uses and the display
/// spelling the chain diagnostic cites (`` `hash_pair[Sector]` ``).
pub(crate) fn canonical_key(kind: InstKind, name: &str, args: &[TypeArg]) -> String {
    debug_assert_ne!(
        kind,
        InstKind::Method,
        "method keys use canonical_method_key"
    );
    format!("{}:{}", kind.tag(), display_name(name, args))
}

/// `method:{ReceiverType}.{method}[{args}]` — plans/M13.md item Q's
/// `(receiver type, method, type-args)` key.
pub(crate) fn canonical_method_key(receiver: &Type, method: &str, args: &[TypeArg]) -> String {
    format!(
        "method:{}.{}",
        types::render_type(receiver),
        display_name(method, args)
    )
}

fn display_name(name: &str, args: &[TypeArg]) -> String {
    if args.is_empty() {
        return name.to_string();
    }
    let rendered: Vec<String> = args.iter().map(types::render_type_arg).collect();
    format!("{name}[{}]", rendered.join(", "))
}

/// Display spelling used by the requirement-chain diagnostic
/// (`Table.entry[Sector]`, `hash_pair[Sector]`).
fn display_inst_name(entry: &QueuedInstantiation) -> String {
    match (&entry.kind, &entry.receiver) {
        (InstKind::Method, Some(recv)) => {
            format!(
                "{}.{}",
                types::render_type(recv),
                display_name(&entry.name, &entry.args)
            )
        }
        _ => display_name(&entry.name, &entry.args),
    }
}

// --- substitution: Type::Generic(name) -> concrete Type ----------------

#[derive(Debug, Clone, Default)]
struct Subst {
    types: BTreeMap<String, Type>,
    consts: BTreeMap<String, Expr>,
}

fn subst_type(ty: &Type, subst: &Subst) -> Type {
    match ty {
        Type::Generic(name) => subst.types.get(name).cloned().unwrap_or_else(|| ty.clone()),
        Type::Array(elem, len) => Type::Array(
            Box::new(subst_type(elem, subst)),
            Box::new(subst_expr(len, subst)),
        ),
        Type::Tuple(elems) => Type::Tuple(elems.iter().map(|t| subst_type(t, subst)).collect()),
        Type::Option(inner) => Type::Option(Box::new(subst_type(inner, subst))),
        Type::Result(ok, err) => Type::Result(
            Box::new(subst_type(ok, subst)),
            Box::new(subst_type(err, subst)),
        ),
        Type::Own(pool, inner) => Type::Own(pool.clone(), Box::new(subst_type(inner, subst))),
        Type::Static(inner) => Type::Static(Box::new(subst_type(inner, subst))),
        Type::Bytes(Some(len)) => Type::Bytes(Some(Box::new(subst_expr(len, subst)))),
        Type::Bytes(None) => Type::Bytes(None),
        Type::String(len) => Type::String(Box::new(subst_expr(len, subst))),
        Type::Fn(params, ret) => Type::Fn(
            params
                .iter()
                .map(|(m, t)| (*m, subst_type(t, subst)))
                .collect(),
            Box::new(subst_type(ret, subst)),
        ),
        Type::Named(name, targs) => Type::Named(
            name.clone(),
            targs.iter().map(|a| subst_type_arg(a, subst)).collect(),
        ),
        other => other.clone(),
    }
}

fn subst_type_arg(arg: &TypeArg, subst: &Subst) -> TypeArg {
    match arg {
        TypeArg::Type(t) => TypeArg::Type(subst_type(t, subst)),
        TypeArg::Const(e) => TypeArg::Const(subst_expr(e, subst)),
        TypeArg::Bound(e) => TypeArg::Bound(subst_expr(e, subst)),
        // A pool name (plans/M7.md item D) is a `pool Name` declaration,
        // never a generic parameter, so nothing substitutes into it.
        TypeArg::Pool(p) => TypeArg::Pool(p.clone()),
    }
}

/// Rewrite const-generic names to their concrete argument expressions.
/// Length/type positions only need a bare-name rewrite; method bodies
/// (plans/M9.md item F1 decision 342) need a deep walk so `return N` /
/// `[None; N]` / `self.len >= N` see the concrete value.
fn subst_expr(e: &Expr, subst: &Subst) -> Expr {
    match e {
        Expr::Name(_, name) => subst.consts.get(name).cloned().unwrap_or_else(|| e.clone()),
        Expr::Field(base, span, name) => {
            Expr::Field(Box::new(subst_expr(base, subst)), *span, name.clone())
        }
        Expr::Index(base, span, args) => Expr::Index(
            Box::new(subst_expr(base, subst)),
            *span,
            args.iter().map(|a| subst_expr(a, subst)).collect(),
        ),
        Expr::Call(callee, span, args) => Expr::Call(
            Box::new(subst_expr(callee, subst)),
            *span,
            args.iter()
                .map(|a| Arg {
                    span: a.span,
                    label: a.label.clone(),
                    mode: a.mode,
                    value: subst_expr(&a.value, subst),
                })
                .collect(),
        ),
        Expr::Unary(span, op, inner) => Expr::Unary(*span, *op, Box::new(subst_expr(inner, subst))),
        Expr::Try(span, inner) => Expr::Try(*span, Box::new(subst_expr(inner, subst))),
        Expr::Binary(span, op, l, r) => Expr::Binary(
            *span,
            *op,
            Box::new(subst_expr(l, subst)),
            Box::new(subst_expr(r, subst)),
        ),
        Expr::Range(span, f, t, incl) => Expr::Range(
            *span,
            Box::new(subst_expr(f, subst)),
            Box::new(subst_expr(t, subst)),
            *incl,
        ),
        Expr::Is(span, inner, pat) => {
            Expr::Is(*span, Box::new(subst_expr(inner, subst)), pat.clone())
        }
        Expr::Not(span, inner) => Expr::Not(*span, Box::new(subst_expr(inner, subst))),
        Expr::And(span, l, r) => Expr::And(
            *span,
            Box::new(subst_expr(l, subst)),
            Box::new(subst_expr(r, subst)),
        ),
        Expr::Or(span, l, r) => Expr::Or(
            *span,
            Box::new(subst_expr(l, subst)),
            Box::new(subst_expr(r, subst)),
        ),
        Expr::DotVariant(span, name, args) => Expr::DotVariant(
            *span,
            name.clone(),
            args.iter()
                .map(|a| Arg {
                    span: a.span,
                    label: a.label.clone(),
                    mode: a.mode,
                    value: subst_expr(&a.value, subst),
                })
                .collect(),
        ),
        Expr::Send(span, inner) => Expr::Send(*span, Box::new(subst_expr(inner, subst))),
        Expr::Tuple(span, items) => {
            Expr::Tuple(*span, items.iter().map(|i| subst_expr(i, subst)).collect())
        }
        Expr::List(span, items) => {
            Expr::List(*span, items.iter().map(|i| subst_expr(i, subst)).collect())
        }
        Expr::ArrayRepeat(span, elem, count) => Expr::ArrayRepeat(
            *span,
            Box::new(subst_expr(elem, subst)),
            Box::new(subst_expr(count, subst)),
        ),
        Expr::Closure(c) => Expr::Closure(crate::syntax::ast::ClosureExpr {
            body: match &c.body {
                ClosureBody::Expr(e) => ClosureBody::Expr(Box::new(subst_expr(e, subst))),
                ClosureBody::Suite(stmts) => {
                    ClosureBody::Suite(stmts.iter().map(|s| subst_stmt(s, subst)).collect())
                }
            },
            ..c.clone()
        }),
        Expr::Int(_, _)
        | Expr::Float(_, _)
        | Expr::Str(_, _)
        | Expr::BStr(_, _)
        | Expr::Char(_, _)
        | Expr::FStr(_)
        | Expr::Bool(_, _)
        | Expr::Unit(_) => e.clone(),
    }
}

fn subst_stmt(s: &Stmt, subst: &Subst) -> Stmt {
    match s {
        Stmt::Assign(a) => Stmt::Assign(crate::syntax::ast::AssignStmt {
            target: subst_expr(&a.target, subst),
            value: subst_expr(&a.value, subst),
            ..a.clone()
        }),
        Stmt::If(i) => Stmt::If(crate::syntax::ast::IfStmt {
            cond: subst_expr(&i.cond, subst),
            then_branch: i.then_branch.iter().map(|s| subst_stmt(s, subst)).collect(),
            elifs: i
                .elifs
                .iter()
                .map(|e| crate::syntax::ast::ElifClause {
                    cond: subst_expr(&e.cond, subst),
                    body: e.body.iter().map(|s| subst_stmt(s, subst)).collect(),
                    ..e.clone()
                })
                .collect(),
            else_branch: i
                .else_branch
                .as_ref()
                .map(|b| b.iter().map(|s| subst_stmt(s, subst)).collect()),
            ..i.clone()
        }),
        Stmt::Match(m) => Stmt::Match(crate::syntax::ast::MatchStmt {
            scrutinee: subst_expr(&m.scrutinee, subst),
            arms: m
                .arms
                .iter()
                .map(|a| crate::syntax::ast::MatchArm {
                    guard: a.guard.as_ref().map(|g| subst_expr(g, subst)),
                    body: a.body.iter().map(|s| subst_stmt(s, subst)).collect(),
                    ..a.clone()
                })
                .collect(),
            ..m.clone()
        }),
        Stmt::For(f) => Stmt::For(crate::syntax::ast::ForStmt {
            iterable: subst_expr(&f.iterable, subst),
            body: f.body.iter().map(|s| subst_stmt(s, subst)).collect(),
            ..f.clone()
        }),
        Stmt::While(w) => Stmt::While(crate::syntax::ast::WhileStmt {
            cond: subst_expr(&w.cond, subst),
            body: w.body.iter().map(|s| subst_stmt(s, subst)).collect(),
            ..w.clone()
        }),
        Stmt::Return(span, Some(e)) => Stmt::Return(*span, Some(subst_expr(e, subst))),
        Stmt::Assert(a) => Stmt::Assert(crate::syntax::ast::AssertStmt {
            cond: subst_expr(&a.cond, subst),
            message: a.message.as_ref().map(|m| subst_expr(m, subst)),
            ..a.clone()
        }),
        Stmt::Defer(d) => Stmt::Defer(crate::syntax::ast::DeferStmt {
            body: match &d.body {
                crate::syntax::ast::DeferBody::Expr(e) => {
                    crate::syntax::ast::DeferBody::Expr(Box::new(subst_expr(e, subst)))
                }
                crate::syntax::ast::DeferBody::Suite(stmts) => {
                    crate::syntax::ast::DeferBody::Suite(
                        stmts.iter().map(|s| subst_stmt(s, subst)).collect(),
                    )
                }
            },
            ..d.clone()
        }),
        Stmt::With(w) => Stmt::With(crate::syntax::ast::WithStmt {
            expr: subst_expr(&w.expr, subst),
            body: w.body.iter().map(|s| subst_stmt(s, subst)).collect(),
            ..w.clone()
        }),
        Stmt::Send(span, e) => Stmt::Send(*span, subst_expr(e, subst)),
        Stmt::Expr(span, e) => Stmt::Expr(*span, subst_expr(e, subst)),
        Stmt::ComptimeAssert(span, cond, msg) => Stmt::ComptimeAssert(
            *span,
            subst_expr(cond, subst),
            msg.as_ref().map(|m| subst_expr(m, subst)),
        ),
        Stmt::Break(_)
        | Stmt::Continue(_)
        | Stmt::Return(_, None)
        | Stmt::Pass(_)
        | Stmt::ComptimeIf(_) => s.clone(),
    }
}

fn subst_member_ast(m: &Member, subst: &Subst) -> Member {
    match m {
        Member::Fn(f) => Member::Fn(crate::syntax::ast::FnItem {
            body: f
                .body
                .as_ref()
                .map(|b| b.iter().map(|s| subst_stmt(s, subst)).collect()),
            ..f.clone()
        }),
        Member::Init(i) => Member::Init(crate::syntax::ast::InitItem {
            body: i.body.iter().map(|s| subst_stmt(s, subst)).collect(),
            ..i.clone()
        }),
        Member::Field(f) => Member::Field(crate::syntax::ast::FieldItem {
            default: f.default.as_ref().map(|d| subst_expr(d, subst)),
            ..f.clone()
        }),
        Member::Pool(_) | Member::ComptimeIf(_) => m.clone(),
    }
}

/// `pub(crate)` reuse for `flow.rs`'s own best-effort place typing
/// (decision 10's minimal-footprint rule, the same pattern the rest of
/// this file's `pub(crate)` surface follows): substitutes a single
/// unsubstituted field type using a struct's own generics zipped against
/// a use site's concrete type arguments — a type-only `Subst` (no consts;
/// a field's own type never needs one, only an array/`Bytes` *length*
/// might, and `place_type`'s callers only ever ask "is this field's type
/// a resource", never its size) so a resource classification through an
/// instantiated generic struct's field (`Box[DmaBlock].item`, not just a
/// bare generic parameter) is answered correctly instead of silently
/// falling back to "unknown" the moment a use site's type arguments are
/// non-empty.
pub(crate) fn subst_field_type(
    field_ty: &Type,
    generics: &[DeclGenericParam],
    targs: &[TypeArg],
) -> Type {
    let mut types = BTreeMap::new();
    for (g, a) in generics.iter().zip(targs.iter()) {
        if let TypeArg::Type(t) = a {
            types.insert(g.name.clone(), t.clone());
        }
    }
    let subst = Subst {
        types,
        consts: BTreeMap::new(),
    };
    subst_type(field_ty, &subst)
}

// --- const argument evaluation (plans/M3.md item B) ---------------------
//
// M2-H's own literal/const-name/fieldless-variant-only subset (decision
// 4 of that plan) is replaced here by the real evaluator: a const
// generic argument may now be any comptime-evaluable expression
// (arithmetic, a reference to another `const`, a fieldless-enum
// variant) — `eval::interp::eval_standalone` is the same tree-walking
// interpreter `eval::check_consts` runs a plain module-level `const`
// through. It is *not* threaded the fully assembled `TypedProgram`
// (`mod.rs::check_typed`'s own `program` local): half of this file's
// callers (`bodies.rs`'s own call/construction checking) run *during*
// that program's own assembly, before it exists — so a const-generic
// argument's evaluation is scoped to what a bare `const`'s own
// initializer can already see standalone: `mctx`'s plain top-level
// `const`s, freshly type-checked here into a small ad-hoc consts-only
// `TypedProgram` (`build_consts_program` below). A const argument
// expression that calls a plain `fn` is therefore not yet supported
// (documented scope limit, not silently approximated: the evaluator
// itself fails closed, naming the missing callee, exactly like any
// other unimplemented construct) — 02-language.md §6.3's own value
// vocabulary (`bool`/`char`/an integer/a fieldless enum) never needed a
// function call to produce one in the first place.

/// A throwaway `TypedProgram` carrying only `mctx`'s plain top-level
/// `const`s (freshly type-checked, independent of whatever point in the
/// main `bodies::check` pass this file's own caller is running from —
/// cheap and safe to rebuild per call, decision 4: dumb, no state
/// threaded across calls). A const whose own initializer does not type-
/// check here is simply omitted — unreachable in practice, since every
/// `const` in a module that passed `mod.rs::check`'s earlier passes
/// already type-checks; this function only ever runs *during* that same
/// module's own checking.
fn build_consts_program(mctx: &ModuleCtx) -> Result<crate::sema::typed::TypedProgram, SemaError> {
    let mut program = crate::sema::typed::TypedProgram::default();
    for (name, ty) in &mctx.consts {
        let Some(raw) = mctx.const_values.get(name) else {
            continue;
        };
        // A const whose initializer contains a generic bracket is outside
        // the vocabulary this throwaway program exists to supply
        // (02-language.md §6.3: a const argument's value is a
        // bool/char/integer/fieldless-enum — never something a generic
        // instantiation had to be built to produce) — and type-checking
        // it here would recurse: `bodies::check_expr` on a generic
        // construction resolves its const arguments through
        // `eval_const_expr`, whose evaluation program is built by this
        // very function, re-checking this very const, forever (a native
        // stack overflow, found by M3-G's adversarial sweep on
        // `const A: Array[10] = Array[10](dummy=10)`). Omitting it is
        // fail-closed, not lossy: a const-arg expression referencing an
        // omitted const gets the evaluator's own "unknown const"
        // diagnostic, while the const itself still checks and evaluates
        // normally in the real pipeline, which owns the instantiations.
        if contains_generic_brackets(raw) {
            continue;
        }
        let mut fctx = bodies::FnCtx::new(Type::Unit, mctx.module_pools.clone());
        if let Ok(value) = bodies::check_expr(raw, Some(ty), &mut fctx, mctx) {
            program.consts.insert(
                name.clone(),
                crate::sema::typed::TypedConst {
                    ty: ty.clone(),
                    value,
                },
            );
        }
    }
    // A fieldless-enum-variant const argument needs its own enum's
    // variant order (`interp::variant_index`) exactly like a plain
    // module-level `const` does (`typed::TypedProgram::enums`'s own doc
    // comment) — a generic enum's is never needed here (constructing a
    // generic enum's variant already fails closed in `bodies.rs` before
    // producing a typed node at all).
    for (name, en) in &mctx.enums {
        if en.generics.is_empty() {
            program.enums.insert(
                name.clone(),
                TypedEnum::from_variants(en.variants.iter().map(|v| v.name.clone()).collect()),
            );
        }
    }
    // plans/M7.md item G / plans/M9.md item I: stdlib enums for
    // const-generic arguments (`DriverMode.Irq`).
    for name in ["Target", "Failure", "DriverMode"] {
        // plans/M9.md item QQ: load failures are `error[build]`.
        if let Some(vs) = crate::sema::stdlib_enums::variant_strs(name)? {
            program.enums.entry(name.to_string()).or_insert_with(|| {
                TypedEnum::from_variants(vs.iter().map(|v| v.to_string()).collect())
            });
        }
    }
    Ok(program)
}

/// True when an expression contains any `Expr::Index` node anywhere —
/// the bracket shape a generic construction (`Name[Args](...)`) or a
/// plain array index both wear (the parser cannot tell them apart —
/// `ast::Expr::Index`'s own doc comment), used by
/// `build_consts_program` above to keep its consts-only vocabulary from
/// re-entering generic-argument resolution. Deliberately conservative,
/// same shape as `specialize::collect_names_in_expr`: a suite-bodied
/// closure is treated as containing one (its statements are not
/// scanned), and an innocent array index excludes its const too — being
/// too generous here only ever omits a const from the throwaway
/// program, which the evaluator reports as an unknown const (fail
/// closed), never a wrong answer.
fn contains_generic_brackets(e: &Expr) -> bool {
    match e {
        Expr::Index(..) => true,
        Expr::Name(..)
        | Expr::Int(..)
        | Expr::Float(..)
        | Expr::Str(..)
        | Expr::BStr(..)
        | Expr::Char(..)
        | Expr::Bool(..)
        | Expr::Unit(..)
        | Expr::FStr(_) => false,
        Expr::Field(base, _, _) => contains_generic_brackets(base),
        Expr::Call(callee, _, args) => {
            contains_generic_brackets(callee)
                || args.iter().any(|a| contains_generic_brackets(&a.value))
        }
        Expr::Unary(_, _, inner) | Expr::Try(_, inner) | Expr::Not(_, inner) => {
            contains_generic_brackets(inner)
        }
        Expr::Binary(_, _, l, r) | Expr::And(_, l, r) | Expr::Or(_, l, r) => {
            contains_generic_brackets(l) || contains_generic_brackets(r)
        }
        Expr::Range(_, a, b, _) => contains_generic_brackets(a) || contains_generic_brackets(b),
        Expr::Is(_, scrutinee, _) => contains_generic_brackets(scrutinee),
        Expr::DotVariant(_, _, args) => args.iter().any(|a| contains_generic_brackets(&a.value)),
        Expr::Closure(c) => match &c.body {
            ClosureBody::Expr(e) => contains_generic_brackets(e),
            ClosureBody::Suite(_) => true,
        },
        Expr::Send(_, inner) => contains_generic_brackets(inner),
        Expr::Tuple(_, items) | Expr::List(_, items) => items.iter().any(contains_generic_brackets),
        Expr::ArrayRepeat(_, elem, count) => {
            contains_generic_brackets(elem) || contains_generic_brackets(count)
        }
    }
}

/// Encodes a decoded char back into lexable char-literal token text
/// (quotes included — `ConstVal`'s old doc comment's own "raw lexed char
/// text" shape, unchanged by item B): the common printable-ASCII case
/// spells itself; `'`/`\`/the fixed named escapes/anything else use the
/// matching escape (`\u{...}` covers every remaining codepoint, always
/// lexable).
fn encode_char_literal(ch: char) -> String {
    match ch {
        '\'' => "'\\''".to_string(),
        '\\' => "'\\\\'".to_string(),
        '\n' => "'\\n'".to_string(),
        '\r' => "'\\r'".to_string(),
        '\t' => "'\\t'".to_string(),
        '\0' => "'\\0'".to_string(),
        c if !c.is_control() => format!("'{c}'"),
        c => format!("'\\u{{{:x}}}'", c as u32),
    }
}

/// The `Value` a const-generic argument's real evaluation produced,
/// translated back into the `Expr` shape `subst.consts` still carries
/// (decision 1's array-length/const-parameter substitution keeps
/// substituted `const`s as `Expr`s — `subst_expr` rewrites a bare
/// `Expr::Name` to one of these, unchanged by item B): 02-language.md
/// §6.3 bounds the *value* a const argument may hold to `bool`/`char`/an
/// integer/a fieldless enum, regardless of how comptime-rich the
/// expression that computed it was.
fn value_to_const_arg_expr(
    v: &crate::eval::Value,
    enum_name: Option<&str>,
    mctx: &ModuleCtx,
    span: Span,
) -> Result<Expr, SemaError> {
    use crate::eval::value;
    match v {
        crate::eval::Value::Bool(b) => Ok(Expr::Bool(span, *b)),
        crate::eval::Value::Char(c) => Ok(Expr::Char(span, encode_char_literal(*c))),
        crate::eval::Value::Enum(idx, payload) if payload.is_empty() => {
            let Some(enum_name) = enum_name else {
                return Err(unimplemented_at("this const generic argument is", span));
            };
            let variant = match enum_name {
                "Option" => match *idx {
                    value::OPTION_NONE => "None".to_string(),
                    value::OPTION_SOME => "Some".to_string(),
                    _ => return Err(unimplemented_at("this const generic argument is", span)),
                },
                "Result" => match *idx {
                    value::RESULT_OK => "Ok".to_string(),
                    value::RESULT_ERR => "Err".to_string(),
                    _ => return Err(unimplemented_at("this const generic argument is", span)),
                },
                _ => {
                    // plans/M7.md item G / plans/M9.md item I: stdlib
                    // enums (`DriverMode`) are not in `mctx.enums`.
                    // plans/M9.md item QQ: load failures are `error[build]`.
                    if let Some(vs) = crate::sema::stdlib_enums::variant_strs(enum_name)? {
                        vs.get(*idx).map(|v| v.to_string()).ok_or_else(|| {
                            unimplemented_at("this const generic argument is", span)
                        })?
                    } else {
                        let Some(en) = mctx.enums.get(enum_name) else {
                            return Err(unimplemented_at("this const generic argument is", span));
                        };
                        let Some(dv) = en.variants.get(*idx) else {
                            return Err(unimplemented_at("this const generic argument is", span));
                        };
                        dv.name.clone()
                    }
                }
            };
            Ok(Expr::Field(
                Box::new(Expr::Name(span, enum_name.to_string())),
                span,
                variant,
            ))
        }
        other => value::as_i128(other)
            .map(|n| Expr::Int(span, n.to_string()))
            .ok_or_else(|| unimplemented_at("this const generic argument is", span)),
    }
}

fn eval_const_expr(e: &Expr, expected: Option<&Type>, mctx: &ModuleCtx) -> Result<Expr, SemaError> {
    let span = e.span();
    let mut fctx = bodies::FnCtx::new(Type::Unit, mctx.module_pools.clone());
    let typed = bodies::check_expr(e, expected, &mut fctx, mctx)?;
    let enum_name = match &typed.ty {
        Type::Named(name, _) => Some(name.clone()),
        _ => None,
    };
    let program = build_consts_program(mctx)?;
    let value =
        crate::eval::interp::eval_standalone(&program, &typed, "<generic argument>".to_string())
            .map_err(crate::eval::to_sema_error)?;
    value_to_const_arg_expr(&value, enum_name.as_deref(), mctx, span)
}

// --- resolving raw call-site brackets into `TypeArg`s -------------------

/// `Name[Args](...)`'s `Args` are raw `Expr`s (the parser cannot tell
/// indexing from a generic bracket — `ast::Expr::Index`'s own doc
/// comment) — this is the call-site mirror of `types::resolve_type_arg`
/// (which runs on `ast::GenericArg`, the *annotation*-position grammar).
/// A bare name is a scalar/struct/enum type if it names one, else a
/// const-shaped expression (evaluated later, lazily, by `eval_const_expr`
/// — resolving it eagerly here would reject a legal `const` name before
/// even trying, since a name alone can't be told apart from a type name
/// syntactically past this point either). No nested generic arguments
/// (`Ring[Foo[Bar], 4]`) — that call shape fails closed at its own
/// `unimplemented_at` in `resolve_call_type_arg`'s fallthrough.
pub(crate) fn resolve_call_targs(
    targs: &[Expr],
    mctx: &ModuleCtx,
) -> Result<Vec<TypeArg>, SemaError> {
    targs
        .iter()
        .map(|e| resolve_call_type_arg(e, mctx))
        .collect()
}

fn resolve_call_type_arg(e: &Expr, mctx: &ModuleCtx) -> Result<TypeArg, SemaError> {
    match e {
        Expr::Name(_, name) => {
            if let Some(t) = bodies::scalar_type_by_name(name) {
                return Ok(TypeArg::Type(t));
            }
            if mctx.structs.contains_key(name) || mctx.enums.contains_key(name) {
                return Ok(TypeArg::Type(Type::Named(name.clone(), vec![])));
            }
            // Not a known type name: a const-shaped argument (a `const`
            // item's own name, most commonly) — left unevaluated here,
            // `eval_const_expr` resolves it when the argument actually
            // binds to a const parameter.
            Ok(TypeArg::Const(e.clone()))
        }
        Expr::Int(..) | Expr::Bool(..) | Expr::Char(..) | Expr::Field(..) => {
            Ok(TypeArg::Const(e.clone()))
        }
        other => Err(unimplemented_at(
            "this generic argument shape is",
            other.span(),
        )),
    }
}

// --- building a `Subst` from a declaration's generics + resolved args ---

fn check_arity(
    generics: &[DeclGenericParam],
    args: &[TypeArg],
    name: &str,
    call_span: Span,
) -> Result<(), SemaError> {
    if generics.len() != args.len() {
        return Err(SemaError::at(
            "type",
            format!(
                "`{name}` expects {} generic argument(s), found {}",
                generics.len(),
                args.len()
            ),
            call_span,
        ));
    }
    Ok(())
}

fn build_subst(
    generics: &[DeclGenericParam],
    args: &[TypeArg],
    mctx: &ModuleCtx,
    call_span: Span,
) -> Result<Subst, SemaError> {
    let mut subst = Subst::default();
    for (g, a) in generics.iter().zip(args.iter()) {
        match (&g.kind, a) {
            (DeclGenericKind::Type, TypeArg::Type(t)) => {
                subst.types.insert(g.name.clone(), t.clone());
            }
            (DeclGenericKind::Const(cty), TypeArg::Const(e))
            // plans/M9.md item F1 decision 341: `..N` is occupancy
            // spelling of the same const argument.
            | (DeclGenericKind::Const(cty), TypeArg::Bound(e)) => {
                let v = eval_const_expr(e, Some(cty), mctx)?;
                subst.consts.insert(g.name.clone(), v);
            }
            (DeclGenericKind::Type, _) => {
                return Err(SemaError::at(
                    "type",
                    format!("generic parameter `{}` requires a type argument", g.name),
                    call_span,
                ));
            }
            (DeclGenericKind::Const(_), _) => {
                return Err(SemaError::at(
                    "type",
                    format!("generic parameter `{}` requires a const argument", g.name),
                    call_span,
                ));
            }
        }
    }
    Ok(subst)
}

// --- substituting a declaration's members --------------------------------

fn subst_decl_field(f: &DeclField, subst: &Subst) -> DeclField {
    DeclField {
        name: f.name.clone(),
        ty: subst_type(&f.ty, subst),
        is_pub: f.is_pub,
    }
}

fn subst_decl_param(p: &DeclParam, subst: &Subst) -> DeclParam {
    DeclParam {
        mode: p.mode,
        name: p.name.clone(),
        ty: subst_type(&p.ty, subst),
    }
}

/// Substitutes a *member* fn/method/init's signature using the
/// *enclosing struct's* substitution: its own generics (if it happens to
/// have any, beyond the struct's — a "generic method", item H's own
/// documented scope boundary) are left exactly as declared, since they
/// are never themselves instantiated.
fn subst_decl_fn_member(f: &DeclFn, subst: &Subst) -> DeclFn {
    DeclFn {
        name: f.name.clone(),
        is_async: f.is_async,
        is_task: f.is_task,
        generics: f.generics.clone(),
        receiver: f.receiver.clone(),
        params: f
            .params
            .iter()
            .map(|p| subst_decl_param(p, subst))
            .collect(),
        ret: subst_type(&f.ret, subst),
    }
}

/// Substitutes a *directly instantiated* fn's own signature, clearing its
/// generics list — this is the one place a `DeclFn` actually stops being
/// generic (`bodies::check_top_fn`'s guard is `f.generics.is_empty()` on
/// the *ast*, but every check function downstream reads types off `d`
/// alone, so clearing this is what actually makes the substituted
/// declaration behave like a real concrete one).
fn subst_decl_fn_direct(f: &DeclFn, subst: &Subst) -> DeclFn {
    DeclFn {
        name: f.name.clone(),
        is_async: f.is_async,
        is_task: f.is_task,
        generics: Vec::new(),
        receiver: f.receiver.clone(),
        params: f
            .params
            .iter()
            .map(|p| subst_decl_param(p, subst))
            .collect(),
        ret: subst_type(&f.ret, subst),
    }
}

fn subst_decl_member(m: &DeclMember, subst: &Subst) -> DeclMember {
    match m {
        DeclMember::Field(f) => DeclMember::Field(subst_decl_field(f, subst)),
        DeclMember::Fn(f) => DeclMember::Fn(subst_decl_fn_member(f, subst)),
        DeclMember::Init(f) => DeclMember::Init(subst_decl_fn_member(f, subst)),
        DeclMember::Pool(p) => DeclMember::Pool(p.clone()),
    }
}

/// Reclassifies a substituted struct/enum (decision 4's "a struct
/// generic over T containing a T is a resource iff the argument is"):
/// `bodies::is_resource_type` mirrors `types::classify_type` exactly and
/// already resolves a `Type::Named` component through `mctx`'s
/// (unsubstituted, but structurally identical for this purpose — a
/// *named* component's own classification never depends on the
/// *outer* instantiation's args) struct/enum tables.
fn reclassify(
    is_resource_fiat: bool,
    component_types: &[(Type, Span)],
    mctx: &ModuleCtx,
) -> Classification {
    let resource = is_resource_fiat
        || component_types
            .iter()
            .any(|(t, _)| bodies::is_resource_type(t, mctx));
    if resource {
        Classification::Resource
    } else {
        Classification::Data
    }
}

fn subst_decl_struct(d: &DeclStruct, subst: &Subst, mctx: &ModuleCtx) -> DeclStruct {
    let members: Vec<DeclMember> = d
        .members
        .iter()
        .map(|m| subst_decl_member(m, subst))
        .collect();
    let component_types: Vec<(Type, Span)> = d
        .component_types
        .iter()
        .map(|(t, sp)| (subst_type(t, subst), *sp))
        .collect();
    let classification = reclassify(d.is_resource_fiat, &component_types, mctx);
    DeclStruct {
        name: d.name.clone(),
        generics: Vec::new(),
        deriving: d.deriving.clone(),
        classification,
        members,
        is_resource_fiat: d.is_resource_fiat,
        is_actor: d.is_actor,
        is_driver: d.is_driver,
        layout_kind: d.layout_kind,
        component_types,
        span: d.span,
    }
}

fn subst_variant_payload(p: &DeclVariantPayload, subst: &Subst) -> DeclVariantPayload {
    match p {
        DeclVariantPayload::None => DeclVariantPayload::None,
        DeclVariantPayload::Tuple(ts) => {
            DeclVariantPayload::Tuple(ts.iter().map(|t| subst_type(t, subst)).collect())
        }
        DeclVariantPayload::Named(fs) => DeclVariantPayload::Named(
            fs.iter()
                .map(|(n, t)| (n.clone(), subst_type(t, subst)))
                .collect(),
        ),
    }
}

fn subst_decl_enum(d: &DeclEnum, subst: &Subst, mctx: &ModuleCtx) -> DeclEnum {
    let variants: Vec<DeclVariant> = d
        .variants
        .iter()
        .map(|v| DeclVariant {
            name: v.name.clone(),
            payload: subst_variant_payload(&v.payload, subst),
        })
        .collect();
    let members: Vec<DeclMember> = d
        .members
        .iter()
        .map(|m| subst_decl_member(m, subst))
        .collect();
    let component_types: Vec<(Type, Span)> = d
        .component_types
        .iter()
        .map(|(t, sp)| (subst_type(t, subst), *sp))
        .collect();
    let classification = reclassify(false, &component_types, mctx);
    DeclEnum {
        name: d.name.clone(),
        generics: Vec::new(),
        deriving: d.deriving.clone(),
        classification,
        variants,
        members,
        component_types,
        span: d.span,
    }
}

// --- instantiation entry points: build + enqueue -------------------------

/// Resolves `name[args]` as a struct instantiation: looks up the
/// *declared* shape, substitutes it, and enqueues it (item H: "typing a
/// `Generic[Args]` use enqueues the concrete instantiation") — every
/// caller (a construction call, a field/method/variant lookup through an
/// already-typed `Type::Named` value, an annotation) routes through this
/// one function, so every use registers exactly once per unique
/// `(name, args)` (the queue's own dedup).
pub(crate) fn instantiate_struct(
    mctx: &ModuleCtx,
    name: &str,
    args: &[TypeArg],
    call_span: Span,
) -> Result<StructInfo, SemaError> {
    let Some(orig) = mctx.structs.get(name) else {
        return Err(SemaError::at(
            "type",
            format!("unknown type `{name}`"),
            call_span,
        ));
    };
    check_arity(&orig.decl.generics, args, name, call_span)?;
    let subst = build_subst(&orig.decl.generics, args, mctx, call_span)?;
    // =====================================================================
    // plans/M7.md item G, decision 18: expand deferred `comptime if`
    // members/stmts under this instantiation's const arguments, then
    // re-declare so Irq-only methods (ISR) exist only on Irq builds.
    // =====================================================================
    let const_subst: BTreeMap<String, Expr> = subst.consts.clone();
    let expanded = crate::sema::specialize::expand_deferred_members(
        &orig.ast_members,
        &orig.deferred_comptime_members,
        &const_subst,
        mctx,
    )?;
    // plans/M9.md item F1 decision 342: deep-substitute const names
    // through method/init bodies so capacity checks and `[None; N]`
    // see the concrete argument.
    let expanded: Vec<Member> = expanded
        .iter()
        .map(|m| subst_member_ast(m, &subst))
        .collect();
    let decl = if orig.deferred_comptime_members.is_empty()
        && !orig
            .ast_members
            .iter()
            .any(|m| member_has_deferred_comptime_stmt(m))
    {
        subst_decl_struct(&orig.decl, &subst, mctx)
    } else {
        let mut decl = types::declare_struct_members_for_instantiation(
            name, &expanded, &orig.decl, mctx, call_span,
        )?;
        // Still run type substitution on field/param types (a type
        // generic on the same struct, if any).
        decl = subst_decl_struct(&decl, &subst, mctx);
        decl
    };
    bodies::enqueue_instantiation(mctx, InstKind::Struct, name, args, call_span)?;
    Ok(StructInfo {
        decl,
        ast_members: expanded,
        deferred_comptime_members: Vec::new(),
    })
}

fn member_has_deferred_comptime_stmt(m: &Member) -> bool {
    match m {
        Member::Fn(f) => f
            .body
            .as_ref()
            .is_some_and(|b| stmts_have_deferred_comptime(b)),
        Member::Init(i) => stmts_have_deferred_comptime(&i.body),
        Member::ComptimeIf(_) => true,
        Member::Field(_) | Member::Pool(_) => false,
    }
}

fn stmts_have_deferred_comptime(stmts: &[crate::syntax::ast::Stmt]) -> bool {
    use crate::syntax::ast::Stmt;
    stmts.iter().any(|s| match s {
        Stmt::ComptimeIf(_) => true,
        Stmt::If(i) => {
            stmts_have_deferred_comptime(&i.then_branch)
                || i.elifs
                    .iter()
                    .any(|e| stmts_have_deferred_comptime(&e.body))
                || i.else_branch
                    .as_ref()
                    .is_some_and(|b| stmts_have_deferred_comptime(b))
        }
        Stmt::Match(m) => m.arms.iter().any(|a| stmts_have_deferred_comptime(&a.body)),
        Stmt::For(f) => stmts_have_deferred_comptime(&f.body),
        Stmt::While(w) => stmts_have_deferred_comptime(&w.body),
        Stmt::Defer(d) => match &d.body {
            crate::syntax::ast::DeferBody::Suite(s) => stmts_have_deferred_comptime(s),
            crate::syntax::ast::DeferBody::Expr(_) => false,
        },
        Stmt::With(w) => stmts_have_deferred_comptime(&w.body),
        _ => false,
    })
}

pub(crate) fn instantiate_enum(
    mctx: &ModuleCtx,
    name: &str,
    args: &[TypeArg],
    call_span: Span,
) -> Result<DeclEnum, SemaError> {
    let Some(orig) = mctx.enums.get(name) else {
        return Err(SemaError::at(
            "type",
            format!("unknown type `{name}`"),
            call_span,
        ));
    };
    check_arity(&orig.generics, args, name, call_span)?;
    let subst = build_subst(&orig.generics, args, mctx, call_span)?;
    let decl = subst_decl_enum(orig, &subst, mctx);
    bodies::enqueue_instantiation(mctx, InstKind::Enum, name, args, call_span)?;
    Ok(decl)
}

pub(crate) fn instantiate_fn(
    mctx: &ModuleCtx,
    name: &str,
    args: &[TypeArg],
    call_span: Span,
) -> Result<FnInfo, SemaError> {
    let Some(orig) = mctx.fns.get(name) else {
        return Err(SemaError::at(
            "type",
            format!("unknown function `{name}`"),
            call_span,
        ));
    };
    check_arity(&orig.decl.generics, args, name, call_span)?;
    let subst = build_subst(&orig.decl.generics, args, mctx, call_span)?;
    let decl = subst_decl_fn_direct(&orig.decl, &subst);
    let mut ast = orig.ast.clone();
    ast.generics = Vec::new();
    // plans/M9.md item F1 decision 342: const names in the body too.
    if let Some(body) = ast.body.as_mut() {
        *body = body.iter().map(|s| subst_stmt(s, &subst)).collect();
    }
    bodies::enqueue_instantiation(mctx, InstKind::Fn, name, args, call_span)?;
    Ok(FnInfo { ast, decl })
}

/// Substitute + enqueue a method's (or associated fn's) own generic
/// parameters (plans/M13.md item Q). `receiver` is the concrete
/// `Type::Named` the call was made through (struct/enum args already
/// resolved). Returns the substituted AST/decl pair ready for
/// `check_call_args` / body checking.
pub(crate) fn instantiate_method(
    mctx: &ModuleCtx,
    receiver: &Type,
    method: &str,
    args: &[TypeArg],
    call_span: Span,
) -> Result<(ast::FnItem, DeclFn), SemaError> {
    let Type::Named(type_name, type_args) = receiver else {
        return Err(SemaError::at(
            "type",
            format!(
                "method `{method}` called on non-nominal type `{}`",
                types::render_type(receiver)
            ),
            call_span,
        ));
    };
    let (ast_orig, decl_orig) = lookup_method_decl(mctx, type_name, type_args, method, call_span)?;
    check_arity(&decl_orig.generics, args, method, call_span)?;
    let subst = build_subst(&decl_orig.generics, args, mctx, call_span)?;
    let decl = subst_decl_fn_direct(&decl_orig, &subst);
    let mut ast = ast_orig;
    ast.generics = Vec::new();
    if let Some(body) = ast.body.as_mut() {
        *body = body.iter().map(|s| subst_stmt(s, &subst)).collect();
    }
    bodies::enqueue_method_instantiation(mctx, receiver, method, args, call_span)?;
    Ok((ast, decl))
}

/// Resolve `(ast, decl)` for `method` on `type_name[type_args]`, applying
/// struct/enum-level substitution first when the receiver is itself a
/// generic instantiation.
fn lookup_method_decl(
    mctx: &ModuleCtx,
    type_name: &str,
    type_args: &[TypeArg],
    method: &str,
    call_span: Span,
) -> Result<(ast::FnItem, DeclFn), SemaError> {
    if let Some(s) = mctx.structs.get(type_name) {
        let info = if type_args.is_empty() {
            s.clone()
        } else {
            instantiate_struct(mctx, type_name, type_args, call_span)?
        };
        if let Some((f, d)) = info.method(method).or_else(|| info.assoc_fn(method)) {
            return Ok((f.clone(), d.clone()));
        }
        return Err(SemaError::at(
            "type",
            format!("type `{type_name}` has no method `{method}`"),
            call_span,
        ));
    }
    if let Some(e) = mctx.enums.get(type_name) {
        if !type_args.is_empty() {
            return Err(unimplemented_at("generic instantiation is", call_span));
        }
        if let Some((f, d)) = e.method(method).or_else(|| e.assoc_fn(method)) {
            return Ok((f.clone(), d.clone()));
        }
        return Err(SemaError::at(
            "type",
            format!("type `{type_name}` has no method `{method}`"),
            call_span,
        ));
    }
    Err(SemaError::at(
        "type",
        format!("unknown type `{type_name}`"),
        call_span,
    ))
}

// --- item 2: inferring a generic fn's type arguments ---------------------

/// The dumbest honest inference (item 2): a type parameter used directly
/// as a parameter's own type (`a: T`) is inferred from that argument's
/// synthesized type. Plans/M13.md item Q adds one nested shape the §8.3
/// idiom needs: a parameter typed `fn(...) -> R` with bare generic `R`
/// (in return position only) is inferred from a closure body's result
/// type or a named fn/method's declared return. Anything else nested
/// still contributes nothing. A const generic parameter is never
/// inferred. Mismatched or never-constrained type parameters are named
/// in the error, exactly as item 2 asks.
pub(crate) fn infer_fn_targs(
    fi: &FnInfo,
    args: &[Arg],
    fctx: &mut bodies::FnCtx,
    mctx: &ModuleCtx,
    call_span: Span,
) -> Result<Vec<TypeArg>, SemaError> {
    infer_generic_targs(
        &fi.decl.name,
        &fi.decl.generics,
        &fi.decl.params,
        args,
        fctx,
        mctx,
        call_span,
    )
}

/// Same inference as [`infer_fn_targs`], for a method/associated-fn
/// declaration (plans/M13.md item Q).
pub(crate) fn infer_method_targs(
    method_name: &str,
    decl: &DeclFn,
    args: &[Arg],
    fctx: &mut bodies::FnCtx,
    mctx: &ModuleCtx,
    call_span: Span,
) -> Result<Vec<TypeArg>, SemaError> {
    infer_generic_targs(
        method_name,
        &decl.generics,
        &decl.params,
        args,
        fctx,
        mctx,
        call_span,
    )
}

fn infer_generic_targs(
    display_name: &str,
    generics: &[DeclGenericParam],
    params: &[DeclParam],
    args: &[Arg],
    fctx: &mut bodies::FnCtx,
    mctx: &ModuleCtx,
    call_span: Span,
) -> Result<Vec<TypeArg>, SemaError> {
    let bound = bind_args_positionally(params, args);
    let mut inferred: BTreeMap<String, Type> = BTreeMap::new();
    for (i, p) in params.iter().enumerate() {
        let Some(arg_expr) = bound[i] else {
            continue; // a default-valued, unbound parameter: nothing to infer from.
        };
        match &p.ty {
            Type::Generic(gname) => {
                let synthesized = bodies::check_expr(arg_expr, None, fctx, mctx)?.ty;
                record_inferred(&mut inferred, gname, synthesized, display_name, call_span)?;
            }
            Type::Fn(fparams, fret) => {
                if let Type::Generic(gname) = fret.as_ref() {
                    if let Some(synthesized) =
                        infer_fn_arg_return(arg_expr, fparams, fctx, mctx, call_span)?
                    {
                        record_inferred(
                            &mut inferred,
                            gname,
                            synthesized,
                            display_name,
                            call_span,
                        )?;
                    }
                }
            }
            _ => {}
        }
    }
    let mut out = Vec::with_capacity(generics.len());
    for g in generics {
        match &g.kind {
            DeclGenericKind::Type => match inferred.get(&g.name) {
                Some(t) => out.push(TypeArg::Type(t.clone())),
                None => {
                    return Err(SemaError::at(
                        "generic",
                        format!(
                            "`{display_name}` requires explicit `[Args]`: parameter `{}` cannot be inferred",
                            g.name
                        ),
                        call_span,
                    ));
                }
            },
            DeclGenericKind::Const(_) => {
                return Err(SemaError::at(
                    "generic",
                    format!(
                        "`{display_name}` requires explicit `[Args]`: const parameter `{}` cannot be inferred",
                        g.name
                    ),
                    call_span,
                ));
            }
        }
    }
    Ok(out)
}

fn record_inferred(
    inferred: &mut BTreeMap<String, Type>,
    gname: &str,
    synthesized: Type,
    display_name: &str,
    call_span: Span,
) -> Result<(), SemaError> {
    if let Some(existing) = inferred.get(gname) {
        if !bodies::types_eq(existing, &synthesized) {
            return Err(SemaError::at(
                "generic",
                format!(
                    "`{display_name}` requires explicit `[Args]`: parameter `{gname}` is both `{}` and `{}`",
                    types::render_type(existing),
                    types::render_type(&synthesized)
                ),
                call_span,
            ));
        }
    } else {
        inferred.insert(gname.to_string(), synthesized);
    }
    Ok(())
}

/// Infer the return type of a `fn(...) -> R` argument for generic
/// inference (plans/M13.md item Q): a closure body's synthesized
/// result, or a named fn/method's declared return. Suite closures with
/// no valued `return` (the §8.3 `item.count += 1` shape) are `unit`.
/// `Ok(None)` means "contributes nothing" (caller must write `[Args]`
/// unless another occurrence binds the parameter).
fn infer_fn_arg_return(
    arg_expr: &Expr,
    fparams: &[(crate::syntax::ast::AccessMode, Type)],
    fctx: &mut bodies::FnCtx,
    mctx: &ModuleCtx,
    call_span: Span,
) -> Result<Option<Type>, SemaError> {
    match arg_expr {
        Expr::Closure(c) => {
            if c.params.len() != fparams.len() {
                return Err(SemaError::at(
                    "type",
                    format!(
                        "expected {} arguments, found {}",
                        fparams.len(),
                        c.params.len()
                    ),
                    c.span,
                ));
            }
            fctx.push_scope();
            for (cp, (_mode, ety)) in c.params.iter().zip(fparams.iter()) {
                let pty = match &cp.ty {
                    Some(t) => {
                        let resolved = mctx.resolve_type(t, &fctx.local_pools)?;
                        if !bodies::types_eq(&resolved, ety) {
                            fctx.pop_scope();
                            return Err(SemaError::at(
                                "type",
                                format!(
                                    "closure parameter `{}` expects `{}`, found `{}`",
                                    cp.name,
                                    types::render_type(ety),
                                    types::render_type(&resolved)
                                ),
                                cp.span,
                            ));
                        }
                        resolved
                    }
                    None => ety.clone(),
                };
                fctx.insert_local(cp.name.clone(), pty);
            }
            let result = match &c.body {
                ClosureBody::Expr(e) => {
                    bodies::check_expr(e, None, fctx, mctx).map(|te| Some(te.ty))
                }
                ClosureBody::Suite(stmts) => Ok(suite_inferred_return(stmts)),
            };
            fctx.pop_scope();
            result
        }
        Expr::Name(span, name) => {
            if let Some(fi) = mctx.fns.get(name) {
                if !fi.decl.generics.is_empty() {
                    return Err(unimplemented_at("generic instantiation is", *span));
                }
                return Ok(Some(fi.decl.ret.clone()));
            }
            Err(SemaError::at(
                "generic",
                format!("cannot infer return type of `fn(...) -> R` argument from `{name}`"),
                call_span,
            ))
        }
        Expr::Field(base, span, name) => {
            if let Expr::Name(_, bname) = base.as_ref() {
                if fctx.lookup_local(bname).is_none() {
                    if let Some(s) = mctx.structs.get(bname.as_str()) {
                        if let Some((_, d)) = s.assoc_fn(name).or_else(|| s.method(name)) {
                            if !d.generics.is_empty() || !s.decl.generics.is_empty() {
                                return Err(unimplemented_at("generic instantiation is", *span));
                            }
                            return Ok(Some(d.ret.clone()));
                        }
                    }
                    if let Some(e) = mctx.enums.get(bname.as_str()) {
                        if let Some((_, d)) = e.assoc_fn(name).or_else(|| e.method(name)) {
                            if !d.generics.is_empty() || !e.generics.is_empty() {
                                return Err(unimplemented_at("generic instantiation is", *span));
                            }
                            return Ok(Some(d.ret.clone()));
                        }
                    }
                }
            }
            Err(SemaError::at(
                "generic",
                "cannot infer return type of `fn(...) -> R` argument from this expression"
                    .to_string(),
                call_span,
            ))
        }
        _ => Err(SemaError::at(
            "generic",
            "cannot infer return type of `fn(...) -> R` argument from this expression".to_string(),
            call_span,
        )),
    }
}

/// A suite used as a short-form closure body (assign-only §8.3 shape)
/// returns `Some(unit)` when it never `return`s a value. A suite with a
/// valued `return` contributes nothing to inference (`None`) — write
/// explicit `[Args]`.
fn suite_inferred_return(stmts: &[Stmt]) -> Option<Type> {
    fn has_valued_return(stmts: &[Stmt]) -> bool {
        stmts.iter().any(|s| match s {
            Stmt::Return(_, Some(_)) => true,
            Stmt::If(i) => {
                has_valued_return(&i.then_branch)
                    || i.elifs.iter().any(|e| has_valued_return(&e.body))
                    || i.else_branch.as_ref().is_some_and(|b| has_valued_return(b))
            }
            Stmt::Match(m) => m.arms.iter().any(|a| has_valued_return(&a.body)),
            Stmt::For(f) => has_valued_return(&f.body),
            Stmt::While(w) => has_valued_return(&w.body),
            _ => false,
        })
    }
    if has_valued_return(stmts) {
        None
    } else {
        Some(Type::Unit)
    }
}

/// Binds `args` to `decl_params` (label or positional cursor, mirroring
/// `bodies::check_call_args`'s own binding order) far enough to hand
/// `infer_fn_targs` each bound parameter's argument *expression* — arity/
/// label validity itself is re-checked properly afterwards by the real
/// `check_call_args` call `bodies.rs` makes once the instantiation is
/// resolved, so this binder is deliberately permissive (out-of-range or
/// duplicate binds are simply ignored here rather than reported twice).
fn bind_args_positionally<'a>(decl_params: &[DeclParam], args: &'a [Arg]) -> Vec<Option<&'a Expr>> {
    let mut bound: Vec<Option<&'a Expr>> = vec![None; decl_params.len()];
    let mut cursor = 0usize;
    for a in args {
        let idx = match &a.label {
            Some(lbl) => decl_params.iter().position(|p| &p.name == lbl),
            None => {
                while cursor < decl_params.len() && bound[cursor].is_some() {
                    cursor += 1;
                }
                let i = cursor;
                cursor += 1;
                Some(i).filter(|&i| i < decl_params.len())
            }
        };
        if let Some(idx) = idx {
            bound[idx] = Some(&a.value);
        }
        // An unresolvable label/arity mismatch is left for the real
        // `check_call_args` to report properly; inference just skips it.
    }
    bound
}

// --- draining the queue: the actual per-instantiation checking ----------

/// Item H's own pass, run last (`mod.rs::check`): drains
/// `mctx.generics_queue` to a fixed point — checking one instantiation
/// can enqueue more (a nested generic use inside its substituted body),
/// so this loops until nothing new appears — running the *same* three
/// passes (`bodies`/`access`/`matches`) `check` already ran, concretely,
/// against each substituted declaration. `path` is cited verbatim in the
/// chain diagnostic (decision 2; see `mod.rs::check`'s own doc comment).
/// plans/M3.md item A: also returns every drained instantiation's typed
/// body (`typed::TypedInstantiation`), keyed by this file's own
/// canonical spelling — the identical string a `Call`/`OpCall` node's
/// `CalleeKey::FnInstance`/`MethodInstance` carries for the same
/// instantiation (`typed.rs`'s own doc comment).
pub(crate) fn check(
    _module: &Module,
    _decl_items: &[types::DeclItem],
    mctx: &ModuleCtx,
    path: &str,
) -> Result<BTreeMap<String, TypedInstantiation>, SemaError> {
    let mut processed: BTreeSet<String> = BTreeSet::new();
    let mut typed_instantiations: BTreeMap<String, TypedInstantiation> = BTreeMap::new();
    loop {
        let next = {
            let q = mctx.generics_queue.borrow();
            q.iter()
                .find(|(k, _)| !processed.contains(*k))
                .map(|(k, v)| (k.clone(), v.clone()))
        };
        let Some((key, entry)) = next else {
            break;
        };
        processed.insert(key.clone());
        *mctx.current_chain.borrow_mut() = entry.chain.clone();
        let result = check_one_instantiation(mctx, &entry);
        *mctx.current_chain.borrow_mut() = Vec::new();
        match result {
            Ok(typed_inst) => {
                typed_instantiations.insert(key, typed_inst);
            }
            Err(e) => return Err(finalize_diagnostic(e, &entry, mctx, path)),
        }
    }
    Ok(typed_instantiations)
}

fn check_one_instantiation(
    mctx: &ModuleCtx,
    entry: &QueuedInstantiation,
) -> Result<TypedInstantiation, SemaError> {
    let call_span = *entry
        .chain
        .last()
        .expect("a queued instantiation's chain always has at least its own triggering call");
    // plans/M13.md item G3: when re-typing an exporter's generic body
    // under an importer's ModuleCtx, field privacy must still see the
    // exporter as the use-site module (same carve-out the G1 census
    // applied by skipping Struct instantiation walks).
    let home = instantiation_visibility_home(mctx, entry);
    *mctx.visibility_home.borrow_mut() = Some(home);
    let result = check_one_instantiation_inner(mctx, entry, call_span);
    *mctx.visibility_home.borrow_mut() = None;
    result
}

fn instantiation_visibility_home(mctx: &ModuleCtx, entry: &QueuedInstantiation) -> String {
    match entry.kind {
        InstKind::Fn => mctx
            .fn_decl_module
            .get(&entry.name)
            .cloned()
            .unwrap_or_else(|| mctx.module_path.clone()),
        InstKind::Struct => mctx
            .struct_decl_module
            .get(&entry.name)
            .cloned()
            .unwrap_or_else(|| mctx.module_path.clone()),
        InstKind::Method => {
            let receiver = entry
                .receiver
                .as_ref()
                .expect("InstKind::Method always carries a receiver type");
            if let Type::Named(type_name, _) = receiver {
                mctx.struct_decl_module
                    .get(type_name)
                    .cloned()
                    .unwrap_or_else(|| mctx.module_path.clone())
            } else {
                mctx.module_path.clone()
            }
        }
        InstKind::Enum => mctx.module_path.clone(),
    }
}

fn check_one_instantiation_inner(
    mctx: &ModuleCtx,
    entry: &QueuedInstantiation,
    call_span: Span,
) -> Result<TypedInstantiation, SemaError> {
    match entry.kind {
        InstKind::Fn => {
            let fi = instantiate_fn(mctx, &entry.name, &entry.args, call_span)?;
            let tf = bodies::check_top_fn(&fi.ast, &fi.decl, mctx)?
                .expect("an instantiated fn is always concrete, never itself generic");
            let empty_effects = access::EffectMap::new();
            access::check_top_fn(&fi.ast, &fi.decl, mctx, &empty_effects)?;
            flow::check_top_fn(&fi.ast, &fi.decl, mctx, &empty_effects)?;
            matches::check_top_fn(&fi.ast, &fi.decl, mctx)?;
            Ok(TypedInstantiation::Fn(tf))
        }
        InstKind::Method => {
            let receiver = entry
                .receiver
                .as_ref()
                .expect("InstKind::Method always carries a receiver type");
            let (ast, decl) =
                instantiate_method(mctx, receiver, &entry.name, &entry.args, call_span)?;
            let mini = method_instantiation_struct_info(mctx, receiver, &ast, &decl, call_span)?;
            let ts = bodies::check_struct_members(&mini, receiver.clone(), mctx)?;
            let effects = access::infer_effects_over(mctx);
            access::check_struct_members(&mini, receiver.clone(), mctx, &effects)?;
            flow::check_struct_members(&mini, receiver.clone(), mctx, &effects)?;
            matches::check_struct_members(&mini, receiver.clone(), mctx)?;
            let tf = ts
                .methods
                .get(&entry.name)
                .or_else(|| ts.assoc_fns.get(&entry.name))
                .cloned()
                .expect("instantiated method was just checked into the mini struct");
            Ok(TypedInstantiation::Fn(tf))
        }
        InstKind::Struct => {
            let si = instantiate_struct(mctx, &entry.name, &entry.args, call_span)?;
            let self_ty = Type::Named(entry.name.clone(), entry.args.clone());
            let ts = bodies::check_struct_members(&si, self_ty.clone(), mctx)?;
            let effects = access::infer_effects_over(mctx);
            access::check_struct_members(&si, self_ty.clone(), mctx, &effects)?;
            flow::check_struct_members(&si, self_ty.clone(), mctx, &effects)?;
            matches::check_struct_members(&si, self_ty, mctx)?;
            Ok(TypedInstantiation::Struct(ts))
        }
        InstKind::Enum => {
            // Enums carry no bodies/methods (02-language.md §7.2):
            // substitution + reclassification (already run by
            // `instantiate_enum`) is the whole of "checking" one.
            instantiate_enum(mctx, &entry.name, &entry.args, call_span)?;
            Ok(TypedInstantiation::Enum)
        }
    }
}

/// One-method `StructInfo` so the existing struct-member check/access/
/// flow/matches passes run over a method instantiation unchanged
/// (plans/M13.md item Q — reuse, no parallel checker).
fn method_instantiation_struct_info(
    mctx: &ModuleCtx,
    receiver: &Type,
    ast: &ast::FnItem,
    decl: &DeclFn,
    call_span: Span,
) -> Result<StructInfo, SemaError> {
    let Type::Named(type_name, type_args) = receiver else {
        return Err(SemaError::at(
            "type",
            format!(
                "method `{}` called on non-nominal type `{}`",
                decl.name,
                types::render_type(receiver)
            ),
            call_span,
        ));
    };
    let mut base = if let Some(s) = mctx.structs.get(type_name.as_str()) {
        if type_args.is_empty() {
            s.clone()
        } else {
            instantiate_struct(mctx, type_name, type_args, call_span)?
        }
    } else if let Some(e) = mctx.enums.get(type_name.as_str()) {
        // Enum methods: fabricate a struct-shaped shell carrying only the
        // method — check_struct_members only reads name/members/pools.
        return Ok(StructInfo {
            decl: DeclStruct {
                name: type_name.clone(),
                generics: Vec::new(),
                deriving: e.deriving.clone(),
                classification: e.classification,
                members: vec![DeclMember::Fn(decl.clone())],
                is_resource_fiat: false,
                is_actor: false,
                is_driver: false,
                layout_kind: None,
                component_types: Vec::new(),
                span: e.span,
            },
            ast_members: vec![Member::Fn(ast.clone())],
            deferred_comptime_members: Vec::new(),
        });
    } else {
        return Err(SemaError::at(
            "type",
            format!("unknown type `{type_name}`"),
            call_span,
        ));
    };
    base.decl.members = vec![DeclMember::Fn(decl.clone())];
    base.ast_members = vec![Member::Fn(ast.clone())];
    base.deferred_comptime_members = Vec::new();
    Ok(base)
}

// --- the requirement-chain diagnostic (decision 2) -----------------------
//
// Pinned verbatim (02-language.md §7.3):
//
//   error[generic]: `hash_pair[Sector]` requires `Sector.hash(read self) -> u64`
//     required by `a.hash()` at util/hash.wr:2
//     instantiated at storage/extent.wr:41
//
// This is recognized, not derived through general inference (decision 4
// rules out a constraint solver): a *directly substituted* instantiation
// fn body that fails with exactly the "no method"/"no operator method"
// shape `bodies.rs`'s own `check_call_by_field`/`resolve_operator_method`
// already produce, on a parameter whose *un*substituted type was a bare
// `T`, is recognized as a missing requirement. The required method's
// return type comes from re-scanning the *original* (unsubstituted)
// declaration's own body for the narrow shape 02 §7.3's own example is:
// a `return` whose expression is directly `<param>.<method>()`, or a
// builtin same-type-result binary operator (`+ - * / % & | ^ << >>`)
// applied to two such calls — decision 4's expected-type propagation
// applied by hand to exactly the return-position case the docs pin.
// Anything else recognizably "missing" but outside this shape, or any
// other kind of failure, keeps its ordinary one-line message and still
// gets the `instantiated at` chain appended (item 3): only the "required
// by" line and the location-free primary line are specific to this exact
// shape.

fn finalize_diagnostic(
    e: SemaError,
    entry: &QueuedInstantiation,
    mctx: &ModuleCtx,
    path: &str,
) -> SemaError {
    if e.category == "type" && e.extra_lines.is_empty() {
        if let Some((type_name, method_name)) = e.missing_method.clone() {
            if let Some((call_expr, ret_ty)) =
                find_requirement(mctx, entry, &type_name, &method_name)
            {
                let sig = format!(
                    "{type_name}.{method_name}(read self) -> {}",
                    types::render_type(&ret_ty)
                );
                let display = display_inst_name(entry);
                let mut extra_lines = vec![format!(
                    "  required by `{}` at {path}:{}",
                    printer::print_expr_bare(call_expr),
                    call_expr.span().line
                )];
                for span in entry.chain.iter().rev() {
                    extra_lines.push(format!("  instantiated at {path}:{}", span.line));
                }
                return SemaError {
                    category: "generic",
                    message: format!("`{display}` requires `{sig}`"),
                    line: 0,
                    col: 0,
                    extra_lines,
                    omit_location: true,
                    missing_method: None,
                };
            }
        }
    }
    let mut e = e;
    for span in entry.chain.iter().rev() {
        e.extra_lines
            .push(format!("  instantiated at {path}:{}", span.line));
    }
    e
}

/// Recognizes a top-level generic `fn` (`InstKind::Fn`) or a method-owned
/// generic (`InstKind::Method`, plans/M13.md item Q) — the pinned §7.3
/// shape. A struct/enum instantiation's own missing-method failures fall
/// back to the ordinary one-line-plus-chain case instead of trying to
/// attribute the failure to one particular method among many.
fn find_requirement<'a>(
    mctx: &'a ModuleCtx,
    entry: &QueuedInstantiation,
    type_name: &str,
    method_name: &str,
) -> Option<(&'a Expr, Type)> {
    match entry.kind {
        InstKind::Fn => {
            let fi = mctx.fns.get(&entry.name)?;
            find_requirement_in(
                &fi.decl.generics,
                &entry.args,
                &fi.decl.params,
                fi.ast.body.as_ref()?,
                &fi.decl.ret,
                type_name,
                method_name,
            )
        }
        InstKind::Method => {
            let receiver = entry.receiver.as_ref()?;
            let Type::Named(recv_name, recv_args) = receiver else {
                return None;
            };
            // Requirement scan uses the *unsubstituted* method body from
            // the declared (possibly struct-generic) type — same as free
            // fns. Struct-level args do not change which bare type
            // parameter of the *method* was the receiver of `.hash()`.
            let (ast, decl) = if let Some(s) = mctx.structs.get(recv_name.as_str()) {
                if !recv_args.is_empty() {
                    // Prefer the original declaration's method (pre
                    // struct-subst) so param types still show Type::Generic.
                    let (f, d) = s.method(&entry.name).or_else(|| s.assoc_fn(&entry.name))?;
                    (f, d)
                } else {
                    let (f, d) = s.method(&entry.name).or_else(|| s.assoc_fn(&entry.name))?;
                    (f, d)
                }
            } else if let Some(e) = mctx.enums.get(recv_name.as_str()) {
                let (f, d) = e.method(&entry.name).or_else(|| e.assoc_fn(&entry.name))?;
                (f, d)
            } else {
                return None;
            };
            find_requirement_in(
                &decl.generics,
                &entry.args,
                &decl.params,
                ast.body.as_ref()?,
                &decl.ret,
                type_name,
                method_name,
            )
        }
        InstKind::Struct | InstKind::Enum => None,
    }
}

fn find_requirement_in<'a>(
    generics: &[DeclGenericParam],
    args: &[TypeArg],
    params: &[DeclParam],
    body: &'a [Stmt],
    ret: &Type,
    type_name: &str,
    method_name: &str,
) -> Option<(&'a Expr, Type)> {
    let target_param = generics
        .iter()
        .zip(args.iter())
        .find_map(|(g, a)| match (&g.kind, a) {
            (DeclGenericKind::Type, TypeArg::Type(Type::Named(n, targs)))
                if n == type_name && targs.is_empty() =>
            {
                Some(g.name.clone())
            }
            _ => None,
        })?;
    let mut param_types = BTreeMap::new();
    for p in params {
        param_types.insert(p.name.clone(), p.ty.clone());
    }
    let (call_expr, found_method) = infer_requirement_call(body, &target_param, &param_types)?;
    if found_method != method_name {
        return None;
    }
    Some((call_expr, ret.clone()))
}

fn infer_requirement_call<'a>(
    body: &'a [Stmt],
    generic_param: &str,
    param_types: &BTreeMap<String, Type>,
) -> Option<(&'a Expr, String)> {
    for stmt in body {
        if let Stmt::Return(_, Some(expr)) = stmt {
            if let Some(found) = scan_return_expr(expr, generic_param, param_types) {
                return Some(found);
            }
        }
    }
    None
}

fn scan_return_expr<'a>(
    expr: &'a Expr,
    g: &str,
    param_types: &BTreeMap<String, Type>,
) -> Option<(&'a Expr, String)> {
    if let Some(method) = direct_method_call(expr, g, param_types) {
        return Some((expr, method));
    }
    if let Expr::Binary(_, op, l, r) = expr {
        if is_same_type_result_op(*op) {
            if let Some(method) = direct_method_call(l, g, param_types) {
                return Some((l, method));
            }
            if let Some(method) = direct_method_call(r, g, param_types) {
                return Some((r, method));
            }
        }
    }
    None
}

fn is_same_type_result_op(op: BinOp) -> bool {
    matches!(
        op,
        BinOp::Add
            | BinOp::Sub
            | BinOp::Mul
            | BinOp::Div
            | BinOp::Rem
            | BinOp::AddW
            | BinOp::SubW
            | BinOp::MulW
            | BinOp::BitAnd
            | BinOp::BitOr
            | BinOp::BitXor
            | BinOp::Shl
            | BinOp::Shr
    )
}

/// `<name>.<method>()` (zero arguments — the pinned example's own shape;
/// a requirement with arguments is not attempted) where `name`'s
/// *declared* (unsubstituted) type is exactly the bare generic parameter
/// `g`.
fn direct_method_call(
    expr: &Expr,
    g: &str,
    param_types: &BTreeMap<String, Type>,
) -> Option<String> {
    let Expr::Call(callee, _, args) = expr else {
        return None;
    };
    if !args.is_empty() {
        return None;
    }
    let Expr::Field(base, _, method) = callee.as_ref() else {
        return None;
    };
    let Expr::Name(_, base_name) = base.as_ref() else {
        return None;
    };
    match param_types.get(base_name) {
        Some(Type::Generic(pn)) if pn == g => Some(method.clone()),
        _ => None,
    }
}

// --- tests --------------------------------------------------------------
//
// 02-language.md §6.3: "A const argument is `bool`, `char`, an integer, or
// a fieldless enum, evaluated by the comptime engine." plans/M3.md item B
// lifts M2-H's literal-only subset: `eval_const_expr` now runs the real
// evaluator, so arithmetic and const-name chains both evaluate (rather
// than failing closed) — these pin that directly, rather than only
// through a full generic-instantiation golden.
//
// `eval_const_expr` takes a `&ModuleCtx` (for const-name/enum-variant
// lookup), so the tests build one the same way `sema::mod::check` does —
// lex -> parse -> `types::declare` -> `bodies::build_module_ctx` — real
// production code, not a mock/fixture rig.
#[cfg(test)]
mod tests {
    use super::*;
    use crate::syntax::{lexer, parser};

    fn build_mctx(src: &str) -> ModuleCtx {
        let tokens = lexer::lex(src).expect("test source must lex");
        let module = parser::parse(tokens).expect("test source must parse");
        let decl_items = types::declare(&module).expect("test source must declare");
        bodies::build_module_ctx(&module, &decl_items, &types::ImportedTypes::new())
    }

    const SRC: &str = "module examples.const_eval

const LIMIT: u64 = 4

enum Color:
    Red
    Green
    Blue

pub fn use_const() -> u64:
    return LIMIT
";

    #[test]
    fn eval_const_expr_literal_int_bool_char() {
        let mctx = build_mctx(SRC);
        let span = Span::default();
        assert_eq!(
            eval_const_expr(&Expr::Int(span, "42".to_string()), Some(&Type::U64), &mctx).unwrap(),
            Expr::Int(span, "42".to_string())
        );
        assert_eq!(
            eval_const_expr(&Expr::Bool(span, true), Some(&Type::Bool), &mctx).unwrap(),
            Expr::Bool(span, true)
        );
        assert_eq!(
            eval_const_expr(
                &Expr::Char(span, "'x'".to_string()),
                Some(&Type::Char),
                &mctx
            )
            .unwrap(),
            Expr::Char(span, "'x'".to_string())
        );
    }

    /// A bare `const` reference resolves by evaluating its own
    /// initializer with the real evaluator (here, `LIMIT`'s own `4`
    /// literal).
    #[test]
    fn eval_const_expr_resolves_a_const_name() {
        let mctx = build_mctx(SRC);
        let span = Span::default();
        let result = eval_const_expr(
            &Expr::Name(span, "LIMIT".to_string()),
            Some(&Type::U64),
            &mctx,
        );
        assert_eq!(result.unwrap(), Expr::Int(span, "4".to_string()));
    }

    /// A fieldless enum variant path (`Color.Red`) evaluates to its own
    /// `Enum.Variant` shape unchanged.
    #[test]
    fn eval_const_expr_fieldless_enum_variant() {
        let mctx = build_mctx(SRC);
        let span = Span::default();
        let expr = Expr::Field(
            Box::new(Expr::Name(span, "Color".to_string())),
            span,
            "Red".to_string(),
        );
        let expected_ty = Type::Named("Color".to_string(), vec![]);
        let result = eval_const_expr(&expr, Some(&expected_ty), &mctx);
        assert_eq!(
            result.unwrap(),
            Expr::Field(
                Box::new(Expr::Name(span, "Color".to_string())),
                span,
                "Red".to_string()
            )
        );
    }

    /// An unknown const name fails closed rather than guessing.
    #[test]
    fn eval_const_expr_unknown_const_name_fails_closed() {
        let mctx = build_mctx(SRC);
        let span = Span::default();
        assert!(
            eval_const_expr(
                &Expr::Name(span, "NOPE".to_string()),
                Some(&Type::U64),
                &mctx
            )
            .is_err()
        );
    }

    /// Arithmetic is no longer out of scope (plans/M3.md item B lifts
    /// M2-H's own literal-only limit): `1 + 1` evaluates to `2`. Neither
    /// literal has a concrete sibling, so both default to `i64`
    /// (`check_same_type_operands`'s own rule) — the expected type here
    /// matches that default rather than forcing a mismatch.
    #[test]
    fn eval_const_expr_evaluates_arithmetic() {
        let mctx = build_mctx(SRC);
        let span = Span::default();
        let expr = Expr::Binary(
            span,
            BinOp::Add,
            Box::new(Expr::Int(span, "1".to_string())),
            Box::new(Expr::Int(span, "1".to_string())),
        );
        assert_eq!(
            eval_const_expr(&expr, Some(&Type::I64), &mctx).unwrap(),
            Expr::Int(span, "2".to_string())
        );
    }
}
