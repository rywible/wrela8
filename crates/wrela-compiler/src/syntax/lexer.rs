//! Tokenizer for wrela source (02-language.md §1).
//!
//! Implements: ASCII identifiers and keywords; `#` comments; four-space
//! significant indentation with tabs rejected; newline suppression inside
//! `()[]{}`; integer literals (decimal/hex/octal/binary with underscores);
//! text, f-string, and byte-string literals (contents kept raw — escape and
//! interpolation validation happen in the parser); and the operator set of
//! 02 §8.2. Anything unsupported is a lex error, never a guess.

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TokenKind {
    Ident,
    Keyword,
    Int,
    Str,
    FStr,
    BStr,
    Char,
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

/// Keywords of the trimmed language. `02-language.md` is authoritative.
pub const KEYWORDS: &[&str] = &[
    "and", "assert", "async", "await", "break", "case", "comptime", "const", "continue", "defer",
    "deriving", "elif", "else", "enum", "false", "fn", "for", "from", "if", "import", "in", "init",
    "is", "match", "module", "mut", "not", "or", "pass", "pool", "pub", "read", "resource",
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
        !matches!(
            self.tokens.last().map(|t| &t.kind),
            None | Some(TokenKind::Newline) | Some(TokenKind::Indent) | Some(TokenKind::Dedent)
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
            Some(b'#') => {
                while let Some(b) = self.peek() {
                    if b == b'\n' {
                        break;
                    }
                    self.bump();
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

    fn lex_number(&mut self) -> Result<(), LexError> {
        let (line, col) = (self.line, self.col);
        let start = self.pos;
        let radix_letters: &[u8] = if self.peek() == Some(b'0')
            && matches!(self.peek2(), Some(b'x') | Some(b'o') | Some(b'b'))
        {
            self.bump();
            self.bump();
            b"0123456789abcdefABCDEF_"
        } else {
            b"0123456789_."
        };
        let mut prev = 0u8;
        while let Some(b) = self.peek() {
            if !radix_letters.contains(&b) {
                break;
            }
            // `..` is a range, not part of a number.
            if b == b'.' && self.peek2() == Some(b'.') {
                break;
            }
            // A `.` not followed by a digit is member access.
            if b == b'.' && !self.peek2().is_some_and(|d| d.is_ascii_digit()) {
                break;
            }
            prev = b;
            self.bump();
        }
        if prev == b'_' {
            return Err(self.error("underscore must sit between two digits"));
        }
        let text = std::str::from_utf8(&self.src[start..self.pos])
            .expect("ASCII number bytes are valid UTF-8")
            .to_string();
        self.push(TokenKind::Int, text, line, col);
        Ok(())
    }

    /// Lexes a quoted literal starting at `"`. Contents are kept raw;
    /// escape/interpolation validation is the parser's job. `prefix` is the
    /// already-consumed `f`/`b` marker, included in the token text.
    fn lex_string(&mut self, kind: TokenKind, prefix: &str) -> Result<(), LexError> {
        let (line, col) = (self.line, self.col.saturating_sub(prefix.len() as u32));
        let start = self.pos;
        self.bump(); // opening quote
        loop {
            match self.peek() {
                None => return Err(self.error("unterminated string literal")),
                Some(b'\n') => return Err(self.error("string literal contains a raw newline")),
                Some(b'\\') => {
                    self.bump();
                    self.bump();
                }
                Some(b'"') => {
                    self.bump();
                    break;
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
                Some(b'\\') => {
                    self.bump();
                    self.bump();
                }
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
