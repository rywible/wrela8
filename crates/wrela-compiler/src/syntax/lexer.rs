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
    pub byte_start: usize,
    pub byte_end: usize,
}

#[derive(Debug)]
pub struct LexError {
    pub message: String,
    pub line: u32,
    pub col: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum EscapeContext {
    Text,
    Byte,
}

pub const KEYWORDS: &[&str] = &[
    "and", "assert", "async", "await", "break", "case", "comptime", "const", "continue", "defer",
    "deriving", "elif", "else", "enum", "false", "fn", "for", "from", "if", "import", "in", "init",
    "is", "match", "module", "mut", "not", "or", "own", "pass", "pool", "pub", "read", "resource",
    "return", "self", "send", "static", "struct", "take", "true", "unit", "while", "with",
];

const MULTI_OPS: &[&str] = &[
    "<<=", ">>=", "..=", "+%", "-%", "*%", "->", "..", "<<", ">>", "<=", ">=", "==", "!=", "+=",
    "-=", "*=", "/=", "%=", "&=", "|=", "^=",
];

const SINGLE_OPS: &[char] = &[
    '+', '-', '*', '/', '%', '&', '|', '^', '~', '<', '>', '=', '(', ')', '[', ']', '{', '}', ',',
    ':', '.', '?', '@', ';',
];

pub fn lex(source: &str) -> Result<Vec<Token>, LexError> {
    Lexer::new(source).run()
}

struct Island {
    open_depth: usize,
    indents: Vec<u32>,
}

struct Lexer<'s> {
    src: &'s [u8],
    pos: usize,
    line: u32,
    col: u32,
    line_start: usize,
    depth: usize,
    indents: Vec<u32>,
    islands: Vec<Island>,
    tokens: Vec<Token>,
}

impl<'s> Lexer<'s> {
    fn new(source: &'s str) -> Self {
        Lexer {
            src: source.as_bytes(),
            pos: 0,
            line: 1,
            col: 1,
            line_start: 0,
            depth: 0,
            indents: vec![0],
            islands: Vec::new(),
            tokens: Vec::new(),
        }
    }

    fn layout_active(&self) -> bool {
        match self.islands.last() {
            Some(island) => self.depth == island.open_depth,
            None => self.depth == 0,
        }
    }

    fn innermost_indent(&self) -> u32 {
        match self.islands.last() {
            Some(island) => *island
                .indents
                .last()
                .expect("island indent stack is never empty"),
            None => *self.indents.last().expect("indent stack is never empty"),
        }
    }

    fn last_token_is_colon(&self) -> bool {
        matches!(self.tokens.last(), Some(t) if t.kind == TokenKind::Op && t.text == ":")
    }

    fn open_layout_island(&mut self) {
        let base = self.innermost_indent();
        self.islands.push(Island {
            open_depth: self.depth,
            indents: vec![base],
        });
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
            self.line_start = self.pos;
        } else {
            self.col += 1;
        }
        Some(b)
    }

    fn push(&mut self, kind: TokenKind, text: impl Into<String>, line: u32, col: u32) {
        let byte_start = self
            .line_start
            .saturating_add(col.saturating_sub(1) as usize);
        self.push_at(kind, text, line, col, byte_start);
    }

    fn push_at(
        &mut self,
        kind: TokenKind,
        text: impl Into<String>,
        line: u32,
        col: u32,
        byte_start: usize,
    ) {
        self.tokens.push(Token {
            kind,
            text: text.into(),
            line,
            col,
            byte_start,
            byte_end: self.pos,
        });
    }

    fn run(mut self) -> Result<Vec<Token>, LexError> {
        let mut at_line_start = true;
        loop {
            if at_line_start && self.layout_active() {
                self.handle_indentation()?;
                at_line_start = false;
                continue;
            }
            match self.peek() {
                None => break,
                Some(b'\n') => {
                    if !self.layout_active() && self.last_token_is_colon() {
                        self.open_layout_island();
                    }
                    self.bump();
                    if self.layout_active() {
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
                Some(c) if c >= 0x80 => {
                    return Err(self.error(
                        "non-ASCII byte in source; identifiers and source structure are ASCII in revision 0.1",
                    ));
                }
                Some(_) => self.lex_operator()?,
            }
        }
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
            None | Some(TokenKind::Newline)
                | Some(TokenKind::Indent)
                | Some(TokenKind::Dedent)
                | Some(TokenKind::DocComment)
        )
    }

    fn handle_indentation(&mut self) -> Result<(), LexError> {
        loop {
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
                None => return Ok(()),
                Some(b'\n') => {
                    self.bump();
                    continue;
                }
                Some(b'#') => {
                    let (line, col) = (self.line, self.col);
                    let token_start = self.pos;
                    if self.peek2() == Some(b'#') {
                        self.dispatch_indent(width)?;
                        self.bump();
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
                        self.push_at(TokenKind::DocComment, text, line, col, token_start);
                    } else {
                        self.bump();
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
            return self.dispatch_indent(width);
        }
    }

    fn dispatch_indent(&mut self, width: u32) -> Result<(), LexError> {
        let in_island = matches!(self.islands.last(), Some(i) if i.open_depth == self.depth);
        if in_island {
            self.apply_island_indent(width)
        } else {
            self.apply_top_level_indent(width)
        }
    }

    fn apply_top_level_indent(&mut self, width: u32) -> Result<(), LexError> {
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

    fn apply_island_indent(&mut self, width: u32) -> Result<(), LexError> {
        let current = *self
            .islands
            .last()
            .expect("checked by caller")
            .indents
            .last()
            .expect("island indent stack is never empty");
        if width == current {
            return Ok(());
        }
        if width > current {
            if width != current + 4 {
                return Err(self.error(format!(
                    "indentation must deepen by exactly four spaces (found {width}, parent {current})"
                )));
            }
            self.islands
                .last_mut()
                .expect("checked by caller")
                .indents
                .push(width);
            let (l, c) = (self.line, 1);
            self.push(TokenKind::Indent, "", l, c);
            return Ok(());
        }
        loop {
            let island = self.islands.last_mut().expect("checked by caller");
            if island.indents.len() > 1 && width < *island.indents.last().expect("non-empty") {
                island.indents.pop();
                let (l, c) = (self.line, 1);
                self.push(TokenKind::Dedent, "", l, c);
                self.push(TokenKind::Newline, "", l, c);
                continue;
            }
            break;
        }
        let island = self.islands.last().expect("checked by caller");
        if island.indents.len() == 1 && width <= island.indents[0] {
            self.islands.pop();
            return Ok(());
        }
        if width != *island.indents.last().expect("non-empty") {
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
        self.push_at(kind, text, line, col, start);
    }

    fn lex_digit_run(&mut self, allowed: &[u8]) -> Result<(), LexError> {
        let mut prev = 0u8;
        let mut saw_digit = false;
        while let Some(b) = self.peek() {
            if !allowed.contains(&b) {
                break;
            }
            if b == b'_' {
                if !saw_digit || prev == b'_' {
                    return Err(self.error("underscore must sit between two digits"));
                }
            } else {
                saw_digit = true;
            }
            prev = b;
            self.bump();
        }
        if prev == b'_' {
            return Err(self.error("underscore must sit between two digits"));
        }
        Ok(())
    }

    fn lex_number(&mut self) -> Result<(), LexError> {
        let (line, col) = (self.line, self.col);
        let start = self.pos;
        const DECIMAL: &[u8] = b"0123456789_";
        if self.peek() == Some(b'0') && matches!(self.peek2(), Some(b'x') | Some(b'o') | Some(b'b'))
        {
            let radix = self.peek2().expect("checked above");
            self.bump();
            self.bump();
            let digits: &[u8] = match radix {
                b'x' => b"0123456789abcdefABCDEF_",
                b'o' => b"01234567_",
                b'b' => b"01_",
                _ => unreachable!("checked by matches! above"),
            };
            let digits_start = self.pos;
            self.lex_digit_run(digits)?;
            if self.pos == digits_start {
                return Err(
                    self.error("integer literal needs at least one digit after the radix prefix")
                );
            }
            if self
                .peek()
                .is_some_and(|b| b.is_ascii_alphanumeric() || b == b'_')
            {
                return Err(self.error("invalid digit for this integer literal's radix"));
            }
            let text = std::str::from_utf8(&self.src[start..self.pos])
                .expect("ASCII number bytes are valid UTF-8")
                .to_string();
            self.push_at(TokenKind::Int, text, line, col, start);
            return Ok(());
        }
        self.lex_digit_run(DECIMAL)?;
        let mut is_float = false;
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
        self.push_at(kind, text, line, col, start);
        Ok(())
    }

    fn lex_string(&mut self, kind: TokenKind, prefix: &str) -> Result<(), LexError> {
        let (line, col) = (self.line, self.col.saturating_sub(prefix.len() as u32));
        let start = self.pos;
        let token_start = start.saturating_sub(prefix.len());
        self.bump();
        if self.peek() == Some(b'"') && self.peek2() == Some(b'"') {
            return Err(self.error("triple-quoted string literals are reserved"));
        }
        let ctx = if matches!(kind, TokenKind::BStr) {
            EscapeContext::Byte
        } else {
            EscapeContext::Text
        };
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
        self.push_at(kind, format!("{prefix}{body}"), line, col, token_start);
        Ok(())
    }

    fn lex_char(&mut self) -> Result<(), LexError> {
        let (line, col) = (self.line, self.col);
        let start = self.pos;
        self.bump();
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
        self.push_at(TokenKind::Char, body.to_string(), line, col, start);
        Ok(())
    }

    fn lex_escape(&mut self, ctx: EscapeContext) -> Result<(), LexError> {
        self.bump();
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
                let mut value = 0u32;
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
                            value = value * 16 + (b as char).to_digit(16).expect("hex digit");
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
                if char::from_u32(value).is_none() {
                    let what = if (0xD800..=0xDFFF).contains(&value) {
                        "a surrogate"
                    } else {
                        "above the U+10FFFF maximum"
                    };
                    return Err(self.error(format!(
                        "`\\u{{{value:X}}}` is not a Unicode scalar value ({what})"
                    )));
                }
                self.bump();
                Ok(())
            }
            Some(c) => Err(self.error(format!("unknown escape sequence `\\{}`", c as char))),
        }
    }

    fn lex_operator(&mut self) -> Result<(), LexError> {
        let (line, col) = (self.line, self.col);
        let start = self.pos;
        let rest = &self.src[self.pos..];
        for op in MULTI_OPS {
            if rest.starts_with(op.as_bytes()) {
                for _ in 0..op.len() {
                    self.bump();
                }
                self.push_at(TokenKind::Op, *op, line, col, start);
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
                self.close_layout_islands_before_bracket_close(line, col);
                self.depth -= 1;
            }
            self.bump();
            self.push_at(TokenKind::Op, c.to_string(), line, col, start);
            return Ok(());
        }
        Err(self.error(format!("unexpected character `{c}`")))
    }

    fn close_layout_islands_before_bracket_close(&mut self, line: u32, col: u32) {
        while let Some(island) = self.islands.last() {
            if island.open_depth != self.depth {
                break;
            }
            while self
                .islands
                .last()
                .expect("checked Some above")
                .indents
                .len()
                > 1
            {
                self.islands
                    .last_mut()
                    .expect("checked Some above")
                    .indents
                    .pop();
                self.push(TokenKind::Dedent, "", line, col);
                self.push(TokenKind::Newline, "", line, col);
            }
            self.islands.pop();
        }
    }
}

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

#[cfg(test)]
mod tests {
    use super::*;

    fn kinds(src: &str) -> Vec<TokenKind> {
        lex(src)
            .unwrap_or_else(|e| panic!("expected `{src:?}` to lex, got {e:?}"))
            .into_iter()
            .map(|t| t.kind)
            .collect()
    }

    #[test]
    fn four_space_indent_opens_and_closes_one_level() {
        let toks = kinds("a\n    b\n");
        assert_eq!(
            toks.iter().filter(|k| **k == TokenKind::Indent).count(),
            1,
            "found {toks:?}"
        );
        assert_eq!(
            toks.iter().filter(|k| **k == TokenKind::Dedent).count(),
            1,
            "found {toks:?}"
        );
    }

    #[test]
    fn nested_indent_dedent_counts_balance() {
        let toks = kinds("a\n    b\n        c\n    d\ne\n");
        let indents = toks.iter().filter(|k| **k == TokenKind::Indent).count();
        let dedents = toks.iter().filter(|k| **k == TokenKind::Dedent).count();
        assert_eq!(indents, 2, "two nested levels open two INDENTs: {toks:?}");
        assert_eq!(
            dedents, 2,
            "two nested levels close with two DEDENTs: {toks:?}"
        );
        let mut depth = 0i32;
        for k in &toks {
            match k {
                TokenKind::Indent => depth += 1,
                TokenKind::Dedent => depth -= 1,
                _ => {}
            }
            assert!(depth >= 0, "nesting depth must never go negative: {toks:?}");
        }
        assert_eq!(depth, 0, "every INDENT must be matched by EOF: {toks:?}");
    }

    #[test]
    fn tab_in_leading_indentation_rejected() {
        assert!(
            lex("a\n\tb\n").is_err(),
            "a tab in leading indentation must be rejected"
        );
    }

    #[test]
    fn tab_anywhere_in_source_rejected() {
        assert!(
            lex("a\tb\n").is_err(),
            "a tab anywhere in source is rejected outright, not only in indentation"
        );
    }

    #[test]
    fn five_space_indent_rejected() {
        assert!(
            lex("a\n     b\n").is_err(),
            "indentation must deepen by exactly four spaces, not five"
        );
    }

    #[test]
    fn three_space_indent_rejected() {
        assert!(
            lex("a\n   b\n").is_err(),
            "indentation must deepen by exactly four spaces, not three"
        );
    }

    #[test]
    fn blank_and_comment_only_lines_emit_no_layout_tokens() {
        let toks = kinds("a\n    b\n\n    # comment\n    c\nd\n");
        assert_eq!(
            toks.iter().filter(|k| **k == TokenKind::Indent).count(),
            1,
            "the blank line and the comment-only line must not open a new INDENT: {toks:?}"
        );
        assert_eq!(
            toks.iter().filter(|k| **k == TokenKind::Dedent).count(),
            1,
            "the blank line and the comment-only line must not close the block early: {toks:?}"
        );
        assert!(
            !toks.contains(&TokenKind::DocComment),
            "a plain `#` comment is not a doc comment: {toks:?}"
        );
    }

    #[test]
    fn island_opens_for_bracketed_multi_statement_suite() {
        let toks = lex("f(||:\n    a\n    b\n)\n").expect("should lex");
        let kinds: Vec<_> = toks.iter().map(|t| t.kind.clone()).collect();
        assert_eq!(
            kinds.iter().filter(|k| **k == TokenKind::Indent).count(),
            1,
            "one INDENT for the suite body: {kinds:?}"
        );
        assert_eq!(
            kinds.iter().filter(|k| **k == TokenKind::Dedent).count(),
            1,
            "one balancing DEDENT closing the island: {kinds:?}"
        );
        for name in ["a", "b"] {
            let i = toks
                .iter()
                .position(|t| t.kind == TokenKind::Ident && t.text == name)
                .unwrap_or_else(|| panic!("`{name}` token: {kinds:?}"));
            assert_eq!(
                toks[i + 1].kind,
                TokenKind::Newline,
                "`{name}` must be immediately followed by a real Newline: {kinds:?}"
            );
        }
    }

    #[test]
    fn bracket_inside_island_suppresses_layout_again() {
        let toks = kinds("f(||:\n    a(\n        1,\n        2,\n    )\n    b\n)\n");
        assert_eq!(
            toks.iter().filter(|k| **k == TokenKind::Indent).count(),
            1,
            "the nested call's own multi-line argument list opens no \
             INDENT of its own: {toks:?}"
        );
        assert_eq!(
            toks.iter().filter(|k| **k == TokenKind::Dedent).count(),
            1,
            "{toks:?}"
        );
    }

    #[test]
    fn island_closes_before_bracket_when_never_dedents_on_own_line() {
        let toks = lex("f(||:\n    a\n    b)\n").expect("should lex");
        let close_paren_idx = toks
            .iter()
            .position(|t| t.kind == TokenKind::Op && t.text == ")")
            .expect("closing paren token");
        assert!(
            close_paren_idx >= 2
                && toks[close_paren_idx - 2].kind == TokenKind::Dedent
                && toks[close_paren_idx - 1].kind == TokenKind::Newline,
            "the island's DEDENT/NEWLINE must appear immediately before \
             the closing `)`: {toks:?}"
        );
    }

    #[test]
    fn colon_same_line_content_does_not_open_island() {
        let toks = kinds("f(||: a)\n");
        assert!(
            !toks.contains(&TokenKind::Indent) && !toks.contains(&TokenKind::Dedent),
            "no layout tokens for a same-line inline suite: {toks:?}"
        );
    }

    #[test]
    fn radix_literals_reject_digits_outside_their_set() {
        for src in ["0b102", "0o9", "0b2", "0o8"] {
            let err = lex(src).expect_err(&format!("{src} must be a lex error"));
            assert!(
                err.message.contains("invalid digit") || err.message.contains("radix"),
                "{src}: {err:?}"
            );
        }
    }

    #[test]
    fn radix_prefix_with_no_digits_is_a_lex_error() {
        for src in ["0x", "0o", "0b"] {
            let err = lex(src).expect_err(&format!("{src} must be a lex error"));
            assert!(err.message.contains("at least one digit"), "{src}: {err:?}");
        }
    }

    #[test]
    fn underscores_must_sit_between_digits() {
        for src in ["1__0", "0x_1", "0b_0", "0o_7", "1_", "0x1_"] {
            let err = lex(src).expect_err(&format!("{src} must be a lex error"));
            assert!(err.message.contains("underscore"), "{src}: {err:?}");
        }
    }

    #[test]
    fn many_blank_lines_lex_without_stack_overflow() {
        let src = "\n".repeat(100_000) + "x\n";
        let toks = lex(&src).expect("blank-line loop must not overflow");
        assert!(
            toks.iter()
                .any(|t| t.kind == TokenKind::Ident && t.text == "x"),
            "identifier after the blank run must still lex"
        );
    }

    #[test]
    fn content_tokens_retain_exact_source_byte_bounds() {
        let source = "module scene\n    value = f\"é {value}\"\n";
        let tokens = lex(source).unwrap();
        let module = tokens.iter().find(|token| token.text == "module").unwrap();
        let scene = tokens.iter().find(|token| token.text == "scene").unwrap();
        let value = tokens.iter().find(|token| token.text == "value").unwrap();
        let fstring = tokens
            .iter()
            .find(|token| token.kind == TokenKind::FStr)
            .unwrap();
        assert_eq!((module.byte_start, module.byte_end), (0, 6));
        assert_eq!((scene.byte_start, scene.byte_end), (7, 12));
        assert_eq!((value.byte_start, value.byte_end), (17, 22));
        assert_eq!(
            &source.as_bytes()[fstring.byte_start..fstring.byte_end],
            "f\"é {value}\"".as_bytes()
        );
    }
}
