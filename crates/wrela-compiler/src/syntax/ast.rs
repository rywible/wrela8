#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Span {
    pub line: u32,
    pub col: u32,
    pub byte_start: usize,
    pub byte_end: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Doc {
    pub span: Span,
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Attr {
    pub span: Span,
    pub name: String,
    pub args: Vec<Arg>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Module {
    pub span: Span,
    pub path: Vec<String>,
    pub doc: Option<Doc>,
    pub imports: Vec<Import>,
    pub items: Vec<Item>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImportName {
    pub span: Span,
    pub name: String,
    pub alias: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Import {
    pub span: Span,
    pub is_pub: bool,
    pub path: Vec<String>,
    pub names: Vec<ImportName>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Item {
    Const(ConstItem),
    Static(StaticItem),
    Fn(FnItem),
    Struct(StructItem),
    Enum(EnumItem),
    Pool(PoolItem),
    ComptimeIf(ComptimeIfItem),
}

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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StaticItem {
    pub span: Span,
    pub name: String,
    pub is_pub: bool,
    pub doc: Option<Doc>,
    pub attrs: Vec<Attr>,
    pub ty: Type,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GenericParam {
    Type { span: Span, name: String },
    Const { span: Span, name: String, ty: Type },
}

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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Receiver {
    pub span: Span,
    pub mode: Option<AccessMode>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Param {
    pub span: Span,
    pub mode: AccessMode,
    pub name: String,
    pub ty: Type,
    pub default: Option<Expr>,
}

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

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Member {
    Field(FieldItem),
    Fn(FnItem),
    Init(InitItem),
    Pool(PoolItem),
    ComptimeIf(ComptimeIfMember),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StructItem {
    pub span: Span,
    pub name: String,
    pub is_pub: bool,
    pub is_resource: bool,
    pub is_manual_resource: bool,
    pub doc: Option<Doc>,
    pub attrs: Vec<Attr>,
    pub generics: Vec<GenericParam>,
    pub deriving: Vec<String>,
    pub members: Vec<Member>,
}

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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PoolItem {
    pub span: Span,
    pub name: String,
    pub doc: Option<Doc>,
    pub attrs: Vec<Attr>,
}

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

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GenericArg {
    Type(Type),
    Bound(Expr),
    Expr(Expr),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NamedType {
    pub span: Span,
    pub name: String,
    pub args: Vec<GenericArg>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArrayType {
    pub span: Span,
    pub elem: Type,
    pub len: Expr,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TupleType {
    pub span: Span,
    pub elems: Vec<Type>,
}

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

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Pattern {
    Wildcard(Span),
    Literal(Span, Expr),
    Binding(Span, String),
    Take(Span, Box<Pattern>),
    Variant {
        span: Span,
        enum_name: Option<String>,
        variant: String,
        payload: Vec<Pattern>,
    },
    Tuple(Span, Vec<Pattern>),
    Array(Span, Vec<Pattern>),
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
    Index(Box<Expr>, Span, Vec<Expr>),
    Call(Box<Expr>, Span, Vec<Arg>),
    Unary(Span, UnaryOp, Box<Expr>),
    Try(Span, Box<Expr>),
    Binary(Span, BinOp, Box<Expr>, Box<Expr>),
    Range(Span, Box<Expr>, Box<Expr>, bool),
    Is(Span, Box<Expr>, Box<Pattern>),
    Not(Span, Box<Expr>),
    And(Span, Box<Expr>, Box<Expr>),
    Or(Span, Box<Expr>, Box<Expr>),
    DotVariant(Span, String, Vec<Arg>),
    Closure(ClosureExpr),
    Send(Span, Box<Expr>),
    Tuple(Span, Vec<Expr>),
    List(Span, Vec<Expr>),
    ArrayRepeat(Span, Box<Expr>, Box<Expr>),
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
            | Expr::List(s, _)
            | Expr::ArrayRepeat(s, _, _) => *s,
            Expr::Field(_, s, _) => *s,
            Expr::FStr(f) => f.span,
            Expr::Closure(c) => c.span,
        }
    }
}

pub fn is_place_expr(expr: &Expr) -> bool {
    matches!(expr, Expr::Name(..) | Expr::Field(..) | Expr::Index(..))
}

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
    pub discard: Option<Attr>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ForStmt {
    pub span: Span,
    pub take_binding: bool,
    pub name: String,
    pub iterable: Expr,
    pub body: Vec<Stmt>,
    pub budget: Option<Attr>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WhileStmt {
    pub span: Span,
    pub cond: Expr,
    pub body: Vec<Stmt>,
    pub budget: Option<Attr>,
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
    Send(Span, Expr),
    Expr(Span, Expr),
    Dmb(Attr),
    ComptimeIf(ComptimeIfStmt),
    ComptimeAssert(Span, Expr, Option<Expr>),
}
