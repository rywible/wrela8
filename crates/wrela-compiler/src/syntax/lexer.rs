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
//!
//! ## Layout islands (ledger clause syntax.lexer.layout-islands)
//!
//! `()[]{}` suppress NEWLINE/INDENT/DEDENT — except a `:` immediately
//! followed by a newline while bracket depth > 0 is unambiguously a suite
//! header (a closure body embedded in an enclosing call's argument list,
//! `docs/language/examples/virtio-storage.wr`'s `BlockCache.edit`/`peek`
//! call sites), so layout tracking *resumes* for exactly that suite: a
//! fresh indentation sub-stack opens ("a layout island"), seeded from
//! whatever the innermost enclosing context's current indent level already
//! was (`innermost_indent`), and stays active — real NEWLINE/INDENT/DEDENT
//! tokens flow — until the island closes. Two independent events close it:
//! (a) a later line's indentation falls back to (or below) the island's own
//! base level (`apply_island_indent`'s dedent branch), or (b) a closing
//! bracket brings the bracket depth below the level the island opened at,
//! before the island's own indentation ever dedented on its own line (the
//! `))`-on-one-line shape in `BlockCache.edit`/`peek`; handled in
//! `lex_operator` via `close_layout_islands_before_bracket_close`, which
//! force-emits the balancing DEDENTs *before* the closing bracket's own
//! token). A bracket opened *inside* an active island suppresses layout
//! again immediately (no bookkeeping needed: `layout_active` simply stops
//! matching once `depth` moves past the island's `open_depth`), and a
//! further `:`-newline found while suppressed there opens a nested island
//! the same way, so islands stack arbitrarily deep. A `:`-newline seen
//! while already inside an *active* island's own body (e.g. a nested `if`
//! in a closure suite) is not a new island at all — it's just an ordinary
//! deeper level on that island's existing indent stack, exactly like a
//! nested block at the top level. This closes the roundtrip ambiguity
//! recorded in `plans/pre-M3-findings.md` (finding 3): the parser no
//! longer has to guess statement boundaries inside an embedded suite with
//! more than one statement, because the lexer now hands it real separators.

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

/// One open layout island (module doc comment): a suite whose layout
/// tracking was resumed inside brackets. `open_depth` is the bracket
/// `depth` at the moment its `:`-newline was seen — layout is active for
/// this island exactly while `depth == open_depth`, and it closes for
/// good once a line dedents to (or below) `indents[0]` (`apply_island_indent`)
/// or a closing bracket drives `depth` below `open_depth`
/// (`close_layout_islands_before_bracket_close`). `indents` is a sub-stack
/// exactly like the top-level `Lexer::indents`, seeded with the island's
/// base level as its permanent floor (`indents[0]`) instead of `0`.
struct Island {
    open_depth: usize,
    indents: Vec<u32>,
}

struct Lexer<'s> {
    src: &'s [u8],
    pos: usize,
    line: u32,
    col: u32,
    depth: usize, // () [] {} nesting: newlines are suppressed inside
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
            depth: 0,
            indents: vec![0],
            islands: Vec::new(),
            tokens: Vec::new(),
        }
    }

    /// True when NEWLINE/INDENT/DEDENT tracking is active right now: either
    /// true top level (`depth == 0`, no island ever needed there) or inside
    /// the topmost open island, exactly at the bracket depth it opened at.
    /// A bracket opened deeper than that (inside an island or not) makes
    /// this false again until its own `:`-newline opens a nested island.
    fn layout_active(&self) -> bool {
        match self.islands.last() {
            Some(island) => self.depth == island.open_depth,
            None => self.depth == 0,
        }
    }

    /// The current indentation width of whichever layout context is
    /// innermost right now — an open island's own indent stack if one
    /// exists, else the top-level stack — regardless of whether that
    /// context is presently *active* (suppressed contexts still have a
    /// frozen "current level" from before suppression began). Used only to
    /// seed a newly opened island's base (`open_layout_island`): the common
    /// case is a closure suite one bracket deep with no enclosing island at
    /// all, where this is simply the top-level stack's frozen top.
    fn innermost_indent(&self) -> u32 {
        match self.islands.last() {
            Some(island) => *island
                .indents
                .last()
                .expect("island indent stack is never empty"),
            None => *self.indents.last().expect("indent stack is never empty"),
        }
    }

    /// True exactly when the last token pushed so far is a bare `:` — the
    /// trigger check for opening a layout island. Comments push no token,
    /// so `:  # trailing comment` before the newline still counts: no real
    /// code follows the colon on that line either way (module doc comment:
    /// "a `:` followed by same-line content does not [open an island]").
    fn last_token_is_colon(&self) -> bool {
        matches!(self.tokens.last(), Some(t) if t.kind == TokenKind::Op && t.text == ":")
    }

    /// Opens a new layout island. Only called when `!layout_active()`: a
    /// `:`-newline seen while already active (a nested suite inside an
    /// open island's own body, or an ordinary top-level suite) is not a new
    /// island, just the next line's width being read against the context
    /// that's already tracking it.
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
            if at_line_start && self.layout_active() {
                self.handle_indentation()?;
                at_line_start = false;
                continue;
            }
            match self.peek() {
                None => break,
                Some(b'\n') => {
                    // A `:` immediately followed by this newline while
                    // suppressed (bracket depth > 0, no already-active
                    // island at this depth) is a suite header: open a
                    // layout island before deciding whether to emit the
                    // Newline itself (module doc comment).
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
        // Dispatch to whichever indent stack is active at this depth: the
        // topmost open island if one exactly matches, else the top-level
        // stack. `run()` only calls `handle_indentation` when
        // `layout_active()` held at line start, so exactly one of these
        // applies (module doc comment on layout islands).
        let in_island = matches!(self.islands.last(), Some(i) if i.open_depth == self.depth);
        if in_island {
            self.apply_island_indent(width)
        } else {
            self.apply_top_level_indent(width)
        }
    }

    /// The original (pre-island) top-level indent-stack logic, unchanged:
    /// `width` against `self.indents`, whose floor (`indents[0] == 0`) is
    /// permanent for the whole file.
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

    /// Same shape as `apply_top_level_indent`, but against the topmost
    /// island's own indent sub-stack, whose floor (`indents[0]`) is the
    /// island's base rather than a permanent `0`: reaching it (or falling
    /// below it) closes the island for good instead of erroring (module
    /// doc comment, closing case (a)).
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
        // width < current: dedent, one level at a time, same as the
        // top-level loop — but never popping the island's own floor
        // (indents[0], its base level) off the stack.
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
            // Back at (or below) the island's own base: the suite this
            // island tracked is over — close it and resume suppression.
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
                // A closing bracket that drops `depth` below an open
                // island's `open_depth` closes that island right now (module
                // doc comment, closing case (b)): the balancing DEDENTs are
                // force-emitted before this bracket's own token, since the
                // island's line-based dedent check (`apply_island_indent`)
                // never gets a further line of its own to trigger on.
                self.close_layout_islands_before_bracket_close(line, col);
                self.depth -= 1;
            }
            self.bump();
            self.push(TokenKind::Op, c.to_string(), line, col);
            return Ok(());
        }
        Err(self.error(format!("unexpected character `{c}`")))
    }

    /// Closes every open island whose `open_depth` equals `self.depth` (the
    /// depth this closing-bracket token is about to leave) — at most one,
    /// since island `open_depth`s are strictly increasing bottom-to-top on
    /// the stack (each new island only opens deeper than whatever enclosed
    /// it) — emitting its remaining DEDENT/NEWLINE pairs at the bracket's
    /// own position first.
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

// --- tests --------------------------------------------------------------
//
// 02-language.md §1: "Blocks use a trailing `:` and significant
// indentation, exactly four spaces per level; tabs in leading whitespace
// are errors." These pin the indentation-stack unit behavior directly
// (INDENT/DEDENT balance, the exact-four-spaces rule, tab rejection, and
// blank/comment-only lines being layout-transparent) — the corpus/fuzz
// lanes only check "lexes without panic" and golden dumps pin one fixed
// snippet's exact rendering, neither of which independently confirms
// these structural invariants across arbitrary nesting.
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

    /// A nested snippet's INDENT/DEDENT tokens balance: every INDENT is
    /// matched by exactly one DEDENT by EOF, and the running nesting depth
    /// (INDENT +1, DEDENT -1) never goes negative.
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

    /// Blank lines and comment-only lines are layout-transparent: they
    /// emit no INDENT/DEDENT/NEWLINE of their own and do not perturb the
    /// indentation stack, even when their own leading whitespace would
    /// otherwise mismatch the current level.
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

    // --- layout islands (syntax.lexer.layout-islands) ---------------------

    /// A `:` followed by a newline while bracket depth > 0 opens a layout
    /// island: a multi-statement suite embedded in a call's argument list
    /// gets real NEWLINE/INDENT/DEDENT tokens, closing (case (a)) once a
    /// later line's own indentation returns to the island's base.
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
        // Each body statement (`a`, `b`) is a real, separately lexed
        // token immediately followed by its own Newline — a real
        // separator, not the parser guessing where one statement ends
        // and the next begins.
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

    /// A bracket opened inside an active island suppresses layout again
    /// immediately, with no bookkeeping beyond bracket depth: a multi-line
    /// call nested inside the suite body must not itself contribute any
    /// layout tokens.
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

    /// Closing case (b): a closing bracket that drives depth below the
    /// island's own opening depth force-emits the balancing DEDENT (and its
    /// paired Newline) *before* the bracket's own token, since the island
    /// never gets a further line of its own to dedent on.
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

    /// A `:` followed by content on the same physical line never opens an
    /// island, bracketed or not — the inline single-statement suite stays
    /// exactly as layout-free as before.
    #[test]
    fn colon_same_line_content_does_not_open_island() {
        let toks = kinds("f(||: a)\n");
        assert!(
            !toks.contains(&TokenKind::Indent) && !toks.contains(&TokenKind::Dedent),
            "no layout tokens for a same-line inline suite: {toks:?}"
        );
    }
}
