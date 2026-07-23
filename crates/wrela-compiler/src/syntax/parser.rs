//! Recursive-descent parser: token stream -> `ast::Module` (02-language.md
//! §1, §2). A hand-written `Parser` struct with `pos` plus small
//! peek/bump/expect helpers (ROADMAP.md: no combinators, no traits, no
//! lookahead machinery) — dumb and correct.
//!
//! Item C (plans/M1.md) lands the spine only: the `module` header, every
//! import form (aliases, the parenthesized multi-line list, `pub from`),
//! doc-comment attachment, and top-level dispatch that recognizes which
//! `Item` each declaration starts. Each item's header (its flags and name)
//! is parsed; everything after is skipped by tracking INDENT/DEDENT depth
//! to the declaration's end, so the whole module parses at the item level
//! today without pretending to understand bodies (item D replaces the
//! skip). Errors are fail-fast (plans/M1.md decision 2): the first one stops
//! parsing.

use super::ast::{
    Attr, ComptimeIfItem, ConstItem, Doc, EnumItem, FnItem, Import, ImportName, Item, Module,
    PoolItem, Span, StructItem,
};
use super::lexer::{Token, TokenKind};

#[derive(Debug)]
pub struct ParseError {
    pub message: String,
    pub line: u32,
    pub col: u32,
}

pub fn parse(tokens: Vec<Token>) -> Result<Module, ParseError> {
    Parser::new(tokens).parse_module()
}

struct Parser {
    tokens: Vec<Token>,
    pos: usize,
}

impl Parser {
    fn new(tokens: Vec<Token>) -> Self {
        assert!(
            !tokens.is_empty() && tokens.last().unwrap().kind == TokenKind::Eof,
            "token stream must end with Eof"
        );
        Parser { tokens, pos: 0 }
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

    fn expect_ident(&mut self, what: &str) -> Result<(Span, String), ParseError> {
        if self.at_kind(TokenKind::Ident) {
            let span = self.peek_span();
            let text = self.bump().text;
            Ok((span, text))
        } else {
            Err(self.error_here(format!("expected {what}, found `{}`", self.peek_display())))
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

    // --- module ----------------------------------------------------------

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

    fn parse_dotted_path(&mut self) -> Result<Vec<String>, ParseError> {
        let mut path = vec![self.expect_ident("a path segment")?.1];
        while self.at_op(".") {
            self.bump();
            path.push(self.expect_ident("a path segment")?.1);
        }
        Ok(path)
    }

    // --- imports -----------------------------------------------------------

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

    // --- top-level items -----------------------------------------------

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

    /// A run of `##` doc-comment lines and `@name(...)` attributes,
    /// interleaved in any order, immediately before a declaration. Doc
    /// lines join with `\n`; attributes are recorded in source order as raw
    /// text (item D interprets them — this is recognize-and-record only).
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

    /// `@name` or `@name(...)`, recorded verbatim as raw text — the
    /// argument list is opaque to item C, only balanced for skipping.
    fn parse_attr(&mut self) -> Result<Attr, ParseError> {
        let span = self.expect_op("@")?;
        let (_, name) = self.expect_ident("an attribute name")?;
        let mut text = format!("@{name}");
        if self.at_op("(") {
            self.bump();
            text.push('(');
            let mut depth: u32 = 1;
            let mut first = true;
            loop {
                if self.at_kind(TokenKind::Eof) {
                    return Err(self.error_here("unterminated attribute argument list"));
                }
                if self.at_op("(") {
                    depth += 1;
                } else if self.at_op(")") {
                    depth -= 1;
                    if depth == 0 {
                        self.bump();
                        text.push(')');
                        break;
                    }
                }
                if !first {
                    text.push(' ');
                }
                text.push_str(&self.peek_text());
                first = false;
                self.bump();
            }
        }
        Ok(Attr { span, text })
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
            let (_, name) = self.expect_ident("a function name")?;
            let todo = self.skip_declaration_remainder();
            return Ok(Item::Fn(FnItem {
                span: start,
                name,
                is_pub,
                is_async: true,
                doc,
                attrs,
                todo,
            }));
        }
        if self.at_keyword("fn") {
            self.bump();
            let (_, name) = self.expect_ident("a function name")?;
            let todo = self.skip_declaration_remainder();
            return Ok(Item::Fn(FnItem {
                span: start,
                name,
                is_pub,
                is_async: false,
                doc,
                attrs,
                todo,
            }));
        }
        if self.at_keyword("resource") && self.peek_is_keyword_at(1, "struct") {
            self.bump(); // resource
            self.bump(); // struct
            let (_, name) = self.expect_ident("a struct name")?;
            let todo = self.skip_declaration_remainder();
            return Ok(Item::Struct(StructItem {
                span: start,
                name,
                is_pub,
                is_resource: true,
                doc,
                attrs,
                todo,
            }));
        }
        if self.at_keyword("struct") {
            self.bump();
            let (_, name) = self.expect_ident("a struct name")?;
            let todo = self.skip_declaration_remainder();
            return Ok(Item::Struct(StructItem {
                span: start,
                name,
                is_pub,
                is_resource: false,
                doc,
                attrs,
                todo,
            }));
        }
        if self.at_keyword("enum") {
            self.bump();
            let (_, name) = self.expect_ident("an enum name")?;
            let todo = self.skip_declaration_remainder();
            return Ok(Item::Enum(EnumItem {
                span: start,
                name,
                is_pub,
                doc,
                attrs,
                todo,
            }));
        }
        if self.at_keyword("const") {
            self.bump();
            let (_, name) = self.expect_ident("a const name")?;
            let todo = self.skip_declaration_remainder();
            return Ok(Item::Const(ConstItem {
                span: start,
                name,
                is_pub,
                doc,
                attrs,
                todo,
            }));
        }
        if self.at_keyword("pool") {
            if is_pub {
                return Err(self.error_here("`pub` is not valid before `pool`"));
            }
            self.bump();
            let (_, name) = self.expect_ident("a pool name")?;
            let todo = self.skip_declaration_remainder();
            return Ok(Item::Pool(PoolItem {
                span: start,
                name,
                doc,
                attrs,
                todo,
            }));
        }
        if self.at_keyword("comptime") && self.peek_is_keyword_at(1, "if") {
            if is_pub {
                return Err(self.error_here("`pub` is not valid before `comptime if`"));
            }
            self.bump(); // comptime
            self.bump(); // if
            let todo = self.skip_declaration_remainder();
            return Ok(Item::ComptimeIf(ComptimeIfItem {
                span: start,
                doc,
                attrs,
                todo,
            }));
        }

        Err(self.error_here(format!(
            "expected a top-level declaration, found `{}`",
            self.peek_display()
        )))
    }

    /// Skips from the current token to the end of the declaration whose
    /// header was just parsed: either the rest of a one-line declaration (up
    /// to and including its `Newline`) or a full indented suite (tracking
    /// `Indent`/`Dedent` depth so a nested suite is skipped correctly too).
    /// Returns the span of the first skipped token, or `None` if nothing
    /// followed the header but its terminating newline (e.g. a bare `pool
    /// Name`) — the caller uses this to decide whether a `Todo` node is
    /// honest (there was something left unparsed) or would just be noise.
    fn skip_declaration_remainder(&mut self) -> Option<Span> {
        let mut first: Option<Span> = None;
        let mut depth: i32 = 0;
        loop {
            match self.peek_kind() {
                TokenKind::Eof => break,
                TokenKind::Indent => {
                    if first.is_none() {
                        first = Some(self.peek_span());
                    }
                    depth += 1;
                    self.bump();
                }
                TokenKind::Dedent => {
                    depth -= 1;
                    self.bump();
                    if depth <= 0 {
                        if self.at_kind(TokenKind::Newline) {
                            self.bump();
                        }
                        break;
                    }
                }
                TokenKind::Newline => {
                    self.bump();
                    // A one-line declaration ends at its newline — unless a
                    // suite opens right after (the common case: a `:`
                    // header), in which case the `Indent` branch above
                    // picks up the depth tracking on the next iteration.
                    if depth == 0 && !self.at_kind(TokenKind::Indent) {
                        break;
                    }
                }
                _ => {
                    if first.is_none() {
                        first = Some(self.peek_span());
                    }
                    self.bump();
                }
            }
        }
        first
    }
}

/// `## text` conventionally has one space after the markers; the lexer
/// keeps it raw (it is not the lexer's job to decode), so the parser strips
/// exactly one leading space, if present, when building a `Doc` node.
fn strip_doc_leading_space(raw: &str) -> String {
    raw.strip_prefix(' ').unwrap_or(raw).to_string()
}

// --- dump ------------------------------------------------------------------
//
// Stable text dump (plans/M1.md decision 5): one node per line, two-space
// child indent, `Kind @line:col key=value`, string payloads quoted. Source
// order throughout, so the dump is deterministic by construction. A skipped
// declaration body dumps as a `Todo @line:col` child so the dump stays
// honest about what item C did not parse.

pub fn dump(module: &Module) -> String {
    let mut out = String::new();
    dump_module(module, 0, &mut out);
    out
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

fn dump_doc(doc: &Option<Doc>, depth: usize, out: &mut String) {
    if let Some(doc) = doc {
        push_line(
            out,
            depth,
            &format!(
                "Doc @{}:{} text={}",
                doc.span.line,
                doc.span.col,
                quote(&doc.text)
            ),
        );
    }
}

fn dump_attrs(attrs: &[Attr], depth: usize, out: &mut String) {
    for attr in attrs {
        push_line(
            out,
            depth,
            &format!(
                "Attr @{}:{} text={}",
                attr.span.line,
                attr.span.col,
                quote(&attr.text)
            ),
        );
    }
}

fn dump_todo(todo: &Option<Span>, depth: usize, out: &mut String) {
    if let Some(span) = todo {
        push_line(out, depth, &format!("Todo @{}:{}", span.line, span.col));
    }
}

fn dump_module(m: &Module, depth: usize, out: &mut String) {
    push_line(
        out,
        depth,
        &format!(
            "Module @{}:{} path={}",
            m.span.line,
            m.span.col,
            m.path.join(".")
        ),
    );
    dump_doc(&m.doc, depth + 1, out);
    for import in &m.imports {
        dump_import(import, depth + 1, out);
    }
    for item in &m.items {
        dump_item(item, depth + 1, out);
    }
}

fn dump_import(import: &Import, depth: usize, out: &mut String) {
    let mut header = format!(
        "Import @{}:{} from={}",
        import.span.line,
        import.span.col,
        import.path.join(".")
    );
    if import.is_pub {
        header.push_str(" pub=true");
    }
    push_line(out, depth, &header);
    for name in &import.names {
        let mut line = format!(
            "ImportName @{}:{} name={}",
            name.span.line, name.span.col, name.name
        );
        if let Some(alias) = &name.alias {
            line.push_str(&format!(" alias={alias}"));
        }
        push_line(out, depth + 1, &line);
    }
}

fn dump_item(item: &Item, depth: usize, out: &mut String) {
    match item {
        Item::Const(c) => {
            let mut header = format!("Const @{}:{} name={}", c.span.line, c.span.col, c.name);
            if c.is_pub {
                header.push_str(" pub=true");
            }
            push_line(out, depth, &header);
            dump_doc(&c.doc, depth + 1, out);
            dump_attrs(&c.attrs, depth + 1, out);
            dump_todo(&c.todo, depth + 1, out);
        }
        Item::Fn(f) => {
            let mut header = format!("Fn @{}:{} name={}", f.span.line, f.span.col, f.name);
            if f.is_pub {
                header.push_str(" pub=true");
            }
            if f.is_async {
                header.push_str(" async=true");
            }
            push_line(out, depth, &header);
            dump_doc(&f.doc, depth + 1, out);
            dump_attrs(&f.attrs, depth + 1, out);
            dump_todo(&f.todo, depth + 1, out);
        }
        Item::Struct(s) => {
            let mut header = format!("Struct @{}:{} name={}", s.span.line, s.span.col, s.name);
            if s.is_pub {
                header.push_str(" pub=true");
            }
            if s.is_resource {
                header.push_str(" resource=true");
            }
            push_line(out, depth, &header);
            dump_doc(&s.doc, depth + 1, out);
            dump_attrs(&s.attrs, depth + 1, out);
            dump_todo(&s.todo, depth + 1, out);
        }
        Item::Enum(e) => {
            let mut header = format!("Enum @{}:{} name={}", e.span.line, e.span.col, e.name);
            if e.is_pub {
                header.push_str(" pub=true");
            }
            push_line(out, depth, &header);
            dump_doc(&e.doc, depth + 1, out);
            dump_attrs(&e.attrs, depth + 1, out);
            dump_todo(&e.todo, depth + 1, out);
        }
        Item::Pool(p) => {
            let header = format!("Pool @{}:{} name={}", p.span.line, p.span.col, p.name);
            push_line(out, depth, &header);
            dump_doc(&p.doc, depth + 1, out);
            dump_attrs(&p.attrs, depth + 1, out);
            dump_todo(&p.todo, depth + 1, out);
        }
        Item::ComptimeIf(c) => {
            let header = format!("ComptimeIf @{}:{}", c.span.line, c.span.col);
            push_line(out, depth, &header);
            dump_doc(&c.doc, depth + 1, out);
            dump_attrs(&c.attrs, depth + 1, out);
            dump_todo(&c.todo, depth + 1, out);
        }
    }
}
