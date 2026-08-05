//! Symbolic derivative bundles for projective feature programs.

use std::collections::BTreeMap;

use super::ids::{
    CoeffId, DerivativeBundleId, FieldId, ObjectId, ParamId, PolyProgramId, ScalarId,
};
use super::polynomial::{Exponents, PolyBuilder, PolyProgram, ProgramLimit, Var};
use super::program::{CoeffBuilder, CoeffOp};
use super::projective::ProjectiveEquations;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FirstDerivatives {
    pub u: PolyProgramId,
    pub v: PolyProgramId,
    pub q: PolyProgramId,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SecondDerivatives {
    pub uu: PolyProgramId,
    pub uv: PolyProgramId,
    pub uq: PolyProgramId,
    pub vv: PolyProgramId,
    pub vq: PolyProgramId,
    pub qq: PolyProgramId,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ThirdDerivatives {
    pub uuu: PolyProgramId,
    pub uuv: PolyProgramId,
    pub uuq: PolyProgramId,
    pub uvv: PolyProgramId,
    pub uvq: PolyProgramId,
    pub uqq: PolyProgramId,
    pub vvv: PolyProgramId,
    pub vvq: PolyProgramId,
    pub vqq: PolyProgramId,
    pub qqq: PolyProgramId,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ParameterDerivative {
    pub parameter: ParamId,
    pub polynomial: PolyProgramId,
    pub declared_rate: Option<(u64, u64)>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DerivativeBundle {
    pub id: DerivativeBundleId,
    pub feature: super::ids::FeatureId,
    pub g: PolyProgramId,
    pub first: FirstDerivatives,
    pub second: SecondDerivatives,
    pub third: ThirdDerivatives,
    pub parameter: Vec<ParameterDerivative>,
    pub g_t: PolyProgramId,
    pub g_tt: Option<PolyProgramId>,
    pub influencing_params: Vec<ParamId>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct DerivativeClusterTemplate {
    pub object: ObjectId,
    pub leaf_signature: Vec<Vec<super::support::OccurrenceStep>>,
    pub bundles: Vec<DerivativeBundleId>,
    /// The full composed object scalar, including every active smooth blend.
    /// Primitive equations are predictors/support slabs only; this program is
    /// the authoritative zero whose local tube the runtime verifies.
    pub root_tube: SmoothRootTubeProgram,
}

#[derive(Clone, Debug, PartialEq)]
pub struct SmoothRootTubeProgram {
    pub scalar_root: ScalarId,
    pub scalar_derivative_sources: Vec<ScalarId>,
    pub value_domain: super::reference::interval::F64Interval,
    pub first_world_abs: [f64; 3],
    pub second_world_abs: f64,
    pub third_world_abs: f64,
    pub parameter_abs: Vec<(ParamId, f64)>,
    pub frame_delta_abs: Option<f64>,
    pub frame_second_delta_abs: Option<f64>,
    pub taylor_order: u8,
    pub subdivision_depth: u8,
    pub world_delta_abs_bound: f64,
    pub remainder: f64,
    pub maximum_predictor_roots: u32,
    pub maximum_object_roots: u32,
    pub requires_boundary_events: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct DerivativePrograms {
    pub bundles: Vec<DerivativeBundle>,
    pub clusters: Vec<DerivativeClusterTemplate>,
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct PolyKey {
    terms: Vec<(Exponents, CoeffId)>,
}

fn key(program: &PolyProgram) -> PolyKey {
    PolyKey {
        terms: program
            .terms
            .iter()
            .map(|term| (term.exponents, term.coefficient))
            .collect(),
    }
}

struct Compiler {
    coefficients: CoeffBuilder,
    polynomials: Vec<PolyProgram>,
    canonical_polynomials: BTreeMap<PolyKey, PolyProgramId>,
    coefficient_derivatives: BTreeMap<(CoeffId, ParamId), CoeffId>,
    coefficient_time_derivatives: BTreeMap<CoeffId, CoeffId>,
}

impl Compiler {
    fn new(equations: &ProjectiveEquations) -> Result<Self, String> {
        let canonical_polynomials = equations
            .polynomials
            .iter()
            .map(|program| (key(program), program.id))
            .collect();
        Ok(Self {
            coefficients: CoeffBuilder::from_program(equations.coefficients.clone())?,
            polynomials: equations.polynomials.clone(),
            canonical_polynomials,
            coefficient_derivatives: BTreeMap::new(),
            coefficient_time_derivatives: BTreeMap::new(),
        })
    }

    fn intern(&mut self, polynomial: PolyBuilder) -> Result<PolyProgramId, String> {
        let id = PolyProgramId(
            u32::try_from(self.polynomials.len())
                .map_err(|_| "P015: derivative program ID overflow".to_string())?,
        );
        let candidate = polynomial.finish(id, ProgramLimit::UnrestrictedIntermediate)?;
        let key = key(&candidate);
        if let Some(existing) = self.canonical_polynomials.get(&key) {
            return Ok(*existing);
        }
        self.canonical_polynomials.insert(key, id);
        self.polynomials.push(candidate);
        Ok(id)
    }

    fn derivative(
        &mut self,
        source: PolyProgramId,
        variable: Var,
    ) -> Result<PolyProgramId, String> {
        let source = self
            .polynomials
            .get(source.index())
            .cloned()
            .ok_or_else(|| format!("pixels::derivatives: missing source {source}"))?;
        let derivative =
            PolyBuilder::from_program(&source).differentiate(variable, &mut self.coefficients)?;
        self.intern(derivative)
    }

    fn coefficient_derivative(
        &mut self,
        source: CoeffId,
        parameter: ParamId,
    ) -> Result<CoeffId, String> {
        if let Some(id) = self.coefficient_derivatives.get(&(source, parameter)) {
            return Ok(*id);
        }
        let op = self.coefficients.program().get(source)?.op.clone();
        let derivative = match op {
            CoeffOp::ConstF64(_) | CoeffOp::Camera(_) | CoeffOp::ParamRate(_, _) => {
                self.coefficients.constant(0.0)?
            }
            CoeffOp::Scalar(scalar) => self
                .coefficients
                .scalar_param_derivative(scalar, parameter)?,
            CoeffOp::ScalarParamDerivative(_, _) => {
                return Err(
                    "pixels::derivatives: second parameter derivative requires a sealed scalar Hessian program"
                        .to_string(),
                );
            }
            CoeffOp::Add(a, b) => {
                let a = self.coefficient_derivative(a, parameter)?;
                let b = self.coefficient_derivative(b, parameter)?;
                self.coefficients.add(a, b)?
            }
            CoeffOp::Mul(a, b) => {
                let da = self.coefficient_derivative(a, parameter)?;
                let db = self.coefficient_derivative(b, parameter)?;
                let left = self.coefficients.mul(da, b)?;
                let right = self.coefficients.mul(a, db)?;
                self.coefficients.add(left, right)?
            }
            CoeffOp::Neg(value) => {
                let value = self.coefficient_derivative(value, parameter)?;
                self.coefficients.neg(value)?
            }
        };
        self.coefficient_derivatives
            .insert((source, parameter), derivative);
        Ok(derivative)
    }

    fn parameter_derivative(
        &mut self,
        source: PolyProgramId,
        parameter: ParamId,
    ) -> Result<PolyProgramId, String> {
        let source = self
            .polynomials
            .get(source.index())
            .cloned()
            .ok_or_else(|| format!("pixels::derivatives: missing source {source}"))?;
        let explicit = PolyBuilder::from_program(&source)
            .differentiate(Var::Param(parameter), &mut self.coefficients)?;
        let mut coefficient = PolyBuilder::zero();
        for term in &source.terms {
            let derivative = self.coefficient_derivative(term.coefficient, parameter)?;
            let term =
                PolyBuilder::monomial(derivative, term.exponents, self.coefficients.program());
            coefficient = coefficient.add(term, &mut self.coefficients)?;
        }
        let combined = explicit.add(coefficient, &mut self.coefficients)?;
        self.intern(combined)
    }

    fn coefficient_time_derivative(&mut self, source: CoeffId) -> Result<CoeffId, String> {
        if let Some(id) = self.coefficient_time_derivatives.get(&source) {
            return Ok(*id);
        }
        let op = self.coefficients.program().get(source)?.op.clone();
        let derivative = match op {
            CoeffOp::ConstF64(_)
            | CoeffOp::Scalar(_)
            | CoeffOp::ScalarParamDerivative(_, _)
            | CoeffOp::ParamRate(_, _) => self.coefficients.constant(0.0)?,
            CoeffOp::Camera(camera) => match camera.temporal_rate() {
                Some(rate) => self.coefficients.camera(rate)?,
                None => self.coefficients.constant(0.0)?,
            },
            CoeffOp::Add(a, b) => {
                let da = self.coefficient_time_derivative(a)?;
                let db = self.coefficient_time_derivative(b)?;
                self.coefficients.add(da, db)?
            }
            CoeffOp::Mul(a, b) => {
                let da = self.coefficient_time_derivative(a)?;
                let db = self.coefficient_time_derivative(b)?;
                let left = self.coefficients.mul(da, b)?;
                let right = self.coefficients.mul(a, db)?;
                self.coefficients.add(left, right)?
            }
            CoeffOp::Neg(value) => {
                let derivative = self.coefficient_time_derivative(value)?;
                self.coefficients.neg(derivative)?
            }
        };
        self.coefficient_time_derivatives.insert(source, derivative);
        Ok(derivative)
    }

    fn temporal_derivative(
        &mut self,
        source: PolyProgramId,
        parameter: &[(ParamId, PolyProgramId)],
    ) -> Result<PolyProgramId, String> {
        let mut result = PolyBuilder::zero();
        let source = self
            .polynomials
            .get(source.index())
            .cloned()
            .ok_or_else(|| format!("pixels::derivatives: missing temporal source {source}"))?;
        for term in source.terms {
            let derivative = self.coefficient_time_derivative(term.coefficient)?;
            let term =
                PolyBuilder::monomial(derivative, term.exponents, self.coefficients.program());
            result = result.add(term, &mut self.coefficients)?;
        }
        for (param, derivative) in parameter {
            let rate = self.coefficients.param_rate(*param, 1)?;
            let derivative = self
                .polynomials
                .get(derivative.index())
                .cloned()
                .ok_or_else(|| {
                    format!("pixels::derivatives: missing parameter derivative {derivative}")
                })?;
            let term =
                PolyBuilder::from_program(&derivative).scale(rate, &mut self.coefficients)?;
            result = result.add(term, &mut self.coefficients)?;
        }
        self.intern(result)
    }
}

fn declared_rate(
    layout: &super::params::ParameterLayout,
    parameter: ParamId,
) -> Result<Option<(u64, u64)>, String> {
    let slot = layout
        .slots
        .iter()
        .find(|slot| slot.id == parameter)
        .ok_or_else(|| format!("pixels::derivatives: missing parameter slot {parameter}"))?;
    Ok(slot
        .rate
        .map(|rate| (rate.max_delta.to_bits(), rate.max_second_delta.to_bits())))
}

pub(crate) fn contains_smooth_operation(
    graph: &super::symbolic::SymbolicGraph,
    root: FieldId,
) -> Result<bool, String> {
    let mut stack = vec![root];
    let mut seen = std::collections::BTreeSet::new();
    while let Some(field) = stack.pop() {
        if !seen.insert(field) {
            continue;
        }
        match &graph.fields.get(field)?.kind {
            super::graph::FieldKind::SmoothUnion { .. }
            | super::graph::FieldKind::SmoothIntersection { .. }
            | super::graph::FieldKind::SmoothSubtract { .. } => return Ok(true),
            super::graph::FieldKind::Transform { child, .. }
            | super::graph::FieldKind::FiniteRepeat { child, .. }
            | super::graph::FieldKind::Mark { child, .. }
            | super::graph::FieldKind::Neg { child } => stack.push(*child),
            super::graph::FieldKind::BoundedDisplace { base, .. } => stack.push(*base),
            super::graph::FieldKind::HardUnion { a, b }
            | super::graph::FieldKind::HardIntersection { a, b }
            | super::graph::FieldKind::HardSubtract { a, b } => stack.extend([*a, *b]),
            super::graph::FieldKind::Primitive(_) => {}
        }
    }
    Ok(false)
}

pub fn compile(
    graph: &super::symbolic::SymbolicGraph,
    equations: &mut ProjectiveEquations,
    structural: &super::verify::StructuralProgram,
) -> Result<DerivativePrograms, String> {
    let mut compiler = Compiler::new(equations)?;
    let mut bundles = Vec::new();
    for feature in &equations.features {
        let g = feature.root_equation;
        let u = compiler.derivative(g, Var::U)?;
        let v = compiler.derivative(g, Var::V)?;
        let q = compiler.derivative(g, Var::Q)?;
        let uu = compiler.derivative(u, Var::U)?;
        let uv = compiler.derivative(u, Var::V)?;
        let uq = compiler.derivative(u, Var::Q)?;
        let vv = compiler.derivative(v, Var::V)?;
        let vq = compiler.derivative(v, Var::Q)?;
        let qq = compiler.derivative(q, Var::Q)?;
        let third = ThirdDerivatives {
            uuu: compiler.derivative(uu, Var::U)?,
            uuv: compiler.derivative(uu, Var::V)?,
            uuq: compiler.derivative(uu, Var::Q)?,
            uvv: compiler.derivative(uv, Var::V)?,
            uvq: compiler.derivative(uv, Var::Q)?,
            uqq: compiler.derivative(uq, Var::Q)?,
            vvv: compiler.derivative(vv, Var::V)?,
            vvq: compiler.derivative(vv, Var::Q)?,
            vqq: compiler.derivative(vq, Var::Q)?,
            qqq: compiler.derivative(qq, Var::Q)?,
        };
        let mut parameter = Vec::new();
        let mut temporal_sources = Vec::new();
        for param in &feature.influencing_params {
            let polynomial = compiler.parameter_derivative(g, *param)?;
            temporal_sources.push((*param, polynomial));
            parameter.push(ParameterDerivative {
                parameter: *param,
                polynomial,
                declared_rate: declared_rate(&structural.params, *param)?,
            });
        }
        let g_t = compiler.temporal_derivative(g, &temporal_sources)?;
        bundles.push(DerivativeBundle {
            id: DerivativeBundleId(
                u32::try_from(bundles.len())
                    .map_err(|_| "P015: derivative bundle ID overflow".to_string())?,
            ),
            feature: feature.feature,
            g,
            first: FirstDerivatives { u, v, q },
            second: SecondDerivatives {
                uu,
                uv,
                uq,
                vv,
                vq,
                qq,
            },
            third,
            parameter,
            g_t,
            // P4 makes G_tt optional. It is omitted when the scalar
            // coefficient DAG does not carry exact second parameter
            // derivative programs.
            g_tt: None,
            influencing_params: feature.influencing_params.clone(),
        });
    }
    let mut by_object = BTreeMap::<ObjectId, Vec<DerivativeBundleId>>::new();
    for (feature, bundle) in structural.features.iter().zip(&bundles) {
        if feature.id != bundle.feature {
            return Err(format!(
                "pixels::derivatives: structural/bundle feature order differs at {}",
                feature.id
            ));
        }
        by_object.entry(feature.object).or_default().push(bundle.id);
    }
    let clusters = by_object
        .into_iter()
        .map(|(object, bundles)| -> Result<DerivativeClusterTemplate, String> {
            let object_record = structural
                .objects
                .objects
                .get(object.index())
                .filter(|record| record.id == object)
                .ok_or_else(|| format!("pixels::derivatives: missing smooth object {object}"))?;
            let mut leaf_signature = object_record.primitive_occurrences.clone();
            leaf_signature.sort();
            leaf_signature.dedup();
            let scalar_root = object_record.scalar_root;
            let value_domain = structural.values.get(scalar_root)?;
            let derivative = structural.derivatives.get(scalar_root)?;
            let (world_delta_abs_bound, remainder) =
                super::events::quadratic_taylor_remainder(
                    derivative.third_derivative_norm,
                    object_record.bounds,
                )?;
            let maximum_predictor_roots =
                bundles.iter().try_fold(0_u32, |total, bundle| {
                    let roots = equations
                        .features
                        .get(bundle.index())
                        .filter(|feature| feature.feature.index() == bundle.index())
                        .map(|feature| u32::from(feature.max_root_count))
                        .ok_or_else(|| {
                            format!(
                                "pixels::derivatives: smooth cluster {object} lacks predictor for dense bundle {bundle}"
                            )
                        })?;
                    total.checked_add(roots).ok_or_else(|| {
                        "P015: smooth cluster predictor-root count overflow".to_string()
                    })
                })?;
            if maximum_predictor_roots == 0 {
                return Err(format!(
                    "pixels::derivatives: smooth cluster {object} has no predictor root slabs"
                ));
            }
            let requires_boundary_events =
                contains_smooth_operation(graph, object_record.source_root)?;
            let maximum_object_roots = if requires_boundary_events {
                maximum_predictor_roots
                    .checked_mul(
                        1_u32
                            .checked_shl(
                                super::capacities::PixelsCeilings::MACHINE_V1
                                    .event_isolation_depth,
                            )
                            .ok_or_else(|| {
                                "P015: smooth root subdivision shift overflow".to_string()
                            })?,
                    )
                    .ok_or_else(|| {
                        "P015: smooth object root/corridor count overflow".to_string()
                    })?
            } else {
                maximum_predictor_roots
            };
            Ok(DerivativeClusterTemplate {
                object,
                leaf_signature,
                bundles,
                root_tube: SmoothRootTubeProgram {
                    scalar_root,
                    scalar_derivative_sources: vec![scalar_root],
                    value_domain,
                    first_world_abs: derivative.world_components.map(f64::abs),
                    second_world_abs: derivative.hessian_norm,
                    third_world_abs: derivative.third_derivative_norm,
                    parameter_abs: derivative
                        .parameter
                        .iter()
                        .map(|(parameter, value)| (*parameter, value.abs()))
                        .collect(),
                    frame_delta_abs: derivative.frame_delta.map(f64::abs),
                    frame_second_delta_abs: derivative.frame_second_delta.map(f64::abs),
                    taylor_order: 2,
                    subdivision_depth: super::capacities::checked_event_isolation_depth(
                        super::capacities::PixelsCeilings::MACHINE_V1.event_isolation_depth,
                    )?,
                    world_delta_abs_bound,
                    remainder,
                    maximum_predictor_roots,
                    maximum_object_roots,
                    requires_boundary_events,
                },
            })
        })
        .collect::<Result<Vec<_>, String>>()?;
    equations.coefficients = compiler.coefficients.finish();
    equations.polynomials = compiler.polynomials;
    super::projective::canonicalize_coefficient_ids(equations)?;
    Ok(DerivativePrograms { bundles, clusters })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pixels::camera::CameraContract;
    use crate::pixels::program::CoeffBuilder;
    use crate::pixels::reference::interval::F64Interval;

    #[test]
    fn mixed_partial_programs_cse_to_one_id() {
        let mut coefficients = CoeffBuilder::new();
        let one = coefficients.constant(1.0).unwrap();
        let u = PolyBuilder::variable(one, Var::U, coefficients.program()).unwrap();
        let v = PolyBuilder::variable(one, Var::V, coefficients.program()).unwrap();
        let q = PolyBuilder::variable(one, Var::Q, coefficients.program()).unwrap();
        let root = u
            .mul(v, &mut coefficients)
            .unwrap()
            .mul(q, &mut coefficients)
            .unwrap()
            .finish(PolyProgramId(0), ProgramLimit::Feature)
            .unwrap();
        let equations = ProjectiveEquations {
            camera: CameraContract {
                width: 1,
                height: 1,
                aspect: 1.0,
                tan_half_fov_y: 1.0,
                q: F64Interval::new(0.1, 10.0).unwrap(),
                eye_component: [F64Interval::new(-1.0, 1.0).unwrap(); 3],
                forward_component: [
                    F64Interval::point(0.0).unwrap(),
                    F64Interval::point(0.0).unwrap(),
                    F64Interval::point(1.0).unwrap(),
                ],
                right_component: [
                    F64Interval::point(1.0).unwrap(),
                    F64Interval::point(0.0).unwrap(),
                    F64Interval::point(0.0).unwrap(),
                ],
                up_component: [
                    F64Interval::point(0.0).unwrap(),
                    F64Interval::point(1.0).unwrap(),
                    F64Interval::point(0.0).unwrap(),
                ],
                eye_rate_component: [F64Interval::point(0.0).unwrap(); 3],
                forward_rate_component: [F64Interval::point(0.0).unwrap(); 3],
                right_rate_component: [F64Interval::point(0.0).unwrap(); 3],
                up_rate_component: [F64Interval::point(0.0).unwrap(); 3],
                max_frame_motion: 0.0,
            },
            coefficients: coefficients.finish(),
            polynomials: vec![root],
            rationals: Vec::new(),
            predicates: Vec::new(),
            features: Vec::new(),
        };
        let mut compiler = Compiler::new(&equations).unwrap();
        let u = compiler.derivative(PolyProgramId(0), Var::U).unwrap();
        let uv = compiler.derivative(u, Var::V).unwrap();
        let v = compiler.derivative(PolyProgramId(0), Var::V).unwrap();
        let vu = compiler.derivative(v, Var::U).unwrap();
        assert_eq!(uv, vu);
    }

    #[test]
    fn camera_motion_contributes_to_symbolic_temporal_derivative() {
        let mut coefficients = CoeffBuilder::new();
        let eye = coefficients
            .camera(crate::pixels::program::CameraCoeff::Eye(2))
            .unwrap();
        let one = coefficients.constant(1.0).unwrap();
        let q = PolyBuilder::variable(one, Var::Q, coefficients.program()).unwrap();
        let root = q
            .scale(eye, &mut coefficients)
            .unwrap()
            .finish(PolyProgramId(0), ProgramLimit::Feature)
            .unwrap();
        let equations = ProjectiveEquations {
            camera: CameraContract {
                width: 1,
                height: 1,
                aspect: 1.0,
                tan_half_fov_y: 1.0,
                q: F64Interval::new(0.1, 10.0).unwrap(),
                eye_component: [F64Interval::new(-1.0, 1.0).unwrap(); 3],
                forward_component: [F64Interval::point(0.0).unwrap(); 3],
                right_component: [F64Interval::point(0.0).unwrap(); 3],
                up_component: [F64Interval::point(0.0).unwrap(); 3],
                eye_rate_component: [F64Interval::new(-0.5, 0.5).unwrap(); 3],
                forward_rate_component: [F64Interval::new(-0.5, 0.5).unwrap(); 3],
                right_rate_component: [F64Interval::new(-0.5, 0.5).unwrap(); 3],
                up_rate_component: [F64Interval::new(-0.5, 0.5).unwrap(); 3],
                max_frame_motion: 0.5,
            },
            coefficients: coefficients.finish(),
            polynomials: vec![root],
            rationals: Vec::new(),
            predicates: Vec::new(),
            features: Vec::new(),
        };
        let mut compiler = Compiler::new(&equations).unwrap();
        let g_t = compiler.temporal_derivative(PolyProgramId(0), &[]).unwrap();
        let rate = 0.375;
        let q_value = 2.25;
        let coefficients = compiler
            .coefficients
            .program()
            .evaluate(&|_| Err("unexpected scalar".to_string()), &|camera| {
                Ok(match camera {
                    crate::pixels::program::CameraCoeff::Eye(2) => 1.25,
                    crate::pixels::program::CameraCoeff::EyeRate(2) => rate,
                    _ => 0.0,
                })
            })
            .unwrap();
        let evaluate = |program: PolyProgramId, q: f64, coefficients: &[f64]| {
            compiler.polynomials[program.index()]
                .terms
                .iter()
                .map(|term| {
                    coefficients[term.coefficient.index()] * q.powi(i32::from(term.exponents.q))
                })
                .sum::<f64>()
        };
        let symbolic = evaluate(g_t, q_value, &coefficients);
        assert!(symbolic.abs() > 0.0);
        let step = 1.0e-6;
        let root_at = |eye_value: f64| {
            let values = compiler
                .coefficients
                .program()
                .evaluate(&|_| Err("unexpected scalar".to_string()), &|camera| {
                    Ok(match camera {
                        crate::pixels::program::CameraCoeff::Eye(2) => eye_value,
                        crate::pixels::program::CameraCoeff::EyeRate(2) => rate,
                        _ => 0.0,
                    })
                })
                .unwrap();
            evaluate(PolyProgramId(0), q_value, &values)
        };
        let finite_difference =
            (root_at(1.25 + rate * step) - root_at(1.25 - rate * step)) / (2.0 * step);
        assert!((symbolic - finite_difference).abs() < 1.0e-8);
    }
}
