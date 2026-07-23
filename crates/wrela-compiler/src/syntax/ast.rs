//! The abstract syntax tree (02-language.md, whole chapter).
//!
//! Shape decisions frozen by plans/M1.md (item C): plain enums/structs, one
//! file, no arena — `Box` for recursion, `Vec` for sequences, nothing
//! generic (decision 4); every node carries a `line:col` span, u32 pairs, no
//! byte offsets (decision 3).
//!
//! This is the *spine* the parser skeleton (parser.rs) produces: a `Module`
//! with its imports fully understood, and one header-only placeholder per
//! top-level declaration form. Full grammar (plans/M1.md item D) replaces
//! each placeholder's `todo` field with real content — the field exists so
//! today's dump is honest about exactly how much of each declaration was
//! actually parsed versus skipped.

/// A source position, `(line, col)`, both 1-based — matches lexer::Token.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Span {
    pub line: u32,
    pub col: u32,
}

/// A `##` doc comment attached to the declaration (or module) immediately
/// following it. Consecutive `##` lines join with `\n`; `text` has the one
/// conventional leading space (`## text`) stripped, if present.
#[derive(Debug, Clone)]
pub struct Doc {
    pub span: Span,
    pub text: String,
}

/// A skipped-and-recorded `@name(...)` (or bare `@name`) attribute. Item C
/// does not interpret attributes; it only recognizes and preserves them as
/// raw text for item D.
#[derive(Debug, Clone)]
pub struct Attr {
    pub span: Span,
    pub text: String,
}

/// One `module path.name` file.
#[derive(Debug, Clone)]
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
#[derive(Debug, Clone)]
pub struct ImportName {
    pub span: Span,
    pub name: String,
    pub alias: Option<String>,
}

/// `[pub] from path.to.module import Name [as Alias][, ...]`, including the
/// parenthesized multi-line list form (02-language.md §2).
#[derive(Debug, Clone)]
pub struct Import {
    pub span: Span,
    pub is_pub: bool,
    /// Dotted `from` path segments.
    pub path: Vec<String>,
    pub names: Vec<ImportName>,
}

/// One top-level declaration. Item C recognizes the header (and, for
/// `pub`/`async`/`resource`, the flags spelled before the name) of every
/// form docs/language/02-language.md declares at module scope, then skips
/// the remainder — everything from the first unrecognized token to the end
/// of the declaration (a suite, or the rest of a one-line declaration) — and
/// records where it stopped in `todo`. Item D replaces each placeholder
/// struct with the real grammar.
#[derive(Debug, Clone)]
pub enum Item {
    Const(ConstItem),
    Fn(FnItem),
    Struct(StructItem),
    Enum(EnumItem),
    Pool(PoolItem),
    ComptimeIf(ComptimeIfItem),
}

/// `[pub] const NAME ...` (02-language.md §12: a `const` initializer is a
/// comptime context).
#[derive(Debug, Clone)]
pub struct ConstItem {
    pub span: Span,
    pub name: String,
    pub is_pub: bool,
    pub doc: Option<Doc>,
    pub attrs: Vec<Attr>,
    /// Where token-skipping started consuming the unparsed remainder of
    /// this declaration; `None` when there was nothing to skip.
    pub todo: Option<Span>,
}

/// `[pub] [async] fn NAME ...` (02-language.md §5).
#[derive(Debug, Clone)]
pub struct FnItem {
    pub span: Span,
    pub name: String,
    pub is_pub: bool,
    pub is_async: bool,
    pub doc: Option<Doc>,
    pub attrs: Vec<Attr>,
    pub todo: Option<Span>,
}

/// `[pub] [resource] struct NAME ...` (02-language.md §7.1).
#[derive(Debug, Clone)]
pub struct StructItem {
    pub span: Span,
    pub name: String,
    pub is_pub: bool,
    pub is_resource: bool,
    pub doc: Option<Doc>,
    pub attrs: Vec<Attr>,
    pub todo: Option<Span>,
}

/// `[pub] enum NAME ...` (02-language.md §7.2).
#[derive(Debug, Clone)]
pub struct EnumItem {
    pub span: Span,
    pub name: String,
    pub is_pub: bool,
    pub doc: Option<Doc>,
    pub attrs: Vec<Attr>,
    pub todo: Option<Span>,
}

/// `pool NAME` (02-language.md §4) — an image- or actor-scoped pool name
/// binding. Grammatically `pub`-less (the pool name's visibility follows its
/// owner), so there is no `is_pub` here.
#[derive(Debug, Clone)]
pub struct PoolItem {
    pub span: Span,
    pub name: String,
    pub doc: Option<Doc>,
    pub attrs: Vec<Attr>,
    pub todo: Option<Span>,
}

/// `comptime if ...` at declaration scope (02-language.md §12). No name: the
/// condition is an expression, left entirely to the skipped remainder.
#[derive(Debug, Clone)]
pub struct ComptimeIfItem {
    pub span: Span,
    pub doc: Option<Doc>,
    pub attrs: Vec<Attr>,
    pub todo: Option<Span>,
}
