//! Exact homogeneous projective feature equations.

use std::collections::{BTreeMap, BTreeSet};

use super::bounds::ValueBounds;
use super::camera::{CameraContract, CameraSample, ScreenPoint};
use super::features::FeatureRecord;
use super::graph::{FieldKind, Primitive, TransformProgram};
use super::ids::{
    CoeffId, DomainId, FeatureId, ParamId, PolyProgramId, PredicateProgramId, RationalProgramId,
    ScalarId,
};
use super::objects::{ObjectPartition, SmoothObject};
use super::polynomial::{
    CompositionPlan, PolyBuilder, PolyProgram, ProgramLimit, Var, plan_quadratic_composition,
};
use super::primitive::{AnalyticPredicate, FeatureKind, OrientationProgram};
use super::program::{
    CameraCoeff, CoeffBuilder, CoeffOp, CoeffProgram, PredicateProgram, PredicateSense,
};
use super::reference::interval::F64Interval;
use super::symbolic::SymbolicGraph;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StrictSign {
    Negative,
    Positive,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct StrictSignObligation {
    pub coefficient: CoeffId,
    pub enclosure: F64Interval,
    pub sign: StrictSign,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum SeedKind {
    Affine {
        denominator: StrictSignObligation,
    },
    StableQuadratic {
        leading_coefficient: CoeffId,
        leading_enclosure: F64Interval,
        leading_sign: Option<StrictSign>,
        linear_fallback: bool,
        generic_isolation_fallback: bool,
    },
    GenericIsolatedRoot,
}

pub const BERNSTEIN_ISOLATION_DEPTH_V1: u8 = 64;
pub const BERNSTEIN_AMBIGUITY_DEPTH_V1: u8 = 12;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RootIsolationProgram {
    Affine,
    StableQuadratic {
        linear_fallback: bool,
        generic_isolation_fallback: bool,
    },
    CertifiedBernstein {
        maximum_depth: u8,
        ambiguity_depth: u8,
        preserve_all_positive_q_roots: bool,
    },
}

#[derive(Clone, Debug, PartialEq)]
pub struct QuadraticCandidates {
    pub roots: Vec<f64>,
    pub discriminant: f64,
    pub used_linear_fallback: bool,
    /// Candidate arithmetic cannot prove absence. When set, the authoritative
    /// certified interval/Bernstein isolator must still search the complete q
    /// domain.
    pub requires_generic_isolation: bool,
}

/// Produce only numerical seed candidates. A caller must still validate every
/// returned value against the authoritative polynomial and validity
/// predicates; a negative discriminant is never treated as a proof of absence.
pub fn stable_quadratic_candidates(a: f64, b: f64, c: f64) -> Result<QuadraticCandidates, String> {
    if !a.is_finite() || !b.is_finite() || !c.is_finite() {
        return Err("pixels::projective: non-finite quadratic coefficient".to_string());
    }
    if a == 0.0 {
        if b == 0.0 {
            return Err(
                "pixels::projective: degenerate quadratic has no finite seed rule".to_string(),
            );
        }
        let root = -c / b;
        if !root.is_finite() {
            return Err("pixels::projective: non-finite linear seed".to_string());
        }
        return Ok(QuadraticCandidates {
            roots: vec![root],
            discriminant: b.mul_add(b, 0.0),
            used_linear_fallback: true,
            requires_generic_isolation: false,
        });
    }
    let discriminant = b.mul_add(b, -4.0 * a * c);
    if !discriminant.is_finite() {
        return Err("pixels::projective: non-finite quadratic discriminant".to_string());
    }
    if discriminant < 0.0 {
        return Ok(QuadraticCandidates {
            roots: Vec::new(),
            discriminant,
            used_linear_fallback: false,
            requires_generic_isolation: true,
        });
    }
    let square_root = discriminant.sqrt();
    let stable_numerator = -0.5 * (b + square_root.copysign(b));
    let mut roots = if stable_numerator == 0.0 {
        vec![-b / (2.0 * a)]
    } else {
        vec![stable_numerator / a, c / stable_numerator]
    };
    if roots.iter().any(|root| !root.is_finite()) {
        return Err("pixels::projective: non-finite quadratic seed".to_string());
    }
    roots.sort_by(f64::total_cmp);
    roots.dedup_by(|left, right| left.to_bits() == right.to_bits());
    Ok(QuadraticCandidates {
        roots,
        discriminant,
        used_linear_fallback: false,
        requires_generic_isolation: false,
    })
}

#[derive(Clone, Debug, PartialEq)]
pub struct ProjectiveFeature {
    pub feature: FeatureId,
    pub root_equation: PolyProgramId,
    pub rational_program: Option<RationalProgramId>,
    pub q_degree: u8,
    pub validity_predicates: Vec<PredicateProgramId>,
    pub orientation_program: OrientationProgram,
    pub q_seed_kind: SeedKind,
    pub root_isolation: RootIsolationProgram,
    pub quadratic_composition: CompositionPlan,
    pub max_root_count: u8,
    pub deformed_predictor: bool,
    pub influencing_params: Vec<ParamId>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct RationalProgram {
    pub id: RationalProgramId,
    pub numerator: PolyProgramId,
    pub denominator: PolyProgramId,
    pub domain: DomainId,
    pub denominator_proof: StrictSignObligation,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ProjectiveEquations {
    pub camera: CameraContract,
    pub coefficients: CoeffProgram,
    pub polynomials: Vec<PolyProgram>,
    pub rationals: Vec<RationalProgram>,
    pub predicates: Vec<PredicateProgram>,
    pub features: Vec<ProjectiveFeature>,
}

fn canonicalize_coefficient_program(
    coefficients: CoeffProgram,
    polynomials: &mut [PolyProgram],
) -> Result<(CoeffProgram, Vec<Option<CoeffId>>), String> {
    let roots = polynomials
        .iter()
        .flat_map(|polynomial| polynomial.terms.iter().map(|term| term.coefficient))
        .collect::<Vec<_>>();
    let (coefficients, remap) = coefficients.canonicalize_reachable(roots)?;
    for polynomial in polynomials {
        for term in &mut polynomial.terms {
            term.coefficient = remap
                .get(term.coefficient.index())
                .copied()
                .flatten()
                .ok_or_else(|| {
                    format!(
                        "pixels::projective: polynomial {} coefficient {} was not canonicalized",
                        polynomial.id, term.coefficient
                    )
                })?;
        }
    }
    Ok((coefficients, remap))
}

pub(super) fn canonicalize_coefficient_ids(
    equations: &mut ProjectiveEquations,
) -> Result<Vec<Option<CoeffId>>, String> {
    let (coefficients, remap) = canonicalize_coefficient_program(
        std::mem::take(&mut equations.coefficients),
        &mut equations.polynomials,
    )?;
    let mapped = |id: CoeffId| {
        remap.get(id.index()).copied().flatten().ok_or_else(|| {
            format!("pixels::projective: live coefficient {id} was not canonicalized")
        })
    };
    for feature in &mut equations.features {
        feature.q_seed_kind = match feature.q_seed_kind {
            SeedKind::Affine { mut denominator } => {
                denominator.coefficient = mapped(denominator.coefficient)?;
                SeedKind::Affine { denominator }
            }
            SeedKind::StableQuadratic {
                leading_coefficient,
                leading_enclosure,
                leading_sign,
                linear_fallback,
                generic_isolation_fallback,
            } => SeedKind::StableQuadratic {
                leading_coefficient: mapped(leading_coefficient)?,
                leading_enclosure,
                leading_sign,
                linear_fallback,
                generic_isolation_fallback,
            },
            SeedKind::GenericIsolatedRoot => SeedKind::GenericIsolatedRoot,
        };
    }
    for rational in &mut equations.rationals {
        rational.denominator_proof.coefficient = mapped(rational.denominator_proof.coefficient)?;
    }
    equations.coefficients = coefficients;
    Ok(remap)
}

#[derive(Clone)]
struct HomogeneousPoint {
    components: [PolyBuilder; 3],
}

fn scalar_coefficient(
    coefficients: &mut CoeffBuilder,
    scalar: ScalarId,
) -> Result<CoeffId, String> {
    coefficients.scalar(scalar)
}

fn coefficient_poly(coefficients: &mut CoeffBuilder, coefficient: CoeffId) -> PolyBuilder {
    PolyBuilder::constant(coefficient, coefficients.program())
}

fn q_coefficient_poly(
    coefficients: &mut CoeffBuilder,
    coefficient: CoeffId,
) -> Result<PolyBuilder, String> {
    PolyBuilder::variable(coefficient, Var::Q, coefficients.program())
}

fn scalar_q(coefficients: &mut CoeffBuilder, scalar: ScalarId) -> Result<PolyBuilder, String> {
    let coefficient = scalar_coefficient(coefficients, scalar)?;
    q_coefficient_poly(coefficients, coefficient)
}

fn camera_point(coefficients: &mut CoeffBuilder) -> Result<HomogeneousPoint, String> {
    let one = coefficients.constant(1.0)?;
    let mut components = std::array::from_fn(|_| PolyBuilder::zero());
    for (component, output) in components.iter_mut().enumerate() {
        let eye = coefficients.camera(CameraCoeff::Eye(component as u8))?;
        let forward = coefficients.camera(CameraCoeff::Forward(component as u8))?;
        let right = coefficients.camera(CameraCoeff::Right(component as u8))?;
        let up = coefficients.camera(CameraCoeff::Up(component as u8))?;
        let eye = PolyBuilder::variable(eye, Var::Q, coefficients.program())?;
        let forward = coefficient_poly(coefficients, forward);
        let right = PolyBuilder::variable(right, Var::U, coefficients.program())?;
        let up = PolyBuilder::variable(up, Var::V, coefficients.program())?;
        *output = eye
            .add(forward, coefficients)?
            .add(right, coefficients)?
            .add(up, coefficients)?;
    }
    // Force the exact multiplicative identity into the coefficient program so
    // later generic homogenization can use one stable coefficient ID.
    let _ = one;
    Ok(HomogeneousPoint { components })
}

fn scale_poly(
    value: PolyBuilder,
    coefficient: CoeffId,
    coefficients: &mut CoeffBuilder,
) -> Result<PolyBuilder, String> {
    value.scale(coefficient, coefficients)
}

fn apply_atomic_transform(
    point: HomogeneousPoint,
    transform: &TransformProgram,
    coefficients: &mut CoeffBuilder,
) -> Result<HomogeneousPoint, String> {
    match transform {
        TransformProgram::Translate { by } => {
            let mut components = point.components;
            for component in 0..3 {
                components[component] = components[component]
                    .clone()
                    .sub(scalar_q(coefficients, by[component])?, coefficients)?;
            }
            Ok(HomogeneousPoint { components })
        }
        TransformProgram::Rotate {
            row_x,
            row_y,
            row_z,
        } => apply_rows(point, [*row_x, *row_y, *row_z], coefficients),
        TransformProgram::Rigid {
            translation,
            row_x,
            row_y,
            row_z,
        } => {
            let translated = apply_atomic_transform(
                point,
                &TransformProgram::Translate { by: *translation },
                coefficients,
            )?;
            apply_rows(translated, [*row_x, *row_y, *row_z], coefficients)
        }
        // UniformScale changes only the authoritative field value, not the
        // source coordinates or zero set.
        TransformProgram::UniformScale { .. } => Ok(point),
        TransformProgram::SourceRigidSequence { steps, .. }
        | TransformProgram::RigidSequence { steps, .. } => {
            let mut point = point;
            for step in steps {
                point = apply_atomic_transform(point, step, coefficients)?;
            }
            Ok(point)
        }
    }
}

fn apply_rows(
    point: HomogeneousPoint,
    rows: [[ScalarId; 3]; 3],
    coefficients: &mut CoeffBuilder,
) -> Result<HomogeneousPoint, String> {
    let source = point.components;
    let mut components = std::array::from_fn(|_| PolyBuilder::zero());
    for row in 0..3 {
        for component in 0..3 {
            let scalar = scalar_coefficient(coefficients, rows[row][component])?;
            let term = scale_poly(source[component].clone(), scalar, coefficients)?;
            components[row] = components[row].clone().add(term, coefficients)?;
        }
    }
    Ok(HomogeneousPoint { components })
}

fn fixed_repeat_translation(
    object: &SmoothObject,
    repeat_field: super::ids::FieldId,
) -> Result<(usize, f64), String> {
    let instance = object
        .repeat_instances
        .iter()
        .find(|instance| {
            instance.repeat_field == repeat_field
                || instance.equivalent_fields.contains(&repeat_field)
        })
        .ok_or_else(|| {
            format!(
                "pixels::projective: object {} has no fixed repeat instance for {repeat_field}",
                object.id
            )
        })?;
    let axis = match instance.axis {
        super::graph::Axis::X => 0,
        super::graph::Axis::Y => 1,
        super::graph::Axis::Z => 2,
    };
    // Match the source f32 index construction exactly.
    let ordinal = instance
        .index
        .checked_sub(instance.first)
        .ok_or_else(|| "pixels::projective: repeat ordinal underflow".to_string())?;
    let source_index = (instance.first as f32) + (ordinal as f32);
    Ok((axis, f64::from(source_index)))
}

fn local_point(
    graph: &SymbolicGraph,
    feature: &FeatureRecord,
    object: &SmoothObject,
    coefficients: &mut CoeffBuilder,
) -> Result<(HomogeneousPoint, bool), String> {
    let mut point = camera_point(coefficients)?;
    let mut deformed = false;
    for parent in feature.occurrence_path.iter().skip(1) {
        match &graph.fields.get(parent.field)?.kind {
            FieldKind::Transform { transform, .. } => {
                point = apply_atomic_transform(point, transform, coefficients)?;
            }
            FieldKind::FiniteRepeat { period, .. } => {
                let (axis, index) = fixed_repeat_translation(object, parent.field)?;
                let period = scalar_coefficient(coefficients, *period)?;
                let index = coefficients.constant(index)?;
                let translation = coefficients.mul(period, index)?;
                point.components[axis] = point.components[axis]
                    .clone()
                    .sub(q_coefficient_poly(coefficients, translation)?, coefficients)?;
            }
            FieldKind::BoundedDisplace { .. } => deformed = true,
            _ => {}
        }
    }
    Ok((point, deformed))
}

fn vector_difference(
    point: &[PolyBuilder; 3],
    center: [ScalarId; 3],
    coefficients: &mut CoeffBuilder,
) -> Result<[PolyBuilder; 3], String> {
    let mut result = point.clone();
    for component in 0..3 {
        result[component] = result[component]
            .clone()
            .sub(scalar_q(coefficients, center[component])?, coefficients)?;
    }
    Ok(result)
}

fn dot_polys(
    a: &[PolyBuilder; 3],
    b: &[PolyBuilder; 3],
    coefficients: &mut CoeffBuilder,
) -> Result<PolyBuilder, String> {
    let mut result = PolyBuilder::zero();
    for component in 0..3 {
        result = result.add(
            a[component]
                .clone()
                .mul(b[component].clone(), coefficients)?,
            coefficients,
        )?;
    }
    Ok(result)
}

fn dot_poly_scalars(
    a: &[PolyBuilder; 3],
    b: [ScalarId; 3],
    coefficients: &mut CoeffBuilder,
) -> Result<PolyBuilder, String> {
    let mut result = PolyBuilder::zero();
    for component in 0..3 {
        let coefficient = scalar_coefficient(coefficients, b[component])?;
        result = result.add(
            scale_poly(a[component].clone(), coefficient, coefficients)?,
            coefficients,
        )?;
    }
    Ok(result)
}

fn scalar_vector_sub(
    a: [ScalarId; 3],
    b: [ScalarId; 3],
    coefficients: &mut CoeffBuilder,
) -> Result<[CoeffId; 3], String> {
    let mut result = [CoeffId(0); 3];
    for component in 0..3 {
        let a = scalar_coefficient(coefficients, a[component])?;
        let b = scalar_coefficient(coefficients, b[component])?;
        result[component] = coefficients.sub(a, b)?;
    }
    Ok(result)
}

fn coeff_dot(
    a: [CoeffId; 3],
    b: [CoeffId; 3],
    coefficients: &mut CoeffBuilder,
) -> Result<CoeffId, String> {
    let mut result = coefficients.constant(0.0)?;
    for component in 0..3 {
        let product = coefficients.mul(a[component], b[component])?;
        result = coefficients.add(result, product)?;
    }
    Ok(result)
}

fn implicit_plane(
    point: &[PolyBuilder; 3],
    normal: [ScalarId; 3],
    offset: ScalarId,
    coefficients: &mut CoeffBuilder,
) -> Result<PolyBuilder, String> {
    dot_poly_scalars(point, normal, coefficients)?
        .add(scalar_q(coefficients, offset)?, coefficients)
}

fn implicit_sphere(
    point: &[PolyBuilder; 3],
    center: [ScalarId; 3],
    radius: ScalarId,
    coefficients: &mut CoeffBuilder,
) -> Result<PolyBuilder, String> {
    let difference = vector_difference(point, center, coefficients)?;
    let squared = dot_polys(&difference, &difference, coefficients)?;
    let radius = scalar_coefficient(coefficients, radius)?;
    let radius_squared = coefficients.mul(radius, radius)?;
    let one = coefficients.constant(1.0)?;
    let q_one = q_coefficient_poly(coefficients, one)?;
    let q2 = q_coefficient_poly(coefficients, radius_squared)?.mul(q_one, coefficients)?;
    squared.sub(q2, coefficients)
}

fn implicit_segment_side(
    point: &[PolyBuilder; 3],
    a: [ScalarId; 3],
    b: [ScalarId; 3],
    radius_a: ScalarId,
    radius_b: ScalarId,
    coefficients: &mut CoeffBuilder,
) -> Result<PolyBuilder, String> {
    let w = vector_difference(point, a, coefficients)?;
    let h = scalar_vector_sub(b, a, coefficients)?;
    let h2 = coeff_dot(h, h, coefficients)?;
    let w2 = dot_polys(&w, &w, coefficients)?;
    let mut axial = PolyBuilder::zero();
    for component in 0..3 {
        axial = axial.add(
            scale_poly(w[component].clone(), h[component], coefficients)?,
            coefficients,
        )?;
    }
    let h2_squared = coefficients.mul(h2, h2)?;
    let left = scale_poly(w2, h2_squared, coefficients)?;
    let axial_squared = axial.clone().mul(axial.clone(), coefficients)?;
    let middle = scale_poly(axial_squared, h2, coefficients)?;
    let radius_a = scalar_coefficient(coefficients, radius_a)?;
    let radius_b = scalar_coefficient(coefficients, radius_b)?;
    let radius_delta = coefficients.sub(radius_b, radius_a)?;
    let radius_base = coefficients.mul(radius_a, h2)?;
    let radius = q_coefficient_poly(coefficients, radius_base)?
        .add(scale_poly(axial, radius_delta, coefficients)?, coefficients)?;
    let radius_squared = radius.clone().mul(radius, coefficients)?;
    left.sub(middle, coefficients)?
        .sub(radius_squared, coefficients)
}

fn implicit_torus(
    point: &[PolyBuilder; 3],
    center: [ScalarId; 3],
    axis: [ScalarId; 3],
    major: ScalarId,
    minor: ScalarId,
    coefficients: &mut CoeffBuilder,
) -> Result<PolyBuilder, String> {
    let w = vector_difference(point, center, coefficients)?;
    let w2 = dot_polys(&w, &w, coefficients)?;
    let axis_coeffs = [
        scalar_coefficient(coefficients, axis[0])?,
        scalar_coefficient(coefficients, axis[1])?,
        scalar_coefficient(coefficients, axis[2])?,
    ];
    let axis2 = coeff_dot(axis_coeffs, axis_coeffs, coefficients)?;
    let mut axial = PolyBuilder::zero();
    for component in 0..3 {
        axial = axial.add(
            scale_poly(w[component].clone(), axis_coeffs[component], coefficients)?,
            coefficients,
        )?;
    }
    let axial2 = axial.clone().mul(axial, coefficients)?;
    let major = scalar_coefficient(coefficients, major)?;
    let minor = scalar_coefficient(coefficients, minor)?;
    let major2 = coefficients.mul(major, major)?;
    let minor2 = coefficients.mul(minor, minor)?;
    let radius_delta = coefficients.sub(major2, minor2)?;
    let one = coefficients.constant(1.0)?;
    let q2 = q_coefficient_poly(coefficients, one)?.mul(
        q_coefficient_poly(coefficients, radius_delta)?,
        coefficients,
    )?;
    let sum = w2.clone().add(q2, coefficients)?;
    let square = sum.clone().mul(sum, coefficients)?;
    let first = scale_poly(square, axis2, coefficients)?;
    let axis_w2 = scale_poly(w2, axis2, coefficients)?;
    let perpendicular = axis_w2.sub(axial2, coefficients)?;
    let four = coefficients.constant(4.0)?;
    let four_major2 = coefficients.mul(four, major2)?;
    let q2_scale = q_coefficient_poly(coefficients, one)?
        .mul(q_coefficient_poly(coefficients, four_major2)?, coefficients)?;
    let second = perpendicular.mul(q2_scale, coefficients)?;
    first.sub(second, coefficients)
}

fn root_equation(
    primitive: &Primitive,
    validity: &AnalyticPredicate,
    point: &[PolyBuilder; 3],
    coefficients: &mut CoeffBuilder,
) -> Result<PolyBuilder, String> {
    match validity {
        AnalyticPredicate::Always => match primitive {
            Primitive::Plane { normal, offset } => {
                implicit_plane(point, *normal, *offset, coefficients)
            }
            _ => Err("pixels::projective: `Always` validity belongs to a non-plane".to_string()),
        },
        AnalyticPredicate::Sphere { center, radius } => {
            implicit_sphere(point, *center, *radius, coefficients)
        }
        AnalyticPredicate::RoundBoxCorner {
            signs,
            center,
            half,
            radius,
        } => {
            let mut difference = point.clone();
            for component in 0..3 {
                let center_coefficient = scalar_coefficient(coefficients, center[component])?;
                let half_coefficient = scalar_coefficient(coefficients, half[component])?;
                let signed = coefficients.scale(half_coefficient, f64::from(signs[component]))?;
                let combined = coefficients.add(center_coefficient, signed)?;
                difference[component] = difference[component]
                    .clone()
                    .sub(q_coefficient_poly(coefficients, combined)?, coefficients)?;
            }
            let squared = dot_polys(&difference, &difference, coefficients)?;
            let radius = scalar_coefficient(coefficients, *radius)?;
            let radius2 = coefficients.mul(radius, radius)?;
            let one = coefficients.constant(1.0)?;
            let q_one = q_coefficient_poly(coefficients, one)?;
            let q_radius = q_coefficient_poly(coefficients, radius2)?;
            let q2 = q_one.mul(q_radius, coefficients)?;
            squared.sub(q2, coefficients)
        }
        AnalyticPredicate::BoxFace {
            axis,
            sign,
            center,
            half,
        } => {
            let center = scalar_coefficient(coefficients, center[usize::from(*axis)])?;
            let half = scalar_coefficient(coefficients, half[usize::from(*axis)])?;
            let half = coefficients.scale(half, f64::from(*sign))?;
            let plane = coefficients.add(center, half)?;
            point[usize::from(*axis)]
                .clone()
                .sub(q_coefficient_poly(coefficients, plane)?, coefficients)
        }
        AnalyticPredicate::RoundBoxFace {
            axis,
            sign,
            center,
            half,
            radius,
        } => {
            let center = scalar_coefficient(coefficients, center[usize::from(*axis)])?;
            let half = scalar_coefficient(coefficients, half[usize::from(*axis)])?;
            let radius = scalar_coefficient(coefficients, *radius)?;
            let extent = coefficients.add(half, radius)?;
            let extent = coefficients.scale(extent, f64::from(*sign))?;
            let plane = coefficients.add(center, extent)?;
            point[usize::from(*axis)]
                .clone()
                .sub(q_coefficient_poly(coefficients, plane)?, coefficients)
        }
        AnalyticPredicate::RoundBoxEdge {
            axis,
            signs,
            center,
            half,
            radius,
        } => {
            let radial = (0..3)
                .filter(|component| *component != usize::from(*axis))
                .collect::<Vec<_>>();
            // Scalar IDs cannot encode addition. Build the two radial
            // differences directly.
            let mut equation = PolyBuilder::zero();
            for (ordinal, component) in radial.into_iter().enumerate() {
                let center = scalar_coefficient(coefficients, center[component])?;
                let half = scalar_coefficient(coefficients, half[component])?;
                let signed = coefficients.scale(half, f64::from(signs[ordinal]))?;
                let center = coefficients.add(center, signed)?;
                let difference = point[component]
                    .clone()
                    .sub(q_coefficient_poly(coefficients, center)?, coefficients)?;
                equation = equation.add(
                    difference.clone().mul(difference, coefficients)?,
                    coefficients,
                )?;
            }
            let radius = scalar_coefficient(coefficients, *radius)?;
            let radius2 = coefficients.mul(radius, radius)?;
            let one = coefficients.constant(1.0)?;
            let q2 = q_coefficient_poly(coefficients, one)?
                .mul(q_coefficient_poly(coefficients, radius2)?, coefficients)?;
            equation.sub(q2, coefficients)
        }
        AnalyticPredicate::SegmentSide {
            a,
            b,
            radius_a,
            radius_b,
        } => implicit_segment_side(point, *a, *b, *radius_a, *radius_b, coefficients),
        AnalyticPredicate::SegmentCap {
            endpoint,
            a,
            b,
            radius,
            hemisphere,
        } => {
            let endpoint_point = if *endpoint == 0 { *a } else { *b };
            if *hemisphere {
                return implicit_sphere(point, endpoint_point, *radius, coefficients);
            }
            let difference = vector_difference(point, endpoint_point, coefficients)?;
            let axis = if *endpoint == 0 { (*a, *b) } else { (*b, *a) };
            let normal = scalar_vector_sub(axis.1, axis.0, coefficients)?;
            let mut equation = PolyBuilder::zero();
            for component in 0..3 {
                equation = equation.add(
                    scale_poly(
                        difference[component].clone(),
                        normal[component],
                        coefficients,
                    )?,
                    coefficients,
                )?;
            }
            Ok(equation)
        }
        AnalyticPredicate::TorusDomain {
            center,
            axis,
            major,
            minor,
        } => implicit_torus(point, *center, *axis, *major, *minor, coefficients),
    }
}

fn validity_boundaries(
    predicate: &AnalyticPredicate,
    point: &[PolyBuilder; 3],
    coefficients: &mut CoeffBuilder,
) -> Result<Vec<(PolyBuilder, PredicateSense)>, String> {
    let mut boundaries = Vec::new();
    match predicate {
        AnalyticPredicate::Always
        | AnalyticPredicate::Sphere { .. }
        | AnalyticPredicate::TorusDomain { .. } => {}
        AnalyticPredicate::BoxFace {
            axis, center, half, ..
        }
        | AnalyticPredicate::RoundBoxFace {
            axis, center, half, ..
        } => {
            for component in 0..3 {
                if component == usize::from(*axis) {
                    continue;
                }
                for sign in [-1.0, 1.0] {
                    let center = scalar_coefficient(coefficients, center[component])?;
                    let half = scalar_coefficient(coefficients, half[component])?;
                    let signed_half = coefficients.scale(half, sign)?;
                    let bound = coefficients.add(center, signed_half)?;
                    boundaries.push((
                        point[component]
                            .clone()
                            .sub(q_coefficient_poly(coefficients, bound)?, coefficients)?,
                        if sign < 0.0 {
                            PredicateSense::NonNegative
                        } else {
                            PredicateSense::NonPositive
                        },
                    ));
                }
            }
        }
        AnalyticPredicate::RoundBoxEdge {
            axis,
            signs,
            center,
            half,
            ..
        } => {
            let axial = usize::from(*axis);
            for sign in [-1.0, 1.0] {
                let center = scalar_coefficient(coefficients, center[axial])?;
                let half = scalar_coefficient(coefficients, half[axial])?;
                let signed_half = coefficients.scale(half, sign)?;
                let bound = coefficients.add(center, signed_half)?;
                boundaries.push((
                    point[axial]
                        .clone()
                        .sub(q_coefficient_poly(coefficients, bound)?, coefficients)?,
                    if sign < 0.0 {
                        PredicateSense::NonNegative
                    } else {
                        PredicateSense::NonPositive
                    },
                ));
            }
            let radial = (0..3)
                .filter(|component| *component != axial)
                .collect::<Vec<_>>();
            for (ordinal, component) in radial.into_iter().enumerate() {
                let center = scalar_coefficient(coefficients, center[component])?;
                let half = scalar_coefficient(coefficients, half[component])?;
                let signed_half = coefficients.scale(half, f64::from(signs[ordinal]))?;
                let bound = coefficients.add(center, signed_half)?;
                boundaries.push((
                    point[component]
                        .clone()
                        .sub(q_coefficient_poly(coefficients, bound)?, coefficients)?,
                    if signs[ordinal] < 0 {
                        PredicateSense::NonPositive
                    } else {
                        PredicateSense::NonNegative
                    },
                ));
            }
        }
        AnalyticPredicate::RoundBoxCorner {
            signs,
            center,
            half,
            ..
        } => {
            for component in 0..3 {
                let center = scalar_coefficient(coefficients, center[component])?;
                let half = scalar_coefficient(coefficients, half[component])?;
                let signed_half = coefficients.scale(half, f64::from(signs[component]))?;
                let bound = coefficients.add(center, signed_half)?;
                boundaries.push((
                    point[component]
                        .clone()
                        .sub(q_coefficient_poly(coefficients, bound)?, coefficients)?,
                    if signs[component] < 0 {
                        PredicateSense::NonPositive
                    } else {
                        PredicateSense::NonNegative
                    },
                ));
            }
        }
        AnalyticPredicate::SegmentSide { a, b, .. } => {
            let w = vector_difference(point, *a, coefficients)?;
            let axis = scalar_vector_sub(*b, *a, coefficients)?;
            let mut projection = PolyBuilder::zero();
            for component in 0..3 {
                projection = projection.add(
                    scale_poly(w[component].clone(), axis[component], coefficients)?,
                    coefficients,
                )?;
            }
            boundaries.push((projection.clone(), PredicateSense::NonNegative));
            let axis2 = coeff_dot(axis, axis, coefficients)?;
            boundaries.push((
                projection.sub(q_coefficient_poly(coefficients, axis2)?, coefficients)?,
                PredicateSense::NonPositive,
            ));
        }
        AnalyticPredicate::SegmentCap {
            endpoint,
            a,
            b,
            radius,
            hemisphere,
        } => {
            let center = if *endpoint == 0 { *a } else { *b };
            let w = vector_difference(point, center, coefficients)?;
            let axis = scalar_vector_sub(*b, *a, coefficients)?;
            if *hemisphere {
                let projection = w.iter().zip(axis).try_fold(
                    PolyBuilder::zero(),
                    |sum, (component, axis)| {
                        sum.add(
                            scale_poly(component.clone(), axis, coefficients)?,
                            coefficients,
                        )
                    },
                )?;
                boundaries.push((
                    projection,
                    if *endpoint == 0 {
                        PredicateSense::NonPositive
                    } else {
                        PredicateSense::NonNegative
                    },
                ));
            } else {
                let axis2 = coeff_dot(axis, axis, coefficients)?;
                let w2 = dot_polys(&w, &w, coefficients)?;
                let projection = w.iter().zip(axis).try_fold(
                    PolyBuilder::zero(),
                    |sum, (component, axis)| {
                        sum.add(
                            scale_poly(component.clone(), axis, coefficients)?,
                            coefficients,
                        )
                    },
                )?;
                let projection2 = projection.clone().mul(projection, coefficients)?;
                let perpendicular =
                    scale_poly(w2, axis2, coefficients)?.sub(projection2, coefficients)?;
                let radius = scalar_coefficient(coefficients, *radius)?;
                let radius2 = coefficients.mul(radius, radius)?;
                let radial_bound = coefficients.mul(radius2, axis2)?;
                let one = coefficients.constant(1.0)?;
                let q2 = q_coefficient_poly(coefficients, one)?.mul(
                    q_coefficient_poly(coefficients, radial_bound)?,
                    coefficients,
                )?;
                boundaries.push((
                    perpendicular.sub(q2, coefficients)?,
                    PredicateSense::NonPositive,
                ));
            }
        }
    }
    let expected = predicate.boundary_generator_count();
    let actual = u32::try_from(boundaries.len())
        .map_err(|_| "P015: validity boundary count overflow".to_string())?;
    if actual != expected {
        return Err(format!(
            "pixels::projective: validity predicate boundary count {actual} differs from structural count {expected}"
        ));
    }
    Ok(boundaries)
}

pub(crate) fn coefficient_intervals(
    program: &CoeffProgram,
    values: &ValueBounds,
    camera: CameraContract,
) -> Result<Vec<F64Interval>, String> {
    coefficient_intervals_for_roots(
        program,
        values,
        camera,
        program.nodes.iter().map(|node| node.id),
    )
}

pub(crate) fn coefficient_intervals_for_roots(
    program: &CoeffProgram,
    values: &ValueBounds,
    camera: CameraContract,
    roots: impl IntoIterator<Item = CoeffId>,
) -> Result<Vec<F64Interval>, String> {
    let mut needed = BTreeSet::new();
    let mut pending = roots.into_iter().collect::<Vec<_>>();
    while let Some(id) = pending.pop() {
        if !needed.insert(id) {
            continue;
        }
        match program.get(id)?.op {
            CoeffOp::Add(a, b) | CoeffOp::Mul(a, b) => pending.extend([a, b]),
            CoeffOp::Neg(value) => pending.push(value),
            _ => {}
        }
    }
    let mut intervals = Vec::with_capacity(program.nodes.len());
    for node in &program.nodes {
        if !needed.contains(&node.id) {
            intervals.push(F64Interval::point(0.0)?);
            continue;
        }
        let get = |id: CoeffId, intervals: &[F64Interval]| {
            intervals.get(id.index()).copied().ok_or_else(|| {
                format!("pixels::projective: coefficient {id} names a non-predecessor")
            })
        };
        let interval = match node.op {
            CoeffOp::ConstF64(bits) => F64Interval::point(f64::from_bits(bits))?,
            CoeffOp::Scalar(id) => values.get(id)?,
            CoeffOp::ScalarParamDerivative(_, _) | CoeffOp::ParamRate(_, _) => {
                return Err(
                    "pixels::projective: derivative-only coefficient reached root seed analysis"
                        .to_string(),
                );
            }
            CoeffOp::Camera(CameraCoeff::Eye(component)) => camera
                .eye_component
                .get(usize::from(component))
                .copied()
                .ok_or_else(|| "pixels::projective: camera eye component overflow".to_string())?,
            CoeffOp::Camera(CameraCoeff::Forward(component)) => camera
                .forward_component
                .get(usize::from(component))
                .copied()
                .ok_or_else(|| {
                    "pixels::projective: camera forward component overflow".to_string()
                })?,
            CoeffOp::Camera(CameraCoeff::Right(component)) => camera
                .right_component
                .get(usize::from(component))
                .copied()
                .ok_or_else(|| "pixels::projective: camera right component overflow".to_string())?,
            CoeffOp::Camera(CameraCoeff::Up(component)) => camera
                .up_component
                .get(usize::from(component))
                .copied()
                .ok_or_else(|| "pixels::projective: camera up component overflow".to_string())?,
            CoeffOp::Camera(CameraCoeff::EyeRate(component)) => camera
                .eye_rate_component
                .get(usize::from(component))
                .copied()
                .ok_or_else(|| {
                    "pixels::projective: camera eye-rate component overflow".to_string()
                })?,
            CoeffOp::Camera(CameraCoeff::ForwardRate(component)) => camera
                .forward_rate_component
                .get(usize::from(component))
                .copied()
                .ok_or_else(|| {
                    "pixels::projective: camera forward-rate component overflow".to_string()
                })?,
            CoeffOp::Camera(CameraCoeff::RightRate(component)) => camera
                .right_rate_component
                .get(usize::from(component))
                .copied()
                .ok_or_else(|| {
                    "pixels::projective: camera right-rate component overflow".to_string()
                })?,
            CoeffOp::Camera(CameraCoeff::UpRate(component)) => camera
                .up_rate_component
                .get(usize::from(component))
                .copied()
                .ok_or_else(|| {
                    "pixels::projective: camera up-rate component overflow".to_string()
                })?,
            CoeffOp::Camera(CameraCoeff::TanHalfFovY) => F64Interval::point(camera.tan_half_fov_y)?,
            CoeffOp::Camera(CameraCoeff::Aspect) => F64Interval::point(camera.aspect)?,
            CoeffOp::Add(a, b) => get(a, &intervals)?.add_outward(get(b, &intervals)?)?,
            CoeffOp::Mul(a, b) => get(a, &intervals)?.mul_outward(get(b, &intervals)?)?,
            CoeffOp::Neg(value) => get(value, &intervals)?.neg(),
        };
        intervals.push(interval);
    }
    Ok(intervals)
}

fn q_leading_coefficient(program: &PolyProgram) -> Option<CoeffId> {
    let candidates = program
        .terms
        .iter()
        .filter(|term| {
            term.exponents.q == program.degree_q
                && term.exponents.u == 0
                && term.exponents.v == 0
                && term.exponents.x == 0
                && term.exponents.t == 0
                && term.exponents.param_terms.count == 0
        })
        .map(|term| term.coefficient)
        .collect::<Vec<_>>();
    (candidates.len() == 1).then_some(candidates[0])
}

pub(super) fn affine_rational_parts(
    root: &PolyProgram,
    coefficients: &mut CoeffBuilder,
) -> Result<(PolyBuilder, PolyBuilder), String> {
    if root.degree_q != 1 {
        return Err(format!(
            "pixels::projective: polynomial {} is not affine in q",
            root.id
        ));
    }
    let mut numerator = PolyBuilder::zero();
    let mut denominator = PolyBuilder::zero();
    for term in &root.terms {
        let mut exponents = term.exponents;
        exponents.q = 0;
        let coefficient = match term.exponents.q {
            0 => coefficients.neg(term.coefficient)?,
            1 => term.coefficient,
            degree => {
                return Err(format!(
                    "pixels::projective: affine polynomial {} has q degree {degree}",
                    root.id
                ));
            }
        };
        let monomial = PolyBuilder::monomial(coefficient, exponents, coefficients.program());
        if term.exponents.q == 0 {
            numerator = numerator.add(monomial, coefficients)?;
        } else {
            denominator = denominator.add(monomial, coefficients)?;
        }
    }
    Ok((numerator, denominator))
}

fn strict_sign(coefficient: CoeffId, interval: F64Interval) -> Option<StrictSignObligation> {
    let sign = if interval.lo > 0.0 {
        StrictSign::Positive
    } else if interval.hi < 0.0 {
        StrictSign::Negative
    } else {
        return None;
    };
    Some(StrictSignObligation {
        coefficient,
        enclosure: interval,
        sign,
    })
}

fn select_seed_kind(q_degree: u8, root: &PolyProgram, intervals: &[F64Interval]) -> SeedKind {
    let leading = q_leading_coefficient(root).map(|coefficient| {
        (
            coefficient,
            strict_sign(coefficient, intervals[coefficient.index()]),
        )
    });
    match (q_degree, leading) {
        (1, Some((_, Some(denominator)))) => SeedKind::Affine { denominator },
        (2, Some((leading_coefficient, obligation))) => SeedKind::StableQuadratic {
            leading_coefficient,
            leading_enclosure: intervals[leading_coefficient.index()],
            leading_sign: obligation.map(|obligation| obligation.sign),
            linear_fallback: obligation.is_none(),
            generic_isolation_fallback: true,
        },
        _ => SeedKind::GenericIsolatedRoot,
    }
}

fn scalar_param_dependencies(
    graph: &SymbolicGraph,
) -> Result<BTreeMap<ScalarId, Vec<ParamId>>, String> {
    let mut result = BTreeMap::new();
    for (id, node) in graph.scalar.iter() {
        let mut dependencies = BTreeSet::new();
        if let super::scalar::ScalarOp::Param(param) = node.op {
            dependencies.insert(param);
        }
        for child in super::params::scalar_children(&node.op) {
            let child = result.get(&child).ok_or_else(|| {
                format!("pixels::projective: scalar {id} names non-predecessor {child}")
            })?;
            dependencies.extend(child);
        }
        result.insert(id, dependencies.into_iter().collect());
    }
    Ok(result)
}

pub(crate) fn polynomial_influencing_params(
    graph: &SymbolicGraph,
    equations: &ProjectiveEquations,
    polynomial: PolyProgramId,
) -> Result<Vec<ParamId>, String> {
    let polynomial = equations
        .polynomials
        .get(polynomial.index())
        .filter(|program| program.id == polynomial)
        .ok_or_else(|| format!("pixels::projective: missing polynomial {polynomial}"))?;
    equations.coefficients.influencing_params(
        polynomial.terms.iter().map(|term| term.coefficient),
        &scalar_param_dependencies(graph)?,
    )
}

pub fn compile(
    graph: &SymbolicGraph,
    config: &super::config::RendererConfig,
    values: &ValueBounds,
    objects: &ObjectPartition,
    features: &[FeatureRecord],
) -> Result<ProjectiveEquations, String> {
    let camera = CameraContract::derive(config)?;
    let scalar_params = scalar_param_dependencies(graph)?;
    let mut coefficients = CoeffBuilder::new();
    let mut polynomials = Vec::new();
    let mut predicates = Vec::new();
    let mut lowered = Vec::new();
    for feature in features {
        let object = objects.objects.get(feature.object.index()).ok_or_else(|| {
            format!(
                "pixels::projective: feature {} names missing object {}",
                feature.id, feature.object
            )
        })?;
        let FieldKind::Primitive(primitive) = &graph.fields.get(feature.primitive)?.kind else {
            return Err(format!(
                "pixels::projective: feature {} primitive {} is not primitive",
                feature.id, feature.primitive
            ));
        };
        let [validity] = feature.validity.constraints.as_slice() else {
            return Err(format!(
                "pixels::projective: feature {} has noncanonical validity arity {}",
                feature.id,
                feature.validity.constraints.len()
            ));
        };
        let (point, deformed) = local_point(graph, feature, object, &mut coefficients)?;
        let root = root_equation(primitive, validity, &point.components, &mut coefficients)?;
        let root_id = PolyProgramId(
            u32::try_from(polynomials.len())
                .map_err(|_| "P015: projective program ID overflow".to_string())?,
        );
        let root = root.finish(root_id, ProgramLimit::Feature)?;
        let q_degree = root.degree_q;
        let root_coefficients = root
            .terms
            .iter()
            .map(|term| term.coefficient)
            .collect::<Vec<_>>();
        polynomials.push(root);
        let rational_parts = if q_degree == 1 {
            let (numerator, denominator) =
                affine_rational_parts(&polynomials[root_id.index()], &mut coefficients)?;
            let numerator_id = PolyProgramId(
                u32::try_from(polynomials.len())
                    .map_err(|_| "P015: projective program ID overflow".to_string())?,
            );
            polynomials.push(numerator.finish(numerator_id, ProgramLimit::Feature)?);
            let denominator_id = PolyProgramId(
                u32::try_from(polynomials.len())
                    .map_err(|_| "P015: projective program ID overflow".to_string())?,
            );
            polynomials.push(denominator.finish(denominator_id, ProgramLimit::Feature)?);
            Some((numerator_id, denominator_id))
        } else {
            None
        };
        let mut validity_predicates = Vec::new();
        for (family, (boundary, sense)) in
            validity_boundaries(validity, &point.components, &mut coefficients)?
                .into_iter()
                .enumerate()
        {
            let polynomial = PolyProgramId(
                u32::try_from(polynomials.len())
                    .map_err(|_| "P015: projective program ID overflow".to_string())?,
            );
            polynomials.push(boundary.finish(polynomial, ProgramLimit::EventBivariate)?);
            let id = PredicateProgramId(
                u32::try_from(predicates.len())
                    .map_err(|_| "P015: predicate program ID overflow".to_string())?,
            );
            predicates.push(PredicateProgram {
                id,
                polynomial,
                sense,
                boundary_family: u16::try_from(family)
                    .map_err(|_| "P015: predicate boundary family overflow".to_string())?,
            });
            validity_predicates.push(id);
        }
        lowered.push((
            feature,
            q_degree,
            root_id,
            root_coefficients,
            validity_predicates,
            deformed,
            rational_parts,
        ));
    }
    let (coefficient_program, coefficient_remap) =
        canonicalize_coefficient_program(coefficients.finish(), &mut polynomials)?;
    for (_, _, _, roots, _, _, _) in &mut lowered {
        for root in roots {
            *root = coefficient_remap
                .get(root.index())
                .copied()
                .flatten()
                .ok_or_else(|| {
                    format!("pixels::projective: feature coefficient {root} was not canonicalized")
                })?;
        }
    }
    let intervals = coefficient_intervals(&coefficient_program, values, camera)?;
    let mut projective_features = Vec::new();
    let mut rationals = Vec::new();
    for (feature, q_degree, root_equation, roots, validity_predicates, deformed, rational_parts) in
        lowered
    {
        let root = &polynomials[root_equation.index()];
        let q_seed_kind = select_seed_kind(q_degree, root, &intervals);
        let rational_program = match (q_seed_kind, rational_parts) {
            (
                SeedKind::Affine {
                    denominator: denominator_proof,
                },
                Some((numerator, denominator)),
            ) => {
                let id = RationalProgramId(
                    u32::try_from(rationals.len())
                        .map_err(|_| "P015: rational program ID overflow".to_string())?,
                );
                rationals.push(RationalProgram {
                    id,
                    numerator,
                    denominator,
                    domain: DomainId(0),
                    denominator_proof,
                });
                Some(id)
            }
            (SeedKind::Affine { .. }, None) => {
                return Err(format!(
                    "pixels::projective: affine feature {} lacks rational parts",
                    feature.id
                ));
            }
            (_, _) => None,
        };
        let root_isolation = match q_seed_kind {
            SeedKind::Affine { .. } => RootIsolationProgram::Affine,
            SeedKind::StableQuadratic {
                linear_fallback,
                generic_isolation_fallback,
                ..
            } => RootIsolationProgram::StableQuadratic {
                linear_fallback,
                generic_isolation_fallback,
            },
            SeedKind::GenericIsolatedRoot => RootIsolationProgram::CertifiedBernstein {
                maximum_depth: BERNSTEIN_ISOLATION_DEPTH_V1,
                ambiguity_depth: BERNSTEIN_AMBIGUITY_DEPTH_V1,
                preserve_all_positive_q_roots: true,
            },
        };
        let quadratic_composition = plan_quadratic_composition(root)?;
        let max_root_count = match feature.kind {
            FeatureKind::Plane => 1,
            FeatureKind::Quadric => 2,
            FeatureKind::Quartic => 4,
        };
        let influencing_params = coefficient_program.influencing_params(roots, &scalar_params)?;
        projective_features.push(ProjectiveFeature {
            feature: feature.id,
            root_equation,
            rational_program,
            q_degree,
            validity_predicates,
            orientation_program: feature.orientation,
            q_seed_kind,
            root_isolation,
            quadratic_composition,
            max_root_count,
            deformed_predictor: deformed,
            influencing_params,
        });
    }
    Ok(ProjectiveEquations {
        camera,
        coefficients: coefficient_program,
        polynomials,
        rationals,
        predicates,
        features: projective_features,
    })
}

pub fn evaluate_feature(
    equations: &ProjectiveEquations,
    feature: &ProjectiveFeature,
    scalar: &impl Fn(ScalarId) -> Result<f64, String>,
    camera: CameraSample,
    screen: ScreenPoint,
    q: f64,
) -> Result<f64, String> {
    super::camera::validate_sample_against_contract(equations.camera, camera)?;
    let camera_value = |source| {
        Ok(match source {
            CameraCoeff::Eye(component) => camera.eye[usize::from(component)],
            CameraCoeff::Forward(component) => camera.forward[usize::from(component)],
            CameraCoeff::Right(component) => camera.right[usize::from(component)],
            CameraCoeff::Up(component) => camera.up[usize::from(component)],
            CameraCoeff::EyeRate(_)
            | CameraCoeff::ForwardRate(_)
            | CameraCoeff::RightRate(_)
            | CameraCoeff::UpRate(_) => {
                return Err(
                    "pixels::projective: camera-rate coefficient escaped into a root equation"
                        .to_string(),
                );
            }
            CameraCoeff::TanHalfFovY => equations.camera.tan_half_fov_y,
            CameraCoeff::Aspect => equations.camera.aspect,
        })
    };
    let coefficients = equations.coefficients.evaluate(scalar, &camera_value)?;
    let coefficient = |id: CoeffId| {
        coefficients
            .get(id.index())
            .copied()
            .ok_or_else(|| format!("missing evaluated coefficient {id}"))
    };
    equations.polynomials[feature.root_equation.index()].direct_sum(
        &equations.coefficients,
        &coefficient,
        &|variable| {
            Ok(match variable {
                Var::U => screen.u,
                Var::V => screen.v,
                Var::Q => q,
                Var::X | Var::T | Var::Param(_) => 0.0,
            })
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pixels::arena::NodeOrigin;
    use crate::pixels::config::{RgbRangeConfig, ScalarRangeConfig, Vec3Config};
    use crate::pixels::graph::{FieldArena, FieldNode};
    use crate::pixels::ids::{FieldId, MaterialId, ObjectId};
    use crate::pixels::material_graph::MaterialArena;
    use crate::pixels::objects::{CsgExpr, IdentitySet};
    use crate::pixels::primitive::{FeatureKind, PredicateProgram as StructuralPredicate};
    use crate::pixels::scalar::{Dependency, ScalarArena, ScalarNode, ScalarOp};
    use crate::pixels::symbolic::SymbolicGraph;
    use crate::pixels::world_bounds::Aabb64;
    use crate::sema::types::Type;

    fn sample(
        primitive: Primitive,
        validity: AnalyticPredicate,
        kind: FeatureKind,
    ) -> (
        SymbolicGraph,
        ValueBounds,
        ObjectPartition,
        Vec<FeatureRecord>,
        super::super::config::RendererConfig,
    ) {
        let mut scalar = ScalarArena::new(1);
        for value in [0.0_f32, 1.0, 2.0, 0.5] {
            scalar
                .push(
                    ScalarNode {
                        op: ScalarOp::ConstF32(value.to_bits()),
                        dependency: Dependency::Constant,
                    },
                    NodeOrigin::synthetic("projective-test"),
                )
                .unwrap();
        }
        let mut fields = FieldArena::new(2);
        fields
            .push(
                FieldNode {
                    kind: FieldKind::Primitive(primitive),
                    scalar_value: ScalarId(0),
                },
                NodeOrigin::synthetic("projective-test"),
            )
            .unwrap();
        let graph = SymbolicGraph {
            renderer_index: 0,
            field_key: "field".to_string(),
            material_key: "material".to_string(),
            params_type: Type::Unit,
            material_type: Type::Unit,
            params: Vec::new(),
            scalar,
            fields,
            materials: MaterialArena::new(3),
            field_root: FieldId(0),
            material_root: MaterialId(0),
            obligations: Vec::new(),
            quota: Default::default(),
        };
        let config = super::super::config::RendererConfig {
            declaration_index: 0,
            worker_count: 1,
            params_type: Type::Unit,
            field: String::new(),
            material: String::new(),
            material_type: Type::Unit,
            display_index: 0,
            width: 64,
            height: 32,
            refresh_hz: 60,
            shade_hz: 60,
            profile: "AaaByteExact".to_string(),
            tone_curve: "Linear".to_string(),
            near: 0.1,
            far: 10.0,
            world_min: Vec3Config {
                x: -4.0,
                y: -4.0,
                z: -4.0,
            },
            world_max: Vec3Config {
                x: 4.0,
                y: 4.0,
                z: 4.0,
            },
            camera_pose: None,
            camera_max_motion: 0.0,
            light_capacity: 0,
            light_kinds: Vec::new(),
            exposure: ScalarRangeConfig { min: 0.0, max: 0.0 },
            environment: RgbRangeConfig {
                min: [0.0; 3],
                max: [0.0; 3],
            },
            ao_enabled: false,
            probes_enabled: false,
            probe_initialization_worst_case_ms: 0,
            initialization_deadline_ms: 1,
            parameter_contracts: Vec::new(),
        };
        let values = super::super::bounds::propagate(&graph, &config).unwrap();
        let bounds = Aabb64::new([-4.0; 3], [4.0; 3]).unwrap();
        let objects = ObjectPartition {
            objects: vec![SmoothObject {
                id: ObjectId(0),
                source_root: FieldId(0),
                scalar_root: ScalarId(0),
                bounds,
                primitive_occurrences: vec![vec![super::super::support::OccurrenceStep {
                    field: FieldId(0),
                    child_slot: 0,
                }]],
                support_max: F64Interval::point(0.0).unwrap(),
                identity_set: 0,
                repeat_instances: Vec::new(),
            }],
            identities: vec![IdentitySet {
                id: 0,
                pairs: Vec::new(),
            }],
            csg: CsgExpr::Leaf(ObjectId(0)),
        };
        let features = vec![FeatureRecord {
            id: FeatureId(0),
            template_id: 0,
            object: ObjectId(0),
            primitive: FieldId(0),
            occurrence_path: vec![super::super::support::OccurrenceStep {
                field: FieldId(0),
                child_slot: 0,
            }],
            kind,
            world_bounds: bounds,
            support_expand: 0.0,
            validity: StructuralPredicate {
                constraints: vec![validity],
                shared_boundary: true,
            },
            orientation: OrientationProgram::Outward,
            identity_set: 0,
            scalar_semantic_root: FieldId(0),
        }];
        (graph, values, objects, features, config)
    }

    fn ray_q_coefficients(
        equations: &ProjectiveEquations,
        values: &ValueBounds,
        u: f64,
        v: f64,
    ) -> Vec<F64Interval> {
        let root = &equations.polynomials[equations.features[0].root_equation.index()];
        let mut point_camera = equations.camera;
        point_camera.eye_component = point_camera
            .eye_component
            .map(|value| F64Interval::point(value.lo + value.width() * 0.5).unwrap());
        point_camera.forward_component = [
            F64Interval::point(0.0).unwrap(),
            F64Interval::point(0.0).unwrap(),
            F64Interval::point(1.0).unwrap(),
        ];
        point_camera.right_component = [
            F64Interval::point(1.0).unwrap(),
            F64Interval::point(0.0).unwrap(),
            F64Interval::point(0.0).unwrap(),
        ];
        point_camera.up_component = [
            F64Interval::point(0.0).unwrap(),
            F64Interval::point(1.0).unwrap(),
            F64Interval::point(0.0).unwrap(),
        ];
        let coefficient_intervals = coefficient_intervals_for_roots(
            &equations.coefficients,
            values,
            point_camera,
            root.terms.iter().map(|term| term.coefficient),
        )
        .unwrap();
        let zero = F64Interval::point(0.0).unwrap();
        let interval_power = |value: f64, exponent: u8| {
            (0..exponent).fold(F64Interval::point(1.0).unwrap(), |power, _| {
                power
                    .mul_outward(F64Interval::point(value).unwrap())
                    .unwrap()
            })
        };
        let mut q_coefficients = vec![zero; usize::from(root.degree_q) + 1];
        for term in &root.terms {
            assert_eq!(term.exponents.x, 0);
            assert_eq!(term.exponents.t, 0);
            assert_eq!(term.exponents.param_terms.count, 0);
            let coefficient = coefficient_intervals[term.coefficient.index()]
                .mul_outward(interval_power(u, term.exponents.u))
                .unwrap()
                .mul_outward(interval_power(v, term.exponents.v))
                .unwrap();
            let slot = &mut q_coefficients[usize::from(term.exponents.q)];
            *slot = slot.add_outward(coefficient).unwrap();
        }
        q_coefficients
    }

    fn center_ray_q_coefficients(
        equations: &ProjectiveEquations,
        values: &ValueBounds,
    ) -> Vec<F64Interval> {
        ray_q_coefficients(equations, values, 0.0, 0.0)
    }

    #[test]
    fn plane_inverse_depth_is_affine_and_matches_direct_equation() {
        let normal = [ScalarId(0), ScalarId(1), ScalarId(0)];
        let primitive = Primitive::Plane {
            normal,
            offset: ScalarId(0),
        };
        let (graph, values, objects, features, config) =
            sample(primitive, AnalyticPredicate::Always, FeatureKind::Plane);
        let equations = compile(&graph, &config, &values, &objects, &features).unwrap();
        let feature = &equations.features[0];
        assert_eq!(feature.q_degree, 1);
        let eye = equations
            .camera
            .eye_component
            .map(|interval| interval.lo + (interval.hi - interval.lo) * 0.5);
        let camera = CameraSample {
            eye,
            forward: [0.0, 0.0, 1.0],
            right: [1.0, 0.0, 0.0],
            up: [0.0, 1.0, 0.0],
        };
        let screen = ScreenPoint { u: 0.25, v: -0.5 };
        let scalar = |id: ScalarId| {
            Ok(match id.0 {
                0 => 0.0,
                1 => 1.0,
                2 => 2.0,
                3 => 0.5,
                _ => unreachable!(),
            })
        };
        let q = 0.25;
        let projective = evaluate_feature(&equations, feature, &scalar, camera, screen, q).unwrap();
        let point = equations.camera.point(camera, screen, q).unwrap();
        let direct = q * point[1];
        assert_eq!(projective, direct);
    }

    #[test]
    fn sphere_and_torus_emit_exact_expected_degrees() {
        let sphere = Primitive::Sphere {
            center: [ScalarId(0); 3],
            radius: ScalarId(1),
        };
        let (graph, values, objects, features, config) = sample(
            sphere,
            AnalyticPredicate::Sphere {
                center: [ScalarId(0); 3],
                radius: ScalarId(1),
            },
            FeatureKind::Quadric,
        );
        let equations = compile(&graph, &config, &values, &objects, &features).unwrap();
        assert_eq!(equations.features[0].q_degree, 2);

        let torus = Primitive::Torus {
            center: [ScalarId(0); 3],
            axis: [ScalarId(0), ScalarId(1), ScalarId(0)],
            major: ScalarId(2),
            minor: ScalarId(1),
        };
        let (graph, values, objects, features, config) = sample(
            torus,
            AnalyticPredicate::TorusDomain {
                center: [ScalarId(0); 3],
                axis: [ScalarId(0), ScalarId(1), ScalarId(0)],
                major: ScalarId(2),
                minor: ScalarId(1),
            },
            FeatureKind::Quartic,
        );
        let equations = compile(&graph, &config, &values, &objects, &features).unwrap();
        assert_eq!(equations.features[0].q_degree, 4);
        assert_eq!(equations.features[0].max_root_count, 4);
    }

    fn assert_actual_feature_composition(
        primitive: Primitive,
        validity: AnalyticPredicate,
        kind: FeatureKind,
    ) {
        let (graph, values, objects, features, config) = sample(primitive, validity, kind);
        let equations = compile(&graph, &config, &values, &objects, &features).unwrap();
        let feature = &equations.features[0];
        let CompositionPlan::Specialized(schedule) = &feature.quadratic_composition else {
            panic!("permanent plane/quadric/quartic feature selected composition fallback")
        };
        let root = &equations.polynomials[feature.root_equation.index()];
        assert_eq!(schedule.source_degree_u, root.degree_u);
        assert!(
            schedule.composed_degree
                >= root
                    .terms
                    .iter()
                    .map(|term| term.exponents.u + term.exponents.x + 2 * term.exponents.q)
                    .max()
                    .unwrap_or(0)
        );
        let camera = CameraSample {
            eye: equations
                .camera
                .eye_component
                .map(|value| value.lo + value.width() * 0.5),
            forward: [0.0, 0.0, 1.0],
            right: [1.0, 0.0, 0.0],
            up: [0.0, 1.0, 0.0],
        };
        let scalar = |id: ScalarId| {
            Ok(match id.0 {
                0 => 0.0,
                1 => 1.0,
                2 => 2.0,
                3 => 0.5,
                _ => unreachable!(),
            })
        };
        let camera_value = |source| {
            Ok(match source {
                CameraCoeff::Eye(component) => camera.eye[usize::from(component)],
                CameraCoeff::Forward(component) => camera.forward[usize::from(component)],
                CameraCoeff::Right(component) => camera.right[usize::from(component)],
                CameraCoeff::Up(component) => camera.up[usize::from(component)],
                CameraCoeff::TanHalfFovY => equations.camera.tan_half_fov_y,
                CameraCoeff::Aspect => equations.camera.aspect,
                CameraCoeff::EyeRate(_)
                | CameraCoeff::ForwardRate(_)
                | CameraCoeff::RightRate(_)
                | CameraCoeff::UpRate(_) => 0.0,
            })
        };
        let values = equations
            .coefficients
            .evaluate(&scalar, &camera_value)
            .unwrap();
        let coefficient = |id: CoeffId| Ok(values[id.index()]);
        let u0 = -0.375;
        let x = 0.5;
        let v = 0.125;
        let q_hat = [0.75, -0.125, 0.0625];
        let composed = super::super::polynomial::evaluate_quadratic_composition(
            root,
            schedule,
            &equations.coefficients,
            &coefficient,
            &|variable| {
                Ok(match variable {
                    Var::U => u0,
                    Var::V => v,
                    Var::T | Var::Param(_) => 0.0,
                    Var::Q | Var::X => unreachable!(),
                })
            },
            x,
            q_hat,
        )
        .unwrap();
        let q = q_hat[0] + x * (q_hat[1] + x * q_hat[2]);
        let direct = root
            .direct_sum(&equations.coefficients, &coefficient, &|variable| {
                Ok(match variable {
                    Var::U => u0 + x,
                    Var::V => v,
                    Var::Q => q,
                    Var::X => x,
                    Var::T | Var::Param(_) => 0.0,
                })
            })
            .unwrap();
        assert!(
            (composed - direct).abs() <= 64.0 * f64::EPSILON * direct.abs().max(1.0),
            "actual {kind:?} composition omitted an affine U contribution"
        );
    }

    #[test]
    fn actual_feature_programs_compose_u_as_the_local_scanline_coordinate() {
        assert_actual_feature_composition(
            Primitive::Plane {
                normal: [ScalarId(1), ScalarId(0), ScalarId(1)],
                offset: ScalarId(1),
            },
            AnalyticPredicate::Always,
            FeatureKind::Plane,
        );
        assert_actual_feature_composition(
            Primitive::Sphere {
                center: [ScalarId(0); 3],
                radius: ScalarId(1),
            },
            AnalyticPredicate::Sphere {
                center: [ScalarId(0); 3],
                radius: ScalarId(1),
            },
            FeatureKind::Quadric,
        );
        assert_actual_feature_composition(
            Primitive::Torus {
                center: [ScalarId(0); 3],
                axis: [ScalarId(0), ScalarId(1), ScalarId(0)],
                major: ScalarId(2),
                minor: ScalarId(1),
            },
            AnalyticPredicate::TorusDomain {
                center: [ScalarId(0); 3],
                axis: [ScalarId(0), ScalarId(1), ScalarId(0)],
                major: ScalarId(2),
                minor: ScalarId(1),
            },
            FeatureKind::Quartic,
        );
    }

    #[test]
    fn lowered_torus_preserves_four_ordered_positive_roots() {
        let torus = Primitive::Torus {
            center: [ScalarId(0); 3],
            axis: [ScalarId(0), ScalarId(1), ScalarId(0)],
            major: ScalarId(2),
            minor: ScalarId(1),
        };
        let (graph, values, objects, features, config) = sample(
            torus,
            AnalyticPredicate::TorusDomain {
                center: [ScalarId(0); 3],
                axis: [ScalarId(0), ScalarId(1), ScalarId(0)],
                major: ScalarId(2),
                minor: ScalarId(1),
            },
            FeatureKind::Quartic,
        );
        let equations = compile(&graph, &config, &values, &objects, &features).unwrap();
        assert!(matches!(
            equations.features[0].root_isolation,
            RootIsolationProgram::CertifiedBernstein {
                preserve_all_positive_q_roots: true,
                ..
            }
        ));
        // The center ray has u=v=0. These coefficients come from the actual
        // lowered torus program rather than a hand-written quartic.
        let q_coefficients = center_ray_q_coefficients(&equations, &values);
        let roots = super::super::reference::polynomial::isolate_all_roots_bernstein(
            &q_coefficients,
            equations.camera.q,
            BERNSTEIN_ISOLATION_DEPTH_V1,
            BERNSTEIN_AMBIGUITY_DEPTH_V1,
        )
        .unwrap_or_else(|error| panic!("{error}; coefficients={q_coefficients:?}"));
        let expected = [1.0 / 7.1, 1.0 / 5.1, 1.0 / 3.1, 1.0 / 1.1];
        assert_eq!(roots.len(), expected.len(), "{roots:?}");
        for (root, expected) in roots.iter().zip(expected) {
            assert!(
                root.contains(expected),
                "{root:?} does not contain {expected}"
            );
        }
        assert!(
            roots.windows(2).all(|pair| pair[0].hi <= pair[1].lo),
            "root intervals are not ordered and disjoint: {roots:?}"
        );

        let two_root_ray = [0.125, 0.25, 0.5, 0.75, 1.0]
            .into_iter()
            .find_map(|u| {
                let coefficients = ray_q_coefficients(&equations, &values, u, 0.0);
                let roots = super::super::reference::polynomial::isolate_all_roots_bernstein(
                    &coefficients,
                    equations.camera.q,
                    BERNSTEIN_ISOLATION_DEPTH_V1,
                    BERNSTEIN_AMBIGUITY_DEPTH_V1,
                )
                .ok()?;
                (roots.len() == 2).then_some((u, roots))
            })
            .expect("a ray outside the torus hole must retain the outer two-root sheet");
        assert_eq!(two_root_ray.1.len(), 2);
    }

    #[test]
    fn lowered_tangent_torus_retains_the_even_multiplicity_root_corridor() {
        let torus = Primitive::Torus {
            center: [ScalarId(0); 3],
            axis: [ScalarId(0), ScalarId(1), ScalarId(0)],
            major: ScalarId(2),
            minor: ScalarId(1),
        };
        let (graph, values, objects, features, mut config) = sample(
            torus,
            AnalyticPredicate::TorusDomain {
                center: [ScalarId(0); 3],
                axis: [ScalarId(0), ScalarId(1), ScalarId(0)],
                major: ScalarId(2),
                minor: ScalarId(1),
            },
            FeatureKind::Quartic,
        );
        // Put the canonical center ray at x=major+minor=3. It is tangent to
        // the outer torus circle at z=0, producing one positive double root.
        config.world_min.x = 2.0;
        config.world_max.x = 4.0;
        let equations = compile(&graph, &config, &values, &objects, &features).unwrap();
        let coefficients = center_ray_q_coefficients(&equations, &values);
        let corridors = super::super::reference::polynomial::isolate_all_roots_bernstein(
            &coefficients,
            equations.camera.q,
            BERNSTEIN_ISOLATION_DEPTH_V1,
            BERNSTEIN_AMBIGUITY_DEPTH_V1,
        )
        .unwrap();
        let tangent_q = 1.0 / 4.1;
        assert!(
            corridors
                .iter()
                .any(|corridor| corridor.contains(tangent_q)),
            "tangent root {tangent_q} was not retained in {corridors:?}"
        );
        assert!(matches!(
            equations.features[0].root_isolation,
            RootIsolationProgram::CertifiedBernstein {
                preserve_all_positive_q_roots: true,
                ..
            }
        ));
    }

    #[test]
    fn stable_quadratic_seeds_avoid_cancellation_and_remain_untrusted() {
        let candidates = stable_quadratic_candidates(1.0, 1.0e16, 1.0).unwrap();
        assert_eq!(candidates.roots.len(), 2);
        assert!((candidates.roots[0] + 1.0e16).abs() < 2.0);
        assert!((candidates.roots[1] + 1.0e-16).abs() < 1.0e-30);
        assert!(!candidates.used_linear_fallback);
        assert!(!candidates.requires_generic_isolation);

        let linear = stable_quadratic_candidates(0.0, 2.0, -4.0).unwrap();
        assert_eq!(linear.roots, vec![2.0]);
        assert!(linear.used_linear_fallback);
        assert!(!linear.requires_generic_isolation);

        let negative = stable_quadratic_candidates(1.0, 0.0, 1.0).unwrap();
        assert!(negative.roots.is_empty());
        assert!(negative.discriminant < 0.0);
        assert!(negative.requires_generic_isolation);

        // The point discriminant rounds negative although the coefficients
        // are a near-tangent candidate. Candidate arithmetic must retain the
        // generic verifier path instead of concluding that no root exists.
        let rounded_tangent = stable_quadratic_candidates(1.0, 2.0, 1.0 + f64::EPSILON).unwrap();
        assert!(rounded_tangent.discriminant < 0.0);
        assert!(rounded_tangent.requires_generic_isolation);
    }

    #[test]
    fn quadratic_seed_records_uncertain_leading_coefficient_and_both_fallbacks() {
        let mut coefficients = CoeffBuilder::new();
        let leading = coefficients.scalar(ScalarId(0)).unwrap();
        let root = PolyBuilder::monomial(
            leading,
            super::super::polynomial::Exponents {
                q: 2,
                ..super::super::polynomial::Exponents::default()
            },
            coefficients.program(),
        )
        .finish(PolyProgramId(0), ProgramLimit::Feature)
        .unwrap();
        let uncertain = F64Interval::new(-1.0, 1.0).unwrap();
        assert_eq!(
            select_seed_kind(2, &root, &[uncertain]),
            SeedKind::StableQuadratic {
                leading_coefficient: leading,
                leading_enclosure: uncertain,
                leading_sign: None,
                linear_fallback: true,
                generic_isolation_fallback: true,
            }
        );
        let positive = F64Interval::new(1.0, 2.0).unwrap();
        assert_eq!(
            select_seed_kind(2, &root, &[positive]),
            SeedKind::StableQuadratic {
                leading_coefficient: leading,
                leading_enclosure: positive,
                leading_sign: Some(StrictSign::Positive),
                linear_fallback: false,
                generic_isolation_fallback: true,
            }
        );
    }

    fn assert_projective_zero(
        primitive: Primitive,
        validity: AnalyticPredicate,
        kind: FeatureKind,
        point: [f64; 3],
    ) {
        let (graph, values, objects, features, config) = sample(primitive, validity, kind);
        let equations = compile(&graph, &config, &values, &objects, &features).unwrap();
        let eye = equations
            .camera
            .eye_component
            .map(|interval| interval.lo + (interval.hi - interval.lo) * 0.5);
        let camera = CameraSample {
            eye,
            forward: [0.0, 0.0, 1.0],
            right: [1.0, 0.0, 0.0],
            up: [0.0, 1.0, 0.0],
        };
        let q = 1.0 / (point[2] - eye[2]);
        let screen = ScreenPoint {
            u: q * (point[0] - eye[0]),
            v: q * (point[1] - eye[1]),
        };
        let scalar = |id: ScalarId| {
            Ok(match id.0 {
                0 => 0.0,
                1 => 1.0,
                2 => 2.0,
                3 => 0.5,
                _ => unreachable!(),
            })
        };
        let value = evaluate_feature(
            &equations,
            &equations.features[0],
            &scalar,
            camera,
            screen,
            q,
        )
        .unwrap();
        assert!(
            value.abs() <= 1.0e-12,
            "projective feature equation differs at {point:?}: {value}"
        );
    }

    #[test]
    fn all_planar_quadric_and_quartic_feature_classes_match_direct_zeros() {
        assert_projective_zero(
            Primitive::Box {
                center: [ScalarId(0); 3],
                half: [ScalarId(1); 3],
            },
            AnalyticPredicate::BoxFace {
                axis: 2,
                sign: 1,
                center: [ScalarId(0); 3],
                half: [ScalarId(1); 3],
            },
            FeatureKind::Plane,
            [0.25, -0.5, 1.0],
        );
        assert_projective_zero(
            Primitive::RoundBox {
                center: [ScalarId(0); 3],
                half: [ScalarId(1); 3],
                radius: ScalarId(3),
            },
            AnalyticPredicate::RoundBoxCorner {
                signs: [1, 1, 1],
                center: [ScalarId(0); 3],
                half: [ScalarId(1); 3],
                radius: ScalarId(3),
            },
            FeatureKind::Quadric,
            [1.5, 1.0, 1.0],
        );
        for primitive in [
            Primitive::Capsule {
                a: [ScalarId(0); 3],
                b: [ScalarId(0), ScalarId(2), ScalarId(0)],
                radius: ScalarId(1),
            },
            Primitive::FiniteCylinder {
                a: [ScalarId(0); 3],
                b: [ScalarId(0), ScalarId(2), ScalarId(0)],
                radius: ScalarId(1),
            },
        ] {
            assert_projective_zero(
                primitive,
                AnalyticPredicate::SegmentSide {
                    a: [ScalarId(0); 3],
                    b: [ScalarId(0), ScalarId(2), ScalarId(0)],
                    radius_a: ScalarId(1),
                    radius_b: ScalarId(1),
                },
                FeatureKind::Quadric,
                [1.0, 1.0, 0.0],
            );
        }
        assert_projective_zero(
            Primitive::Capsule {
                a: [ScalarId(0); 3],
                b: [ScalarId(0), ScalarId(2), ScalarId(0)],
                radius: ScalarId(1),
            },
            AnalyticPredicate::SegmentCap {
                endpoint: 0,
                a: [ScalarId(0); 3],
                b: [ScalarId(0), ScalarId(2), ScalarId(0)],
                radius: ScalarId(1),
                hemisphere: true,
            },
            FeatureKind::Quadric,
            [0.0, -1.0, 0.0],
        );
        assert_projective_zero(
            Primitive::FiniteCylinder {
                a: [ScalarId(0); 3],
                b: [ScalarId(0), ScalarId(2), ScalarId(0)],
                radius: ScalarId(1),
            },
            AnalyticPredicate::SegmentCap {
                endpoint: 0,
                a: [ScalarId(0); 3],
                b: [ScalarId(0), ScalarId(2), ScalarId(0)],
                radius: ScalarId(1),
                hemisphere: false,
            },
            FeatureKind::Plane,
            [0.5, 0.0, 0.0],
        );
        assert_projective_zero(
            Primitive::FiniteCone {
                a: [ScalarId(0); 3],
                b: [ScalarId(0), ScalarId(2), ScalarId(0)],
                radius_a: ScalarId(1),
                radius_b: ScalarId(3),
            },
            AnalyticPredicate::SegmentSide {
                a: [ScalarId(0); 3],
                b: [ScalarId(0), ScalarId(2), ScalarId(0)],
                radius_a: ScalarId(1),
                radius_b: ScalarId(3),
            },
            FeatureKind::Quadric,
            [0.75, 1.0, 0.0],
        );
        assert_projective_zero(
            Primitive::Torus {
                center: [ScalarId(0); 3],
                axis: [ScalarId(0), ScalarId(1), ScalarId(0)],
                major: ScalarId(2),
                minor: ScalarId(1),
            },
            AnalyticPredicate::TorusDomain {
                center: [ScalarId(0); 3],
                axis: [ScalarId(0), ScalarId(1), ScalarId(0)],
                major: ScalarId(2),
                minor: ScalarId(1),
            },
            FeatureKind::Quartic,
            [3.0, 0.0, 0.0],
        );
    }
}
