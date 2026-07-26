//! Recursive-descent parser: token stream -> `ast::Module` (02-language.md,
//! whole chapter). A hand-written `Parser` struct with `pos` plus small
//! peek/bump/expect helpers (ROADMAP.md: no combinators, no traits, no
//! lookahead machinery) — dumb and correct: one function per grammar
//! production, precedence realized as a chain of functions (parse_or ->
//! parse_and -> ... -> parse_postfix -> parse_primary), no Pratt tables.
//!
//! Item D (plans/M1.md) replaces the item-C token-skip with the full
//! grammar. Two entry points exist: `parse` (a complete `module ...` file)
//! and `parse_fragment`/`parse_any` (a bare sequence of items and/or
//! statements with no `module` header) — most illustrative code blocks in
//! docs/language/*.md are not full modules, so the corpus driver
//! (xtask's `corpus` command) needs a lenient top-level entry point too.
//! `parse_fragment` returns the parsed `FragmentEntry` sequence (not just a
//! yes/no) because item E's oracles (xtask's `fuzz parser`/`roundtrip`)
//! need real content to dump and pretty-print, exactly like a module's.
//!
//! Suite parsing note: `()[]{}` suppress NEWLINE/INDENT/DEDENT in the lexer
//! (02-language.md §1) — except a `:` immediately followed by a newline
//! opens a *layout island* (lexer.rs's module doc comment) that resumes
//! real layout tokens for exactly that suite, so a suite-form closure body
//! embedded inside an enclosing call's argument list (see
//! docs/language/examples/virtio-storage.wr, `BlockCache.edit`/`peek`)
//! parses through the ordinary NEWLINE+INDENT...DEDENT path in
//! `parse_stmt_suite`, the same as any top-level suite. The only case left
//! with no layout tokens at all is a `:` followed by real content on the
//! *same* physical line (no newline ever appears for the lexer to act on),
//! handled by `parse_inline_stmt_seq` — restricted to exactly one statement,
//! since two statements jammed onto one line with no separator token is a
//! genuine grammar ambiguity (`plans/pre-M3-findings.md`'s roundtrip-
//! ambiguity finding; ledger clause syntax.lexer.layout-islands).
//!
//! Errors are fail-fast (plans/M1.md decision 2): the first one stops
//! parsing.

use super::ast::*;
use super::lexer::{Token, TokenKind};

#[derive(Debug)]
pub struct ParseError {
    pub message: String,
    pub line: u32,
    pub col: u32,
}

/// The deepest live nesting of the expression precedence chain a single
/// parse may build (`parse_unary`'s own guard) — mirrors
/// `sema::bodies::MAX_GENERIC_DEPTH`/`eval::quota::MAX_CALL_DEPTH`'s own
/// role one layer either side of this pass: an unbounded-nesting input
/// (deeply parenthesized/bracketed groups, chained unary prefixes, or any
/// mix — every one of them re-enters the chain through `parse_unary`
/// exactly once per level) must fail closed with a diagnostic *before* it
/// overflows this process's own native stack, which nothing before this
/// guard existed to notice (found by `cargo xtask fuzz sema`, seed=11 —
/// deeply nested groups blew the native stack well before any other
/// limit fired). Chosen empirically, not measured/profiled: parsing (and
/// the downstream AST dump/pretty-print/`sema::bodies::check_expr` walks,
/// which recurse over the same shape) stayed clean through roughly 350
/// levels of plain paren-nesting on this process's default stack, and
/// broke somewhere before 400; 100 keeps a >3x margin below that observed
/// ceiling while comfortably covering any reasonable hand-written
/// expression (the deepest in the whole doc/example corpus is nowhere
/// close).
const MAX_EXPR_DEPTH: u32 = 100;

/// The deepest nesting of *indented statement blocks* a single parse may
/// build (`parse_stmts_until_dedent`'s own guard) — `MAX_EXPR_DEPTH`'s
/// sibling for the second of this parser's two recursive descents
/// (plans/M9.md item RR). The expression guard bounds only the precedence
/// chain; a body of nested `if`/`while`/`for`/`match` suites recurses
/// through `parse_stmt` -> `parse_suite` -> `parse_stmts_until_dedent`
/// instead and was unbounded, so ~800 levels aborted the process with a
/// native stack overflow — not a diagnostic, and not something the fuzz
/// lanes can even observe, since every one of their guards is
/// `std::panic::catch_unwind` and a stack-overflow abort does not unwind.
///
/// Same empirical method as `MAX_EXPR_DEPTH`, measured the same way:
/// `wrela dump --stage=ast` on a chain of nested `if true:` suites stayed
/// clean past 400 and aborted before 800, and the downstream walks that
/// recurse over the same shape (`sema::bodies::check_stmt`, the AST dump,
/// the pretty-printer) give out earlier than the parser does — `--stage=check`
/// aborted around 600. 100 keeps a comfortable margin below the *earliest*
/// of those ceilings while sitting far above any hand-written body.
const MAX_BLOCK_DEPTH: u32 = 100;

/// The deepest nesting of *type syntax* a single parse may build
/// (`parse_type`'s own guard) — the third recursive descent, and the one
/// whose downstream consumers give out first (plans/M9.md item RR).
/// `Option[Option[...]]` nested ~300 deep parsed fine but aborted
/// `sema::types::resolve_type` with a native stack overflow, and the
/// parser itself aborted before 1600; `render_type`, `size_of` and the
/// layout walks all recurse over the identical shape. Bounding the
/// *source* depth here is what bounds every one of them at once: no later
/// pass can be handed a type the parser refused to build. Generic
/// instantiation can still synthesize types deeper than the source spells
/// them, which is `sema::bodies::MAX_GENERIC_DEPTH`'s separate job.
///
/// 100 by the same margin argument as its two siblings; the deepest type
/// in the whole doc/example corpus is nowhere close.
const MAX_TYPE_DEPTH: u32 = 100;

pub fn parse(tokens: Vec<Token>) -> Result<Module, ParseError> {
    Parser::new(tokens).parse_module()
}

/// One top-level construct accepted outside a `module` header: a
/// declaration or a bare statement, interleaved freely (see
/// `parse_fragment`'s doc comment). Corpus doc-blocks and the parser
/// fuzzer's token-soup strategy (plans/M1.md item E) both need the actual
/// parsed content — not just a yes/no — so it can be dumped and
/// pretty-printed like a real module's contents.
#[derive(Debug, Clone)]
pub enum FragmentEntry {
    Item(Item),
    Stmt(Stmt),
}

/// The result of `parse_any`: a complete module, or a bare fragment.
#[derive(Debug, Clone)]
pub enum Parsed {
    Module(Module),
    Fragment(Vec<FragmentEntry>),
}

/// Parses a bare sequence of items and/or statements with no `module`
/// header — used for corpus doc-blocks that are illustrative fragments
/// rather than complete files.
pub fn parse_fragment(tokens: Vec<Token>) -> Result<Vec<FragmentEntry>, ParseError> {
    Parser::new(tokens).parse_fragment_body()
}

/// Picks `parse` or `parse_fragment` based on whether the token stream's
/// first substantive token is the `module` keyword.
pub fn parse_any(tokens: Vec<Token>) -> Result<Parsed, ParseError> {
    let is_module = tokens
        .iter()
        .find(|t| t.kind != TokenKind::DocComment)
        .map(|t| t.kind == TokenKind::Keyword && t.text == "module")
        .unwrap_or(false);
    if is_module {
        Ok(Parsed::Module(parse(tokens)?))
    } else {
        Ok(Parsed::Fragment(parse_fragment(tokens)?))
    }
}

/// Parses a single expression from a token stream that ends at `Eof`
/// (plans/M9.md item D: f-string interpolation interiors). Trailing
/// NEWLINE tokens the lexer inserts at EOF are skipped; anything else
/// after the expression is a parse error.
pub fn parse_expr(tokens: Vec<Token>) -> Result<Expr, ParseError> {
    let mut p = Parser::new(tokens);
    let expr = p.parse_or()?;
    while p.at_kind(TokenKind::Newline) {
        p.bump();
    }
    if !p.at_kind(TokenKind::Eof) {
        return Err(p.error_here(format!(
            "unexpected token after f-string interpolation expression: `{}`",
            p.peek_display()
        )));
    }
    Ok(expr)
}

struct Parser {
    tokens: Vec<Token>,
    pos: usize,
    /// Live nesting depth of the expression precedence chain
    /// (`parse_unary`'s own guard, `MAX_EXPR_DEPTH`) — every recursive
    /// re-entry into the chain (a parenthesized/bracketed group, a call
    /// argument, a chained unary prefix) passes through `parse_unary`
    /// exactly once per level, so counting there bounds native recursion
    /// depth regardless of which construct is doing the nesting.
    expr_depth: u32,
    /// Live nesting depth of indented statement blocks
    /// (`parse_stmts_until_dedent`'s own guard, `MAX_BLOCK_DEPTH`) — every
    /// nested suite in the language reaches its statements through that
    /// one function, so counting there bounds native recursion depth
    /// regardless of which compound statement is doing the nesting.
    block_depth: u32,
    /// Live nesting depth of type syntax (`parse_type`'s own guard,
    /// `MAX_TYPE_DEPTH`) — every nested type position (a generic argument,
    /// an array element, a tuple component, `own[P] T`'s inner type, an
    /// `fn(...)` parameter or return) re-enters `parse_type` exactly once
    /// per level.
    type_depth: u32,
    /// Nesting depth of single-line inline suites (`parse_inline_stmt_seq`):
    /// a `:` followed by real content on the same physical line, with no
    /// `Newline` token ever going to appear (module doc comment above —
    /// a `:`-newline instead opens a layout island or an ordinary indented
    /// block, never this path). `parse_inline_stmt_seq` now parses exactly
    /// one statement, but that one statement's own `end_of_simple_stmt`
    /// call still needs to tolerate having no terminator token to consume
    /// while this is nonzero (there's nothing there — the enclosing token,
    /// `)`/`]`/`}`/`,`/Eof/Dedent, is what `parse_inline_stmt_seq` itself
    /// checks for once control returns to it). At true depth 0 the lexer
    /// always inserts a real `Newline`, so this fallback is unreachable
    /// outside an inline suite.
    inline_depth: u32,
}

impl Parser {
    fn new(tokens: Vec<Token>) -> Self {
        assert!(
            !tokens.is_empty() && tokens.last().unwrap().kind == TokenKind::Eof,
            "token stream must end with Eof"
        );
        Parser {
            tokens,
            pos: 0,
            expr_depth: 0,
            block_depth: 0,
            type_depth: 0,
            inline_depth: 0,
        }
    }

    // --- low-level cursor ---------------------------------------------

    fn peek(&self) -> &Token {
        &self.tokens[self.pos]
    }

    /// The token `offset` positions ahead, clamped to the trailing `Eof`
    /// (there is nothing sensible to look at past the end of the stream).
    fn peek_at(&self, offset: usize) -> &Token {
        self.tokens
            .get(self.pos + offset)
            .unwrap_or_else(|| self.tokens.last().expect("non-empty by construction"))
    }

    fn peek_span(&self) -> Span {
        let t = self.peek();
        Span {
            line: t.line,
            col: t.col,
        }
    }

    fn peek_kind(&self) -> TokenKind {
        self.peek().kind.clone()
    }

    fn peek_text(&self) -> String {
        self.peek().text.clone()
    }

    /// Human-readable name for the current token, for error messages.
    fn peek_display(&self) -> String {
        let t = self.peek();
        if t.text.is_empty() {
            format!("{:?}", t.kind).to_uppercase()
        } else {
            t.text.clone()
        }
    }

    /// Advances past the current token and returns it. A no-op at `Eof` —
    /// callers never need to special-case running off the end.
    fn bump(&mut self) -> Token {
        let t = self.tokens[self.pos].clone();
        if t.kind != TokenKind::Eof {
            self.pos += 1;
        }
        t
    }

    fn at_kind(&self, kind: TokenKind) -> bool {
        self.peek().kind == kind
    }

    fn at_keyword(&self, kw: &str) -> bool {
        self.peek().kind == TokenKind::Keyword && self.peek().text == kw
    }

    fn at_ident_text(&self, s: &str) -> bool {
        self.peek().kind == TokenKind::Ident && self.peek().text == s
    }

    fn at_op(&self, op: &str) -> bool {
        self.peek().kind == TokenKind::Op && self.peek().text == op
    }

    fn peek_is_keyword_at(&self, offset: usize, kw: &str) -> bool {
        let t = self.peek_at(offset);
        t.kind == TokenKind::Keyword && t.text == kw
    }

    fn error_here(&self, message: impl Into<String>) -> ParseError {
        let t = self.peek();
        ParseError {
            message: message.into(),
            line: t.line,
            col: t.col,
        }
    }

    fn expect_keyword(&mut self, kw: &str) -> Result<Span, ParseError> {
        if self.at_keyword(kw) {
            let span = self.peek_span();
            self.bump();
            Ok(span)
        } else {
            Err(self.error_here(format!("expected `{kw}`, found `{}`", self.peek_display())))
        }
    }

    fn expect_op(&mut self, op: &str) -> Result<Span, ParseError> {
        if self.at_op(op) {
            let span = self.peek_span();
            self.bump();
            Ok(span)
        } else {
            Err(self.error_here(format!("expected `{op}`, found `{}`", self.peek_display())))
        }
    }

    /// Where a fresh name is declared, or a type is named: a `Keyword`
    /// token gets its own diagnostic (`keyword \`<kw>\` cannot be used as a
    /// name`) rather than the generic "expected X, found Y" — declaration
    /// positions are not the member/label/`pool`-callable exceptions
    /// (`expect_word`, `parse_path_segment`) where a reserved word doubles
    /// as an ordinary word.
    fn expect_ident(&mut self, what: &str) -> Result<(Span, String), ParseError> {
        if self.at_kind(TokenKind::Ident) {
            let span = self.peek_span();
            let text = self.bump().text;
            Ok((span, text))
        } else if self.at_kind(TokenKind::Keyword) {
            Err(self.error_here(format!(
                "keyword `{}` cannot be used as a name",
                self.peek_text()
            )))
        } else {
            Err(self.error_here(format!("expected {what}, found `{}`", self.peek_display())))
        }
    }

    /// Like `expect_ident`, but also accepts a `Keyword` token (its exact
    /// text becomes the name). Used after `.` for field/method names: a
    /// method can be named `read` (`interrupt_status.read()`), colliding
    /// with the access-mode keyword — the docs' own worked example does
    /// this, so member names are a separate namespace from reserved words.
    fn expect_word(&mut self, what: &str) -> Result<(Span, String), ParseError> {
        if self.at_kind(TokenKind::Ident) || self.at_kind(TokenKind::Keyword) {
            let span = self.peek_span();
            let text = self.bump().text;
            Ok((span, text))
        } else {
            Err(self.error_here(format!("expected {what}, found `{}`", self.peek_display())))
        }
    }

    /// A declared `fn` name: an `Ident`, or the keyword `from` (02 §7.4 /
    /// 05 §8's conversion associated fn, plans/M9.md item B decision 105).
    ///
    /// Ambiguity argument, established against this file: an import
    /// statement's `from` is only ever recognized at statement position
    /// (`parse_imports` / a future item-level `from`, both gated on
    /// `at_keyword("from")` as the *first* token of the statement, or
    /// `pub` then `from`). A function name is only ever read *after*
    /// `fn` has already been consumed (`parse_fn_item`). Those two
    /// positions never overlap, so accepting `from` here cannot steal an
    /// import and cannot leave `from path import Name` unparseable. Other
    /// keywords stay rejected (`keyword \`X\` cannot be used as a name`).
    fn expect_fn_name(&mut self) -> Result<(Span, String), ParseError> {
        if self.at_kind(TokenKind::Ident) {
            let span = self.peek_span();
            let text = self.bump().text;
            Ok((span, text))
        } else if self.at_keyword("from") {
            let span = self.peek_span();
            self.bump();
            Ok((span, "from".to_string()))
        } else if self.at_kind(TokenKind::Keyword) {
            Err(self.error_here(format!(
                "keyword `{}` cannot be used as a name",
                self.peek_text()
            )))
        } else {
            Err(self.error_here(format!(
                "expected a function name, found `{}`",
                self.peek_display()
            )))
        }
    }

    fn expect_newline(&mut self) -> Result<(), ParseError> {
        if self.at_kind(TokenKind::Newline) {
            self.bump();
            Ok(())
        } else {
            Err(self.error_here(format!(
                "expected end of line, found `{}`",
                self.peek_display()
            )))
        }
    }

    /// Like `expect_newline`, but tolerant of a declaration's value
    /// expression ending in a nested suite (a closure's `|params|: suite`
    /// body, `parse_closure`) whose own `Dedent`/trailing `Newline` already
    /// closed out this logical line — mirrors `end_of_simple_stmt`'s own
    /// depth-0 fallback (same reasoning, ast-expressions golden), minus
    /// that one's statement-only bracket/`,` tolerances: a declaration's
    /// value is never parsed inside an embedded `()[]{}` context, so those
    /// never apply here. Without this, a `const`/field initializer whose
    /// value is a suite-bodied closure round-trips through the printer
    /// (which always renders `ClosureBody::Suite` as an indented `:` block,
    /// print_closure's own doc comment) into something that fails to
    /// reparse — sema.check.roundtrip-stable's fuzz-sema finding, seed=11.
    fn expect_declaration_terminator(&mut self) -> Result<(), ParseError> {
        if self.at_kind(TokenKind::Newline) {
            self.bump();
            return Ok(());
        }
        if self.pos > 0 && self.tokens[self.pos - 1].kind == TokenKind::Newline {
            return Ok(());
        }
        Err(self.error_here(format!(
            "expected end of line, found `{}`",
            self.peek_display()
        )))
    }

    fn expect_indent(&mut self) -> Result<(), ParseError> {
        if self.at_kind(TokenKind::Indent) {
            self.bump();
            Ok(())
        } else {
            Err(self.error_here(format!(
                "expected an indented block, found `{}`",
                self.peek_display()
            )))
        }
    }

    /// A `Dedent` is always immediately followed by a `Newline` in the
    /// token stream (lexer.rs `handle_indentation`); consume both.
    fn expect_dedent(&mut self) -> Result<(), ParseError> {
        if !self.at_kind(TokenKind::Dedent) {
            return Err(self.error_here(format!(
                "expected the block to end, found `{}`",
                self.peek_display()
            )));
        }
        self.bump();
        if self.at_kind(TokenKind::Newline) {
            self.bump();
        }
        Ok(())
    }

    /// A dotted-path segment is any word token — `Ident` or `Keyword`.
    /// Module/import paths are a separate namespace from ordinary
    /// identifiers (02-language.md §2), and the docs' own worked example
    /// uses a reserved word as a path segment (`from runtime.pool import
    /// Pool`); rejecting it would make the parser wrong, not the doc.
    fn parse_path_segment(&mut self) -> Result<(Span, String), ParseError> {
        if self.at_kind(TokenKind::Ident) || self.at_kind(TokenKind::Keyword) {
            let span = self.peek_span();
            let text = self.bump().text;
            Ok((span, text))
        } else {
            Err(self.error_here(format!(
                "expected a path segment, found `{}`",
                self.peek_display()
            )))
        }
    }

    fn parse_dotted_path(&mut self) -> Result<Vec<String>, ParseError> {
        let mut path = vec![self.parse_path_segment()?.1];
        while self.at_op(".") {
            self.bump();
            path.push(self.parse_path_segment()?.1);
        }
        Ok(path)
    }
}

/// `## text` conventionally has one space after the markers; the lexer
/// keeps it raw (it is not the lexer's job to decode), so the parser strips
/// exactly one leading space, if present, when building a `Doc` node.
fn strip_doc_leading_space(raw: &str) -> String {
    raw.strip_prefix(' ').unwrap_or(raw).to_string()
}

// --- module ------------------------------------------------------------

impl Parser {
    fn parse_module(&mut self) -> Result<Module, ParseError> {
        let doc = self.collect_leading_doc();
        let span = self.expect_keyword("module")?;
        let path = self.parse_dotted_path()?;
        self.expect_newline()?;
        let imports = self.parse_imports()?;
        let items = self.parse_items()?;
        if !self.at_kind(TokenKind::Eof) {
            return Err(self.error_here(format!(
                "expected a top-level declaration, found `{}`",
                self.peek_display()
            )));
        }
        Ok(Module {
            span,
            path,
            doc,
            imports,
            items,
        })
    }

    /// Leading `##` doc comment(s) directly before the `module` header.
    /// Consecutive lines join with `\n`.
    fn collect_leading_doc(&mut self) -> Option<Doc> {
        let mut span = None;
        let mut lines = Vec::new();
        while self.at_kind(TokenKind::DocComment) {
            if span.is_none() {
                span = Some(self.peek_span());
            }
            lines.push(strip_doc_leading_space(&self.bump().text));
        }
        span.map(|span| Doc {
            span,
            text: lines.join("\n"),
        })
    }

    // --- imports ---------------------------------------------------------

    fn parse_imports(&mut self) -> Result<Vec<Import>, ParseError> {
        let mut imports = Vec::new();
        loop {
            let is_pub_from = self.at_keyword("pub") && self.peek_is_keyword_at(1, "from");
            if is_pub_from || self.at_keyword("from") {
                imports.push(self.parse_import()?);
            } else {
                break;
            }
        }
        Ok(imports)
    }

    fn parse_import(&mut self) -> Result<Import, ParseError> {
        // Starts at `pub` when present, otherwise at `from` — either way,
        // the first token of the statement.
        let span = self.peek_span();
        let mut is_pub = false;
        if self.at_keyword("pub") {
            self.bump();
            is_pub = true;
        }
        self.expect_keyword("from")?;
        let path = self.parse_dotted_path()?;
        self.expect_keyword("import")?;
        let names = self.parse_import_names()?;
        self.expect_newline()?;
        Ok(Import {
            span,
            is_pub,
            path,
            names,
        })
    }

    fn parse_import_names(&mut self) -> Result<Vec<ImportName>, ParseError> {
        if self.at_op("(") {
            self.bump();
            let mut names = Vec::new();
            loop {
                if self.at_op(")") {
                    break;
                }
                names.push(self.parse_import_name()?);
                if self.at_op(",") {
                    self.bump();
                    continue;
                }
                break;
            }
            // plans/M6.md item H, a soak find (sema lane, seed 72): the
            // parenthesized form accepted an EMPTY list, so `from a
            // import()` parsed into an `Import` node with no names, which
            // the pretty-printer faithfully rendered as `from a import `
            // — and that cannot reparse, breaking the roundtrip oracle.
            // 02-language.md §2's grammar is `from path import Name [as
            // Alias]`: at least one name, always. Rejected by name rather
            // than by letting an unspellable node exist.
            if names.is_empty() {
                return Err(self.error_here(
                    "an import list cannot be empty (`from <path> import <Name>` needs at least \
                     one name — 02-language.md §2)",
                ));
            }
            self.expect_op(")")?;
            Ok(names)
        } else {
            let mut names = vec![self.parse_import_name()?];
            while self.at_op(",") {
                self.bump();
                names.push(self.parse_import_name()?);
            }
            Ok(names)
        }
    }

    fn parse_import_name(&mut self) -> Result<ImportName, ParseError> {
        let (span, name) = self.expect_ident("an imported name")?;
        let alias = if self.at_ident_text("as") {
            self.bump();
            Some(self.expect_ident("an alias name")?.1)
        } else {
            None
        };
        Ok(ImportName { span, name, alias })
    }

    // --- doc/attrs ---------------------------------------------------------

    /// A run of `##` doc-comment lines and `@name(...)` attributes,
    /// interleaved in any order, immediately before a declaration. Doc
    /// lines join with `\n`; attributes are parsed as structured nodes in
    /// source order.
    fn collect_doc_and_attrs(&mut self) -> Result<(Option<Doc>, Vec<Attr>), ParseError> {
        let mut doc_span = None;
        let mut doc_lines = Vec::new();
        let mut attrs = Vec::new();
        loop {
            if self.at_kind(TokenKind::DocComment) {
                if doc_span.is_none() {
                    doc_span = Some(self.peek_span());
                }
                doc_lines.push(strip_doc_leading_space(&self.bump().text));
                continue;
            }
            if self.at_op("@") {
                attrs.push(self.parse_attr()?);
                // An attribute occupies its own logical line (unlike a doc
                // comment, it is not layout-transparent in the lexer).
                if self.at_kind(TokenKind::Newline) {
                    self.bump();
                }
                continue;
            }
            break;
        }
        let doc = doc_span.map(|span| Doc {
            span,
            text: doc_lines.join("\n"),
        });
        Ok((doc, attrs))
    }

    /// `@name` or `@name(arg, key=value, ...)` (02-language.md §13).
    fn parse_attr(&mut self) -> Result<Attr, ParseError> {
        let span = self.expect_op("@")?;
        let (_, name) = self.expect_ident("an attribute name")?;
        let args = if self.at_op("(") {
            self.parse_call_args()?
        } else {
            Vec::new()
        };
        Ok(Attr { span, name, args })
    }
}

// --- top-level items ---------------------------------------------------

impl Parser {
    fn parse_items(&mut self) -> Result<Vec<Item>, ParseError> {
        let mut items = Vec::new();
        loop {
            if self.at_kind(TokenKind::Eof) {
                break;
            }
            let (doc, attrs) = self.collect_doc_and_attrs()?;
            if self.at_kind(TokenKind::Eof) {
                if doc.is_some() || !attrs.is_empty() {
                    return Err(
                        self.error_here("expected a declaration after doc comment/attribute")
                    );
                }
                break;
            }
            items.push(self.parse_item(doc, attrs)?);
        }
        Ok(items)
    }

    /// Items nested inside a `comptime if`/`comptime else` branch at module
    /// scope: same shape as `parse_items`, bounded by `Dedent` instead of
    /// `Eof`. Assumes the caller has already consumed the branch's `:`.
    fn parse_indented_items(&mut self) -> Result<Vec<Item>, ParseError> {
        self.expect_newline()?;
        self.expect_indent()?;
        let mut items = Vec::new();
        loop {
            if self.at_kind(TokenKind::Dedent) {
                break;
            }
            let (doc, attrs) = self.collect_doc_and_attrs()?;
            if self.at_kind(TokenKind::Dedent) {
                if doc.is_some() || !attrs.is_empty() {
                    return Err(
                        self.error_here("expected a declaration after doc comment/attribute")
                    );
                }
                break;
            }
            items.push(self.parse_item(doc, attrs)?);
        }
        self.expect_dedent()?;
        Ok(items)
    }

    fn parse_item(&mut self, doc: Option<Doc>, attrs: Vec<Attr>) -> Result<Item, ParseError> {
        let start = self.peek_span();
        let mut is_pub = false;
        if self.at_keyword("pub") {
            self.bump();
            is_pub = true;
        }

        if self.at_keyword("async") && self.peek_is_keyword_at(1, "fn") {
            self.bump(); // async
            self.bump(); // fn
            return self
                .parse_fn_item(start, is_pub, true, doc, attrs)
                .map(Item::Fn);
        }
        if self.at_keyword("fn") {
            self.bump();
            return self
                .parse_fn_item(start, is_pub, false, doc, attrs)
                .map(Item::Fn);
        }
        if self.at_keyword("resource") && self.peek_is_keyword_at(1, "struct") {
            self.bump(); // resource
            self.bump(); // struct
            return self
                .parse_struct_item(start, is_pub, true, doc, attrs)
                .map(Item::Struct);
        }
        if self.at_keyword("struct") {
            self.bump();
            return self
                .parse_struct_item(start, is_pub, false, doc, attrs)
                .map(Item::Struct);
        }
        if self.at_keyword("enum") {
            self.bump();
            return self
                .parse_enum_item(start, is_pub, doc, attrs)
                .map(Item::Enum);
        }
        if self.at_keyword("const") {
            self.bump();
            return self
                .parse_const_item(start, is_pub, doc, attrs)
                .map(Item::Const);
        }
        if self.at_keyword("pool") {
            if is_pub {
                return Err(self.error_here("`pub` is not valid before `pool`"));
            }
            self.bump();
            let (_, name) = self.expect_ident("a pool name")?;
            self.expect_newline()?;
            return Ok(Item::Pool(PoolItem {
                span: start,
                name,
                doc,
                attrs,
            }));
        }
        if self.at_keyword("comptime") && self.peek_is_keyword_at(1, "if") {
            if is_pub {
                return Err(self.error_here("`pub` is not valid before `comptime if`"));
            }
            self.bump(); // comptime
            self.bump(); // if
            return self
                .parse_comptime_if_item(start, doc, attrs)
                .map(Item::ComptimeIf);
        }

        Err(self.error_here(format!(
            "expected a top-level declaration, found `{}`",
            self.peek_display()
        )))
    }

    fn parse_comptime_if_item(
        &mut self,
        start: Span,
        doc: Option<Doc>,
        attrs: Vec<Attr>,
    ) -> Result<ComptimeIfItem, ParseError> {
        let cond = self.parse_or()?;
        self.expect_op(":")?;
        let then_branch = self.parse_indented_items()?;
        let else_branch = if self.at_keyword("comptime") && self.peek_is_keyword_at(1, "else") {
            self.bump();
            self.bump();
            self.expect_op(":")?;
            Some(self.parse_indented_items()?)
        } else {
            None
        };
        Ok(ComptimeIfItem {
            span: start,
            doc,
            attrs,
            cond,
            then_branch,
            else_branch,
        })
    }

    fn parse_const_item(
        &mut self,
        start: Span,
        is_pub: bool,
        doc: Option<Doc>,
        attrs: Vec<Attr>,
    ) -> Result<ConstItem, ParseError> {
        let (_, name) = self.expect_ident("a const name")?;
        let ty = if self.at_op(":") {
            self.bump();
            Some(self.parse_type()?)
        } else {
            None
        };
        self.expect_op("=")?;
        let value = self.parse_or()?;
        self.expect_declaration_terminator()?;
        Ok(ConstItem {
            span: start,
            name,
            is_pub,
            doc,
            attrs,
            ty,
            value,
        })
    }
}

// --- fragments (corpus doc blocks with no `module` header) -------------

impl Parser {
    fn parse_fragment_body(&mut self) -> Result<Vec<FragmentEntry>, ParseError> {
        let mut entries = Vec::new();
        loop {
            if self.at_kind(TokenKind::Eof) {
                break;
            }
            let (doc, attrs) = self.collect_doc_and_attrs()?;
            if self.at_kind(TokenKind::Eof) {
                if doc.is_some() || !attrs.is_empty() {
                    return Err(self.error_here(
                        "expected a declaration or statement after doc comment/attribute",
                    ));
                }
                break;
            }
            if self.looks_like_item_start() {
                entries.push(FragmentEntry::Item(self.parse_item(doc, attrs)?));
            } else {
                if doc.is_some() || !attrs.is_empty() {
                    return Err(
                        self.error_here("doc comments/attributes may only precede a declaration")
                    );
                }
                entries.push(FragmentEntry::Stmt(self.parse_stmt()?));
            }
        }
        Ok(entries)
    }

    fn looks_like_item_start(&self) -> bool {
        if self.at_keyword("pub") {
            return true;
        }
        if self.at_keyword("async") && self.peek_is_keyword_at(1, "fn") {
            return true;
        }
        if self.at_keyword("fn") {
            return true;
        }
        if self.at_keyword("resource") && self.peek_is_keyword_at(1, "struct") {
            return true;
        }
        if self.at_keyword("struct") {
            return true;
        }
        if self.at_keyword("enum") {
            return true;
        }
        if self.at_keyword("const") {
            return true;
        }
        if self.at_keyword("pool") {
            return true;
        }
        false
    }
}

// --- generics, parameters, functions -------------------------------------

impl Parser {
    /// `[T, const N: usize]`, or nothing.
    fn parse_generic_params(&mut self) -> Result<Vec<GenericParam>, ParseError> {
        if !self.at_op("[") {
            return Ok(Vec::new());
        }
        self.bump();
        let mut params = Vec::new();
        loop {
            if self.at_op("]") {
                break;
            }
            let span = self.peek_span();
            if self.at_keyword("const") {
                self.bump();
                let (_, name) = self.expect_ident("a const generic parameter name")?;
                self.expect_op(":")?;
                let ty = self.parse_type()?;
                params.push(GenericParam::Const { span, name, ty });
            } else {
                let (_, name) = self.expect_ident("a generic parameter name")?;
                params.push(GenericParam::Type { span, name });
            }
            if self.at_op(",") {
                self.bump();
                continue;
            }
            break;
        }
        self.expect_op("]")?;
        Ok(params)
    }

    fn parse_optional_mode(&mut self) -> AccessMode {
        if self.at_keyword("read") {
            self.bump();
            AccessMode::Read
        } else if self.at_keyword("mut") {
            self.bump();
            AccessMode::Mut
        } else if self.at_keyword("take") {
            self.bump();
            AccessMode::Take
        } else {
            AccessMode::Read
        }
    }

    /// `(params...)`, with the first parameter recognized as the receiver
    /// when it is a bare (possibly mode-prefixed) `self` (02-language.md
    /// §5.1).
    fn parse_param_list(&mut self) -> Result<(Option<Receiver>, Vec<Param>), ParseError> {
        self.expect_op("(")?;
        let mut receiver = None;
        let mut params = Vec::new();
        let mut first = true;
        loop {
            if self.at_op(")") {
                break;
            }
            let pspan = self.peek_span();
            let mode = self.parse_optional_mode();
            if first && self.at_keyword("self") {
                self.bump();
                receiver = Some(Receiver { span: pspan, mode });
            } else {
                let (_, name) = self.expect_ident("a parameter name")?;
                self.expect_op(":")?;
                let ty = self.parse_type()?;
                let default = if self.at_op("=") {
                    self.bump();
                    Some(self.parse_or()?)
                } else {
                    None
                };
                params.push(Param {
                    span: pspan,
                    mode,
                    name,
                    ty,
                    default,
                });
            }
            first = false;
            if self.at_op(",") {
                self.bump();
                continue;
            }
            break;
        }
        self.expect_op(")")?;
        Ok((receiver, params))
    }

    fn parse_fn_item(
        &mut self,
        start: Span,
        is_pub: bool,
        is_async: bool,
        doc: Option<Doc>,
        attrs: Vec<Attr>,
    ) -> Result<FnItem, ParseError> {
        // `from` is the one reserved word a `fn` name may spell (02 §7.4);
        // see `expect_fn_name`. Other keywords stay rejected.
        let (_, name) = self.expect_fn_name()?;
        let generics = self.parse_generic_params()?;
        let (receiver, params) = self.parse_param_list()?;
        let (ret, body) = self.parse_fn_tail()?;
        Ok(FnItem {
            span: start,
            name,
            is_pub,
            is_async,
            doc,
            attrs,
            generics,
            receiver,
            params,
            ret,
            body,
        })
    }

    /// The `-> Ret: body` tail shared by `fn`/`init`, called right after the
    /// closing `)` of the parameter list. Section 1 allows the header to
    /// continue onto an indented line beginning with `->`; when it does,
    /// the continuation shares one indentation level with the body itself
    /// (there is no separate INDENT/DEDENT pair for the arrow line). A
    /// header with neither a suite nor a one-line body (bare `fn NAME(...)
    /// [-> Ret]` followed directly by a newline) is accepted with `body =
    /// None` — a few library-contract tables in the docs show bare method
    /// signatures this way (e.g. 05-library.md §8's operator-method table)
    /// to describe a desugaring target, not a real declaration; the docs
    /// are otherwise explicit that every real function has a body, so this
    /// is a deliberately narrow, fail-open-on-syntax/fail-closed-on-
    /// semantics allowance (a bodyless `fn` is syntactically well formed;
    /// whether one may exist for real is a later milestone's question).
    ///
    /// Narrow fix (found by `xtask fuzz sema`'s sema-roundtrip oracle,
    /// ledger clause sema.check.roundtrip-stable; golden/err-empty-body-
    /// continuation): this branch's continuation line shares its one
    /// INDENT with the body itself (see the doc comment above), so the
    /// body never gets a fresh INDENT token the way `parse_stmt_suite`'s
    /// ordinary path always does. A comment-only (or otherwise token-free)
    /// body is invisible to the lexer's indentation tracking, so
    /// `parse_stmts_until_dedent` below can land on an immediate `Dedent`
    /// with zero statements collected -- its own end-of-file check does
    /// not catch that, since this is a `Dedent`, not `Eof`. The ordinary
    /// (non-continuation) path never has this hole: a body with no real
    /// statement line never produces an INDENT at all, so its own
    /// `expect_indent()` already fails closed. 02-language.md section 1's
    /// `pass` statement exists exactly because every real body needs
    /// explicit content, so an empty body here is rejected the same way.
    fn parse_fn_tail(&mut self) -> Result<(Option<Type>, Option<Vec<Stmt>>), ParseError> {
        if self.at_kind(TokenKind::Newline) {
            self.bump();
            self.expect_indent()?;
            let ret = if self.at_op("->") {
                self.bump();
                Some(self.parse_type()?)
            } else {
                None
            };
            self.expect_op(":")?;
            self.expect_newline()?;
            let body = self.parse_stmts_until_dedent()?;
            if body.is_empty() {
                return Err(self.error_here(format!(
                    "expected a statement, found `{}`",
                    self.peek_display()
                )));
            }
            self.expect_dedent()?;
            Ok((ret, Some(body)))
        } else {
            let ret = if self.at_op("->") {
                self.bump();
                Some(self.parse_type()?)
            } else {
                None
            };
            if self.at_op(":") {
                self.bump();
                let body = self.parse_stmt_suite()?;
                Ok((ret, Some(body)))
            } else {
                self.expect_newline()?;
                Ok((ret, None))
            }
        }
    }

    fn parse_init(&mut self, doc: Option<Doc>, attrs: Vec<Attr>) -> Result<InitItem, ParseError> {
        let span = self.expect_keyword("init")?;
        let (receiver, params) = self.parse_param_list()?;
        let receiver =
            receiver.ok_or_else(|| self.error_here("`init` must declare a `self` receiver"))?;
        let (ret, body) = self.parse_fn_tail()?;
        let body = body.ok_or_else(|| self.error_here("`init` requires a body"))?;
        Ok(InitItem {
            span,
            doc,
            attrs,
            receiver,
            params,
            ret,
            body,
        })
    }
}

// --- struct / enum bodies --------------------------------------------------

impl Parser {
    fn parse_optional_deriving(&mut self) -> Result<Vec<String>, ParseError> {
        if !self.at_keyword("deriving") {
            return Ok(Vec::new());
        }
        self.bump();
        self.expect_op("(")?;
        let mut names = Vec::new();
        loop {
            if self.at_op(")") {
                break;
            }
            names.push(self.expect_ident("a derived trait name")?.1);
            if self.at_op(",") {
                self.bump();
                continue;
            }
            break;
        }
        self.expect_op(")")?;
        Ok(names)
    }

    fn parse_struct_item(
        &mut self,
        start: Span,
        is_pub: bool,
        is_resource: bool,
        doc: Option<Doc>,
        attrs: Vec<Attr>,
    ) -> Result<StructItem, ParseError> {
        let (_, name) = self.expect_ident("a struct name")?;
        let generics = self.parse_generic_params()?;
        let deriving = self.parse_optional_deriving()?;
        self.expect_op(":")?;
        let members = self.parse_indented_members()?;
        Ok(StructItem {
            span: start,
            name,
            is_pub,
            is_resource,
            doc,
            attrs,
            generics,
            deriving,
            members,
        })
    }

    /// Assumes the caller has already consumed the body's leading `:`.
    fn parse_indented_members(&mut self) -> Result<Vec<Member>, ParseError> {
        self.expect_newline()?;
        self.expect_indent()?;
        let mut members = Vec::new();
        loop {
            if self.at_kind(TokenKind::Dedent) {
                break;
            }
            let (doc, attrs) = self.collect_doc_and_attrs()?;
            if self.at_kind(TokenKind::Dedent) {
                if doc.is_some() || !attrs.is_empty() {
                    return Err(self.error_here("expected a member after doc comment/attribute"));
                }
                break;
            }
            members.push(self.parse_member(doc, attrs)?);
        }
        self.expect_dedent()?;
        Ok(members)
    }

    fn parse_member(&mut self, doc: Option<Doc>, attrs: Vec<Attr>) -> Result<Member, ParseError> {
        let start = self.peek_span();
        if self.at_keyword("pool") {
            self.bump();
            let (_, name) = self.expect_ident("a pool name")?;
            self.expect_newline()?;
            return Ok(Member::Pool(PoolItem {
                span: start,
                name,
                doc,
                attrs,
            }));
        }
        if self.at_keyword("comptime") && self.peek_is_keyword_at(1, "if") {
            self.bump();
            self.bump();
            return self
                .parse_comptime_if_member(start, doc, attrs)
                .map(Member::ComptimeIf);
        }
        if self.at_keyword("init") {
            return self.parse_init(doc, attrs).map(Member::Init);
        }
        let mut is_pub = false;
        if self.at_keyword("pub") {
            self.bump();
            is_pub = true;
        }
        if self.at_keyword("async") && self.peek_is_keyword_at(1, "fn") {
            self.bump();
            self.bump();
            return self
                .parse_fn_item(start, is_pub, true, doc, attrs)
                .map(Member::Fn);
        }
        if self.at_keyword("fn") {
            self.bump();
            return self
                .parse_fn_item(start, is_pub, false, doc, attrs)
                .map(Member::Fn);
        }
        let (_, name) = self.expect_ident("a field name")?;
        self.expect_op(":")?;
        let ty = self.parse_type()?;
        let default = if self.at_op("=") {
            self.bump();
            Some(self.parse_or()?)
        } else {
            None
        };
        self.expect_declaration_terminator()?;
        Ok(Member::Field(FieldItem {
            span: start,
            name,
            is_pub,
            doc,
            attrs,
            ty,
            default,
        }))
    }

    fn parse_comptime_if_member(
        &mut self,
        start: Span,
        doc: Option<Doc>,
        attrs: Vec<Attr>,
    ) -> Result<ComptimeIfMember, ParseError> {
        let cond = self.parse_or()?;
        self.expect_op(":")?;
        let then_branch = self.parse_indented_members()?;
        let else_branch = if self.at_keyword("comptime") && self.peek_is_keyword_at(1, "else") {
            self.bump();
            self.bump();
            self.expect_op(":")?;
            Some(self.parse_indented_members()?)
        } else {
            None
        };
        Ok(ComptimeIfMember {
            span: start,
            doc,
            attrs,
            cond,
            then_branch,
            else_branch,
        })
    }

    fn parse_enum_item(
        &mut self,
        start: Span,
        is_pub: bool,
        doc: Option<Doc>,
        attrs: Vec<Attr>,
    ) -> Result<EnumItem, ParseError> {
        let (_, name) = self.expect_ident("an enum name")?;
        let generics = self.parse_generic_params()?;
        let deriving = self.parse_optional_deriving()?;
        self.expect_op(":")?;
        let (variants, members) = self.parse_indented_enum_body()?;
        Ok(EnumItem {
            span: start,
            name,
            is_pub,
            doc,
            attrs,
            generics,
            deriving,
            variants,
            members,
        })
    }

    /// An enum body is a sequence of variants and `fn` members
    /// (02-language.md §7.2 / §5; plans/M9.md item B2). The distinction is
    /// lexical and unambiguous: a variant name is an Ident, while a method
    /// or associated fn begins with `fn`, `pub`, or `async` (Keyword
    /// tokens). Before this item, `parse_indented_variants` fed every line
    /// to `parse_variant` → `expect_ident`, so `pub fn` / `fn` surfaced as
    /// `keyword \`pub\`/\`fn\` cannot be used as a name` — a typo-shaped
    /// diagnostic for a missing language surface. `init`, `pool`, and
    /// field-shaped `name: Type` lines are refused by name.
    fn parse_indented_enum_body(&mut self) -> Result<(Vec<Variant>, Vec<Member>), ParseError> {
        self.expect_newline()?;
        self.expect_indent()?;
        let mut variants = Vec::new();
        let mut members = Vec::new();
        loop {
            if self.at_kind(TokenKind::Dedent) {
                break;
            }
            let (doc, attrs) = self.collect_doc_and_attrs()?;
            if self.at_kind(TokenKind::Dedent) {
                if doc.is_some() || !attrs.is_empty() {
                    return Err(
                        self.error_here("expected a variant or method after doc comment/attribute")
                    );
                }
                break;
            }
            if self.at_enum_method_start() {
                members.push(self.parse_enum_method(doc, attrs)?);
                continue;
            }
            if self.at_keyword("init") {
                return Err(self.error_here("an enum may not declare `init`"));
            }
            if self.at_keyword("pool") {
                return Err(self.error_here("an enum may not declare a `pool`"));
            }
            // Field-shaped `name: Type` — a struct member, not a variant
            // (`Name` or `Name(...)`). Refuse before `parse_variant` would
            // mis-read the ident and choke on the colon.
            if self.at_kind(TokenKind::Ident)
                && self.peek_at(1).kind == TokenKind::Op
                && self.peek_at(1).text == ":"
            {
                return Err(self.error_here(
                    "an enum may not declare fields; a variant is `Name` or `Name(...)`",
                ));
            }
            if doc.is_some() || !attrs.is_empty() {
                // Variants do not attach doc/attrs today; refuse rather
                // than silently drop them onto the next variant.
                return Err(self
                    .error_here("a doc comment or attribute on an enum variant is not supported"));
            }
            variants.push(self.parse_variant()?);
        }
        self.expect_dedent()?;
        Ok((variants, members))
    }

    /// `fn` / `pub fn` / `async fn` / `pub async fn` at the start of an
    /// enum-body line — never a variant name (those are Idents).
    fn at_enum_method_start(&self) -> bool {
        if self.at_keyword("fn") {
            return true;
        }
        if self.at_keyword("pub") {
            let next = self.peek_at(1);
            if next.kind == TokenKind::Keyword && (next.text == "fn" || next.text == "async") {
                return true;
            }
        }
        if self.at_keyword("async") && self.peek_is_keyword_at(1, "fn") {
            return true;
        }
        false
    }

    fn parse_enum_method(
        &mut self,
        doc: Option<Doc>,
        attrs: Vec<Attr>,
    ) -> Result<Member, ParseError> {
        let start = self.peek_span();
        let mut is_pub = false;
        if self.at_keyword("pub") {
            self.bump();
            is_pub = true;
        }
        if self.at_keyword("async") && self.peek_is_keyword_at(1, "fn") {
            self.bump();
            self.bump();
            return self
                .parse_fn_item(start, is_pub, true, doc, attrs)
                .map(Member::Fn);
        }
        if self.at_keyword("fn") {
            self.bump();
            return self
                .parse_fn_item(start, is_pub, false, doc, attrs)
                .map(Member::Fn);
        }
        Err(self.error_here("expected `fn` after `pub` in an enum body"))
    }

    fn parse_variant(&mut self) -> Result<Variant, ParseError> {
        let start = self.peek_span();
        let (_, name) = self.expect_ident("a variant name")?;
        let payload = if self.at_op("(") {
            self.bump();
            if self.at_op(")") {
                self.bump();
                VariantPayload::Tuple(Vec::new())
            } else if self.at_kind(TokenKind::Ident)
                && self.peek_at(1).kind == TokenKind::Op
                && self.peek_at(1).text == ":"
            {
                let mut fields = Vec::new();
                loop {
                    if self.at_op(")") {
                        break;
                    }
                    let fspan = self.peek_span();
                    let (_, fname) = self.expect_ident("a variant field name")?;
                    self.expect_op(":")?;
                    let ty = self.parse_type()?;
                    fields.push(VariantField {
                        span: fspan,
                        name: fname,
                        ty,
                    });
                    if self.at_op(",") {
                        self.bump();
                        continue;
                    }
                    break;
                }
                self.expect_op(")")?;
                VariantPayload::Named(fields)
            } else {
                let mut types = vec![self.parse_type()?];
                while self.at_op(",") {
                    self.bump();
                    if self.at_op(")") {
                        break;
                    }
                    types.push(self.parse_type()?);
                }
                self.expect_op(")")?;
                VariantPayload::Tuple(types)
            }
        } else {
            VariantPayload::None
        };
        self.expect_newline()?;
        Ok(Variant {
            span: start,
            name,
            payload,
        })
    }
}

// --- types (02-language.md §6) ------------------------------------------

impl Parser {
    /// Guarded entry for type nesting (`MAX_TYPE_DEPTH`'s own doc
    /// comment): every nested type position re-enters here exactly once
    /// per level, so counting on entry/exit bounds native recursion depth
    /// in this pass *and* in every later pass that walks the same shape.
    /// The grammar lives in `parse_type_body`, unchanged below; this
    /// wrapper only ever adds the counter (`parse_unary`'s own shape).
    fn parse_type(&mut self) -> Result<Type, ParseError> {
        self.type_depth += 1;
        if self.type_depth > MAX_TYPE_DEPTH {
            self.type_depth -= 1;
            return Err(self.error_here(format!("type nesting depth exceeded {MAX_TYPE_DEPTH}")));
        }
        let result = self.parse_type_body();
        self.type_depth -= 1;
        result
    }

    fn parse_type_body(&mut self) -> Result<Type, ParseError> {
        let span = self.peek_span();
        if self.at_op("[") {
            self.bump();
            let elem = self.parse_type()?;
            self.expect_op(";")?;
            let len = self.parse_or()?;
            self.expect_op("]")?;
            return Ok(Type::Array(Box::new(ArrayType { span, elem, len })));
        }
        if self.at_op("(") {
            self.bump();
            if self.at_op(")") {
                self.bump();
                return Ok(Type::Tuple(TupleType {
                    span,
                    elems: Vec::new(),
                }));
            }
            let first = self.parse_type()?;
            if self.at_op(")") {
                self.bump();
                return Ok(first); // pure grouping: `(u64)` == `u64`
            }
            let mut elems = vec![first];
            loop {
                if self.at_op(",") {
                    self.bump();
                    if self.at_op(")") {
                        break;
                    }
                    elems.push(self.parse_type()?);
                } else {
                    break;
                }
            }
            self.expect_op(")")?;
            return Ok(Type::Tuple(TupleType { span, elems }));
        }
        if self.at_keyword("own") {
            self.bump();
            self.expect_op("[")?;
            let pool = self.parse_dotted_path()?;
            self.expect_op("]")?;
            let inner = self.parse_type()?;
            return Ok(Type::Own(Box::new(OwnType { span, pool, inner })));
        }
        if self.at_keyword("fn") {
            self.bump();
            self.expect_op("(")?;
            let mut params = Vec::new();
            loop {
                if self.at_op(")") {
                    break;
                }
                let pspan = self.peek_span();
                let mode = self.parse_optional_mode();
                let ty = self.parse_type()?;
                params.push(FnTypeParam {
                    span: pspan,
                    mode,
                    ty,
                });
                if self.at_op(",") {
                    self.bump();
                    continue;
                }
                break;
            }
            self.expect_op(")")?;
            let ret = if self.at_op("->") {
                self.bump();
                Some(self.parse_type()?)
            } else {
                None
            };
            return Ok(Type::Fn(Box::new(FnType { span, params, ret })));
        }
        let name = if self.at_kind(TokenKind::Ident) {
            self.bump().text
        } else if self.at_keyword("unit") {
            self.bump();
            "unit".to_string()
        } else if self.at_keyword("self") {
            self.bump();
            "self".to_string()
        } else {
            return Err(
                self.error_here(format!("expected a type, found `{}`", self.peek_display()))
            );
        };
        let args = if self.at_op("[") {
            self.parse_generic_args()?
        } else {
            Vec::new()
        };
        Ok(Type::Named(NamedType { span, name, args }))
    }

    fn parse_generic_args(&mut self) -> Result<Vec<GenericArg>, ParseError> {
        self.expect_op("[")?;
        let mut args = Vec::new();
        loop {
            if self.at_op("]") {
                break;
            }
            args.push(self.parse_generic_arg()?);
            if self.at_op(",") {
                self.bump();
                continue;
            }
            break;
        }
        self.expect_op("]")?;
        Ok(args)
    }

    /// One generic argument at a use site: `..N` (bounded occupancy), a
    /// type, or — since bounded-type parameter positions can also take a
    /// A type argument, a comptime expression like `40 + z + 96`, or a
    /// plain comptime expression like `256.KiB` — a fallback to expression
    /// parsing when the current token cannot start a type at all.
    ///
    /// Expression starters include unary prefixes (`-`/`~`) and a `(`
    /// whose interior itself starts an expression. The pretty-printer
    /// always wraps non-atomic Binary/Unary operands in parens
    /// (`(40 + z) + 96`, `(-40) + z`), and without this branch those
    /// spellings fell into `parse_type`'s tuple/grouping arm and rejected
    /// the integer — a sema-roundtrip hole the deep soak found
    /// (seed=8802). Tuple types `(T, U)` and `(u64)` grouping still start
    /// with a type token after `(` and keep the type path.
    fn parse_generic_arg(&mut self) -> Result<GenericArg, ParseError> {
        if self.at_op("..") {
            self.bump();
            let e = self.parse_or()?;
            return Ok(GenericArg::Bound(e));
        }
        if self.starts_generic_arg_expr() {
            let e = self.parse_or()?;
            return Ok(GenericArg::Expr(e));
        }
        let ty = self.parse_type()?;
        Ok(GenericArg::Type(ty))
    }

    /// See `parse_generic_arg`: tokens that begin a const/expression
    /// generic arg rather than a type.
    fn starts_generic_arg_expr(&self) -> bool {
        if matches!(
            self.peek_kind(),
            TokenKind::Int
                | TokenKind::Float
                | TokenKind::Str
                | TokenKind::BStr
                | TokenKind::FStr
                | TokenKind::Char
        ) || self.at_keyword("true")
            || self.at_keyword("false")
        {
            return true;
        }
        if self.at_op("-") || self.at_op("~") {
            return true;
        }
        if self.at_op("(") {
            let inner = self.peek_at(1);
            if matches!(
                inner.kind,
                TokenKind::Int
                    | TokenKind::Float
                    | TokenKind::Str
                    | TokenKind::BStr
                    | TokenKind::FStr
                    | TokenKind::Char
            ) {
                return true;
            }
            if inner.kind == TokenKind::Keyword
                && matches!(
                    inner.text.as_str(),
                    "true" | "false" | "not" | "await" | "take"
                )
            {
                return true;
            }
            if inner.kind == TokenKind::Op && matches!(inner.text.as_str(), "-" | "~" | "(") {
                return true;
            }
        }
        false
    }
}

// --- expressions (02-language.md §8.2) -----------------------------------
//
// Precedence, tightest first: member/call/index; unary `-` `~` `await`
// `take`; postfix `?`; `* / % *%`; `+ - +% -%`; `<< >>`; `& ^ |`; ranges;
// comparisons and `is`; `not`; `and`; `or`. Realized as a chain of
// functions, loosest first (each calls the next-tighter one): parse_or ->
// parse_and -> parse_not -> parse_compare -> parse_range -> parse_bitor ->
// parse_shift -> parse_addsub -> parse_muldiv -> parse_try -> parse_unary
// -> parse_postfix -> parse_primary. `await op()?` therefore parses as
// `(await op())?`: parse_try parses one parse_unary (which parses `await`
// around the postfix chain `op()`) and only then wraps the trailing `?`.

impl Parser {
    /// The general expression entry point used everywhere a bare expression
    /// is expected (there is no separate "top-level expr" production).
    fn parse_or(&mut self) -> Result<Expr, ParseError> {
        let mut left = self.parse_and()?;
        while self.at_keyword("or") {
            let span = self.peek_span();
            self.bump();
            let right = self.parse_and()?;
            left = Expr::Or(span, Box::new(left), Box::new(right));
        }
        Ok(left)
    }

    fn parse_and(&mut self) -> Result<Expr, ParseError> {
        let mut left = self.parse_not()?;
        while self.at_keyword("and") {
            let span = self.peek_span();
            self.bump();
            let right = self.parse_not()?;
            left = Expr::And(span, Box::new(left), Box::new(right));
        }
        Ok(left)
    }

    fn parse_not(&mut self) -> Result<Expr, ParseError> {
        if self.at_keyword("not") {
            let span = self.peek_span();
            self.bump();
            let inner = self.parse_not()?;
            return Ok(Expr::Not(span, Box::new(inner)));
        }
        self.parse_compare()
    }

    /// Comparisons and `is` do not chain: at most one applies per level.
    fn parse_compare(&mut self) -> Result<Expr, ParseError> {
        let left = self.parse_range()?;
        if self.at_keyword("is") {
            let span = self.peek_span();
            self.bump();
            let pat = self.parse_pattern()?;
            return Ok(Expr::Is(span, Box::new(left), Box::new(pat)));
        }
        const CMP_OPS: &[(&str, BinOp)] = &[
            ("<=", BinOp::Le),
            (">=", BinOp::Ge),
            ("==", BinOp::Eq),
            ("!=", BinOp::Ne),
            ("<", BinOp::Lt),
            (">", BinOp::Gt),
        ];
        for (op, bop) in CMP_OPS {
            if self.at_op(op) {
                let span = self.peek_span();
                self.bump();
                let right = self.parse_range()?;
                return Ok(Expr::Binary(span, *bop, Box::new(left), Box::new(right)));
            }
        }
        Ok(left)
    }

    /// Ranges do not chain either (no example combines more than one).
    fn parse_range(&mut self) -> Result<Expr, ParseError> {
        let left = self.parse_bitor()?;
        if self.at_op("..") || self.at_op("..=") {
            let inclusive = self.at_op("..=");
            let span = self.peek_span();
            self.bump();
            let right = self.parse_bitor()?;
            return Ok(Expr::Range(
                span,
                Box::new(left),
                Box::new(right),
                inclusive,
            ));
        }
        Ok(left)
    }

    fn parse_bitor(&mut self) -> Result<Expr, ParseError> {
        let mut left = self.parse_shift()?;
        loop {
            let bop = if self.at_op("&") {
                BinOp::BitAnd
            } else if self.at_op("^") {
                BinOp::BitXor
            } else if self.at_op("|") {
                BinOp::BitOr
            } else {
                break;
            };
            let span = self.peek_span();
            self.bump();
            let right = self.parse_shift()?;
            left = Expr::Binary(span, bop, Box::new(left), Box::new(right));
        }
        Ok(left)
    }

    fn parse_shift(&mut self) -> Result<Expr, ParseError> {
        let mut left = self.parse_addsub()?;
        loop {
            let bop = if self.at_op("<<") {
                BinOp::Shl
            } else if self.at_op(">>") {
                BinOp::Shr
            } else {
                break;
            };
            let span = self.peek_span();
            self.bump();
            let right = self.parse_addsub()?;
            left = Expr::Binary(span, bop, Box::new(left), Box::new(right));
        }
        Ok(left)
    }

    fn parse_addsub(&mut self) -> Result<Expr, ParseError> {
        let mut left = self.parse_muldiv()?;
        loop {
            let bop = if self.at_op("+%") {
                BinOp::AddW
            } else if self.at_op("-%") {
                BinOp::SubW
            } else if self.at_op("+") {
                BinOp::Add
            } else if self.at_op("-") {
                BinOp::Sub
            } else {
                break;
            };
            let span = self.peek_span();
            self.bump();
            let right = self.parse_muldiv()?;
            left = Expr::Binary(span, bop, Box::new(left), Box::new(right));
        }
        Ok(left)
    }

    fn parse_muldiv(&mut self) -> Result<Expr, ParseError> {
        let mut left = self.parse_try()?;
        loop {
            let bop = if self.at_op("*%") {
                BinOp::MulW
            } else if self.at_op("*") {
                BinOp::Mul
            } else if self.at_op("/") {
                BinOp::Div
            } else if self.at_op("%") {
                BinOp::Rem
            } else {
                break;
            };
            let span = self.peek_span();
            self.bump();
            let right = self.parse_try()?;
            left = Expr::Binary(span, bop, Box::new(left), Box::new(right));
        }
        Ok(left)
    }

    fn parse_try(&mut self) -> Result<Expr, ParseError> {
        let mut e = self.parse_unary()?;
        while self.at_op("?") {
            let span = self.peek_span();
            self.bump();
            e = Expr::Try(span, Box::new(e));
        }
        Ok(e)
    }

    /// Guarded entry to the expression precedence chain (`MAX_EXPR_DEPTH`'s
    /// own doc comment): every recursive re-entry — a parenthesized/
    /// bracketed group via `parse_primary`, a chained unary prefix via
    /// this function's own self-recursion — passes through here exactly
    /// once per nesting level, so counting on entry/exit bounds native
    /// recursion depth regardless of which construct is doing the
    /// nesting. The actual grammar lives in `parse_unary_body`, unchanged
    /// below; this wrapper only ever adds the counter.
    fn parse_unary(&mut self) -> Result<Expr, ParseError> {
        self.expr_depth += 1;
        if self.expr_depth > MAX_EXPR_DEPTH {
            self.expr_depth -= 1;
            return Err(self.error_here(format!(
                "expression nesting depth exceeded {MAX_EXPR_DEPTH}"
            )));
        }
        let result = self.parse_unary_body();
        self.expr_depth -= 1;
        result
    }

    fn parse_unary_body(&mut self) -> Result<Expr, ParseError> {
        if self.at_op("-") {
            let span = self.peek_span();
            self.bump();
            let inner = self.parse_unary()?;
            return Ok(Expr::Unary(span, UnaryOp::Neg, Box::new(inner)));
        }
        if self.at_op("~") {
            let span = self.peek_span();
            self.bump();
            let inner = self.parse_unary()?;
            return Ok(Expr::Unary(span, UnaryOp::BitNot, Box::new(inner)));
        }
        if self.at_keyword("await") {
            let span = self.peek_span();
            self.bump();
            let inner = self.parse_unary()?;
            return Ok(Expr::Unary(span, UnaryOp::Await, Box::new(inner)));
        }
        if self.at_keyword("take") {
            let span = self.peek_span();
            self.bump();
            let inner = self.parse_unary()?;
            if !is_place_expr(&inner) {
                return Err(self.error_here(
                    "operand of `take` must be a place expression (name, field, index)",
                ));
            }
            return Ok(Expr::Unary(span, UnaryOp::Take, Box::new(inner)));
        }
        if self.at_keyword("send") {
            let span = self.peek_span();
            self.bump();
            let inner = self.parse_unary()?;
            if !matches!(inner, Expr::Call(..)) {
                return Err(self.error_here("`send` requires a call expression"));
            }
            return Ok(Expr::Send(span, Box::new(inner)));
        }
        self.parse_postfix()
    }

    fn parse_postfix(&mut self) -> Result<Expr, ParseError> {
        let mut e = self.parse_primary()?;
        loop {
            if self.at_op(".") {
                self.bump();
                let (span, name) = self.expect_word("a field or method name")?;
                e = Expr::Field(Box::new(e), span, name);
            } else if self.at_op("(") {
                let span = self.peek_span();
                let args = self.parse_call_args()?;
                e = Expr::Call(Box::new(e), span, args);
            } else if self.at_op("[") {
                let span = self.peek_span();
                self.bump();
                let mut items = Vec::new();
                loop {
                    if self.at_op("]") {
                        break;
                    }
                    items.push(self.parse_or()?);
                    if self.at_op(",") {
                        self.bump();
                        continue;
                    }
                    break;
                }
                self.expect_op("]")?;
                e = Expr::Index(Box::new(e), span, items);
            } else {
                break;
            }
        }
        Ok(e)
    }

    /// `(args...)` shared by calls and attributes: `[label=][mut|take]
    /// value`. The mirrored `mut`/`take` marker's operand must be a place
    /// expression (name, field, index — parens are transparent, dropped at
    /// parse time so a parenthesized place needs no extra case here).
    fn parse_call_args(&mut self) -> Result<Vec<Arg>, ParseError> {
        self.expect_op("(")?;
        let mut args = Vec::new();
        loop {
            if self.at_op(")") {
                break;
            }
            let span = self.peek_span();
            // A label is any word token (`Ident` or `Keyword` — e.g.
            // `VirtQueue.configure(pool=take control_pool, ...)` labels an
            // argument `pool`, which is otherwise reserved) directly
            // followed by `=` (never `==`, a distinct token).
            let label = if matches!(self.peek_kind(), TokenKind::Ident | TokenKind::Keyword) && {
                let t = self.peek_at(1);
                t.kind == TokenKind::Op && t.text == "="
            } {
                Some(self.bump().text)
            } else {
                None
            };
            if label.is_some() {
                self.bump(); // '='
            }
            let mode = if self.at_keyword("mut") {
                self.bump();
                AccessMode::Mut
            } else if self.at_keyword("take") {
                self.bump();
                AccessMode::Take
            } else {
                AccessMode::Read
            };
            let value = self.parse_or()?;
            if mode != AccessMode::Read && !is_place_expr(&value) {
                return Err(self.error_here(format!(
                    "operand of `{}` must be a place expression (name, field, index)",
                    mode.as_str()
                )));
            }
            args.push(Arg {
                span,
                label,
                mode,
                value,
            });
            if self.at_op(",") {
                self.bump();
                continue;
            }
            break;
        }
        self.expect_op(")")?;
        Ok(args)
    }

    fn parse_primary(&mut self) -> Result<Expr, ParseError> {
        let span = self.peek_span();
        match self.peek_kind() {
            TokenKind::Int => Ok(Expr::Int(span, self.bump().text)),
            TokenKind::Float => Ok(Expr::Float(span, self.bump().text)),
            TokenKind::Str => Ok(Expr::Str(span, self.bump().text)),
            TokenKind::BStr => Ok(Expr::BStr(span, self.bump().text)),
            TokenKind::Char => Ok(Expr::Char(span, self.bump().text)),
            TokenKind::FStr => {
                let t = self.bump();
                Ok(Expr::FStr(split_fstring(&t)))
            }
            TokenKind::Keyword if self.peek_text() == "true" => {
                self.bump();
                Ok(Expr::Bool(span, true))
            }
            TokenKind::Keyword if self.peek_text() == "false" => {
                self.bump();
                Ok(Expr::Bool(span, false))
            }
            TokenKind::Keyword if self.peek_text() == "unit" => {
                self.bump();
                Ok(Expr::Unit(span))
            }
            TokenKind::Keyword if self.peek_text() == "self" => {
                self.bump();
                Ok(Expr::Name(span, "self".to_string()))
            }
            // `pool(...)` (the scoped-pool constructor, 02-language.md §4)
            // is an ordinary call in expression position; the `pool NAME`
            // declaration form is recognized earlier, at item/member
            // dispatch, so there is no ambiguity here.
            TokenKind::Keyword if self.peek_text() == "pool" => {
                self.bump();
                Ok(Expr::Name(span, "pool".to_string()))
            }
            TokenKind::Ident => Ok(Expr::Name(span, self.bump().text)),
            TokenKind::Op if self.peek_text() == "." => {
                self.bump();
                let (_, name) = self.expect_ident("a variant name")?;
                let args = if self.at_op("(") {
                    self.parse_call_args()?
                } else {
                    Vec::new()
                };
                Ok(Expr::DotVariant(span, name, args))
            }
            TokenKind::Op if self.peek_text() == "(" => {
                self.bump();
                if self.at_op(")") {
                    self.bump();
                    return Ok(Expr::Tuple(span, Vec::new()));
                }
                let first = self.parse_or()?;
                if self.at_op(")") {
                    self.bump();
                    return Ok(first); // pure grouping
                }
                let mut elems = vec![first];
                loop {
                    if self.at_op(",") {
                        self.bump();
                        if self.at_op(")") {
                            break;
                        }
                        elems.push(self.parse_or()?);
                    } else {
                        break;
                    }
                }
                self.expect_op(")")?;
                Ok(Expr::Tuple(span, elems))
            }
            TokenKind::Op if self.peek_text() == "[" => {
                self.bump();
                if self.at_op("]") {
                    self.bump();
                    return Ok(Expr::List(span, Vec::new()));
                }
                let first = self.parse_or()?;
                // plans/M9.md item F1 decision 343: `[elem; N]` array-repeat.
                if self.at_op(";") {
                    self.bump();
                    let count = self.parse_or()?;
                    self.expect_op("]")?;
                    return Ok(Expr::ArrayRepeat(span, Box::new(first), Box::new(count)));
                }
                let mut elems = vec![first];
                loop {
                    if self.at_op(",") {
                        self.bump();
                        if self.at_op("]") {
                            break;
                        }
                        elems.push(self.parse_or()?);
                        continue;
                    }
                    break;
                }
                self.expect_op("]")?;
                Ok(Expr::List(span, elems))
            }
            TokenKind::Op if self.peek_text() == "|" => self.parse_closure(),
            _ => Err(self.error_here(format!(
                "expected an expression, found `{}`",
                self.peek_display()
            ))),
        }
    }

    /// `|params| expr` or `|params|: suite` (02-language.md §8.3).
    fn parse_closure(&mut self) -> Result<Expr, ParseError> {
        let span = self.expect_op("|")?;
        let mut params = Vec::new();
        loop {
            if self.at_op("|") {
                break;
            }
            let pspan = self.peek_span();
            let mode = self.parse_optional_mode();
            let (_, name) = self.expect_ident("a closure parameter name")?;
            let ty = if self.at_op(":") {
                self.bump();
                Some(self.parse_type()?)
            } else {
                None
            };
            params.push(ClosureParam {
                span: pspan,
                mode,
                name,
                ty,
            });
            if self.at_op(",") {
                self.bump();
                continue;
            }
            break;
        }
        self.expect_op("|")?;
        let body = if self.at_op(":") {
            self.bump();
            ClosureBody::Suite(self.parse_stmt_suite()?)
        } else {
            self.parse_closure_short_body()?
        };
        Ok(Expr::Closure(ClosureExpr { span, params, body }))
    }

    /// The `|params| BODY` short form's body: normally a pure expression,
    /// but 02-language.md §8.3's own example body is a compound assignment
    /// (`item.count += 1`) — assignment is a statement, not an expression,
    /// in this grammar (§8.1). This deliberately does **not** go through
    /// `parse_stmt`/`end_of_simple_stmt`: the closure is itself nested
    /// inside a larger expression, so its body must not consume the
    /// terminator that belongs to whatever statement encloses it.
    fn parse_closure_short_body(&mut self) -> Result<ClosureBody, ParseError> {
        let span = self.peek_span();
        let target = self.parse_or()?;
        if let Some(op) = self.assign_op_here() {
            // Same place check as the ordinary statement path above
            // (err-assign-nonplace-*): without it, `|x| 5 = true` built
            // an AssignStmt with a non-place target through this one
            // remaining side door — accepted here, rejected on reparse
            // of its own pretty-printed suite form, which is exactly the
            // roundtrip asymmetry `fuzz sema` found at seeds 41-43
            // (golden/err-assign-nonplace-closure pins the shape).
            if !is_place_expr(&target) {
                return Err(self.error_here(
                    "the left side of an assignment must be a place expression (name, field, index)",
                ));
            }
            self.bump();
            let value = self.parse_or()?;
            return Ok(ClosureBody::Suite(vec![Stmt::Assign(AssignStmt {
                span,
                target,
                ty: None,
                op,
                value,
            })]));
        }
        Ok(ClosureBody::Expr(Box::new(target)))
    }
}

/// One UTF-8 code point's byte length from its leading byte. Content here
/// was already validated as UTF-8 by the lexer.
fn utf8_char_len(b: u8) -> usize {
    if b & 0x80 == 0 {
        1
    } else if b & 0xE0 == 0xC0 {
        2
    } else if b & 0xF0 == 0xE0 {
        3
    } else {
        4
    }
}

/// Segments an `FStr` token's raw text into literal/interpolation parts
/// (02-language.md §1.1): `{{`/`}}` unescape to literal braces; a single
/// `{` opens a balanced-brace interpolation extent. Interpolation contents
/// are **not** parsed recursively in M1 (ast.rs `FStringPart` doc comment)
/// — each `Interp` keeps its raw source text and span. Column arithmetic is
/// byte-based, matching the lexer's own convention (lexer.rs `Lexer::bump`),
/// so non-ASCII content inside the literal keeps consistent spans.
fn split_fstring(token: &Token) -> FStringLit {
    let raw = token.text.as_str();
    // Strip the `f"` prefix (2 bytes) and the trailing `"` (1 byte); a
    // `b"..."`-style prefix never applies to FStr tokens (lexer.rs only
    // tags FStr for the `f"` spelling).
    let body = &raw[2..raw.len() - 1];
    let bytes = body.as_bytes();
    let mut parts = Vec::new();
    let mut i = 0usize;
    let mut col = token.col + 2;
    let mut lit_start_col = col;
    let mut lit = String::new();
    while i < bytes.len() {
        match bytes[i] {
            b'{' if bytes.get(i + 1) == Some(&b'{') => {
                lit.push('{');
                i += 2;
                col += 2;
            }
            b'{' => {
                if !lit.is_empty() {
                    parts.push(FStringPart::Literal(
                        Span {
                            line: token.line,
                            col: lit_start_col,
                        },
                        std::mem::take(&mut lit),
                    ));
                }
                i += 1;
                col += 1;
                let interp_col = col;
                let start = i;
                let mut depth = 1u32;
                while i < bytes.len() && depth > 0 {
                    match bytes[i] {
                        b'{' => depth += 1,
                        b'}' => {
                            depth -= 1;
                            if depth == 0 {
                                break;
                            }
                        }
                        _ => {}
                    }
                    i += 1;
                    col += 1;
                }
                let interp_text = body[start..i].to_string();
                if i < bytes.len() {
                    i += 1; // closing `}`
                    col += 1;
                }
                parts.push(FStringPart::Interp(
                    Span {
                        line: token.line,
                        col: interp_col,
                    },
                    interp_text,
                ));
                lit_start_col = col;
            }
            b'}' if bytes.get(i + 1) == Some(&b'}') => {
                lit.push('}');
                i += 2;
                col += 2;
            }
            b => {
                let n = utf8_char_len(b);
                lit.push_str(&body[i..i + n]);
                i += n;
                col += n as u32;
            }
        }
    }
    if !lit.is_empty() {
        parts.push(FStringPart::Literal(
            Span {
                line: token.line,
                col: lit_start_col,
            },
            lit,
        ));
    }
    FStringLit {
        span: Span {
            line: token.line,
            col: token.col,
        },
        parts,
    }
}

// --- patterns (02-language.md §7.2) -------------------------------------

impl Parser {
    fn parse_pattern(&mut self) -> Result<Pattern, ParseError> {
        let first = self.parse_pattern_primary()?;
        if self.at_op("|") {
            let span = first.span();
            let mut alts = vec![first];
            while self.at_op("|") {
                self.bump();
                alts.push(self.parse_pattern_primary()?);
            }
            return Ok(Pattern::Or(span, alts));
        }
        Ok(first)
    }

    fn parse_pattern_payload(&mut self) -> Result<Vec<Pattern>, ParseError> {
        self.expect_op("(")?;
        let mut items = Vec::new();
        loop {
            if self.at_op(")") {
                break;
            }
            items.push(self.parse_pattern()?);
            if self.at_op(",") {
                self.bump();
                continue;
            }
            break;
        }
        self.expect_op(")")?;
        Ok(items)
    }

    fn parse_pattern_primary(&mut self) -> Result<Pattern, ParseError> {
        let span = self.peek_span();
        if self.at_kind(TokenKind::Ident) && self.peek_text() == "_" {
            self.bump();
            return Ok(Pattern::Wildcard(span));
        }
        if self.at_keyword("take") {
            self.bump();
            let inner = self.parse_pattern_primary()?;
            return Ok(Pattern::Take(span, Box::new(inner)));
        }
        if self.at_op(".") {
            self.bump();
            let (_, variant) = self.expect_ident("a variant name")?;
            let payload = if self.at_op("(") {
                self.parse_pattern_payload()?
            } else {
                Vec::new()
            };
            return Ok(Pattern::Variant {
                span,
                enum_name: None,
                variant,
                payload,
            });
        }
        if self.at_op("(") {
            self.bump();
            if self.at_op(")") {
                self.bump();
                return Ok(Pattern::Tuple(span, Vec::new()));
            }
            let first = self.parse_pattern()?;
            if self.at_op(")") {
                self.bump();
                return Ok(first); // pure grouping
            }
            let mut elems = vec![first];
            loop {
                if self.at_op(",") {
                    self.bump();
                    if self.at_op(")") {
                        break;
                    }
                    elems.push(self.parse_pattern()?);
                } else {
                    break;
                }
            }
            self.expect_op(")")?;
            return Ok(Pattern::Tuple(span, elems));
        }
        if self.at_op("[") {
            self.bump();
            let mut elems = Vec::new();
            loop {
                if self.at_op("]") {
                    break;
                }
                elems.push(self.parse_pattern()?);
                if self.at_op(",") {
                    self.bump();
                    continue;
                }
                break;
            }
            self.expect_op("]")?;
            return Ok(Pattern::Array(span, elems));
        }
        if self.at_op("-") && matches!(self.peek_at(1).kind, TokenKind::Int | TokenKind::Float) {
            let minus_span = span;
            self.bump();
            let lit_span = self.peek_span();
            let value = match self.peek_kind() {
                TokenKind::Int => Expr::Int(lit_span, self.bump().text),
                TokenKind::Float => Expr::Float(lit_span, self.bump().text),
                _ => unreachable!("guarded by the match above"),
            };
            return Ok(Pattern::Literal(
                span,
                Expr::Unary(minus_span, UnaryOp::Neg, Box::new(value)),
            ));
        }
        match self.peek_kind() {
            TokenKind::Int => Ok(Pattern::Literal(span, Expr::Int(span, self.bump().text))),
            TokenKind::Float => Ok(Pattern::Literal(span, Expr::Float(span, self.bump().text))),
            TokenKind::Str => Ok(Pattern::Literal(span, Expr::Str(span, self.bump().text))),
            TokenKind::BStr => Ok(Pattern::Literal(span, Expr::BStr(span, self.bump().text))),
            TokenKind::Char => Ok(Pattern::Literal(span, Expr::Char(span, self.bump().text))),
            TokenKind::Keyword if self.peek_text() == "true" => {
                self.bump();
                Ok(Pattern::Literal(span, Expr::Bool(span, true)))
            }
            TokenKind::Keyword if self.peek_text() == "false" => {
                self.bump();
                Ok(Pattern::Literal(span, Expr::Bool(span, false)))
            }
            TokenKind::Ident => {
                let (nspan, name) = self.expect_ident("a pattern")?;
                if self.at_op(".") {
                    self.bump();
                    let (_, variant) = self.expect_ident("a variant name")?;
                    let payload = if self.at_op("(") {
                        self.parse_pattern_payload()?
                    } else {
                        Vec::new()
                    };
                    return Ok(Pattern::Variant {
                        span,
                        enum_name: Some(name),
                        variant,
                        payload,
                    });
                }
                Ok(Pattern::Binding(nspan, name))
            }
            _ => Err(self.error_here(format!(
                "expected a pattern, found `{}`",
                self.peek_display()
            ))),
        }
    }
}

// --- statements (02-language.md §8.1, §9.4, §10) -------------------------

impl Parser {
    /// Loops `parse_stmt` until `Dedent`, without consuming it — used right
    /// after an `Indent` this function's caller already consumed.
    ///
    /// Guarded entry for block nesting (`MAX_BLOCK_DEPTH`'s own doc
    /// comment): every nested suite in the language reaches its statements
    /// through here exactly once per level, so counting on entry/exit
    /// bounds native recursion depth regardless of which compound
    /// statement is doing the nesting. The loop itself lives in
    /// `parse_stmts_until_dedent_body`, unchanged below; this wrapper only
    /// ever adds the counter (`parse_unary`'s own shape).
    fn parse_stmts_until_dedent(&mut self) -> Result<Vec<Stmt>, ParseError> {
        self.block_depth += 1;
        if self.block_depth > MAX_BLOCK_DEPTH {
            self.block_depth -= 1;
            return Err(self.error_here(format!("block nesting depth exceeded {MAX_BLOCK_DEPTH}")));
        }
        let result = self.parse_stmts_until_dedent_body();
        self.block_depth -= 1;
        result
    }

    fn parse_stmts_until_dedent_body(&mut self) -> Result<Vec<Stmt>, ParseError> {
        let mut stmts = Vec::new();
        while !self.at_kind(TokenKind::Dedent) {
            if self.at_kind(TokenKind::Eof) {
                return Err(self.error_here("expected a statement, found end of file"));
            }
            stmts.push(self.parse_stmt()?);
        }
        Ok(stmts)
    }

    /// The suite following a `:` — either a normal indented block (now the
    /// path taken for every multi-statement suite, including one embedded
    /// in an enclosing `()[]{}`: the lexer's layout islands, see
    /// `lexer.rs`'s module doc comment, hand real NEWLINE/INDENT/DEDENT
    /// tokens back to the parser for exactly this case), or (see the module
    /// doc comment) the single-statement inline form when the `:` is
    /// followed by real content on the same physical line instead of a
    /// newline. Assumes the caller has already consumed the leading `:`.
    fn parse_stmt_suite(&mut self) -> Result<Vec<Stmt>, ParseError> {
        if self.at_kind(TokenKind::Newline) {
            self.bump();
            self.expect_indent()?;
            let stmts = self.parse_stmts_until_dedent()?;
            self.expect_dedent()?;
            // A suite holds at least one statement (`pass` exists for the
            // empty case). Unreachable at depth 0 — a content-free block
            // never lexes an INDENT there — but a layout island's INDENT
            // can be immediately closed by the enclosing bracket
            // (`combine(||:` then an indented `)`), which parsed as an
            // empty suite until the sema-roundtrip oracle caught it.
            if stmts.is_empty() {
                return Err(self.error_here(format!(
                    "expected a statement, found `{}`",
                    self.peek_display()
                )));
            }
            Ok(stmts)
        } else {
            self.inline_depth += 1;
            let result = self.parse_inline_stmt_seq();
            self.inline_depth -= 1;
            result
        }
    }

    /// The single-line inline suite: `:` immediately followed by real
    /// content on the same physical line, with no layout tokens at all
    /// (`if x: return 0`, or — since the lexer now only suppresses layout
    /// entirely for a suite that never sees a newline before its content —
    /// the same shape found one bracket deeper, e.g. a closure passed as a
    /// call argument written `|x|: x.size` with nothing after it on the
    /// line). Holds **exactly one** statement: with no separator token
    /// available here (no real `Newline` was ever going to appear — a
    /// `:`-newline always opens a layout island or an ordinary indented
    /// block instead, see `parse_stmt_suite`), a second statement's leading
    /// tokens are genuinely ambiguous with the first statement's own
    /// trailing expression grammar (`plans/pre-M3-findings.md`'s
    /// roundtrip-ambiguity finding) — so it is rejected outright rather than
    /// guessed at.
    fn parse_inline_stmt_seq(&mut self) -> Result<Vec<Stmt>, ParseError> {
        let stmt = self.parse_stmt()?;
        if self.at_kind(TokenKind::Newline) {
            self.bump();
        } else if !(matches!(self.peek_kind(), TokenKind::Eof | TokenKind::Dedent)
            || self.at_op(")")
            || self.at_op("]")
            || self.at_op("}")
            || self.at_op(","))
        {
            return Err(self.error_here(
                "an embedded suite on one line holds one statement; use an indented block",
            ));
        }
        Ok(vec![stmt])
    }

    /// Validates (without consuming) that a simple statement has ended:
    /// either a real `Newline` (consumed here) or one of the delimiters
    /// that bounds an embedded suite. Inside an embedded suite
    /// (`inline_depth > 0`) a missing terminator is tolerated — the next
    /// statement simply starts here with no separator token at all — but
    /// this can never mask a real error at depth 0: the lexer always
    /// inserts a genuine `Newline` there, so this fallback is unreachable
    /// outside an embedded context.
    fn end_of_simple_stmt(&mut self) -> Result<(), ParseError> {
        if self.at_kind(TokenKind::Newline) {
            self.bump();
            return Ok(());
        }
        if matches!(self.peek_kind(), TokenKind::Eof | TokenKind::Dedent)
            || self.at_op(")")
            || self.at_op("]")
            || self.at_op("}")
            || self.at_op(",")
            || self.inline_depth > 0
        {
            return Ok(());
        }
        // A simple statement's RHS expression can itself end in a nested
        // suite (a closure's `|params|: suite` body — ast-expressions
        // golden) whose own `Dedent`+`Newline` already closed out this
        // logical line; nothing is left to consume.
        if self.pos > 0 && self.tokens[self.pos - 1].kind == TokenKind::Newline {
            return Ok(());
        }
        Err(self.error_here(format!(
            "expected end of line, found `{}`",
            self.peek_display()
        )))
    }

    fn stmt_ends_here(&self) -> bool {
        matches!(
            self.peek_kind(),
            TokenKind::Newline | TokenKind::Eof | TokenKind::Dedent
        ) || self.at_op(")")
            || self.at_op("]")
            || self.at_op("}")
            || self.at_op(",")
    }

    fn parse_stmt(&mut self) -> Result<Stmt, ParseError> {
        let span = self.peek_span();
        // 02-language.md §1: a `##` doc comment is "attached to the
        // immediately following declaration". Statements are not
        // declarations, so a doc comment in a body attaches to nothing.
        // Say that, rather than letting it fall through to the expression
        // parser and report the doc text as an unexpected token.
        if self.at_kind(TokenKind::DocComment) {
            return Err(self.error_here(
                "a `##` doc comment attaches to the immediately following declaration \
                 (02-language.md §1), and a statement is not a declaration — use a plain \
                 `#` comment inside a body",
            ));
        }
        if self.at_keyword("if") {
            return self.parse_if_stmt().map(Stmt::If);
        }
        if self.at_keyword("match") {
            return self.parse_match_stmt().map(Stmt::Match);
        }
        if self.at_keyword("for") {
            return self.parse_for_stmt().map(Stmt::For);
        }
        if self.at_keyword("while") {
            return self.parse_while_stmt().map(Stmt::While);
        }
        if self.at_keyword("with") {
            return self.parse_with_stmt().map(Stmt::With);
        }
        if self.at_keyword("break") {
            self.bump();
            self.end_of_simple_stmt()?;
            return Ok(Stmt::Break(span));
        }
        if self.at_keyword("continue") {
            self.bump();
            self.end_of_simple_stmt()?;
            return Ok(Stmt::Continue(span));
        }
        if self.at_keyword("pass") {
            self.bump();
            self.end_of_simple_stmt()?;
            return Ok(Stmt::Pass(span));
        }
        if self.at_keyword("return") {
            self.bump();
            let value = if self.stmt_ends_here() {
                None
            } else {
                Some(self.parse_or()?)
            };
            self.end_of_simple_stmt()?;
            return Ok(Stmt::Return(span, value));
        }
        if self.at_keyword("assert") {
            self.bump();
            let cond = self.parse_or()?;
            let message = if self.at_op(",") {
                self.bump();
                Some(self.parse_or()?)
            } else {
                None
            };
            self.end_of_simple_stmt()?;
            return Ok(Stmt::Assert(AssertStmt {
                span,
                cond,
                message,
            }));
        }
        if self.at_keyword("defer") {
            self.bump();
            if self.at_op(":") {
                self.bump();
                let body = self.parse_stmt_suite()?;
                return Ok(Stmt::Defer(DeferStmt {
                    span,
                    body: DeferBody::Suite(body),
                }));
            }
            let e = self.parse_or()?;
            self.end_of_simple_stmt()?;
            return Ok(Stmt::Defer(DeferStmt {
                span,
                body: DeferBody::Expr(Box::new(e)),
            }));
        }
        if self.at_keyword("comptime") {
            if self.peek_is_keyword_at(1, "if") {
                return self.parse_comptime_if_stmt().map(Stmt::ComptimeIf);
            }
            if self.peek_is_keyword_at(1, "assert") {
                self.bump();
                self.bump();
                let cond = self.parse_or()?;
                let message = if self.at_op(",") {
                    self.bump();
                    Some(self.parse_or()?)
                } else {
                    None
                };
                self.end_of_simple_stmt()?;
                return Ok(Stmt::ComptimeAssert(span, cond, message));
            }
            return Err(self.error_here("expected `if` or `assert` after `comptime`"));
        }
        self.parse_assign_or_expr_stmt(span)
    }

    fn assign_op_here(&self) -> Option<AssignOp> {
        const OPS: &[(&str, AssignOp)] = &[
            ("+=", AssignOp::Add),
            ("-=", AssignOp::Sub),
            ("*=", AssignOp::Mul),
            ("/=", AssignOp::Div),
            ("%=", AssignOp::Rem),
            ("&=", AssignOp::BitAnd),
            ("|=", AssignOp::BitOr),
            ("^=", AssignOp::BitXor),
            ("<<=", AssignOp::Shl),
            (">>=", AssignOp::Shr),
            ("=", AssignOp::Assign),
        ];
        for (text, op) in OPS {
            if self.at_op(text) {
                return Some(*op);
            }
        }
        None
    }

    fn parse_assign_or_expr_stmt(&mut self, span: Span) -> Result<Stmt, ParseError> {
        let target = self.parse_or()?;
        let ty = if self.at_op(":") {
            self.bump();
            Some(self.parse_type()?)
        } else {
            None
        };
        if let Some(op) = self.assign_op_here() {
            // The left side of `=`/a compound-assign op must itself be a
            // place (name, field, index -- same shallow shape `take`/`mut`
            // operands require above): the precedence chain happily parses
            // a unary- or binary-wrapped target (`~total = ...`, or
            // `i + 2 = ...`, which a stray `i +2= 1` typo produces once
            // `+2` lexes as `+` `2`) with no complaint, and nothing
            // downstream (sema's `check_assign`) re-derives this --
            // lower.rs's `lower_place_write` and eval/interp.rs's
            // `place_mut` both *assume* it and fail with their own
            // internal-error guard when it does not hold, which is
            // exactly the fuzzer-found disagreement (`cargo xtask fuzz
            // lower` seeds 32/33; err-assign-nonplace-unary/-arith pin the
            // two minimized shapes). Rejecting here, before an
            // `AssignStmt` with a non-place target can even exist, is
            // narrower than teaching every later pass to re-check the
            // same shape.
            if !is_place_expr(&target) {
                return Err(self.error_here(
                    "the left side of an assignment must be a place expression (name, field, index)",
                ));
            }
            self.bump();
            let value = self.parse_or()?;
            self.end_of_simple_stmt()?;
            return Ok(Stmt::Assign(AssignStmt {
                span,
                target,
                ty,
                op,
                value,
            }));
        }
        if ty.is_some() {
            return Err(self.error_here("expected `=` after a type annotation"));
        }
        self.end_of_simple_stmt()?;
        // `send actor.method(...)` parses through the general expression
        // grammar (parse_unary recognizes the `send` prefix) but is a
        // named statement form (02-language.md §9.4) — re-tag it here
        // rather than leaving it as a generic expression statement.
        if let Expr::Send(s, inner) = target {
            return Ok(Stmt::Send(s, *inner));
        }
        Ok(Stmt::Expr(span, target))
    }

    fn parse_if_stmt(&mut self) -> Result<IfStmt, ParseError> {
        let span = self.expect_keyword("if")?;
        let cond = self.parse_or()?;
        self.expect_op(":")?;
        let then_branch = self.parse_stmt_suite()?;
        let mut elifs = Vec::new();
        while self.at_keyword("elif") {
            let espan = self.peek_span();
            self.bump();
            let econd = self.parse_or()?;
            self.expect_op(":")?;
            let ebody = self.parse_stmt_suite()?;
            elifs.push(ElifClause {
                span: espan,
                cond: econd,
                body: ebody,
            });
        }
        let else_branch = if self.at_keyword("else") {
            self.bump();
            self.expect_op(":")?;
            Some(self.parse_stmt_suite()?)
        } else {
            None
        };
        Ok(IfStmt {
            span,
            cond,
            then_branch,
            elifs,
            else_branch,
        })
    }

    fn parse_match_arm(&mut self) -> Result<MatchArm, ParseError> {
        let span = self.expect_keyword("case")?;
        let pattern = self.parse_pattern()?;
        let guard = if self.at_keyword("if") {
            self.bump();
            Some(self.parse_or()?)
        } else {
            None
        };
        self.expect_op(":")?;
        let body = self.parse_stmt_suite()?;
        Ok(MatchArm {
            span,
            pattern,
            guard,
            body,
        })
    }

    fn parse_match_stmt(&mut self) -> Result<MatchStmt, ParseError> {
        let span = self.expect_keyword("match")?;
        let scrutinee = self.parse_or()?;
        self.expect_op(":")?;
        self.expect_newline()?;
        self.expect_indent()?;
        let mut arms = Vec::new();
        while !self.at_kind(TokenKind::Dedent) {
            if self.at_kind(TokenKind::Eof) {
                return Err(self.error_here("expected a `case` arm, found end of file"));
            }
            arms.push(self.parse_match_arm()?);
        }
        self.expect_dedent()?;
        Ok(MatchStmt {
            span,
            scrutinee,
            arms,
        })
    }

    fn parse_for_stmt(&mut self) -> Result<ForStmt, ParseError> {
        let span = self.expect_keyword("for")?;
        let take_binding = if self.at_keyword("take") {
            self.bump();
            true
        } else {
            false
        };
        let (_, name) = self.expect_ident("a loop variable name")?;
        self.expect_keyword("in")?;
        let iterable = self.parse_or()?;
        self.expect_op(":")?;
        let body = self.parse_stmt_suite()?;
        Ok(ForStmt {
            span,
            take_binding,
            name,
            iterable,
            body,
        })
    }

    fn parse_while_stmt(&mut self) -> Result<WhileStmt, ParseError> {
        let span = self.expect_keyword("while")?;
        let cond = self.parse_or()?;
        self.expect_op(":")?;
        let body = self.parse_stmt_suite()?;
        Ok(WhileStmt { span, cond, body })
    }

    fn parse_with_stmt(&mut self) -> Result<WithStmt, ParseError> {
        let span = self.expect_keyword("with")?;
        let expr = self.parse_or()?;
        let as_name = if self.at_ident_text("as") {
            self.bump();
            Some(self.expect_ident("a name")?.1)
        } else {
            None
        };
        self.expect_op(":")?;
        let body = self.parse_stmt_suite()?;
        Ok(WithStmt {
            span,
            expr,
            as_name,
            body,
        })
    }

    fn parse_comptime_if_stmt(&mut self) -> Result<ComptimeIfStmt, ParseError> {
        let span = self.peek_span();
        self.expect_keyword("comptime")?;
        self.expect_keyword("if")?;
        let cond = self.parse_or()?;
        self.expect_op(":")?;
        let then_branch = self.parse_stmt_suite()?;
        let else_branch = if self.at_keyword("comptime") && self.peek_is_keyword_at(1, "else") {
            self.bump();
            self.bump();
            self.expect_op(":")?;
            Some(self.parse_stmt_suite()?)
        } else {
            None
        };
        Ok(ComptimeIfStmt {
            span,
            cond,
            then_branch,
            else_branch,
        })
    }
}

// --- dump ------------------------------------------------------------------
//
// Stable text dump (plans/M1.md decision 5): one node per line, two-space
// child indent, `Kind @line:col key=value`, string payloads quoted. Source
// order throughout, so the dump is deterministic by construction. Every
// statement-holding construct wraps its branch/body in an explicit `Then` /
// `Else` / `Body` / `Case` / `Guard` / `Message` node so two adjacent
// expression children are never ambiguous about which role they play.
//
// Every helper below threads a `strip: bool` flag alongside `depth`: when
// true, the `@line:col` part of every node header is omitted entirely
// (`hdr` below is the single place that decides). This is plumbing for the
// roundtrip oracle (plans/M1.md item E, `xtask roundtrip`), which compares
// a dump of the original parse against a dump of the pretty-printed-then-
// reparsed result — the two ASTs are structurally identical but their
// spans necessarily differ, since the pretty-printed text is laid out
// differently from the original source. Adding the mode here (rather than
// stripping spans out of the rendered text after the fact) keeps the
// stripped dump an actual property of the AST, not a text-hack.

pub fn dump(module: &Module) -> String {
    let mut out = String::new();
    dump_module(module, 0, false, &mut out);
    out
}

/// Same as `dump`, but every `@line:col` span is omitted.
pub fn dump_no_spans(module: &Module) -> String {
    let mut out = String::new();
    dump_module(module, 0, true, &mut out);
    out
}

/// Dumps a bare fragment (`parse_fragment`'s result): each top-level item
/// or statement in source order, same node format as `dump`, but with no
/// enclosing `Module` header (a fragment has no module path).
pub fn dump_fragment(entries: &[FragmentEntry]) -> String {
    let mut out = String::new();
    dump_fragment_entries(entries, 0, false, &mut out);
    out
}

/// Same as `dump_fragment`, but every `@line:col` span is omitted.
pub fn dump_fragment_no_spans(entries: &[FragmentEntry]) -> String {
    let mut out = String::new();
    dump_fragment_entries(entries, 0, true, &mut out);
    out
}

fn dump_fragment_entries(entries: &[FragmentEntry], depth: usize, strip: bool, out: &mut String) {
    for entry in entries {
        match entry {
            FragmentEntry::Item(item) => dump_item(item, depth, strip, out),
            FragmentEntry::Stmt(stmt) => dump_stmt(stmt, depth, strip, out),
        }
    }
}

fn push_line(out: &mut String, depth: usize, line: &str) {
    for _ in 0..depth {
        out.push_str("  ");
    }
    out.push_str(line);
    out.push('\n');
}

fn quote(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            _ => out.push(c),
        }
    }
    out.push('"');
    out
}

/// A dump line's `Kind` (spans stripped) or `Kind @line:col` (spans kept)
/// header — the one place that decides, per `strip`.
fn hdr(strip: bool, kind: &str, span: Span) -> String {
    if strip {
        kind.to_string()
    } else {
        format!("{kind} @{}:{}", span.line, span.col)
    }
}

fn dump_doc(doc: &Option<Doc>, depth: usize, strip: bool, out: &mut String) {
    if let Some(doc) = doc {
        push_line(
            out,
            depth,
            &format!("{} text={}", hdr(strip, "Doc", doc.span), quote(&doc.text)),
        );
    }
}

fn dump_attrs(attrs: &[Attr], depth: usize, strip: bool, out: &mut String) {
    for attr in attrs {
        push_line(
            out,
            depth,
            &format!("{} name={}", hdr(strip, "Attr", attr.span), attr.name),
        );
        for arg in &attr.args {
            dump_arg(arg, depth + 1, strip, out);
        }
    }
}

fn dump_module(m: &Module, depth: usize, strip: bool, out: &mut String) {
    push_line(
        out,
        depth,
        &format!("{} path={}", hdr(strip, "Module", m.span), m.path.join(".")),
    );
    dump_doc(&m.doc, depth + 1, strip, out);
    for import in &m.imports {
        dump_import(import, depth + 1, strip, out);
    }
    for item in &m.items {
        dump_item(item, depth + 1, strip, out);
    }
}

fn dump_import(import: &Import, depth: usize, strip: bool, out: &mut String) {
    let mut header = format!(
        "{} from={}",
        hdr(strip, "Import", import.span),
        import.path.join(".")
    );
    if import.is_pub {
        header.push_str(" pub=true");
    }
    push_line(out, depth, &header);
    for name in &import.names {
        let mut line = format!("{} name={}", hdr(strip, "ImportName", name.span), name.name);
        if let Some(alias) = &name.alias {
            line.push_str(&format!(" alias={alias}"));
        }
        push_line(out, depth + 1, &line);
    }
}

fn dump_generics(generics: &[GenericParam], depth: usize, strip: bool, out: &mut String) {
    for g in generics {
        match g {
            GenericParam::Type { span, name } => {
                push_line(
                    out,
                    depth,
                    &format!("{} name={}", hdr(strip, "GenericType", *span), name),
                );
            }
            GenericParam::Const { span, name, ty } => {
                push_line(
                    out,
                    depth,
                    &format!("{} name={}", hdr(strip, "GenericConst", *span), name),
                );
                dump_type(ty, depth + 1, strip, out);
            }
        }
    }
}

fn dump_receiver(receiver: &Receiver, depth: usize, strip: bool, out: &mut String) {
    push_line(
        out,
        depth,
        &format!(
            "{} mode={}",
            hdr(strip, "Receiver", receiver.span),
            receiver.mode.as_str()
        ),
    );
}

fn dump_param(param: &Param, depth: usize, strip: bool, out: &mut String) {
    push_line(
        out,
        depth,
        &format!(
            "{} name={} mode={}",
            hdr(strip, "Param", param.span),
            param.name,
            param.mode.as_str()
        ),
    );
    dump_type(&param.ty, depth + 1, strip, out);
    if let Some(default) = &param.default {
        dump_expr(default, depth + 1, strip, out);
    }
}

#[allow(clippy::too_many_arguments)]
fn dump_fn_common(
    generics: &[GenericParam],
    receiver: &Option<Receiver>,
    params: &[Param],
    ret: &Option<Type>,
    body: &Option<Vec<Stmt>>,
    depth: usize,
    strip: bool,
    out: &mut String,
) {
    dump_generics(generics, depth, strip, out);
    if let Some(r) = receiver {
        dump_receiver(r, depth, strip, out);
    }
    for p in params {
        dump_param(p, depth, strip, out);
    }
    if let Some(ret) = ret {
        dump_type(ret, depth, strip, out);
    }
    if let Some(body) = body {
        push_line(out, depth, "Body");
        for stmt in body {
            dump_stmt(stmt, depth + 1, strip, out);
        }
    }
}

fn dump_item(item: &Item, depth: usize, strip: bool, out: &mut String) {
    match item {
        Item::Const(c) => {
            let mut header = format!("{} name={}", hdr(strip, "Const", c.span), c.name);
            if c.is_pub {
                header.push_str(" pub=true");
            }
            push_line(out, depth, &header);
            dump_doc(&c.doc, depth + 1, strip, out);
            dump_attrs(&c.attrs, depth + 1, strip, out);
            if let Some(ty) = &c.ty {
                dump_type(ty, depth + 1, strip, out);
            }
            dump_expr(&c.value, depth + 1, strip, out);
        }
        Item::Fn(f) => {
            let mut header = format!("{} name={}", hdr(strip, "Fn", f.span), f.name);
            if f.is_pub {
                header.push_str(" pub=true");
            }
            if f.is_async {
                header.push_str(" async=true");
            }
            push_line(out, depth, &header);
            dump_doc(&f.doc, depth + 1, strip, out);
            dump_attrs(&f.attrs, depth + 1, strip, out);
            dump_fn_common(
                &f.generics,
                &f.receiver,
                &f.params,
                &f.ret,
                &f.body,
                depth + 1,
                strip,
                out,
            );
        }
        Item::Struct(s) => {
            let mut header = format!("{} name={}", hdr(strip, "Struct", s.span), s.name);
            if s.is_pub {
                header.push_str(" pub=true");
            }
            if s.is_resource {
                header.push_str(" resource=true");
            }
            if !s.deriving.is_empty() {
                header.push_str(&format!(" deriving={}", s.deriving.join(",")));
            }
            push_line(out, depth, &header);
            dump_doc(&s.doc, depth + 1, strip, out);
            dump_attrs(&s.attrs, depth + 1, strip, out);
            dump_generics(&s.generics, depth + 1, strip, out);
            for m in &s.members {
                dump_member(m, depth + 1, strip, out);
            }
        }
        Item::Enum(e) => {
            let mut header = format!("{} name={}", hdr(strip, "Enum", e.span), e.name);
            if e.is_pub {
                header.push_str(" pub=true");
            }
            if !e.deriving.is_empty() {
                header.push_str(&format!(" deriving={}", e.deriving.join(",")));
            }
            push_line(out, depth, &header);
            dump_doc(&e.doc, depth + 1, strip, out);
            dump_attrs(&e.attrs, depth + 1, strip, out);
            dump_generics(&e.generics, depth + 1, strip, out);
            for v in &e.variants {
                dump_variant(v, depth + 1, strip, out);
            }
            for m in &e.members {
                dump_member(m, depth + 1, strip, out);
            }
        }
        Item::Pool(p) => {
            let header = format!("{} name={}", hdr(strip, "Pool", p.span), p.name);
            push_line(out, depth, &header);
            dump_doc(&p.doc, depth + 1, strip, out);
            dump_attrs(&p.attrs, depth + 1, strip, out);
        }
        Item::ComptimeIf(c) => {
            let header = hdr(strip, "ComptimeIf", c.span);
            push_line(out, depth, &header);
            dump_doc(&c.doc, depth + 1, strip, out);
            dump_attrs(&c.attrs, depth + 1, strip, out);
            dump_expr(&c.cond, depth + 1, strip, out);
            push_line(out, depth + 1, "Then");
            for it in &c.then_branch {
                dump_item(it, depth + 2, strip, out);
            }
            if let Some(else_branch) = &c.else_branch {
                push_line(out, depth + 1, "Else");
                for it in else_branch {
                    dump_item(it, depth + 2, strip, out);
                }
            }
        }
    }
}

fn dump_member(member: &Member, depth: usize, strip: bool, out: &mut String) {
    match member {
        Member::Field(f) => {
            let mut header = format!("{} name={}", hdr(strip, "Field", f.span), f.name);
            if f.is_pub {
                header.push_str(" pub=true");
            }
            push_line(out, depth, &header);
            dump_doc(&f.doc, depth + 1, strip, out);
            dump_attrs(&f.attrs, depth + 1, strip, out);
            dump_type(&f.ty, depth + 1, strip, out);
            if let Some(default) = &f.default {
                dump_expr(default, depth + 1, strip, out);
            }
        }
        Member::Fn(f) => {
            let mut header = format!("{} name={}", hdr(strip, "Fn", f.span), f.name);
            if f.is_pub {
                header.push_str(" pub=true");
            }
            if f.is_async {
                header.push_str(" async=true");
            }
            push_line(out, depth, &header);
            dump_doc(&f.doc, depth + 1, strip, out);
            dump_attrs(&f.attrs, depth + 1, strip, out);
            dump_fn_common(
                &f.generics,
                &f.receiver,
                &f.params,
                &f.ret,
                &f.body,
                depth + 1,
                strip,
                out,
            );
        }
        Member::Init(i) => {
            push_line(out, depth, &hdr(strip, "Init", i.span));
            dump_doc(&i.doc, depth + 1, strip, out);
            dump_attrs(&i.attrs, depth + 1, strip, out);
            dump_fn_common(
                &[],
                &Some(i.receiver.clone()),
                &i.params,
                &i.ret,
                &Some(i.body.clone()),
                depth + 1,
                strip,
                out,
            );
        }
        Member::Pool(p) => {
            let header = format!("{} name={}", hdr(strip, "Pool", p.span), p.name);
            push_line(out, depth, &header);
            dump_doc(&p.doc, depth + 1, strip, out);
            dump_attrs(&p.attrs, depth + 1, strip, out);
        }
        Member::ComptimeIf(c) => {
            let header = hdr(strip, "ComptimeIf", c.span);
            push_line(out, depth, &header);
            dump_doc(&c.doc, depth + 1, strip, out);
            dump_attrs(&c.attrs, depth + 1, strip, out);
            dump_expr(&c.cond, depth + 1, strip, out);
            push_line(out, depth + 1, "Then");
            for m in &c.then_branch {
                dump_member(m, depth + 2, strip, out);
            }
            if let Some(else_branch) = &c.else_branch {
                push_line(out, depth + 1, "Else");
                for m in else_branch {
                    dump_member(m, depth + 2, strip, out);
                }
            }
        }
    }
}

fn dump_variant(v: &Variant, depth: usize, strip: bool, out: &mut String) {
    push_line(
        out,
        depth,
        &format!("{} name={}", hdr(strip, "Variant", v.span), v.name),
    );
    match &v.payload {
        VariantPayload::None => {}
        VariantPayload::Tuple(types) => {
            for ty in types {
                dump_type(ty, depth + 1, strip, out);
            }
        }
        VariantPayload::Named(fields) => {
            for f in fields {
                push_line(
                    out,
                    depth + 1,
                    &format!("{} name={}", hdr(strip, "VariantField", f.span), f.name),
                );
                dump_type(&f.ty, depth + 2, strip, out);
            }
        }
    }
}

fn dump_type(ty: &Type, depth: usize, strip: bool, out: &mut String) {
    match ty {
        Type::Named(t) => {
            push_line(
                out,
                depth,
                &format!("{} name={}", hdr(strip, "TypeName", t.span), t.name),
            );
            for arg in &t.args {
                dump_generic_arg(arg, depth + 1, strip, out);
            }
        }
        Type::Array(t) => {
            push_line(out, depth, &hdr(strip, "ArrayType", t.span));
            dump_type(&t.elem, depth + 1, strip, out);
            dump_expr(&t.len, depth + 1, strip, out);
        }
        Type::Tuple(t) => {
            push_line(out, depth, &hdr(strip, "TupleType", t.span));
            for elem in &t.elems {
                dump_type(elem, depth + 1, strip, out);
            }
        }
        Type::Own(t) => {
            push_line(
                out,
                depth,
                &format!(
                    "{} pool={}",
                    hdr(strip, "OwnType", t.span),
                    t.pool.join(".")
                ),
            );
            dump_type(&t.inner, depth + 1, strip, out);
        }
        Type::Fn(t) => {
            push_line(out, depth, &hdr(strip, "FnType", t.span));
            for p in &t.params {
                push_line(
                    out,
                    depth + 1,
                    &format!(
                        "{} mode={}",
                        hdr(strip, "FnTypeParam", p.span),
                        p.mode.as_str()
                    ),
                );
                dump_type(&p.ty, depth + 2, strip, out);
            }
            if let Some(ret) = &t.ret {
                dump_type(ret, depth + 1, strip, out);
            }
        }
    }
}

fn dump_generic_arg(arg: &GenericArg, depth: usize, strip: bool, out: &mut String) {
    match arg {
        GenericArg::Type(t) => dump_type(t, depth, strip, out),
        GenericArg::Expr(e) => dump_expr(e, depth, strip, out),
        GenericArg::Bound(e) => {
            push_line(out, depth, &hdr(strip, "Bound", e.span()));
            dump_expr(e, depth + 1, strip, out);
        }
    }
}

fn dump_pattern(p: &Pattern, depth: usize, strip: bool, out: &mut String) {
    match p {
        Pattern::Wildcard(s) => push_line(out, depth, &hdr(strip, "Wildcard", *s)),
        Pattern::Literal(s, e) => {
            push_line(out, depth, &hdr(strip, "PatternLiteral", *s));
            dump_expr(e, depth + 1, strip, out);
        }
        Pattern::Binding(s, name) => {
            push_line(
                out,
                depth,
                &format!("{} name={}", hdr(strip, "Binding", *s), name),
            );
        }
        Pattern::Take(s, inner) => {
            push_line(out, depth, &hdr(strip, "TakePattern", *s));
            dump_pattern(inner, depth + 1, strip, out);
        }
        Pattern::Variant {
            span,
            enum_name,
            variant,
            payload,
        } => {
            let mut header = format!(
                "{} variant={}",
                hdr(strip, "VariantPattern", *span),
                variant
            );
            if let Some(en) = enum_name {
                header.push_str(&format!(" enum={en}"));
            }
            push_line(out, depth, &header);
            for pat in payload {
                dump_pattern(pat, depth + 1, strip, out);
            }
        }
        Pattern::Tuple(s, elems) => {
            push_line(out, depth, &hdr(strip, "TuplePattern", *s));
            for e in elems {
                dump_pattern(e, depth + 1, strip, out);
            }
        }
        Pattern::Array(s, elems) => {
            push_line(out, depth, &hdr(strip, "ArrayPattern", *s));
            for e in elems {
                dump_pattern(e, depth + 1, strip, out);
            }
        }
        Pattern::Or(s, alts) => {
            push_line(out, depth, &hdr(strip, "OrPattern", *s));
            for a in alts {
                dump_pattern(a, depth + 1, strip, out);
            }
        }
    }
}

fn dump_arg(arg: &Arg, depth: usize, strip: bool, out: &mut String) {
    let mut header = hdr(strip, "Arg", arg.span);
    if let Some(label) = &arg.label {
        header.push_str(&format!(" label={label}"));
    }
    if arg.mode != AccessMode::Read {
        header.push_str(&format!(" mode={}", arg.mode.as_str()));
    }
    push_line(out, depth, &header);
    dump_expr(&arg.value, depth + 1, strip, out);
}

fn dump_expr(e: &Expr, depth: usize, strip: bool, out: &mut String) {
    match e {
        Expr::Int(s, text) => push_line(
            out,
            depth,
            &format!("{} text={}", hdr(strip, "Int", *s), text),
        ),
        Expr::Float(s, text) => push_line(
            out,
            depth,
            &format!("{} text={}", hdr(strip, "Float", *s), text),
        ),
        // Str/BStr/Char keep the lexer's raw token text, which already
        // includes the source's own delimiting quotes (and a `b`/`f`
        // prefix) — printed as-is rather than double-quoted; the lexer
        // guarantees no raw newline can appear inside, so this stays a
        // single dump line.
        Expr::Str(s, text) => push_line(
            out,
            depth,
            &format!("{} text={}", hdr(strip, "Str", *s), text),
        ),
        Expr::BStr(s, text) => push_line(
            out,
            depth,
            &format!("{} text={}", hdr(strip, "BStr", *s), text),
        ),
        Expr::Char(s, text) => push_line(
            out,
            depth,
            &format!("{} text={}", hdr(strip, "Char", *s), text),
        ),
        Expr::FStr(f) => {
            push_line(out, depth, &hdr(strip, "FStr", f.span));
            for part in &f.parts {
                match part {
                    FStringPart::Literal(s, text) => push_line(
                        out,
                        depth + 1,
                        &format!("{} text={}", hdr(strip, "Literal", *s), quote(text)),
                    ),
                    FStringPart::Interp(s, text) => push_line(
                        out,
                        depth + 1,
                        &format!("{} text={}", hdr(strip, "Interp", *s), quote(text)),
                    ),
                }
            }
        }
        Expr::Bool(s, v) => push_line(
            out,
            depth,
            &format!("{} value={}", hdr(strip, "Bool", *s), v),
        ),
        Expr::Unit(s) => push_line(out, depth, &hdr(strip, "Unit", *s)),
        Expr::Name(s, name) => push_line(
            out,
            depth,
            &format!("{} name={}", hdr(strip, "Name", *s), name),
        ),
        Expr::Field(base, s, name) => {
            push_line(
                out,
                depth,
                &format!("{} name={}", hdr(strip, "Field", *s), name),
            );
            dump_expr(base, depth + 1, strip, out);
        }
        Expr::Index(base, s, args) => {
            push_line(out, depth, &hdr(strip, "Index", *s));
            dump_expr(base, depth + 1, strip, out);
            for a in args {
                dump_expr(a, depth + 1, strip, out);
            }
        }
        Expr::Call(callee, s, args) => {
            push_line(out, depth, &hdr(strip, "Call", *s));
            dump_expr(callee, depth + 1, strip, out);
            for a in args {
                dump_arg(a, depth + 1, strip, out);
            }
        }
        Expr::Unary(s, op, inner) => {
            let name = match op {
                UnaryOp::Neg => "neg",
                UnaryOp::BitNot => "bitnot",
                UnaryOp::Await => "await",
                UnaryOp::Take => "take",
            };
            push_line(
                out,
                depth,
                &format!("{} op={}", hdr(strip, "Unary", *s), name),
            );
            dump_expr(inner, depth + 1, strip, out);
        }
        Expr::Try(s, inner) => {
            push_line(out, depth, &hdr(strip, "Try", *s));
            dump_expr(inner, depth + 1, strip, out);
        }
        Expr::Binary(s, op, l, r) => {
            push_line(
                out,
                depth,
                &format!("{} op={}", hdr(strip, "Binary", *s), op.as_str()),
            );
            dump_expr(l, depth + 1, strip, out);
            dump_expr(r, depth + 1, strip, out);
        }
        Expr::Range(s, l, r, inclusive) => {
            push_line(
                out,
                depth,
                &format!("{} inclusive={}", hdr(strip, "Range", *s), inclusive),
            );
            dump_expr(l, depth + 1, strip, out);
            dump_expr(r, depth + 1, strip, out);
        }
        Expr::Is(s, l, pat) => {
            push_line(out, depth, &hdr(strip, "Is", *s));
            dump_expr(l, depth + 1, strip, out);
            dump_pattern(pat, depth + 1, strip, out);
        }
        Expr::Not(s, inner) => {
            push_line(out, depth, &hdr(strip, "Not", *s));
            dump_expr(inner, depth + 1, strip, out);
        }
        Expr::And(s, l, r) => {
            push_line(out, depth, &hdr(strip, "And", *s));
            dump_expr(l, depth + 1, strip, out);
            dump_expr(r, depth + 1, strip, out);
        }
        Expr::Or(s, l, r) => {
            push_line(out, depth, &hdr(strip, "Or", *s));
            dump_expr(l, depth + 1, strip, out);
            dump_expr(r, depth + 1, strip, out);
        }
        Expr::DotVariant(s, name, args) => {
            push_line(
                out,
                depth,
                &format!("{} variant={}", hdr(strip, "DotVariant", *s), name),
            );
            for a in args {
                dump_arg(a, depth + 1, strip, out);
            }
        }
        Expr::Closure(c) => {
            push_line(out, depth, &hdr(strip, "Closure", c.span));
            for p in &c.params {
                let mut header = format!(
                    "{} name={} mode={}",
                    hdr(strip, "ClosureParam", p.span),
                    p.name,
                    p.mode.as_str()
                );
                if p.ty.is_none() {
                    header.push_str(" untyped=true");
                }
                push_line(out, depth + 1, &header);
                if let Some(ty) = &p.ty {
                    dump_type(ty, depth + 2, strip, out);
                }
            }
            match &c.body {
                ClosureBody::Expr(e) => dump_expr(e, depth + 1, strip, out),
                ClosureBody::Suite(stmts) => {
                    push_line(out, depth + 1, "Body");
                    for st in stmts {
                        dump_stmt(st, depth + 2, strip, out);
                    }
                }
            }
        }
        Expr::Send(s, inner) => {
            push_line(out, depth, &hdr(strip, "Send", *s));
            dump_expr(inner, depth + 1, strip, out);
        }
        Expr::Tuple(s, elems) => {
            push_line(out, depth, &hdr(strip, "Tuple", *s));
            for e in elems {
                dump_expr(e, depth + 1, strip, out);
            }
        }
        Expr::List(s, elems) => {
            push_line(out, depth, &hdr(strip, "List", *s));
            for e in elems {
                dump_expr(e, depth + 1, strip, out);
            }
        }
        Expr::ArrayRepeat(s, elem, count) => {
            push_line(out, depth, &hdr(strip, "ArrayRepeat", *s));
            dump_expr(elem, depth + 1, strip, out);
            dump_expr(count, depth + 1, strip, out);
        }
    }
}

fn dump_stmts(stmts: &[Stmt], depth: usize, strip: bool, out: &mut String) {
    for s in stmts {
        dump_stmt(s, depth, strip, out);
    }
}

fn dump_stmt(stmt: &Stmt, depth: usize, strip: bool, out: &mut String) {
    match stmt {
        Stmt::Assign(a) => {
            push_line(
                out,
                depth,
                &format!("{} op={}", hdr(strip, "Assign", a.span), a.op.as_str()),
            );
            dump_expr(&a.target, depth + 1, strip, out);
            if let Some(ty) = &a.ty {
                dump_type(ty, depth + 1, strip, out);
            }
            dump_expr(&a.value, depth + 1, strip, out);
        }
        Stmt::If(i) => {
            push_line(out, depth, &hdr(strip, "If", i.span));
            dump_expr(&i.cond, depth + 1, strip, out);
            push_line(out, depth + 1, "Then");
            dump_stmts(&i.then_branch, depth + 2, strip, out);
            for elif in &i.elifs {
                push_line(out, depth + 1, &hdr(strip, "Elif", elif.span));
                dump_expr(&elif.cond, depth + 2, strip, out);
                dump_stmts(&elif.body, depth + 2, strip, out);
            }
            if let Some(else_branch) = &i.else_branch {
                push_line(out, depth + 1, "Else");
                dump_stmts(else_branch, depth + 2, strip, out);
            }
        }
        Stmt::Match(m) => {
            push_line(out, depth, &hdr(strip, "Match", m.span));
            dump_expr(&m.scrutinee, depth + 1, strip, out);
            for arm in &m.arms {
                push_line(out, depth + 1, &hdr(strip, "Case", arm.span));
                dump_pattern(&arm.pattern, depth + 2, strip, out);
                if let Some(guard) = &arm.guard {
                    push_line(out, depth + 2, "Guard");
                    dump_expr(guard, depth + 3, strip, out);
                }
                dump_stmts(&arm.body, depth + 2, strip, out);
            }
        }
        Stmt::For(f) => {
            let mut header = format!("{} name={}", hdr(strip, "For", f.span), f.name);
            if f.take_binding {
                header.push_str(" take=true");
            }
            push_line(out, depth, &header);
            dump_expr(&f.iterable, depth + 1, strip, out);
            push_line(out, depth + 1, "Body");
            dump_stmts(&f.body, depth + 2, strip, out);
        }
        Stmt::While(w) => {
            push_line(out, depth, &hdr(strip, "While", w.span));
            dump_expr(&w.cond, depth + 1, strip, out);
            push_line(out, depth + 1, "Body");
            dump_stmts(&w.body, depth + 2, strip, out);
        }
        Stmt::Break(s) => push_line(out, depth, &hdr(strip, "Break", *s)),
        Stmt::Continue(s) => push_line(out, depth, &hdr(strip, "Continue", *s)),
        Stmt::Pass(s) => push_line(out, depth, &hdr(strip, "Pass", *s)),
        Stmt::Return(s, value) => {
            push_line(out, depth, &hdr(strip, "Return", *s));
            if let Some(v) = value {
                dump_expr(v, depth + 1, strip, out);
            }
        }
        Stmt::Assert(a) => {
            push_line(out, depth, &hdr(strip, "Assert", a.span));
            dump_expr(&a.cond, depth + 1, strip, out);
            if let Some(msg) = &a.message {
                push_line(out, depth + 1, "Message");
                dump_expr(msg, depth + 2, strip, out);
            }
        }
        Stmt::Defer(d) => {
            push_line(out, depth, &hdr(strip, "Defer", d.span));
            match &d.body {
                DeferBody::Expr(e) => dump_expr(e, depth + 1, strip, out),
                DeferBody::Suite(stmts) => {
                    push_line(out, depth + 1, "Body");
                    dump_stmts(stmts, depth + 2, strip, out);
                }
            }
        }
        Stmt::With(w) => {
            let mut header = hdr(strip, "With", w.span);
            if let Some(name) = &w.as_name {
                header.push_str(&format!(" as={name}"));
            }
            push_line(out, depth, &header);
            dump_expr(&w.expr, depth + 1, strip, out);
            push_line(out, depth + 1, "Body");
            dump_stmts(&w.body, depth + 2, strip, out);
        }
        Stmt::Send(s, e) => {
            push_line(out, depth, &hdr(strip, "Send", *s));
            dump_expr(e, depth + 1, strip, out);
        }
        Stmt::Expr(s, e) => {
            push_line(out, depth, &hdr(strip, "ExprStmt", *s));
            dump_expr(e, depth + 1, strip, out);
        }
        Stmt::ComptimeIf(c) => {
            push_line(out, depth, &hdr(strip, "ComptimeIf", c.span));
            dump_expr(&c.cond, depth + 1, strip, out);
            push_line(out, depth + 1, "Then");
            dump_stmts(&c.then_branch, depth + 2, strip, out);
            if let Some(else_branch) = &c.else_branch {
                push_line(out, depth + 1, "Else");
                dump_stmts(else_branch, depth + 2, strip, out);
            }
        }
        Stmt::ComptimeAssert(s, cond, message) => {
            push_line(out, depth, &hdr(strip, "ComptimeAssert", *s));
            dump_expr(cond, depth + 1, strip, out);
            if let Some(msg) = message {
                push_line(out, depth + 1, "Message");
                dump_expr(msg, depth + 2, strip, out);
            }
        }
    }
}
