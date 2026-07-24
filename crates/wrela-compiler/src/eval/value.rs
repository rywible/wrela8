//! Comptime evaluator values (plans/M3.md item B, decision 5): one plain
//! enum. Scalars are stored at target width (a distinct Rust integer
//! type per wrela scalar type, rather than one wide host integer plus a
//! tag — decision 4's "dumb, no seams for their own sake" applied to
//! values the same way it already applies to `sema::types::Type`);
//! `bool`/`char`/`unit`; tuples/arrays as `Vec<Value>`; structs as
//! field-ordered `Vec<Value>` (`typed::TypedStruct::fields`, plans/M3.md
//! item B's own addition to the typed tree, gives the name<->index
//! mapping — see that field's doc comment); enums as `(variant index,
//! payload values)`; `Static[Str]`/`Static[Bytes[N]]` as owned bytes;
//! named functions as callee keys. `Clone`s freely (CLAUDE.md) — no
//! interning, no arena, no `Rc`.
//!
//! Arithmetic is exact per docs/language/02-language.md §6.1: ordinary
//! `+ - *` (and negation, and `MIN / -1`) abandon on overflow in every
//! profile; `+% -% *%` wrap modulo `2^width`; division truncates toward
//! zero and abandons on division by zero; shifts abandon on an
//! out-of-range count or (for `<<`) lost bits. Every "abandon" path here
//! returns `Err(String)` — a bare description of the operation that
//! overflowed/abandoned (no call-stack, no formatting) — `interp.rs`
//! wraps it into an `EvalError` with the live comptime call stack
//! attached (decision 5's "carrying the comptime call stack").

use std::collections::BTreeMap;

use crate::sema::typed::{CalleeKey, TypedClosureBody, TypedClosureParam};
use crate::sema::types::Type;
use crate::syntax::ast::BinOp;

/// `Option`'s two variants, indexed in declaration order (`None` first,
/// `Some` second) — 02-language.md §2's fixed vocabulary; there is no
/// `DeclEnum` for it to read an order from (`prelude.rs`), so the order
/// is pinned here instead.
pub const OPTION_NONE: usize = 0;
pub const OPTION_SOME: usize = 1;
/// `Result`'s two variants, indexed in declaration order (`Ok` first,
/// `Err` second) — same reasoning as `OPTION_NONE`/`OPTION_SOME`.
pub const RESULT_OK: usize = 0;
pub const RESULT_ERR: usize = 1;

pub type Env = Vec<BTreeMap<String, Value>>;

#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    U8(u8),
    U16(u16),
    U32(u32),
    U64(u64),
    Usize(u64),
    I8(i8),
    I16(i16),
    I32(i32),
    I64(i64),
    Isize(i64),
    F32(f32),
    F64(f64),
    Bool(bool),
    Char(char),
    Unit,
    /// `Static[Str]` — owned, already-decoded UTF-8 bytes.
    Str(Vec<u8>),
    /// `Static[Bytes[N]]` and peers — owned bytes.
    Bytes(Vec<u8>),
    Tuple(Vec<Value>),
    /// A fixed-array value.
    Array(Vec<Value>),
    /// Field-ordered (decision 5): index `i` holds the value of
    /// `typed::TypedStruct::fields[i]`.
    Struct(Vec<Value>),
    /// `(variant index, payload)` — `OPTION_NONE`/`OPTION_SOME`/
    /// `RESULT_OK`/`RESULT_ERR` above for the two builtin sums; a user
    /// enum's variant index is its position in `sema::types::DeclEnum::variants`
    /// (declaration order), threaded through by `interp.rs` at
    /// construction/match time (the typed tree names variants, not
    /// indices — `interp::variant_index` looks the position up).
    Enum(usize, Vec<Value>),
    /// A bare fn/method value (`TypedExprKind::FnRef`) — never called
    /// through this variant directly; `CallValue` resolves it back to a
    /// callee key and dispatches through the ordinary call path.
    Fn(CalleeKey),
    /// A closure literal's value: params/body cloned straight off the
    /// typed node, plus a snapshot of the defining environment at
    /// creation time (plans/M3.md item B, "closures (non-escaping,
    /// direct application)" — a snapshot clone is correct and simplest;
    /// no true capture-by-reference, no `Rc`).
    Closure {
        params: Vec<TypedClosureParam>,
        body: TypedClosureBody,
        env: Env,
    },
    /// One `@image` builder declaration handle (plans/M4.md item B,
    /// decision 5): the result of `img.device`/`img.driver`/`img.actor`/
    /// `img.pool`/`img.dma_pool`, and `decl.handle()`'s own passthrough of
    /// one of these — `eval::image::ImageDeclRef` names *which*
    /// declaration, in construction order (devices/drivers/actors) or by
    /// its own bound pool name (pools/dma pools), mirroring
    /// `ImageGraph`'s own two recording disciplines exactly (that
    /// module's own doc comment).
    ImageDecl(crate::eval::image::ImageDeclRef),
}

impl Value {
    /// The number of `Value`/byte elements this value owns, one level
    /// deep plus its own children — `quota.rs`'s memory counter charges
    /// this at every construction site so a large literal/collection
    /// costs quota proportionally to its own size (decision 6: "a simple
    /// running counter is fine").
    pub fn weight(&self) -> u64 {
        match self {
            Value::Str(b) | Value::Bytes(b) => b.len() as u64,
            Value::Tuple(v) | Value::Array(v) | Value::Struct(v) => {
                1 + v.iter().map(Value::weight).sum::<u64>()
            }
            Value::Enum(_, v) => 1 + v.iter().map(Value::weight).sum::<u64>(),
            _ => 1,
        }
    }
}

// --- integer scalar helpers ------------------------------------------------

/// `(bit width, signed)` for the ten integer scalar types; `None` for
/// anything else (float/bool/char/...).
fn int_shape(ty: &Type) -> Option<(u32, bool)> {
    match ty {
        Type::U8 => Some((8, false)),
        Type::U16 => Some((16, false)),
        Type::U32 => Some((32, false)),
        Type::U64 | Type::Usize => Some((64, false)),
        Type::I8 => Some((8, true)),
        Type::I16 => Some((16, true)),
        Type::I32 => Some((32, true)),
        Type::I64 | Type::Isize => Some((64, true)),
        _ => None,
    }
}

fn int_bounds(ty: &Type) -> Option<(i128, i128)> {
    match ty {
        Type::U8 => Some((0, u8::MAX as i128)),
        Type::U16 => Some((0, u16::MAX as i128)),
        Type::U32 => Some((0, u32::MAX as i128)),
        Type::U64 | Type::Usize => Some((0, u64::MAX as i128)),
        Type::I8 => Some((i8::MIN as i128, i8::MAX as i128)),
        Type::I16 => Some((i16::MIN as i128, i16::MAX as i128)),
        Type::I32 => Some((i32::MIN as i128, i32::MAX as i128)),
        Type::I64 | Type::Isize => Some((i64::MIN as i128, i64::MAX as i128)),
        _ => None,
    }
}

/// Reads any integer-scalar `Value` out as a host `i128` (always lossless:
/// the widest wrela integer is 64-bit). `None` for a non-integer value.
pub fn as_i128(v: &Value) -> Option<i128> {
    Some(match *v {
        Value::U8(x) => x as i128,
        Value::U16(x) => x as i128,
        Value::U32(x) => x as i128,
        Value::U64(x) => x as i128,
        Value::Usize(x) => x as i128,
        Value::I8(x) => x as i128,
        Value::I16(x) => x as i128,
        Value::I32(x) => x as i128,
        Value::I64(x) => x as i128,
        Value::Isize(x) => x as i128,
        _ => return None,
    })
}

/// Builds an integer-scalar `Value` of type `ty` from a host `i128`,
/// truncating to the type's own bit pattern (`as`'s own two's-complement
/// truncation) — callers only ever pass an already-range-checked (or
/// intentionally wrapped/masked) value, never a raw unchecked one.
pub fn make_int(ty: &Type, v: i128) -> Value {
    match ty {
        Type::U8 => Value::U8(v as u8),
        Type::U16 => Value::U16(v as u16),
        Type::U32 => Value::U32(v as u32),
        Type::U64 => Value::U64(v as u64),
        Type::Usize => Value::Usize(v as u64),
        Type::I8 => Value::I8(v as i8),
        Type::I16 => Value::I16(v as i16),
        Type::I32 => Value::I32(v as i32),
        Type::I64 => Value::I64(v as i64),
        Type::Isize => Value::Isize(v as i64),
        other => unreachable!("make_int: `{other:?}` is not an integer scalar type"),
    }
}

/// Parses an integer literal's raw source text (`0x`/`0o`/`0b`/decimal,
/// `_` separators) — mirrors `sema::bodies::parse_int_literal` exactly
/// (the literal is already range-checked by sema; this only decodes the
/// digits).
pub fn parse_int_literal(text: &str) -> Option<i128> {
    let cleaned: String = text.chars().filter(|c| *c != '_').collect();
    let (radix, digits): (u32, &str) = if let Some(d) = cleaned
        .strip_prefix("0x")
        .or_else(|| cleaned.strip_prefix("0X"))
    {
        (16, d)
    } else if let Some(d) = cleaned
        .strip_prefix("0o")
        .or_else(|| cleaned.strip_prefix("0O"))
    {
        (8, d)
    } else if let Some(d) = cleaned
        .strip_prefix("0b")
        .or_else(|| cleaned.strip_prefix("0B"))
    {
        (2, d)
    } else {
        (10, cleaned.as_str())
    };
    i128::from_str_radix(digits, radix).ok()
}

fn checked_op(op: BinOp, a: i128, b: i128) -> Option<i128> {
    match op {
        BinOp::Add => a.checked_add(b),
        BinOp::Sub => a.checked_sub(b),
        BinOp::Mul => a.checked_mul(b),
        _ => unreachable!("checked_op: `{}` is not ordinary arithmetic", op.as_str()),
    }
}

/// Ordinary `+ - *` (02-language.md §6.1): abandon on overflow in every
/// profile — checked in `i128` (always wide enough: the widest wrela
/// integer is 64-bit) against the target type's own bounds.
pub fn eval_ordinary(op: BinOp, ty: &Type, l: &Value, r: &Value) -> Result<Value, String> {
    let (a, b) = (as_i128(l).unwrap(), as_i128(r).unwrap());
    let (min, max) = int_bounds(ty).expect("eval_ordinary: not an integer scalar type");
    let raw = checked_op(op, a, b);
    match raw {
        Some(v) if v >= min && v <= max => Ok(make_int(ty, v)),
        _ => Err(format!("arithmetic overflow in `{}`", op.as_str())),
    }
}

/// Wrapping `+% -% *%` (02-language.md §6.1): reduce modulo `2^width`,
/// never abandons.
pub fn eval_wrapping(op: BinOp, ty: &Type, l: &Value, r: &Value) -> Value {
    let (bits, signed) = int_shape(ty).expect("eval_wrapping: not an integer scalar type");
    let (a, b) = (as_i128(l).unwrap(), as_i128(r).unwrap());
    let raw = match op {
        BinOp::AddW => a.wrapping_add(b),
        BinOp::SubW => a.wrapping_sub(b),
        BinOp::MulW => a.wrapping_mul(b),
        _ => unreachable!(
            "eval_wrapping: `{}` is not wrapping arithmetic",
            op.as_str()
        ),
    };
    make_int(ty, mask_to_width(raw, bits, signed))
}

/// Reduces a host `i128` to the two's-complement bit pattern of `bits`
/// width, then reinterprets it back as signed/unsigned per `signed` —
/// the shared "wrap to width" step both `eval_wrapping` and the shift
/// operators use.
fn mask_to_width(v: i128, bits: u32, signed: bool) -> i128 {
    let mask: u128 = if bits >= 128 {
        u128::MAX
    } else {
        (1u128 << bits) - 1
    };
    let bits_pattern = (v as u128) & mask;
    if signed && bits < 128 && (bits_pattern & (1u128 << (bits - 1))) != 0 {
        // Sign-extend: the top bit of this width is set, so the value is
        // negative in `bits`-wide two's complement.
        (bits_pattern as i128) - (1i128 << bits)
    } else {
        bits_pattern as i128
    }
}

/// Division/remainder (02-language.md §6.1): truncates toward zero
/// (`i128`'s own `/`/`%` already do this); abandons on division by
/// zero, and on the signed `MIN / -1` overflow case. `checked_div`/
/// `checked_rem` alone are not enough to detect that second case here:
/// they only return `None` on the *native* width's own `MIN / -1`
/// (e.g. `i32::MIN`), but this runs the division in `i128` — wide
/// enough that `i32::MIN / -1` never overflows `i128` itself — so the
/// result is bounds-checked against the *target* type afterward,
/// exactly like `eval_ordinary`.
pub fn eval_div_rem(op: BinOp, ty: &Type, l: &Value, r: &Value) -> Result<Value, String> {
    let (a, b) = (as_i128(l).unwrap(), as_i128(r).unwrap());
    let (min, max) = int_bounds(ty).expect("eval_div_rem: not an integer scalar type");
    if b == 0 {
        return Err(format!(
            "{} by zero",
            if op == BinOp::Div {
                "division"
            } else {
                "remainder"
            }
        ));
    }
    let raw = match op {
        BinOp::Div => a / b,
        BinOp::Rem => a % b,
        _ => unreachable!("eval_div_rem: `{}` is not division/remainder", op.as_str()),
    };
    if raw >= min && raw <= max {
        Ok(make_int(ty, raw))
    } else {
        Err(format!("arithmetic overflow in `{}`", op.as_str()))
    }
}

/// Shifts (02-language.md §6.1): abandon on an out-of-range count (`>=
/// width`); `<<` additionally abandons if any bit shifted out (discarded)
/// was set ("lost bits") — checked on the value's own bit pattern,
/// signedness aside. `count` shares `l`'s own type (`check_binary`
/// requires same-type operands for every builtin op, shifts included).
pub fn eval_shift(op: BinOp, ty: &Type, l: &Value, count: &Value) -> Result<Value, String> {
    let (bits, signed) = int_shape(ty).expect("eval_shift: not an integer scalar type");
    let a = as_i128(l).unwrap();
    let c = as_i128(count).unwrap();
    if c < 0 || c >= bits as i128 {
        return Err(format!(
            "shift count {c} is out of range for a {bits}-bit type"
        ));
    }
    let c = c as u32;
    let mask: u128 = if bits >= 128 {
        u128::MAX
    } else {
        (1u128 << bits) - 1
    };
    let bit_pattern = (a as u128) & mask;
    match op {
        BinOp::Shl => {
            if c > 0 {
                let lost = bit_pattern >> (bits - c);
                if lost != 0 {
                    return Err("`<<` lost nonzero high bits".to_string());
                }
            }
            let raw = (bit_pattern << c) & mask;
            Ok(make_int(ty, mask_to_width(raw as i128, bits, signed)))
        }
        BinOp::Shr => {
            // Arithmetic shift for signed types (sign-fills), logical for
            // unsigned — matches Rust's own `>>` per host integer type,
            // reproduced here on the bit pattern directly.
            let raw = if signed {
                a >> c
            } else {
                (bit_pattern >> c) as i128
            };
            Ok(make_int(ty, mask_to_width(raw, bits, signed)))
        }
        _ => unreachable!("eval_shift: `{}` is not a shift", op.as_str()),
    }
}

/// Bitwise `& | ^` — never abandons.
pub fn eval_bitwise(op: BinOp, ty: &Type, l: &Value, r: &Value) -> Value {
    let (bits, signed) = int_shape(ty).expect("eval_bitwise: not an integer scalar type");
    let (a, b) = (as_i128(l).unwrap(), as_i128(r).unwrap());
    let raw = match op {
        BinOp::BitAnd => a & b,
        BinOp::BitOr => a | b,
        BinOp::BitXor => a ^ b,
        _ => unreachable!("eval_bitwise: `{}` is not bitwise", op.as_str()),
    };
    make_int(ty, mask_to_width(raw, bits, signed))
}

/// Ordering (`< <= > >=`) — numeric scalars and `char` only
/// (`build_binop_expr`'s own scope).
pub fn eval_compare(op: BinOp, l: &Value, r: &Value) -> bool {
    use std::cmp::Ordering;
    let ord = match (l, r) {
        (Value::F32(a), Value::F32(b)) => a.partial_cmp(b),
        (Value::F64(a), Value::F64(b)) => a.partial_cmp(b),
        (Value::Char(a), Value::Char(b)) => Some(a.cmp(b)),
        _ => as_i128(l).and_then(|a| as_i128(r).map(|b| a.cmp(&b))),
    };
    // A `partial_cmp` of `None` (a NaN operand) makes every ordering
    // comparison false, matching IEEE 754 (02-language.md §6.1: "strict
    // IEEE 754 with canonical NaN").
    let Some(ord) = ord else { return false };
    match op {
        BinOp::Lt => ord == Ordering::Less,
        BinOp::Le => ord != Ordering::Greater,
        BinOp::Gt => ord == Ordering::Greater,
        BinOp::Ge => ord != Ordering::Less,
        other => unreachable!("eval_compare: `{}` is not an ordering", other.as_str()),
    }
}

/// Unary negation (02-language.md §6.1): abandons on overflow (the one
/// signed case, `MIN.neg()`); floats never abandon.
pub fn eval_neg(v: &Value) -> Result<Value, String> {
    let checked = match v {
        Value::F32(x) => return Ok(Value::F32(-x)),
        Value::F64(x) => return Ok(Value::F64(-x)),
        Value::I8(x) => x.checked_neg().map(Value::I8),
        Value::I16(x) => x.checked_neg().map(Value::I16),
        Value::I32(x) => x.checked_neg().map(Value::I32),
        Value::I64(x) => x.checked_neg().map(Value::I64),
        Value::Isize(x) => x.checked_neg().map(Value::Isize),
        other => unreachable!("eval_neg: `{other:?}` is not signed/float"),
    };
    checked.ok_or_else(|| "arithmetic overflow in unary `-`".to_string())
}

/// Bitwise NOT (`~`) — never abandons.
pub fn eval_bitnot(ty: &Type, v: &Value) -> Value {
    let (bits, signed) = int_shape(ty).expect("eval_bitnot: not an integer scalar type");
    let a = as_i128(v).unwrap();
    let mask: u128 = if bits >= 128 {
        u128::MAX
    } else {
        (1u128 << bits) - 1
    };
    let raw = (!(a as u128)) & mask;
    make_int(ty, mask_to_width(raw as i128, bits, signed))
}

/// `x.to[T]()` (02-language.md §6.1): checked scalar-to-scalar
/// conversion — build error (abandon) when the value does not fit `T`.
pub fn eval_to_scalar(target: &Type, v: &Value) -> Result<Value, String> {
    if let Some((_, _)) = int_shape(target) {
        let (min, max) = int_bounds(target).unwrap();
        let raw = match v {
            Value::F32(x) => {
                if !x.is_finite() {
                    return Err("conversion from a non-finite float".to_string());
                }
                x.trunc() as i128
            }
            Value::F64(x) => {
                if !x.is_finite() {
                    return Err("conversion from a non-finite float".to_string());
                }
                x.trunc() as i128
            }
            other => as_i128(other).ok_or_else(|| "unsupported `.to` conversion".to_string())?,
        };
        return if raw >= min && raw <= max {
            Ok(make_int(target, raw))
        } else {
            Err(format!(
                "`.to[{}]()` conversion out of range",
                crate::sema::types::render_type(target)
            ))
        };
    }
    match target {
        Type::F32 => {
            let raw = match v {
                Value::F64(x) => *x as f32,
                other => {
                    as_i128(other).ok_or_else(|| "unsupported `.to` conversion".to_string())? as f32
                }
            };
            Ok(Value::F32(raw))
        }
        Type::F64 => {
            let raw = match v {
                Value::F32(x) => *x as f64,
                other => {
                    as_i128(other).ok_or_else(|| "unsupported `.to` conversion".to_string())? as f64
                }
            };
            Ok(Value::F64(raw))
        }
        _ => Err("unsupported `.to` conversion".to_string()),
    }
}

// --- literal text decoding (str/bstr/char) --------------------------------

/// Decodes a plain string literal's raw token text (`"..."`, quotes
/// included, never a prefix — an f-string never reaches a typed `Str`
/// node, sema's own fail-closed rule) into its UTF-8 bytes.
pub fn decode_str(text: &str) -> Vec<u8> {
    let inner = &text[1..text.len() - 1];
    decode_escapes(inner, false)
        .into_iter()
        .collect::<String>()
        .into_bytes()
}

/// Decodes a byte-string literal's raw token text (`b"..."`) into its
/// raw bytes (`\xNN` allowed, `\u{...}` is not — lexer-enforced already).
pub fn decode_bstr(text: &str) -> Vec<u8> {
    let inner = &text[2..text.len() - 1];
    decode_byte_escapes(inner)
}

/// Decodes a char literal's raw token text (`'x'`) into its codepoint.
pub fn decode_char(text: &str) -> char {
    let inner = &text[1..text.len() - 1];
    let decoded = decode_escapes(inner, true);
    decoded
        .into_iter()
        .next()
        .expect("lexer guarantees exactly one char literal codepoint")
}

/// Shared text-escape decoder (str/char contexts: `\\ \" \' \n \r \t \0`
/// plus `\u{H..H}`) — returns the decoded codepoints in source order.
fn decode_escapes(s: &str, _char_ctx: bool) -> Vec<char> {
    let mut out = Vec::new();
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('\\') => out.push('\\'),
            Some('"') => out.push('"'),
            Some('\'') => out.push('\''),
            Some('n') => out.push('\n'),
            Some('r') => out.push('\r'),
            Some('t') => out.push('\t'),
            Some('0') => out.push('\0'),
            Some('u') => {
                // `\u{H..H}` — the lexer already validated shape/digit
                // count; this just decodes it.
                chars.next(); // '{'
                let mut hex = String::new();
                for h in chars.by_ref() {
                    if h == '}' {
                        break;
                    }
                    hex.push(h);
                }
                let cp = u32::from_str_radix(&hex, 16).unwrap_or(0);
                out.push(char::from_u32(cp).unwrap_or('\u{FFFD}'));
            }
            Some(other) => out.push(other),
            None => {}
        }
    }
    out
}

/// Byte-string escape decoder (`\xNN` in addition to the shared set) —
/// non-escaped source bytes are re-encoded as their own UTF-8 (the
/// source is UTF-8; a byte string's un-escaped bytes are whatever UTF-8
/// bytes the source itself spelled).
fn decode_byte_escapes(s: &str) -> Vec<u8> {
    let mut out = Vec::new();
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '\\' {
            let mut buf = [0u8; 4];
            out.extend_from_slice(c.encode_utf8(&mut buf).as_bytes());
            continue;
        }
        match chars.next() {
            Some('\\') => out.push(b'\\'),
            Some('"') => out.push(b'"'),
            Some('\'') => out.push(b'\''),
            Some('n') => out.push(b'\n'),
            Some('r') => out.push(b'\r'),
            Some('t') => out.push(b'\t'),
            Some('0') => out.push(0),
            Some('x') => {
                let hi = chars.next().unwrap_or('0');
                let lo = chars.next().unwrap_or('0');
                let byte = u8::from_str_radix(&format!("{hi}{lo}"), 16).unwrap_or(0);
                out.push(byte);
            }
            Some(other) => {
                let mut buf = [0u8; 4];
                out.extend_from_slice(other.encode_utf8(&mut buf).as_bytes());
            }
            None => {}
        }
    }
    out
}

// --- scalar arithmetic table (docs/language/02-language.md §6.1) --------
//
// Hand-computed cases, one per row of the doc's own rule table: ordinary
// `+ - *`/negation abandon on overflow at each width's boundary; `+%`
// wraps modulo `2^width`; division/remainder truncate toward zero and
// abandon on division by zero and the signed `MIN / -1` case; shifts
// abandon on an out-of-range count or (for `<<`) lost bits, `>>` is
// arithmetic (sign-filling) for signed types and logical for unsigned;
// `.to[T]()` is a checked conversion. plans/M3.md item B's own required
// "at least a dozen" table.
#[cfg(test)]
mod scalar_arithmetic_tests {
    use super::*;
    use crate::syntax::ast::BinOp;

    #[test]
    fn ordinary_add_overflows_at_u8_max() {
        assert_eq!(
            eval_ordinary(BinOp::Add, &Type::U8, &Value::U8(250), &Value::U8(10)),
            Err("arithmetic overflow in `+`".to_string())
        );
    }

    #[test]
    fn ordinary_add_in_range_at_u8_max() {
        assert_eq!(
            eval_ordinary(BinOp::Add, &Type::U8, &Value::U8(250), &Value::U8(5)),
            Ok(Value::U8(255))
        );
    }

    #[test]
    fn ordinary_add_overflows_at_i8_max() {
        assert_eq!(
            eval_ordinary(BinOp::Add, &Type::I8, &Value::I8(100), &Value::I8(100)),
            Err("arithmetic overflow in `+`".to_string())
        );
    }

    #[test]
    fn ordinary_sub_overflows_at_i8_min() {
        assert_eq!(
            eval_ordinary(BinOp::Sub, &Type::I8, &Value::I8(i8::MIN), &Value::I8(1)),
            Err("arithmetic overflow in `-`".to_string())
        );
    }

    #[test]
    fn ordinary_mul_overflows_at_u16_max() {
        assert_eq!(
            eval_ordinary(BinOp::Mul, &Type::U16, &Value::U16(1000), &Value::U16(1000)),
            Err("arithmetic overflow in `*`".to_string())
        );
    }

    #[test]
    fn unary_neg_overflows_at_i8_min() {
        assert_eq!(
            eval_neg(&Value::I8(i8::MIN)),
            Err("arithmetic overflow in unary `-`".to_string())
        );
    }

    #[test]
    fn unary_neg_ok_for_i8_max() {
        assert_eq!(eval_neg(&Value::I8(i8::MAX)), Ok(Value::I8(-i8::MAX)));
    }

    #[test]
    fn wrapping_add_wraps_at_u8_max() {
        // 250 +% 10 == 260 mod 256 == 4.
        assert_eq!(
            eval_wrapping(BinOp::AddW, &Type::U8, &Value::U8(250), &Value::U8(10)),
            Value::U8(4)
        );
    }

    #[test]
    fn wrapping_add_wraps_at_i8_max_to_min() {
        assert_eq!(
            eval_wrapping(BinOp::AddW, &Type::I8, &Value::I8(i8::MAX), &Value::I8(1)),
            Value::I8(i8::MIN)
        );
    }

    #[test]
    fn division_truncates_toward_zero_for_negative_operands() {
        // -7 / 2 == -3 (truncating, not -4 as floor division would give).
        assert_eq!(
            eval_div_rem(BinOp::Div, &Type::I32, &Value::I32(-7), &Value::I32(2)),
            Ok(Value::I32(-3))
        );
    }

    #[test]
    fn remainder_matches_truncating_division_sign() {
        // -7 % 2 == -1 (sign follows the dividend under truncation).
        assert_eq!(
            eval_div_rem(BinOp::Rem, &Type::I32, &Value::I32(-7), &Value::I32(2)),
            Ok(Value::I32(-1))
        );
    }

    #[test]
    fn division_by_zero_abandons() {
        assert_eq!(
            eval_div_rem(BinOp::Div, &Type::U32, &Value::U32(9), &Value::U32(0)),
            Err("division by zero".to_string())
        );
    }

    #[test]
    fn signed_min_div_neg_one_abandons() {
        assert_eq!(
            eval_div_rem(
                BinOp::Div,
                &Type::I32,
                &Value::I32(i32::MIN),
                &Value::I32(-1)
            ),
            Err("arithmetic overflow in `/`".to_string())
        );
    }

    #[test]
    fn shift_left_ok_when_no_bits_are_lost() {
        // 1u8 << 7 == 128 (the vacated low bits are all zero already, the
        // shifted-out high bits are all zero too).
        assert_eq!(
            eval_shift(BinOp::Shl, &Type::U8, &Value::U8(1), &Value::U8(7)),
            Ok(Value::U8(128))
        );
    }

    #[test]
    fn shift_left_abandons_on_lost_bits() {
        // 0xFF << 1 discards a set high bit.
        assert_eq!(
            eval_shift(BinOp::Shl, &Type::U8, &Value::U8(0xFF), &Value::U8(1)),
            Err("`<<` lost nonzero high bits".to_string())
        );
    }

    #[test]
    fn shift_abandons_on_out_of_range_count() {
        // A u8 shift count must be < 8.
        assert_eq!(
            eval_shift(BinOp::Shl, &Type::U8, &Value::U8(1), &Value::U8(8)),
            Err("shift count 8 is out of range for a 8-bit type".to_string())
        );
    }

    #[test]
    fn shift_right_is_logical_for_unsigned() {
        assert_eq!(
            eval_shift(BinOp::Shr, &Type::U8, &Value::U8(0xFF), &Value::U8(4)),
            Ok(Value::U8(0x0F))
        );
    }

    #[test]
    fn shift_right_is_arithmetic_for_signed() {
        // -1i8 (0xFF) >> 1 sign-extends to -1, not 0x7F.
        assert_eq!(
            eval_shift(BinOp::Shr, &Type::I8, &Value::I8(-1), &Value::I8(1)),
            Ok(Value::I8(-1))
        );
    }

    #[test]
    fn bitwise_and_masks_bits() {
        assert_eq!(
            eval_bitwise(
                BinOp::BitAnd,
                &Type::U8,
                &Value::U8(0b1010),
                &Value::U8(0b0110)
            ),
            Value::U8(0b0010)
        );
    }

    #[test]
    fn compare_with_nan_is_always_false() {
        assert!(!eval_compare(
            BinOp::Lt,
            &Value::F64(f64::NAN),
            &Value::F64(1.0)
        ));
        assert!(!eval_compare(
            BinOp::Ge,
            &Value::F64(f64::NAN),
            &Value::F64(1.0)
        ));
    }

    #[test]
    fn to_scalar_checked_conversion_out_of_range_abandons() {
        assert_eq!(
            eval_to_scalar(&Type::U8, &Value::I32(-1)),
            Err("`.to[u8]()` conversion out of range".to_string())
        );
    }

    #[test]
    fn to_scalar_checked_conversion_in_range_succeeds() {
        assert_eq!(
            eval_to_scalar(&Type::U8, &Value::U64(200)),
            Ok(Value::U8(200))
        );
    }
}
