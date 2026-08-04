//! Exact symbolic scalar graph.

use super::arena::Arena;
use super::ids::{ParamId, ScalarId};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ScalarIntrinsic {
    Sqrt,
    Rsqrt,
    Sin,
    Cos,
    Dot3,
    Cross3,
    Length2,
    Length3,
    Normalize3,
}

pub fn classify_intrinsic(name: &str) -> Option<ScalarIntrinsic> {
    Some(match name {
        "sqrt_scalar" => ScalarIntrinsic::Sqrt,
        "rsqrt_scalar" => ScalarIntrinsic::Rsqrt,
        "sin_scalar" => ScalarIntrinsic::Sin,
        "cos_scalar" => ScalarIntrinsic::Cos,
        "dot3" => ScalarIntrinsic::Dot3,
        "cross3" => ScalarIntrinsic::Cross3,
        "length2" => ScalarIntrinsic::Length2,
        "length3" => ScalarIntrinsic::Length3,
        "normalize3" => ScalarIntrinsic::Normalize3,
        _ => return None,
    })
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Dependency {
    Constant,
    Coordinate,
    Parameter,
    Surface,
    CoordinateAndParameter,
    CoordinateAndSurface,
    ParameterAndSurface,
    CoordinateParameterAndSurface,
}

impl Dependency {
    pub fn combine(self, other: Self) -> Self {
        let bits = dependency_bits(self) | dependency_bits(other);
        match bits {
            0 => Self::Constant,
            0b001 => Self::Coordinate,
            0b010 => Self::Parameter,
            0b100 => Self::Surface,
            0b011 => Self::CoordinateAndParameter,
            0b101 => Self::CoordinateAndSurface,
            0b110 => Self::ParameterAndSurface,
            0b111 => Self::CoordinateParameterAndSurface,
            _ => unreachable!("closed dependency bit set"),
        }
    }
}

fn dependency_bits(dependency: Dependency) -> u8 {
    match dependency {
        Dependency::Constant => 0,
        Dependency::Coordinate => 0b001,
        Dependency::Parameter => 0b010,
        Dependency::Surface => 0b100,
        Dependency::CoordinateAndParameter => 0b011,
        Dependency::CoordinateAndSurface => 0b101,
        Dependency::ParameterAndSurface => 0b110,
        Dependency::CoordinateParameterAndSurface => 0b111,
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum CompareOp {
    Lt,
    Le,
    Gt,
    Ge,
    Eq,
    Ne,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum SemanticOpId {
    SqrtF32V1,
    RsqrtF32V1,
    SinRestrictedF32V1,
    CosRestrictedF32V1,
    Normalize3F32V1,
    SmoothMinF32V1,
    FiniteColorF32V1,
    MaterialRoughnessF32V1,
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum ScalarOp {
    ConstF32(u32),
    ConstF64(u64),
    CoordX,
    CoordY,
    CoordZ,
    SurfacePosition(u8),
    SurfaceNormal(u8),
    Param(ParamId),
    Add(ScalarId, ScalarId),
    Sub(ScalarId, ScalarId),
    Mul(ScalarId, ScalarId),
    Div(ScalarId, ScalarId),
    Neg(ScalarId),
    Abs(ScalarId),
    Min(ScalarId, ScalarId),
    Max(ScalarId, ScalarId),
    Clamp {
        value: ScalarId,
        lo: ScalarId,
        hi: ScalarId,
    },
    Sqrt(ScalarId, SemanticOpId),
    Rsqrt(ScalarId, SemanticOpId),
    SinRestricted(ScalarId, SemanticOpId),
    CosRestricted(ScalarId, SemanticOpId),
    Dot3([ScalarId; 3], [ScalarId; 3]),
    Cross3Component {
        component: u8,
        a: [ScalarId; 3],
        b: [ScalarId; 3],
    },
    Length2([ScalarId; 2]),
    Length3([ScalarId; 3]),
    Normalize3Component {
        component: u8,
        value: [ScalarId; 3],
        semantic: SemanticOpId,
    },
    Compare {
        op: CompareOp,
        a: ScalarId,
        b: ScalarId,
    },
    Select {
        predicate: ScalarId,
        a: ScalarId,
        b: ScalarId,
    },
    SelectIndex {
        index: ScalarId,
        options: Vec<ScalarId>,
    },
    SmoothMin {
        a: ScalarId,
        b: ScalarId,
        k: ScalarId,
        semantic: SemanticOpId,
    },
    FiniteOr {
        value: ScalarId,
        fallback: ScalarId,
        semantic: SemanticOpId,
    },
    MaterialRoughness {
        value: ScalarId,
        semantic: SemanticOpId,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum ProofObligation {
    DenominatorNonZero {
        denominator: ScalarId,
    },
    /// The source division is evaluated only when `predicate` is true.
    GuardedDenominatorNonZero {
        denominator: ScalarId,
        predicate: ScalarId,
    },
    RestrictedTrigDomain {
        argument: ScalarId,
    },
    DynamicIndexInBounds {
        index: ScalarId,
        extent: u32,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ScalarNode {
    pub op: ScalarOp,
    pub dependency: Dependency,
}

pub type ScalarArena = Arena<ScalarId, ScalarNode>;

pub fn constant_bits(node: &ScalarNode) -> Option<u32> {
    match node.op {
        ScalarOp::ConstF32(bits) => Some(bits),
        _ => None,
    }
}

pub fn constant_value(arena: &ScalarArena, id: ScalarId) -> Option<f32> {
    constant_bits(arena.get(id).ok()?).map(f32::from_bits)
}

pub fn fold_constant(arena: &ScalarArena, op: &ScalarOp) -> Option<ScalarOp> {
    let constant = |value: f32| Some(ScalarOp::ConstF32(value.to_bits()));
    let pair = |a, b| Some((constant_value(arena, a)?, constant_value(arena, b)?));
    if let ScalarOp::SelectIndex { index, options } = op {
        let index = constant_value(arena, *index)?;
        if !index.is_finite() || index < 0.0 || index.fract() != 0.0 {
            return None;
        }
        return constant(constant_value(arena, *options.get(index as usize)?)?);
    }
    match *op {
        ScalarOp::Add(a, b) => {
            let (a, b) = pair(a, b)?;
            constant(a + b)
        }
        ScalarOp::Sub(a, b) => {
            let (a, b) = pair(a, b)?;
            constant(a - b)
        }
        ScalarOp::Mul(a, b) => {
            let (a, b) = pair(a, b)?;
            constant(a * b)
        }
        ScalarOp::Div(a, b) => {
            let (a, b) = pair(a, b)?;
            constant(a / b)
        }
        ScalarOp::Neg(value) => constant(-constant_value(arena, value)?),
        ScalarOp::Abs(value) => {
            let value = constant_value(arena, value)?;
            constant(if value < 0.0 { -value } else { value })
        }
        ScalarOp::Min(a, b) => {
            let (a, b) = pair(a, b)?;
            constant(source_min(a, b))
        }
        ScalarOp::Max(a, b) => {
            let (a, b) = pair(a, b)?;
            constant(source_max(a, b))
        }
        ScalarOp::Clamp { value, lo, hi } => {
            let value = constant_value(arena, value)?;
            let lo = constant_value(arena, lo)?;
            let hi = constant_value(arena, hi)?;
            constant(source_min(source_max(value, lo), hi))
        }
        ScalarOp::Sqrt(value, SemanticOpId::SqrtF32V1) => {
            constant(source_sqrt(constant_value(arena, value)?))
        }
        ScalarOp::Rsqrt(value, SemanticOpId::RsqrtF32V1) => {
            constant(source_rsqrt(constant_value(arena, value)?))
        }
        ScalarOp::SinRestricted(value, SemanticOpId::SinRestrictedF32V1) => {
            constant(source_sin(constant_value(arena, value)?))
        }
        ScalarOp::CosRestricted(value, SemanticOpId::CosRestrictedF32V1) => {
            constant(source_cos(constant_value(arena, value)?))
        }
        ScalarOp::Dot3(a, b) => {
            let a = a
                .map(|id| constant_value(arena, id))
                .into_iter()
                .collect::<Option<Vec<_>>>()?;
            let b = b
                .map(|id| constant_value(arena, id))
                .into_iter()
                .collect::<Option<Vec<_>>>()?;
            let xy = a[0] * b[0] + a[1] * b[1];
            constant(xy + a[2] * b[2])
        }
        ScalarOp::Cross3Component { component, a, b } => {
            let a = a
                .map(|id| constant_value(arena, id))
                .into_iter()
                .collect::<Option<Vec<_>>>()?;
            let b = b
                .map(|id| constant_value(arena, id))
                .into_iter()
                .collect::<Option<Vec<_>>>()?;
            let value = match component {
                0 => a[1] * b[2] - a[2] * b[1],
                1 => a[2] * b[0] - a[0] * b[2],
                2 => a[0] * b[1] - a[1] * b[0],
                _ => return None,
            };
            constant(value)
        }
        ScalarOp::Length2(value) => {
            let [x, y] = value.map(|id| constant_value(arena, id));
            constant(source_sqrt(x? * x? + y? * y?))
        }
        ScalarOp::Length3(value) => {
            let [x, y, z] = value.map(|id| constant_value(arena, id));
            let xy = x? * x? + y? * y?;
            constant(source_sqrt(xy + z? * z?))
        }
        ScalarOp::Normalize3Component {
            component,
            value,
            semantic: SemanticOpId::Normalize3F32V1,
        } => {
            let values = value
                .map(|id| constant_value(arena, id))
                .into_iter()
                .collect::<Option<Vec<_>>>()?;
            let xy = values[0] * values[0] + values[1] * values[1];
            let length = source_sqrt(xy + values[2] * values[2]);
            let value = if length <= 0.0 {
                0.0
            } else {
                *values.get(component as usize)? / length
            };
            constant(value)
        }
        ScalarOp::Compare { op, a, b } => {
            let (a, b) = pair(a, b)?;
            let value = match op {
                CompareOp::Lt => a < b,
                CompareOp::Le => a <= b,
                CompareOp::Gt => a > b,
                CompareOp::Ge => a >= b,
                CompareOp::Eq => a == b,
                CompareOp::Ne => a != b,
            };
            constant(if value { 1.0 } else { 0.0 })
        }
        ScalarOp::Select { predicate, a, b } => {
            let predicate = constant_value(arena, predicate)?;
            constant(constant_value(arena, if predicate != 0.0 { a } else { b })?)
        }
        ScalarOp::SmoothMin { a, b, k, .. } => constant(source_smooth_min(
            constant_value(arena, a)?,
            constant_value(arena, b)?,
            constant_value(arena, k)?,
        )),
        ScalarOp::FiniteOr {
            value, fallback, ..
        } => {
            let value = constant_value(arena, value)?;
            let fallback = constant_value(arena, fallback)?;
            constant(if value.is_nan() { fallback } else { value })
        }
        ScalarOp::MaterialRoughness {
            value,
            semantic: SemanticOpId::MaterialRoughnessF32V1,
        } => constant(source_material_roughness(constant_value(arena, value)?)),
        _ => None,
    }
}

pub fn source_material_roughness(mut value: f32) -> f32 {
    if value.is_nan() {
        value = 0.0;
    }
    if value < 0.0 {
        value = 0.0;
    }
    if value > 1.0 {
        value = 1.0;
    }
    value
}

pub fn source_min(a: f32, b: f32) -> f32 {
    if a < b { a } else { b }
}

pub fn source_max(a: f32, b: f32) -> f32 {
    if a > b { a } else { b }
}

pub fn source_sqrt(x: f32) -> f32 {
    if x <= 0.0 {
        return 0.0;
    }
    if x > f32::MAX {
        return x;
    }
    let mut guess = if x < 1.0 { 1.0 } else { x };
    for _ in 0..128 {
        let quotient = x / guess;
        let sum = guess + quotient;
        guess = 0.5 * sum;
    }
    guess
}

pub fn source_rsqrt(x: f32) -> f32 {
    let root = source_sqrt(x);
    if root <= 0.0 { 0.0 } else { 1.0 / root }
}

pub fn source_sin(angle: f32) -> f32 {
    let two_pi = 6.283185307179586_f32;
    let pi = 3.141592653589793_f32;
    let mut x = angle % two_pi;
    if x > pi {
        x -= two_pi;
    }
    if x < -pi {
        x += two_pi;
    }
    let x2 = x * x;
    let c3 = -0.16666666666666666_f32;
    let c5 = 0.008333333333333333_f32;
    let c7 = -0.0001984126984126984_f32;
    let c9 = 0.0000027557319223985893_f32;
    let c11 = -0.00000002505210838544172_f32;
    let polynomial = c9 + x2 * c11;
    let polynomial = c7 + x2 * polynomial;
    let polynomial = c5 + x2 * polynomial;
    let polynomial = c3 + x2 * polynomial;
    x * (1.0 + x2 * polynomial)
}

pub fn source_cos(angle: f32) -> f32 {
    source_sin(angle + 1.5707963267948966_f32)
}

pub fn source_smooth_min(a: f32, b: f32, k: f32) -> f32 {
    if a <= b - k {
        a
    } else if b <= a - k {
        b
    } else {
        let h = 0.5 + 0.5 * (b - a) / k;
        b + (a - b) * h - k * h * (1.0 - h)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pixels::arena::NodeOrigin;

    fn constants(values: &[f32]) -> (ScalarArena, Vec<ScalarId>) {
        let mut arena = ScalarArena::new(1);
        let ids = values
            .iter()
            .map(|value| {
                arena
                    .push(
                        ScalarNode {
                            op: ScalarOp::ConstF32(value.to_bits()),
                            dependency: Dependency::Constant,
                        },
                        NodeOrigin::synthetic("constant-fold-test"),
                    )
                    .unwrap()
            })
            .collect();
        (arena, ids)
    }

    fn folded_bits(arena: &ScalarArena, op: ScalarOp) -> u32 {
        match fold_constant(arena, &op).expect("constant operation folds") {
            ScalarOp::ConstF32(bits) => bits,
            other => panic!("constant operation folded to {other:?}"),
        }
    }

    #[test]
    fn dependency_union_retains_every_input_domain() {
        assert_eq!(
            Dependency::Surface.combine(Dependency::Parameter),
            Dependency::ParameterAndSurface
        );
        assert_eq!(
            Dependency::CoordinateAndParameter.combine(Dependency::Surface),
            Dependency::CoordinateParameterAndSurface
        );
        assert_eq!(
            Dependency::Constant.combine(Dependency::Surface),
            Dependency::Surface
        );
    }

    #[test]
    fn scalar_intrinsic_surface_is_closed_and_complete() {
        for name in [
            "sqrt_scalar",
            "rsqrt_scalar",
            "sin_scalar",
            "cos_scalar",
            "dot3",
            "cross3",
            "length2",
            "length3",
            "normalize3",
        ] {
            assert!(classify_intrinsic(name).is_some(), "missing `{name}`");
        }
        assert!(classify_intrinsic("texture_noise").is_none());
    }

    #[test]
    fn semantic_intrinsics_fold_to_the_exact_source_operation_order() {
        let (arena, ids) = constants(&[4.0, 0.5, 1.0, 2.0, 3.0, 4.0, 5.0, 6.0]);
        let [four, half, one, two, three, four_b, five, six] = ids.as_slice() else {
            unreachable!()
        };
        let cases = [
            (
                ScalarOp::Sqrt(*four, SemanticOpId::SqrtF32V1),
                source_sqrt(4.0),
            ),
            (
                ScalarOp::Rsqrt(*four, SemanticOpId::RsqrtF32V1),
                source_rsqrt(4.0),
            ),
            (
                ScalarOp::SinRestricted(*half, SemanticOpId::SinRestrictedF32V1),
                source_sin(0.5),
            ),
            (
                ScalarOp::CosRestricted(*half, SemanticOpId::CosRestrictedF32V1),
                source_cos(0.5),
            ),
            (
                ScalarOp::Dot3([*one, *two, *three], [*four_b, *five, *six]),
                32.0,
            ),
            (
                ScalarOp::Cross3Component {
                    component: 1,
                    a: [*one, *two, *three],
                    b: [*four_b, *five, *six],
                },
                6.0,
            ),
            (ScalarOp::Length2([*three, *four_b]), 5.0),
            (ScalarOp::Length3([*two, *three, *six]), source_sqrt(49.0)),
            (
                ScalarOp::Normalize3Component {
                    component: 1,
                    value: [*three, *four_b, *four],
                    semantic: SemanticOpId::Normalize3F32V1,
                },
                4.0 / source_sqrt(41.0),
            ),
            (
                ScalarOp::SelectIndex {
                    index: *two,
                    options: vec![*three, *four_b, *five],
                },
                5.0,
            ),
        ];
        for (op, expected) in cases {
            assert_eq!(folded_bits(&arena, op), expected.to_bits());
        }
    }

    #[test]
    fn guarded_intrinsics_are_total_at_zero_and_for_negative_inputs() {
        let (arena, ids) = constants(&[0.0, -1.0, -3.0]);
        let [zero, negative_one, negative_three] = ids.as_slice() else {
            unreachable!()
        };
        for value in [*zero, *negative_one] {
            assert_eq!(
                folded_bits(&arena, ScalarOp::Rsqrt(value, SemanticOpId::RsqrtF32V1)),
                0.0_f32.to_bits()
            );
        }
        for component in 0..3 {
            assert_eq!(
                folded_bits(
                    &arena,
                    ScalarOp::Normalize3Component {
                        component,
                        value: [*zero; 3],
                        semantic: SemanticOpId::Normalize3F32V1,
                    }
                ),
                0.0_f32.to_bits()
            );
        }
        assert_eq!(
            folded_bits(
                &arena,
                ScalarOp::Normalize3Component {
                    component: 0,
                    value: [*negative_three, *zero, *zero],
                    semantic: SemanticOpId::Normalize3F32V1,
                }
            ),
            (-1.0_f32).to_bits()
        );
    }
}
