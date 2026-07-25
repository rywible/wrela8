//! F-string desugar (plans/M9.md item D; 02-language.md §1.1 / 05 §6).
//!
//! `f"...{expr:spec}..."` becomes a left-fold of `String[..N] + String[..M]`
//! over literal pieces and `expr.format()` calls — the same Format + concat
//! surface item C2 locked. Empty-spec only (`{expr}` or `{expr:}` with an
//! empty spec); a nonempty `spec` refuses by name. `{{`/`}}` are already
//! unescaped into literal parts by `syntax::parser::split_fstring`.

use crate::sema::SemaError;
use crate::syntax::ast::{BinOp, Expr, FStringLit, FStringPart, Span};
use crate::syntax::{lexer, parser};

/// Desugar an f-string into ordinary `+` / `.format()` expressions so
/// bodies, access, flow, matches, and symbols all see one tree.
pub(crate) fn desugar_fstring(f: &FStringLit) -> Result<Expr, SemaError> {
    if f.parts.is_empty() {
        return Ok(Expr::Str(f.span, "\"\"".to_string()));
    }
    let mut acc: Option<Expr> = None;
    for part in &f.parts {
        let piece = match part {
            FStringPart::Literal(span, text) => {
                // Literal text still carries source escapes (`\n`, `\"`);
                // wrapping as a Str token lets `decode_str` settle bytes.
                Expr::Str(*span, format!("\"{text}\""))
            }
            FStringPart::Interp(span, text) => {
                let (expr_src, spec) = split_interp_spec(text);
                if expr_src.is_empty() {
                    return Err(SemaError::at(
                        "type",
                        "f-string interpolation expression is empty".to_string(),
                        *span,
                    ));
                }
                if !spec.is_empty() {
                    // C2 locked empty-spec Format; nonempty specs stay
                    // fail-closed rather than inventing FormatSpec.
                    return Err(SemaError::at(
                        "type",
                        "non-empty f-string format specs are not supported \
                         (empty-spec Format only)"
                            .to_string(),
                        *span,
                    ));
                }
                let inner = parse_interp_expr(expr_src, *span)?;
                Expr::Call(
                    Box::new(Expr::Field(Box::new(inner), *span, "format".to_string())),
                    *span,
                    Vec::new(),
                )
            }
        };
        acc = Some(match acc {
            None => piece,
            Some(left) => {
                let span = piece.span();
                Expr::Binary(span, BinOp::Add, Box::new(left), Box::new(piece))
            }
        });
    }
    Ok(acc.expect("parts non-empty"))
}

/// Split `{expression [: format_spec]}` on the first top-level `:`.
fn split_interp_spec(text: &str) -> (&str, &str) {
    let bytes = text.as_bytes();
    let mut paren = 0i32;
    let mut brack = 0i32;
    let mut brace = 0i32;
    let mut i = 0usize;
    while i < bytes.len() {
        match bytes[i] {
            b'(' => paren += 1,
            b')' => paren -= 1,
            b'[' => brack += 1,
            b']' => brack -= 1,
            b'{' => brace += 1,
            b'}' => brace -= 1,
            b':' if paren == 0 && brack == 0 && brace == 0 => {
                let expr = text[..i].trim();
                let spec = text[i + 1..].trim();
                return (expr, spec);
            }
            _ => {}
        }
        i += 1;
    }
    (text.trim(), "")
}

fn parse_interp_expr(src: &str, span: Span) -> Result<Expr, SemaError> {
    let tokens = lexer::lex(src).map_err(|e| {
        SemaError::at(
            "type",
            format!("f-string interpolation: {}", e.message),
            Span {
                line: span.line,
                col: span.col.saturating_add(e.col.saturating_sub(1)),
            },
        )
    })?;
    parser::parse_expr(tokens).map_err(|e| {
        SemaError::at(
            "type",
            format!("f-string interpolation: {}", e.message),
            Span {
                line: span.line,
                col: span.col.saturating_add(e.col.saturating_sub(1)),
            },
        )
    })
}
