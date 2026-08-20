//! Conservative scalar value intervals for the complete renderer domain.

use std::collections::{BTreeMap, BTreeSet};

use super::config::RendererConfig;
use super::ids::{ParamId, ScalarId};
use super::reference::interval::F64Interval;
use super::scalar::{CompareOp, ScalarOp};
use super::symbolic::{PendingObligation, SymbolicGraph};

#[derive(Clone, Debug, PartialEq)]
pub struct ScalarBound {
    pub value: F64Interval,
    pub rule: &'static str,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ValueBounds {
    pub scalar: BTreeMap<ScalarId, ScalarBound>,
}

impl ValueBounds {
    pub fn get(&self, id: ScalarId) -> Result<F64Interval, String> {
        self.scalar
            .get(&id)
            .map(|bound| bound.value)
            .ok_or_else(|| format!("pixels::bounds: missing predecessor bound for {id}"))
    }
}

fn vector_length(values: &[F64Interval]) -> Result<F64Interval, String> {
    let mut sum = F64Interval::point(0.0)?;
    for value in values {
        sum = sum.add_f32(value.square_f32()?)?;
    }
    sum.sqrt_source_f32()
}

fn normalize_component(
    numerator: F64Interval,
    length: F64Interval,
) -> Result<(F64Interval, &'static str), String> {
    if length.lo > 0.0 {
        Ok((numerator.div_f32(length)?, "normalize3-nonzero"))
    } else {
        // Once the length interval contains zero, interval analysis has lost
        // the correlation between the numerator and the rounded subnormal
        // denominator. A component can exceed one substantially, so no small
        // unit-vector envelope is justified. Keep the analysis finite but
        // otherwise fail-closed.
        Ok((
            F64Interval::new(-f64::from(f32::MAX), f64::from(f32::MAX))?,
            "normalize3-zero-containing-finite",
        ))
    }
}

fn guarded_denominators(graph: &SymbolicGraph) -> BTreeSet<ScalarId> {
    graph
        .obligations
        .iter()
        .filter_map(|obligation| match obligation {
            PendingObligation::Scalar(
                super::scalar::ProofObligation::GuardedDenominatorNonZero { denominator, .. },
            ) => Some(*denominator),
            _ => None,
        })
        .collect()
}

pub fn propagate(graph: &SymbolicGraph, config: &RendererConfig) -> Result<ValueBounds, String> {
    let params = graph
        .params
        .iter()
        .map(|param| {
            F64Interval::new(param.range_min, param.range_max)
                .map(|range| (param.id, range))
                .map_err(|error| format!("pixels::bounds: parameter {}: {error}", param.spelling))
        })
        .collect::<Result<BTreeMap<ParamId, F64Interval>, _>>()?;
    let world = [
        F64Interval::new(config.world_min.x.into(), config.world_max.x.into())?,
        F64Interval::new(config.world_min.y.into(), config.world_max.y.into())?,
        F64Interval::new(config.world_min.z.into(), config.world_max.z.into())?,
    ];
    let guarded = guarded_denominators(graph);
    let mut result = ValueBounds {
        scalar: BTreeMap::new(),
    };
    for (id, node) in graph.scalar.iter() {
        let get = |id| result.get(id);
        let computed = (|| -> Result<(F64Interval, &'static str), String> {
            Ok(match &node.op {
                ScalarOp::ConstF32(bits) => (
                    F64Interval::point(f32::from_bits(*bits).into())?,
                    "const-f32",
                ),
                ScalarOp::ConstF64(bits) => {
                    (F64Interval::point(f64::from_bits(*bits))?, "const-f64")
                }
                ScalarOp::CoordX => (world[0], "world-x"),
                ScalarOp::CoordY => (world[1], "world-y"),
                ScalarOp::CoordZ => (world[2], "world-z"),
                ScalarOp::SurfacePosition(component) => (
                    *world.get(*component as usize).ok_or_else(|| {
                        format!("pixels::bounds: invalid surface component {component}")
                    })?,
                    "surface-position",
                ),
                ScalarOp::SurfaceNormal(component) => {
                    if *component >= 3 {
                        return Err(format!(
                            "pixels::bounds: invalid surface normal component {component}"
                        ));
                    }
                    (F64Interval::new(-1.0, 1.0)?, "surface-normal")
                }
                ScalarOp::Param(param) => (
                    *params.get(param).ok_or_else(|| {
                        format!("pixels::bounds: missing parameter record {param}")
                    })?,
                    "parameter-range",
                ),
                ScalarOp::Add(a, b) => (get(*a)?.add_f32(get(*b)?)?, "add-f32"),
                ScalarOp::Sub(a, b) => (get(*a)?.sub_f32(get(*b)?)?, "sub-f32"),
                ScalarOp::Mul(a, b) if a == b => (get(*a)?.square_f32()?, "square-f32"),
                ScalarOp::Mul(a, b) => (get(*a)?.mul_f32(get(*b)?)?, "mul-f32"),
                ScalarOp::Div(a, b) => {
                    let denominator = get(*b)?;
                    if denominator.contains_zero() && guarded.contains(b) {
                        (
                            F64Interval::new(-(f32::MAX as f64), f32::MAX as f64)?,
                            "guarded-div",
                        )
                    } else if denominator.contains_zero() {
                        return Err(format!(
                            "P004: field operation `division` is not available in `AaaByteExact`: denominator {b} may reach zero over {denominator:?}"
                        ));
                    } else {
                        (get(*a)?.div_f32(denominator)?, "div-f32")
                    }
                }
                ScalarOp::Neg(value) => (get(*value)?.neg(), "neg"),
                ScalarOp::Abs(value) => (get(*value)?.abs_f32()?, "abs-f32"),
                ScalarOp::Min(a, b) => (get(*a)?.min_f32(get(*b)?)?, "min-f32"),
                ScalarOp::Max(a, b) => (get(*a)?.max_f32(get(*b)?)?, "max-f32"),
                ScalarOp::Clamp { value, lo, hi } => {
                    (get(*value)?.clamp_f32(get(*lo)?, get(*hi)?)?, "clamp-f32")
                }
                ScalarOp::Sqrt(value, _) => (get(*value)?.sqrt_source_f32()?, "sqrt-source-f32"),
                ScalarOp::Rsqrt(value, _) => (
                    get(*value)?.rsqrt_source_f32()?,
                    "reciprocal-sqrt-source-f32",
                ),
                ScalarOp::SinRestricted(value, _) => {
                    (get(*value)?.sin_source_f32()?, "restricted-sin-source-f32")
                }
                ScalarOp::CosRestricted(value, _) => {
                    (get(*value)?.cos_source_f32()?, "restricted-cos-source-f32")
                }
                ScalarOp::Dot3(a, b) => {
                    let mut sum = F64Interval::point(0.0)?;
                    for component in 0..3 {
                        sum = sum.add_f32(get(a[component])?.mul_f32(get(b[component])?)?)?;
                    }
                    (sum, "dot3-f32")
                }
                ScalarOp::Cross3Component { component, a, b } => {
                    let (j, k) = match component {
                        0 => (1, 2),
                        1 => (2, 0),
                        2 => (0, 1),
                        _ => {
                            return Err(format!(
                                "pixels::bounds: invalid cross component {component}"
                            ));
                        }
                    };
                    (
                        get(a[j])?
                            .mul_f32(get(b[k])?)?
                            .sub_f32(get(a[k])?.mul_f32(get(b[j])?)?)?,
                        "cross3-f32",
                    )
                }
                ScalarOp::Length2(values) => (
                    vector_length(&[get(values[0])?, get(values[1])?])?,
                    "length2",
                ),
                ScalarOp::Length3(values) => (
                    vector_length(&[get(values[0])?, get(values[1])?, get(values[2])?])?,
                    "length3",
                ),
                ScalarOp::Normalize3Component {
                    component, value, ..
                } => {
                    if *component >= 3 {
                        return Err(format!(
                            "pixels::bounds: invalid normalize component {component}"
                        ));
                    }
                    let length = vector_length(&[get(value[0])?, get(value[1])?, get(value[2])?])?;
                    normalize_component(get(value[*component as usize])?, length)?
                }
                ScalarOp::Compare { op, a, b } => {
                    let a = get(*a)?;
                    let b = get(*b)?;
                    let stable_true = match op {
                        CompareOp::Lt => a.hi < b.lo,
                        CompareOp::Le => a.hi <= b.lo,
                        CompareOp::Gt => a.lo > b.hi,
                        CompareOp::Ge => a.lo >= b.hi,
                        CompareOp::Eq => a.lo == a.hi && a == b,
                        CompareOp::Ne => a.hi < b.lo || b.hi < a.lo,
                    };
                    let stable_false = match op {
                        CompareOp::Lt => a.lo >= b.hi,
                        CompareOp::Le => a.lo > b.hi,
                        CompareOp::Gt => a.hi <= b.lo,
                        CompareOp::Ge => a.hi < b.lo,
                        CompareOp::Eq => a.hi < b.lo || b.hi < a.lo,
                        CompareOp::Ne => a.lo == a.hi && a == b,
                    };
                    (
                        if stable_true {
                            F64Interval::point(1.0)?
                        } else if stable_false {
                            F64Interval::point(0.0)?
                        } else {
                            F64Interval::new(0.0, 1.0)?
                        },
                        "compare",
                    )
                }
                ScalarOp::Select { predicate, a, b } => {
                    let predicate = get(*predicate)?;
                    if predicate.lo == 0.0 && predicate.hi == 0.0 {
                        (get(*b)?, "select-stable-false")
                    } else if !predicate.contains_zero() {
                        (get(*a)?, "select-stable-true")
                    } else {
                        (get(*a)?.hull(get(*b)?), "select-union")
                    }
                }
                ScalarOp::SelectIndex { index, options } => {
                    let index = get(*index)?;
                    if index.lo == index.hi
                        && index.lo.fract() == 0.0
                        && index.lo >= 0.0
                        && index.lo < options.len() as f64
                    {
                        (get(options[index.lo as usize])?, "select-index-stable")
                    } else {
                        let mut options = options.iter();
                        let first = *options
                            .next()
                            .ok_or_else(|| "pixels::bounds: empty SelectIndex".to_string())?;
                        let mut value = get(first)?;
                        for option in options {
                            value = value.hull(get(*option)?);
                        }
                        (value, "select-index-union")
                    }
                }
                ScalarOp::SmoothMin { a, b, k, .. } => {
                    let a = get(*a)?;
                    let b = get(*b)?;
                    let k = get(*k)?;
                    if k.lo <= 0.0 {
                        return Err(format!(
                            "P011: smooth CSG radius `k` is not strictly positive over its declared range: {k:?}"
                        ));
                    }
                    (a.smooth_min_source_f32(b, k)?, "smooth-min-source-f32")
                }
                ScalarOp::FiniteOr {
                    value, fallback, ..
                } => (get(*value)?.hull(get(*fallback)?), "finite-or"),
                ScalarOp::MaterialRoughness { .. } => {
                    (F64Interval::new(0.0, 1.0)?, "material-roughness")
                }
            })
        })();
        let (value, rule) = computed.map_err(|error| {
            format!(
                "pixels::bounds: node {id} with operation {:?} failed for its input ranges: {error}",
                node.op
            )
        })?;
        if !value.lo.is_finite() || !value.hi.is_finite() {
            return Err(format!(
                "pixels::bounds: node {id} produced non-finite range {value:?}"
            ));
        }
        result.scalar.insert(id, ScalarBound { value, rule });
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pixels::arena::NodeOrigin;
    use crate::pixels::scalar::{Dependency, ScalarArena, ScalarNode};

    #[test]
    fn topological_scalar_pass_preserves_signed_zero_bits() {
        let mut scalar = ScalarArena::new(1);
        scalar
            .push(
                ScalarNode {
                    op: ScalarOp::ConstF32((-0.0_f32).to_bits()),
                    dependency: Dependency::Constant,
                },
                NodeOrigin::synthetic("test"),
            )
            .unwrap();
        let graph = SymbolicGraph {
            renderer_index: 0,
            field_key: String::new(),
            material_key: String::new(),
            params_type: crate::sema::types::Type::Unit,
            material_type: crate::sema::types::Type::Unit,
            params: Vec::new(),
            scalar,
            fields: super::super::graph::FieldArena::new(2),
            materials: super::super::material_graph::MaterialArena::new(3),
            field_root: super::super::ids::FieldId(0),
            material_root: super::super::ids::MaterialId(0),
            obligations: Vec::new(),
            quota: Default::default(),
        };
        let config = RendererConfig {
            declaration_index: 0,
            worker_count: 1,
            params_type: crate::sema::types::Type::Unit,
            field: String::new(),
            material: String::new(),
            material_type: crate::sema::types::Type::Unit,
            display_index: 0,
            display_doorbell_addr: wrela_machine::pixels::DOORBELL_ADDR,
            width: 1,
            height: 1,
            refresh_hz: 1,
            shade_hz: 1,
            profile: String::new(),
            tone_curve: String::new(),
            near: 0.1,
            far: 1.0,
            world_min: super::super::config::Vec3Config {
                x: -1.0,
                y: -1.0,
                z: -1.0,
            },
            world_max: super::super::config::Vec3Config {
                x: 1.0,
                y: 1.0,
                z: 1.0,
            },
            camera_pose: None,
            camera_max_motion: 0.0,
            light_capacity: 0,
            light_kinds: Vec::new(),
            light_ranges: super::super::config::default_light_ranges(),
            exposure: super::super::config::ScalarRangeConfig { min: 0.0, max: 0.0 },
            environment: super::super::config::RgbRangeConfig {
                min: [0.0; 3],
                max: [0.0; 3],
            },
            ao_enabled: false,
            ao_radius: 1.0,
            ao_strength: 1.0,
            probes_enabled: false,
            probe_initialization_worst_case_ms: 0,
            initialization_deadline_ms: 1,
            parameter_contracts: Vec::new(),
        };
        let bounds = propagate(&graph, &config).unwrap();
        let value = bounds.get(ScalarId(0)).unwrap();
        assert_eq!(value.lo.to_bits(), (-0.0_f64).to_bits());
        assert_eq!(value.hi.to_bits(), (-0.0_f64).to_bits());
    }

    #[test]
    fn fused_vector_interval_rules_contain_deterministic_random_samples() {
        let a = [
            F64Interval::new(-3.0, 2.0).unwrap(),
            F64Interval::new(-1.0, 4.0).unwrap(),
            F64Interval::new(0.25, 5.0).unwrap(),
        ];
        let b = [
            F64Interval::new(-2.0, 3.0).unwrap(),
            F64Interval::new(1.0, 2.0).unwrap(),
            F64Interval::new(-4.0, -0.5).unwrap(),
        ];
        let mut dot = F64Interval::point(0.0).unwrap();
        for component in 0..3 {
            dot = dot
                .add_outward(a[component].mul_outward(b[component]).unwrap())
                .unwrap();
        }
        let cross = [
            a[1].mul_outward(b[2])
                .unwrap()
                .sub_outward(a[2].mul_outward(b[1]).unwrap())
                .unwrap(),
            a[2].mul_outward(b[0])
                .unwrap()
                .sub_outward(a[0].mul_outward(b[2]).unwrap())
                .unwrap(),
            a[0].mul_outward(b[1])
                .unwrap()
                .sub_outward(a[1].mul_outward(b[0]).unwrap())
                .unwrap(),
        ];
        let length = vector_length(&a).unwrap();
        let normalized = [
            a[0].div_f32(length).unwrap(),
            a[1].div_f32(length).unwrap(),
            a[2].div_f32(length).unwrap(),
        ];

        let mut state = 0x517c_c1b7_2722_0a95_u64;
        for _ in 0..100_000 {
            let mut next = || {
                state ^= state << 13;
                state ^= state >> 7;
                state ^= state << 17;
                (state >> 11) as f64 / (u64::MAX >> 11) as f64
            };
            let av: [f64; 3] =
                std::array::from_fn(|component| a[component].lo + a[component].width() * next());
            let bv: [f64; 3] =
                std::array::from_fn(|component| b[component].lo + b[component].width() * next());
            let point_dot: f64 = (0..3).map(|component| av[component] * bv[component]).sum();
            let point_cross = [
                av[1] * bv[2] - av[2] * bv[1],
                av[2] * bv[0] - av[0] * bv[2],
                av[0] * bv[1] - av[1] * bv[0],
            ];
            let point_length = av.iter().map(|value| value * value).sum::<f64>().sqrt();
            assert!(dot.contains(point_dot));
            assert!((0..3).all(|component| cross[component].contains(point_cross[component])));
            assert!(length.contains(point_length));
            let av32 = av.map(|value| value as f32);
            let sum32 = av32
                .iter()
                .fold(0.0_f32, |sum, value| sum + *value * *value);
            let source_length = super::super::scalar::source_sqrt(sum32);
            let source_normalized = av32.map(|value| {
                if source_length <= 0.0 {
                    0.0
                } else {
                    value / source_length
                }
            });
            assert!((0..3).all(|component| {
                normalized[component].contains(f64::from(source_normalized[component]))
            }));
            let selected = if state & 1 == 0 { av32[0] } else { av32[1] };
            assert!(a[0].hull(a[1]).contains(f64::from(selected)));
            let selected_index = (state as usize) % 3;
            assert!(
                a.into_iter()
                    .reduce(F64Interval::hull)
                    .unwrap()
                    .contains(f64::from(av32[selected_index]))
            );
        }
    }

    #[test]
    fn normalize_bound_contains_source_value_one_ulp_above_one() {
        let x = f32::from_bits(0x3f35_04f7);
        let components = [
            F64Interval::point(f64::from(x)).unwrap(),
            F64Interval::point(0.0).unwrap(),
            F64Interval::point(0.0).unwrap(),
        ];
        let length = vector_length(&components).unwrap();
        let interval = components[0].div_f32(length).unwrap();
        let actual = x / super::super::scalar::source_sqrt(x * x);
        assert_eq!(actual.to_bits(), 0x3f80_0001);
        assert!(interval.contains(f64::from(actual)));
    }

    #[test]
    fn zero_containing_normalize_bound_contains_subnormal_rounding_amplification() {
        let x = -4.583183093948745e-23_f32;
        let y = -8.387244833356612e-28_f32;
        let z = -2.3371651277121527e-36_f32;
        let components = [
            F64Interval::new(f64::from(x), 0.0).unwrap(),
            F64Interval::new(f64::from(y), 0.0).unwrap(),
            F64Interval::new(f64::from(z), 0.0).unwrap(),
        ];
        let length = vector_length(&components).unwrap();
        assert_eq!(length.lo, 0.0);
        let (interval, rule) = normalize_component(components[0], length).unwrap();
        let squared = x * x + y * y + z * z;
        assert_eq!(squared.to_bits(), 1);
        let actual = x / super::super::scalar::source_sqrt(squared);
        assert!(actual.abs() > 1.2);
        assert!(interval.contains(f64::from(actual)));
        assert_eq!(rule, "normalize3-zero-containing-finite");
    }
}
