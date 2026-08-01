use std::collections::BTreeMap;

use crate::sema::typed::{CalleeKey, TypedClosureBody, TypedClosureParam};
use crate::sema::types::Type;
use crate::syntax::ast::BinOp;

pub const OPTION_NONE: usize = 0;
pub const OPTION_SOME: usize = 1;
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
    Str(Vec<u8>),
    Bytes(Vec<u8>),
    Tuple(Vec<Value>),
    Array(Vec<Value>),
    Struct(Vec<Value>),
    Enum(usize, Vec<Value>),
    Fn(CalleeKey),
    Closure {
        params: Vec<TypedClosureParam>,
        body: TypedClosureBody,
        env: Env,
    },
    ImageDecl(crate::eval::image::ImageDeclRef),
}

impl Value {
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

pub(crate) fn int_bounds(ty: &Type) -> Option<(i128, i128)> {
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

pub fn format_scalar(v: &Value) -> Value {
    let s = match v {
        Value::Bool(true) => "true".to_string(),
        Value::Bool(false) => "false".to_string(),
        Value::Char(c) => c.to_string(),
        Value::U8(x) => x.to_string(),
        Value::U16(x) => x.to_string(),
        Value::U32(x) => x.to_string(),
        Value::U64(x) => x.to_string(),
        Value::Usize(x) => x.to_string(),
        Value::I8(x) => x.to_string(),
        Value::I16(x) => x.to_string(),
        Value::I32(x) => x.to_string(),
        Value::I64(x) => x.to_string(),
        Value::Isize(x) => x.to_string(),
        other => unreachable!("format_scalar: `{other:?}` is not a Format scalar"),
    };
    Value::Str(s.into_bytes())
}

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

pub fn eval_ordinary(op: BinOp, ty: &Type, l: &Value, r: &Value) -> Result<Value, String> {
    let a = as_i128(l).ok_or_else(|| "eval: left operand is not an integer scalar".to_string())?;
    let b = as_i128(r).ok_or_else(|| "eval: right operand is not an integer scalar".to_string())?;
    let (min, max) =
        int_bounds(ty).ok_or_else(|| "eval: result type is not an integer scalar".to_string())?;
    let raw = checked_op(op, a, b);
    match raw {
        Some(v) if v >= min && v <= max => Ok(make_int(ty, v)),
        _ => Err(format!("arithmetic overflow in `{}`", op.as_str())),
    }
}

pub fn eval_wrapping(op: BinOp, ty: &Type, l: &Value, r: &Value) -> Result<Value, String> {
    let (bits, signed) =
        int_shape(ty).ok_or_else(|| "eval: result type is not an integer scalar".to_string())?;
    let a = as_i128(l).ok_or_else(|| "eval: left operand is not an integer scalar".to_string())?;
    let b = as_i128(r).ok_or_else(|| "eval: right operand is not an integer scalar".to_string())?;
    let raw = match op {
        BinOp::AddW => a.wrapping_add(b),
        BinOp::SubW => a.wrapping_sub(b),
        BinOp::MulW => a.wrapping_mul(b),
        _ => {
            return Err(format!(
                "eval: `{}` is not wrapping arithmetic",
                op.as_str()
            ));
        }
    };
    Ok(make_int(ty, mask_to_width(raw, bits, signed)))
}

fn mask_to_width(v: i128, bits: u32, signed: bool) -> i128 {
    let mask: u128 = if bits >= 128 {
        u128::MAX
    } else {
        (1u128 << bits) - 1
    };
    let bits_pattern = (v as u128) & mask;
    if signed && bits < 128 && (bits_pattern & (1u128 << (bits - 1))) != 0 {
        (bits_pattern as i128) - (1i128 << bits)
    } else {
        bits_pattern as i128
    }
}

pub fn eval_div_rem(op: BinOp, ty: &Type, l: &Value, r: &Value) -> Result<Value, String> {
    let a = as_i128(l).ok_or_else(|| "eval: left operand is not an integer scalar".to_string())?;
    let b = as_i128(r).ok_or_else(|| "eval: right operand is not an integer scalar".to_string())?;
    let (min, max) =
        int_bounds(ty).ok_or_else(|| "eval: result type is not an integer scalar".to_string())?;
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
        _ => {
            return Err(format!("eval: `{}` is not division/remainder", op.as_str()));
        }
    };
    if raw >= min && raw <= max {
        Ok(make_int(ty, raw))
    } else {
        Err(format!("arithmetic overflow in `{}`", op.as_str()))
    }
}

pub fn eval_shift(op: BinOp, ty: &Type, l: &Value, count: &Value) -> Result<Value, String> {
    let (bits, signed) =
        int_shape(ty).ok_or_else(|| "eval: result type is not an integer scalar".to_string())?;
    let a = as_i128(l).ok_or_else(|| "eval: left operand is not an integer scalar".to_string())?;
    let c =
        as_i128(count).ok_or_else(|| "eval: shift count is not an integer scalar".to_string())?;
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
            let raw = if signed {
                a >> c
            } else {
                (bit_pattern >> c) as i128
            };
            Ok(make_int(ty, mask_to_width(raw, bits, signed)))
        }
        _ => Err(format!("eval: `{}` is not a shift", op.as_str())),
    }
}

pub fn eval_bitwise(op: BinOp, ty: &Type, l: &Value, r: &Value) -> Result<Value, String> {
    let (bits, signed) =
        int_shape(ty).ok_or_else(|| "eval: result type is not an integer scalar".to_string())?;
    let a = as_i128(l).ok_or_else(|| "eval: left operand is not an integer scalar".to_string())?;
    let b = as_i128(r).ok_or_else(|| "eval: right operand is not an integer scalar".to_string())?;
    let raw = match op {
        BinOp::BitAnd => a & b,
        BinOp::BitOr => a | b,
        BinOp::BitXor => a ^ b,
        _ => {
            return Err(format!("eval: `{}` is not bitwise", op.as_str()));
        }
    };
    Ok(make_int(ty, mask_to_width(raw, bits, signed)))
}

pub fn eval_compare(op: BinOp, l: &Value, r: &Value) -> bool {
    use std::cmp::Ordering;
    let ord = match (l, r) {
        (Value::F32(a), Value::F32(b)) => a.partial_cmp(b),
        (Value::F64(a), Value::F64(b)) => a.partial_cmp(b),
        (Value::Char(a), Value::Char(b)) => Some(a.cmp(b)),
        _ => as_i128(l).and_then(|a| as_i128(r).map(|b| a.cmp(&b))),
    };
    let Some(ord) = ord else { return false };
    match op {
        BinOp::Lt => ord == Ordering::Less,
        BinOp::Le => ord != Ordering::Greater,
        BinOp::Gt => ord == Ordering::Greater,
        BinOp::Ge => ord != Ordering::Less,
        other => unreachable!("eval_compare: `{}` is not an ordering", other.as_str()),
    }
}

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

pub fn eval_bitnot(ty: &Type, v: &Value) -> Result<Value, String> {
    let (bits, signed) =
        int_shape(ty).ok_or_else(|| "eval: result type is not an integer scalar".to_string())?;
    let a = as_i128(v).ok_or_else(|| "eval: operand is not an integer scalar".to_string())?;
    let mask: u128 = if bits >= 128 {
        u128::MAX
    } else {
        (1u128 << bits) - 1
    };
    let raw = (!(a as u128)) & mask;
    Ok(make_int(ty, mask_to_width(raw as i128, bits, signed)))
}

pub fn eval_to_scalar(target: &Type, v: &Value) -> Result<Value, String> {
    if let Some((_, _)) = int_shape(target) {
        let (min, max) = int_bounds(target)
            .ok_or_else(|| "eval: `.to[T]()` target is not an integer scalar".to_string())?;
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

pub fn decode_str(text: &str) -> Vec<u8> {
    let inner = &text[1..text.len() - 1];
    decode_escapes(inner, false)
        .into_iter()
        .collect::<String>()
        .into_bytes()
}

pub fn decode_bstr(text: &str) -> Vec<u8> {
    let inner = &text[2..text.len() - 1];
    decode_byte_escapes(inner)
}

pub fn decode_char(text: &str) -> char {
    let inner = &text[1..text.len() - 1];
    let decoded = decode_escapes(inner, true);
    decoded
        .into_iter()
        .next()
        .expect("lexer guarantees exactly one char literal codepoint")
}

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
                chars.next();
                let mut hex = String::new();
                for h in chars.by_ref() {
                    if h == '}' {
                        break;
                    }
                    hex.push(h);
                }
                let cp = u32::from_str_radix(&hex, 16)
                    .expect("lexer validated `\\u{...}` as one to six hex digits");
                out.push(
                    char::from_u32(cp).expect("lexer validated `\\u{...}` as a Unicode scalar"),
                );
            }
            Some(other) => out.push(other),
            None => {}
        }
    }
    out
}

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
        assert_eq!(
            eval_wrapping(BinOp::AddW, &Type::U8, &Value::U8(250), &Value::U8(10)),
            Ok(Value::U8(4))
        );
    }

    #[test]
    fn wrapping_add_wraps_at_i8_max_to_min() {
        assert_eq!(
            eval_wrapping(BinOp::AddW, &Type::I8, &Value::I8(i8::MAX), &Value::I8(1)),
            Ok(Value::I8(i8::MIN))
        );
    }

    #[test]
    fn division_truncates_toward_zero_for_negative_operands() {
        assert_eq!(
            eval_div_rem(BinOp::Div, &Type::I32, &Value::I32(-7), &Value::I32(2)),
            Ok(Value::I32(-3))
        );
    }

    #[test]
    fn remainder_matches_truncating_division_sign() {
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
        assert_eq!(
            eval_shift(BinOp::Shl, &Type::U8, &Value::U8(1), &Value::U8(7)),
            Ok(Value::U8(128))
        );
    }

    #[test]
    fn shift_left_abandons_on_lost_bits() {
        assert_eq!(
            eval_shift(BinOp::Shl, &Type::U8, &Value::U8(0xFF), &Value::U8(1)),
            Err("`<<` lost nonzero high bits".to_string())
        );
    }

    #[test]
    fn shift_abandons_on_out_of_range_count() {
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
            Ok(Value::U8(0b0010))
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
