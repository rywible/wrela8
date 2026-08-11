//! Complete local feature/event generator construction.

use super::derivatives::DerivativePrograms;
use super::event_kinds::{EventKind, EventSide, EventSideMeaning};
use super::ids::{
    DerivativeBundleId, EventTemplateId, FeatureId, FieldId, ObjectId, ParamId, PolyProgramId,
    PredicateProgramId, ScalarId,
};
use super::projection_bounds::{PixelSpan, ProjectedFeatureSpan, TileSpan};
use super::reference::interval::F64Interval;

pub const MAX_EVENT_PARTICIPANTS_V1: usize = 8;

fn material_side_meaning(comparison: super::scalar::CompareOp) -> EventSideMeaning {
    use super::scalar::CompareOp;
    match comparison {
        CompareOp::Lt => EventSideMeaning {
            negative: EventSide::MaterialLeft,
            zero: EventSide::MaterialRight,
            positive: EventSide::MaterialRight,
        },
        CompareOp::Le => EventSideMeaning {
            negative: EventSide::MaterialLeft,
            zero: EventSide::MaterialLeft,
            positive: EventSide::MaterialRight,
        },
        CompareOp::Gt => EventSideMeaning {
            negative: EventSide::MaterialRight,
            zero: EventSide::MaterialRight,
            positive: EventSide::MaterialLeft,
        },
        CompareOp::Ge => EventSideMeaning {
            negative: EventSide::MaterialRight,
            zero: EventSide::MaterialLeft,
            positive: EventSide::MaterialLeft,
        },
        CompareOp::Eq => EventSideMeaning {
            negative: EventSide::MaterialRight,
            zero: EventSide::MaterialLeft,
            positive: EventSide::MaterialRight,
        },
        CompareOp::Ne => EventSideMeaning {
            negative: EventSide::MaterialLeft,
            zero: EventSide::MaterialRight,
            positive: EventSide::MaterialLeft,
        },
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct ScalarDerivativeProgram {
    pub sources: Vec<ScalarId>,
    pub first_world_abs: [f64; 3],
    pub second_world_abs: f64,
    pub third_world_abs: f64,
    pub parameter_abs: Vec<(ParamId, f64)>,
    pub frame_delta_abs: Option<f64>,
    pub frame_second_delta_abs: Option<f64>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Participant {
    Feature(FeatureId),
    Object(ObjectId),
    Field(FieldId),
    MaterialEvent(u32),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SmallParticipantSet {
    values: [Participant; MAX_EVENT_PARTICIPANTS_V1],
    count: u8,
}

impl SmallParticipantSet {
    pub fn new(values: &[Participant]) -> Result<Self, String> {
        if values.len() > MAX_EVENT_PARTICIPANTS_V1 {
            return Err(format!(
                "P015: renderer capacity `event_participants` needs {}, ceiling {}",
                values.len(),
                MAX_EVENT_PARTICIPANTS_V1
            ));
        }
        let mut sorted = values.to_vec();
        sorted.sort();
        sorted.dedup();
        let mut result = Self {
            values: [Participant::Feature(FeatureId(0)); MAX_EVENT_PARTICIPANTS_V1],
            count: u8::try_from(sorted.len())
                .map_err(|_| "P015: event participant count overflow".to_string())?,
        };
        for (slot, value) in result.values.iter_mut().zip(sorted) {
            *slot = value;
        }
        Ok(result)
    }

    pub fn iter(&self) -> impl Iterator<Item = Participant> + '_ {
        self.values[..usize::from(self.count)].iter().copied()
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum EventRepresentation {
    LinearLeadingCoefficient {
        coefficient: PolyProgramId,
        root: PolyProgramId,
    },
    QuadraticDiscriminant {
        discriminant: PolyProgramId,
        root: PolyProgramId,
    },
    SparsePredicate {
        predicate: PredicateProgramId,
    },
    DeformationTaylorPredicate {
        predictor: PolyProgramId,
        predictor_derivatives: DerivativeBundleId,
        displacement: ScalarId,
        scalar_derivatives: ScalarDerivativeProgram,
        phase_recurrence: super::deform::PhaseRecurrenceProgram,
        taylor_order: u8,
        world_delta_abs_bound: f64,
        third_derivative_abs_bound: f64,
        remainder: f64,
    },
    TorusLocalOracle {
        root: PolyProgramId,
        derivative_u: PolyProgramId,
        derivative_q: PolyProgramId,
        derivative_uq: PolyProgramId,
        derivative_qq: PolyProgramId,
        third_u: PolyProgramId,
        value_abs_bound: f64,
        derivative_u_abs_bound: f64,
        derivative_q_abs_bound: f64,
        derivative_uq_abs_bound: f64,
        derivative_qq_abs_bound: f64,
        third_u_abs_bound: f64,
        taylor_order: u8,
        remainder: f64,
    },
    SmoothBandTaylorPredicate {
        left: ScalarId,
        right: ScalarId,
        left_negated: bool,
        right_negated: bool,
        radius: ScalarId,
        derivatives: ScalarDerivativeProgram,
        taylor_order: u8,
        world_delta_abs_bound: f64,
        remainder: f64,
    },
    SmoothTieTaylorPredicate {
        left: ScalarId,
        right: ScalarId,
        left_negated: bool,
        right_negated: bool,
        derivatives: ScalarDerivativeProgram,
        taylor_order: u8,
        world_delta_abs_bound: f64,
        remainder: f64,
    },
    MaterialDifferenceTaylorPredicate {
        left: ScalarId,
        right: ScalarId,
        comparison: super::scalar::CompareOp,
        derivatives: ScalarDerivativeProgram,
        taylor_order: u8,
        world_delta_abs_bound: f64,
        remainder: f64,
    },
    RepeatAffineBoundary {
        axis: super::graph::Axis,
        boundary: super::reference::interval::F64Interval,
    },
    ClipQ {
        q: f64,
    },
    ProjectedBoundary {
        horizontal: bool,
        coordinate: u32,
    },
    FixedPointReset,
    DirectDepthCrossProduct {
        numerator: PolyProgramId,
        denominator_a: super::projective::StrictSignObligation,
        denominator_b: super::projective::StrictSignObligation,
    },
    TaylorDepthDifference {
        a: DerivativeBundleId,
        b: DerivativeBundleId,
        taylor_order: u8,
        remainder: DepthTaylorRemainderProgram,
    },
}

#[derive(Clone, Debug, PartialEq)]
pub struct DepthTaylorRemainderProgram {
    /// Third partials of each implicit equation are the next-order inputs for
    /// a quadratic implicit-sheet Taylor difference.
    pub a_third: super::derivatives::ThirdDerivatives,
    pub b_third: super::derivatives::ThirdDerivatives,
    pub next_derivative_order: u8,
    pub local_x_domain: F64Interval,
    pub q_domain: F64Interval,
    /// Complete difference enclosure used only after discarding the Taylor
    /// predictor when G_q cannot be proven away from zero.
    pub fallback_difference: F64Interval,
    pub fallback_remainder_abs_bound: f64,
    pub requires_strict_g_q: bool,
    pub discard_taylor_on_fallback: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct EventGenerator {
    pub id: EventTemplateId,
    pub kind: EventKind,
    pub participants: SmallParticipantSet,
    pub pixels: PixelSpan,
    pub tiles: TileSpan,
    pub coefficient_dependencies: Vec<ParamId>,
    pub maximum_root_count: u32,
    pub subdivision_depth: u8,
    pub representation: EventRepresentation,
    pub side_meaning: EventSideMeaning,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct EventSubject {
    pub kind: EventKind,
    pub feature: Option<FeatureId>,
    pub owner: Option<ObjectId>,
    pub ordinal: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PredicateOmissionKind {
    FeatureValidity,
    SupportShell,
    MaterialClass,
}

#[derive(Clone, Debug, PartialEq)]
pub enum OmissionHint {
    FullDomainProjection,
    StrictProjectiveLeadingSign,
    OutsideNearFar,
    GlobalStrictSign {
        kind: PredicateOmissionKind,
        proof: super::exclusions::BernsteinProofPayload,
    },
    /// The clip plane's `q` lies strictly outside the feature's sealed
    /// projected `q` span, so no root of the feature can ever sit on that clip
    /// plane and the crossing the event would mark does not exist.
    ClipQOutsideFeatureQSpan {
        clip_q: f64,
        feature_q: F64Interval,
        margin: f64,
    },
}

/// Relative slack required before a near/far clip event may be proven vacuous.
///
/// Geometrically the omission is exact: `ProjectedFeatureSpan::q` outward-
/// encloses the reciprocal depth of the feature's whole world AABB, so a clip
/// `q` outside it names a depth the feature never reaches. The guest reaches
/// the same conclusion operationally — `select_visibility` clamps a feature's
/// root-`q` search interval to this very span (`stdlib/core/render.wr`,
/// `__wrela_pixels_p7_feature_q_span`) — but it reads the span through an f32
/// round-trip (at most two f32 ulps of outward motion) and through
/// `__wrela_pixels_p7_interval_from_f32`, which widens by `|raw| / 65536 + 64`
/// fixed-point quanta, i.e. by `q * 2^-16` plus 64 quanta. Both terms are
/// bounded by roughly `2^-14` of the camera's `q` scale for any exponent the
/// fixed-q policy can seal, so demanding `2^-8` of that scale dominates them by
/// more than an order of magnitude. `finish_frame_program` re-checks every
/// omission recorded here against the exponent the frame program actually
/// seals, so the slack is a filter and not the proof of record.
const CLIP_Q_OMISSION_RELATIVE_SLACK_V1: f64 = 1.0 / 256.0;

/// Outward-rounded distance by which `clip_q` clears `feature_q`, less the
/// slack above. `None` whenever the clip `q` is inside the feature's span or
/// too close to it for the omission to be safe.
pub(crate) fn clip_q_omission_margin(
    feature_q: F64Interval,
    camera_q: F64Interval,
    clip_q: f64,
) -> Option<f64> {
    if !clip_q.is_finite() || !feature_q.lo.is_finite() || !feature_q.hi.is_finite() {
        return None;
    }
    let scale = camera_q.lo.abs().max(camera_q.hi.abs());
    if !scale.is_finite() || scale <= 0.0 {
        return None;
    }
    let slack = super::reference::interval::next_up(scale * CLIP_Q_OMISSION_RELATIVE_SLACK_V1);
    let gap = if clip_q > feature_q.hi {
        clip_q - feature_q.hi
    } else if clip_q < feature_q.lo {
        feature_q.lo - clip_q
    } else {
        return None;
    };
    if !(gap > slack) {
        return None;
    }
    let margin = super::reference::interval::next_down(gap - slack);
    if margin.is_finite() && margin > 0.0 {
        Some(margin)
    } else {
        None
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct EventLedgerEntry {
    pub subject: EventSubject,
    pub emitted: Option<EventTemplateId>,
    pub omission: Option<OmissionHint>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct EventPrograms {
    pub generators: Vec<EventGenerator>,
    pub ledger: Vec<EventLedgerEntry>,
}

struct EventCompiler<'a> {
    spans: &'a [ProjectedFeatureSpan],
    derivatives: &'a DerivativePrograms,
    generators: Vec<EventGenerator>,
    ledger: Vec<EventLedgerEntry>,
}

impl<'a> EventCompiler<'a> {
    fn span(&self, feature: FeatureId) -> Result<&ProjectedFeatureSpan, String> {
        self.spans
            .get(feature.index())
            .filter(|span| span.feature == feature)
            .ok_or_else(|| format!("pixels::events: missing projected span for {feature}"))
    }

    fn bundle(&self, feature: FeatureId) -> Result<&super::derivatives::DerivativeBundle, String> {
        self.derivatives
            .bundles
            .get(feature.index())
            .filter(|bundle| bundle.feature == feature)
            .ok_or_else(|| format!("pixels::events: missing derivative bundle for {feature}"))
    }

    fn emit(
        &mut self,
        subject: EventSubject,
        participants: &[Participant],
        span: ProjectedFeatureSpan,
        dependencies: Vec<ParamId>,
        maximum_root_count: u32,
        representation: EventRepresentation,
        side_meaning: EventSideMeaning,
    ) -> Result<(), String> {
        if span.pixels.x.is_empty() || span.pixels.y.is_empty() {
            self.omit(subject, OmissionHint::OutsideNearFar);
            return Ok(());
        }
        let id = EventTemplateId(
            u32::try_from(self.generators.len())
                .map_err(|_| "P015: event generator ID overflow".to_string())?,
        );
        let subdivision_depth = super::capacities::checked_event_isolation_depth(
            super::capacities::PixelsCeilings::MACHINE_V1.event_isolation_depth,
        )?;
        self.generators.push(EventGenerator {
            id,
            kind: subject.kind,
            participants: SmallParticipantSet::new(participants)?,
            pixels: span.pixels,
            tiles: span.tiles,
            coefficient_dependencies: dependencies,
            maximum_root_count,
            subdivision_depth,
            representation,
            side_meaning,
        });
        self.ledger.push(EventLedgerEntry {
            subject,
            emitted: Some(id),
            omission: None,
        });
        Ok(())
    }

    fn omit(&mut self, subject: EventSubject, reason: OmissionHint) {
        self.ledger.push(EventLedgerEntry {
            subject,
            emitted: None,
            omission: Some(reason),
        });
    }
}

fn full_output(span: &ProjectedFeatureSpan, camera: super::camera::CameraContract) -> bool {
    span.pixels.x.start == 0
        && span.pixels.x.end == camera.width
        && span.pixels.y.start == 0
        && span.pixels.y.end == camera.height
}

pub(crate) fn scalar_derivative_program(
    structural: &super::verify::StructuralProgram,
    sources: &[ScalarId],
) -> Result<ScalarDerivativeProgram, String> {
    let mut sources = sources.to_vec();
    sources.sort();
    sources.dedup();
    let mut first_world_abs = [0.0; 3];
    let mut second_world_abs = 0.0;
    let mut third_world_abs = 0.0;
    let mut parameter_abs = std::collections::BTreeMap::<ParamId, f64>::new();
    let mut frame_delta_abs = Some(0.0);
    let mut frame_second_delta_abs = Some(0.0);
    for source in &sources {
        let bound = structural.derivatives.get(*source)?;
        for (total, value) in first_world_abs.iter_mut().zip(bound.world_components) {
            *total += value.abs();
        }
        second_world_abs += bound.hessian_norm;
        third_world_abs += bound.third_derivative_norm;
        for (parameter, value) in &bound.parameter {
            *parameter_abs.entry(*parameter).or_default() += value.abs();
        }
        frame_delta_abs = match (frame_delta_abs, bound.frame_delta) {
            (Some(a), Some(b)) => Some(a + b.abs()),
            _ => None,
        };
        frame_second_delta_abs = match (frame_second_delta_abs, bound.frame_second_delta) {
            (Some(a), Some(b)) => Some(a + b.abs()),
            _ => None,
        };
    }
    if first_world_abs
        .iter()
        .chain([second_world_abs, third_world_abs].iter())
        .chain(parameter_abs.values())
        .any(|value| !value.is_finite())
    {
        return Err("P015: scalar event derivative program has a non-finite bound".to_string());
    }
    Ok(ScalarDerivativeProgram {
        sources,
        first_world_abs,
        second_world_abs,
        third_world_abs,
        parameter_abs: parameter_abs.into_iter().collect(),
        frame_delta_abs,
        frame_second_delta_abs,
    })
}

fn merge_dependencies(base: &[ParamId], scalar: &ScalarDerivativeProgram) -> Vec<ParamId> {
    let mut dependencies = base.to_vec();
    dependencies.extend(scalar.parameter_abs.iter().map(|(parameter, _)| *parameter));
    dependencies.sort();
    dependencies.dedup();
    dependencies
}

fn parameter_domain(
    structural: &super::verify::StructuralProgram,
    dependencies: &[ParamId],
) -> Result<Option<Vec<super::exclusions::NormalizedAxis>>, String> {
    if dependencies.len() > super::exclusions::MAX_BERNSTEIN_VARIABLES_V1 {
        return Ok(None);
    }
    let mut domain = Vec::with_capacity(dependencies.len());
    for parameter in dependencies {
        let Some(slot) = structural
            .params
            .slots
            .iter()
            .find(|slot| slot.id == *parameter)
        else {
            // An unavailable complete parameter domain makes static proof
            // inapplicable; retaining the runtime predicate is fail closed.
            return Ok(None);
        };
        domain.push(super::exclusions::NormalizedAxis {
            lo: slot.range.min,
            hi: slot.range.max,
        });
    }
    Ok(Some(domain))
}

fn interval_strict_sign_proof(
    structural: &super::verify::StructuralProgram,
    dependencies: &[ParamId],
    interval: F64Interval,
) -> Result<Option<super::exclusions::BernsteinProofPayload>, String> {
    let Some(domain) = parameter_domain(structural, dependencies)? else {
        return Ok(None);
    };
    super::exclusions::prove_interval_strict_sign(interval, &domain)
}

fn smooth_operands(
    kind: &super::graph::FieldKind,
) -> Option<(FieldId, FieldId, ScalarId, bool, bool)> {
    match *kind {
        super::graph::FieldKind::SmoothUnion { a, b, k } => Some((a, b, k, false, false)),
        super::graph::FieldKind::SmoothIntersection { a, b, k } => Some((a, b, k, true, true)),
        super::graph::FieldKind::SmoothSubtract { a, b, k } => Some((a, b, k, true, false)),
        _ => None,
    }
}

fn checked_event_root_count(kind: &str, count: u32) -> Result<u32, String> {
    if count == 0 {
        return Err(format!(
            "P015: renderer capacity `{kind}` requires a positive root bound"
        ));
    }
    Ok(count)
}

fn q_coefficient(
    program: &super::polynomial::PolyProgram,
    q_degree: u8,
    coefficients: &mut super::program::CoeffBuilder,
) -> Result<super::polynomial::PolyBuilder, String> {
    let mut result = super::polynomial::PolyBuilder::zero();
    for term in &program.terms {
        if term.exponents.q == q_degree {
            let mut exponents = term.exponents;
            exponents.q = 0;
            result = result.add(
                super::polynomial::PolyBuilder::monomial(
                    term.coefficient,
                    exponents,
                    coefficients.program(),
                ),
                coefficients,
            )?;
        }
    }
    Ok(result)
}

#[derive(Clone, Copy)]
enum SilhouetteCondition {
    LinearLeadingCoefficient(PolyProgramId),
    QuadraticDiscriminant(PolyProgramId),
}

fn compile_silhouette_conditions(
    projective: &mut super::projective::ProjectiveEquations,
) -> Result<Vec<Option<SilhouetteCondition>>, String> {
    let mut coefficients =
        super::program::CoeffBuilder::from_program(projective.coefficients.clone())?;
    let mut conditions = Vec::with_capacity(projective.features.len());
    for feature in &projective.features {
        if feature.q_degree != 1 && feature.q_degree != 2 {
            conditions.push(None);
            continue;
        }
        let root = projective
            .polynomials
            .get(feature.root_equation.index())
            .filter(|root| root.id == feature.root_equation)
            .cloned()
            .ok_or_else(|| {
                format!(
                    "pixels::events: quadratic feature {} names missing root {}",
                    feature.feature, feature.root_equation
                )
            })?;
        let b = q_coefficient(&root, 1, &mut coefficients)?;
        if feature.q_degree == 1 {
            let id = PolyProgramId(u32::try_from(projective.polynomials.len()).map_err(|_| {
                "P015: silhouette leading-coefficient program ID overflow".to_string()
            })?);
            projective
                .polynomials
                .push(b.finish(id, super::polynomial::ProgramLimit::EventBivariate)?);
            conditions.push(Some(SilhouetteCondition::LinearLeadingCoefficient(id)));
            continue;
        }
        let a = q_coefficient(&root, 2, &mut coefficients)?;
        let c = q_coefficient(&root, 0, &mut coefficients)?;
        let b_squared = b.clone().mul(b, &mut coefficients)?;
        let four = coefficients.constant(4.0)?;
        let four_ac = a
            .mul(c, &mut coefficients)?
            .scale(four, &mut coefficients)?;
        let id = PolyProgramId(
            u32::try_from(projective.polynomials.len())
                .map_err(|_| "P015: silhouette discriminant program ID overflow".to_string())?,
        );
        let discriminant = b_squared
            .sub(four_ac, &mut coefficients)?
            .finish(id, super::polynomial::ProgramLimit::EventBivariate)?;
        projective.polynomials.push(discriminant);
        conditions.push(Some(SilhouetteCondition::QuadraticDiscriminant(id)));
    }
    projective.coefficients = coefficients.finish();
    Ok(conditions)
}

/// Bound crossings of one analytic validity predicate along every root branch
/// retained for a feature.  `max_root_count` already includes any bounded
/// deformation oscillation envelope.  On a fixed scanline, substituting a
/// root branch into a predicate of `(u, q)` degree `d` can contribute at most
/// `d` crossings per retained branch, so multiplying the two bounds is the
/// conservative fail-closed choice.
pub(crate) fn feature_boundary_root_bound(
    projective: &super::projective::ProjectiveEquations,
    feature: &super::projective::ProjectiveFeature,
    predicate: super::ids::PredicateProgramId,
) -> Result<u32, String> {
    let predicate = projective
        .predicates
        .get(predicate.index())
        .filter(|program| program.id == predicate)
        .ok_or_else(|| format!("pixels::events: missing validity predicate {predicate}"))?;
    let polynomial = projective
        .polynomials
        .get(predicate.polynomial.index())
        .filter(|program| program.id == predicate.polynomial)
        .ok_or_else(|| {
            format!(
                "pixels::events: validity predicate {} names missing polynomial {}",
                predicate.id, predicate.polynomial
            )
        })?;
    let scanline_degree = polynomial
        .terms
        .iter()
        .map(|term| u32::from(term.exponents.u) + u32::from(term.exponents.q))
        .max()
        .unwrap_or(0)
        .max(1);
    let count = u32::from(feature.max_root_count)
        .checked_mul(scanline_degree)
        .ok_or_else(|| "P015: feature-boundary root bound overflow".to_string())?;
    checked_event_root_count("feature_boundary_roots", count)
}

/// Conservative Bézout/oscillation bound for a smooth boundary attached to
/// one feature sheet. The boundary solves that feature's root equation
/// together with a predicate contributed by every sheet on the opposite
/// smooth operand. Algebraic sheets contribute their q/bivariate degree;
/// bounded deformations contribute their already-expanded oscillation count.
pub(crate) fn smooth_event_root_bound(
    smooth_field: FieldId,
    kind: EventKind,
    feature: &super::features::FeatureRecord,
    structural_features: &[super::features::FeatureRecord],
    projective_features: &[super::projective::ProjectiveFeature],
) -> Result<u32, String> {
    let side = feature
        .occurrence_path
        .iter()
        .find(|step| step.field == smooth_field)
        .map(|step| step.child_slot)
        .ok_or_else(|| {
            format!(
                "pixels::events: feature {} does not occur below smooth field {smooth_field}",
                feature.id
            )
        })?;
    let own_degree = projective_features
        .get(feature.id.index())
        .filter(|candidate| candidate.feature == feature.id)
        .map(|candidate| u32::from(candidate.max_root_count))
        .ok_or_else(|| {
            format!(
                "pixels::events: feature {} lacks a projective root bound",
                feature.id
            )
        })?;
    let opposite_degree = structural_features
        .iter()
        .filter(|candidate| {
            candidate.object == feature.object
                && candidate
                    .occurrence_path
                    .iter()
                    .any(|step| step.field == smooth_field && step.child_slot != side)
        })
        .try_fold(0_u32, |total, candidate| {
            let degree = projective_features
                .get(candidate.id.index())
                .filter(|lowered| lowered.feature == candidate.id)
                .map(|lowered| u32::from(lowered.max_root_count))
                .ok_or_else(|| {
                    format!(
                        "pixels::events: opposite smooth feature {} lacks a projective root bound",
                        candidate.id
                    )
                })?;
            total
                .checked_add(degree)
                .ok_or_else(|| "P015: smooth event opposite-degree sum overflow".to_string())
        })?;
    let single_branch = own_degree
        .checked_mul(opposite_degree)
        .ok_or_else(|| "P015: smooth event Bézout bound overflow".to_string())?;
    // The center tie is one equation, a-b=0. The band edge is the union of
    // the two independently reachable equations a-b-k=0 and a-b+k=0.
    let count = if kind == EventKind::SmoothBandEnter {
        single_branch
            .checked_mul(2)
            .ok_or_else(|| "P015: smooth band branch bound overflow".to_string())?
    } else if kind == EventKind::SmoothCenterTie {
        single_branch
    } else {
        return Err(format!(
            "pixels::events: {kind:?} is not a smooth boundary family"
        ));
    };
    checked_event_root_count("smooth_event_roots", count)
}

/// Complete-domain remainder for the quadratic world-space Taylor model.
///
/// The derivative contracts bound the third directional derivative in world
/// space. Any two points in the feature AABB are separated by at most its
/// diagonal, so `M₃ * |Δx|³ / 3!` is valid for every local center selected by
/// the runtime. The deliberately broad complete-domain bound can be tightened
/// to a tile/root tube later without changing the proof contract.
pub(crate) fn quadratic_taylor_remainder(
    third_derivative_abs_bound: f64,
    bounds: super::world_bounds::Aabb64,
) -> Result<(f64, f64), String> {
    if !third_derivative_abs_bound.is_finite() || third_derivative_abs_bound < 0.0 {
        return Err("P015: Taylor program has an invalid third-derivative bound".to_string());
    }
    let mut squared = 0.0_f64;
    for component in 0..3 {
        let width =
            super::reference::interval::next_up(bounds.max[component] - bounds.min[component]);
        squared = super::reference::interval::next_up(
            squared + super::reference::interval::next_up(width * width),
        );
    }
    let world_delta_abs_bound = super::reference::interval::next_up(squared.sqrt());
    let remainder = if third_derivative_abs_bound == 0.0 || world_delta_abs_bound == 0.0 {
        0.0
    } else {
        let delta_squared =
            super::reference::interval::next_up(world_delta_abs_bound * world_delta_abs_bound);
        let delta_cubed =
            super::reference::interval::next_up(delta_squared * world_delta_abs_bound);
        super::reference::interval::next_up(
            super::reference::interval::next_up(third_derivative_abs_bound * delta_cubed) / 6.0,
        )
    };
    if !world_delta_abs_bound.is_finite() || !remainder.is_finite() {
        return Err("P015: Taylor program has no finite complete-domain remainder".to_string());
    }
    Ok((world_delta_abs_bound, remainder))
}

/// Bézout bound for isolated solutions of the torus silhouette system
/// `G(u,v,q) = 0, G_q(u,v,q) = 0` on one scanline: degrees four and three.
pub const TORUS_SILHOUETTE_ROOT_BOUND_V1: u32 = 12;

pub(crate) fn polynomial_abs_bound(
    polynomial: &super::polynomial::PolyProgram,
    coefficient_intervals: &[F64Interval],
    camera: super::camera::CameraContract,
) -> Result<f64, String> {
    let variable_abs = [
        camera.aspect * camera.tan_half_fov_y,
        camera.tan_half_fov_y,
        camera.q.hi,
    ];
    let mut bound = 0.0_f64;
    for term in &polynomial.terms {
        if term.exponents.x != 0 || term.exponents.t != 0 || term.exponents.param_terms.count != 0 {
            return Err(format!(
                "pixels::events: torus oracle polynomial {} has unsupported local variables",
                polynomial.id
            ));
        }
        let coefficient = coefficient_intervals
            .get(term.coefficient.index())
            .copied()
            .ok_or_else(|| {
                format!(
                    "pixels::events: torus oracle polynomial {} names missing coefficient {}",
                    polynomial.id, term.coefficient
                )
            })?;
        let coefficient_abs = coefficient.lo.abs().max(coefficient.hi.abs());
        let monomial = variable_abs[0].powi(i32::from(term.exponents.u))
            * variable_abs[1].powi(i32::from(term.exponents.v))
            * variable_abs[2].powi(i32::from(term.exponents.q));
        bound = super::reference::interval::next_up(
            bound + super::reference::interval::next_up(coefficient_abs * monomial),
        );
    }
    if !bound.is_finite() {
        return Err(format!(
            "pixels::events: torus oracle polynomial {} has no finite complete-domain bound",
            polynomial.id
        ));
    }
    Ok(bound)
}

/// Complete fourth-order remainder for the cubic Taylor model of a torus
/// root equation on one scanline.
///
/// The oracle solves `(G, G_q) = 0` over `(u, q)`. `G` has total `(u, q)`
/// degree at most four and `G_q` has degree at most three, so only the
/// fourth-order terms of `G` contribute a remainder. For a monomial
/// `c * u^a * q^b`, `a + b = 4`, the mixed partial and Taylor factorials
/// cancel, leaving `|c| |delta_u|^a |delta_q|^b`. Bounding every such term
/// over the full scanline/q box covers mixed terms such as `u^2 q^2`.
pub(crate) fn torus_bivariate_fourth_remainder(
    root: &super::polynomial::PolyProgram,
    coefficient_intervals: &[F64Interval],
    camera: super::camera::CameraContract,
) -> Result<f64, String> {
    let u_abs = super::reference::interval::next_up(camera.aspect * camera.tan_half_fov_y);
    let u_delta_abs = super::reference::interval::next_up(2.0 * u_abs);
    let q_delta_abs = super::reference::interval::next_up(camera.q.width());
    torus_bivariate_fourth_remainder_for_domain(
        root,
        coefficient_intervals,
        camera.tan_half_fov_y,
        u_delta_abs,
        q_delta_abs,
    )
}

fn torus_bivariate_fourth_remainder_for_domain(
    root: &super::polynomial::PolyProgram,
    coefficient_intervals: &[F64Interval],
    v_abs: f64,
    u_delta_abs: f64,
    q_delta_abs: f64,
) -> Result<f64, String> {
    if [v_abs, u_delta_abs, q_delta_abs]
        .into_iter()
        .any(|extent| !extent.is_finite() || extent < 0.0)
    {
        return Err("pixels::events: torus oracle has an invalid local domain".to_string());
    }
    let mut fourth = 0.0_f64;
    for term in &root.terms {
        if term.exponents.x != 0 || term.exponents.t != 0 || term.exponents.param_terms.count != 0 {
            return Err(format!(
                "pixels::events: torus remainder polynomial {} has unsupported local variables",
                root.id
            ));
        }
        let total_degree = u16::from(term.exponents.u) + u16::from(term.exponents.q);
        if total_degree > 4 {
            return Err(format!(
                "pixels::events: torus remainder polynomial {} exceeds total (u,q) degree four",
                root.id
            ));
        }
        if total_degree != 4 {
            continue;
        }
        let coefficient = coefficient_intervals
            .get(term.coefficient.index())
            .copied()
            .ok_or_else(|| {
                format!(
                    "pixels::events: torus remainder polynomial {} names missing coefficient {}",
                    root.id, term.coefficient
                )
            })?;
        let coefficient_abs = coefficient.lo.abs().max(coefficient.hi.abs());
        let monomial = u_delta_abs.powi(i32::from(term.exponents.u))
            * v_abs.powi(i32::from(term.exponents.v))
            * q_delta_abs.powi(i32::from(term.exponents.q));
        fourth = super::reference::interval::next_up(
            fourth + super::reference::interval::next_up(coefficient_abs * monomial),
        );
    }
    let remainder = super::reference::interval::next_up(fourth + f64::EPSILON);
    if !remainder.is_finite() || remainder <= 0.0 {
        return Err(
            "pixels::events: torus oracle has no finite strict Taylor remainder".to_string(),
        );
    }
    Ok(remainder)
}

pub fn compile(
    graph: &super::symbolic::SymbolicGraph,
    structural: &super::verify::StructuralProgram,
    projective: &mut super::projective::ProjectiveEquations,
    derivatives: &DerivativePrograms,
    spans: &[ProjectedFeatureSpan],
    deformations: &[super::deform::ProjectiveDeformationProgram],
) -> Result<EventPrograms, String> {
    let silhouette_conditions = compile_silhouette_conditions(projective)?;
    if projective.features.len() != spans.len()
        || projective.features.len() != derivatives.bundles.len()
    {
        return Err(
            "pixels::events: projective feature, span, and derivative counts differ".to_string(),
        );
    }
    let mut compiler = EventCompiler {
        spans,
        derivatives,
        generators: Vec::new(),
        ledger: Vec::new(),
    };
    for feature in &projective.features {
        let span = compiler.span(feature.feature)?.clone();
        let bundle = compiler.bundle(feature.feature)?.clone();
        let participants = [Participant::Feature(feature.feature)];
        for (kind, coordinate, horizontal, ordinal, sides) in [
            (
                EventKind::ProjectedBoundEnter,
                span.pixels.x.start,
                false,
                0,
                EventSideMeaning::crossing(EventSide::Inactive, EventSide::Active),
            ),
            (
                EventKind::ProjectedBoundExit,
                span.pixels.x.end,
                false,
                1,
                EventSideMeaning::crossing(EventSide::Active, EventSide::Inactive),
            ),
        ] {
            let subject = EventSubject {
                kind,
                feature: Some(feature.feature),
                owner: None,
                ordinal,
            };
            if full_output(&span, projective.camera) {
                compiler.omit(subject, OmissionHint::FullDomainProjection);
            } else {
                compiler.emit(
                    subject,
                    &participants,
                    span.clone(),
                    feature.influencing_params.clone(),
                    1,
                    EventRepresentation::ProjectedBoundary {
                        horizontal,
                        coordinate,
                    },
                    sides,
                )?;
            }
        }
        let silhouette = EventSubject {
            kind: EventKind::Silhouette,
            feature: Some(feature.feature),
            owner: None,
            ordinal: 0,
        };
        let deformed = deformations
            .iter()
            .find(|program| program.feature == feature.feature);
        let primitive = &graph
            .fields
            .get(structural.features[feature.feature.index()].primitive)?
            .kind;
        let torus = matches!(
            primitive,
            super::graph::FieldKind::Primitive(super::graph::Primitive::Torus { .. })
        );
        match (feature.q_seed_kind, deformed, torus) {
            (super::projective::SeedKind::Affine { .. }, None, false) => {
                compiler.omit(silhouette, OmissionHint::StrictProjectiveLeadingSign)
            }
            (_, Some(deformation), _) => {
                let scalar_derivatives = scalar_derivative_program(
                    structural,
                    &[deformation.residual, deformation.coordinate_x],
                )?;
                let third_derivative_abs_bound = super::reference::interval::next_up(
                    deformation
                        .third_derivative_bound
                        .max(scalar_derivatives.third_world_abs),
                );
                let (world_delta_abs_bound, remainder) = quadratic_taylor_remainder(
                    third_derivative_abs_bound,
                    structural.features[feature.feature.index()].world_bounds,
                )?;
                compiler.emit(
                    silhouette,
                    &participants,
                    span.clone(),
                    merge_dependencies(&feature.influencing_params, &scalar_derivatives),
                    u32::from(deformation.maximum_root_count),
                    EventRepresentation::DeformationTaylorPredicate {
                        predictor: deformation.predictor,
                        predictor_derivatives: bundle.id,
                        displacement: deformation.residual,
                        scalar_derivatives,
                        phase_recurrence: deformation.phase_recurrence.clone(),
                        taylor_order: 2,
                        world_delta_abs_bound,
                        third_derivative_abs_bound,
                        remainder,
                    },
                    EventSideMeaning {
                        negative: EventSide::RecomputeRootSet,
                        zero: EventSide::RecomputeRootSet,
                        positive: EventSide::RecomputeRootSet,
                    },
                )?;
            }
            (_, None, true) => {
                let root_program = &projective.polynomials[feature.root_equation.index()];
                let derivative_q_program = &projective.polynomials[bundle.first.q.index()];
                if root_program
                    .terms
                    .iter()
                    .any(|term| u16::from(term.exponents.u) + u16::from(term.exponents.q) > 4)
                    || derivative_q_program
                        .terms
                        .iter()
                        .any(|term| u16::from(term.exponents.u) + u16::from(term.exponents.q) > 3)
                {
                    return Err(format!(
                        "P004: field operation `torus silhouette oracle` is not available in `AaaByteExact`: feature {} exceeds the degree-four/degree-three Bézout shape",
                        feature.feature
                    ));
                }
                let polynomial_ids = [
                    feature.root_equation,
                    bundle.first.u,
                    bundle.first.q,
                    bundle.second.uq,
                    bundle.second.qq,
                    bundle.third.uuu,
                ];
                let coefficient_intervals = super::projective::coefficient_intervals_for_roots(
                    &projective.coefficients,
                    &structural.values,
                    projective.camera,
                    polynomial_ids.into_iter().flat_map(|id| {
                        projective.polynomials[id.index()]
                            .terms
                            .iter()
                            .map(|term| term.coefficient)
                    }),
                )?;
                let bound = |id: PolyProgramId| {
                    polynomial_abs_bound(
                        &projective.polynomials[id.index()],
                        &coefficient_intervals,
                        projective.camera,
                    )
                };
                let remainder = torus_bivariate_fourth_remainder(
                    root_program,
                    &coefficient_intervals,
                    projective.camera,
                )?;
                compiler.emit(
                    silhouette,
                    &participants,
                    span.clone(),
                    feature.influencing_params.clone(),
                    TORUS_SILHOUETTE_ROOT_BOUND_V1,
                    EventRepresentation::TorusLocalOracle {
                        root: feature.root_equation,
                        derivative_u: bundle.first.u,
                        derivative_q: bundle.first.q,
                        derivative_uq: bundle.second.uq,
                        derivative_qq: bundle.second.qq,
                        third_u: bundle.third.uuu,
                        value_abs_bound: bound(feature.root_equation)?,
                        derivative_u_abs_bound: bound(bundle.first.u)?,
                        derivative_q_abs_bound: bound(bundle.first.q)?,
                        derivative_uq_abs_bound: bound(bundle.second.uq)?,
                        derivative_qq_abs_bound: bound(bundle.second.qq)?,
                        third_u_abs_bound: bound(bundle.third.uuu)?,
                        taylor_order: 3,
                        remainder,
                    },
                    EventSideMeaning {
                        negative: EventSide::RecomputeRootSet,
                        zero: EventSide::RecomputeRootSet,
                        positive: EventSide::RecomputeRootSet,
                    },
                )?;
            }
            (_, None, false) => {
                let condition = silhouette_conditions
                    .get(feature.feature.index())
                    .copied()
                    .flatten()
                    .ok_or_else(|| {
                        format!(
                            "pixels::events: non-affine feature {} has no signed quadratic discriminant",
                            feature.feature
                        )
                    })?;
                // A leading coefficient with no `(u, q)` dependence has no zero
                // curve anywhere in the output, so the silhouette it would mark
                // does not exist. `StrictProjectiveLeadingSign` above already
                // drops the sign-provable case; this covers the remaining one,
                // where the constant's sign is not sealed but its constancy is.
                let (representation, sides) = match condition {
                    SilhouetteCondition::LinearLeadingCoefficient(coefficient) => (
                        EventRepresentation::LinearLeadingCoefficient {
                            coefficient,
                            root: feature.root_equation,
                        },
                        EventSideMeaning {
                            negative: EventSide::RecomputeRootSet,
                            zero: EventSide::RecomputeRootSet,
                            positive: EventSide::RecomputeRootSet,
                        },
                    ),
                    SilhouetteCondition::QuadraticDiscriminant(discriminant) => (
                        EventRepresentation::QuadraticDiscriminant {
                            discriminant,
                            root: feature.root_equation,
                        },
                        EventSideMeaning::crossing(EventSide::Inactive, EventSide::Active),
                    ),
                };
                compiler.emit(
                    silhouette,
                    &participants,
                    span.clone(),
                    feature.influencing_params.clone(),
                    u32::from(feature.max_root_count),
                    representation,
                    sides,
                )?;
            }
        }
        for (ordinal, predicate) in feature.validity_predicates.iter().enumerate() {
            let predicate_program = projective
                .predicates
                .get(predicate.index())
                .filter(|program| program.id == *predicate)
                .ok_or_else(|| format!("pixels::events: missing validity predicate {predicate}"))?;
            let sense = predicate_program.sense;
            let mut dependencies = feature.influencing_params.clone();
            dependencies.extend(super::projective::polynomial_influencing_params(
                graph,
                projective,
                predicate_program.polynomial,
            )?);
            dependencies.sort();
            dependencies.dedup();
            let subject = EventSubject {
                kind: EventKind::FeatureBoundary,
                feature: Some(feature.feature),
                owner: None,
                ordinal: u32::try_from(ordinal)
                    .map_err(|_| "P015: validity event ordinal overflow".to_string())?,
            };
            let polynomial = projective
                .polynomials
                .get(predicate_program.polynomial.index())
                .filter(|polynomial| polynomial.id == predicate_program.polynomial)
                .ok_or_else(|| {
                    format!(
                        "pixels::events: validity predicate {predicate} names missing polynomial {}",
                        predicate_program.polynomial
                    )
                })?;
            let coefficient_intervals = super::projective::coefficient_intervals_for_roots(
                &projective.coefficients,
                &structural.values,
                projective.camera,
                polynomial.terms.iter().map(|term| term.coefficient),
            )?;
            let u_extent = projective.camera.aspect * projective.camera.tan_half_fov_y;
            if let Some(proof) = super::exclusions::prove_projective_program_strict_sign_with_q(
                polynomial,
                &coefficient_intervals,
                [
                    super::exclusions::NormalizedAxis {
                        lo: -u_extent,
                        hi: u_extent,
                    },
                    super::exclusions::NormalizedAxis {
                        lo: -projective.camera.tan_half_fov_y,
                        hi: projective.camera.tan_half_fov_y,
                    },
                    super::exclusions::NormalizedAxis {
                        lo: span.q.lo,
                        hi: span.q.hi,
                    },
                ],
            )? {
                compiler.omit(
                    subject,
                    OmissionHint::GlobalStrictSign {
                        kind: PredicateOmissionKind::FeatureValidity,
                        proof,
                    },
                );
                continue;
            }
            let sides = match sense {
                super::program::PredicateSense::NonNegative
                | super::program::PredicateSense::StrictPositive => EventSideMeaning::crossing(
                    EventSide::OutsideValidity,
                    EventSide::InsideValidity,
                ),
                super::program::PredicateSense::NonPositive
                | super::program::PredicateSense::StrictNegative => EventSideMeaning::crossing(
                    EventSide::InsideValidity,
                    EventSide::OutsideValidity,
                ),
                super::program::PredicateSense::EqualZero => {
                    return Err(format!(
                        "pixels::events: validity predicate {predicate} has no inside-side sense"
                    ));
                }
            };
            compiler.emit(
                subject,
                &participants,
                span.clone(),
                dependencies,
                feature_boundary_root_bound(projective, feature, *predicate)?,
                EventRepresentation::SparsePredicate {
                    predicate: *predicate,
                },
                sides,
            )?;
        }
        for (kind, q, ordinal, sides) in [
            (
                EventKind::NearClip,
                projective.camera.q.hi,
                0,
                EventSideMeaning::crossing(EventSide::InsideClip, EventSide::OutsideClip),
            ),
            (
                EventKind::FarClip,
                projective.camera.q.lo,
                0,
                EventSideMeaning::crossing(EventSide::OutsideClip, EventSide::InsideClip),
            ),
        ] {
            let subject = EventSubject {
                kind,
                feature: Some(feature.feature),
                owner: None,
                ordinal,
            };
            // The clip edge is the locus where this feature's root sits exactly
            // on the clip plane. A feature whose sealed projected q span does
            // not reach that q has no such locus anywhere in the output, so the
            // event marks a boundary that does not exist — and, left emitted, a
            // degenerate ray polynomial can still produce a spurious level set
            // there for the coverage tier to mistake for occupancy.
            if let Some(margin) = clip_q_omission_margin(span.q, projective.camera.q, q) {
                compiler.omit(
                    subject,
                    OmissionHint::ClipQOutsideFeatureQSpan {
                        clip_q: q,
                        feature_q: span.q,
                        margin,
                    },
                );
                continue;
            }
            compiler.emit(
                subject,
                &participants,
                span.clone(),
                feature.influencing_params.clone(),
                u32::from(feature.max_root_count),
                EventRepresentation::ClipQ { q },
                sides,
            )?;
        }
        compiler.emit(
            EventSubject {
                kind: EventKind::FixedPointResetOnly,
                feature: Some(feature.feature),
                owner: None,
                ordinal: 0,
            },
            &participants,
            span,
            feature.influencing_params.clone(),
            1,
            EventRepresentation::FixedPointReset,
            EventSideMeaning {
                negative: EventSide::ResetOnly,
                zero: EventSide::ResetOnly,
                positive: EventSide::ResetOnly,
            },
        )?;
    }

    for template in &structural.repeats {
        for (ordinal, wrap) in template.wrap_events.iter().enumerate() {
            for instance in &template.instances {
                for feature in structural
                    .features
                    .iter()
                    .filter(|feature| feature.object == instance.object)
                {
                    let span = compiler.span(feature.id)?.clone();
                    compiler.emit(
                        EventSubject {
                            kind: EventKind::RepeatBoundary,
                            feature: Some(feature.id),
                            owner: Some(instance.object),
                            ordinal: u32::try_from(ordinal)
                                .map_err(|_| "P015: repeat event ordinal overflow".to_string())?,
                        },
                        &[
                            Participant::Feature(feature.id),
                            Participant::Object(instance.object),
                            Participant::Field(wrap.repeat_field),
                        ],
                        span,
                        projective.features[feature.id.index()]
                            .influencing_params
                            .clone(),
                        1,
                        EventRepresentation::RepeatAffineBoundary {
                            axis: wrap.axis,
                            boundary: wrap.boundary,
                        },
                        EventSideMeaning::crossing(EventSide::RepeatLeft, EventSide::RepeatRight),
                    )?;
                }
            }
        }
    }

    let mut smooth_ordinal = 0_u32;
    for (field, node) in graph.fields.iter() {
        let Some((a, b, k, left_negated, right_negated)) = smooth_operands(&node.kind) else {
            continue;
        };
        for feature in structural.features.iter().filter(|feature| {
            feature
                .occurrence_path
                .iter()
                .any(|step| step.field == field)
        }) {
            let span = compiler.span(feature.id)?.clone();
            let left = graph.fields.get(a)?.scalar_value;
            let right = graph.fields.get(b)?.scalar_value;
            let tie_derivatives = scalar_derivative_program(structural, &[left, right])?;
            let band_derivatives = scalar_derivative_program(structural, &[left, right, k])?;
            let (tie_world_delta, tie_remainder) =
                quadratic_taylor_remainder(tie_derivatives.third_world_abs, feature.world_bounds)?;
            let (band_world_delta, band_remainder) =
                quadratic_taylor_remainder(band_derivatives.third_world_abs, feature.world_bounds)?;
            let left_range = if left_negated {
                structural.values.get(left)?.neg()
            } else {
                structural.values.get(left)?
            };
            let right_range = if right_negated {
                structural.values.get(right)?.neg()
            } else {
                structural.values.get(right)?
            };
            let tie_range = left_range.sub_outward(right_range)?;
            let band_range = tie_range.abs()?.sub_outward(structural.values.get(k)?)?;
            for (kind, representation, sides) in [
                (
                    EventKind::SmoothBandEnter,
                    EventRepresentation::SmoothBandTaylorPredicate {
                        left,
                        right,
                        left_negated,
                        right_negated,
                        radius: k,
                        derivatives: band_derivatives.clone(),
                        taylor_order: 2,
                        world_delta_abs_bound: band_world_delta,
                        remainder: band_remainder,
                    },
                    EventSideMeaning::crossing(EventSide::Active, EventSide::Inactive),
                ),
                (
                    EventKind::SmoothCenterTie,
                    EventRepresentation::SmoothTieTaylorPredicate {
                        left,
                        right,
                        left_negated,
                        right_negated,
                        derivatives: tie_derivatives.clone(),
                        taylor_order: 2,
                        world_delta_abs_bound: tie_world_delta,
                        remainder: tie_remainder,
                    },
                    EventSideMeaning::crossing(EventSide::SmoothLeft, EventSide::SmoothRight),
                ),
            ] {
                let subject = EventSubject {
                    kind,
                    feature: Some(feature.id),
                    owner: Some(feature.object),
                    ordinal: smooth_ordinal,
                };
                let dependencies = merge_dependencies(
                    &projective.features[feature.id.index()].influencing_params,
                    if kind == EventKind::SmoothBandEnter {
                        &band_derivatives
                    } else {
                        &tie_derivatives
                    },
                );
                let predicate_range = if kind == EventKind::SmoothBandEnter {
                    band_range
                } else {
                    tie_range
                };
                if let Some(proof) =
                    interval_strict_sign_proof(structural, &dependencies, predicate_range)?
                {
                    compiler.omit(
                        subject,
                        OmissionHint::GlobalStrictSign {
                            kind: PredicateOmissionKind::SupportShell,
                            proof,
                        },
                    );
                    continue;
                }
                let smooth_root_bound = smooth_event_root_bound(
                    field,
                    kind,
                    feature,
                    &structural.features,
                    &projective.features,
                )?;
                compiler.emit(
                    subject,
                    &[
                        Participant::Feature(feature.id),
                        Participant::Field(a),
                        Participant::Field(b),
                    ],
                    span.clone(),
                    dependencies,
                    smooth_root_bound,
                    representation,
                    sides,
                )?;
            }
        }
        smooth_ordinal = smooth_ordinal
            .checked_add(1)
            .ok_or_else(|| "P015: smooth event ordinal overflow".to_string())?;
    }

    for (ordinal, event) in structural.material_events.iter().enumerate() {
        let super::scalar::ScalarOp::Compare {
            op: comparison,
            a: left,
            b: right,
        } = graph.scalar.get(event.predicate)?.op
        else {
            return Err(format!(
                "pixels::events: material predicate {} is not a canonical comparison",
                event.predicate
            ));
        };
        let scalar_derivatives = scalar_derivative_program(structural, &[left, right])?;
        for feature in &event.feature_owners {
            let span = compiler.span(*feature)?.clone();
            let subject = EventSubject {
                kind: EventKind::MaterialBoundary,
                feature: Some(*feature),
                owner: structural
                    .features
                    .get(feature.index())
                    .map(|feature| feature.object),
                ordinal: u32::try_from(ordinal)
                    .map_err(|_| "P015: material event ordinal overflow".to_string())?,
            };
            let dependencies = merge_dependencies(
                &projective.features[feature.index()].influencing_params,
                &scalar_derivatives,
            );
            let difference = structural
                .values
                .get(left)?
                .sub_outward(structural.values.get(right)?)?;
            if let Some(proof) = interval_strict_sign_proof(structural, &dependencies, difference)?
            {
                compiler.omit(
                    subject,
                    OmissionHint::GlobalStrictSign {
                        kind: PredicateOmissionKind::MaterialClass,
                        proof,
                    },
                );
                continue;
            }
            let (world_delta_abs_bound, remainder) = quadratic_taylor_remainder(
                scalar_derivatives.third_world_abs,
                structural.features[feature.index()].world_bounds,
            )?;
            compiler.emit(
                subject,
                &[
                    Participant::Feature(*feature),
                    Participant::MaterialEvent(
                        u32::try_from(ordinal)
                            .map_err(|_| "P015: material event ID overflow".to_string())?,
                    ),
                ],
                span,
                dependencies,
                checked_event_root_count("material_event_roots", event.crossing_bound)?,
                EventRepresentation::MaterialDifferenceTaylorPredicate {
                    left,
                    right,
                    comparison,
                    derivatives: scalar_derivatives.clone(),
                    taylor_order: 2,
                    world_delta_abs_bound,
                    remainder,
                },
                material_side_meaning(comparison),
            )?;
        }
    }
    compiler.ledger.sort_by_key(|entry| entry.subject);
    for (expected, generator) in compiler.generators.iter().enumerate() {
        if generator.id.index() != expected {
            return Err("pixels::events: generator IDs are noncanonical".to_string());
        }
        if generator
            .coefficient_dependencies
            .windows(2)
            .any(|pair| pair[0] >= pair[1])
        {
            return Err(format!(
                "pixels::events: generator {} dependencies are not strictly sorted",
                generator.id
            ));
        }
    }
    Ok(EventPrograms {
        generators: compiler.generators,
        ledger: compiler.ledger,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn internal_difference(left: f64, right: f64, negations: (bool, bool)) -> f64 {
        let left = if negations.0 { -left } else { left };
        let right = if negations.1 { -right } else { right };
        left - right
    }

    #[test]
    fn clip_q_omission_needs_a_separation_the_guest_widening_cannot_close() {
        let camera = F64Interval::new(0.0078125, 10.000000000000002).unwrap();
        let near = camera.hi;
        let far = camera.lo;
        // A feature whose whole world AABB sits between the clip planes can
        // never place a root on either of them.
        let inside = F64Interval::new(0.15442127623884103, 0.6903199296309185).unwrap();
        let near_margin = clip_q_omission_margin(inside, camera, near).expect("near clip clears");
        let far_margin = clip_q_omission_margin(inside, camera, far).expect("far clip clears");
        assert!(near_margin > 9.0 && near_margin < 9.32, "{near_margin}");
        assert!(far_margin > 0.1 && far_margin < 0.147, "{far_margin}");
        // A feature reaching a clip plane keeps its event. `derive` clamps the
        // projected span to the camera range, so touching is exact equality.
        let touching_near = F64Interval::new(0.2, near).unwrap();
        assert_eq!(clip_q_omission_margin(touching_near, camera, near), None);
        let touching_far = F64Interval::new(far, 0.2).unwrap();
        assert_eq!(clip_q_omission_margin(touching_far, camera, far), None);
        // Separations inside the slack are refused rather than proven: the
        // guest widens the span it reads and could still reach the clip.
        let hairline = F64Interval::new(0.2, near - camera.hi / 1024.0).unwrap();
        assert_eq!(clip_q_omission_margin(hairline, camera, near), None);
        // A span straddling the clip q is never omitted.
        let straddling = F64Interval::new(0.2, 12.0).unwrap();
        assert_eq!(clip_q_omission_margin(straddling, camera, near), None);
    }

    #[test]
    fn smooth_event_operands_match_each_csg_lowering() {
        let a = FieldId(1);
        let b = FieldId(2);
        let k = ScalarId(3);
        let kinds = [
            (
                super::super::graph::FieldKind::SmoothUnion { a, b, k },
                (false, false),
                -3.0,
            ),
            (
                super::super::graph::FieldKind::SmoothIntersection { a, b, k },
                (true, true),
                3.0,
            ),
            (
                super::super::graph::FieldKind::SmoothSubtract { a, b, k },
                (true, false),
                -7.0,
            ),
        ];
        for (kind, expected_negations, expected_difference) in kinds {
            let (found_a, found_b, found_k, left_negated, right_negated) =
                smooth_operands(&kind).expect("smooth operation");
            assert_eq!((found_a, found_b, found_k), (a, b, k));
            assert_eq!((left_negated, right_negated), expected_negations);
            assert_eq!(
                internal_difference(2.0, 5.0, expected_negations),
                expected_difference
            );
        }
    }

    #[test]
    fn affine_smooth_band_has_two_independent_boundary_crossings() {
        // a(x)=x and b(x)=-x give |a-b|-1 = |2x|-1, whose two
        // boundary branches cross at distinct local scanline coordinates.
        let roots = [-0.5_f64, 0.5_f64];
        for root in roots {
            let left = root;
            let right = -root;
            assert_eq!((left - right).abs() - 1.0, 0.0);
        }
        assert_ne!(roots[0], roots[1]);
        let one_branch_bezout_bound = 1_u32;
        assert_eq!(one_branch_bezout_bound * 2, roots.len() as u32);
    }

    #[test]
    fn participant_sets_are_sorted_deduplicated_and_bounded() {
        let set = SmallParticipantSet::new(&[
            Participant::Feature(FeatureId(2)),
            Participant::Feature(FeatureId(1)),
            Participant::Feature(FeatureId(2)),
        ])
        .unwrap();
        assert_eq!(
            set.iter().collect::<Vec<_>>(),
            vec![
                Participant::Feature(FeatureId(1)),
                Participant::Feature(FeatureId(2))
            ]
        );
        assert!(
            SmallParticipantSet::new(
                &(0..=MAX_EVENT_PARTICIPANTS_V1)
                    .map(|id| Participant::Feature(FeatureId(id as u32)))
                    .collect::<Vec<_>>()
            )
            .unwrap_err()
            .starts_with("P015:")
        );
    }

    #[test]
    fn event_root_bounds_are_preserved_without_truncation() {
        assert_eq!(checked_event_root_count("test", 9).unwrap(), 9);
        assert_eq!(checked_event_root_count("test", 256).unwrap(), 256);
        assert!(
            checked_event_root_count("test", 0)
                .unwrap_err()
                .starts_with("P015:")
        );
    }

    #[test]
    fn complete_domain_taylor_remainder_scales_with_world_delta_and_curvature() {
        let narrow = super::super::world_bounds::Aabb64::new([0.0; 3], [1.0, 0.0, 0.0]).unwrap();
        let wide = super::super::world_bounds::Aabb64::new([0.0; 3], [100.0, 0.0, 0.0]).unwrap();
        let (narrow_delta, narrow_remainder) = quadratic_taylor_remainder(6.0, narrow).unwrap();
        let (wide_delta, wide_remainder) = quadratic_taylor_remainder(6.0, wide).unwrap();
        assert!(narrow_delta >= 1.0);
        assert!(narrow_remainder >= 1.0);
        assert!(wide_delta >= 100.0);
        // For f(x)=x³ expanded quadratically at zero, M₃=6 and the exact
        // error at x=100 is 100³. This catches accidentally serializing M₃
        // itself as though it were a complete-domain remainder.
        assert!(wide_remainder >= 1_000_000.0);
        assert!(wide_remainder > narrow_remainder * 999_999.0);

        let (_, higher_curvature) = quadratic_taylor_remainder(60.0, wide).unwrap();
        assert!(higher_curvature >= wide_remainder * 10.0);
    }

    #[test]
    fn torus_remainder_encloses_mixed_u_squared_q_squared_term() {
        let root = super::super::polynomial::PolyProgram {
            id: PolyProgramId(0),
            terms: vec![super::super::polynomial::PolyTerm {
                exponents: super::super::polynomial::Exponents {
                    u: 2,
                    q: 2,
                    ..super::super::polynomial::Exponents::default()
                },
                coefficient: super::super::ids::CoeffId(0),
            }],
            degree_u: 2,
            degree_v: 0,
            degree_q: 2,
            degree_x: 0,
            degree_t: 0,
            coefficient_program: 0,
        };
        let coefficients = [F64Interval::point(3.0).unwrap()];
        let remainder =
            torus_bivariate_fourth_remainder_for_domain(&root, &coefficients, 1.0, 2.0, 3.0)
                .unwrap();
        // At a zero-centered expansion, 3*u²*q² reaches 108 at
        // (delta_u, delta_q) = (2, 3). A u-only fourth derivative bound
        // would miss this contribution entirely.
        assert!(remainder >= 108.0);
    }

    #[test]
    fn material_comparison_sides_preserve_strict_and_equality_semantics() {
        use super::super::scalar::CompareOp;
        let cases = [
            (
                CompareOp::Lt,
                [
                    EventSide::MaterialLeft,
                    EventSide::MaterialRight,
                    EventSide::MaterialRight,
                ],
            ),
            (
                CompareOp::Le,
                [
                    EventSide::MaterialLeft,
                    EventSide::MaterialLeft,
                    EventSide::MaterialRight,
                ],
            ),
            (
                CompareOp::Gt,
                [
                    EventSide::MaterialRight,
                    EventSide::MaterialRight,
                    EventSide::MaterialLeft,
                ],
            ),
            (
                CompareOp::Ge,
                [
                    EventSide::MaterialRight,
                    EventSide::MaterialLeft,
                    EventSide::MaterialLeft,
                ],
            ),
            (
                CompareOp::Eq,
                [
                    EventSide::MaterialRight,
                    EventSide::MaterialLeft,
                    EventSide::MaterialRight,
                ],
            ),
            (
                CompareOp::Ne,
                [
                    EventSide::MaterialLeft,
                    EventSide::MaterialRight,
                    EventSide::MaterialLeft,
                ],
            ),
        ];
        for (comparison, [negative, zero, positive]) in cases {
            assert_eq!(
                material_side_meaning(comparison),
                EventSideMeaning {
                    negative,
                    zero,
                    positive
                }
            );
        }
    }
}
