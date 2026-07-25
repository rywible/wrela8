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

use std::collections::{BTreeMap, BTreeSet};

use crate::sema::types::{self, Type};
use crate::syntax::ast::{AccessMode, BinOp, Span};

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

/// Whether `key` (a `TypedExprKind::Intrinsic::key` spelling) is one of
/// the *graph-building* `@image` builder intrinsics (plans/M4.md item B,
/// decision 5) — legal only during `@image` evaluation
/// (`eval::legal`'s own first real illegal arm). `RestartIntensity`/
/// `seconds` share the same node kind (dumbest way that works, one
/// dispatch point in `eval::interp`) but are ordinary comptime-legal
/// prelude helpers, not graph effects, so they are deliberately excluded
/// — legal everywhere, exactly like `Option`/`Result`.
pub fn is_restricted_intrinsic(key: &str) -> bool {
    matches!(
        key,
        "Image"
            | "Image.device"
            | "Image.driver"
            | "Image.actor"
            | "Image.pool"
            | "Image.dma_pool"
            | "Image.supervise"
            | "Image.check_layout"
            | "Image.seal"
            | "ImageDecl.handle"
    )
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
    /// One `@image`-builder intrinsic call (plans/M4.md item B, decision
    /// 5: 05-library.md §9's whole builder surface — `Image(...)`,
    /// `img.device[D](...)`, `img.driver(A, ...)`, `img.actor(A, ...)`,
    /// `img.pool[T](...)`, `img.dma_pool[T](...)`, `img.supervise(...)`,
    /// `img.check_layout(f)`, `img.seal()`, `decl.handle()` — plus the
    /// two prelude helpers `RestartIntensity(...)`/`seconds(n)`) is one
    /// dedicated node kind rather than an ordinary `Call`: none of these
    /// have a declared parameter list a `Call` node could align its
    /// `args` against positionally, and `eval::image`'s report renders
    /// every argument by its own source label, not position, so each
    /// label is kept alongside its value instead of erased.
    ///
    /// `key` is the fixed intrinsic spelling (`"Image"`, `"Image.driver"`,
    /// `"ImageDecl.handle"`, `"seconds"`, ...) `sema::typed::is_restricted_intrinsic`
    /// and `eval::image`'s own dispatch both match on directly —
    /// `sema::bodies`'s own callee-key-by-spelling convention, reused
    /// verbatim for the builder surface (decision 5: "recognized by
    /// callee key exactly like the existing prelude/intrinsic
    /// machinery"). `receiver` is `Some` for a method-shaped intrinsic
    /// (`img.driver(...)`, `decl.handle()`) and `None` for a bare
    /// call-by-name one (`Image(...)`, `seconds(n)`); a method-shaped
    /// intrinsic's own receiver is only ever actually *read* by
    /// `ImageDecl.handle` (every `Image`-rooted intrinsic mutates the
    /// evaluator's own single active builder instead, decision 6: at
    /// most one `@image` fn, so at most one builder is ever live) but is
    /// still carried uniformly, mirroring `Call`'s own shape. `type_arg`
    /// is the builder's own bare type-name slot, already resolved
    /// (`img.device[D]`/`img.pool[T]`/`img.dma_pool[T]`'s bracket
    /// argument, or `img.driver`/`img.actor`'s leading unlabeled
    /// argument) when the intrinsic has one. `args` is every remaining
    /// argument, labeled, in source order — `img.pool`/`img.dma_pool`'s
    /// own `name=` argument is the one case that is not an ordinary
    /// value expression (a bound `pool` name is not usable as a value
    /// anywhere else in the language) and is instead carried as a
    /// `PoolName` leaf node (below).
    Intrinsic {
        key: String,
        receiver: Option<Box<TypedExpr>>,
        type_arg: Option<Type>,
        args: Vec<(String, TypedExpr)>,
    },
    /// A bare `pool` name used as `img.pool[T](name=P, ...)`/
    /// `img.dma_pool[T](name=P, ...)`'s own `name=` argument (plans/M4.md
    /// item B) — the one builder argument that is not an ordinary value
    /// expression: a module- or actor-scoped pool name (02-language.md
    /// §4) is otherwise only ever spelled inside an `own[P] T` type
    /// annotation, never referenced as a value, so it needs its own leaf
    /// node rather than resolving through `synth_name`'s ordinary
    /// local/const/fn lookup (which would, correctly, reject it). `ty` is
    /// always the builder surface's own opaque `PoolName` type.
    PoolName(String),
    /// `await expr` (plans/M6.md item A, 02-language.md §9.4/§9.2):
    /// `inner` is the "raw" (uncomposed) call this await resolves —
    /// either an actor-handle method `Call` (receiver typed `Actor[T]`)
    /// or a `Group.join_all` `Intrinsic` — and `ty` (on the wrapping
    /// node) is the *composed* result: the CallError table applied
    /// directly for an actor call, or mapped element-wise over the raw
    /// `[R; N]`/`[Result[T,E]; N]` for a group join (`bodies::compose_call_error`,
    /// `bodies::check_await`). One dedicated node rather than folding
    /// composition into `Call`/`Intrinsic` directly: the *uncomposed*
    /// type is still needed wherever `inner` is inspected on its own
    /// (none today, but keeps the raw call shape uniform with an
    /// ordinary, non-awaited `Call`).
    Await(Box<TypedExpr>),
    /// `send actor.method(...)` (02-language.md §9.4) — either used as a
    /// value (`match send x.y(...): ...`'s own operand) or carried by a
    /// `TypedStmtKind::BareSend` statement, the proof-conditioned form
    /// `sema::send_proof` decides on (plans/M6.md item G). The node's own
    /// type is identical either way: the proof is a *statement legality*
    /// verdict, never a type refinement — 02 §9.4's "the error type is
    /// `never`" erasure is not shipped at M6 (decision 8's own "erasure
    /// shipped: none", recorded in `actors.send.statement-requires-proof`).
    /// `inner` is the raw message `Call` (receiver `Actor[T]`, callee a
    /// `unit`-returning method); `ty` (on the wrapping node) is always
    /// `Result[unit, Rejected]` — `Rejected`'s own payload (the take-args
    /// handed back) is opaque at M6 (02 §9.4, the moved-payloads story).
    Send(Box<TypedExpr>),
    /// `g.start(callee, args...)`'s own first (callee) argument
    /// (plans/M6.md item A, 02-language.md §9.5) — the one `Group.start`
    /// argument that is not an ordinary value expression: a group
    /// child's callee is a same-module `async fn` or a `self` method,
    /// recognized directly (`bodies::resolve_group_child_callee`) rather
    /// than resolved through `synth_name`'s ordinary lookup (an async
    /// fn/method is never otherwise a callable value — see the module's
    /// own "only invocation forms" note). Mirrors `PoolName`'s own doc
    /// comment exactly. `ty` is the callee's own structural `fn` type
    /// (`bodies::fn_value_type`), for display only — nothing calls it as
    /// a value.
    GroupChild(CalleeKey),
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
    /// `comptime assert` (plans/M3.md item D, decision 8: "evaluates
    /// after typing; failure is a build error with the message"). The
    /// one deliberate exception to decision 1's "no spans anywhere": a
    /// comptime assert is checked exactly once, unconditionally, by
    /// `eval::check_comptime_asserts` — independent of whether anything
    /// ever calls the fn/method it lives in (the ordinary per-call
    /// evaluator, `interp::exec_stmt`, treats this node as a no-op, since
    /// re-running it per call would be redundant and its own vocabulary
    /// — module consts/literals only, no locals — never depends on a
    /// call's own arguments anyway) — so its own build-error diagnostic
    /// needs a real `L:C` to be useful, unlike every other typed node's
    /// failure (which already has a live call-stack instead).
    ComptimeAssert {
        span: Span,
        cond: TypedExpr,
        message: Option<TypedExpr>,
    },
    Defer(TypedDeferBody),
    ExprStmt(TypedExpr),
    /// A bare `send actor.method(...)` **statement** — 02-language.md
    /// §9.4's one proof-conditioned form ("when mailbox analysis proves
    /// admission cannot fail ... `send` stands as a bare statement;
    /// otherwise the result must be consumed"). `expr` is always a
    /// `TypedExprKind::Send` node; the statement discards its
    /// `Result[unit, Rejected]` value.
    ///
    /// Its own node kind rather than an `ExprStmt` wrapping a `Send`
    /// (plans/M6.md item G): the whole-image proof
    /// (`sema::send_proof`) runs *after* every module is typed, needs to
    /// find exactly these statements (never the expression form, which
    /// is always legal), and — like `ComptimeAssert` — is the second and
    /// last deliberate exception to decision 1's "no spans anywhere":
    /// its rejection is produced long after the body walk that could
    /// have reported a location, so the node carries the `send`
    /// keyword's own span to keep the diagnostic's `at L:C` real
    /// instead of omitted.
    BareSend {
        span: Span,
        expr: TypedExpr,
    },
    /// `with group(capacity=.., deadline=..) [as g]:` (plans/M6.md item
    /// A, 02-language.md §9.5, §10). `capacity`/`deadline` are the
    /// group-constructor's own (optional) labeled arguments, already
    /// typed; `as_name`, when present, is bound (`Type::Named("Group",
    /// [])`, an opaque builtin resource) inside `body` only — the
    /// group's own `with`-scoping (decision: consumed at block end,
    /// 02-language.md §10) — `bodies::check_with` pops the binding
    /// itself, `TypedFn`'s own scope stack carries no trace of it past
    /// this node. The scoped-`pool` `with` form stays fail-closed
    /// (02-language.md §10's other intrinsic scope — out of the M6
    /// honest-scope line); this node is `group` only.
    WithGroup {
        capacity: Option<TypedExpr>,
        deadline: Option<TypedExpr>,
        as_name: Option<String>,
        body: Vec<TypedStmt>,
    },
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
    /// Plans/M6.md item A's own addition: mirrors `types::DeclFn::is_async`
    /// (already recorded at declaration time, M2) onto the checked body —
    /// `eval::legal::classify` reads this directly (an `async fn`'s own
    /// node is illegal for comptime unconditionally, regardless of what
    /// its body contains: 02-language.md §12's "free of ... async/actor
    /// operations" names the fn's own color, not merely its statements).
    /// Not rendered by `dump` below — same bookkeeping-only reasoning as
    /// `TypedStruct::fields`/`TypedProgram::enums` (the `--stage=check`
    /// dump, `types.rs`, already prints `AsyncFn`; this is `--stage=typed`'s
    /// own tree, whose existing `Fn`/`Method` line text must not move for
    /// any already-checked async fn, e.g. golden `check-decls`'s `fetch`).
    pub is_async: bool,
    /// plans/M7.md item G: `@task` bottom half (03-hardware.md §6).
    pub is_task: bool,
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
    /// Each field's own declared type, keyed by name — the same walk
    /// `fields` above is built by, one step further (`bodies::check_struct_members`
    /// already has the resolved `Type` in hand there: it is the expected
    /// type it checks that field's own default against). Kept as a
    /// by-name map rather than folded into `fields` because `fields` is
    /// consumed *by index* everywhere else (`eval::value::Value::Struct`,
    /// `mwir`'s own field-index projections), and this is only ever
    /// looked up by name.
    ///
    /// Added for `eval::image_checks::check_one_decl`, which needs a
    /// field's declared type to check an image wiring argument against a
    /// struct that declares no `init` (05-library.md §9: an actor
    /// declaration's arguments "must match `A.init` (or its literal
    /// constructor)" — a struct with no `init` is constructed by its
    /// declared fields). Evaluator/checker-only bookkeeping, exactly like
    /// `fields`: not rendered by `dump` below, so no pinned golden text
    /// changes.
    pub field_types: BTreeMap<String, Type>,
    /// Every field that declared a default, typed once (with `self`
    /// bound, since a default may reference it) — a struct literal that
    /// omits the field elides it from its own `fields` list instead of
    /// re-typing the same default at every construction site.
    pub field_defaults: BTreeMap<String, TypedExpr>,
    pub methods: BTreeMap<String, TypedFn>,
    pub assoc_fns: BTreeMap<String, TypedFn>,
    pub init: Option<TypedFn>,
}

/// One non-generic enum's checked body (plans/M9.md item B2): variant
/// names in declaration order (decision 5's `Value::Enum` index) plus
/// the methods/associated fns 02 §5 / §7.5 give every type — the same
/// two maps a `TypedStruct` already carries. Before B2 an enum was only
/// a `Vec<String>` of variant names and had no method surface at all.
#[derive(Debug, Clone, PartialEq)]
pub struct TypedEnum {
    pub variants: Vec<String>,
    pub methods: BTreeMap<String, TypedFn>,
    pub assoc_fns: BTreeMap<String, TypedFn>,
}

impl TypedEnum {
    /// A TypedEnum that only records variant names (prelude enums and
    /// the pre-B2 shape). Methods/assoc_fns start empty.
    pub fn from_variants(variants: Vec<String>) -> Self {
        Self {
            variants,
            methods: BTreeMap::new(),
            assoc_fns: BTreeMap::new(),
        }
    }
}

/// One generic instantiation's checked body (plans/M2.md item H, carried
/// into M3): keyed in `TypedProgram::instantiations` by
/// `generics::canonical_key`'s own spelling. An enum instantiation with
/// no methods still carries no payload beyond confirming it was
/// checked/reclassified; one with methods is out of scope for B2
/// (generic enum methods fail closed at the same generic-method
/// boundary structs already use).
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
/// gap); `Exhaustive` for `@test(exhaustive)` (02-language.md §12.2:
/// every input in the fn's finite parameter domain is enumerated and
/// the body run once per case, each under its own fresh quota — a
/// passing exhaustive test is a verified statement about the whole
/// domain, not a sample; `sema::bodies` validates the parameter types
/// are enumerable before this kind is ever recorded).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TestKind {
    Comptime,
    Runtime,
    Exhaustive,
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
    /// Every top-level (non-generic) user enum (plans/M3.md item B's
    /// variant-order record, extended by plans/M9.md item B2 with
    /// methods/associated fns). Not rendered by `dump` below — same
    /// reasoning as `TypedStruct::fields`.
    pub enums: BTreeMap<String, TypedEnum>,
    pub instantiations: BTreeMap<String, TypedInstantiation>,
    /// The bare name of this module's own `@image fn`, if it declares
    /// one (plans/M4.md item B) — `sema::bodies::check`'s own addition,
    /// set at most once per module (a second `@image fn` in the *same*
    /// module is rejected there directly); `eval::legal::classify` reads
    /// this to exempt the one fn actually allowed to use the builder
    /// intrinsics directly, and `wrela dump --stage=image` reads it (via
    /// every checked module, decision 6: exactly one reachable `@image`
    /// in the whole build) to find the fn to evaluate.
    pub image_fn: Option<String>,
    /// This module's own module-scoped `pool Name` declarations
    /// (plans/M4.md item C, `image.graph.pools-bound-once`/`seal-fully-bound`):
    /// `sema::bodies::check`'s own copy of `ModuleCtx::module_pools`, kept
    /// only so `eval::image_checks`'s post-seal pass can name a pool the
    /// `@image` fn's own module declared but never bound by `img.pool`/
    /// `img.dma_pool` before `img.seal()` — the graph itself only ever
    /// records *bound* pools (`ImageGraph::pools`/`dma_pools`), so an
    /// unbound one leaves no trace there at all. Not rendered by `dump`
    /// below — evaluator-only bookkeeping, same reasoning as
    /// `TypedStruct::fields`/`TypedProgram::enums`.
    pub declared_pools: BTreeSet<String>,
    /// Every `@layout` type this module declares, already checked and
    /// laid out by `types::check_layouts` (plans/M7.md item B), in
    /// declaration order. Carried forward for exactly one reason
    /// (plans/M7.md item D): `eval::image_checks::check_pool_decls` has
    /// to answer 03-hardware.md §3's "`T` is `@layout(dma)`" about an
    /// `img.dma_pool[T]`'s own payload type, and `check_sealed` is handed
    /// `TypedProgram`s and nothing else. `check_layouts` already ran (and
    /// its rejections already fired) inside the sema pass that produced
    /// this program — this field is that same pass's *table*, kept
    /// instead of discarded, never a second computation. Not rendered by
    /// `dump` below, same reasoning as `declared_pools` above.
    pub layouts: Vec<types::LayoutType>,
    /// plans/M7.md item E1: the image-declared virtio-blk capacity
    /// (`img.device(..., capacity_sectors=N)`), filled by the image
    /// check / eval path before lowering so `read_capacity_sectors`
    /// can emit it as a build constant. `None` until an `@image` seals
    /// a device that declares one — a call with no capacity then fails
    /// closed at lower by name.
    pub blk_capacity_sectors: Option<u64>,
    /// plans/M7.md item E1: every `VirtQueue.configure(pool=take P, ...,
    /// depth=N)` site this module typed — `(pool name, depth)`. Layout
    /// places the ring from these facts and nowhere else.
    pub virtqueue_configures: Vec<(String, u16)>,
    /// plans/M9.md item A1b: the declarations this module *imports*,
    /// keyed by the local (possibly aliased) spelling — the comptime
    /// evaluator's read-only window onto the rest of the build closure.
    /// Empty for every single-module build (`sema::mod::check_typed`
    /// rejects an import outright) and filled once, by
    /// `sema::mod::check_program_typed`, after every module in the
    /// closure has finished its own `bodies::check`.
    ///
    /// Deliberately a *separate* field rather than extra entries in the
    /// four maps above: `consts`/`fns`/`structs`/`enums` are "what this
    /// module declares", and every backend stage
    /// (`lower`/`flowwir_lower`/`layout`/`codegen`) iterates them to emit
    /// code exactly once per declaration. Merging imports into them
    /// would emit an imported fn once per importer. `eval::interp` is
    /// the only consumer that wants the union, so the union lives at the
    /// lookup, not in the maps.
    pub imported: ImportedDecls,
}

/// plans/M9.md item A1b: one module's imported declarations, copied
/// wholesale from the exporting module's already-finished
/// `TypedProgram` and re-keyed under the importing module's own local
/// name. Read-only reuse of finished output, exactly like
/// `sema::mod::check_program_typed`'s existing `ModuleCtx` splice —
/// nothing here is re-checked, and nothing here requires one module's
/// evaluation to finish before another's can begin, so import cycles
/// stay free (golden/check-import-comptime-cycle).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct ImportedDecls {
    pub consts: BTreeMap<String, TypedConst>,
    pub fns: BTreeMap<String, TypedFn>,
    pub structs: BTreeMap<String, TypedStruct>,
    pub enums: BTreeMap<String, TypedEnum>,
    /// The exporting module's own generic instantiations, keyed by
    /// `generics::canonical_key`'s spelling *as the exporter spelled it*.
    /// An importing module that instantiates an imported generic
    /// registers its own instantiation under its own local spelling
    /// (`generics::check` runs per module), so this map is a fallback
    /// for an instantiation only the exporter ever built.
    pub instantiations: BTreeMap<String, TypedInstantiation>,
    /// The fail-closed half (plans/M9.md item A1b, decision 15): every
    /// declaration name in the build closure that this module's comptime
    /// evaluator **cannot** resolve, mapped to the sentence explaining
    /// why. The evaluator walks one `TypedProgram`'s name tables and has
    /// no notion of which module a body came from, so two shapes are out
    /// of reach and both are recorded here rather than left to abandon
    /// with `internal error:` (a bug by house rule, CLAUDE.md):
    ///
    /// 1. A name only some *other* module of the closure declares and
    ///    this module does not import — reachable when an imported fn's
    ///    own body refers to a helper/const/type private to its own
    ///    module.
    /// 2. A name this module *does* import, but whose exporting module
    ///    declares some other name this module also declares — splicing
    ///    that body in would make the body's own reference to that other
    ///    name silently resolve to this module's declaration instead of
    ///    its own. The splice is withheld rather than allowed to produce
    ///    a wrong value.
    ///
    /// Consulted by `eval::interp` at every lookup that can miss, and
    /// nowhere else.
    pub unresolvable: BTreeMap<String, String>,
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
        TypedStmtKind::ComptimeAssert { cond, message, .. } => {
            push_line(out, depth, "ComptimeAssert");
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
        TypedStmtKind::BareSend { expr, .. } => {
            // The span is deliberately not rendered — every other node in
            // this dump is span-free (decision 1) and a golden must not
            // start moving when an unrelated line above it shifts.
            push_line(out, depth, "BareSend");
            dump_expr(expr, depth + 1, out);
        }
        TypedStmtKind::WithGroup {
            capacity,
            deadline,
            as_name,
            body,
        } => {
            let mut header = "WithGroup".to_string();
            if let Some(name) = as_name {
                header.push_str(&format!(" as={name}"));
            }
            push_line(out, depth, &header);
            if let Some(c) = capacity {
                push_line(out, depth + 1, "Capacity");
                dump_expr(c, depth + 2, out);
            }
            if let Some(d) = deadline {
                push_line(out, depth + 1, "Deadline");
                dump_expr(d, depth + 2, out);
            }
            push_line(out, depth + 1, "Body");
            dump_stmts(body, depth + 2, out);
        }
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
        TypedExprKind::Intrinsic {
            key,
            receiver,
            type_arg,
            args,
        } => {
            let mut header = format!("Intrinsic key={key}");
            if let Some(ta) = type_arg {
                header.push_str(&format!(" type_arg={}", ty(ta)));
            }
            header.push_str(&format!(" ty={t}"));
            push_line(out, depth, &header);
            if let Some(r) = receiver {
                push_line(out, depth + 1, "Receiver");
                dump_expr(r, depth + 2, out);
            }
            for (label, val) in args {
                push_line(out, depth + 1, &format!("Arg label={label}"));
                dump_expr(val, depth + 2, out);
            }
        }
        TypedExprKind::PoolName(name) => {
            push_line(out, depth, &format!("PoolName name={name} ty={t}"));
        }
        TypedExprKind::Await(inner) => {
            push_line(out, depth, &format!("Await ty={t}"));
            dump_expr(inner, depth + 1, out);
        }
        TypedExprKind::Send(inner) => {
            push_line(out, depth, &format!("Send ty={t}"));
            dump_expr(inner, depth + 1, out);
        }
        TypedExprKind::GroupChild(key) => {
            push_line(
                out,
                depth,
                &format!("GroupChild key={} ty={t}", key.spelling()),
            );
        }
    }
}

// --- plans/M9.md item DD: re-key an imported struct under its local alias --
//
// Decision 9: an imported type resolves to `Type::Named(<local name>)`.
// The `ModuleCtx` / `TypedProgram` splices install the declaration under
// that local key, but the *body* of a spliced `TypedStruct` was typed in
// the exporting module, so every `Type::Named` / `CalleeKey::Method` /
// `StructLiteral` still carries the exporter's spelling. Evaluating
// `Duo(...).sum()` then looked up `Pair` inside the method body and hit
// A1b's `unresolvable` table (`Pair` is not a name the importer bound).
// One rewrite at the typed splice, keyed by the same `(from, to)` the
// import binding already knows — not a second lookup path at every
// consumer.

/// Re-key a spliced `TypedStruct` from the exporter's spelling to the
/// importer's local (possibly aliased) spelling. No-op when they match.
pub(crate) fn rekey_struct_name(s: &mut TypedStruct, from: &str, to: &str) {
    if from == to {
        return;
    }
    if s.name == from {
        s.name = to.to_string();
    }
    for ty in s.field_types.values_mut() {
        rekey_type(ty, from, to);
    }
    for e in s.field_defaults.values_mut() {
        rekey_expr(e, from, to);
    }
    for f in s.methods.values_mut() {
        rekey_fn(f, from, to);
    }
    for f in s.assoc_fns.values_mut() {
        rekey_fn(f, from, to);
    }
    if let Some(f) = s.init.as_mut() {
        rekey_fn(f, from, to);
    }
}

/// plans/M9.md item B2: same local-spelling re-key as `rekey_struct_name`,
/// for a spliced `TypedEnum`'s method/associated-fn bodies.
pub(crate) fn rekey_enum_name(e: &mut TypedEnum, from: &str, to: &str) {
    if from == to {
        return;
    }
    for f in e.methods.values_mut() {
        rekey_fn(f, from, to);
    }
    for f in e.assoc_fns.values_mut() {
        rekey_fn(f, from, to);
    }
}

fn rekey_fn(f: &mut TypedFn, from: &str, to: &str) {
    if let Some((_, ty)) = f.receiver.as_mut() {
        rekey_type(ty, from, to);
    }
    for p in &mut f.params {
        rekey_type(&mut p.ty, from, to);
        if let Some(d) = p.default.as_mut() {
            rekey_expr(d, from, to);
        }
    }
    rekey_type(&mut f.ret, from, to);
    for st in &mut f.body {
        rekey_stmt(st, from, to);
    }
}

fn rekey_type(ty: &mut Type, from: &str, to: &str) {
    match ty {
        Type::Array(elem, _) => rekey_type(elem, from, to),
        Type::Tuple(elems) => {
            for e in elems {
                rekey_type(e, from, to);
            }
        }
        Type::Option(inner) => rekey_type(inner, from, to),
        Type::Result(ok, err) => {
            rekey_type(ok, from, to);
            rekey_type(err, from, to);
        }
        Type::Own(_, inner) | Type::Static(inner) => rekey_type(inner, from, to),
        Type::Fn(params, ret) => {
            for (_, p) in params {
                rekey_type(p, from, to);
            }
            rekey_type(ret, from, to);
        }
        Type::Named(name, targs) => {
            if name == from {
                *name = to.to_string();
            }
            for a in targs {
                rekey_type_arg(a, from, to);
            }
        }
        Type::Bytes(_)
        | Type::Bool
        | Type::U8
        | Type::U16
        | Type::U32
        | Type::U64
        | Type::Usize
        | Type::I8
        | Type::I16
        | Type::I32
        | Type::I64
        | Type::Isize
        | Type::F32
        | Type::F64
        | Type::Char
        | Type::Unit
        | Type::Never
        | Type::Str
        | Type::Generic(_) => {}
    }
}

fn rekey_type_arg(a: &mut types::TypeArg, from: &str, to: &str) {
    match a {
        types::TypeArg::Type(t) => rekey_type(t, from, to),
        types::TypeArg::Const(_) | types::TypeArg::Bound(_) | types::TypeArg::Pool(_) => {}
    }
}

fn rekey_callee(key: &mut CalleeKey, from: &str, to: &str) {
    match key {
        CalleeKey::Method(sname, _) if sname == from => *sname = to.to_string(),
        // Instantiation keys keep the exporter's `canonical_key` spelling
        // (ImportedDecls::instantiations is keyed that way); do not touch.
        CalleeKey::Fn(_)
        | CalleeKey::FnInstance(_)
        | CalleeKey::Method(_, _)
        | CalleeKey::MethodInstance(_, _) => {}
    }
}

fn rekey_expr(e: &mut TypedExpr, from: &str, to: &str) {
    rekey_type(&mut e.ty, from, to);
    match &mut e.kind {
        TypedExprKind::Int(_)
        | TypedExprKind::Float(_)
        | TypedExprKind::Str(_)
        | TypedExprKind::BStr(_)
        | TypedExprKind::Char(_)
        | TypedExprKind::Bool(_)
        | TypedExprKind::Unit
        | TypedExprKind::Local(_)
        | TypedExprKind::Const(_)
        | TypedExprKind::PoolName(_) => {}
        TypedExprKind::FnRef(key) | TypedExprKind::GroupChild(key) => rekey_callee(key, from, to),
        TypedExprKind::Field(base, _)
        | TypedExprKind::ToScalar(base)
        | TypedExprKind::Neg(base)
        | TypedExprKind::BitNot(base)
        | TypedExprKind::Take(base)
        | TypedExprKind::Not(base)
        | TypedExprKind::Panic(base)
        | TypedExprKind::Await(base)
        | TypedExprKind::Send(base) => rekey_expr(base, from, to),
        TypedExprKind::Index(base, idx) => {
            rekey_expr(base, from, to);
            rekey_expr(idx, from, to);
        }
        TypedExprKind::Call {
            callee,
            receiver,
            args,
        } => {
            rekey_callee(callee, from, to);
            if let Some(r) = receiver {
                rekey_expr(r, from, to);
            }
            for a in args {
                if let Some(a) = a {
                    rekey_expr(a, from, to);
                }
            }
        }
        TypedExprKind::CallValue(f, args) => {
            rekey_expr(f, from, to);
            for a in args {
                rekey_expr(a, from, to);
            }
        }
        TypedExprKind::Try(inner, conv) => {
            rekey_expr(inner, from, to);
            if let Some(key) = conv {
                rekey_callee(key, from, to);
            }
        }
        TypedExprKind::Binary(_, l, r) | TypedExprKind::And(l, r) | TypedExprKind::Or(l, r) => {
            rekey_expr(l, from, to);
            rekey_expr(r, from, to);
        }
        TypedExprKind::OpCall(key, l, r) => {
            rekey_callee(key, from, to);
            rekey_expr(l, from, to);
            rekey_expr(r, from, to);
        }
        TypedExprKind::Is(inner, pat) => {
            rekey_expr(inner, from, to);
            rekey_pattern(pat, from, to);
        }
        TypedExprKind::EnumConstruct {
            enum_name, args, ..
        } => {
            if enum_name == from {
                *enum_name = to.to_string();
            }
            for a in args {
                rekey_expr(a, from, to);
            }
        }
        TypedExprKind::Closure { params, body } => {
            for p in params {
                rekey_type(&mut p.ty, from, to);
            }
            match body {
                TypedClosureBody::Expr(e) => rekey_expr(e, from, to),
                TypedClosureBody::Suite(stmts) => {
                    for st in stmts {
                        rekey_stmt(st, from, to);
                    }
                }
            }
        }
        TypedExprKind::Tuple(items) | TypedExprKind::List(items) => {
            for i in items {
                rekey_expr(i, from, to);
            }
        }
        TypedExprKind::StructLiteral { name, fields } => {
            if name == from {
                *name = to.to_string();
            }
            for (_, f) in fields {
                rekey_expr(f, from, to);
            }
        }
        TypedExprKind::Intrinsic {
            receiver,
            type_arg,
            args,
            ..
        } => {
            if let Some(r) = receiver {
                rekey_expr(r, from, to);
            }
            if let Some(t) = type_arg {
                rekey_type(t, from, to);
            }
            for (_, a) in args {
                rekey_expr(a, from, to);
            }
        }
    }
}

fn rekey_pattern(p: &mut TypedPattern, from: &str, to: &str) {
    rekey_type(&mut p.ty, from, to);
    match &mut p.kind {
        TypedPatternKind::Wildcard | TypedPatternKind::Binding(_) => {}
        TypedPatternKind::Literal(e) => rekey_expr(e, from, to),
        TypedPatternKind::Take(inner) => rekey_pattern(inner, from, to),
        TypedPatternKind::Variant {
            enum_name, payload, ..
        } => {
            if enum_name == from {
                *enum_name = to.to_string();
            }
            for sp in payload {
                rekey_pattern(sp, from, to);
            }
        }
        TypedPatternKind::Tuple(items) | TypedPatternKind::Array(items) => {
            for sp in items {
                rekey_pattern(sp, from, to);
            }
        }
        TypedPatternKind::Or(alts) => {
            for a in alts {
                rekey_pattern(a, from, to);
            }
        }
    }
}

fn rekey_stmt(st: &mut TypedStmt, from: &str, to: &str) {
    match &mut st.kind {
        TypedStmtKind::Let { ty, value, .. } => {
            rekey_type(ty, from, to);
            rekey_expr(value, from, to);
        }
        TypedStmtKind::Assign { target, value } => {
            rekey_expr(target, from, to);
            rekey_expr(value, from, to);
        }
        TypedStmtKind::If {
            cond,
            then_branch,
            elifs,
            else_branch,
        } => {
            rekey_expr(cond, from, to);
            for s in then_branch {
                rekey_stmt(s, from, to);
            }
            for e in elifs {
                rekey_expr(&mut e.cond, from, to);
                for s in &mut e.body {
                    rekey_stmt(s, from, to);
                }
            }
            if let Some(body) = else_branch {
                for s in body {
                    rekey_stmt(s, from, to);
                }
            }
        }
        TypedStmtKind::Match { scrutinee, arms } => {
            rekey_expr(scrutinee, from, to);
            for arm in arms {
                rekey_pattern(&mut arm.pattern, from, to);
                if let Some(g) = arm.guard.as_mut() {
                    rekey_expr(g, from, to);
                }
                for s in &mut arm.body {
                    rekey_stmt(s, from, to);
                }
            }
        }
        TypedStmtKind::For {
            elem_ty,
            iter,
            body,
            ..
        } => {
            rekey_type(elem_ty, from, to);
            match iter {
                TypedForIter::Range(a, b, _) => {
                    rekey_expr(a, from, to);
                    rekey_expr(b, from, to);
                }
                TypedForIter::Expr(e) => rekey_expr(e, from, to),
            }
            for s in body {
                rekey_stmt(s, from, to);
            }
        }
        TypedStmtKind::While { cond, body } => {
            rekey_expr(cond, from, to);
            for s in body {
                rekey_stmt(s, from, to);
            }
        }
        TypedStmtKind::Break | TypedStmtKind::Continue | TypedStmtKind::Pass => {}
        TypedStmtKind::Return(v) => {
            if let Some(e) = v {
                rekey_expr(e, from, to);
            }
        }
        TypedStmtKind::Assert { cond, message }
        | TypedStmtKind::ComptimeAssert { cond, message, .. } => {
            rekey_expr(cond, from, to);
            if let Some(m) = message {
                rekey_expr(m, from, to);
            }
        }
        TypedStmtKind::Defer(body) => match body {
            TypedDeferBody::Expr(e) => rekey_expr(e, from, to),
            TypedDeferBody::Suite(stmts) => {
                for s in stmts {
                    rekey_stmt(s, from, to);
                }
            }
        },
        TypedStmtKind::ExprStmt(e) | TypedStmtKind::BareSend { expr: e, .. } => {
            rekey_expr(e, from, to);
        }
        TypedStmtKind::WithGroup {
            capacity,
            deadline,
            body,
            ..
        } => {
            if let Some(c) = capacity {
                rekey_expr(c, from, to);
            }
            if let Some(d) = deadline {
                rekey_expr(d, from, to);
            }
            for s in body {
                rekey_stmt(s, from, to);
            }
        }
    }
}
