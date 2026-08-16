//! Full-frame P8 visibility conformance controls.

use std::fmt::Write as _;

use super::csg::CsgInstruction;
use super::events::EventInterval;
use super::frame::DebugPixel;
use super::iv32::Iv32;
use super::oracle::{
    CoverageCell, Interval as OracleInterval, OracleCell, OracleRoot, OracleTerminal, SemanticRay,
    Vec3 as OracleVec3, event_coverage, first_boundary, isolate_all_roots,
};
use super::rebuild::{RebuildCell, RebuildLimits, RebuildTier, TierResult};
use super::sweep::{
    ExclusionResult, FeatureId, IdentitySetId, IndexedFeature, NormalModel, ObjectId, QModel,
    RootRecord, RootSheet, SweepError,
};
use super::visibility::{
    RootIsolationSummary, TileDomain, VisibilityProgram, VisibilityWorkspace,
    render_visibility_tile,
};

#[derive(Clone, Copy, Debug, PartialEq)]
struct RenderedVisibility {
    hit: bool,
    identity: u32,
    q_lo: f64,
    q_hi: f64,
    normal_lo: [f64; 3],
    normal_hi: [f64; 3],
    tile_digest: [u8; 32],
}

/// Frame inputs and complete debug framebuffer captured from an instrumented
/// guest run. The camera and packed parameters come from the guest's own
/// validated snapshot, so the host oracle scores against the exact frame
/// inputs rather than a guessed canonical camera.
#[derive(Clone, Debug, PartialEq)]
pub struct FrameDump {
    pub camera: [f32; 12],
    pub params: [f32; 16],
    pub bytes: Vec<u8>,
    /// Three guest-emitted words per visible pixel: fixed-q and its certified
    /// radius, q derivatives, their certified radii, and the raster class.
    /// This exists only in instrumented conformance images and is independently
    /// checked against the semantic host oracle for every regular hit lane.
    pub raster_evidence: Vec<[u64; 3]>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct GuestObservation {
    pub case: String,
    pub certificate_runs: u64,
    pub event_corridors: u64,
    pub revalidated_proposals: u64,
    pub frame_digest: [u64; 4],
    pub alpha_samples: [u8; 3],
    pub visibility_probe: Option<GuestVisibilityProbe>,
    pub run_evidence: Option<[u64; 16]>,
    pub frame_dump: Option<FrameDump>,
}

pub fn debug_frame_digest(bytes: &[u8]) -> [u64; 4] {
    let mut digest = [
        1_469_598_103_934_665_603_u64,
        1_099_511_628_211_u64,
        7_809_847_782_465_536_322_u64,
        1_609_587_929_392_839_161_u64,
    ];
    for (offset, value) in bytes.iter().copied().enumerate() {
        let value = u64::from(value);
        digest[0] = (digest[0] ^ value).wrapping_mul(1_099_511_628_211);
        digest[1] = (digest[1] ^ value.wrapping_add(offset as u64))
            .wrapping_mul(14_029_467_366_897_019_727);
        digest[2] = digest[2]
            .wrapping_add(value)
            .wrapping_mul(11_400_714_785_074_694_791);
        digest[3] = (digest[3] ^ (value << (offset & 7))).wrapping_mul(9_650_029_242_287_828_579);
    }
    digest
}

/// The canonical fixture camera used by synthetic oracles when no guest
/// snapshot is available: eye (0, 0, -4.35) looking down +z with an identity
/// basis, matching `Camera.canonical` in the permanent fixtures.
const CANONICAL_CAMERA: [f32; 12] = [0.0, 0.0, -4.35, 0.0, 0.0, 1.0, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GuestVisibilityProbe {
    pub hit: bool,
    pub identity: u32,
    pub q_lo: i32,
    pub q_hi: i32,
    pub normal_valid: bool,
    pub normal: [i32; 3],
    pub coverage: u8,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct FrameScore {
    pub checked_interior: u32,
    pub interior_mismatches: u32,
    pub edge_center_violations: u32,
    pub ambiguous_identity: u32,
    pub unresolved: u32,
    /// Pixels whose point probes all missed but whose complete frustum could
    /// not be proved root-free within the fixed interval budget. This is a
    /// hard conformance failure; acceptance never silently skips a pixel.
    pub skipped_unproven: u32,
    /// Pixels the guest painted although the semantic field is provably
    /// root-free over the entire pixel frustum. These are surfaces that do
    /// not exist and therefore fail P8 conformance.
    pub phantom_surface: u32,
    pub q_checked: u32,
    pub normal_checked: u32,
    pub event_bytes_checked: u32,
    /// Event pixels whose true coverage straddles a byte-rounding boundary, so
    /// the sampled oracle settled on two adjacent bytes and accepted either.
    /// Recorded rather than hidden: a rise here means more of the frame is
    /// being judged by containment instead of exact equality.
    pub boundary_limited_event_bytes: u32,
    pub raster_evidence_failures: u32,
    /// `[x, y, kind]` for the first issue in scan order. Kinds: 1 unresolved,
    /// 2 expected-background mismatch, 3 expected-hit/identity mismatch,
    /// 4 all-hit edge became background, 5 centre-hit edge became background,
    /// 6 adjacent-identity mismatch, 7 unproved all-miss pixel, 8 dense-edge
    /// background or identity mismatch.
    pub first_issue: Option<[u16; 3]>,
}

fn merge_frame_score(total: &mut FrameScore, part: FrameScore) -> Result<(), String> {
    macro_rules! add {
        ($field:ident) => {
            total.$field = total.$field.checked_add(part.$field).ok_or_else(|| {
                concat!("frame ", stringify!($field), " counter overflow").to_string()
            })?;
        };
    }
    add!(checked_interior);
    add!(interior_mismatches);
    add!(edge_center_violations);
    add!(ambiguous_identity);
    add!(unresolved);
    add!(skipped_unproven);
    add!(phantom_surface);
    add!(q_checked);
    add!(normal_checked);
    add!(event_bytes_checked);
    add!(boundary_limited_event_bytes);
    add!(raster_evidence_failures);
    if total.first_issue.is_none() {
        total.first_issue = part.first_issue;
    }
    Ok(())
}

#[derive(Clone, Copy, Debug)]
struct RaySample {
    hit: bool,
    unresolved: u32,
    identity: Option<u32>,
}

fn sample_ray(
    evaluator: &SemanticFieldEvaluator<'_>,
    camera: &[f32; 12],
    params: &[f32; 16],
    u: f64,
    v: f64,
) -> Result<RaySample, String> {
    let score = semantic_ray_score_with(evaluator, camera, params, u, v)?;
    let identity = if score.hit && score.unresolved == 0 {
        let depth = (score.t.lo + score.t.hi) * 0.5;
        let point: [f64; 3] = std::array::from_fn(|axis| {
            f64::from(camera[axis])
                + (f64::from(camera[3 + axis])
                    + u * f64::from(camera[6 + axis])
                    + v * f64::from(camera[9 + axis]))
                    * depth
        });
        evaluator.identity_at(point, params)?
    } else {
        None
    };
    Ok(RaySample {
        hit: score.hit,
        unresolved: score.unresolved,
        identity,
    })
}

/// Prove that no semantic surface can intersect any ray through a pixel.
/// Five agreeing point samples are not such a proof: a thin or enclosed
/// feature can live wholly between them. This evaluates the complete pixel
/// ray frustum over the renderer's sealed depth domain and claims root-free
/// only when the field interval excludes zero over every sub-box.
///
/// A single evaluation of the whole frustum is badly over-approximated —
/// depth spans the entire sealed range, so the interval routinely straddles
/// zero for pixels that are obviously empty. Left unsubdivided it left most
/// of a frame unscored: `check-pixels-close-depth` proved background for 8
/// pixels out of 2048, so a guest painting a phantom surface across the
/// other 2040 would have been scored only by the 3-sample alpha lattice.
/// Bisecting the widest axis recovers those pixels at a bounded cost.
fn pixel_bundle_root_free(
    evaluator: &SemanticFieldEvaluator<'_>,
    camera: &[f32; 12],
    params: &[f32; 16],
    u0: f64,
    v0: f64,
    u1: f64,
    v1: f64,
) -> Result<bool, String> {
    // The permanent fixture corpus includes thin, repeated, displaced, and
    // near-tangent features. Keep enough subdivision headroom to prove every
    // all-miss pixel instead of silently dropping difficult cells from the
    // acceptance frame.
    pixel_bundle_root_free_with_budget(evaluator, camera, params, u0, v0, u1, v1, 32_768)
}

fn pixel_bundle_root_free_with_budget(
    evaluator: &SemanticFieldEvaluator<'_>,
    camera: &[f32; 12],
    params: &[f32; 16],
    u0: f64,
    v0: f64,
    u1: f64,
    v1: f64,
    cell_budget: usize,
) -> Result<bool, String> {
    if cell_budget == 0 {
        return Ok(false);
    }

    // The semantic field can contain dependency-heavy constructs such as a
    // folded finite repeat.  Its interval may retain zero even when every
    // independently lowered feature equation excludes zero.  The projective
    // equations are an especially useful second proof for the corner of a
    // repeated sphere's projected AABB: no feature root means no CSG surface,
    // regardless of how conservatively the source repeat expression ranges.
    if projective_bundle_root_free(
        evaluator.renderer,
        camera,
        params,
        u0,
        v0,
        u1,
        v1,
        cell_budget,
    )? {
        return Ok(true);
    }

    let mut stack = vec![(
        SemanticInterval::new(u0.min(u1), u0.max(u1))?,
        SemanticInterval::new(v0.min(v1), v0.max(v1))?,
        SemanticInterval::new(
            evaluator.renderer.config.near,
            evaluator.renderer.config.far,
        )?,
    )];
    let mut visited = 0_usize;
    while let Some((u, v, depth)) = stack.pop() {
        visited += 1;
        if visited > cell_budget {
            return Ok(false);
        }
        if field_excludes_zero(evaluator, camera, params, u, v, depth)? {
            continue;
        }
        // Split the widest axis. Depth dominates for a pixel-sized frustum,
        // which is why splitting it first is what actually retires cells.
        let widths = [u.hi - u.lo, v.hi - v.lo, depth.hi - depth.lo];
        let axis = (0..3)
            .max_by(|a, b| widths[*a].total_cmp(&widths[*b]))
            .expect("three axes");
        let target = [u, v, depth][axis];
        let middle = target.lo + (target.hi - target.lo) * 0.5;
        if !(middle > target.lo && middle < target.hi) {
            // The widest axis is already at the resolution of the format, so
            // no further split can separate this cell.
            return Ok(false);
        }
        for half in [
            SemanticInterval::new(target.lo, middle)?,
            SemanticInterval::new(middle, target.hi)?,
        ] {
            let mut child = (u, v, depth);
            match axis {
                0 => child.0 = half,
                1 => child.1 = half,
                _ => child.2 = half,
            }
            stack.push(child);
        }
    }
    Ok(true)
}

fn interval_pow(
    value: super::interval::F64Interval,
    exponent: u8,
) -> Result<super::interval::F64Interval, String> {
    let mut result = super::interval::F64Interval::point(1.0)?;
    for _ in 0..exponent {
        result = result.mul_outward(value)?;
    }
    Ok(result)
}

fn projective_coefficient_intervals(
    renderer: &crate::pixels::CompiledRenderer,
    camera: &[f32; 12],
) -> Result<Option<Vec<super::interval::F64Interval>>, String> {
    use super::interval::F64Interval;
    use crate::pixels::program::{CameraCoeff, CoeffOp};

    let equations = &renderer.projective.program().equations;
    let values = &renderer.structural.program().values;
    let mut needed = vec![false; equations.coefficients.nodes.len()];
    let mut pending = Vec::new();
    for feature in &equations.features {
        let polynomial = equations
            .polynomials
            .get(feature.root_equation.index())
            .ok_or_else(|| "projective feature root equation is absent".to_string())?;
        pending.extend(polynomial.terms.iter().map(|term| term.coefficient));
    }
    while let Some(id) = pending.pop() {
        let slot = needed
            .get_mut(id.index())
            .ok_or_else(|| "projective coefficient dependency is absent".to_string())?;
        if *slot {
            continue;
        }
        *slot = true;
        match equations.coefficients.get(id)?.op {
            CoeffOp::Add(a, b) | CoeffOp::Mul(a, b) => pending.extend([a, b]),
            CoeffOp::Neg(value) => pending.push(value),
            _ => {}
        }
    }
    let mut intervals = Vec::with_capacity(equations.coefficients.nodes.len());
    for node in &equations.coefficients.nodes {
        if !needed[node.id.index()] {
            intervals.push(F64Interval::point(0.0)?);
            continue;
        }
        let get = |id: crate::pixels::ids::CoeffId, intervals: &[F64Interval]| {
            intervals
                .get(id.index())
                .copied()
                .ok_or_else(|| format!("projective coefficient {id} names a non-predecessor"))
        };
        let interval = match node.op {
            CoeffOp::ConstF64(bits) => F64Interval::point(f64::from_bits(bits))?,
            // Structural value bounds are authoritative enclosures of every
            // runtime parameter value.  Singleton constants stay singleton;
            // parameterized coefficients merely make this optional proof
            // more conservative.
            CoeffOp::Scalar(id) => values.get(id)?,
            CoeffOp::Camera(source) => {
                let value = match source {
                    CameraCoeff::Eye(component) => camera.get(usize::from(component)),
                    CameraCoeff::Forward(component) => camera.get(3 + usize::from(component)),
                    CameraCoeff::Right(component) => camera.get(6 + usize::from(component)),
                    CameraCoeff::Up(component) => camera.get(9 + usize::from(component)),
                    CameraCoeff::TanHalfFovY => {
                        intervals.push(F64Interval::point(equations.camera.tan_half_fov_y)?);
                        continue;
                    }
                    CameraCoeff::Aspect => {
                        intervals.push(F64Interval::point(equations.camera.aspect)?);
                        continue;
                    }
                    CameraCoeff::EyeRate(_)
                    | CameraCoeff::ForwardRate(_)
                    | CameraCoeff::RightRate(_)
                    | CameraCoeff::UpRate(_) => return Ok(None),
                }
                .copied()
                .ok_or_else(|| "projective camera coefficient component overflow".to_string())?;
                F64Interval::point(f64::from(value))?
            }
            CoeffOp::ScalarParamDerivative(_, _) | CoeffOp::ParamRate(_, _) => return Ok(None),
            CoeffOp::Add(a, b) => get(a, &intervals)?.add_outward(get(b, &intervals)?)?,
            CoeffOp::Mul(a, b) => get(a, &intervals)?.mul_outward(get(b, &intervals)?)?,
            CoeffOp::Neg(value) => get(value, &intervals)?.neg(),
        };
        intervals.push(interval);
    }
    Ok(Some(intervals))
}

fn projective_polynomial_interval(
    renderer: &crate::pixels::CompiledRenderer,
    coefficients: &[super::interval::F64Interval],
    polynomial: &crate::pixels::polynomial::PolyProgram,
    params: &[f32; 16],
    u: super::interval::F64Interval,
    v: super::interval::F64Interval,
    q: super::interval::F64Interval,
) -> Result<Option<super::interval::F64Interval>, String> {
    use super::interval::F64Interval;

    let mut sum = F64Interval::point(0.0)?;
    for term in &polynomial.terms {
        // Root equations are projective in (u,v,q). X/T belong to derived
        // event programs; if one ever reaches a root equation this proof tier
        // declines instead of assigning it an unsupported value.
        if term.exponents.x != 0 || term.exponents.t != 0 {
            return Ok(None);
        }
        renderer
            .projective
            .program()
            .equations
            .coefficients
            .get(term.coefficient)?;
        let mut value = coefficients
            .get(term.coefficient.index())
            .copied()
            .ok_or_else(|| "projective polynomial coefficient is absent".to_string())?;
        value = value.mul_outward(interval_pow(u, term.exponents.u)?)?;
        value = value.mul_outward(interval_pow(v, term.exponents.v)?)?;
        value = value.mul_outward(interval_pow(q, term.exponents.q)?)?;
        for parameter in term.exponents.param_terms.iter() {
            let parameter_value = params
                .get(parameter.param.index())
                .copied()
                .ok_or_else(|| "projective polynomial parameter slot overflow".to_string())?;
            value = value.mul_outward(interval_pow(
                F64Interval::point(f64::from(parameter_value))?,
                parameter.exponent,
            )?)?;
        }
        sum = sum.add_outward(value)?;
    }
    Ok(Some(sum))
}

fn projective_quadratic_has_no_real_root(
    coefficients: &[super::interval::F64Interval],
    polynomial: &crate::pixels::polynomial::PolyProgram,
    params: &[f32; 16],
    u: super::interval::F64Interval,
    v: super::interval::F64Interval,
) -> Result<Option<bool>, String> {
    use super::interval::F64Interval;

    if polynomial.degree_q != 2 {
        return Ok(None);
    }
    let mut by_q = [F64Interval::point(0.0)?; 3];
    for term in &polynomial.terms {
        if term.exponents.q > 2 || term.exponents.x != 0 || term.exponents.t != 0 {
            return Ok(None);
        }
        let mut value = coefficients
            .get(term.coefficient.index())
            .copied()
            .ok_or_else(|| "projective polynomial coefficient is absent".to_string())?;
        value = value.mul_outward(interval_pow(u, term.exponents.u)?)?;
        value = value.mul_outward(interval_pow(v, term.exponents.v)?)?;
        for parameter in term.exponents.param_terms.iter() {
            let parameter_value = params
                .get(parameter.param.index())
                .copied()
                .ok_or_else(|| "projective polynomial parameter slot overflow".to_string())?;
            value = value.mul_outward(interval_pow(
                F64Interval::point(f64::from(parameter_value))?,
                parameter.exponent,
            )?)?;
        }
        let slot = &mut by_q[usize::from(term.exponents.q)];
        *slot = slot.add_outward(value)?;
    }
    let [c, b, a] = by_q;
    if a.lo <= 0.0 {
        return Ok(Some(false));
    }
    let discriminant = b
        .square()?
        .sub_outward(F64Interval::point(4.0)?.mul_outward(a)?.mul_outward(c)?)?;
    Ok(Some(discriminant.hi < 0.0))
}

fn projective_quadratic_bundle_root_free(
    coefficients: &[super::interval::F64Interval],
    polynomial: &crate::pixels::polynomial::PolyProgram,
    params: &[f32; 16],
    initial_u: super::interval::F64Interval,
    initial_v: super::interval::F64Interval,
    cell_budget: usize,
) -> Result<bool, String> {
    use super::interval::F64Interval;

    let full_widths = [
        initial_u.width().max(f64::MIN_POSITIVE),
        initial_v.width().max(f64::MIN_POSITIVE),
    ];
    let mut stack = vec![(initial_u, initial_v)];
    let mut visited = 0_usize;
    while let Some((u, v)) = stack.pop() {
        visited += 1;
        if visited > cell_budget {
            return Ok(false);
        }
        match projective_quadratic_has_no_real_root(coefficients, polynomial, params, u, v)? {
            Some(true) => continue,
            Some(false) => {}
            None => return Ok(false),
        }
        let domains = [u, v];
        let axis = (0..2)
            .max_by(|a, b| {
                (domains[*a].width() / full_widths[*a])
                    .total_cmp(&(domains[*b].width() / full_widths[*b]))
            })
            .expect("two projective screen axes");
        let target = domains[axis];
        let middle = target.lo + (target.hi - target.lo) * 0.5;
        if !(middle > target.lo && middle < target.hi) {
            return Ok(false);
        }
        for half in [
            F64Interval::new(target.lo, middle)?,
            F64Interval::new(middle, target.hi)?,
        ] {
            let mut child = (u, v);
            if axis == 0 {
                child.0 = half;
            } else {
                child.1 = half;
            }
            stack.push(child);
        }
    }
    Ok(true)
}

/// Prove a complete pixel frustum root-free from the independent projective
/// feature equations.  This is opportunistic: unsupported equation variables
/// or a bounded subdivision budget return `false`, never an absence claim.
fn projective_bundle_root_free(
    renderer: &crate::pixels::CompiledRenderer,
    camera: &[f32; 12],
    params: &[f32; 16],
    u0: f64,
    v0: f64,
    u1: f64,
    v1: f64,
    cell_budget: usize,
) -> Result<bool, String> {
    use super::interval::F64Interval;

    let Some(coefficients) = projective_coefficient_intervals(renderer, camera)? else {
        return Ok(false);
    };
    let equations = &renderer.projective.program().equations;
    if equations.features.is_empty()
        || equations
            .features
            .iter()
            .any(|feature| feature.deformed_predictor)
    {
        // Deformed feature equations are predictor models whose certified
        // remainder is handled by the full semantic oracle. Their raw
        // polynomial excluding zero is not, by itself, an absence proof.
        return Ok(false);
    }
    let initial_u = F64Interval::new(
        super::interval::next_down(u0.min(u1)),
        super::interval::next_up(u0.max(u1)),
    )?;
    let initial_v = F64Interval::new(
        super::interval::next_down(v0.min(v1)),
        super::interval::next_up(v0.max(v1)),
    )?;
    let mut visited = 0_usize;
    for feature in &equations.features {
        let polynomial = equations
            .polynomials
            .get(feature.root_equation.index())
            .ok_or_else(|| "projective feature root equation is absent".to_string())?;
        if polynomial.degree_q == 2 {
            let feature_budget = cell_budget
                .checked_div(equations.features.len())
                .unwrap_or(0)
                .max(64);
            if projective_quadratic_bundle_root_free(
                &coefficients,
                polynomial,
                params,
                initial_u,
                initial_v,
                feature_budget,
            )? {
                continue;
            }
            return Ok(false);
        }
        let mut stack = vec![(initial_u, initial_v, equations.camera.q)];
        while let Some((u, v, q)) = stack.pop() {
            visited += 1;
            if visited > cell_budget {
                return Ok(false);
            }
            let Some(value) = projective_polynomial_interval(
                renderer,
                &coefficients,
                polynomial,
                params,
                u,
                v,
                q,
            )?
            else {
                return Ok(false);
            };
            if !value.contains_zero() {
                continue;
            }

            // Split by relative domain width so inverse depth does not
            // dominate merely because its units are larger than a pixel's
            // screen domain. Proving one feature at a time avoids the
            // combinatorial partition formed by disjoint projected objects.
            let domains = [u, v, q];
            let full_widths = [
                (u1 - u0).abs().max(f64::MIN_POSITIVE),
                (v1 - v0).abs().max(f64::MIN_POSITIVE),
                equations.camera.q.width().max(f64::MIN_POSITIVE),
            ];
            let axis = (0..3)
                .max_by(|a, b| {
                    (domains[*a].width() / full_widths[*a])
                        .total_cmp(&(domains[*b].width() / full_widths[*b]))
                })
                .expect("three projective axes");
            let target = domains[axis];
            let middle = target.lo + (target.hi - target.lo) * 0.5;
            if !(middle > target.lo && middle < target.hi) {
                return Ok(false);
            }
            for half in [
                F64Interval::new(target.lo, middle)?,
                F64Interval::new(middle, target.hi)?,
            ] {
                let mut child = (u, v, q);
                match axis {
                    0 => child.0 = half,
                    1 => child.1 = half,
                    _ => child.2 = half,
                }
                stack.push(child);
            }
        }
    }
    Ok(true)
}

fn field_excludes_zero(
    evaluator: &SemanticFieldEvaluator<'_>,
    camera: &[f32; 12],
    params: &[f32; 16],
    u: SemanticInterval,
    v: SemanticInterval,
    depth: SemanticInterval,
) -> Result<bool, String> {
    let u_depth = u.mul(depth)?;
    let v_depth = v.mul(depth)?;
    let zero = SemanticInterval::point(0.0)?;
    let mut coordinates = [SemanticDual {
        value: zero,
        derivative: zero,
    }; 3];
    for axis in 0..3 {
        let eye = SemanticInterval::point(f64::from(camera[axis]))?;
        let forward = SemanticInterval::point(f64::from(camera[3 + axis]))?.mul(depth)?;
        let right = SemanticInterval::point(f64::from(camera[6 + axis]))?.mul(u_depth)?;
        let up = SemanticInterval::point(f64::from(camera[9 + axis]))?.mul(v_depth)?;
        coordinates[axis] = SemanticDual {
            value: eye.add(forward)?.add(right)?.add(up)?,
            derivative: zero,
        };
    }
    let field = evaluator.field_dual(coordinates, params)?;
    Ok(field.value.lo > 0.0 || field.value.hi < 0.0)
}

#[derive(Clone, Copy)]
struct PixelUvCell {
    u0: f64,
    v0: f64,
    u1: f64,
    v1: f64,
}

#[derive(Clone, Copy)]
struct PossibleDepthCell {
    interval: SemanticInterval,
    level: u8,
}

#[derive(Clone)]
struct PossiblePixelCell {
    uv: PixelUvCell,
    depths: Vec<PossibleDepthCell>,
}

/// Retain a sound enclosure of every depth at which a ray in `uv` may meet
/// the semantic surface. Depth intervals inherited from the parent UV cell
/// are already a complete enclosure, so descendants never restart from the
/// renderer's entire near/far range. Hitting either bound retains all work
/// still on the stack; it can make the enclosure looser, never unsound.
fn refine_possible_depths(
    evaluator: &SemanticFieldEvaluator<'_>,
    camera: &[f32; 12],
    params: &[f32; 16],
    uv: PixelUvCell,
    inherited: &[PossibleDepthCell],
) -> Result<Vec<PossibleDepthCell>, String> {
    const MAX_DEPTH_LEVEL: u8 = 20;
    const CELL_BUDGET: usize = 8_192;

    let u = SemanticInterval::new(uv.u0.min(uv.u1), uv.u0.max(uv.u1))?;
    let v = SemanticInterval::new(uv.v0.min(uv.v1), uv.v0.max(uv.v1))?;
    let mut stack = inherited.to_vec();
    let mut retained = Vec::new();
    let mut visited = 0_usize;
    while let Some(cell) = stack.pop() {
        if visited == CELL_BUDGET {
            retained.push(cell);
            retained.extend(stack);
            break;
        }
        visited += 1;
        if field_excludes_zero(evaluator, camera, params, u, v, cell.interval)? {
            continue;
        }
        if cell.level == MAX_DEPTH_LEVEL {
            retained.push(cell);
            continue;
        }
        let middle = cell.interval.lo + (cell.interval.hi - cell.interval.lo) * 0.5;
        if !(middle > cell.interval.lo && middle < cell.interval.hi) {
            retained.push(cell);
            continue;
        }
        let next_level = cell.level + 1;
        stack.push(PossibleDepthCell {
            interval: SemanticInterval::new(middle, cell.interval.hi)?,
            level: next_level,
        });
        stack.push(PossibleDepthCell {
            interval: SemanticInterval::new(cell.interval.lo, middle)?,
            level: next_level,
        });
    }
    Ok(retained)
}

fn possible_coverage_rounds_to_zero(possible_cells: usize, level: u32) -> bool {
    let total_cells = 4_u64.pow(level);
    (possible_cells as u64).saturating_mul(510) < total_cells
}

/// Resolve a pixel that has five miss samples but whose complete ray bundle
/// cannot be proved root-free. Point sampling alone is not an acceptance
/// proof: an enclosed feature can fall between every sample. This dyadic
/// refinement retains every cell that may contain a semantic root. A
/// displayed surface must be witnessed by an independent ray with the same
/// identity. Background is accepted only when the total retained cell area
/// is small enough that even 100% coverage of every retained cell rounds to
/// the zero coverage byte.
fn resolve_unproven_pixel(
    evaluator: &SemanticFieldEvaluator<'_>,
    camera: &[f32; 12],
    params: &[f32; 16],
    initial: PixelUvCell,
    guest_background: bool,
    guest_identity: u32,
) -> Result<(std::collections::BTreeSet<u32>, u32, bool), String> {
    const MAX_LEVEL: u32 = 11;

    let initial_depth = PossibleDepthCell {
        interval: SemanticInterval::new(
            evaluator.renderer.config.near,
            evaluator.renderer.config.far,
        )?,
        level: 0,
    };
    let mut possible = vec![PossiblePixelCell {
        uv: initial,
        depths: vec![initial_depth],
    }];
    let mut identities = std::collections::BTreeSet::new();
    let mut unresolved = 0_u32;
    let mut matching_identity_witnessed = false;
    for level in 1..=MAX_LEVEL {
        let mut next = Vec::new();
        for cell in possible {
            let um = cell.uv.u0 + (cell.uv.u1 - cell.uv.u0) * 0.5;
            let vm = cell.uv.v0 + (cell.uv.v1 - cell.uv.v0) * 0.5;
            for child in [
                PixelUvCell {
                    u0: cell.uv.u0,
                    v0: cell.uv.v0,
                    u1: um,
                    v1: vm,
                },
                PixelUvCell {
                    u0: um,
                    v0: cell.uv.v0,
                    u1: cell.uv.u1,
                    v1: vm,
                },
                PixelUvCell {
                    u0: cell.uv.u0,
                    v0: vm,
                    u1: um,
                    v1: cell.uv.v1,
                },
                PixelUvCell {
                    u0: um,
                    v0: vm,
                    u1: cell.uv.u1,
                    v1: cell.uv.v1,
                },
            ] {
                let depths =
                    refine_possible_depths(evaluator, camera, params, child, &cell.depths)?;
                if depths.is_empty() {
                    continue;
                }
                if !guest_background {
                    let sample = sample_ray(
                        evaluator,
                        camera,
                        params,
                        child.u0 + (child.u1 - child.u0) * 0.5,
                        child.v0 + (child.v1 - child.v0) * 0.5,
                    )?;
                    unresolved = unresolved
                        .checked_add(sample.unresolved)
                        .ok_or_else(|| "frame unresolved counter overflow".to_string())?;
                    if let Some(identity) = sample.identity {
                        identities.insert(identity);
                        matching_identity_witnessed |= identity == guest_identity;
                    }
                }
                next.push(PossiblePixelCell { uv: child, depths });
            }
        }
        possible = next;
        if possible.is_empty() {
            return Ok((identities, unresolved, true));
        }
        if !guest_background && matching_identity_witnessed {
            return Ok((identities, unresolved, false));
        }
        if guest_background {
            // Each level partitions the pixel into 4^level equal cells. If
            // every retained cell were covered, the exact area is still too
            // small to round to one byte: possible / 4^level * 255 < 1/2.
            if possible_coverage_rounds_to_zero(possible.len(), level) {
                return Ok((identities, unresolved, true));
            }
        }
    }
    Ok((identities, unresolved, false))
}

/// Score every pixel of a guest debug frame against the independent semantic
/// oracle. Interior pixels (all four corner rays and the centre ray agree on
/// hit state and identity) must match the guest exactly; silhouette-adjacent
/// pixels are held to the weaker-but-sound rules that a centre-hit pixel is
/// never background and any displayed identity belongs to an adjacent
/// surface. Oracle-unresolved rays are counted and fail the case.
fn derivative_enclosure_contains(
    raw: i32,
    radius: i64,
    q_scale: f64,
    q_lo: f64,
    q_hi: f64,
    factor: f64,
) -> bool {
    let certified_lo = (i64::from(raw) - radius) as f64 * q_scale;
    let certified_hi = (i64::from(raw) + radius) as f64 * q_scale;
    let oracle_a = q_lo * factor;
    let oracle_b = q_hi * factor;
    [certified_lo, certified_hi, oracle_a, oracle_b]
        .into_iter()
        .all(f64::is_finite)
        && certified_lo <= oracle_a.min(oracle_b)
        && certified_hi >= oracle_a.max(oracle_b)
}

fn expected_event_arena_bounds(
    renderer: &crate::pixels::CompiledRenderer,
    x: u32,
    y: u32,
) -> Result<(u16, u16, u16, u32), String> {
    let mut count = 0_u16;
    let mut first = 0_u16;
    let mut last = 0_u16;
    let mut digest = 2_166_136_261_u32;
    for event in &renderer.projective.program().events.generators {
        if x >= event.pixels.x.start
            && x < event.pixels.x.end
            && y >= event.pixels.y.start
            && y < event.pixels.y.end
        {
            let id = u16::try_from(event.id.0)
                .map_err(|_| "event ID exceeds compact EventPixel domain".to_string())?;
            if count == 0 {
                first = id;
            }
            last = id;
            digest = digest.wrapping_mul(16_777_619) ^ u32::from(id);
            count = count
                .checked_add(1)
                .ok_or_else(|| "event arena count overflow".to_string())?;
        }
    }
    Ok((count, first, last, digest & 0x3fff_ffff))
}

/// Uniform-grid reference retained for the diagnostic probe and unit tests;
/// the acceptance path uses the adaptive quadtree oracle below.
#[cfg(test)]
#[derive(Clone, Debug, PartialEq, Eq)]
struct EventGridOracle {
    counts: std::collections::BTreeMap<u32, u64>,
    background: u64,
    samples: u64,
}

#[cfg(test)]
fn event_grid_oracle(
    evaluator: &SemanticFieldEvaluator<'_>,
    camera: &[f32; 12],
    params: &[f32; 16],
    u_at: impl Fn(f64) -> f64,
    v_at: impl Fn(f64) -> f64,
    x: usize,
    y: usize,
    resolution: u32,
) -> Result<EventGridOracle, String> {
    if resolution == 0 {
        return Err("event coverage oracle has zero resolution".to_string());
    }
    let mut counts = std::collections::BTreeMap::new();
    let mut background = 0_u64;
    for sy in 0..resolution {
        for sx in 0..resolution {
            let local_x = (f64::from(sx) + 0.5) / f64::from(resolution);
            let local_y = (f64::from(sy) + 0.5) / f64::from(resolution);
            let score = semantic_ray_score_with(
                evaluator,
                camera,
                params,
                u_at(x as f64 + local_x),
                v_at(y as f64 + local_y),
            )?;
            if score.unresolved != 0 {
                return Err(format!(
                    "event coverage oracle is unresolved at ({x},{y}) subpixel ({sx},{sy})"
                ));
            }
            if !score.hit {
                background = background
                    .checked_add(1)
                    .ok_or_else(|| "event coverage background count overflow".to_string())?;
                continue;
            }
            let depth = (score.t.lo + score.t.hi) * 0.5;
            let u = u_at(x as f64 + local_x);
            let v = v_at(y as f64 + local_y);
            let point: [f64; 3] = std::array::from_fn(|axis| {
                f64::from(camera[axis])
                    + (f64::from(camera[3 + axis])
                        + u * f64::from(camera[6 + axis])
                        + v * f64::from(camera[9 + axis]))
                        * depth
            });
            let identity = evaluator.winner_identity_at(point, params)?;
            let count = counts.entry(identity).or_insert(0_u64);
            *count = count
                .checked_add(1)
                .ok_or_else(|| "event coverage identity count overflow".to_string())?;
        }
    }
    let samples = u64::from(resolution)
        .checked_mul(u64::from(resolution))
        .ok_or_else(|| "event coverage sample count overflow".to_string())?;
    if counts.values().copied().sum::<u64>() + background != samples {
        return Err("event coverage oracle did not classify every subpixel".to_string());
    }
    Ok(EventGridOracle {
        counts,
        background,
        samples,
    })
}

/// Round one grid's winner-sample fraction to a display byte.
///
/// This deliberately does *not* Richardson-extrapolate the two grids. That
/// extrapolation (`2*fine - coarse`) assumes the error is a smooth `O(h)` term
/// it can cancel, but the error here is lattice point-counting against a curved
/// region, which is jagged in `h` and has no such expansion — so extrapolating
/// amplified sampling noise instead of removing it. Measured on
/// `check-pixels-hard-csg`, where the guest states 198 at pixel (32,11) and 141
/// at (41,12): the raw grid byte converges to exactly those values by 512
/// samples per axis (198 at 256/512/1024/2048/4096; 141 at 512/1024/2048/4096),
/// while the extrapolation reported 197 and 140 from the very same grids.
/// `converged_coverage_byte` below states a byte only when two successive
/// resolutions agree on it, which is a convergence proof rather than a guess.
/// Lattice span of the quadtree event oracle: pixel-local coordinates run
/// over `0..=EVENT_ORACLE_SPAN` per axis, so the finest cell is
/// 1/4096 pixel — the same limiting resolution the retired uniform-grid
/// ladder capped at.
const EVENT_ORACLE_SPAN: u32 = 4096;

/// Largest cell (in lattice units) the quadtree may classify from agreeing
/// probes: 128 units is 1/32 pixel, so every classification rests on probes
/// at least as dense as a 32-per-axis grid even in featureless regions. A
/// larger cell risks a feature slipping between five agreeing probes; a
/// smaller one only multiplies the cost of provably boring area.
const EVENT_ORACLE_MAX_CLASSIFY_SPAN: u32 = 128;

/// Smallest cell the quadtree subdivides to (2 units = 1/2048 pixel). A cell
/// this small whose probes still disagree straddles the region boundary and
/// becomes uncertainty. A curve of a few pixel-widths length crossing the
/// pixel leaves roughly `3 * 2048` such cells — about 0.4 of one display
/// byte — so the reported byte interval is a single value or an adjacent
/// pair, never a guess.
const EVENT_ORACLE_MIN_LEAF_SPAN: u32 = 2;

fn grid_coverage_byte(count: u64, samples: u64) -> Result<u8, String> {
    if samples == 0 {
        return Err("event coverage grid has no samples".to_string());
    }
    let encoded = u128::from(count)
        .checked_mul(255)
        .and_then(|value| value.checked_add(u128::from(samples) / 2))
        .and_then(|value| value.checked_div(u128::from(samples)))
        .ok_or_else(|| "event coverage byte conversion overflow".to_string())?;
    u8::try_from(encoded).map_err(|_| "event coverage oracle is not a byte".to_string())
}

/// Label of one lattice probe inside an event pixel.
///
/// `Unresolved` is a real verdict, not an error: near an exact edge (a hard
/// CSG box face grazing the pixel) the interval evaluator legitimately
/// cannot classify a thin strip of rays. The retired uniform grids never
/// sampled densely enough to land in that strip; the quadtree deliberately
/// chases the boundary, so it will. An unresolved probe makes its cell
/// unclassifiable — the cell subdivides, and at the leaf its area joins the
/// uncertainty bound, which still fails the pixel honestly if the
/// unresolved measure ever exceeds one display byte.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum EventProbeLabel {
    Background,
    Hit(u32),
    Unresolved,
}

/// Adaptive quadtree replacement for the retired uniform-grid ladder.
///
/// The uniform grids spent almost all of their samples re-measuring area that
/// was never in doubt: a grid at resolution `R` costs `R^2` evaluations while
/// only the `O(R)` cells crossed by the region boundary carry information, so
/// settling a curved-silhouette pixel cost millions of ~100µs field
/// evaluations. The quadtree subdivides the pixel instead: a cell no larger
/// than [`EVENT_ORACLE_MAX_CLASSIFY_SPAN`] whose four corners and center all
/// carry one label contributes its exact dyadic area to that label; a
/// disagreeing cell subdivides, and a disagreeing cell at
/// [`EVENT_ORACLE_MIN_LEAF_SPAN`] becomes uncertainty. Areas are integers in
/// units of the finest lattice cell, the traversal order is fixed, and probes
/// are memoized on the shared corner lattice, so the result is deterministic
/// and independent of any parallel schedule around it.
///
/// The returned byte pair `[low, high]` is `[classified winner area, that
/// plus all uncertain area]`: a single byte when rounding is settled and an
/// adjacent pair when the true coverage sits on a rounding boundary — the
/// same acceptance contract the ladder's straddle rule provided, now derived
/// from an area bound instead of watching estimates alternate.
fn sampled_event_oracle(
    evaluator: &SemanticFieldEvaluator<'_>,
    camera: &[f32; 12],
    params: &[f32; 16],
    u_at: impl Fn(f64) -> f64 + Copy,
    v_at: impl Fn(f64) -> f64 + Copy,
    x: usize,
    y: usize,
) -> Result<(u32, u32, u8, u8), String> {
    let span = EVENT_ORACLE_SPAN;
    let mut probes: std::collections::BTreeMap<(u32, u32), EventProbeLabel> =
        std::collections::BTreeMap::new();
    let probe = |probes: &mut std::collections::BTreeMap<(u32, u32), EventProbeLabel>,
                 i: u32,
                 j: u32|
     -> Result<EventProbeLabel, String> {
        if let Some(label) = probes.get(&(i, j)) {
            return Ok(*label);
        }
        // Half-leaf inset: lattice index 0 and `span` land strictly inside
        // the pixel rather than on its boundary. A scene edge can lie
        // exactly on a pixel corner ray (`check-pixels-hard-csg`'s box faces
        // do, at (31,11)), where the evaluator is legitimately unresolved —
        // the retired uniform grids never sampled boundaries either, for the
        // same reason. Probes witness their cell's label; they are not the
        // cell's geometry, so the inset does not bias the dyadic areas.
        let u = u_at(x as f64 + (f64::from(i) + 0.5) / (f64::from(span) + 1.0));
        let v = v_at(y as f64 + (f64::from(j) + 0.5) / (f64::from(span) + 1.0));
        let score = semantic_ray_score_with(evaluator, camera, params, u, v)?;
        if score.unresolved != 0 {
            probes.insert((i, j), EventProbeLabel::Unresolved);
            return Ok(EventProbeLabel::Unresolved);
        }
        let label = if score.hit {
            let depth = (score.t.lo + score.t.hi) * 0.5;
            let point: [f64; 3] = std::array::from_fn(|axis| {
                f64::from(camera[axis])
                    + (f64::from(camera[3 + axis])
                        + u * f64::from(camera[6 + axis])
                        + v * f64::from(camera[9 + axis]))
                        * depth
            });
            EventProbeLabel::Hit(evaluator.winner_identity_at(point, params)?)
        } else {
            EventProbeLabel::Background
        };
        probes.insert((i, j), label);
        Ok(label)
    };
    let mut areas: std::collections::BTreeMap<u32, u64> = std::collections::BTreeMap::new();
    let mut background_area = 0_u64;
    let mut uncertain_area = 0_u64;
    // Explicit worklist rather than recursion: cells are pushed in reverse so
    // traversal is depth-first in fixed child order, keeping the memoized
    // probe pattern (and therefore the cost) reproducible.
    let mut worklist: Vec<(u32, u32, u32)> = vec![(0, 0, span)];
    while let Some((i0, j0, size)) = worklist.pop() {
        let half = size / 2;
        let corners = [
            probe(&mut probes, i0, j0)?,
            probe(&mut probes, i0 + size, j0)?,
            probe(&mut probes, i0, j0 + size)?,
            probe(&mut probes, i0 + size, j0 + size)?,
        ];
        let center = probe(&mut probes, i0 + half, j0 + half)?;
        let unanimous =
            corners.iter().all(|label| *label == center) && center != EventProbeLabel::Unresolved;
        if unanimous && size <= EVENT_ORACLE_MAX_CLASSIFY_SPAN {
            let area = u64::from(size) * u64::from(size);
            match center {
                EventProbeLabel::Background => background_area += area,
                EventProbeLabel::Hit(identity) => {
                    *areas.entry(identity).or_insert(0) += area;
                }
                EventProbeLabel::Unresolved => unreachable!("unanimity excludes unresolved"),
            }
        } else if size <= EVENT_ORACLE_MIN_LEAF_SPAN {
            uncertain_area += u64::from(size) * u64::from(size);
        } else {
            worklist.push((i0 + half, j0 + half, half));
            worklist.push((i0, j0 + half, half));
            worklist.push((i0 + half, j0, half));
            worklist.push((i0, j0, half));
        }
    }
    let total_area = u64::from(span) * u64::from(span);
    debug_assert_eq!(
        areas.values().sum::<u64>() + background_area + uncertain_area,
        total_area
    );
    let center_label = probe(&mut probes, span / 2, span / 2)?;
    let winner = match center_label {
        EventProbeLabel::Hit(identity) => Some(identity),
        EventProbeLabel::Background | EventProbeLabel::Unresolved => areas
            .iter()
            .max_by_key(|(identity, area)| (**area, std::cmp::Reverse(**identity)))
            .map(|(identity, _)| *identity)
            .or_else(|| {
                // The classified area can be empty while a thin feature still
                // crosses the probe lattice inside uncertain cells; any hit
                // probe then names the feature.
                probes.values().find_map(|label| match label {
                    EventProbeLabel::Hit(identity) => Some(*identity),
                    EventProbeLabel::Background | EventProbeLabel::Unresolved => None,
                })
            }),
    };
    let Some(winner) = winner else {
        // Event spans are conservative. If every probe and every classified
        // cell is background, the exact visible byte is zero and the
        // premultiplied winner identity is intentionally absent.
        if uncertain_area == 0 {
            return Ok((0, 0, 0, 0));
        }
        return Err(format!(
            "event coverage oracle did not select a winner at ({x},{y})"
        ));
    };
    let winner_area = areas.get(&winner).copied().unwrap_or(0);
    let coverage_low = grid_coverage_byte(winner_area, total_area)?;
    let coverage_high = grid_coverage_byte(winner_area + uncertain_area, total_area)?;
    if coverage_high.saturating_sub(coverage_low) > 1 {
        return Err(format!(
            "event coverage oracle did not settle at ({x},{y}) by 1/{span} pixel cells: \
             winner area spans {coverage_low}..={coverage_high}"
        ));
    }
    let back = areas
        .iter()
        .filter(|(identity, _)| **identity != winner)
        .max_by_key(|(identity, area)| (**area, std::cmp::Reverse(**identity)))
        .map(|(identity, _)| *identity)
        .unwrap_or(0);
    Ok((winner, back, coverage_low, coverage_high))
}

fn exact_fixture_event_byte(case: &str, x: usize, y: usize) -> Result<Option<u8>, String> {
    use super::coverage::{BoundaryOwner, HalfPlane, half_plane_area, half_plane_byte};
    if case == "check-pixels-tile-boundary" {
        let horizontal_event = matches!(y, 13 | 17);
        let vertical_event = matches!(x, 50 | 51 | 76 | 77);
        if !(13..=17).contains(&y)
            || !(50..=77).contains(&x)
            || (!horizontal_event && !vertical_event)
        {
            return Err(format!(
                "tile-boundary event evidence lies outside the exact owned lanes at ({x},{y})"
            ));
        }
        let axis_byte = |a, b, c| -> Result<u8, String> {
            let area = half_plane_area(
                HalfPlane { a, b, c },
                super::iv32::FixedDomain::full(-8),
                BoundaryOwner::LowerOrLeft,
            )
            .map_err(|error| format!("tile-boundary event oracle: {error}"))?;
            if area.lo != area.hi {
                return Err(format!("tile-boundary event oracle is not exact: {area:?}"));
            }
            u8::try_from(area.lo)
                .map_err(|_| format!("tile-boundary event oracle is not a byte: {area:?}"))
        };
        let x_coverage = if matches!(x, 50 | 77) {
            axis_byte(-256, 0, 225)?
        } else {
            255
        };
        let y_coverage = if horizontal_event {
            axis_byte(0, -256, 207)?
        } else {
            255
        };
        return u8::try_from((u16::from(x_coverage) * u16::from(y_coverage) + 127) / 255)
            .map(Some)
            .map_err(|_| "tile-boundary event coverage overflow".to_string());
    }
    if case != "check-pixels-material-edge" {
        return Ok(None);
    }
    let x = i64::try_from(x).map_err(|_| "material oracle x overflow".to_string())?;
    let y = i64::try_from(y).map_err(|_| "material oracle y overflow".to_string())?;
    let d = x - y - 16;
    if !matches!(d, 0 | 1) {
        return Err(format!(
            "material event evidence lies outside the exact diagonal lanes at ({x},{y})"
        ));
    }
    let positive = HalfPlane {
        a: 32,
        b: -32,
        c: 32 * d - 11,
    };
    let (selected, owner) = if d == 1 {
        (positive, BoundaryOwner::LowerOrLeft)
    } else {
        (
            HalfPlane {
                a: -positive.a,
                b: -positive.b,
                c: -positive.c,
            },
            BoundaryOwner::UpperOrRight,
        )
    };
    half_plane_byte(selected, owner)
        .map(Some)
        .map_err(|error| format!("material event oracle: {error}"))
}

pub fn score_frame(
    case: &str,
    renderer: &crate::pixels::CompiledRenderer,
    dump: &FrameDump,
) -> Result<FrameScore, String> {
    let width = renderer.config.width as usize;
    let height = renderer.config.height as usize;
    if dump.bytes.len() != width * height * 4 {
        return Err(format!(
            "frame dump is {} bytes, expected {}",
            dump.bytes.len(),
            width * height * 4
        ));
    }
    if dump.raster_evidence.len() != width * height {
        return Err(format!(
            "frame raster evidence has {} pixels, expected {}",
            dump.raster_evidence.len(),
            width * height
        ));
    }
    let fixed_q = renderer
        .program
        .program()
        .table(wrela_machine::pixels::FrameProgramTableKindV1::FixedDomain)
        .and_then(|table| table.records.iter().find(|record| record.tag == 5))
        .ok_or_else(|| "fixed-q record is missing from frame program".to_string())?;
    let q_scale = 2.0_f64.powi(fixed_q.operands[0] as i64 as i32);
    let aspect = width as f64 / height as f64;
    let u_at = |px: f64| (px / width as f64 * 2.0 - 1.0) * aspect;
    let v_at = |py: f64| 1.0 - py / height as f64 * 2.0;
    // The sampled event oracle dominates scoring — thousands of field
    // evaluations per event pixel — and every event pixel is independent of
    // every other. Evaluate them all on a worker pool up front and let the
    // scan-order loop below consume the finished results, so the score, the
    // first-failure choice, and every compared byte are identical to a
    // serial evaluation. Workers own their evaluators because the
    // evaluator's dual-number scratch is single-threaded by design.
    let event_pixels: Vec<(usize, usize)> = (0..height)
        .flat_map(|y| (0..width).map(move |x| (x, y)))
        .filter(|&(x, y)| (dump.raster_evidence[y * width + x][2] >> 62) == 2)
        .collect();
    let event_oracles: std::collections::BTreeMap<
        (usize, usize),
        Result<(u32, u32, u8, u8), String>,
    > = if event_pixels.is_empty() {
        std::collections::BTreeMap::new()
    } else {
        // Two cores of headroom: the conformance gate scores cases while two
        // instrumented guest vCPUs are still running, and a full-width pool
        // would time-slice against them.
        let workers = std::thread::available_parallelism()
            .map(|count| count.get().saturating_sub(2).max(2))
            .unwrap_or(4)
            .min(event_pixels.len());
        let evaluators = (0..workers)
            .map(|_| SemanticFieldEvaluator::new(renderer))
            .collect::<Result<Vec<_>, _>>()?;
        let slots: Vec<std::sync::Mutex<Option<Result<(u32, u32, u8, u8), String>>>> = event_pixels
            .iter()
            .map(|_| std::sync::Mutex::new(None))
            .collect();
        let cursor = std::sync::atomic::AtomicUsize::new(0);
        let event_pixels_ref = &event_pixels;
        let slots_ref = &slots;
        let cursor_ref = &cursor;
        std::thread::scope(|scope| {
            for worker_evaluator in evaluators {
                scope.spawn(move || {
                    loop {
                        let index = cursor_ref.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                        let Some(&(x, y)) = event_pixels_ref.get(index) else {
                            return;
                        };
                        let result = sampled_event_oracle(
                            &worker_evaluator,
                            &dump.camera,
                            &dump.params,
                            u_at,
                            v_at,
                            x,
                            y,
                        );
                        *slots_ref[index].lock().unwrap_or_else(|e| e.into_inner()) = Some(result);
                    }
                });
            }
        });
        event_pixels
            .iter()
            .copied()
            .zip(slots.into_iter().map(|slot| {
                slot.into_inner()
                    .unwrap_or_else(|e| e.into_inner())
                    .unwrap_or_else(|| Err("event oracle worker omitted a pixel".to_string()))
            }))
            .collect()
    };
    let score_pixel = |worker_evaluator: &SemanticFieldEvaluator<'_>,
                       x: usize,
                       y: usize|
     -> Result<(FrameScore, Vec<String>), String> {
        let mut score = FrameScore::default();
        let mut event_coverage_mismatches: Vec<String> = Vec::new();
        let center = sample_ray(
            worker_evaluator,
            &dump.camera,
            &dump.params,
            u_at(x as f64 + 0.5),
            v_at(y as f64 + 0.5),
        )?;
        // Pull the four probes into the pixel. Exact shared pixel edges
        // commonly coincide with a tangent or CSG seam; they carry zero
        // area and should classify the pixel as an edge, not make the
        // independent oracle unresolved.
        let quad = [
            sample_ray(
                worker_evaluator,
                &dump.camera,
                &dump.params,
                u_at(x as f64 + 0.125),
                v_at(y as f64 + 0.125),
            )?,
            sample_ray(
                worker_evaluator,
                &dump.camera,
                &dump.params,
                u_at(x as f64 + 0.875),
                v_at(y as f64 + 0.125),
            )?,
            sample_ray(
                worker_evaluator,
                &dump.camera,
                &dump.params,
                u_at(x as f64 + 0.125),
                v_at(y as f64 + 0.875),
            )?,
            sample_ray(
                worker_evaluator,
                &dump.camera,
                &dump.params,
                u_at(x as f64 + 0.875),
                v_at(y as f64 + 0.875),
            )?,
        ];
        let samples = [center, quad[0], quad[1], quad[2], quad[3]];
        let pixel_unresolved: u32 = samples.iter().map(|sample| sample.unresolved).sum();
        if pixel_unresolved > 0 {
            score.unresolved = score
                .unresolved
                .checked_add(pixel_unresolved)
                .ok_or_else(|| "frame unresolved counter overflow".to_string())?;
            if score.first_issue.is_none() {
                score.first_issue = Some([x as u16, y as u16, 1]);
            }
            return Ok((score, event_coverage_mismatches));
        }
        let base = (y * width + x) * 4;
        let guest = &dump.bytes[base..base + 4];
        let evidence = dump.raster_evidence[y * width + x];
        let signed32 = |bits: u64| (bits as u32) as i32;
        let q_raw = signed32(evidence[0]);
        let q_radius = i64::from(((evidence[0] >> 32) & 0x7fff_ffff) as u32);
        let q_u_raw = signed32(evidence[1]);
        let q_v_raw = signed32(evidence[1] >> 32);
        let q_u_radius = i64::from((evidence[2] & 0x7fff_ffff) as u32);
        let q_v_radius = i64::from(((evidence[2] >> 31) & 0x7fff_ffff) as u32);
        let evidence_class = evidence[2] >> 62;
        // P8 reserves alpha for the opaque scanout contract. Debug
        // visibility lives in RGB: blue carries the q/coverage code and
        // red/green carry the identity. A zero RGB triplet is therefore
        // the unique background representation.
        if guest[3] != 255 {
            return Err(format!(
                "frame alpha at ({x},{y}) is {}, expected opaque 255",
                guest[3]
            ));
        }
        let guest_background = guest[..3] == [0, 0, 0];
        let guest_identity = u32::from(guest[1]) | (u32::from(guest[2]) << 8);
        match evidence_class {
            1 => {
                score.q_checked += 1;
                if center.unresolved != 0 || !center.hit {
                    score.raster_evidence_failures += 1;
                } else {
                    let oracle = semantic_ray_score_with(
                        worker_evaluator,
                        &dump.camera,
                        &dump.params,
                        u_at(x as f64 + 0.5),
                        v_at(y as f64 + 0.5),
                    )?;
                    let q = f64::from(q_raw) * q_scale;
                    let certified_lo = (i64::from(q_raw) - q_radius) as f64 * q_scale;
                    let certified_hi = (i64::from(q_raw) + q_radius) as f64 * q_scale;
                    let q_lo = 1.0 / oracle.t.hi;
                    let q_hi = 1.0 / oracle.t.lo;
                    if oracle.unresolved != 0
                        || !oracle.hit
                        || q < certified_lo
                        || q > certified_hi
                        || certified_lo > q_lo
                        || certified_hi < q_hi
                    {
                        score.raster_evidence_failures += 1;
                    }
                    score.normal_checked += 1;
                    let q_u = f64::from(q_u_raw) * q_scale;
                    let q_v = f64::from(q_v_raw) * q_scale;
                    let scalar_u = u_at(x as f64 + 0.5);
                    let scalar_v = v_at(y as f64 + 0.5);
                    let camera_z = q - scalar_u * q_u - scalar_v * q_v;
                    let mut normal = [
                        f64::from(dump.camera[6]) * q_u
                            + f64::from(dump.camera[9]) * q_v
                            + f64::from(dump.camera[3]) * camera_z,
                        f64::from(dump.camera[7]) * q_u
                            + f64::from(dump.camera[10]) * q_v
                            + f64::from(dump.camera[4]) * camera_z,
                        f64::from(dump.camera[8]) * q_u
                            + f64::from(dump.camera[11]) * q_v
                            + f64::from(dump.camera[5]) * camera_z,
                    ];
                    let raw_length_squared: f64 = normal.into_iter().map(|v| v * v).sum();
                    if !raw_length_squared.is_finite() || raw_length_squared <= 0.0 {
                        score.raster_evidence_failures += 1;
                        return Ok((score, event_coverage_mismatches));
                    }
                    let inverse_length = raw_length_squared.sqrt().recip();
                    normal.iter_mut().for_each(|value| *value *= inverse_length);
                    let length_squared: f64 = normal.into_iter().map(|v| v * v).sum();
                    if !(0.999_999..=1.000_001).contains(&length_squared) {
                        score.raster_evidence_failures += 1;
                    }
                    let expected = [oracle.normal.x, oracle.normal.y, oracle.normal.z];
                    let camera_nu = f64::from(dump.camera[6]) * expected[0]
                        + f64::from(dump.camera[7]) * expected[1]
                        + f64::from(dump.camera[8]) * expected[2];
                    let camera_nv = f64::from(dump.camera[9]) * expected[0]
                        + f64::from(dump.camera[10]) * expected[1]
                        + f64::from(dump.camera[11]) * expected[2];
                    let camera_nz = f64::from(dump.camera[3]) * expected[0]
                        + f64::from(dump.camera[4]) * expected[1]
                        + f64::from(dump.camera[5]) * expected[2];
                    let normal_scale = camera_nz + scalar_u * camera_nu + scalar_v * camera_nv;
                    if !normal_scale.is_finite() || normal_scale.abs() <= f64::EPSILON {
                        score.raster_evidence_failures += 1;
                    } else {
                        if !derivative_enclosure_contains(
                            q_u_raw,
                            q_u_radius,
                            q_scale,
                            q_lo,
                            q_hi,
                            camera_nu / normal_scale,
                        ) || !derivative_enclosure_contains(
                            q_v_raw,
                            q_v_radius,
                            q_scale,
                            q_lo,
                            q_hi,
                            camera_nv / normal_scale,
                        ) {
                            score.raster_evidence_failures += 1;
                        }
                    }
                }
            }
            2 => {
                score.event_bytes_checked += 1;
                let front_run = evidence[1] & 0xffff;
                let back_run = (evidence[1] >> 16) & 0xffff;
                let event_count = (evidence[1] >> 32) & 0xffff;
                let first_event = evidence[2] & 0xffff;
                let last_event = (evidence[2] >> 16) & 0xffff;
                let event_digest = (evidence[2] >> 32) & 0x3fff_ffff;
                let expected = expected_event_arena_bounds(renderer, x as u32, y as u32)?;
                let (oracle_identity, oracle_back_identity, sampled_low, sampled_high) =
                    event_oracles
                        .get(&(x, y))
                        .cloned()
                        .ok_or_else(|| format!("event oracle result is missing at ({x},{y})"))??;
                // An exact analytic oracle, where a fixture has one, states
                // a single byte and is held to it. A sampled oracle states
                // the byte range its grids settled on: normally one value,
                // and two adjacent values for a pixel whose true coverage
                // straddles a rounding boundary, where sampling genuinely
                // cannot choose between them.
                let (coverage_low, coverage_high) = match exact_fixture_event_byte(case, x, y)? {
                    Some(exact) => (exact, exact),
                    None => (sampled_low, sampled_high),
                };
                if guest[0] < coverage_low || guest[0] > coverage_high {
                    // Collect every disagreeing event byte before failing:
                    // one failing pixel says "a defect exists", the full
                    // set says whether it is a systematic integrator error
                    // or an isolated geometric configuration — the first
                    // question any investigation of this failure asks.
                    event_coverage_mismatches.push(format!(
                            "({x},{y}): guest BGRA={guest:?}, oracle coverage={coverage_low}..={coverage_high}, front_identity={oracle_identity}, back_identity={oracle_back_identity}"
                        ));
                    return Ok((score, event_coverage_mismatches));
                }
                if coverage_low != coverage_high {
                    score.boundary_limited_event_bytes += 1;
                }
                // The identities are then checked against the accepted
                // coverage, so an identity error cannot hide behind the
                // coverage tolerance and vice versa.
                let blend = |front: u8, back: u8| -> u8 {
                    let numerator = u32::from(front) * u32::from(guest[0])
                        + u32::from(back) * (255 - u32::from(guest[0]));
                    ((numerator + 127) / 255) as u8
                };
                let oracle_green = blend(
                    (oracle_identity & 0xff) as u8,
                    (oracle_back_identity & 0xff) as u8,
                );
                let oracle_red = blend(
                    ((oracle_identity >> 8) & 0xff) as u8,
                    ((oracle_back_identity >> 8) & 0xff) as u8,
                );
                if guest[1] != oracle_green || guest[2] != oracle_red {
                    return Err(format!(
                        "independent event oracle identities differ for `{case}` at ({x},{y}): guest BGRA={guest:?}, oracle green={oracle_green} red={oracle_red}, front_identity={oracle_identity}, back_identity={oracle_back_identity}"
                    ));
                }
                if front_run == back_run
                    || event_count == 0
                    || (event_count, first_event, last_event)
                        != (
                            u64::from(expected.0),
                            u64::from(expected.1),
                            u64::from(expected.2),
                        )
                    || event_digest != u64::from(expected.3)
                {
                    return Err(format!(
                        "event arena evidence differs for `{case}` at ({x},{y})"
                    ));
                }
                // The independent event oracle has now classified every
                // subpixel, selected both side identities, and matched the
                // exact presented byte. The legacy five-point interior/
                // edge checks below decode RGB as one unblended identity,
                // which is not meaningful for a premultiplied event pixel.
                return Ok((score, event_coverage_mismatches));
            }
            3 => {
                if !guest_background {
                    score.raster_evidence_failures += 1;
                }
            }
            _ => score.raster_evidence_failures += 1,
        }
        let all_hit = samples.iter().all(|sample| sample.hit);
        let all_miss = samples.iter().all(|sample| !sample.hit);
        let identities: std::collections::BTreeSet<u32> = samples
            .iter()
            .filter_map(|sample| sample.identity)
            .collect();
        if all_miss {
            let root_free = pixel_bundle_root_free(
                worker_evaluator,
                &dump.camera,
                &dump.params,
                u_at(x as f64),
                v_at(y as f64),
                u_at(x as f64 + 1.0),
                v_at(y as f64 + 1.0),
            )?;
            if root_free {
                score.checked_interior += 1;
                if !guest_background {
                    score.phantom_surface += 1;
                    if score.first_issue.is_none() {
                        score.first_issue = Some([x as u16, y as u16, 2]);
                    }
                }
            } else {
                let (dense_identities, dense_unresolved, background_proven) =
                    resolve_unproven_pixel(
                        worker_evaluator,
                        &dump.camera,
                        &dump.params,
                        PixelUvCell {
                            u0: u_at(x as f64),
                            v0: v_at(y as f64),
                            u1: u_at(x as f64 + 1.0),
                            v1: v_at(y as f64 + 1.0),
                        },
                        guest_background,
                        guest_identity,
                    )?;
                score.unresolved = score
                    .unresolved
                    .checked_add(dense_unresolved)
                    .ok_or_else(|| "frame unresolved counter overflow".to_string())?;
                let matches = if guest_background {
                    background_proven
                } else {
                    dense_identities.contains(&guest_identity)
                };
                if !matches {
                    score.edge_center_violations += 1;
                    if score.first_issue.is_none() {
                        score.first_issue = Some([x as u16, y as u16, 8]);
                    }
                }
            }
        } else if all_hit
            && identities.len() == 1
            && samples.iter().all(|sample| sample.identity.is_some())
        {
            score.checked_interior += 1;
            let oracle_identity = *identities.iter().next().expect("nonempty");
            if guest_background || guest_identity != oracle_identity {
                score.interior_mismatches += 1;
                if score.first_issue.is_none() {
                    score.first_issue = Some([x as u16, y as u16, 3]);
                }
            }
        } else if all_hit {
            score.ambiguous_identity += 1;
            if guest_background {
                score.edge_center_violations += 1;
                if score.first_issue.is_none() {
                    score.first_issue = Some([x as u16, y as u16, 4]);
                }
            }
        } else {
            // Any mixed pixel has analytically nonzero event activity in
            // the pixel domain, regardless of whether its centre ray is
            // a hit. Requiring only centre-hit edges allowed an enclosed
            // subpixel feature to disappear completely.
            if guest_background {
                score.edge_center_violations += 1;
                if score.first_issue.is_none() {
                    score.first_issue = Some([x as u16, y as u16, 5]);
                }
            } else if !identities.is_empty() && !identities.contains(&guest_identity) {
                score.edge_center_violations += 1;
                if score.first_issue.is_none() {
                    score.first_issue = Some([x as u16, y as u16, 6]);
                }
            }
        }
        Ok((score, event_coverage_mismatches))
    };

    // The complete per-pixel semantic proof is independent across pixels.
    // Compute it on owned evaluators, then merge strictly in scan order so
    // counter overflow, the first reported error, the first issue, and the
    // diagnostic mismatch list are identical to the serial algorithm.
    let pixel_count = width * height;
    let workers = std::thread::available_parallelism()
        .map(|count| count.get().saturating_sub(2).max(2))
        .unwrap_or(4)
        .min(pixel_count);
    let evaluators = (0..workers)
        .map(|_| SemanticFieldEvaluator::new(renderer))
        .collect::<Result<Vec<_>, _>>()?;
    let slots: Vec<std::sync::Mutex<Option<Result<(FrameScore, Vec<String>), String>>>> = (0
        ..pixel_count)
        .map(|_| std::sync::Mutex::new(None))
        .collect();
    let cursor = std::sync::atomic::AtomicUsize::new(0);
    let cursor_ref = &cursor;
    let slots_ref = &slots;
    let score_pixel_ref = &score_pixel;
    std::thread::scope(|scope| {
        for worker_evaluator in evaluators {
            scope.spawn(move || {
                loop {
                    let index = cursor_ref.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    if index >= pixel_count {
                        return;
                    }
                    let result = score_pixel_ref(&worker_evaluator, index % width, index / width);
                    *slots_ref[index].lock().unwrap_or_else(|e| e.into_inner()) = Some(result);
                }
            });
        }
    });
    let mut score = FrameScore::default();
    let mut event_coverage_mismatches = Vec::new();
    for slot in slots {
        let (part, mut mismatches) = slot
            .into_inner()
            .unwrap_or_else(|e| e.into_inner())
            .ok_or_else(|| "frame score worker omitted a pixel".to_string())??;
        merge_frame_score(&mut score, part)?;
        event_coverage_mismatches.append(&mut mismatches);
    }
    if !event_coverage_mismatches.is_empty() {
        return Err(format!(
            "independent event oracle differs for `{case}` at {} pixel(s):\n  {}",
            event_coverage_mismatches.len(),
            event_coverage_mismatches.join("\n  ")
        ));
    }
    Ok(score)
}

pub fn run(
    guest_observations: &[GuestObservation],
    guest_renderers: &[crate::pixels::CompiledRenderer],
) -> Result<String, String> {
    if guest_observations.len() != guest_renderers.len() {
        return Err("guest observation/semantic renderer count differs".to_string());
    }
    // Full-frame semantic scoring is read-only and case-independent. A small
    // fixed worker pool keeps the permanent adversarial corpus practical
    // without making report order or results depend on scheduling.
    let frame_scores: Vec<Result<Option<FrameScore>, String>> = {
        let slots: Vec<std::sync::Mutex<Option<Result<Option<FrameScore>, String>>>> =
            guest_observations
                .iter()
                .map(|_| std::sync::Mutex::new(None))
                .collect();
        let cursor = std::sync::atomic::AtomicUsize::new(0);
        let workers = guest_observations.len().min(4);
        std::thread::scope(|scope| {
            for _ in 0..workers {
                scope.spawn(|| {
                    loop {
                        let index = cursor.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                        let Some(observation) = guest_observations.get(index) else {
                            return;
                        };
                        let result = observation
                            .frame_dump
                            .as_ref()
                            .map(|dump| {
                                score_frame(
                                    &guest_observations[index].case,
                                    &guest_renderers[index],
                                    dump,
                                )
                                .map_err(|error| {
                                    format!("`{}` frame scoring failed: {error}", observation.case)
                                })
                            })
                            .transpose();
                        *slots[index].lock().unwrap_or_else(|e| e.into_inner()) = Some(result);
                    }
                });
            }
        });
        slots
            .into_iter()
            .map(|slot| {
                slot.into_inner()
                    .unwrap_or_else(|e| e.into_inner())
                    .unwrap_or_else(|| Err("frame score worker omitted a case".to_string()))
            })
            .collect()
    };
    run_scored(guest_observations, guest_renderers, frame_scores)
}

/// `run`, with the per-case frame scores already computed. Frame scoring is
/// by far the most expensive part of building the report — the sampled event
/// oracle behind it is millions of field evaluations per event pixel — and it
/// depends only on each case's own dump, so a harness that obtains dumps one
/// at a time (the conformance gate, whose instrumented guest boots trickle in
/// over many minutes) can score each case as its dump lands and hide the
/// entire scoring cost inside the boot window. Each entry must be the exact
/// value `score_frame` returns for the same-index observation's dump — `None`
/// for an absent dump — wrapped in the same "`{case}` frame scoring failed"
/// error context; report content is identical either way.
pub fn run_scored(
    guest_observations: &[GuestObservation],
    guest_renderers: &[crate::pixels::CompiledRenderer],
    frame_scores: Vec<Result<Option<FrameScore>, String>>,
) -> Result<String, String> {
    if guest_observations.len() != guest_renderers.len() {
        return Err("guest observation/semantic renderer count differs".to_string());
    }
    if frame_scores.len() != guest_observations.len() {
        return Err("guest observation/frame score count differs".to_string());
    }
    let mut report = String::from("PixelsConformance version=1 mode=full-frame\n");
    let mut failures = 0_u32;
    let mut unresolved = 0_u32;
    let mut cases = 0_u32;

    for (index, (observation, renderer)) in
        guest_observations.iter().zip(guest_renderers).enumerate()
    {
        let complete_partition =
            observation.certificate_runs != 0 || observation.event_corridors != 0;
        let (frame_camera, frame_params) = observation
            .frame_dump
            .as_ref()
            .map_or((CANONICAL_CAMERA, [0.0; 16]), |dump| {
                (dump.camera, dump.params)
            });
        let evaluator = SemanticFieldEvaluator::new(renderer).map_err(|error| {
            format!(
                "semantic evaluator for `{}` failed: {error}",
                observation.case
            )
        })?;
        let alpha_oracle = semantic_alpha_samples(&evaluator, &frame_camera, &frame_params)
            .map_err(|error| {
                format!(
                    "semantic alpha oracle for `{}` failed: {error}",
                    observation.case
                )
            })?;
        unresolved = unresolved
            .checked_add(alpha_oracle.unresolved)
            .ok_or_else(|| "semantic oracle unresolved count overflow".to_string())?;
        // This oracle is a deterministic 16x16 sample lattice, not an exact
        // area enclosure. It can miss a sub-sample thin sliver or round one
        // byte above an analytic certificate, so it is a proximity control
        // for every case rather than a one-sided proof bound.
        let alpha_matches = observation.alpha_samples == [255; 3];
        let evidence_failures = if let Some(evidence) = observation.run_evidence {
            let certified =
                super::sweep::decode_certified_run_record(evidence).map_err(|error| {
                    format!("`{}` run evidence is invalid: {error:?}", observation.case)
                })?;
            let fixed_q = renderer
                .program
                .program()
                .table(wrela_machine::pixels::FrameProgramTableKindV1::FixedDomain)
                .and_then(|table| table.records.iter().find(|record| record.tag == 5))
                .ok_or_else(|| format!("`{}` fixed-q record is missing", observation.case))?;
            let exponent = fixed_q.operands[0] as i64 as i32;
            let q_scale = 2.0_f64.powi(exponent);
            let evidence_point = super::sweep::decode_certified_run_evidence_point(evidence)
                .map_err(|error| {
                    format!(
                        "`{}` run evidence point is invalid: {error:?}",
                        observation.case
                    )
                })?;
            let y = u32::from(evidence_point.row_y);
            let aspect = f64::from(renderer.config.width) / f64::from(renderer.config.height);
            let u_at = |x: f64| (x / f64::from(renderer.config.width) * 2.0 - 1.0) * aspect;
            let v_at = |sample_y: f64| 1.0 - sample_y / f64::from(renderer.config.height) * 2.0;
            let guest_q_lo = (i64::from(certified.q_model.q0.lo) + i64::from(certified.q_error.lo))
                as f64
                * q_scale;
            let guest_q_hi = (i64::from(certified.q_model.q0.hi) + i64::from(certified.q_error.hi))
                as f64
                * q_scale;
            let mut evidence_failures = 0_u8;
            if y >= renderer.config.height {
                evidence_failures |= 1;
            } else if certified.visible.is_none() {
                let sample_x =
                    f64::from(certified.x0) + f64::from(certified.x1 - certified.x0) * 0.5;
                let sample_y = f64::from(y) + 0.5;
                let oracle = semantic_ray_score_with(
                &evaluator,
                &frame_camera,
                &frame_params,
                u_at(sample_x),
                v_at(sample_y),
            )
            .map_err(|error| {
                format!(
                    "semantic background evidence for `{}` at ({sample_x:.3},{sample_y:.3}) failed: {error}",
                    observation.case
                )
            })?;
                if oracle.unresolved != 0 {
                    evidence_failures |= 2;
                    unresolved = unresolved
                        .checked_add(oracle.unresolved)
                        .ok_or_else(|| "semantic run unresolved count overflow".to_string())?;
                } else if oracle.hit {
                    evidence_failures |= 4;
                }
            } else {
                let sample_x =
                    f64::from(certified.x0) + f64::from(certified.x1 - certified.x0) * 0.5;
                let sample_y = f64::from(y) + 0.5;
                let u = u_at(sample_x);
                let v = v_at(sample_y);
                let oracle = semantic_ray_score_with(
                &evaluator,
                &frame_camera,
                &frame_params,
                u,
                v,
            )
            .map_err(|error| {
                format!(
                    "semantic run oracle for `{}` at ({sample_x:.3},{sample_y:.3}) failed: {error}",
                    observation.case
                )
            })?;
                if !evidence_point.point_witness {
                    evidence_failures |= 8;
                } else if oracle.unresolved != 0 {
                    evidence_failures |= 2;
                    unresolved = unresolved
                        .checked_add(oracle.unresolved)
                        .ok_or_else(|| "semantic run unresolved count overflow".to_string())?;
                } else if !oracle.hit {
                    evidence_failures |= 8;
                } else {
                    let depth = (oracle.t.lo + oracle.t.hi) * 0.5;
                    let point: [f64; 3] = std::array::from_fn(|axis| {
                        f64::from(frame_camera[axis])
                            + (f64::from(frame_camera[3 + axis])
                                + u * f64::from(frame_camera[6 + axis])
                                + v * f64::from(frame_camera[9 + axis]))
                                * depth
                    });
                    if evaluator.identity_at(point, &frame_params)? != Some(certified.identity.0) {
                        evidence_failures |= 16;
                    }
                    let oracle_q_lo = 1.0 / oracle.t.hi;
                    let oracle_q_hi = 1.0 / oracle.t.lo;
                    if guest_q_lo > oracle_q_lo || guest_q_hi < oracle_q_hi {
                        evidence_failures |= 32;
                    }
                    let normal_matches = evidence_point.normal.is_none_or(|guest_normal| {
                        guest_normal
                            .into_iter()
                            .map(|component| f64::from(component) / 32_767.0)
                            .zip([oracle.normal.x, oracle.normal.y, oracle.normal.z])
                            .all(|(guest, oracle)| (guest - oracle).abs() <= 0.003)
                    });
                    if !normal_matches {
                        evidence_failures |= 64;
                    }
                }
            }
            evidence_failures
        } else if observation.case == "check-pixels-tile-boundary" {
            // This seam fixture spends its bounded guest transcript on exact
            // VMM frame bytes and the complete telemetry tail. Representative
            // run semantics remain mandatory in every other density fixture.
            0
        } else {
            return Err(format!("`{}` run evidence is missing", observation.case));
        };
        let evidence_matches = evidence_failures == 0;
        // Per-pixel frame scoring against the independent semantic oracle.
        // The conformance harness always supplies the dump for instrumented
        // runs (and errors when the guest omits it); a synthetic observation
        // without one skips only this stage. Scoring it is the gate that
        // catches whole missing features and wrong identities that
        // aggregate digests cannot see.
        let frame = match observation.frame_dump.as_ref() {
            Some(frame_dump) => {
                let captured_digest = debug_frame_digest(&frame_dump.bytes);
                if captured_digest != observation.frame_digest {
                    return Err(format!(
                        "`{}` frame dump digest differs: guest {:016x?}, captured {:016x?}",
                        observation.case, observation.frame_digest, captured_digest,
                    ));
                }
                frame_scores[index].clone()?
            }
            None => None,
        };
        if let Some(frame) = frame.as_ref() {
            unresolved = unresolved
                .checked_add(frame.unresolved)
                .ok_or_else(|| "frame unresolved counter overflow".to_string())?;
        }
        let frame_matches = frame.as_ref().is_none_or(|frame| {
            frame.interior_mismatches == 0
                && frame.edge_center_violations == 0
                && frame.unresolved == 0
                && frame.skipped_unproven == 0
                && frame.phantom_surface == 0
                && frame.raster_evidence_failures == 0
        });
        let frame = frame.unwrap_or_default();
        let first_issue = frame.first_issue.map_or_else(
            || "none".to_string(),
            |issue| format!("{},{},{}", issue[0], issue[1], issue[2]),
        );
        let evidence_status = if evidence_matches {
            "pass".to_string()
        } else {
            format!("fail:{evidence_failures:02x}")
        };
        let pass = complete_partition && alpha_matches && alpha_oracle.unresolved == 0;
        let pass = pass && evidence_matches && frame_matches;
        failures += u32::from(!pass);
        writeln!(
            report,
            "case=guest-{} runs={} corridors={} proposals={} digest={:016x}{:016x}{:016x}{:016x} alpha={:02x},{:02x},{:02x} oracle={:02x},{:02x},{:02x} run_evidence={} frame_interior={}/{} frame_edge_violations={} frame_ambiguous={} frame_unresolved={} frame_skipped={} frame_phantom={} q_checked={} normal_checked={} event_bytes_checked={} boundary_limited_event_bytes={} raster_evidence_failures={} frame_first={} status={}",
            observation.case,
            observation.certificate_runs,
            observation.event_corridors,
            observation.revalidated_proposals,
            observation.frame_digest[0],
            observation.frame_digest[1],
            observation.frame_digest[2],
            observation.frame_digest[3],
            observation.alpha_samples[0],
            observation.alpha_samples[1],
            observation.alpha_samples[2],
            alpha_oracle.values[0],
            alpha_oracle.values[1],
            alpha_oracle.values[2],
            evidence_status,
            frame.checked_interior - frame.interior_mismatches,
            frame.checked_interior,
            frame.edge_center_violations,
            frame.ambiguous_identity,
            frame.unresolved,
            frame.skipped_unproven,
            frame.phantom_surface,
            frame.q_checked,
            frame.normal_checked,
            frame.event_bytes_checked,
            frame.boundary_limited_event_bytes,
            frame.raster_evidence_failures,
            first_issue,
            if pass { "pass" } else { "fail" },
        )
        .map_err(|_| "conformance report formatting failed".to_string())?;
        cases = cases
            .checked_add(1)
            .ok_or_else(|| "conformance case count overflow".to_string())?;
    }
    let four_core = guest_observations
        .iter()
        .find(|observation| observation.case == "boot-pixels-plane")
        .ok_or_else(|| "guest plane observation is missing".to_string())?;
    let one_core = guest_observations
        .iter()
        .find(|observation| observation.case == "boot-pixels-plane-one-core")
        .ok_or_else(|| "guest one-core plane observation is missing".to_string())?;
    let worker_invariant = four_core.certificate_runs == one_core.certificate_runs
        && four_core.event_corridors == one_core.event_corridors
        && four_core.revalidated_proposals == one_core.revalidated_proposals
        && four_core.alpha_samples == one_core.alpha_samples
        && four_core.frame_digest == one_core.frame_digest
        // Full frame-byte equality between the one- and four-worker builds
        // is the strongest worker-count invariance witness available.
        && four_core.frame_dump.as_ref().map(|dump| &dump.bytes)
            == one_core.frame_dump.as_ref().map(|dump| &dump.bytes);
    failures += u32::from(!worker_invariant);
    writeln!(
        report,
        "case=guest-worker-invariance status={}",
        if worker_invariant { "pass" } else { "fail" },
    )
    .map_err(|_| "conformance report formatting failed".to_string())?;
    cases += 1;

    let oracle_plane = oracle_debug_plane_digest()?;
    let plane_digest = four_core.frame_digest;
    let plane_digest_pass = plane_digest == oracle_plane;
    failures += u32::from(!plane_digest_pass);
    writeln!(
        report,
        "case=guest-plane-frame bytes=8192 digest={:016x}{:016x}{:016x}{:016x} status={}",
        plane_digest[0],
        plane_digest[1],
        plane_digest[2],
        plane_digest[3],
        if plane_digest_pass { "pass" } else { "fail" },
    )
    .map_err(|_| "conformance report formatting failed".to_string())?;
    cases += 1;

    let plane_probe = four_core
        .visibility_probe
        .ok_or_else(|| "guest plane visibility probe is missing".to_string())?;
    let plane_oracle = oracle_linear(1.0 / 4.35, 0)?;
    let q_lo = f64::from(plane_probe.q_lo) / 131_072.0;
    let q_hi = f64::from(plane_probe.q_hi) / 131_072.0;
    let normal = plane_probe
        .normal
        .map(|component| f64::from(component) / 1_000_000.0);
    let probe_pass = plane_probe.hit
        && plane_probe.identity == plane_oracle.identity
        && q_lo <= plane_oracle.t.lo
        && q_hi >= plane_oracle.t.hi
        && plane_probe.normal_valid
        && normal
            .into_iter()
            .zip([
                plane_oracle.normal.x,
                plane_oracle.normal.y,
                plane_oracle.normal.z,
            ])
            .all(|(guest, oracle)| (guest - oracle).abs() <= 1.0 / 1_000_000.0)
        && plane_probe.coverage == 255
        && plane_oracle.unresolved == 0;
    failures += u32::from(!probe_pass);
    writeln!(
        report,
        "case=guest-plane-probe hit={} identity={} q=[{q_lo:.9},{q_hi:.9}] normal=[{:.6},{:.6},{:.6}] coverage={} status={}",
        plane_probe.hit,
        plane_probe.identity,
        normal[0],
        normal[1],
        normal[2],
        plane_probe.coverage,
        if probe_pass { "pass" } else { "fail" },
    )
    .map_err(|_| "conformance report formatting failed".to_string())?;
    cases += 1;

    let plane = rendered_single_root(2047, 2049, 1, 1, 0, 1024)?;
    let plane_oracle = oracle_linear(2.0, 1)?;
    score(
        "plane",
        plane,
        plane_oracle,
        &mut report,
        &mut failures,
        &mut unresolved,
    )?;
    cases += 1;

    let sphere_outside = rendered_sphere(false)?;
    let sphere_oracle = oracle_sphere(false)?;
    score(
        "sphere-outside",
        sphere_outside,
        sphere_oracle,
        &mut report,
        &mut failures,
        &mut unresolved,
    )?;
    cases += 1;

    let sphere_inside = rendered_sphere(true)?;
    let inside_oracle = oracle_sphere(true)?;
    score(
        "camera-inside",
        sphere_inside,
        inside_oracle,
        &mut report,
        &mut failures,
        &mut unresolved,
    )?;
    cases += 1;

    let empty = rendered_source_frame(OneRootSource {
        present: false,
        ..OneRootSource::default()
    })?;
    let enclosed = rendered_source_frame(OneRootSource {
        present: true,
        feature: FeatureId(91),
        identity: 91,
        q_lo: 1535,
        q_hi: 1537,
        orientation: 1,
        initial_inside_bits: 0,
        normal_z: 1024,
    })?;
    let structural_outputs_differ = empty.tile_digest != enclosed.tile_digest;
    if !structural_outputs_differ || empty.hit || !enclosed.hit {
        failures += 1;
    }
    writeln!(
        report,
        "case=enclosed-structural-control structural_outputs_differ={} status={}",
        structural_outputs_differ,
        if structural_outputs_differ {
            "pass"
        } else {
            "fail"
        }
    )
    .map_err(|_| "conformance report formatting failed".to_string())?;
    cases += 1;

    let events = [
        super::events::EventInterval {
            lo: 7,
            hi: 9,
            generator_id: 1,
            subdivision_depth: 2,
        },
        super::events::EventInterval {
            lo: 8,
            hi: 10,
            generator_id: 2,
            subdivision_depth: 2,
        },
    ];
    let mut sorted = [super::events::EventInterval::default(); 2];
    let mut corridors = [super::events::EventCorridor::default(); 2];
    let mut regular = [super::events::RegularDomain::default(); 3];
    let partition = super::events::partition_row_events(
        0,
        16,
        &events,
        &mut sorted,
        &mut corridors,
        &mut regular,
    )
    .map_err(|error| format!("event conformance: {error:?}"))?;
    let event_pass = partition.corridors.len() == 1
        && partition.corridors[0].x0 <= 8
        && partition.corridors[0].x1 >= 9;
    let coverage_range = |lo: f64, hi: f64| OracleInterval::new(lo - 0.375, hi - 0.375);
    let mut coverage_stack = [CoverageCell::default(); 64];
    let coverage = event_coverage(
        &coverage_range,
        0.0,
        1.0,
        1.0 / 65_536.0,
        16,
        &mut coverage_stack,
    )
    .map_err(|error| format!("event coverage oracle: {error:?}"))?;
    let rendered_coverage = f64::from(96_u8) / 255.0;
    let coverage_pass = coverage.unresolved_cells == 0
        && rendered_coverage + 1.0 / 255.0 >= coverage.coverage.lo
        && rendered_coverage - 1.0 / 255.0 <= coverage.coverage.hi;
    failures += u32::from(!event_pass || !coverage_pass);
    unresolved = unresolved
        .checked_add(coverage.unresolved_cells)
        .ok_or_else(|| "event unresolved counter overflow".to_string())?;
    writeln!(
        report,
        "case=event-cover corridor=[{}, {}) simultaneous_ids={} coverage={} status={}",
        partition.corridors[0].x0,
        partition.corridors[0].x1,
        partition.corridors[0].event_count,
        rendered_coverage,
        if event_pass && coverage_pass {
            "pass"
        } else {
            "fail"
        },
    )
    .map_err(|_| "conformance report formatting failed".to_string())?;
    cases += 1;

    let digest = wrela_machine::sha256::sha256_hex(report.as_bytes());
    writeln!(
        report,
        "summary cases={cases} failures={failures} unresolved={unresolved} pre_summary_digest={digest}"
    )
    .map_err(|_| "conformance report formatting failed".to_string())?;
    if failures != 0 || unresolved != 0 {
        return Err(report);
    }
    Ok(report)
}

#[derive(Clone, Copy, Debug)]
struct SemanticAlphaSamples {
    values: [u8; 3],
    unresolved: u32,
}

#[derive(Clone, Copy, Debug)]
struct SemanticInterval {
    lo: f64,
    hi: f64,
}

impl SemanticInterval {
    fn new(lo: f64, hi: f64) -> Result<Self, String> {
        if !lo.is_finite() || !hi.is_finite() || lo > hi {
            return Err("semantic oracle produced an invalid interval".to_string());
        }
        Ok(Self { lo, hi })
    }

    fn point(value: f64) -> Result<Self, String> {
        Self::new(value, value)
    }

    fn hull(self, other: Self) -> Self {
        Self {
            lo: self.lo.min(other.lo),
            hi: self.hi.max(other.hi),
        }
    }

    fn add(self, other: Self) -> Result<Self, String> {
        Self::new(self.lo + other.lo, self.hi + other.hi)
    }

    fn sub(self, other: Self) -> Result<Self, String> {
        Self::new(self.lo - other.hi, self.hi - other.lo)
    }

    fn neg(self) -> Result<Self, String> {
        Self::new(-self.hi, -self.lo)
    }

    fn mul(self, other: Self) -> Result<Self, String> {
        let products = [
            self.lo * other.lo,
            self.lo * other.hi,
            self.hi * other.lo,
            self.hi * other.hi,
        ];
        Self::new(
            products.into_iter().fold(f64::INFINITY, f64::min),
            products.into_iter().fold(f64::NEG_INFINITY, f64::max),
        )
    }

    fn div(self, other: Self) -> Result<Self, String> {
        if other.lo <= 0.0 && other.hi >= 0.0 {
            return Err("semantic oracle interval division contains zero".to_string());
        }
        self.mul(Self::new(1.0 / other.hi, 1.0 / other.lo)?)
    }

    fn abs(self) -> Result<Self, String> {
        if self.lo >= 0.0 {
            Ok(self)
        } else if self.hi <= 0.0 {
            self.neg()
        } else {
            Self::new(0.0, (-self.lo).max(self.hi))
        }
    }

    fn square(self) -> Result<Self, String> {
        if self.lo >= 0.0 {
            Self::new(self.lo * self.lo, self.hi * self.hi)
        } else if self.hi <= 0.0 {
            Self::new(self.hi * self.hi, self.lo * self.lo)
        } else {
            Self::new(0.0, (self.lo * self.lo).max(self.hi * self.hi))
        }
    }

    fn sqrt(self) -> Result<Self, String> {
        if self.lo < 0.0 {
            return Err("semantic oracle square root interval is negative".to_string());
        }
        Self::new(self.lo.sqrt(), self.hi.sqrt())
    }
}

#[derive(Clone, Copy, Debug)]
struct SemanticDual {
    value: SemanticInterval,
    derivative: SemanticInterval,
}

impl SemanticDual {
    fn point(value: f64) -> Result<Self, String> {
        Ok(Self {
            value: SemanticInterval::point(value)?,
            derivative: SemanticInterval::point(0.0)?,
        })
    }

    fn add(self, other: Self) -> Result<Self, String> {
        Ok(Self {
            value: self.value.add(other.value)?,
            derivative: self.derivative.add(other.derivative)?,
        })
    }

    fn sub(self, other: Self) -> Result<Self, String> {
        Ok(Self {
            value: self.value.sub(other.value)?,
            derivative: self.derivative.sub(other.derivative)?,
        })
    }

    fn neg(self) -> Result<Self, String> {
        Ok(Self {
            value: self.value.neg()?,
            derivative: self.derivative.neg()?,
        })
    }

    fn mul(self, other: Self) -> Result<Self, String> {
        Ok(Self {
            value: self.value.mul(other.value)?,
            derivative: self
                .derivative
                .mul(other.value)?
                .add(self.value.mul(other.derivative)?)?,
        })
    }

    fn square(self) -> Result<Self, String> {
        Ok(Self {
            value: self.value.square()?,
            derivative: self
                .value
                .mul(self.derivative)?
                .mul(SemanticInterval::point(2.0)?)?,
        })
    }

    fn div(self, other: Self) -> Result<Self, String> {
        let denominator = other.value.square()?;
        Ok(Self {
            value: self.value.div(other.value)?,
            derivative: self
                .derivative
                .mul(other.value)?
                .sub(self.value.mul(other.derivative)?)?
                .div(denominator)?,
        })
    }

    fn abs(self) -> Result<Self, String> {
        if self.value.lo >= 0.0 {
            Ok(self)
        } else if self.value.hi <= 0.0 {
            self.neg()
        } else {
            Ok(Self {
                value: self.value.abs()?,
                derivative: self.derivative.hull(self.derivative.neg()?),
            })
        }
    }

    fn sqrt(self) -> Result<Self, String> {
        let value = self.value.sqrt()?;
        if value.lo == 0.0 {
            return Ok(Self {
                value,
                derivative: SemanticInterval::new(-1.0e300, 1.0e300)?,
            });
        }
        Ok(Self {
            value,
            derivative: self
                .derivative
                .div(value.mul(SemanticInterval::point(2.0)?)?)?,
        })
    }

    fn min(self, other: Self) -> Self {
        if self.value.hi < other.value.lo {
            self
        } else if other.value.hi < self.value.lo {
            other
        } else {
            Self {
                value: SemanticInterval {
                    lo: self.value.lo.min(other.value.lo),
                    hi: self.value.hi.min(other.value.hi),
                },
                derivative: self.derivative.hull(other.derivative),
            }
        }
    }

    fn max(self, other: Self) -> Self {
        if self.value.lo > other.value.hi {
            self
        } else if other.value.lo > self.value.hi {
            other
        } else {
            Self {
                value: SemanticInterval {
                    lo: self.value.lo.max(other.value.lo),
                    hi: self.value.hi.max(other.value.hi),
                },
                derivative: self.derivative.hull(other.derivative),
            }
        }
    }
}

fn trig_interval(interval: SemanticInterval, cosine: bool) -> Result<SemanticInterval, String> {
    let width = interval.hi - interval.lo;
    if width >= std::f64::consts::TAU {
        return SemanticInterval::new(-1.0, 1.0);
    }
    let evaluate = |value: f64| if cosine { value.cos() } else { value.sin() };
    let mut lo = evaluate(interval.lo).min(evaluate(interval.hi));
    let mut hi = evaluate(interval.lo).max(evaluate(interval.hi));
    let offset = if cosine {
        0.0
    } else {
        std::f64::consts::FRAC_PI_2
    };
    let first = ((interval.lo - offset) / std::f64::consts::PI).ceil() as i64;
    let last = ((interval.hi - offset) / std::f64::consts::PI).floor() as i64;
    for ordinal in first..=last {
        let value = evaluate(offset + ordinal as f64 * std::f64::consts::PI);
        lo = lo.min(value);
        hi = hi.max(value);
    }
    SemanticInterval::new(lo, hi)
}

/// Reusable pruned evaluator for the semantic scalar arena of one renderer.
///
/// Evaluation is restricted to nodes transitively reachable from the field
/// root and the structural feature identity leaves. An unreachable node (the
/// material-only chains) could influence scoring only by erroring during its
/// evaluation, and the shipped corpus evaluates every node without error, so
/// pruning leaves the conformance output identical while skipping the bulk of
/// the arena on every probe. The scratch buffer is reused across probes so a
/// probe performs no allocation.
struct SemanticFieldEvaluator<'r> {
    renderer: &'r crate::pixels::CompiledRenderer,
    /// `reachable[i]` marks scalar node `i` as feeding the field root or a
    /// feature identity leaf; only those nodes are evaluated per probe.
    reachable: Vec<bool>,
    root_index: usize,
    /// Node values indexed by scalar id. Slots of unreachable nodes are
    /// never written or read.
    scratch: std::cell::RefCell<Vec<SemanticDual>>,
}

fn scalar_operands(
    op: &crate::pixels::scalar::ScalarOp,
    mut mark: impl FnMut(crate::pixels::ids::ScalarId),
) {
    use crate::pixels::scalar::ScalarOp;
    match op {
        ScalarOp::ConstF32(_)
        | ScalarOp::ConstF64(_)
        | ScalarOp::CoordX
        | ScalarOp::CoordY
        | ScalarOp::CoordZ
        | ScalarOp::SurfacePosition(_)
        | ScalarOp::SurfaceNormal(_)
        | ScalarOp::Param(_) => {}
        ScalarOp::Add(a, b)
        | ScalarOp::Sub(a, b)
        | ScalarOp::Mul(a, b)
        | ScalarOp::Div(a, b)
        | ScalarOp::Min(a, b)
        | ScalarOp::Max(a, b)
        | ScalarOp::Compare { a, b, .. } => {
            mark(*a);
            mark(*b);
        }
        ScalarOp::Neg(value)
        | ScalarOp::Abs(value)
        | ScalarOp::Sqrt(value, _)
        | ScalarOp::Rsqrt(value, _)
        | ScalarOp::SinRestricted(value, _)
        | ScalarOp::CosRestricted(value, _)
        | ScalarOp::MaterialRoughness { value, .. } => mark(*value),
        ScalarOp::Clamp { value, lo, hi } => {
            mark(*value);
            mark(*lo);
            mark(*hi);
        }
        ScalarOp::Dot3(a, b) | ScalarOp::Cross3Component { a, b, .. } => {
            for id in a.iter().chain(b.iter()) {
                mark(*id);
            }
        }
        ScalarOp::Length2(vector) => {
            for id in vector {
                mark(*id);
            }
        }
        ScalarOp::Length3(vector) | ScalarOp::Normalize3Component { value: vector, .. } => {
            for id in vector {
                mark(*id);
            }
        }
        ScalarOp::Select { predicate, a, b } => {
            mark(*predicate);
            mark(*a);
            mark(*b);
        }
        ScalarOp::SelectIndex { index, options } => {
            mark(*index);
            for id in options {
                mark(*id);
            }
        }
        ScalarOp::SmoothMin { a, b, k, .. } => {
            mark(*a);
            mark(*b);
            mark(*k);
        }
        ScalarOp::FiniteOr {
            value, fallback, ..
        } => {
            mark(*value);
            mark(*fallback);
        }
    }
}

impl<'r> SemanticFieldEvaluator<'r> {
    fn new(renderer: &'r crate::pixels::CompiledRenderer) -> Result<Self, String> {
        let graph = &renderer.symbolic;
        let root = graph.fields.get(graph.field_root)?.scalar_value;
        let len = graph.scalar.len();
        let mut reachable = vec![false; len];
        *reachable
            .get_mut(root.index())
            .ok_or_else(|| "semantic field root is absent from the scalar graph".to_string())? =
            true;
        for feature in &renderer.structural.program().features {
            let leaf = graph.fields.get(feature.scalar_semantic_root)?.scalar_value;
            *reachable
                .get_mut(leaf.index())
                .ok_or_else(|| "feature leaf scalar is absent from the arena".to_string())? = true;
        }
        // Operands precede their consumers in the dense arena order, so one
        // reverse sweep closes the reachable set transitively.
        let ops: Vec<&crate::pixels::scalar::ScalarOp> =
            graph.scalar.iter().map(|(_, node)| &node.op).collect();
        for index in (0..len).rev() {
            if reachable[index] {
                scalar_operands(ops[index], |operand| {
                    if let Some(slot) = reachable.get_mut(operand.index()) {
                        *slot = true;
                    }
                });
            }
        }
        let zero = SemanticDual::point(0.0)?;
        Ok(Self {
            renderer,
            reachable,
            root_index: root.index(),
            scratch: std::cell::RefCell::new(vec![zero; len]),
        })
    }

    fn field_dual(
        &self,
        coordinates: [SemanticDual; 3],
        params: &[f32; 16],
    ) -> Result<SemanticDual, String> {
        let mut scratch = self.scratch.borrow_mut();
        semantic_field_evaluate(self, coordinates, params, &mut scratch)?;
        scratch
            .get(self.root_index)
            .copied()
            .ok_or_else(|| "semantic field root is absent from the scalar graph".to_string())
    }

    fn point(&self, point: [f64; 3], params: &[f32; 16]) -> Result<f64, String> {
        let zero = SemanticInterval::point(0.0)?;
        Ok(self
            .field_dual(
                [
                    SemanticDual {
                        value: SemanticInterval::point(point[0])?,
                        derivative: zero,
                    },
                    SemanticDual {
                        value: SemanticInterval::point(point[1])?,
                        derivative: zero,
                    },
                    SemanticDual {
                        value: SemanticInterval::point(point[2])?,
                        derivative: zero,
                    },
                ],
                params,
            )?
            .value
            .lo)
    }

    /// Evaluate a terminal root's value and spatial gradient with forward
    /// derivatives. One graph walk per coordinate replaces the historical
    /// six finite-difference walks plus a seventh value walk on smooth roots.
    /// At a nonsmooth CSG selection the interval derivative can legitimately
    /// contain zero in every axis; retain the historical symmetric spatial
    /// probe there rather than turning an otherwise resolved boundary into an
    /// oracle failure.
    fn terminal_at(&self, point: [f64; 3], params: &[f32; 16]) -> Result<OracleTerminal, String> {
        let zero = SemanticInterval::point(0.0)?;
        let one = SemanticInterval::point(1.0)?;
        let mut value = None;
        let mut gradient = [0.0; 3];
        let mut gradient_proved_nonzero = false;
        for derivative_axis in 0..3 {
            let mut coordinates = [SemanticDual {
                value: zero,
                derivative: zero,
            }; 3];
            for axis in 0..3 {
                coordinates[axis] = SemanticDual {
                    value: SemanticInterval::point(point[axis])?,
                    derivative: if axis == derivative_axis { one } else { zero },
                };
            }
            let dual = self.field_dual(coordinates, params)?;
            value.get_or_insert((dual.value.lo + dual.value.hi) * 0.5);
            gradient[derivative_axis] = (dual.derivative.lo + dual.derivative.hi) * 0.5;
            gradient_proved_nonzero |= dual.derivative.lo > 0.0 || dual.derivative.hi < 0.0;
        }
        let value = value.ok_or_else(|| "semantic terminal value is absent".to_string())?;
        if !value.is_finite() || gradient.iter().any(|component| !component.is_finite()) {
            return Err("semantic terminal evaluated non-finite".to_string());
        }
        if !gradient_proved_nonzero {
            let epsilon = 1.0e-6;
            for axis in 0..3 {
                let mut below = point;
                let mut above = point;
                below[axis] -= epsilon;
                above[axis] += epsilon;
                gradient[axis] =
                    (self.point(above, params)? - self.point(below, params)?) / (2.0 * epsilon);
            }
        }
        Ok(OracleTerminal {
            value,
            gradient: OracleVec3 {
                x: gradient[0],
                y: gradient[1],
                z: gradient[2],
            },
            // The visibility oracle historically defers semantic identity to
            // the dedicated point classifier after selecting the first root.
            identity: 0,
        })
    }

    /// Independent identity oracle: the surface at `point` belongs to the
    /// structural feature whose semantic leaf magnitude is smallest there.
    /// When the two smallest leaves carry different identity sets and are
    /// within a factor of four of each other the pixel is identity-ambiguous
    /// (a blend or shared boundary) and the oracle abstains rather than
    /// guessing.
    fn identity_at(&self, point: [f64; 3], params: &[f32; 16]) -> Result<Option<u32>, String> {
        let ranked = self.ranked_identities_at(point, params)?;
        match ranked.as_slice() {
            [] => Ok(None),
            [only] => Ok(Some(only.1)),
            [first, rest @ ..] => {
                let ambiguous = rest
                    .iter()
                    .any(|entry| entry.1 != first.1 && entry.0 <= first.0 * 4.0 + 1.0e-9);
                Ok(if ambiguous { None } else { Some(first.1) })
            }
        }
    }

    /// Deterministic first-surface identity for coverage integration. Unlike
    /// the ordinary point scorer, event integration cannot abstain near a
    /// tie: it must partition the entire pixel between concrete side winners.
    /// The ordering is independently reconstructed from semantic leaf values
    /// and the stable identity ID, never from guest raster evidence.
    fn winner_identity_at(&self, point: [f64; 3], params: &[f32; 16]) -> Result<u32, String> {
        self.ranked_identities_at(point, params)?
            .first()
            .map(|entry| entry.1)
            .ok_or_else(|| "event coverage oracle has no structural identity".to_string())
    }

    fn ranked_identities_at(
        &self,
        point: [f64; 3],
        params: &[f32; 16],
    ) -> Result<Vec<(f64, u32)>, String> {
        let zero = SemanticInterval::point(0.0)?;
        let coordinates = [
            SemanticDual {
                value: SemanticInterval::point(point[0])?,
                derivative: zero,
            },
            SemanticDual {
                value: SemanticInterval::point(point[1])?,
                derivative: zero,
            },
            SemanticDual {
                value: SemanticInterval::point(point[2])?,
                derivative: zero,
            },
        ];
        let mut scratch = self.scratch.borrow_mut();
        semantic_field_evaluate(self, coordinates, params, &mut scratch)?;
        let graph = &self.renderer.symbolic;
        let features = &self.renderer.structural.program().features;
        let mut ranked = Vec::with_capacity(features.len());
        for feature in features {
            let node = graph.fields.get(feature.scalar_semantic_root)?;
            let dual = scratch
                .get(node.scalar_value.index())
                .copied()
                .ok_or_else(|| "feature leaf scalar is absent from the arena".to_string())?;
            let value = (dual.value.lo + dual.value.hi) * 0.5;
            if !value.is_finite() {
                return Err("feature leaf evaluated non-finite at a hit point".to_string());
            }
            ranked.push((value.abs(), feature.identity_set));
        }
        ranked.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.cmp(&b.1)));
        Ok(ranked)
    }
}

/// Evaluate the reachable subset of the semantic scalar arena into `scratch`,
/// in arena order, with per-node arithmetic identical to the historical
/// whole-arena evaluation. Nodes outside the field root's reachable set are
/// skipped entirely.
fn semantic_field_evaluate(
    evaluator: &SemanticFieldEvaluator<'_>,
    coordinates: [SemanticDual; 3],
    params: &[f32; 16],
    scratch: &mut [SemanticDual],
) -> Result<(), String> {
    use crate::pixels::scalar::{CompareOp, ScalarOp};

    let graph = &evaluator.renderer.symbolic;
    debug_assert_eq!(
        scratch.len(),
        graph.scalar.len(),
        "scratch tracks the arena"
    );
    let get = |id: crate::pixels::ids::ScalarId, values: &[SemanticDual]| {
        values
            .get(id.index())
            .copied()
            .ok_or_else(|| "semantic scalar references a non-predecessor".to_string())
    };
    for (id, node) in graph.scalar.iter() {
        let index = id.index();
        if !evaluator.reachable[index] {
            continue;
        }
        // The already-evaluated predecessors are exactly the prefix, so reads
        // through `get` see the same window as the historical growing-Vec
        // evaluation (a forward reference still errors).
        let (values, rest) = scratch.split_at_mut(index);
        let values: &[SemanticDual] = values;
        let value = match &node.op {
            ScalarOp::ConstF32(bits) => SemanticDual::point(f64::from(f32::from_bits(*bits)))?,
            ScalarOp::ConstF64(bits) => SemanticDual::point(f64::from_bits(*bits))?,
            ScalarOp::CoordX => coordinates[0],
            ScalarOp::CoordY => coordinates[1],
            ScalarOp::CoordZ => coordinates[2],
            ScalarOp::Param(id) => SemanticDual::point(f64::from(
                params
                    .get(id.index())
                    .copied()
                    .ok_or_else(|| "semantic parameter slot out of range".to_string())?,
            ))?,
            ScalarOp::SurfacePosition(_) | ScalarOp::SurfaceNormal(_) => {
                // Material-only scalars share the arena but can never feed
                // the field root or a feature leaf; a placeholder keeps the
                // whole-arena evaluation total without consulting them.
                SemanticDual::point(0.0)?
            }
            ScalarOp::Add(a, b) => get(*a, values)?.add(get(*b, values)?)?,
            ScalarOp::Sub(a, b) => get(*a, values)?.sub(get(*b, values)?)?,
            ScalarOp::Mul(a, b) => get(*a, values)?.mul(get(*b, values)?)?,
            ScalarOp::Div(a, b) => get(*a, values)?.div(get(*b, values)?)?,
            ScalarOp::Neg(value) => get(*value, values)?.neg()?,
            ScalarOp::Abs(value) => get(*value, values)?.abs()?,
            ScalarOp::Min(a, b) => get(*a, values)?.min(get(*b, values)?),
            ScalarOp::Max(a, b) => get(*a, values)?.max(get(*b, values)?),
            ScalarOp::Clamp { value, lo, hi } => get(*value, values)?
                .max(get(*lo, values)?)
                .min(get(*hi, values)?),
            ScalarOp::Sqrt(value, _) => get(*value, values)?.sqrt()?,
            ScalarOp::Rsqrt(value, _) => {
                SemanticDual::point(1.0)?.div(get(*value, values)?.sqrt()?)?
            }
            ScalarOp::SinRestricted(value, _) | ScalarOp::CosRestricted(value, _) => {
                let argument = get(*value, values)?;
                let cosine = matches!(&node.op, ScalarOp::CosRestricted(_, _));
                let trig = trig_interval(argument.value, cosine)?;
                let derivative_factor = trig_interval(argument.value, !cosine)?;
                let derivative_factor = if cosine {
                    derivative_factor.neg()?
                } else {
                    derivative_factor
                };
                SemanticDual {
                    value: trig,
                    derivative: derivative_factor.mul(argument.derivative)?,
                }
            }
            ScalarOp::Dot3(a, b) => {
                let mut result = SemanticDual::point(0.0)?;
                for axis in 0..3 {
                    result = result.add(get(a[axis], values)?.mul(get(b[axis], values)?)?)?;
                }
                result
            }
            ScalarOp::Cross3Component { component, a, b } => {
                let (left, right) = match component {
                    0 => ((1, 2), (2, 1)),
                    1 => ((2, 0), (0, 2)),
                    2 => ((0, 1), (1, 0)),
                    _ => return Err("semantic cross component is invalid".to_string()),
                };
                get(a[left.0], values)?
                    .mul(get(b[left.1], values)?)?
                    .sub(get(a[right.0], values)?.mul(get(b[right.1], values)?)?)?
            }
            ScalarOp::Length2(vector) => {
                let mut square = SemanticDual::point(0.0)?;
                for component in vector {
                    let value = get(*component, values)?;
                    square = square.add(value.square()?)?;
                }
                square.sqrt()?
            }
            ScalarOp::Length3(vector) => {
                let mut square = SemanticDual::point(0.0)?;
                for component in vector {
                    let value = get(*component, values)?;
                    square = square.add(value.square()?)?;
                }
                square.sqrt()?
            }
            ScalarOp::Normalize3Component {
                component, value, ..
            } => {
                let mut square = SemanticDual::point(0.0)?;
                for coordinate in value {
                    let value = get(*coordinate, values)?;
                    square = square.add(value.square()?)?;
                }
                get(
                    *value
                        .get(usize::from(*component))
                        .ok_or_else(|| "semantic normal component is invalid".to_string())?,
                    values,
                )?
                .div(square.sqrt()?)?
            }
            ScalarOp::Compare { op, a, b } => {
                let a = get(*a, values)?.value;
                let b = get(*b, values)?.value;
                let (always, never) = match op {
                    CompareOp::Lt => (a.hi < b.lo, a.lo >= b.hi),
                    CompareOp::Le => (a.hi <= b.lo, a.lo > b.hi),
                    CompareOp::Gt => (a.lo > b.hi, a.hi <= b.lo),
                    CompareOp::Ge => (a.lo >= b.hi, a.hi < b.lo),
                    CompareOp::Eq => (
                        a.lo == a.hi && a.lo == b.lo && b.lo == b.hi,
                        a.hi < b.lo || b.hi < a.lo,
                    ),
                    CompareOp::Ne => (
                        a.hi < b.lo || b.hi < a.lo,
                        a.lo == a.hi && a.lo == b.lo && b.lo == b.hi,
                    ),
                };
                if always {
                    SemanticDual::point(1.0)?
                } else if never {
                    SemanticDual::point(0.0)?
                } else {
                    SemanticDual {
                        value: SemanticInterval::new(0.0, 1.0)?,
                        derivative: SemanticInterval::point(0.0)?,
                    }
                }
            }
            ScalarOp::Select { predicate, a, b } => {
                let predicate = get(*predicate, values)?.value;
                if predicate.lo >= 1.0 {
                    get(*a, values)?
                } else if predicate.hi <= 0.0 {
                    get(*b, values)?
                } else {
                    let a = get(*a, values)?;
                    let b = get(*b, values)?;
                    SemanticDual {
                        value: a.value.hull(b.value),
                        derivative: a.derivative.hull(b.derivative),
                    }
                }
            }
            ScalarOp::SelectIndex { index, options } => {
                let index = get(*index, values)?.value;
                let first = index.lo.floor().max(0.0) as usize;
                let last = index.hi.ceil().max(0.0) as usize;
                let mut selected = None::<SemanticDual>;
                for option in options
                    .iter()
                    .skip(first)
                    .take(last.saturating_sub(first).saturating_add(1))
                {
                    let value = get(*option, values)?;
                    selected = Some(selected.map_or(value, |old| SemanticDual {
                        value: old.value.hull(value.value),
                        derivative: old.derivative.hull(value.derivative),
                    }));
                }
                selected.ok_or_else(|| "semantic select index is out of bounds".to_string())?
            }
            ScalarOp::SmoothMin { a, b, k, .. } => {
                let a = get(*a, values)?;
                let b = get(*b, values)?;
                let k = get(*k, values)?;
                let half = SemanticDual::point(0.5)?;
                let one = SemanticDual::point(1.0)?;
                let zero = SemanticDual::point(0.0)?;
                let h = half.add(half.mul(b.sub(a)?.div(k)?)?)?.max(zero).min(one);
                b.add(a.sub(b)?.mul(h)?)?.sub(k.mul(h)?.mul(one.sub(h)?)?)?
            }
            ScalarOp::FiniteOr {
                value, fallback, ..
            } => {
                let value = get(*value, values)?;
                if value.value.lo.is_finite() && value.value.hi.is_finite() {
                    value
                } else {
                    get(*fallback, values)?
                }
            }
            ScalarOp::MaterialRoughness { value, .. } => get(*value, values)?,
        };
        rest[0] = value;
    }
    Ok(())
}

fn semantic_ray_score_with(
    evaluator: &SemanticFieldEvaluator<'_>,
    camera: &[f32; 12],
    params: &[f32; 16],
    u: f64,
    v: f64,
) -> Result<super::oracle::VisibilityScore, String> {
    let renderer = evaluator.renderer;
    let eye = [
        f64::from(camera[0]),
        f64::from(camera[1]),
        f64::from(camera[2]),
    ];
    let direction: [f64; 3] = std::array::from_fn(|axis| {
        f64::from(camera[3 + axis])
            + u * f64::from(camera[6 + axis])
            + v * f64::from(camera[9 + axis])
    });
    let ray_dual =
        |component: f64, offset: f64, lo: f64, hi: f64| -> Result<SemanticDual, String> {
            let at_lo = offset + component * lo;
            let at_hi = offset + component * hi;
            Ok(SemanticDual {
                value: SemanticInterval::new(at_lo.min(at_hi), at_lo.max(at_hi))?,
                derivative: SemanticInterval::point(component)?,
            })
        };
    let dual_range_error = std::cell::RefCell::new(None::<String>);
    let dual_range = |lo: f64, hi: f64| {
        let result = evaluator.field_dual(
            [
                ray_dual(direction[0], eye[0], lo, hi).ok()?,
                ray_dual(direction[1], eye[1], lo, hi).ok()?,
                ray_dual(direction[2], eye[2], lo, hi).ok()?,
            ],
            params,
        );
        match result {
            Ok(dual) => Some((
                OracleInterval::new(dual.value.lo, dual.value.hi)?,
                OracleInterval::new(dual.derivative.lo, dual.derivative.hi)?,
            )),
            Err(error) => {
                let mut first = dual_range_error.borrow_mut();
                if first.is_none() {
                    *first = Some(format!("on [{lo}, {hi}]: {error}"));
                }
                None
            }
        }
    };
    let value_error = std::cell::RefCell::new(None::<String>);
    let ray_point = move |depth: f64| -> [f64; 3] {
        std::array::from_fn(|axis| eye[axis] + direction[axis] * depth)
    };
    let value = |depth: f64| {
        evaluator
            .point(ray_point(depth), params)
            .unwrap_or_else(|error| {
                let mut first = value_error.borrow_mut();
                if first.is_none() {
                    *first = Some(format!("at depth {depth}: {error}"));
                }
                f64::NAN
            })
    };
    let terminal_error = std::cell::RefCell::new(None::<String>);
    let terminal = |depth: f64| {
        evaluator
            .terminal_at(ray_point(depth), params)
            .map_err(|error| {
                let mut first = terminal_error.borrow_mut();
                if first.is_none() {
                    *first = Some(format!("at depth {depth}: {error}"));
                }
            })
            .ok()
    };
    // A closed interval that straddles an exact finite-repeat tie loses the
    // dependency between the selected coordinate and its branch. Split at
    // parameter-independent repeat event bands so the independent interval
    // oracle evaluates each smooth side separately. A sign transition at the
    // event itself is retained as a boundary; equal nonzero side signs prove
    // that the tie is not a surface.
    let mut repeat_bands = Vec::<(f64, f64)>::new();
    for generator in &renderer.projective.program().events.generators {
        let crate::pixels::events::EventRepresentation::RepeatAffineBoundary { axis, boundary } =
            generator.representation
        else {
            continue;
        };
        if !generator.coefficient_dependencies.is_empty() {
            continue;
        }
        let axis = match axis {
            crate::pixels::graph::Axis::X => 0,
            crate::pixels::graph::Axis::Y => 1,
            crate::pixels::graph::Axis::Z => 2,
        };
        if direction[axis] == 0.0 {
            continue;
        }
        let a = (boundary.lo - eye[axis]) / direction[axis];
        let b = (boundary.hi - eye[axis]) / direction[axis];
        let band = (a.min(b), a.max(b));
        if band.1 > renderer.config.near && band.0 < renderer.config.far {
            repeat_bands.push((
                band.0.max(renderer.config.near),
                band.1.min(renderer.config.far),
            ));
        }
    }
    repeat_bands.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.total_cmp(&b.1)));
    let mut merged_bands = Vec::<(f64, f64)>::new();
    for band in repeat_bands {
        if let Some(last) = merged_bands.last_mut()
            && band.0 <= last.1
        {
            last.1 = last.1.max(band.1);
        } else {
            merged_bands.push(band);
        }
    }
    let mut segments = Vec::<(f64, f64)>::new();
    let mut roots = Vec::<OracleRoot>::new();
    let mut cursor = renderer.config.near;
    for (lo, hi) in merged_bands {
        let segment_hi = super::interval::next_down(lo);
        if cursor < segment_hi {
            segments.push((cursor, segment_hi));
        }
        let left = value(segment_hi);
        let right_t = super::interval::next_up(hi);
        let right = value(right_t);
        let midpoint = lo + (hi - lo) * 0.5;
        let at_event = value(midpoint);
        let orientation = if left > 0.0 && right < 0.0 {
            1
        } else if left < 0.0 && right > 0.0 {
            -1
        } else {
            0
        };
        if orientation != 0 || at_event == 0.0 {
            if roots.len() == 32 {
                return Err("semantic guest oracle: CapacityExceeded".to_string());
            }
            let terminal = terminal(midpoint)
                .ok_or_else(|| "semantic guest oracle: Unresolved event normal".to_string())?;
            let normal = terminal.gradient;
            let normal_length =
                (normal.x * normal.x + normal.y * normal.y + normal.z * normal.z).sqrt();
            if !normal_length.is_finite() || normal_length <= 0.0 {
                return Err("semantic guest oracle: Unresolved event normal".to_string());
            }
            roots.push(OracleRoot {
                t: OracleInterval { lo, hi },
                identity: 0,
                orientation,
                normal: OracleVec3 {
                    x: normal.x / normal_length,
                    y: normal.y / normal_length,
                    z: normal.z / normal_length,
                },
            });
        }
        cursor = right_t;
    }
    if cursor < renderer.config.far {
        segments.push((cursor, renderer.config.far));
    }
    let map_oracle_error = |error| {
        let detail = value_error
            .borrow()
            .clone()
            .or_else(|| dual_range_error.borrow().clone())
            .or_else(|| terminal_error.borrow().clone());
        detail.map_or_else(
            || format!("semantic guest oracle: {error:?}"),
            |detail| format!("semantic guest oracle: {error:?} ({detail})"),
        )
    };
    for (lo, hi) in segments {
        let mut stack = [OracleCell::default(); 96];
        let mut segment_roots = [OracleRoot::default(); 32];
        let count = isolate_all_roots(
            SemanticRay {
                dual_range: &dual_range,
                value: &value,
                terminal: &terminal,
            },
            lo,
            hi,
            1.0 / 262_144.0,
            26,
            &mut stack,
            &mut segment_roots,
        )
        .map_err(&map_oracle_error)?;
        if roots.len() + count > 32 {
            return Err("semantic guest oracle: CapacityExceeded".to_string());
        }
        roots.extend_from_slice(&segment_roots[..count]);
    }
    roots.sort_by(|a, b| a.t.lo.total_cmp(&b.t.lo).then(a.t.hi.total_cmp(&b.t.hi)));
    let camera_inside = value(renderer.config.near) <= 0.0;
    Ok(first_boundary(&roots, camera_inside))
}

fn semantic_ray_visibility(
    evaluator: &SemanticFieldEvaluator<'_>,
    camera: &[f32; 12],
    params: &[f32; 16],
    u: f64,
    v: f64,
) -> Result<bool, String> {
    Ok(semantic_ray_score_with(evaluator, camera, params, u, v)?.hit)
}

fn semantic_alpha_samples(
    evaluator: &SemanticFieldEvaluator<'_>,
    camera: &[f32; 12],
    params: &[f32; 16],
) -> Result<SemanticAlphaSamples, String> {
    let renderer = evaluator.renderer;
    let positions = [31_u32, 32, 40];
    let mut values = [0_u8; 3];
    let subdivisions = 16_u32;
    let total = subdivisions * subdivisions;
    for (slot, x) in positions.into_iter().enumerate() {
        let mut covered = 0_u32;
        for sy in 0..subdivisions {
            for sx in 0..subdivisions {
                let sample_x = f64::from(x) + (f64::from(sx) + 0.5) / f64::from(subdivisions);
                let sample_y = 16.0 + (f64::from(sy) + 0.5) / f64::from(subdivisions);
                let aspect = f64::from(renderer.config.width) / f64::from(renderer.config.height);
                let u = (sample_x / f64::from(renderer.config.width) * 2.0 - 1.0) * aspect;
                let v = 1.0 - sample_y / f64::from(renderer.config.height) * 2.0;
                let visible =
                    semantic_ray_visibility(evaluator, camera, params, u, v).map_err(|error| {
                        format!("{error} at alpha sample x={x} sx={sx} sy={sy} u={u:.17} v={v:.17}")
                    })?;
                covered += u32::from(visible);
            }
        }
        values[slot] = ((u64::from(covered) * 255 + u64::from(total / 2)) / u64::from(total)) as u8;
    }
    Ok(SemanticAlphaSamples {
        values,
        unresolved: 0,
    })
}

fn oracle_debug_plane_digest() -> Result<[u64; 4], String> {
    let plane = oracle_linear(1.0 / 4.35, 0)?;
    if !plane.hit || plane.unresolved != 0 || plane.identity != 0 {
        return Err("independent plane oracle did not resolve the visible boundary".to_string());
    }
    let mut bytes = [0_u8; 64 * 32 * 4];
    for y in 0..32 {
        for x in 0..64 {
            let at = (y * 64 + x) * 4;
            bytes[at] = 255;
            bytes[at + 1] = 0;
            bytes[at + 2] = 0;
            bytes[at + 3] = 255;
        }
    }
    Ok(debug_digest_words(&bytes))
}

fn debug_digest_words(bytes: &[u8]) -> [u64; 4] {
    let mut words = [
        1_469_598_103_934_665_603_u64,
        1_099_511_628_211_u64,
        7_809_847_782_465_536_322_u64,
        1_609_587_929_392_839_161_u64,
    ];
    for (index, byte) in bytes.iter().copied().enumerate() {
        let octet = u64::from(byte);
        words[0] = (words[0] ^ octet).wrapping_mul(1_099_511_628_211);
        words[1] =
            (words[1] ^ octet.wrapping_add(index as u64)).wrapping_mul(14_029_467_366_897_019_727);
        words[2] = words[2]
            .wrapping_add(octet)
            .wrapping_mul(11_400_714_785_074_694_791);
        words[3] = (words[3] ^ (octet << (index % 8))).wrapping_mul(9_650_029_242_287_828_579);
    }
    words
}

#[derive(Clone, Copy, Debug, Default)]
struct OneRootSource {
    present: bool,
    feature: FeatureId,
    identity: u32,
    q_lo: i32,
    q_hi: i32,
    orientation: i8,
    initial_inside_bits: u64,
    normal_z: i32,
}

impl VisibilityProgram for OneRootSource {
    fn indexed_features(
        &self,
        _tile: TileDomain,
        _row: u16,
        output: &mut [IndexedFeature],
    ) -> Result<usize, SweepError> {
        if !self.present {
            return Ok(0);
        }
        let slot = output.get_mut(0).ok_or(SweepError::CapacityExceeded)?;
        *slot = IndexedFeature {
            id: self.feature,
            row_start: 0,
            row_end: 1,
            exclusion: ExclusionResult::Retain,
        };
        Ok(1)
    }

    fn isolate_feature_roots(
        &self,
        feature: FeatureId,
        _row: u16,
        _x_anchor: u16,
        output: &mut [RootSheet],
    ) -> Result<RootIsolationSummary, SweepError> {
        if !self.present || feature != self.feature {
            return Err(SweepError::InternalInvariant);
        }
        let q = Iv32::new(self.q_lo, self.q_hi).map_err(|_| SweepError::NumericFailure)?;
        let slot = output.get_mut(0).ok_or(SweepError::CapacityExceeded)?;
        *slot = RootSheet {
            root: RootRecord {
                feature,
                object: ObjectId(0),
                identity_set: IdentitySetId(self.identity),
                q,
                orientation: self.orientation,
                validity_margin: 32,
                root_slack: 32,
                dedup_owner: 0,
                support_sublevel_proof: true,
            },
            q_model: QModel {
                q0: q,
                qx: Iv32::point(0),
                qxx: Iv32::point(0),
            },
            q_domain: q,
            q_error: Iv32::point(1),
            q_u: Iv32::point(0),
            q_v: Iv32::point(0),
            normal_model: NormalModel {
                nx: Iv32::point(0),
                ny: Iv32::point(0),
                nz: Iv32::point(self.normal_z),
            },
            q_order_slack: 32,
            root_slack: 32,
            feature_slack: 32,
            branch_slack: 32,
            fixed_q_slack: 32,
            expires_at: 1,
            method: 0,
            composition_shape: 1,
        };
        Ok(RootIsolationSummary {
            root_count: 1,
            complete: true,
        })
    }

    fn isolate_row_events(
        &self,
        _tile: TileDomain,
        _row: u16,
        _output: &mut [EventInterval],
    ) -> Result<usize, SweepError> {
        Ok(0)
    }

    fn csg_program(&self) -> &[CsgInstruction] {
        const PROGRAM: &[CsgInstruction] = &[CsgInstruction::Object(0)];
        PROGRAM
    }

    fn initial_inside_bits(&self, _row: u16, _x_anchor: u16) -> Result<u64, SweepError> {
        Ok(self.initial_inside_bits)
    }

    fn rebuild_pixel(
        &self,
        _tier: RebuildTier,
        _row: u16,
        _cell: RebuildCell,
        _events: &[EventInterval],
    ) -> TierResult<DebugPixel> {
        TierResult::Inconclusive
    }
}

fn rendered_source_frame(source: OneRootSource) -> Result<RenderedVisibility, String> {
    type Workspace = VisibilityWorkspace<1, 2, 1, 2, 2>;
    let mut workspace = Workspace::default();
    let mut pixels = [DebugPixel::default(); 1];
    let tile = render_visibility_tile(
        &source,
        TileDomain {
            tile_id: 0,
            x0: 0,
            x1: 1,
            y0: 0,
            y1: 1,
        },
        RebuildLimits {
            max_x_depth: 0,
            max_q_depth: 0,
            max_cells: 1,
        },
        &mut workspace,
        &mut pixels,
    )
    .map_err(|error| format!("render visibility: {error:?}"))?;
    let run = workspace
        .runs()
        .first()
        .ok_or_else(|| "render visibility emitted no complete run".to_string())?;
    Ok(RenderedVisibility {
        hit: run.visible.is_some(),
        identity: run.identity.0,
        q_lo: f64::from(run.q_model.q0.lo) / 1024.0,
        q_hi: f64::from(run.q_model.q0.hi) / 1024.0,
        normal_lo: [
            f64::from(run.normal_model.nx.lo) / 1024.0,
            f64::from(run.normal_model.ny.lo) / 1024.0,
            f64::from(run.normal_model.nz.lo) / 1024.0,
        ],
        normal_hi: [
            f64::from(run.normal_model.nx.hi) / 1024.0,
            f64::from(run.normal_model.ny.hi) / 1024.0,
            f64::from(run.normal_model.nz.hi) / 1024.0,
        ],
        tile_digest: tile.digest,
    })
}

fn rendered_single_root(
    q_lo: i32,
    q_hi: i32,
    orientation: i8,
    identity: u32,
    initial_inside_bits: u64,
    normal_z: i32,
) -> Result<RenderedVisibility, String> {
    rendered_source_frame(OneRootSource {
        present: true,
        feature: FeatureId(0),
        identity,
        q_lo,
        q_hi,
        orientation,
        initial_inside_bits,
        normal_z,
    })
}

fn rendered_sphere(camera_inside: bool) -> Result<RenderedVisibility, String> {
    if camera_inside {
        rendered_single_root(3071, 3073, -1, 7, 1, 1024)
    } else {
        rendered_single_root(1023, 1025, 1, 7, 0, -1024)
    }
}

fn oracle_linear(root: f64, identity: u32) -> Result<super::oracle::VisibilityScore, String> {
    let dual_range = |lo: f64, hi: f64| {
        Some((
            OracleInterval::new(lo - root, hi - root)?,
            OracleInterval::new(1.0, 1.0)?,
        ))
    };
    let value = |q: f64| q - root;
    let terminal = |q: f64| {
        Some(OracleTerminal {
            value: value(q),
            gradient: OracleVec3 {
                x: 0.0,
                y: 0.0,
                z: 1.0,
            },
            identity,
        })
    };
    oracle(
        SemanticRay {
            dual_range: &dual_range,
            value: &value,
            terminal: &terminal,
        },
        false,
    )
}

fn oracle_sphere(camera_inside: bool) -> Result<super::oracle::VisibilityScore, String> {
    let dual_range = |lo: f64, hi: f64| {
        let at_lo = (lo - 2.0).powi(2) - 1.0;
        let at_hi = (hi - 2.0).powi(2) - 1.0;
        let minimum = if lo <= 2.0 && 2.0 <= hi {
            -1.0
        } else {
            at_lo.min(at_hi)
        };
        Some((
            OracleInterval::new(minimum, at_lo.max(at_hi))?,
            OracleInterval::new(2.0 * (lo - 2.0), 2.0 * (hi - 2.0))?,
        ))
    };
    let value = |q: f64| (q - 2.0).powi(2) - 1.0;
    let terminal = |q: f64| {
        Some(OracleTerminal {
            value: value(q),
            gradient: OracleVec3 {
                x: 0.0,
                y: 0.0,
                z: q - 2.0,
            },
            identity: 7,
        })
    };
    oracle(
        SemanticRay {
            dual_range: &dual_range,
            value: &value,
            terminal: &terminal,
        },
        camera_inside,
    )
}

fn oracle(
    ray: SemanticRay<'_>,
    camera_inside: bool,
) -> Result<super::oracle::VisibilityScore, String> {
    let mut stack = [OracleCell::default(); 64];
    let mut roots = [OracleRoot::default(); 8];
    let count = isolate_all_roots(ray, 0.0, 4.0, 1.0 / 4096.0, 15, &mut stack, &mut roots)
        .map_err(|error| format!("oracle: {error:?}"))?;
    Ok(first_boundary(&roots[..count], camera_inside))
}

fn score(
    name: &str,
    rendered: RenderedVisibility,
    oracle: super::oracle::VisibilityScore,
    report: &mut String,
    failures: &mut u32,
    unresolved: &mut u32,
) -> Result<(), String> {
    *unresolved += oracle.unresolved;
    let contains = rendered.q_lo <= oracle.t.hi && oracle.t.lo <= rendered.q_hi;
    let normal_contains = [oracle.normal.x, oracle.normal.y, oracle.normal.z]
        .into_iter()
        .enumerate()
        .all(|(axis, value)| {
            rendered.normal_lo[axis] <= value && value <= rendered.normal_hi[axis]
        });
    let pass = rendered.hit == oracle.hit
        && (!rendered.hit || (rendered.identity == oracle.identity && contains && normal_contains))
        && oracle.unresolved == 0;
    *failures += u32::from(!pass);
    writeln!(
        report,
        "case={name} hit={} identity={} q=[{:.6},{:.6}] oracle=[{:.6},{:.6}] status={}",
        rendered.hit,
        rendered.identity,
        rendered.q_lo,
        rendered.q_hi,
        oracle.t.lo,
        oracle.t.hi,
        if pass { "pass" } else { "fail" },
    )
    .map_err(|_| "conformance report formatting failed".to_string())
}

#[cfg(test)]
mod tests {

    #[test]
    fn scan_order_score_merge_preserves_first_issue_and_fails_on_overflow() {
        let mut total = super::FrameScore {
            checked_interior: 2,
            first_issue: Some([1, 2, 3]),
            ..Default::default()
        };
        super::merge_frame_score(
            &mut total,
            super::FrameScore {
                checked_interior: 5,
                first_issue: Some([9, 9, 9]),
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(total.checked_interior, 7);
        assert_eq!(total.first_issue, Some([1, 2, 3]));

        total.unresolved = u32::MAX;
        let error = super::merge_frame_score(
            &mut total,
            super::FrameScore {
                unresolved: 1,
                ..Default::default()
            },
        )
        .unwrap_err();
        assert!(error.contains("unresolved counter overflow"));
    }

    fn fixture_renderer(case: &str) -> crate::pixels::CompiledRenderer {
        use std::collections::BTreeMap;
        use std::sync::{Arc, Mutex, OnceLock};

        // Compiling a fixture runs the whole front end, and sibling tests in
        // this module reuse the same cases; cache one compilation per case
        // for the life of the test process. The per-case cell lets parallel
        // test threads block only on the case they need instead of
        // serializing distinct compilations behind one lock.
        type Cache = Mutex<BTreeMap<String, Arc<OnceLock<crate::pixels::CompiledRenderer>>>>;
        static CACHE: OnceLock<Cache> = OnceLock::new();
        let cell = CACHE
            .get_or_init(Cache::default)
            .lock()
            .unwrap()
            .entry(case.to_string())
            .or_default()
            .clone();
        cell.get_or_init(|| {
            let repository = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
            let directory = repository.join("tests/golden").join(case);
            let root = directory.join("root");
            let fixture = if root.is_file() {
                directory.join(std::fs::read_to_string(root).unwrap().trim())
            } else {
                directory.join("input.wr")
            };
            crate::cost::stage::load_pixels_programs(&fixture)
                .unwrap_or_else(|error| panic!("{case}: {error}"))
                .compiled_renderers
                .into_iter()
                .next()
                .unwrap_or_else(|| panic!("{case}: renderer missing"))
        })
        .clone()
    }

    #[test]
    fn captured_frame_digest_matches_the_guest_byte_order() {
        let frame = [255_u8, 0, 0, 255].repeat(64 * 32);
        assert_eq!(
            super::debug_frame_digest(&frame),
            [
                3_954_935_440_892_105_603,
                12_936_886_627_447_603_635,
                9_115_567_371_493_163_330,
                13_084_624_655_846_693_369,
            ],
        );
    }

    #[test]
    fn derivative_evidence_requires_the_complete_certified_radius() {
        // The oracle interval reverses when the projective normal factor is
        // negative. Both orientations must be enclosed, and removing the
        // final raw unit from either side must fail closed.
        assert!(super::derivative_enclosure_contains(
            -100, 20, 0.25, 2.0, 3.0, -10.0,
        ));
        assert!(!super::derivative_enclosure_contains(
            -100, 19, 0.25, 2.0, 3.0, -10.0,
        ));
        assert!(!super::derivative_enclosure_contains(
            -100,
            20,
            0.25,
            2.0,
            3.0,
            f64::NAN,
        ));
    }

    #[test]
    fn event_coverage_grid_rounding_and_exact_fixture_bytes_are_pinned() {
        assert_eq!(super::grid_coverage_byte(0, 64), Ok(0));
        assert_eq!(super::grid_coverage_byte(32, 64), Ok(128));
        assert_eq!(super::grid_coverage_byte(64, 64), Ok(255));
        assert!(super::grid_coverage_byte(1, 0).is_err());
        // The retired Richardson extrapolation would have read this pair as
        // `2*fine - coarse`, i.e. a fraction of 2*33/64 - 17/32 = 0.5, and
        // reported 128. Each grid on its own rounds to 132, and the estimator
        // now refines to agreement instead of extrapolating between them.
        assert_eq!(super::grid_coverage_byte(17, 32), Ok(135));
        assert_eq!(super::grid_coverage_byte(33, 64), Ok(131));
        assert_eq!(
            super::exact_fixture_event_byte("check-pixels-material-edge", 16, 0),
            Ok(Some(200))
        );
        assert_eq!(
            super::exact_fixture_event_byte("check-pixels-material-edge", 17, 0),
            Ok(Some(240))
        );
        assert_eq!(
            super::exact_fixture_event_byte("check-pixels-tile-boundary", 50, 13),
            Ok(Some(183))
        );
        assert_eq!(
            super::exact_fixture_event_byte("check-pixels-tile-boundary", 52, 14),
            Err(
                "tile-boundary event evidence lies outside the exact owned lanes at (52,14)"
                    .to_string()
            )
        );
    }

    #[test]
    #[ignore = "whole-fixture semantic sweep; the mandatory pixels-conformance gate covers every scene, and verify-deep rechecks ignored proofs"]
    fn permanent_visibility_fixtures_have_resolved_source_semantic_alpha_oracles() {
        for case in [
            "boot-pixels-plane",
            "boot-pixels-plane-one-core",
            "check-pixels-camera-inside",
            "check-pixels-close-depth",
            "check-pixels-displace",
            "check-pixels-enclosed-feature",
            "check-pixels-hard-csg",
            "check-pixels-material-edge",
            "check-pixels-repeat",
            "check-pixels-simultaneous-event",
            "check-pixels-smooth-csg",
            "check-pixels-tangent",
            "check-pixels-thin-feature",
            "check-pixels-torus-roots",
        ] {
            let renderer = fixture_renderer(case);
            let evaluator = super::SemanticFieldEvaluator::new(&renderer)
                .unwrap_or_else(|error| panic!("{case}: {error}"));
            let alpha =
                super::semantic_alpha_samples(&evaluator, &super::CANONICAL_CAMERA, &[0.0; 16])
                    .unwrap_or_else(|error| panic!("{case}: {error}"));
            assert_eq!(alpha.unresolved, 0, "{case}");
        }
    }

    #[test]
    fn hard_csg_nonsmooth_alpha_ray_uses_the_resolved_normal_fallback() {
        let renderer = fixture_renderer("check-pixels-hard-csg");
        let evaluator = super::SemanticFieldEvaluator::new(&renderer).expect("evaluator");
        // The first sub-sample of alpha probe x=32 lands on a hard CSG
        // selection where all forward derivative intervals contain zero.
        // The symmetric terminal probe must still recover the visible normal.
        let score = super::semantic_ray_score_with(
            &evaluator,
            &super::CANONICAL_CAMERA,
            &[0.0; 16],
            1.0 / 512.0,
            -1.0 / 512.0,
        )
        .expect("hard CSG boundary must resolve");
        assert!(score.hit);
        assert_eq!(score.unresolved, 0);
        assert!(
            [score.normal.x, score.normal.y, score.normal.z]
                .into_iter()
                .all(f64::is_finite)
        );
    }

    #[test]
    fn repeat_frame_probe_lattice_has_no_unresolved_rays() {
        let renderer = fixture_renderer("check-pixels-repeat");
        let evaluator = super::SemanticFieldEvaluator::new(&renderer).unwrap();
        let width = renderer.config.width;
        let height = renderer.config.height;
        let aspect = f64::from(width) / f64::from(height);
        let mut unresolved = Vec::new();
        for y in 0..height {
            for x in 0..width {
                for (slot, (dx, dy)) in [
                    (0.5, 0.5),
                    (0.125, 0.125),
                    (0.875, 0.125),
                    (0.125, 0.875),
                    (0.875, 0.875),
                ]
                .into_iter()
                .enumerate()
                {
                    let u = ((f64::from(x) + dx) / f64::from(width) * 2.0 - 1.0) * aspect;
                    let v = 1.0 - (f64::from(y) + dy) / f64::from(height) * 2.0;
                    let sample =
                        super::sample_ray(&evaluator, &super::CANONICAL_CAMERA, &[0.0; 16], u, v)
                            .unwrap();
                    if sample.unresolved != 0 {
                        unresolved.push((x, y, slot, dx, dy));
                    }
                }
            }
        }
        assert!(unresolved.is_empty(), "unresolved probes: {unresolved:?}");
    }

    #[test]
    fn repeat_aabb_corner_is_proved_empty_by_projective_features() {
        let renderer = fixture_renderer("check-pixels-repeat");
        let width = f64::from(renderer.config.width);
        let height = f64::from(renderer.config.height);
        let aspect = width / height;
        let u_at = |x: f64| (x / width * 2.0 - 1.0) * aspect;
        let v_at = |y: f64| 1.0 - y / height * 2.0;

        // This is the first historical false-positive cell from interval
        // evaluation of the folded finite-repeat field.  Every point probe
        // misses, and the independent projective sphere equations prove the
        // complete pixel frustum root-free.
        assert!(
            super::projective_bundle_root_free(
                &renderer,
                &super::CANONICAL_CAMERA,
                &[0.0; 16],
                u_at(20.0),
                v_at(14.0),
                u_at(21.0),
                v_at(15.0),
                32_768,
            )
            .unwrap()
        );

        // A pixel crossing the centre sphere must never be certified empty.
        assert!(
            !super::projective_bundle_root_free(
                &renderer,
                &super::CANONICAL_CAMERA,
                &[0.0; 16],
                u_at(31.0),
                v_at(15.0),
                u_at(32.0),
                v_at(16.0),
                32_768,
            )
            .unwrap()
        );
    }

    #[test]
    fn projective_absence_declines_for_deformed_predictor_features() {
        let renderer = fixture_renderer("check-pixels-displace");
        let width = f64::from(renderer.config.width);
        let height = f64::from(renderer.config.height);
        let aspect = width / height;
        let u_at = |x: f64| (x / width * 2.0 - 1.0) * aspect;
        let v_at = |y: f64| 1.0 - y / height * 2.0;

        // This occupied edge pixel was falsely called phantom when a
        // deformation predictor was treated as the final feature equation.
        assert!(
            !super::projective_bundle_root_free(
                &renderer,
                &super::CANONICAL_CAMERA,
                &[0.0; 16],
                u_at(28.0),
                v_at(12.0),
                u_at(29.0),
                v_at(13.0),
                32_768,
            )
            .unwrap()
        );
    }

    #[test]
    fn displaced_coverage_edge_pixel_is_not_a_point_witness() {
        let renderer = fixture_renderer("check-pixels-displace");
        let evaluator = super::SemanticFieldEvaluator::new(&renderer).unwrap();
        let fixed_q = renderer
            .program
            .program()
            .table(wrela_machine::pixels::FrameProgramTableKindV1::FixedDomain)
            .and_then(|table| table.records.iter().find(|record| record.tag == 5))
            .unwrap();
        let q_scale = 2.0_f64.powi(fixed_q.operands[0] as i64 as i32);
        let guest_q = (
            f64::from(0x8de2_u32) * q_scale,
            f64::from(0x9f15_u32) * q_scale,
        );
        let width = f64::from(renderer.config.width);
        let height = f64::from(renderer.config.height);
        let aspect = width / height;
        let mut matched = false;
        for sy in 0..4 {
            for sx in 0..4 {
                let x = 37.0 + (f64::from(sx) + 0.5) / 4.0;
                let y = 18.0 + (f64::from(sy) + 0.5) / 4.0;
                let u = (x / width * 2.0 - 1.0) * aspect;
                let v = 1.0 - y / height * 2.0;
                let oracle = super::semantic_ray_score_with(
                    &evaluator,
                    &super::CANONICAL_CAMERA,
                    &[0.0; 16],
                    u,
                    v,
                )
                .unwrap();
                if oracle.unresolved == 0
                    && oracle.hit
                    && guest_q.0 <= 1.0 / oracle.t.hi
                    && guest_q.1 >= 1.0 / oracle.t.lo
                {
                    matched = true;
                }
            }
        }
        assert!(!matched);

        let u = (56.0 / width * 2.0 - 1.0) * aspect;
        let v = 1.0 - 31.5 / height * 2.0;
        let background =
            super::semantic_ray_score_with(&evaluator, &super::CANONICAL_CAMERA, &[0.0; 16], u, v)
                .unwrap();
        assert!(!background.hit);
        assert_eq!(background.unresolved, 0);
    }

    #[test]
    fn displaced_predictor_corner_is_semantically_empty() {
        let renderer = fixture_renderer("check-pixels-displace");
        let evaluator = super::SemanticFieldEvaluator::new(&renderer).unwrap();
        let width = f64::from(renderer.config.width);
        let height = f64::from(renderer.config.height);
        let aspect = width / height;
        let u_at = |x: f64| (x / width * 2.0 - 1.0) * aspect;
        let v_at = |y: f64| 1.0 - y / height * 2.0;
        let grid = super::event_grid_oracle(
            &evaluator,
            &super::CANONICAL_CAMERA,
            &[0.0; 16],
            u_at,
            v_at,
            28,
            12,
            4,
        )
        .unwrap();
        assert_eq!(grid.background, 16, "{grid:?}");
        assert!(grid.counts.is_empty(), "{grid:?}");

        let edge = super::semantic_ray_score_with(
            &evaluator,
            &super::CANONICAL_CAMERA,
            &[0.0; 16],
            u_at(28.99),
            v_at(12.99),
        )
        .unwrap();
        assert!(edge.hit, "{edge:?}");
        assert_eq!(edge.unresolved, 0, "{edge:?}");
    }

    /// Uniform-grid convergence ladder for the displaced silhouette.
    ///
    /// This is a whole-ladder proof, not a unit: it evaluates roughly 2.2M
    /// semantic rays across 256/512/1024-per-axis grids and cost 22s of the
    /// default lane on its own. `bench/thresholds.toml`'s `[tests]` note
    /// classifies exactly this shape into the milestone lane while a focused
    /// smoke case stays in `verify` — here
    /// `displaced_predictor_corner_is_semantically_empty` (the 4-per-axis
    /// empty corner and the edge ray) and
    /// `quadtree_event_oracle_agrees_with_the_converged_grid_bytes` (the
    /// acceptance-path oracle against these recorded bytes). The recorded
    /// counts below are the convergence evidence those smoke cases cite.
    #[test]
    #[ignore = "milestone lane: 2.2M-ray uniform grid ladder; see verify-deep"]
    fn displaced_predictor_grid_ladder_converges_to_the_recorded_bytes() {
        let renderer = fixture_renderer("check-pixels-displace");
        let evaluator = super::SemanticFieldEvaluator::new(&renderer).unwrap();
        let width = f64::from(renderer.config.width);
        let height = f64::from(renderer.config.height);
        let aspect = width / height;
        let u_at = |x: f64| (x / width * 2.0 - 1.0) * aspect;
        let v_at = |y: f64| 1.0 - y / height * 2.0;

        let coarse = super::event_grid_oracle(
            &evaluator,
            &super::CANONICAL_CAMERA,
            &[0.0; 16],
            u_at,
            v_at,
            28,
            12,
            256,
        )
        .unwrap();
        let fine = super::event_grid_oracle(
            &evaluator,
            &super::CANONICAL_CAMERA,
            &[0.0; 16],
            u_at,
            v_at,
            28,
            12,
            512,
        )
        .unwrap();
        assert_eq!(coarse.samples, 65_536);
        assert_eq!(coarse.background, 65_431);
        assert_eq!(coarse.counts.len(), 1);
        assert_eq!(coarse.counts.get(&0), Some(&105));
        assert_eq!(fine.samples, 262_144);
        assert_eq!(fine.background, 261_709);
        assert_eq!(fine.counts.len(), 1);
        assert_eq!(fine.counts.get(&0), Some(&435));
        let boundary = super::event_grid_oracle(
            &evaluator,
            &super::CANONICAL_CAMERA,
            &[0.0; 16],
            u_at,
            v_at,
            29,
            12,
            512,
        )
        .unwrap();
        assert_eq!(boundary.samples, 262_144);
        assert_eq!(boundary.background, 150_185);
        assert_eq!(boundary.counts.len(), 1);
        assert_eq!(boundary.counts.get(&0), Some(&111_959));
        let failing_coarse = super::event_grid_oracle(
            &evaluator,
            &super::CANONICAL_CAMERA,
            &[0.0; 16],
            u_at,
            v_at,
            27,
            15,
            256,
        )
        .unwrap();
        let failing_fine = super::event_grid_oracle(
            &evaluator,
            &super::CANONICAL_CAMERA,
            &[0.0; 16],
            u_at,
            v_at,
            27,
            15,
            512,
        )
        .unwrap();
        assert_eq!(failing_coarse.samples, 65_536);
        assert_eq!(failing_coarse.background, 54_327);
        assert_eq!(failing_coarse.counts.len(), 1);
        assert_eq!(failing_coarse.counts.get(&0), Some(&11_209));
        assert_eq!(failing_fine.samples, 262_144);
        assert_eq!(failing_fine.background, 217_337);
        assert_eq!(failing_fine.counts.len(), 1);
        assert_eq!(failing_fine.counts.get(&0), Some(&44_807));
        let cap_coarse = super::event_grid_oracle(
            &evaluator,
            &super::CANONICAL_CAMERA,
            &[0.0; 16],
            u_at,
            v_at,
            28,
            13,
            256,
        )
        .unwrap();
        let cap_fine = super::event_grid_oracle(
            &evaluator,
            &super::CANONICAL_CAMERA,
            &[0.0; 16],
            u_at,
            v_at,
            28,
            13,
            512,
        )
        .unwrap();
        assert_eq!(cap_coarse.samples, 65_536);
        assert_eq!(cap_coarse.background, 36_202);
        assert_eq!(cap_coarse.counts.len(), 1);
        assert_eq!(cap_coarse.counts.get(&0), Some(&29_334));
        assert_eq!(cap_fine.samples, 262_144);
        assert_eq!(cap_fine.background, 144_786);
        assert_eq!(cap_fine.counts.len(), 1);
        assert_eq!(cap_fine.counts.get(&0), Some(&117_358));
        let right_fine = super::event_grid_oracle(
            &evaluator,
            &super::CANONICAL_CAMERA,
            &[0.0; 16],
            u_at,
            v_at,
            34,
            13,
            1024,
        )
        .unwrap();
        assert_eq!(right_fine.samples, 1_048_576);
        assert_eq!(right_fine.background, 807_986);
        assert_eq!(right_fine.counts.len(), 1);
        assert_eq!(right_fine.counts.get(&0), Some(&240_590));

        let rounded_byte = |hits: u64, samples: u64| (hits * 255 + samples / 2) / samples;
        assert_eq!(rounded_byte(435, 262_144), 0);
        assert_eq!(rounded_byte(111_959, 262_144), 109);
        assert_eq!(rounded_byte(44_807, 262_144), 44);
        assert_eq!(rounded_byte(117_358, 262_144), 114);
        assert_eq!(rounded_byte(240_590, 1_048_576), 59);
    }

    /// Pixels of `check-pixels-displace` whose display byte the uniform-grid
    /// ladder converged, with the byte it converged to.
    ///
    /// `displaced_predictor_grid_ladder_converges_to_the_recorded_bytes` (the
    /// milestone lane) re-derives these bytes from the grids; the quadtree
    /// oracle checks against them here. `(34,13)` is the pixel whose finite
    /// grids alternated 58/59 across resolutions, which is why the agreement
    /// rule below allows one rounding step.
    const QUADTREE_LADDER_BYTES: &[(usize, usize, u8)] =
        &[(29, 12, 109), (27, 15, 44), (28, 13, 114), (34, 13, 59)];

    /// Assert the quadtree oracle's byte interval agrees with one converged
    /// grid byte: within one rounding step of it, and settled to a single
    /// byte or an adjacent rounding pair.
    fn assert_quadtree_agrees_with_ladder_byte(
        evaluator: &super::SemanticFieldEvaluator<'_>,
        u_at: impl Fn(f64) -> f64 + Copy,
        v_at: impl Fn(f64) -> f64 + Copy,
        x: usize,
        y: usize,
        byte: u8,
    ) {
        let (winner, back, low, high) = super::sampled_event_oracle(
            evaluator,
            &super::CANONICAL_CAMERA,
            &[0.0; 16],
            u_at,
            v_at,
            x,
            y,
        )
        .unwrap();
        assert_eq!((winner, back), (0, 0), "({x},{y})");
        assert!(
            i32::from(low) - 1 <= i32::from(byte) && i32::from(byte) <= i32::from(high) + 1,
            "({x},{y}): grid byte {byte} outside quadtree {low}..={high}"
        );
        assert!(
            high - low <= 1,
            "({x},{y}): quadtree did not settle: {low}..={high}"
        );
    }

    /// Smoke case kept in `verify`: the provably empty pixel.
    ///
    /// Chasing a silhouette boundary costs the quadtree about three seconds
    /// per pixel, so every *converged* pixel runs in the milestone lane
    /// below — which covers all four, not a sample. What stays here is the
    /// cheap end of the same oracle: a pixel the 4-per-axis grid proved
    /// empty must come back exactly zero, with no winner identity. That
    /// catches a broken or mis-wired oracle immediately; the byte-agreement
    /// proof is `verify-deep`'s, per `bench/thresholds.toml`'s `[tests]`
    /// placement rule.
    #[test]
    fn quadtree_event_oracle_reports_a_proved_empty_pixel_as_exactly_zero() {
        let renderer = fixture_renderer("check-pixels-displace");
        let evaluator = super::SemanticFieldEvaluator::new(&renderer).unwrap();
        let width = f64::from(renderer.config.width);
        let height = f64::from(renderer.config.height);
        let aspect = width / height;
        let u_at = |x: f64| (x / width * 2.0 - 1.0) * aspect;
        let v_at = |y: f64| 1.0 - y / height * 2.0;
        // The converged bytes the milestone lane checks against are recorded
        // here, so a table edited without rerunning that lane is visible.
        assert_eq!(
            QUADTREE_LADDER_BYTES,
            &[(29, 12, 109_u8), (27, 15, 44), (28, 13, 114), (34, 13, 59)]
        );
        assert_eq!(
            super::sampled_event_oracle(
                &evaluator,
                &super::CANONICAL_CAMERA,
                &[0.0; 16],
                u_at,
                v_at,
                28,
                12,
            )
            .unwrap(),
            (0, 0, 0, 0)
        );
    }

    #[test]
    #[ignore = "milestone lane: remaining converged quadtree pixels; see verify-deep"]
    fn quadtree_event_oracle_agrees_on_every_converged_grid_byte() {
        let renderer = fixture_renderer("check-pixels-displace");
        let evaluator = super::SemanticFieldEvaluator::new(&renderer).unwrap();
        let width = f64::from(renderer.config.width);
        let height = f64::from(renderer.config.height);
        let aspect = width / height;
        let u_at = |x: f64| (x / width * 2.0 - 1.0) * aspect;
        let v_at = |y: f64| 1.0 - y / height * 2.0;
        for &(x, y, byte) in QUADTREE_LADDER_BYTES {
            assert_quadtree_agrees_with_ladder_byte(&evaluator, u_at, v_at, x, y, byte);
        }
    }

    #[test]
    fn conformance_is_deterministic_and_has_no_unresolved_visibility() {
        let observed = [
            3_954_935_440_892_105_603,
            12_936_886_627_447_603_635,
            9_115_567_371_493_163_330,
            13_084_624_655_846_693_369,
        ];
        let probe = super::GuestVisibilityProbe {
            hit: true,
            identity: 0,
            q_lo: 30_067,
            q_hi: 30_196,
            normal_valid: true,
            normal: [0, 0, 1_000_000],
            coverage: 255,
        };
        let observations = [
            super::GuestObservation {
                case: "boot-pixels-plane".to_string(),
                certificate_runs: 128,
                event_corridors: 0,
                revalidated_proposals: 120,
                frame_digest: observed,
                alpha_samples: [255; 3],
                visibility_probe: Some(probe),
                run_evidence: Some([
                    31 | (32 << 16),
                    0,
                    30_067 | (30_196_u64 << 32),
                    16 | (32_767_u64 << 48),
                    30_067 | (30_196_u64 << 32),
                    0,
                    0,
                    0,
                    1 | (1_u64 << 32),
                    1 | (1_u64 << 32),
                    0,
                    0,
                    32767 | (32767_u64 << 32),
                    u64::from(u32::MAX) | (u64::from(u32::MAX) << 32),
                    8 | (8_u64 << 32),
                    1_u64 << 16,
                ]),
                frame_dump: None,
            },
            super::GuestObservation {
                case: "boot-pixels-plane-one-core".to_string(),
                certificate_runs: 128,
                event_corridors: 0,
                revalidated_proposals: 120,
                frame_digest: observed,
                alpha_samples: [255; 3],
                visibility_probe: None,
                run_evidence: Some([
                    31 | (32 << 16),
                    0,
                    30_067 | (30_196_u64 << 32),
                    16 | (32_767_u64 << 48),
                    30_067 | (30_196_u64 << 32),
                    0,
                    0,
                    0,
                    1 | (1_u64 << 32),
                    1 | (1_u64 << 32),
                    0,
                    0,
                    32767 | (32767_u64 << 32),
                    u64::from(u32::MAX) | (u64::from(u32::MAX) << 32),
                    8 | (8_u64 << 32),
                    1_u64 << 16,
                ]),
                frame_dump: None,
            },
        ];
        // The boot-pixels-plane `root` file resolves to the same
        // src/examples/boot_pixels_plane.wr, so the cached fixture is the
        // identical compilation.
        let renderer = fixture_renderer("boot-pixels-plane");
        let renderers = [renderer.clone(), renderer];
        let first = super::run(&observations, &renderers).unwrap();
        let second = super::run(&observations, &renderers).unwrap();
        assert_eq!(first, second);
        assert!(first.contains("failures=0 unresolved=0"));
    }

    #[test]
    fn retained_pixel_area_must_round_strictly_below_one_coverage_byte() {
        // At level 8 there are 65,536 equal cells. 128 cells cover just less
        // than half of one 8-bit step; 129 cells cross that boundary and may
        // not be accepted as background.
        assert!(super::possible_coverage_rounds_to_zero(128, 8));
        assert!(!super::possible_coverage_rounds_to_zero(129, 8));
        assert!(super::possible_coverage_rounds_to_zero(0, 0));
        assert!(!super::possible_coverage_rounds_to_zero(1, 0));
    }
}

#[cfg(test)]
mod hard_csg_coverage_probe {
    //! Host-side convergence probe for the `check-pixels-hard-csg` event
    //! disagreement. The guest boot for that fixture is ~7 minutes; the
    //! semantic evaluator is host-side, so the *true* coverage can be
    //! converged here in seconds and the guest byte checked against it
    //! without booting anything.
    use super::*;

    #[test]
    #[ignore = "diagnostic probe; run explicitly with --ignored"]
    fn converge_true_event_coverage() {
        let target = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(
            std::env::var("WRELA_PROBE_FIXTURE").unwrap_or_else(|_| {
                "../../tests/golden/check-pixels-hard-csg/src/examples/check_pixels_hard_csg.wr"
                    .to_string()
            }),
        );
        let programs = crate::cost::stage::load_pixels_programs(&target).expect("compile fixture");
        let renderer = programs
            .compiled_renderers
            .into_iter()
            .next()
            .expect("sealed renderer");
        let width = renderer.config.width as usize;
        let height = renderer.config.height as usize;
        let aspect = width as f64 / height as f64;
        let u_at = |px: f64| (px / width as f64 * 2.0 - 1.0) * aspect;
        let v_at = |py: f64| 1.0 - py / height as f64 * 2.0;
        let evaluator = SemanticFieldEvaluator::new(&renderer).expect("evaluator");
        let camera = CANONICAL_CAMERA;
        let params = [0.0_f32; 16];
        let probe_pixels: Vec<(usize, usize, u8)> = std::env::var("WRELA_PROBE_PIXELS")
            .ok()
            .map(|spec| {
                spec.split(';')
                    .filter(|part| !part.is_empty())
                    .map(|part| {
                        let mut it = part.split(',');
                        (
                            it.next().unwrap().parse().unwrap(),
                            it.next().unwrap().parse().unwrap(),
                            it.next().unwrap().parse().unwrap(),
                        )
                    })
                    .collect()
            })
            .unwrap_or_else(|| vec![(32_usize, 11_usize, 198_u8), (41, 12, 141)]);
        for (x, y, guest) in probe_pixels {
            println!("pixel ({x},{y}) guest={guest}");
            match sampled_event_oracle(&evaluator, &camera, &params, u_at, v_at, x, y) {
                Ok((winner, back, low, high)) => {
                    println!("  quadtree winner={winner} back={back} coverage={low}..={high}")
                }
                Err(error) => println!("  quadtree error: {error}"),
            }
            for event in &renderer.projective.program().events.generators {
                if (x as u32) >= event.pixels.x.start
                    && (x as u32) < event.pixels.x.end
                    && (y as u32) >= event.pixels.y.start
                    && (y as u32) < event.pixels.y.end
                {
                    println!("  event: {event:?}");
                }
            }
            // Depth structure: bucket every hit sample's nearest-hit depth to
            // reveal whether the pixel holds one surface sheet or several
            // same-identity sheets separated by an event corridor. Opt-in —
            // it costs 65k ray marches per pixel.
            if std::env::var("WRELA_PROBE_DEPTHS").is_err() {
                continue;
            }
            let resolution = 256_u32;
            let mut depth_buckets: std::collections::BTreeMap<i64, u64> =
                std::collections::BTreeMap::new();
            for sy in 0..resolution {
                for sx in 0..resolution {
                    let u = u_at(x as f64 + (f64::from(sx) + 0.5) / f64::from(resolution));
                    let v = v_at(y as f64 + (f64::from(sy) + 0.5) / f64::from(resolution));
                    let score = semantic_ray_score_with(&evaluator, &camera, &params, u, v)
                        .expect("depth sample");
                    if score.unresolved != 0 || !score.hit {
                        continue;
                    }
                    let depth = (score.t.lo + score.t.hi) * 0.5;
                    *depth_buckets
                        .entry((depth * 1000.0).round() as i64)
                        .or_insert(0) += 1;
                }
            }
            let mut depths: Vec<i64> = depth_buckets
                .iter()
                .flat_map(|(bucket, count)| std::iter::repeat_n(*bucket, *count as usize))
                .collect();
            depths.sort_unstable();
            println!(
                "  depth quantiles ({} hits at r{resolution}):",
                depths.len()
            );
            for percent in [
                0, 10, 20, 30, 40, 50, 60, 70, 80, 85, 90, 93, 95, 97, 99, 100,
            ] {
                let index = (depths.len().saturating_sub(1)) * percent / 100;
                println!("    p{percent} t={:.3}", depths[index] as f64 / 1000.0);
            }
        }
    }
}
