//! Tokenizer for wrela source (02-language.md §1).
//!
//! Implements: ASCII identifiers and keywords; `#` comments and `##` doc
//! comments; four-space significant indentation with tabs rejected; newline
//! suppression inside `()[]{}`; integer and float literals (decimal/hex/
//! octal/binary with underscores; floats require digits on both sides of
//! `.` or a required exponent); text, f-string, and byte-string literals
//! with escapes validated at lex time (contents kept raw — token text is
//! never decoded); f-string brace-balance scanning (interior expressions are
//! not lexed); and the operator set of 02 §8.2. Anything unsupported is a
//! lex error, never a guess.

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TokenKind {
    Ident,
    Keyword,
    Int,
    Float,
    Str,
    FStr,
    BStr,
    Char,
    DocComment,
    Op,
    Newline,
    Indent,
    Dedent,
    Eof,
}

#[derive(Debug, Clone)]
pub struct Token {
    pub kind: TokenKind,
    pub text: String,
    pub line: u32,
    pub col: u32,
}

#[derive(Debug)]
pub struct LexError {
    pub message: String,
    pub line: u32,
    pub col: u32,
}

/// Which escape set applies to the literal currently being scanned
/// (02-language.md §1.1): `\xNN` is byte-string-only, `\u{...}` is
/// text/char-only. F-strings share the text literal's escapes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum EscapeContext {
    Text,
    Byte,
}

/// Keywords of the trimmed language. `02-language.md` is authoritative.
pub const KEYWORDS: &[&str] = &[
    "and", "assert", "async", "await", "break", "case", "comptime", "const", "continue", "defer",
    "deriving", "elif", "else", "enum", "false", "fn", "for", "from", "if", "import", "in", "init",
    "is", "match", "module", "mut", "not", "or", "own", "pass", "pool", "pub", "read", "resource",
    "return", "self", "send", "struct", "take", "true", "unit", "while", "with",
];

/// Multi-character operators, longest first so maximal munch is by order.
const MULTI_OPS: &[&str] = &[
    "<<=", ">>=", "..=", "+%", "-%", "*%", "->", "..", "<<", ">>", "<=", ">=", "==", "!=", "+=",
    "-=", "*=", "/=", "%=", "&=", "|=", "^=",
];

// `;` appears only inside the fixed-array type `[T; N]` (02 §6.2).
const SINGLE_OPS: &[char] = &[
    '+', '-', '*', '/', '%', '&', '|', '^', '~', '<', '>', '=', '(', ')', '[', ']', '{', '}', ',',
    ':', '.', '?', '@', ';',
];

pub fn lex(source: &str) -> Result<Vec<Token>, LexError> {
    Lexer::new(source).run()
}

struct Lexer<'s> {
    src: &'s [u8],
    pos: usize,
    line: u32,
    col: u32,
    depth: usize, // () [] {} nesting: newlines are suppressed inside
    indents: Vec<u32>,
    tokens: Vec<Token>,
}

impl<'s> Lexer<'s> {
    fn new(source: &'s str) -> Self {
        Lexer {
            src: source.as_bytes(),
            pos: 0,
            line: 1,
            col: 1,
            depth: 0,
            indents: vec![0],
            tokens: Vec::new(),
        }
    }

    fn error(&self, message: impl Into<String>) -> LexError {
        LexError {
            message: message.into(),
            line: self.line,
            col: self.col,
        }
    }

    fn peek(&self) -> Option<u8> {
        self.src.get(self.pos).copied()
    }

    fn peek2(&self) -> Option<u8> {
        self.src.get(self.pos + 1).copied()
    }

    fn bump(&mut self) -> Option<u8> {
        let b = self.peek()?;
        self.pos += 1;
        if b == b'\n' {
            self.line += 1;
            self.col = 1;
        } else {
            self.col += 1;
        }
        Some(b)
    }

    fn push(&mut self, kind: TokenKind, text: impl Into<String>, line: u32, col: u32) {
        self.tokens.push(Token {
            kind,
            text: text.into(),
            line,
            col,
        });
    }

    fn run(mut self) -> Result<Vec<Token>, LexError> {
        let mut at_line_start = true;
        loop {
            if at_line_start && self.depth == 0 {
                self.handle_indentation()?;
                at_line_start = false;
                continue;
            }
            match self.peek() {
                None => break,
                Some(b'\n') => {
                    self.bump();
                    if self.depth == 0 {
                        if self.last_is_content() {
                            let (l, c) = (self.line - 1, 1);
                            self.push(TokenKind::Newline, "", l, c);
                        }
                        at_line_start = true;
                    }
                }
                Some(b'#') => {
                    while let Some(b) = self.peek() {
                        if b == b'\n' {
                            break;
                        }
                        self.bump();
                    }
                }
                Some(b' ') => {
                    self.bump();
                }
                Some(b'\t') => {
                    return Err(self.error("tab character in source"));
                }
                Some(b'\r') => {
                    return Err(self.error("carriage return in source"));
                }
                Some(b'"') => self.lex_string(TokenKind::Str, "")?,
                Some(b'\'') => self.lex_char()?,
                Some(c) if c.is_ascii_digit() => self.lex_number()?,
                Some(c) if c.is_ascii_alphabetic() || c == b'_' => {
                    // f"..." and b"..." literal prefixes.
                    if (c == b'f' || c == b'b') && self.peek2() == Some(b'"') {
                        let kind = if c == b'f' {
                            TokenKind::FStr
                        } else {
                            TokenKind::BStr
                        };
                        let prefix = (c as char).to_string();
                        self.bump();
                        self.lex_string(kind, &prefix)?;
                    } else {
                        self.lex_word();
                    }
                }
                // A non-ASCII byte can start a valid string/char literal
                // (checked above) or appear inside one/inside a comment
                // (handled by those scanners without reaching here); at any
                // other token-start position it is a named diagnostic, not
                // the generic "unexpected character" (02-language.md §1:
                // identifiers and source structure are ASCII in rev 0.1).
                Some(c) if c >= 0x80 => {
                    return Err(self.error(
                        "non-ASCII byte in source; identifiers and source structure are ASCII in revision 0.1",
                    ));
                }
                Some(_) => self.lex_operator()?,
            }
        }
        // End of file acts as a newline, then closes open suites (02 §1).
        if self.depth != 0 {
            return Err(self.error("unclosed delimiter at end of file"));
        }
        if self.last_is_content() {
            let (l, c) = (self.line, self.col);
            self.push(TokenKind::Newline, "", l, c);
        }
        while self.indents.len() > 1 {
            self.indents.pop();
            let (l, c) = (self.line, self.col);
            self.push(TokenKind::Dedent, "", l, c);
            self.push(TokenKind::Newline, "", l, c);
        }
        let (l, c) = (self.line, self.col);
        self.push(TokenKind::Eof, "", l, c);
        Ok(self.tokens)
    }

    fn last_is_content(&self) -> bool {
        // DocComment is layout-transparent: a doc-comment-only line behaves
        // exactly like a plain comment-only line for indentation/newline
        // purposes (it never shares a logical line with real tokens — see
        // `handle_indentation`, the only place that emits DocComment).
        !matches!(
            self.tokens.last().map(|t| &t.kind),
            None | Some(TokenKind::Newline)
                | Some(TokenKind::Indent)
                | Some(TokenKind::Dedent)
                | Some(TokenKind::DocComment)
        )
    }

    /// At the start of a physical line (outside delimiters): measure leading
    /// spaces, skip blank/comment-only lines, and emit INDENT/DEDENT per the
    /// exactly-four-spaces rule.
    fn handle_indentation(&mut self) -> Result<(), LexError> {
        let mut width: u32 = 0;
        loop {
            match self.peek() {
                Some(b' ') => {
                    width += 1;
                    self.bump();
                }
                Some(b'\t') => return Err(self.error("tab in leading indentation")),
                _ => break,
            }
        }
        match self.peek() {
            // Blank and comment-only lines emit no layout tokens.
            None => return Ok(()),
            Some(b'\n') => {
                self.bump();
                return self.handle_indentation();
            }
            // `##` at line start is a doc comment: it emits a DocComment
            // token carrying the raw text after `##` (trailing newline
            // excluded) but otherwise behaves like a comment-only line — no
            // INDENT/DEDENT/NEWLINE beyond that (attachment to a
            // declaration is the parser's job, not the lexer's). Plain `#`
            // stays silently skipped.
            Some(b'#') => {
                let (line, col) = (self.line, self.col);
                self.bump();
                if self.peek() == Some(b'#') {
                    self.bump();
                    let start = self.pos;
                    while let Some(b) = self.peek() {
                        if b == b'\n' {
                            break;
                        }
                        self.bump();
                    }
                    let text = std::str::from_utf8(&self.src[start..self.pos])
                        .map_err(|_| self.error("doc comment is not valid UTF-8"))?
                        .to_string();
                    self.push(TokenKind::DocComment, text, line, col);
                } else {
                    while let Some(b) = self.peek() {
                        if b == b'\n' {
                            break;
                        }
                        self.bump();
                    }
                }
                return Ok(());
            }
            _ => {}
        }
        let current = *self.indents.last().expect("indent stack is never empty");
        if width == current {
            return Ok(());
        }
        if width > current {
            if width != current + 4 {
                return Err(self.error(format!(
                    "indentation must deepen by exactly four spaces (found {width}, parent {current})"
                )));
            }
            self.indents.push(width);
            let (l, c) = (self.line, 1);
            self.push(TokenKind::Indent, "", l, c);
            return Ok(());
        }
        while width < *self.indents.last().expect("indent stack is never empty") {
            self.indents.pop();
            let (l, c) = (self.line, 1);
            self.push(TokenKind::Dedent, "", l, c);
            self.push(TokenKind::Newline, "", l, c);
        }
        if width != *self.indents.last().expect("indent stack is never empty") {
            return Err(self.error("dedent does not match any enclosing indentation level"));
        }
        Ok(())
    }

    fn lex_word(&mut self) {
        let (line, col) = (self.line, self.col);
        let start = self.pos;
        while let Some(b) = self.peek() {
            if b.is_ascii_alphanumeric() || b == b'_' {
                self.bump();
            } else {
                break;
            }
        }
        let text = std::str::from_utf8(&self.src[start..self.pos])
            .expect("ASCII word bytes are valid UTF-8")
            .to_string();
        let kind = if KEYWORDS.contains(&text.as_str()) {
            TokenKind::Keyword
        } else {
            TokenKind::Ident
        };
        self.push(kind, text, line, col);
    }

    /// Consumes a run of bytes from `allowed` (a digit set plus `_`),
    /// enforcing that `_` never leads or trails the run — i.e. it never
    /// sits anywhere but between two digits already accepted by the
    /// surrounding calls. Assumes the caller has confirmed at least one
    /// digit follows before calling (radix-prefix bodies are the one
    /// exception, and keep their historical, more permissive behavior).
    fn lex_digit_run(&mut self, allowed: &[u8]) -> Result<(), LexError> {
        let mut prev = 0u8;
        while let Some(b) = self.peek() {
            if !allowed.contains(&b) {
                break;
            }
            prev = b;
            self.bump();
        }
        if prev == b'_' {
            return Err(self.error("underscore must sit between two digits"));
        }
        Ok(())
    }

    /// Integer and float literals (02-language.md §1.1). A float is either
    /// `digits.digits` with an optional `e`/`E` exponent, or bare digits
    /// with a required exponent. Digits are required on both sides of `.`;
    /// a `.` lacking a following digit is left for the operator lexer
    /// (member access) or the `..`/`..=` range operators, so `1..4` and
    /// `256.KiB` keep lexing exactly as before. Once an exponent marker is
    /// seen, it is committed-to: a malformed exponent is a lex error, never
    /// a fallback to plain digits.
    fn lex_number(&mut self) -> Result<(), LexError> {
        let (line, col) = (self.line, self.col);
        let start = self.pos;
        const DECIMAL: &[u8] = b"0123456789_";
        if self.peek() == Some(b'0') && matches!(self.peek2(), Some(b'x') | Some(b'o') | Some(b'b'))
        {
            self.bump();
            self.bump();
            self.lex_digit_run(b"0123456789abcdefABCDEF_")?;
            let text = std::str::from_utf8(&self.src[start..self.pos])
                .expect("ASCII number bytes are valid UTF-8")
                .to_string();
            self.push(TokenKind::Int, text, line, col);
            return Ok(());
        }
        self.lex_digit_run(DECIMAL)?;
        let mut is_float = false;
        // `.` followed by a digit is a fractional part; `..`/`..=` (range)
        // and `.` followed by anything else (member access, `256.KiB`) are
        // left untouched for the operator/word lexers.
        if self.peek() == Some(b'.') && self.peek2().is_some_and(|d| d.is_ascii_digit()) {
            is_float = true;
            self.bump();
            self.lex_digit_run(DECIMAL)?;
        }
        if matches!(self.peek(), Some(b'e') | Some(b'E')) {
            is_float = true;
            self.bump();
            if matches!(self.peek(), Some(b'+') | Some(b'-')) {
                self.bump();
            }
            if !self.peek().is_some_and(|d| d.is_ascii_digit()) {
                return Err(self.error("float exponent requires at least one digit"));
            }
            self.lex_digit_run(DECIMAL)?;
        }
        let text = std::str::from_utf8(&self.src[start..self.pos])
            .expect("ASCII number bytes are valid UTF-8")
            .to_string();
        let kind = if is_float {
            TokenKind::Float
        } else {
            TokenKind::Int
        };
        self.push(kind, text, line, col);
        Ok(())
    }

    /// Lexes a quoted literal starting at `"`. Contents are kept raw (the
    /// token text is the exact source spelling, never decoded); escapes are
    /// validated per 02-language.md §1.1 as they are scanned, and `"""` is
    /// rejected by name (reserved, not "unterminated"). `prefix` is the
    /// already-consumed `f`/`b` marker, included in the token text.
    fn lex_string(&mut self, kind: TokenKind, prefix: &str) -> Result<(), LexError> {
        let (line, col) = (self.line, self.col.saturating_sub(prefix.len() as u32));
        let start = self.pos;
        self.bump(); // opening quote
        if self.peek() == Some(b'"') && self.peek2() == Some(b'"') {
            return Err(self.error("triple-quoted string literals are reserved"));
        }
        let ctx = if matches!(kind, TokenKind::BStr) {
            EscapeContext::Byte
        } else {
            EscapeContext::Text
        };
        // f-string brace scanning (02-language.md §1.1): `{{`/`}}` are
        // literal braces; a lone `{` opens an interpolation extent that
        // must balance before the closing quote. The interior expression
        // is never lexed — only the brace count is tracked — so the
        // closing `"` only ends the token while no interpolation is open.
        let mut brace_depth: u32 = 0;
        loop {
            match self.peek() {
                None => return Err(self.error("unterminated string literal")),
                Some(b'\n') => return Err(self.error("string literal contains a raw newline")),
                Some(b'\\') if brace_depth == 0 => self.lex_escape(ctx)?,
                Some(b'"') if brace_depth == 0 => {
                    self.bump();
                    break;
                }
                Some(b'{') if matches!(kind, TokenKind::FStr) => {
                    if brace_depth == 0 && self.peek2() == Some(b'{') {
                        self.bump();
                        self.bump();
                    } else {
                        brace_depth += 1;
                        self.bump();
                    }
                }
                Some(b'}') if matches!(kind, TokenKind::FStr) => {
                    if brace_depth == 0 {
                        if self.peek2() == Some(b'}') {
                            self.bump();
                            self.bump();
                        } else {
                            return Err(self.error("unmatched `}` in f-string"));
                        }
                    } else {
                        brace_depth -= 1;
                        self.bump();
                    }
                }
                Some(_) => {
                    self.bump();
                }
            }
        }
        let body = std::str::from_utf8(&self.src[start..self.pos])
            .map_err(|_| self.error("string literal is not valid UTF-8"))?;
        self.push(kind, format!("{prefix}{body}"), line, col);
        Ok(())
    }

    fn lex_char(&mut self) -> Result<(), LexError> {
        let (line, col) = (self.line, self.col);
        let start = self.pos;
        self.bump(); // opening quote
        loop {
            match self.peek() {
                None => return Err(self.error("unterminated character literal")),
                Some(b'\n') => return Err(self.error("character literal contains a raw newline")),
                Some(b'\\') => self.lex_escape(EscapeContext::Text)?,
                Some(b'\'') => {
                    self.bump();
                    break;
                }
                Some(_) => {
                    self.bump();
                }
            }
        }
        let body = std::str::from_utf8(&self.src[start..self.pos])
            .map_err(|_| self.error("character literal is not valid UTF-8"))?;
        self.push(TokenKind::Char, body.to_string(), line, col);
        Ok(())
    }

    /// Validates one escape sequence starting at the `\` (which this
    /// consumes along with the rest of the sequence). Token text is never
    /// decoded — this only checks shape and consumes the right number of
    /// raw bytes. Per 02-language.md §1.1: `\\ \" \' \n \r \t \0` are legal
    /// everywhere; `\xNN` (exactly two hex digits) only in byte strings;
    /// `\u{H...}` (one to six hex digits) only in text and char literals.
    fn lex_escape(&mut self, ctx: EscapeContext) -> Result<(), LexError> {
        self.bump(); // '\\'
        match self.peek() {
            None => Err(self.error("unterminated escape sequence")),
            Some(b'\\' | b'"' | b'\'' | b'n' | b'r' | b't' | b'0') => {
                self.bump();
                Ok(())
            }
            Some(b'x') => {
                if !matches!(ctx, EscapeContext::Byte) {
                    return Err(self.error("`\\x` escapes are only valid in byte string literals"));
                }
                self.bump();
                for _ in 0..2 {
                    match self.peek() {
                        Some(b) if b.is_ascii_hexdigit() => {
                            self.bump();
                        }
                        _ => return Err(self.error("`\\x` escape requires exactly two hex digits")),
                    }
                }
                Ok(())
            }
            Some(b'u') => {
                if matches!(ctx, EscapeContext::Byte) {
                    return Err(self.error("`\\u` escapes are not valid in byte string literals"));
                }
                self.bump();
                if self.peek() != Some(b'{') {
                    return Err(self.error("`\\u` escape requires `{` after `u`"));
                }
                self.bump();
                let mut digits = 0u32;
                loop {
                    match self.peek() {
                        Some(b'}') => break,
                        Some(b) if b.is_ascii_hexdigit() => {
                            digits += 1;
                            if digits > 6 {
                                return Err(
                                    self.error("`\\u{...}` escape allows at most six hex digits")
                                );
                            }
                            self.bump();
                        }
                        None | Some(b'\n') => {
                            return Err(self.error("unterminated `\\u{...}` escape"));
                        }
                        Some(_) => {
                            return Err(
                                self.error("`\\u{...}` escape must contain only hex digits")
                            );
                        }
                    }
                }
                if digits == 0 {
                    return Err(self.error("`\\u{...}` escape requires at least one hex digit"));
                }
                self.bump(); // closing `}`
                Ok(())
            }
            Some(c) => Err(self.error(format!("unknown escape sequence `\\{}`", c as char))),
        }
    }

    fn lex_operator(&mut self) -> Result<(), LexError> {
        let (line, col) = (self.line, self.col);
        let rest = &self.src[self.pos..];
        for op in MULTI_OPS {
            if rest.starts_with(op.as_bytes()) {
                for _ in 0..op.len() {
                    self.bump();
                }
                self.push(TokenKind::Op, *op, line, col);
                return Ok(());
            }
        }
        let c = self.peek().expect("caller checked non-empty") as char;
        if SINGLE_OPS.contains(&c) {
            if matches!(c, '(' | '[' | '{') {
                self.depth += 1;
            }
            if matches!(c, ')' | ']' | '}') {
                if self.depth == 0 {
                    return Err(self.error(format!("unmatched closing `{c}`")));
                }
                self.depth -= 1;
            }
            self.bump();
            self.push(TokenKind::Op, c.to_string(), line, col);
            return Ok(());
        }
        Err(self.error(format!("unexpected character `{c}`")))
    }
}

/// Stable text dump, one token per line: `line:col KIND text`. This format
/// is pinned by the golden suite; changing it is a golden-reviewed change.
pub fn dump(tokens: &[Token]) -> String {
    let mut out = String::new();
    for t in tokens {
        let kind = format!("{:?}", t.kind).to_uppercase();
        if t.text.is_empty() {
            out.push_str(&format!("{}:{} {}\n", t.line, t.col, kind));
        } else {
            out.push_str(&format!("{}:{} {} {}\n", t.line, t.col, kind, t.text));
        }
    }
    out
}
