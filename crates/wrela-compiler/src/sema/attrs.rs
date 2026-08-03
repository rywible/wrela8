//! Typed Pixels field attributes.
//!
//! Attribute syntax is ordinary Wrela syntax, but the values recorded here are
//! deliberately small and closed. Later Pixels stages consume these values
//! without re-reading source ASTs or re-evaluating constants.

use std::collections::{BTreeMap, BTreeSet};

use crate::sema::SemaError;
use crate::sema::types::{self, Type};
use crate::syntax::ast::{self, BinOp, Expr, Span, UnaryOp};

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct NumericRange {
    pub min: f64,
    pub max: f64,
    pub integer: bool,
    /// Exact endpoints for integer-typed fields. The floating copies above are
    /// convenient for later numeric stages but are not authoritative for
    /// integer contracts because `f64` cannot represent every `i128` exactly.
    pub exact_integer: Option<(i128, i128)>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RateContract {
    pub max_delta: f64,
    pub max_second_delta: f64,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct FieldContracts {
    pub range: Option<NumericRange>,
    pub rate: Option<RateContract>,
}

fn pixels_error(message: impl Into<String>, span: Span) -> SemaError {
    let message = message.into();
    if let Some(message) = message.strip_prefix("P006: ") {
        SemaError::at("pixels P006", message.to_string(), span)
    } else if let Some(message) = message.strip_prefix("P007: ") {
        SemaError::at("pixels P007", message.to_string(), span)
    } else {
        SemaError::at("pixels", message, span)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ScalarShape {
    Integer { min: i128, max: i128 },
    F32,
    F64,
}

fn scalar_shape(ty: &Type, canonical_f32_vector: bool) -> Option<ScalarShape> {
    Some(match ty {
        Type::U8 => ScalarShape::Integer {
            min: u8::MIN.into(),
            max: u8::MAX.into(),
        },
        Type::U16 => ScalarShape::Integer {
            min: u16::MIN.into(),
            max: u16::MAX.into(),
        },
        Type::U32 => ScalarShape::Integer {
            min: u32::MIN.into(),
            max: u32::MAX.into(),
        },
        Type::U64 | Type::Usize => ScalarShape::Integer {
            min: u64::MIN.into(),
            max: u64::MAX.into(),
        },
        Type::I8 => ScalarShape::Integer {
            min: i8::MIN.into(),
            max: i8::MAX.into(),
        },
        Type::I16 => ScalarShape::Integer {
            min: i16::MIN.into(),
            max: i16::MAX.into(),
        },
        Type::I32 => ScalarShape::Integer {
            min: i32::MIN.into(),
            max: i32::MAX.into(),
        },
        Type::I64 | Type::Isize => ScalarShape::Integer {
            min: i64::MIN.into(),
            max: i64::MAX.into(),
        },
        Type::F32 => ScalarShape::F32,
        Type::F64 => ScalarShape::F64,
        Type::Named(_, args) if args.is_empty() && canonical_f32_vector => ScalarShape::F32,
        _ => return None,
    })
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum NumericConstant {
    Integer(i128),
    Float(f64),
}

impl NumericConstant {
    fn as_f64(self) -> f64 {
        match self {
            Self::Integer(value) => value as f64,
            Self::Float(value) => value,
        }
    }

    fn is_finite(self) -> bool {
        match self {
            Self::Integer(_) => true,
            Self::Float(value) => value.is_finite(),
        }
    }
}

fn numeric_constant(
    expr: &Expr,
    constants: &BTreeMap<String, Expr>,
    active: &mut BTreeSet<String>,
    code: &str,
) -> Result<NumericConstant, SemaError> {
    match expr {
        Expr::Int(span, text) => {
            let value = crate::eval::value::parse_int_literal(text).ok_or_else(|| {
                pixels_error(format!("{code}: invalid integer attribute constant"), *span)
            })?;
            Ok(NumericConstant::Integer(value))
        }
        Expr::Float(span, text) => {
            let cleaned: String = text.chars().filter(|ch| *ch != '_').collect();
            let value = cleaned.parse::<f64>().map_err(|_| {
                pixels_error(
                    format!("{code}: `{text}` is not a numeric attribute constant"),
                    *span,
                )
            })?;
            Ok(NumericConstant::Float(value))
        }
        Expr::Unary(span, UnaryOp::Neg, inner) => {
            match numeric_constant(inner, constants, active, code)? {
                NumericConstant::Integer(value) => value
                    .checked_neg()
                    .map(NumericConstant::Integer)
                    .ok_or_else(|| {
                        pixels_error(
                            format!("{code}: integer attribute constant is out of range"),
                            *span,
                        )
                    }),
                NumericConstant::Float(value) => {
                    let value = -value;
                    if !value.is_finite() {
                        return Err(pixels_error(
                            format!("{code}: numeric attribute value must be finite"),
                            *span,
                        ));
                    }
                    Ok(NumericConstant::Float(value))
                }
            }
        }
        Expr::Name(span, name) => {
            if !active.insert(name.clone()) {
                return Err(pixels_error(
                    format!("{code}: numeric attribute constant cycle reaches `{name}`"),
                    *span,
                ));
            }
            let value = constants.get(name).ok_or_else(|| {
                pixels_error(
                    format!(
                        "{code}: numeric attribute value `{name}` is not a comptime scalar constant"
                    ),
                    *span,
                )
            })?;
            let result = numeric_constant(value, constants, active, code);
            active.remove(name);
            result
        }
        Expr::Binary(span, op, left, right)
            if matches!(
                op,
                BinOp::Add | BinOp::Sub | BinOp::Mul | BinOp::Div | BinOp::Rem
            ) =>
        {
            let left = numeric_constant(left, constants, active, code)?;
            let right = numeric_constant(right, constants, active, code)?;
            match (left, right) {
                (NumericConstant::Integer(left), NumericConstant::Integer(right)) => {
                    let value = match op {
                        BinOp::Add => left.checked_add(right),
                        BinOp::Sub => left.checked_sub(right),
                        BinOp::Mul => left.checked_mul(right),
                        BinOp::Div => left.checked_div(right),
                        BinOp::Rem => left.checked_rem(right),
                        _ => unreachable!("guarded arithmetic operator"),
                    }
                    .ok_or_else(|| {
                        pixels_error(
                            format!(
                                "{code}: integer attribute constant overflows or divides by zero"
                            ),
                            *span,
                        )
                    })?;
                    Ok(NumericConstant::Integer(value))
                }
                (left, right) => {
                    let left = left.as_f64();
                    let right = right.as_f64();
                    let value = match op {
                        BinOp::Add => left + right,
                        BinOp::Sub => left - right,
                        BinOp::Mul => left * right,
                        BinOp::Div => left / right,
                        BinOp::Rem => left % right,
                        _ => unreachable!("guarded arithmetic operator"),
                    };
                    Ok(NumericConstant::Float(value))
                }
            }
        }
        _ => Err(pixels_error(
            format!("{code}: numeric attribute values must be comptime scalar constants"),
            expr.span(),
        )),
    }
}

fn attribute_numeric_constant(
    expr: &Expr,
    constants: &BTreeMap<String, Expr>,
    active: &mut BTreeSet<String>,
    code: &str,
) -> Result<NumericConstant, SemaError> {
    numeric_constant(expr, constants, active, code).map_err(|mut error| {
        error.line = expr.span().line;
        error.col = expr.span().col;
        error
    })
}

fn named_values<'a>(
    attr: &'a ast::Attr,
    required: &[&str],
) -> Result<BTreeMap<&'a str, &'a Expr>, SemaError> {
    let code = if attr.name == "rate" { "P006" } else { "P007" };
    let mut values = BTreeMap::new();
    for arg in &attr.args {
        let Some(label) = arg.label.as_deref() else {
            return Err(pixels_error(
                format!("{code}: `@{}` arguments must be labeled", attr.name),
                arg.span,
            ));
        };
        if !required.contains(&label) {
            return Err(pixels_error(
                format!("{code}: `@{}` has no `{label}=` argument", attr.name),
                arg.span,
            ));
        }
        if values.insert(label, &arg.value).is_some() {
            return Err(pixels_error(
                format!(
                    "{code}: `@{}` argument `{label}` is bound more than once",
                    attr.name
                ),
                arg.span,
            ));
        }
    }
    for label in required {
        if !values.contains_key(label) {
            return Err(pixels_error(
                format!("{code}: `@{}` requires `{label}=`", attr.name),
                attr.span,
            ));
        }
    }
    Ok(values)
}

fn one_attr<'a>(field: &'a ast::FieldItem, name: &str) -> Result<Option<&'a ast::Attr>, SemaError> {
    let attrs: Vec<&ast::Attr> = field
        .attrs
        .iter()
        .filter(|attr| attr.name == name)
        .collect();
    match attrs.as_slice() {
        [] => Ok(None),
        [attr] => Ok(Some(*attr)),
        [_, duplicate, ..] => {
            let code = if name == "rate" { "P006" } else { "P007" };
            Err(pixels_error(
                format!(
                    "{code}: field `{}` carries duplicate `@{name}` attributes",
                    field.name
                ),
                duplicate.span,
            ))
        }
    }
}

pub(crate) fn parse_field_contracts(
    field: &ast::FieldItem,
    ty: &Type,
    constants: &BTreeMap<String, Expr>,
    canonical_f32_vector: bool,
) -> Result<FieldContracts, SemaError> {
    let range_attr = one_attr(field, "range")?;
    let rate_attr = one_attr(field, "rate")?;
    if range_attr.is_none() && rate_attr.is_none() {
        return Ok(FieldContracts::default());
    }
    let Some(scalar_shape) = scalar_shape(ty, canonical_f32_vector) else {
        let code = if range_attr.is_some() { "P007" } else { "P006" };
        return Err(pixels_error(
            format!(
                "{code}: `@range`/`@rate` require a scalar or component-wise vector field, \
                 found `{}` on `{}`",
                types::render_type(ty),
                field.name
            ),
            field.span,
        ));
    };

    let range = if let Some(attr) = range_attr {
        let values = named_values(attr, &["min", "max"])?;
        let mut active = BTreeSet::new();
        let min = attribute_numeric_constant(values["min"], constants, &mut active, "P007")?;
        let max = attribute_numeric_constant(values["max"], constants, &mut active, "P007")?;
        if !min.is_finite() {
            return Err(pixels_error(
                "P007: `@range` minimum must be finite",
                values["min"].span(),
            ));
        }
        if !max.is_finite() {
            return Err(pixels_error(
                "P007: `@range` maximum must be finite",
                values["max"].span(),
            ));
        }
        let exact_integer = if let ScalarShape::Integer {
            min: type_min,
            max: type_max,
        } = scalar_shape
        {
            match (min, max) {
                (NumericConstant::Integer(min), NumericConstant::Integer(max)) => {
                    if min < type_min || max > type_max {
                        return Err(pixels_error(
                            format!(
                                "P007: integer `@range` [{min},{max}] is not representable by `{}`",
                                types::render_type(ty)
                            ),
                            attr.span,
                        ));
                    }
                    if min > max {
                        return Err(pixels_error(
                            format!("P007: `@range` has min={min} greater than max={max}"),
                            attr.span,
                        ));
                    }
                    Some((min, max))
                }
                _ => {
                    return Err(pixels_error(
                        "P007: integer `@range` endpoints must be exact integer constants",
                        attr.span,
                    ));
                }
            }
        } else {
            None
        };
        let mut min = min.as_f64();
        let mut max = max.as_f64();
        if !matches!(scalar_shape, ScalarShape::Integer { .. }) && min > max {
            return Err(pixels_error(
                format!("P007: `@range` has min={min} greater than max={max}"),
                attr.span,
            ));
        }
        if scalar_shape == ScalarShape::F32 {
            let min_f32 = min as f32;
            let max_f32 = max as f32;
            if !min_f32.is_finite() || !max_f32.is_finite() {
                return Err(pixels_error(
                    "P007: `@range` endpoint is not representable as finite `f32`",
                    attr.span,
                ));
            }
            min = f64::from(min_f32);
            max = f64::from(max_f32);
        }
        Some(NumericRange {
            min,
            max,
            integer: matches!(scalar_shape, ScalarShape::Integer { .. }),
            exact_integer,
        })
    } else {
        None
    };

    let rate = if let Some(attr) = rate_attr {
        let values = named_values(attr, &["max_delta", "max_second_delta"])?;
        let mut active = BTreeSet::new();
        let max_delta =
            attribute_numeric_constant(values["max_delta"], constants, &mut active, "P006")?
                .as_f64();
        let max_second_delta =
            attribute_numeric_constant(values["max_second_delta"], constants, &mut active, "P006")?
                .as_f64();
        if !max_delta.is_finite() {
            return Err(pixels_error(
                "P006: `@rate` max_delta must be finite",
                values["max_delta"].span(),
            ));
        }
        if !max_second_delta.is_finite() {
            return Err(pixels_error(
                "P006: `@rate` max_second_delta must be finite",
                values["max_second_delta"].span(),
            ));
        }
        if max_delta < 0.0 {
            return Err(pixels_error(
                "P006: `@rate` max_delta must be nonnegative",
                values["max_delta"].span(),
            ));
        }
        if max_second_delta < 0.0 {
            return Err(pixels_error(
                "P006: `@rate` max_second_delta must be nonnegative",
                values["max_second_delta"].span(),
            ));
        }
        let max_delta = f64::from(max_delta as f32);
        let max_second_delta = f64::from(max_second_delta as f32);
        if !max_delta.is_finite() || !max_second_delta.is_finite() {
            return Err(pixels_error(
                "P006: `@rate` value is not representable as finite `f32`",
                attr.span,
            ));
        }
        Some(RateContract {
            max_delta,
            max_second_delta,
        })
    } else {
        None
    };
    Ok(FieldContracts { range, rate })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn span() -> Span {
        Span { line: 1, col: 1 }
    }

    #[test]
    fn constant_parser_preserves_non_finite_for_the_contract_check() {
        let expr = Expr::Float(span(), "1e999".to_string());
        let value =
            numeric_constant(&expr, &BTreeMap::new(), &mut BTreeSet::new(), "P007").unwrap();
        assert!(!value.is_finite());
    }

    #[test]
    fn constant_parser_resolves_named_negative_values() {
        let mut constants = BTreeMap::new();
        constants.insert("LIMIT".to_string(), Expr::Float(span(), "2.5".to_string()));
        let expr = Expr::Unary(
            span(),
            UnaryOp::Neg,
            Box::new(Expr::Name(span(), "LIMIT".to_string())),
        );
        assert_eq!(
            numeric_constant(&expr, &constants, &mut BTreeSet::new(), "P007").unwrap(),
            NumericConstant::Float(-2.5)
        );
    }

    #[test]
    fn integer_constant_keeps_values_beyond_f64_precision() {
        let expr = Expr::Int(span(), "9007199254740993".to_string());
        assert_eq!(
            numeric_constant(&expr, &BTreeMap::new(), &mut BTreeSet::new(), "P007").unwrap(),
            NumericConstant::Integer(9_007_199_254_740_993)
        );
    }

    #[test]
    fn constant_parser_evaluates_arithmetic_and_exposes_nan() {
        let arithmetic = Expr::Binary(
            span(),
            BinOp::Add,
            Box::new(Expr::Int(span(), "2".to_string())),
            Box::new(Expr::Int(span(), "3".to_string())),
        );
        assert_eq!(
            numeric_constant(&arithmetic, &BTreeMap::new(), &mut BTreeSet::new(), "P007",).unwrap(),
            NumericConstant::Integer(5)
        );
        let nan = Expr::Binary(
            span(),
            BinOp::Div,
            Box::new(Expr::Float(span(), "0.0".to_string())),
            Box::new(Expr::Float(span(), "0.0".to_string())),
        );
        assert!(
            !numeric_constant(&nan, &BTreeMap::new(), &mut BTreeSet::new(), "P007")
                .unwrap()
                .is_finite()
        );
    }

    #[test]
    fn rate_constant_failures_use_the_rate_code() {
        let expr = Expr::Name(span(), "RUNTIME_VALUE".to_string());
        let error =
            numeric_constant(&expr, &BTreeMap::new(), &mut BTreeSet::new(), "P006").unwrap_err();
        assert_eq!(error.category, "pixels P006");
    }
}
