//! The abstract syntax tree (02-language.md, whole chapter).
//!
//! Shape decisions frozen by plans/M1.md (item C): plain enums/structs, one
//! file, no arena — `Box` for recursion, `Vec` for sequences, nothing
//! generic (decision 4); every node carries a `line:col` span, u32 pairs, no
//! byte offsets (decision 3).
//!
//! Item D (plans/M1.md) replaces the item-D-era placeholders with the full
//! grammar of chapter 02: declarations, types, statements, expressions, and
//! patterns. Two module-scope and member-scope `comptime if` shapes exist
//! (`ComptimeIfItem` / `ComptimeIfMember`) rather than one generic node,
//! per decision 4 (nothing generic).
//!
//! F-strings (§1.1) are segmented into literal/interpolation parts at parse
//! time using the lexer's brace-balance scan, but interpolation contents are
//! **not** recursively parsed in M1 — each `Interp` keeps its raw source text
//! and span only; a later milestone parses it as an expression.
//!
//! plans/M2.md item B adds `PartialEq, Eq` throughout (every shape above is
//! unchanged; nothing here stops being "plain"): sema's `Type` (decision 4 —
//! `derive(PartialEq, Eq, Clone, Debug)`, structural, no interning) keeps
//! array lengths, `Bytes[N]`'s length, and generic const arguments as
//! unevaluated `Expr`s embedded directly in itself rather than evaluating
//! them early (item H evaluates the literal subset), so `Expr` — and
//! everything it recursively reaches, `Stmt`/`Pattern` included, via
//! `Closure`/`Is` — needs the same derives for `Type`'s own to compile.

/// A source position, `(line, col)`, both 1-based — matches lexer::Token.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Span {
    pub line: u32,
    pub col: u32,
}

/// A `##` doc comment attached to the declaration (or module) immediately
/// following it. Consecutive `##` lines join with `\n`; `text` has the one
/// conventional leading space (`## text`) stripped, if present.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Doc {
    pub span: Span,
    pub text: String,
}

/// `@name` or `@name(arg, key=value, ...)` (02-language.md §13). Arguments
/// share the call-argument shape (`Arg`) minus the `mut`/`take` mirroring,
/// which is meaningless for comptime attribute arguments.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Attr {
    pub span: Span,
    pub name: String,
    pub args: Vec<Arg>,
}

/// One `module path.name` file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Module {
    pub span: Span,
    /// Dotted path segments, e.g. `["examples", "tokens"]`.
    pub path: Vec<String>,
    /// A doc comment directly preceding the `module` header, if any.
    pub doc: Option<Doc>,
    pub imports: Vec<Import>,
    pub items: Vec<Item>,
}

/// One imported name, with its optional `as Alias`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImportName {
    pub span: Span,
    pub name: String,
    pub alias: Option<String>,
}

/// `[pub] from path.to.module import Name [as Alias][, ...]`, including the
/// parenthesized multi-line list form (02-language.md §2).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Import {
    pub span: Span,
    pub is_pub: bool,
    /// Dotted `from` path segments.
    pub path: Vec<String>,
    pub names: Vec<ImportName>,
}

/// One top-level declaration (02-language.md §§4-7, §12).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Item {
    Const(ConstItem),
    Fn(FnItem),
    Struct(StructItem),
    Enum(EnumItem),
    Pool(PoolItem),
    ComptimeIf(ComptimeIfItem),
}

/// `[pub] const NAME [: Type] = expr` (02-language.md §12: a `const`
/// initializer is a comptime context).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConstItem {
    pub span: Span,
    pub name: String,
    pub is_pub: bool,
    pub doc: Option<Doc>,
    pub attrs: Vec<Attr>,
    pub ty: Option<Type>,
    pub value: Expr,
}

/// A compile-time generic parameter: `T` (type) or `const N: usize` (value).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GenericParam {
    Type { span: Span, name: String },
    Const { span: Span, name: String, ty: Type },
}

/// `read` (the unwritten default) / `mut` / `take` (02-language.md §3).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AccessMode {
    Read,
    Mut,
    Take,
}

impl AccessMode {
    pub fn as_str(self) -> &'static str {
        match self {
            AccessMode::Read => "read",
            AccessMode::Mut => "mut",
            AccessMode::Take => "take",
        }
    }
}

/// The receiver parameter of a method: `read self` / `mut self` / `take
/// self`, or bare `self` (read, the unwritten default — 02-language.md
/// §5.1).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Receiver {
    pub span: Span,
    pub mode: AccessMode,
}

/// One non-receiver parameter: `[mode] name: Type [= default]`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Param {
    pub span: Span,
    pub mode: AccessMode,
    pub name: String,
    pub ty: Type,
    pub default: Option<Expr>,
}

/// `[pub] [async] fn NAME[generics](params) -> Ret: body` (02-language.md
/// §5). `body` is `None` only for the rare bodyless signature shorthand a
/// few doc tables use to describe a desugar target (05-language.md §8) —
/// see the parser's `parse_fn_common` doc comment for the fail-closed
/// reasoning; every real declaration has a body.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FnItem {
    pub span: Span,
    pub name: String,
    pub is_pub: bool,
    pub is_async: bool,
    pub doc: Option<Doc>,
    pub attrs: Vec<Attr>,
    pub generics: Vec<GenericParam>,
    pub receiver: Option<Receiver>,
    pub params: Vec<Param>,
    pub ret: Option<Type>,
    pub body: Option<Vec<Stmt>>,
}

/// `init(mut self, ...) [-> Result[unit, E]]: body` (02-language.md §7.1).
/// Never `pub`, never generic — deliberate, per the docs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InitItem {
    pub span: Span,
    pub doc: Option<Doc>,
    pub attrs: Vec<Attr>,
    pub receiver: Receiver,
    pub params: Vec<Param>,
    pub ret: Option<Type>,
    pub body: Vec<Stmt>,
}

/// `[pub] name: Type [= default]` struct field, with its own attrs (field-
/// position attributes like `@offset(0x060)` land here, 02-language.md
/// §13/03-hardware.md).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FieldItem {
    pub span: Span,
    pub name: String,
    pub is_pub: bool,
    pub doc: Option<Doc>,
    pub attrs: Vec<Attr>,
    pub ty: Type,
    pub default: Option<Expr>,
}

/// One member of a `struct`/`resource struct` body.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Member {
    Field(FieldItem),
    Fn(FnItem),
    Init(InitItem),
    Pool(PoolItem),
    ComptimeIf(ComptimeIfMember),
}

/// `[pub] [resource] struct NAME[generics] [deriving(...)]:` body
/// (02-language.md §7.1, §7.5).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StructItem {
    pub span: Span,
    pub name: String,
    pub is_pub: bool,
    pub is_resource: bool,
    pub doc: Option<Doc>,
    pub attrs: Vec<Attr>,
    pub generics: Vec<GenericParam>,
    pub deriving: Vec<String>,
    pub members: Vec<Member>,
}

/// A closed-sum variant payload: none, a positional tuple, or named fields
/// (both forms bind positionally in match patterns — 02-language.md §7.2).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VariantPayload {
    None,
    Tuple(Vec<Type>),
    Named(Vec<VariantField>),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VariantField {
    pub span: Span,
    pub name: String,
    pub ty: Type,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Variant {
    pub span: Span,
    pub name: String,
    pub payload: VariantPayload,
}

/// `[pub] enum NAME[generics] [deriving(...)]:` body (02-language.md §7.2,
/// §7.5). Variants and methods/associated fns may interleave; the parser
/// distinguishes them by the next token (`fn`/`pub`/`async` vs an ident).
/// `members` holds only `Member::Fn` — an enum has no fields/`init`/`pool`
/// (plans/M9.md item B2).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EnumItem {
    pub span: Span,
    pub name: String,
    pub is_pub: bool,
    pub doc: Option<Doc>,
    pub attrs: Vec<Attr>,
    pub generics: Vec<GenericParam>,
    pub deriving: Vec<String>,
    pub variants: Vec<Variant>,
    pub members: Vec<Member>,
}

/// `pool NAME` (02-language.md §4) — an image- or actor-scoped pool name
/// binding. Grammatically `pub`-less (the pool name's visibility follows its
/// owner), so there is no `is_pub` here.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PoolItem {
    pub span: Span,
    pub name: String,
    pub doc: Option<Doc>,
    pub attrs: Vec<Attr>,
}

/// `comptime if ... : ... [comptime else: ...]` at declaration scope
/// (02-language.md §12). Mirrors `ComptimeIfMember` at member scope and
/// `ComptimeIfStmt` at statement scope — three near-identical shapes over
/// three different content types, per decision 4 (nothing generic in the
/// AST).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ComptimeIfItem {
    pub span: Span,
    pub doc: Option<Doc>,
    pub attrs: Vec<Attr>,
    pub cond: Expr,
    pub then_branch: Vec<Item>,
    pub else_branch: Option<Vec<Item>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ComptimeIfMember {
    pub span: Span,
    pub doc: Option<Doc>,
    pub attrs: Vec<Attr>,
    pub cond: Expr,
    pub then_branch: Vec<Member>,
    pub else_branch: Option<Vec<Member>>,
}

// --- types (02-language.md §6) ---------------------------------------------

/// A generic argument at a type's use site: a type, a bounded-occupancy
/// marker (`..N`), or a plain comptime expression (an integer/const-name
/// argument, or a data-carrying expression like `256.KiB`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GenericArg {
    Type(Type),
    Bound(Expr),
    Expr(Expr),
}

/// `Name[args...]` — a named type, possibly with generic arguments
/// (`u64`, `never`, `unit`, `BootError`, `Option[T]`, `Bytes[..N]`, ...).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NamedType {
    pub span: Span,
    pub name: String,
    pub args: Vec<GenericArg>,
}

/// `[T; N]` — fixed array type.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArrayType {
    pub span: Span,
    pub elem: Type,
    pub len: Expr,
}

/// `(A, B)` / one-element `(T,)` — tuple type.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TupleType {
    pub span: Span,
    pub elems: Vec<Type>,
}

/// `own[P] T` — pool handle (02-language.md §4). `pool` is the dotted pool
/// path (`Name` or `Owner.Name`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OwnType {
    pub span: Span,
    pub pool: Vec<String>,
    pub inner: Type,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FnTypeParam {
    pub span: Span,
    pub mode: AccessMode,
    pub ty: Type,
}

/// `fn(read T, mut U) -> R` — function type (02-language.md §8.3).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FnType {
    pub span: Span,
    pub params: Vec<FnTypeParam>,
    pub ret: Option<Type>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Type {
    Named(NamedType),
    Array(Box<ArrayType>),
    Tuple(TupleType),
    Own(Box<OwnType>),
    Fn(Box<FnType>),
}

impl Type {
    pub fn span(&self) -> Span {
        match self {
            Type::Named(t) => t.span,
            Type::Array(t) => t.span,
            Type::Tuple(t) => t.span,
            Type::Own(t) => t.span,
            Type::Fn(t) => t.span,
        }
    }
}

// --- patterns (02-language.md §7.2) -----------------------------------------

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Pattern {
    Wildcard(Span),
    /// A literal pattern, including a unary-negated numeric literal.
    Literal(Span, Expr),
    Binding(Span, String),
    /// `take` in payload position, wrapping the bound name.
    Take(Span, Box<Pattern>),
    /// `.Name(...)` or `Enum.Name(...)`; payload binds positionally
    /// regardless of whether the variant was declared with named fields.
    Variant {
        span: Span,
        enum_name: Option<String>,
        variant: String,
        payload: Vec<Pattern>,
    },
    Tuple(Span, Vec<Pattern>),
    Array(Span, Vec<Pattern>),
    /// `p1 | p2 | ...` — same bindings, same types, in every alternative.
    Or(Span, Vec<Pattern>),
}

impl Pattern {
    pub fn span(&self) -> Span {
        match self {
            Pattern::Wildcard(s)
            | Pattern::Literal(s, _)
            | Pattern::Binding(s, _)
            | Pattern::Take(s, _)
            | Pattern::Variant { span: s, .. }
            | Pattern::Tuple(s, _)
            | Pattern::Array(s, _)
            | Pattern::Or(s, _) => *s,
        }
    }
}

// --- expressions (02-language.md §8.2) --------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnaryOp {
    Neg,
    BitNot,
    Await,
    Take,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BinOp {
    Mul,
    Div,
    Rem,
    MulW,
    Add,
    Sub,
    AddW,
    SubW,
    Shl,
    Shr,
    BitAnd,
    BitOr,
    BitXor,
    Lt,
    Le,
    Gt,
    Ge,
    Eq,
    Ne,
}

impl BinOp {
    pub fn as_str(self) -> &'static str {
        match self {
            BinOp::Mul => "*",
            BinOp::Div => "/",
            BinOp::Rem => "%",
            BinOp::MulW => "*%",
            BinOp::Add => "+",
            BinOp::Sub => "-",
            BinOp::AddW => "+%",
            BinOp::SubW => "-%",
            BinOp::Shl => "<<",
            BinOp::Shr => ">>",
            BinOp::BitAnd => "&",
            BinOp::BitOr => "|",
            BinOp::BitXor => "^",
            BinOp::Lt => "<",
            BinOp::Le => "<=",
            BinOp::Gt => ">",
            BinOp::Ge => ">=",
            BinOp::Eq => "==",
            BinOp::Ne => "!=",
        }
    }
}

/// One call/constructor argument: `[label=][mut|take] value`. The mirrored
/// `mut`/`take` marker's operand must be a place expression (name, field,
/// index, or a parenthesized place — parens are transparent, dropped at
/// parse time) — checked where the argument is parsed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Arg {
    pub span: Span,
    pub label: Option<String>,
    pub mode: AccessMode,
    pub value: Expr,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClosureParam {
    pub span: Span,
    pub mode: AccessMode,
    pub name: String,
    pub ty: Option<Type>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ClosureBody {
    Expr(Box<Expr>),
    Suite(Vec<Stmt>),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClosureExpr {
    pub span: Span,
    pub params: Vec<ClosureParam>,
    pub body: ClosureBody,
}

/// One segment of an interpolated string literal. `Interp` keeps the
/// interior as raw text at parse time; `sema::fstring::desugar_fstring`
/// (plans/M9.md item D) parses it and rewrites the f-string onto
/// `.format()` + `String` concat before typing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FStringPart {
    Literal(Span, String),
    Interp(Span, String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FStringLit {
    pub span: Span,
    pub parts: Vec<FStringPart>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Expr {
    Int(Span, String),
    Float(Span, String),
    Str(Span, String),
    BStr(Span, String),
    Char(Span, String),
    FStr(FStringLit),
    Bool(Span, bool),
    Unit(Span),
    Name(Span, String),
    Field(Box<Expr>, Span, String),
    /// `expr[args...]` — indexing or a generic-instantiation bracket; the
    /// parser does not disambiguate the two (that is a semantic question).
    Index(Box<Expr>, Span, Vec<Expr>),
    Call(Box<Expr>, Span, Vec<Arg>),
    Unary(Span, UnaryOp, Box<Expr>),
    /// Postfix `?`.
    Try(Span, Box<Expr>),
    Binary(Span, BinOp, Box<Expr>, Box<Expr>),
    Range(Span, Box<Expr>, Box<Expr>, bool),
    Is(Span, Box<Expr>, Box<Pattern>),
    Not(Span, Box<Expr>),
    And(Span, Box<Expr>, Box<Expr>),
    Or(Span, Box<Expr>, Box<Expr>),
    /// Leading-dot enum reference in expression position: `.Variant` or
    /// `.Variant(args)`.
    DotVariant(Span, String, Vec<Arg>),
    Closure(ClosureExpr),
    /// `send actor.method(...)` used as a value (02-language.md §9.4): most
    /// often a bare statement, but `match send logger.record(...):`
    /// matches directly on the `Result`/`never` it produces, so `send` is
    /// also a prefix expression form wrapping its (always-a-call) operand.
    Send(Span, Box<Expr>),
    Tuple(Span, Vec<Expr>),
    /// `[a, b, c]` — a list literal. Not shown in 02-language.md's own
    /// grammar summary, but used throughout its examples and the virtio
    /// worked example wherever a bounded container is built from a
    /// bracketed list (feature sets, child lists, seed files); the docs are
    /// normative in the sense that they must parse, so this is treated as
    /// an ordinary primary expression alongside literals and closures.
    List(Span, Vec<Expr>),
}

impl Expr {
    pub fn span(&self) -> Span {
        match self {
            Expr::Int(s, _)
            | Expr::Float(s, _)
            | Expr::Str(s, _)
            | Expr::BStr(s, _)
            | Expr::Char(s, _)
            | Expr::Bool(s, _)
            | Expr::Unit(s)
            | Expr::Name(s, _)
            | Expr::Index(_, s, _)
            | Expr::Call(_, s, _)
            | Expr::Unary(s, _, _)
            | Expr::Try(s, _)
            | Expr::Binary(s, _, _, _)
            | Expr::Range(s, _, _, _)
            | Expr::Is(s, _, _)
            | Expr::Not(s, _)
            | Expr::And(s, _, _)
            | Expr::Or(s, _, _)
            | Expr::DotVariant(s, _, _)
            | Expr::Send(s, _)
            | Expr::Tuple(s, _)
            | Expr::List(s, _) => *s,
            Expr::Field(_, s, _) => *s,
            Expr::FStr(f) => f.span,
            Expr::Closure(c) => c.span,
        }
    }
}

/// Is `expr` a place (name, field, index, or a parenthesized place — parens
/// are dropped at parse time so this needs no explicit case for them):
/// 02-language.md §3/§5.1, the operand of a mirrored `mut`/`take` marker.
pub fn is_place_expr(expr: &Expr) -> bool {
    matches!(expr, Expr::Name(..) | Expr::Field(..) | Expr::Index(..))
}

// --- statements (02-language.md §8.1, §9.4, §10) ----------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AssignOp {
    Assign,
    Add,
    Sub,
    Mul,
    Div,
    Rem,
    BitAnd,
    BitOr,
    BitXor,
    Shl,
    Shr,
}

impl AssignOp {
    pub fn as_str(self) -> &'static str {
        match self {
            AssignOp::Assign => "=",
            AssignOp::Add => "+=",
            AssignOp::Sub => "-=",
            AssignOp::Mul => "*=",
            AssignOp::Div => "/=",
            AssignOp::Rem => "%=",
            AssignOp::BitAnd => "&=",
            AssignOp::BitOr => "|=",
            AssignOp::BitXor => "^=",
            AssignOp::Shl => "<<=",
            AssignOp::Shr => ">>=",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AssignStmt {
    pub span: Span,
    pub target: Expr,
    pub ty: Option<Type>,
    pub op: AssignOp,
    pub value: Expr,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ElifClause {
    pub span: Span,
    pub cond: Expr,
    pub body: Vec<Stmt>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IfStmt {
    pub span: Span,
    pub cond: Expr,
    pub then_branch: Vec<Stmt>,
    pub elifs: Vec<ElifClause>,
    pub else_branch: Option<Vec<Stmt>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MatchArm {
    pub span: Span,
    pub pattern: Pattern,
    pub guard: Option<Expr>,
    pub body: Vec<Stmt>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MatchStmt {
    pub span: Span,
    pub scrutinee: Expr,
    pub arms: Vec<MatchArm>,
}

/// `for [take] name in iterable: body` (02-language.md §8.1).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ForStmt {
    pub span: Span,
    pub take_binding: bool,
    pub name: String,
    pub iterable: Expr,
    pub body: Vec<Stmt>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WhileStmt {
    pub span: Span,
    pub cond: Expr,
    pub body: Vec<Stmt>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AssertStmt {
    pub span: Span,
    pub cond: Expr,
    pub message: Option<Expr>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DeferBody {
    Expr(Box<Expr>),
    Suite(Vec<Stmt>),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeferStmt {
    pub span: Span,
    pub body: DeferBody,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WithStmt {
    pub span: Span,
    pub expr: Expr,
    pub as_name: Option<String>,
    pub body: Vec<Stmt>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ComptimeIfStmt {
    pub span: Span,
    pub cond: Expr,
    pub then_branch: Vec<Stmt>,
    pub else_branch: Option<Vec<Stmt>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Stmt {
    Assign(AssignStmt),
    If(IfStmt),
    Match(MatchStmt),
    For(ForStmt),
    While(WhileStmt),
    Break(Span),
    Continue(Span),
    Return(Span, Option<Expr>),
    Pass(Span),
    Assert(AssertStmt),
    Defer(DeferStmt),
    With(WithStmt),
    /// `send actor.method(...)` (02-language.md §9.4). The operand is
    /// always a call expression.
    Send(Span, Expr),
    Expr(Span, Expr),
    ComptimeIf(ComptimeIfStmt),
    ComptimeAssert(Span, Expr, Option<Expr>),
}
