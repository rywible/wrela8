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
    let half_pi = 1.5707963267948966_f32;
    let mut x = angle % two_pi;
    if x > pi {
        x -= two_pi;
    }
    if x < -pi {
        x += two_pi;
    }
    // Fold into the polynomial's monotone core. Besides reducing error, this
    // makes both sides of every modulo boundary meet at polynomial(0)=0;
    // the authoritative source operation has no jump at odd multiples of π.
    if x > half_pi {
        x = pi - x;
    }
    if x < -half_pi {
        x = -pi - x;
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

/// Absolute value/derivative multipliers for the pinned folded degree-11
/// source polynomial, including a source-f32 operation envelope.
///
/// # Derivation (revision 2)
///
/// `source_sin` folds its argument onto `|x| <= half_pi` — the fold is
/// `x -> x`, `x -> x -/+ two_pi` and `x -> +/-pi - x`, all of which have
/// derivative `+/-1` exactly — and then evaluates
/// `p(x) = x * (1 + x^2 * P(x^2))` with the pinned f32 Taylor coefficients.
/// So every multiplier below is a bound on `sup |p^(k)|` over the complete
/// folded domain, for `k = 0..3`.
///
/// For the exact sine all four suprema are exactly `1`. The pinned
/// polynomial uses f32-rounded Taylor coefficients, so its suprema sit a few
/// parts in `1e6` above that. `folded_source_polynomial_derivative_sups`
/// bounds them by rigorous subdivision (midpoint value plus a per-cell
/// Lipschitz correction, rounded outward); at `2^18` cells it returns within
/// `1e-4` above these converged values:
///
/// | k | sup \|p^(k)\| |
/// |---|---------------|
/// | 0 | 1.000001899…  |
/// | 1 | 1.000000000…  |
/// | 2 | 1.000005527…  |
/// | 3 | 1.000000029…  |
///
/// On top of that, `source_sin` evaluates `p` in f32: the schedule is one
/// square, five multiply/add pairs and one final multiply — twelve rounded
/// operations, each on an intermediate of magnitude below `4`. That bounds
/// the evaluation envelope by `12 * 2^-24 * 4 < 4e-6`, which is an
/// *absolute* term, not a factor: f32 relative error is `~6e-8`, so a
/// multiplicative factor of `4` (revision 1) over-stated it by six orders of
/// magnitude.
///
/// Rounding in the *fold* does not enter any of these suprema. The fold only
/// decides which `x` the polynomial is evaluated at, and every supremum
/// above is taken over the complete folded domain, so it holds for whichever
/// `x` the fold lands on. The fold's own chain-rule factor is `+/-1` because
/// each branch is affine. What the fold's rounding does perturb is
/// *accuracy*, and that is carried by `SIN_COS_APPROXIMATION_REMAINDER_V1`.
///
/// One honest caveat, unchanged from revision 1: `source_sin` is an f32
/// staircase, and a staircase has no bounded Lipschitz constant relative to
/// its smooth model (its local slope over one ulp grows without bound as the
/// step shrinks), let alone a second or third derivative. These multipliers
/// — like `hessian_norm` and `third_derivative_norm` throughout
/// `derivative_bounds` — bound the smooth model, with the value-level gap to
/// the sealed evaluation carried separately. No finite revision-1 factor
/// made the staircase reading work either, so nothing is given up here.
///
/// The pinned multiplier is therefore `1 + 2^-10` (exactly representable in
/// f32), which dominates `1.0000056 + 4e-6` with more than 400x of headroom
/// while being 4x/8x/32x/128x tighter than revision 1. `deform.rs` adds
/// `SIN_COS_APPROXIMATION_REMAINDER_V1` on top of `bound * factor`, and that
/// is what carries the difference between these source-f32 semantics and the
/// guest's sealed minimax evaluation; it is not what these factors are for.
///
/// `pinned_source_trig_factors_dominate_the_derived_suprema` in
/// `derivative_bounds` asserts the pinned values against the derivation.
pub const SOURCE_TRIG_VALUE_FACTOR_V2: f32 = 1.000_976_562_5;
pub const SOURCE_TRIG_GRADIENT_FACTOR_V2: f32 = 1.000_976_562_5;
pub const SOURCE_TRIG_HESSIAN_FACTOR_V2: f32 = 1.000_976_562_5;
pub const SOURCE_TRIG_THIRD_FACTOR_V2: f32 = 1.000_976_562_5;

/// Rigorous suprema of `|p|`, `|p'|`, `|p''|` and `|p'''|` for the pinned
/// folded degree-11 source polynomial over its complete folded domain
/// `|x| <= half_pi`.
///
/// Each cell contributes `|p^(k)(m)| + L * r`, where `m` is the cell
/// midpoint, `r` its radius and `L` a triangle-inequality bound on the next
/// derivative over the cell. That is a valid upper bound on every cell for
/// every point in it, so the maximum over a partition of the domain is a
/// valid upper bound on the whole domain — no sampling assumption is made.
/// A final outward pad absorbs the f64 rounding of the evaluation itself.
pub fn folded_source_polynomial_derivative_sups() -> [f64; 4] {
    // p(x) = x + c3 x^3 + c5 x^5 + c7 x^7 + c9 x^9 + c11 x^11, with the
    // coefficients exactly as `source_sin` pins them (f32 literals widened).
    let mut coefficients = [0.0_f64; 12];
    coefficients[1] = 1.0;
    coefficients[3] = f64::from(-0.16666666666666666_f32);
    coefficients[5] = f64::from(0.008333333333333333_f32);
    coefficients[7] = f64::from(-0.0001984126984126984_f32);
    coefficients[9] = f64::from(0.0000027557319223985893_f32);
    coefficients[11] = f64::from(-0.00000002505210838544172_f32);

    let differentiate = |source: &[f64; 12]| {
        let mut result = [0.0_f64; 12];
        for degree in 1..12 {
            result[degree - 1] = source[degree] * degree as f64;
        }
        result
    };
    let first = differentiate(&coefficients);
    let second = differentiate(&first);
    let third = differentiate(&second);

    let half_pi = f64::from(1.5707963267948966_f32);
    // 2^18 cells keep the per-cell Lipschitz slack near 3e-5, three orders of
    // magnitude below the headroom the pinned factor carries.
    const CELLS: usize = 1 << 18;
    let supremum = |polynomial: &[f64; 12]| {
        let mut best = 0.0_f64;
        for cell in 0..CELLS {
            let lo = -half_pi + 2.0 * half_pi * (cell as f64) / (CELLS as f64);
            let hi = -half_pi + 2.0 * half_pi * ((cell + 1) as f64) / (CELLS as f64);
            let midpoint = 0.5 * (lo + hi);
            let radius = 0.5 * (hi - lo);
            let extent = lo.abs().max(hi.abs());
            let mut value = 0.0_f64;
            for degree in (0..12).rev() {
                value = value * midpoint + polynomial[degree];
            }
            let mut lipschitz = 0.0_f64;
            for degree in 1..12 {
                lipschitz +=
                    polynomial[degree].abs() * degree as f64 * extent.powi(degree as i32 - 1);
            }
            best = best.max(value.abs() + lipschitz * radius);
        }
        super::reference::interval::next_up(best + 1.0e-9)
    };

    [
        supremum(&coefficients),
        supremum(&first),
        supremum(&second),
        supremum(&third),
    ]
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
