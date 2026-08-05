//! Sparse local q-order competition and swap-event construction.

use std::collections::BTreeSet;

use super::event_kinds::{EventKind, EventSide, EventSideMeaning};
use super::events::{
    EventGenerator, EventLedgerEntry, EventPrograms, EventRepresentation, EventSubject,
    Participant, SmallParticipantSet,
};
use super::ids::{CompetitionPairId, EventTemplateId, FeatureId, PolyProgramId};
use super::polynomial::{PolyBuilder, ProgramLimit};
use super::program::CoeffBuilder;
use super::projection_bounds::{
    HalfOpenRange, PixelSpan, ProjectedFeatureSpan, TILE_HEIGHT_V1, TILE_WIDTH_V1, TileSpan,
};
use super::projective::{ProjectiveEquations, SeedKind, StrictSignObligation};

const TAYLOR_DEPTH_DIFFERENCE_ORDER_V1: u8 = 2;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct CompetitionSubject {
    pub a: FeatureId,
    pub b: FeatureId,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PairOmissionHint {
    ProjectedBoundsDisjoint,
    QRangesDisjoint,
    CsgNonInfluential,
    CsgJointInfluenceImpossible {
        variable_count: u8,
        states_checked: u32,
    },
    OutsideNearFar,
    StaticStrictOrder,
    GlobalStrictOrder,
}

#[derive(Clone, Debug, PartialEq)]
pub struct CompetitionPair {
    pub id: CompetitionPairId,
    pub a: FeatureId,
    pub b: FeatureId,
    pub event: EventTemplateId,
    pub pixels: PixelSpan,
    pub tiles: TileSpan,
    pub q_overlap: super::reference::interval::F64Interval,
}

#[derive(Clone, Debug, PartialEq)]
pub struct CompetitionDecision {
    pub subject: CompetitionSubject,
    pub emitted: Option<CompetitionPairId>,
    pub omission: Option<PairOmissionHint>,
    pub margin: f64,
    pub strict_sign_proof: Option<super::exclusions::BernsteinProofPayload>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct CompetitionPrograms {
    pub pairs: Vec<CompetitionPair>,
    pub ledger: Vec<CompetitionDecision>,
    pub pruned_projected: u32,
    pub pruned_q: u32,
    pub pruned_csg_global: u32,
    pub pruned_csg_pair: u32,
    pub pruned_strict_order: u32,
    /// Feature pairs are generated with `a < b`; these diagonal subjects
    /// document the same-feature duplicate filter without polluting the
    /// complete unordered-pair ledger.
    pub suppressed_same_feature: u32,
    /// Material boundaries are event families over existing geometry and
    /// never enter the geometry competition candidate set.
    pub suppressed_material_only: u32,
}

fn intersect_range(a: HalfOpenRange, b: HalfOpenRange) -> Option<HalfOpenRange> {
    let result = HalfOpenRange {
        start: a.start.max(b.start),
        end: a.end.min(b.end),
    };
    (!result.is_empty()).then_some(result)
}

fn outside_margin(span: &ProjectedFeatureSpan) -> Result<Option<f64>, String> {
    if span.rule != super::projection_bounds::ProjectionRule::OutsideNearFar {
        return Ok(None);
    }
    let margin = span.outside_margin.ok_or_else(|| {
        format!(
            "pixels::competition: outside-near/far span for {} has no proof margin",
            span.feature
        )
    })?;
    if !margin.is_finite() || margin <= 0.0 {
        return Err(format!(
            "pixels::competition: outside-near/far span for {} has invalid margin {margin}",
            span.feature
        ));
    }
    Ok(Some(margin))
}

fn projected_margin(a: PixelSpan, b: PixelSpan) -> f64 {
    let axis_margin = |a: HalfOpenRange, b: HalfOpenRange| {
        if a.end <= b.start {
            f64::from(b.start - a.end)
        } else if b.end <= a.start {
            f64::from(a.start - b.end)
        } else {
            0.0
        }
    };
    axis_margin(a.x, b.x).max(axis_margin(a.y, b.y))
}

fn overlap_or_touch_corridor(a: HalfOpenRange, b: HalfOpenRange) -> Option<HalfOpenRange> {
    if let Some(overlap) = intersect_range(a, b) {
        return Some(overlap);
    }
    if a.end == b.start {
        return Some(HalfOpenRange {
            start: a.end.saturating_sub(1),
            end: b.start.saturating_add(1).min(a.end.max(b.end)),
        });
    }
    if b.end == a.start {
        return Some(HalfOpenRange {
            start: b.end.saturating_sub(1),
            end: a.start.saturating_add(1).min(a.end.max(b.end)),
        });
    }
    None
}

fn csg_noninfluential(
    csg: &super::csg::CsgProgram,
    object: super::ids::ObjectId,
) -> Result<bool, String> {
    let influence = csg
        .influence
        .iter()
        .find(|influence| influence.object == object)
        .ok_or_else(|| format!("pixels::competition: missing CSG influence for {object}"))?;
    Ok(influence.when_false == influence.when_true)
}

fn dependencies_are_zero_rate(
    structural: &super::verify::StructuralProgram,
    dependencies: &[super::ids::ParamId],
) -> Result<bool, String> {
    dependencies.iter().try_fold(true, |all_static, dependency| {
        let slot = structural
            .params
            .slots
            .iter()
            .find(|slot| slot.id == *dependency)
            .ok_or_else(|| {
                format!(
                    "pixels::competition: strict-order dependency {dependency} has no parameter slot"
                )
            })?;
        Ok(all_static && slot.immutable)
    })
}

fn q_coefficient_with_builder(
    program: &super::polynomial::PolyProgram,
    q_degree: u8,
    coefficients: &mut CoeffBuilder,
) -> Result<PolyBuilder, String> {
    let mut result = PolyBuilder::zero();
    for term in &program.terms {
        if term.exponents.q == q_degree {
            let mut exponents = term.exponents;
            exponents.q = 0;
            result = result.add(
                PolyBuilder::monomial(term.coefficient, exponents, coefficients.program()),
                coefficients,
            )?;
        }
    }
    Ok(result)
}

fn affine_obligation(seed: SeedKind) -> Option<StrictSignObligation> {
    match seed {
        SeedKind::Affine { denominator } => Some(denominator),
        _ => None,
    }
}

pub(crate) fn direct_depth_side_meaning(
    a: StrictSignObligation,
    b: StrictSignObligation,
) -> EventSideMeaning {
    let product_negative = a.sign != b.sign;
    if product_negative {
        EventSideMeaning {
            negative: EventSide::DepthAFront,
            zero: EventSide::Ambiguous,
            positive: EventSide::DepthBFront,
        }
    } else {
        EventSideMeaning {
            negative: EventSide::DepthBFront,
            zero: EventSide::Ambiguous,
            positive: EventSide::DepthAFront,
        }
    }
}

fn direct_depth_cross_product(
    polynomials: &mut Vec<super::polynomial::PolyProgram>,
    coefficients: &mut CoeffBuilder,
    a: &super::projective::ProjectiveFeature,
    b: &super::projective::ProjectiveFeature,
) -> Result<Option<(PolyProgramId, StrictSignObligation, StrictSignObligation)>, String> {
    let (Some(a_denominator), Some(b_denominator)) = (
        affine_obligation(a.q_seed_kind),
        affine_obligation(b.q_seed_kind),
    ) else {
        return Ok(None);
    };
    let a_program = polynomials[a.root_equation.index()].clone();
    let b_program = polynomials[b.root_equation.index()].clone();
    let a_linear = q_coefficient_with_builder(&a_program, 1, coefficients)?;
    let a_constant = q_coefficient_with_builder(&a_program, 0, coefficients)?;
    let b_linear = q_coefficient_with_builder(&b_program, 1, coefficients)?;
    let b_constant = q_coefficient_with_builder(&b_program, 0, coefficients)?;
    // q_a - q_b = -B_a/A_a + B_b/A_b. The fixed-sign
    // denominator is A_a*A_b; the emitted numerator is exact.
    let left = b_constant.mul(a_linear, coefficients)?;
    let right = a_constant.mul(b_linear, coefficients)?;
    let numerator = left.sub(right, coefficients)?;
    let id = PolyProgramId(
        u32::try_from(polynomials.len())
            .map_err(|_| "P015: depth swap program ID overflow".to_string())?,
    );
    let numerator = numerator.finish(id, ProgramLimit::EventBivariate)?;
    polynomials.push(numerator);
    Ok(Some((id, a_denominator, b_denominator)))
}

fn taylor_depth_root_bound(a_roots: u8, b_roots: u8) -> Result<u32, String> {
    u32::from(a_roots)
        .checked_mul(u32::from(b_roots))
        .and_then(|pairs| {
            pairs.checked_mul(u32::from(TAYLOR_DEPTH_DIFFERENCE_ORDER_V1))
        })
        .ok_or_else(|| {
            format!(
                "P015: renderer capacity `depth_swap_roots` overflows u32 for {a_roots}x{b_roots} sheets"
            )
        })
}

pub(crate) fn depth_taylor_remainder_program(
    camera: super::camera::CameraContract,
    pixels: PixelSpan,
    common_q_domain: super::reference::interval::F64Interval,
    a_q_domain: super::reference::interval::F64Interval,
    b_q_domain: super::reference::interval::F64Interval,
    a: &super::derivatives::DerivativeBundle,
    b: &super::derivatives::DerivativeBundle,
) -> Result<super::events::DepthTaylorRemainderProgram, String> {
    let pixel_width =
        pixels.x.end.checked_sub(pixels.x.start).ok_or_else(|| {
            "pixels::competition: inverted depth-swap scanline domain".to_string()
        })?;
    let u_per_pixel = 2.0 * camera.aspect * camera.tan_half_fov_y / f64::from(camera.width);
    let maximum_delta = super::reference::interval::next_up(f64::from(pixel_width) * u_per_pixel);
    if !maximum_delta.is_finite() || maximum_delta < 0.0 {
        return Err("P015: depth-swap local domain is non-finite".to_string());
    }
    let local_x_domain =
        super::reference::interval::F64Interval::new(-maximum_delta, maximum_delta)?;
    let fallback_difference = complete_depth_difference(a_q_domain, b_q_domain)?;
    let fallback_remainder_abs_bound =
        super::reference::interval::next_up(fallback_difference.abs_upper());
    if !fallback_remainder_abs_bound.is_finite() {
        return Err("P015: depth-swap fallback remainder is non-finite".to_string());
    }
    Ok(super::events::DepthTaylorRemainderProgram {
        a_third: a.third.clone(),
        b_third: b.third.clone(),
        next_derivative_order: 3,
        local_x_domain,
        q_domain: common_q_domain,
        fallback_difference,
        fallback_remainder_abs_bound,
        requires_strict_g_q: true,
        discard_taylor_on_fallback: true,
    })
}

fn complete_depth_difference(
    a_q_domain: super::reference::interval::F64Interval,
    b_q_domain: super::reference::interval::F64Interval,
) -> Result<super::reference::interval::F64Interval, String> {
    a_q_domain.sub_outward(b_q_domain)
}

pub fn compile(
    equations: &mut ProjectiveEquations,
    structural: &super::verify::StructuralProgram,
    derivatives: &super::derivatives::DerivativePrograms,
    spans: &[ProjectedFeatureSpan],
    events: &mut EventPrograms,
) -> Result<CompetitionPrograms, String> {
    let mut pairs = Vec::new();
    let mut ledger = Vec::new();
    let mut pruned_projected = 0_u32;
    let mut pruned_q = 0_u32;
    let mut pruned_csg_global = 0_u32;
    let mut pruned_csg_pair = 0_u32;
    let mut pruned_strict_order = 0_u32;
    let mut coefficients = CoeffBuilder::from_program(equations.coefficients.clone())?;
    for a_index in 0..equations.features.len() {
        for b_index in (a_index + 1)..equations.features.len() {
            let a = equations.features[a_index].clone();
            let b = equations.features[b_index].clone();
            let subject = CompetitionSubject {
                a: a.feature,
                b: b.feature,
            };
            let a_span = &spans[a.feature.index()];
            let b_span = &spans[b.feature.index()];
            if a_span.pixels.x.is_empty()
                || a_span.pixels.y.is_empty()
                || b_span.pixels.x.is_empty()
                || b_span.pixels.y.is_empty()
            {
                let margin = [outside_margin(a_span)?, outside_margin(b_span)?]
                    .into_iter()
                    .flatten()
                    .reduce(f64::min)
                    .ok_or_else(|| {
                        format!(
                            "pixels::competition: empty span for {subject:?} lacks an outside-near/far proof"
                        )
                    })?;
                ledger.push(CompetitionDecision {
                    subject,
                    emitted: None,
                    omission: Some(PairOmissionHint::OutsideNearFar),
                    margin,
                    strict_sign_proof: None,
                });
                continue;
            }
            let x = overlap_or_touch_corridor(a_span.pixels.x, b_span.pixels.x);
            let y = overlap_or_touch_corridor(a_span.pixels.y, b_span.pixels.y);
            if x.is_none() || y.is_none() {
                let margin = projected_margin(a_span.pixels, b_span.pixels);
                if margin <= 0.0 {
                    return Err(format!(
                        "pixels::competition: disjoint projected pair {subject:?} has nonpositive margin {margin}"
                    ));
                }
                ledger.push(CompetitionDecision {
                    subject,
                    emitted: None,
                    omission: Some(PairOmissionHint::ProjectedBoundsDisjoint),
                    margin,
                    strict_sign_proof: None,
                });
                pruned_projected = pruned_projected
                    .checked_add(1)
                    .ok_or_else(|| "P015: projected prune count overflow".to_string())?;
                continue;
            }
            let (Some(x), Some(y)) = (x, y) else {
                return Err(
                    "pixels::competition: overlap state changed after disjointness check"
                        .to_string(),
                );
            };
            let Some(q_overlap) = a_span.q.intersect(b_span.q) else {
                let margin = (a_span.q.lo - b_span.q.hi).max(b_span.q.lo - a_span.q.hi);
                if margin <= 0.0 {
                    return Err(format!(
                        "pixels::competition: disjoint q pair {subject:?} has nonpositive margin {margin}"
                    ));
                }
                ledger.push(CompetitionDecision {
                    subject,
                    emitted: None,
                    omission: Some(PairOmissionHint::QRangesDisjoint),
                    margin,
                    strict_sign_proof: None,
                });
                pruned_q = pruned_q
                    .checked_add(1)
                    .ok_or_else(|| "P015: q prune count overflow".to_string())?;
                continue;
            };
            let a_object = structural.features[a.feature.index()].object;
            let b_object = structural.features[b.feature.index()].object;
            if csg_noninfluential(&structural.csg, a_object)?
                || csg_noninfluential(&structural.csg, b_object)?
            {
                ledger.push(CompetitionDecision {
                    subject,
                    emitted: None,
                    omission: Some(PairOmissionHint::CsgNonInfluential),
                    margin: 1.0,
                    strict_sign_proof: None,
                });
                pruned_csg_global = pruned_csg_global
                    .checked_add(1)
                    .ok_or_else(|| "P015: CSG prune count overflow".to_string())?;
                continue;
            }
            if let super::csg::PairInfluence::ProvenDisjoint {
                variable_count,
                states_checked,
            } = super::csg::pair_influence(&structural.csg, a_object, b_object)?
            {
                ledger.push(CompetitionDecision {
                    subject,
                    emitted: None,
                    omission: Some(PairOmissionHint::CsgJointInfluenceImpossible {
                        variable_count,
                        states_checked,
                    }),
                    margin: 1.0,
                    strict_sign_proof: None,
                });
                pruned_csg_pair = pruned_csg_pair
                    .checked_add(1)
                    .ok_or_else(|| "P015: pairwise CSG prune count overflow".to_string())?;
                continue;
            }
            let pixels = PixelSpan { x, y };
            let tiles = TileSpan {
                x: HalfOpenRange {
                    start: x.start / TILE_WIDTH_V1,
                    end: x.end.div_ceil(TILE_WIDTH_V1),
                },
                y: HalfOpenRange {
                    start: y.start / TILE_HEIGHT_V1,
                    end: y.end.div_ceil(TILE_HEIGHT_V1),
                },
            };
            let mut dependencies = a.influencing_params.clone();
            dependencies.extend(b.influencing_params.iter().copied());
            dependencies.sort();
            dependencies.dedup();
            let direct =
                direct_depth_cross_product(&mut equations.polynomials, &mut coefficients, &a, &b)?;
            if let Some((numerator, _, _)) = direct {
                let coefficient_intervals = super::projective::coefficient_intervals_for_roots(
                    coefficients.program(),
                    &structural.values,
                    equations.camera,
                    equations.polynomials[numerator.index()]
                        .terms
                        .iter()
                        .map(|term| term.coefficient),
                )?;
                let u_extent = equations.camera.aspect * equations.camera.tan_half_fov_y;
                let proof = super::exclusions::prove_projective_program_strict_sign(
                    &equations.polynomials[numerator.index()],
                    &coefficient_intervals,
                    [
                        super::exclusions::NormalizedAxis {
                            lo: -u_extent,
                            hi: u_extent,
                        },
                        super::exclusions::NormalizedAxis {
                            lo: -equations.camera.tan_half_fov_y,
                            hi: equations.camera.tan_half_fov_y,
                        },
                    ],
                )?;
                if let Some(proof) = proof {
                    let omission = if dependencies_are_zero_rate(structural, &dependencies)? {
                        PairOmissionHint::StaticStrictOrder
                    } else {
                        PairOmissionHint::GlobalStrictOrder
                    };
                    ledger.push(CompetitionDecision {
                        subject,
                        emitted: None,
                        omission: Some(omission),
                        margin: proof.minimum_margin,
                        strict_sign_proof: Some(proof),
                    });
                    pruned_strict_order = pruned_strict_order
                        .checked_add(1)
                        .ok_or_else(|| "P015: strict-order prune count overflow".to_string())?;
                    continue;
                }
            }
            let (representation, maximum_root_count, side_meaning) =
                if let Some((numerator, denominator_a, denominator_b)) = direct {
                    let degree =
                        u32::from(equations.polynomials[numerator.index()].degree_u.max(1));
                    (
                        EventRepresentation::DirectDepthCrossProduct {
                            numerator,
                            denominator_a,
                            denominator_b,
                        },
                        degree,
                        direct_depth_side_meaning(denominator_a, denominator_b),
                    )
                } else {
                    let maximum_root_count =
                        taylor_depth_root_bound(a.max_root_count, b.max_root_count)?;
                    let a_bundle = &derivatives.bundles[a.feature.index()];
                    let b_bundle = &derivatives.bundles[b.feature.index()];
                    let remainder = depth_taylor_remainder_program(
                        equations.camera,
                        pixels,
                        q_overlap,
                        a_span.q,
                        b_span.q,
                        a_bundle,
                        b_bundle,
                    )?;
                    (
                        EventRepresentation::TaylorDepthDifference {
                            a: a_bundle.id,
                            b: b_bundle.id,
                            taylor_order: TAYLOR_DEPTH_DIFFERENCE_ORDER_V1,
                            remainder,
                        },
                        maximum_root_count,
                        EventSideMeaning {
                            negative: EventSide::DepthBFront,
                            zero: EventSide::Ambiguous,
                            positive: EventSide::DepthAFront,
                        },
                    )
                };
            let event = EventTemplateId(
                u32::try_from(events.generators.len())
                    .map_err(|_| "P015: depth swap event ID overflow".to_string())?,
            );
            let pair = CompetitionPairId(
                u32::try_from(pairs.len())
                    .map_err(|_| "P015: competition pair ID overflow".to_string())?,
            );
            let subdivision_depth = super::capacities::checked_event_isolation_depth(
                super::capacities::PixelsCeilings::MACHINE_V1.event_isolation_depth,
            )?;
            events.generators.push(EventGenerator {
                id: event,
                kind: EventKind::DepthSwap,
                participants: SmallParticipantSet::new(&[
                    Participant::Feature(a.feature),
                    Participant::Feature(b.feature),
                ])?,
                pixels,
                tiles,
                coefficient_dependencies: dependencies,
                maximum_root_count,
                subdivision_depth,
                representation,
                side_meaning,
            });
            events.ledger.push(EventLedgerEntry {
                subject: EventSubject {
                    kind: EventKind::DepthSwap,
                    feature: None,
                    owner: None,
                    ordinal: pair.0,
                },
                emitted: Some(event),
                omission: None,
            });
            pairs.push(CompetitionPair {
                id: pair,
                a: a.feature,
                b: b.feature,
                event,
                pixels,
                tiles,
                q_overlap,
            });
            ledger.push(CompetitionDecision {
                subject,
                emitted: Some(pair),
                omission: None,
                margin: 0.0,
                strict_sign_proof: None,
            });
        }
    }
    let unique = pairs
        .iter()
        .map(|pair| (pair.a, pair.b))
        .collect::<BTreeSet<_>>();
    if unique.len() != pairs.len() {
        return Err("pixels::competition: duplicate surviving feature pair".to_string());
    }
    events.ledger.sort_by_key(|entry| entry.subject);
    equations.coefficients = coefficients.finish();
    let suppressed_same_feature = u32::try_from(equations.features.len())
        .map_err(|_| "P015: same-feature suppression count overflow".to_string())?;
    let suppressed_material_only =
        structural
            .material_events
            .iter()
            .try_fold(0_u32, |count, event| {
                count
                    .checked_add(u32::try_from(event.feature_owners.len()).map_err(|_| {
                        "P015: material-only suppression owner count overflow".to_string()
                    })?)
                    .ok_or_else(|| "P015: material-only suppression count overflow".to_string())
            })?;
    Ok(CompetitionPrograms {
        pairs,
        ledger,
        pruned_projected,
        pruned_q,
        pruned_csg_global,
        pruned_csg_pair,
        pruned_strict_order,
        suppressed_same_feature,
        suppressed_material_only,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn half_open_projection_margin_is_strict_for_omitted_pairs() {
        let a = PixelSpan {
            x: HalfOpenRange { start: 0, end: 8 },
            y: HalfOpenRange { start: 0, end: 8 },
        };
        let b = PixelSpan {
            x: HalfOpenRange { start: 9, end: 16 },
            y: HalfOpenRange { start: 0, end: 8 },
        };
        assert_eq!(projected_margin(a, b), 1.0);
        assert!(intersect_range(a.x, b.x).is_none());
    }

    #[test]
    fn direct_depth_sides_follow_the_proven_denominator_product_sign() {
        let obligation = |sign| StrictSignObligation {
            coefficient: super::super::ids::CoeffId(0),
            enclosure: match sign {
                super::super::projective::StrictSign::Positive => {
                    super::super::reference::interval::F64Interval::new(1.0, 2.0).unwrap()
                }
                super::super::projective::StrictSign::Negative => {
                    super::super::reference::interval::F64Interval::new(-2.0, -1.0).unwrap()
                }
            },
            sign,
        };
        let positive = obligation(super::super::projective::StrictSign::Positive);
        let negative = obligation(super::super::projective::StrictSign::Negative);
        assert_eq!(
            direct_depth_side_meaning(positive, positive).positive,
            EventSide::DepthAFront
        );
        assert_eq!(
            direct_depth_side_meaning(positive, negative).positive,
            EventSide::DepthBFront
        );
        assert_eq!(
            direct_depth_side_meaning(negative, positive).negative,
            EventSide::DepthAFront
        );
    }

    #[test]
    fn non_affine_depth_bound_covers_every_sheet_pair_and_quadratic_crossing() {
        assert_eq!(taylor_depth_root_bound(2, 2).unwrap(), 8);
        assert_eq!(taylor_depth_root_bound(4, 4).unwrap(), 32);
        assert_eq!(taylor_depth_root_bound(10, 10).unwrap(), 200);
        assert_eq!(taylor_depth_root_bound(u8::MAX, u8::MAX).unwrap(), 130_050);
    }

    #[test]
    fn ambiguity_fallback_uses_both_complete_sheet_domains() {
        let a = super::super::reference::interval::F64Interval::new(0.1, 0.3).unwrap();
        let b = super::super::reference::interval::F64Interval::new(0.2, 0.4).unwrap();
        let overlap = a.intersect(b).unwrap();
        let difference = complete_depth_difference(a, b).unwrap();
        let minimum_difference = a.lo - b.hi;
        let maximum_difference = a.hi - b.lo;
        assert!(difference.contains(minimum_difference));
        assert!(difference.contains(maximum_difference));
        assert!(
            !overlap
                .sub_outward(overlap)
                .unwrap()
                .contains(minimum_difference),
            "the common crossing domain is not a complete per-sheet difference domain"
        );
    }
}
