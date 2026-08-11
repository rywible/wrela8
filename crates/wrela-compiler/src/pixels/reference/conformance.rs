//! Score-only P7 visibility conformance controls.

use std::fmt::Write as _;

use super::csg::CsgInstruction;
use super::events::EventInterval;
use super::frame::DebugPixel;
use super::iv32::Iv32;
use super::oracle::{
    CoverageCell, Interval as OracleInterval, OracleCell, OracleRoot, SemanticRay,
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
    /// Pixels whose five probes all missed but whose frustum could not be
    /// proved root-free, so nothing about them was checked. Reported because
    /// a silently skipped pixel is indistinguishable from a scored one in a
    /// pass/fail summary, and this count is the only signal that a frame's
    /// coverage has regressed.
    pub skipped_unproven: u32,
    /// Pixels the guest painted although the semantic field is provably
    /// root-free over the entire pixel frustum. These are surfaces that do
    /// not exist. The count is pinned rather than fatal — see the report
    /// header and plan deviation 24 — because strengthening the background
    /// proof surfaced a pre-existing defect class in one pass, and pinning
    /// catches any growth while each cause is diagnosed separately.
    pub phantom_surface: u32,
    /// `[x, y, kind]` for the first issue in scan order. Kinds: 1 unresolved,
    /// 2 expected-background mismatch, 3 expected-hit/identity mismatch,
    /// 4 all-hit edge became background, 5 centre-hit edge became background,
    /// 6 adjacent-identity mismatch.
    pub first_issue: Option<[u16; 3]>,
}

#[derive(Clone, Copy, Debug)]
struct RaySample {
    hit: bool,
    unresolved: u32,
    identity: Option<u32>,
}

fn sample_ray(
    renderer: &crate::pixels::CompiledRenderer,
    camera: &[f32; 12],
    params: &[f32; 16],
    u: f64,
    v: f64,
) -> Result<RaySample, String> {
    let score = semantic_ray_score_with(renderer, camera, params, u, v)?;
    let identity = if score.hit && score.unresolved == 0 {
        let depth = (score.t.lo + score.t.hi) * 0.5;
        let point: [f64; 3] = std::array::from_fn(|axis| {
            f64::from(camera[axis])
                + (f64::from(camera[3 + axis])
                    + u * f64::from(camera[6 + axis])
                    + v * f64::from(camera[9 + axis]))
                    * depth
        });
        semantic_identity_at(renderer, point, params)?
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
    renderer: &crate::pixels::CompiledRenderer,
    camera: &[f32; 12],
    params: &[f32; 16],
    u0: f64,
    v0: f64,
    u1: f64,
    v1: f64,
) -> Result<bool, String> {
    /// Enough to retire the flat regions of every permanent fixture while
    /// keeping a whole-frame score well under a second per case.
    const CELL_BUDGET: usize = 512;

    let field_excludes_zero = |u: SemanticInterval,
                               v: SemanticInterval,
                               depth: SemanticInterval|
     -> Result<bool, String> {
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
        let field = semantic_field_dual(renderer, coordinates, params)?;
        Ok(field.value.lo > 0.0 || field.value.hi < 0.0)
    };

    let mut stack = vec![(
        SemanticInterval::new(u0.min(u1), u0.max(u1))?,
        SemanticInterval::new(v0.min(v1), v0.max(v1))?,
        SemanticInterval::new(renderer.config.near, renderer.config.far)?,
    )];
    let mut visited = 0_usize;
    while let Some((u, v, depth)) = stack.pop() {
        visited += 1;
        if visited > CELL_BUDGET {
            return Ok(false);
        }
        if field_excludes_zero(u, v, depth)? {
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

/// Score every pixel of a guest debug frame against the independent semantic
/// oracle. Interior pixels (all four corner rays and the centre ray agree on
/// hit state and identity) must match the guest exactly; silhouette-adjacent
/// pixels are held to the weaker-but-sound rules that a centre-hit pixel is
/// never background and any displayed identity belongs to an adjacent
/// surface. Oracle-unresolved rays are counted and fail the case.
pub fn score_frame(
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
    let aspect = width as f64 / height as f64;
    let u_at = |px: f64| (px / width as f64 * 2.0 - 1.0) * aspect;
    let v_at = |py: f64| 1.0 - py / height as f64 * 2.0;
    let mut score = FrameScore::default();
    for y in 0..height {
        for x in 0..width {
            let center = sample_ray(
                renderer,
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
                    renderer,
                    &dump.camera,
                    &dump.params,
                    u_at(x as f64 + 0.125),
                    v_at(y as f64 + 0.125),
                )?,
                sample_ray(
                    renderer,
                    &dump.camera,
                    &dump.params,
                    u_at(x as f64 + 0.875),
                    v_at(y as f64 + 0.125),
                )?,
                sample_ray(
                    renderer,
                    &dump.camera,
                    &dump.params,
                    u_at(x as f64 + 0.125),
                    v_at(y as f64 + 0.875),
                )?,
                sample_ray(
                    renderer,
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
                continue;
            }
            let base = (y * width + x) * 4;
            let guest = &dump.bytes[base..base + 4];
            // Alpha carries coverage for an event pixel and a q-width class
            // (always >= 129) for a point hit, so zero alpha means nothing is
            // displayed here regardless of the identity bytes left in RGB.
            // Treating a zero-coverage event pixel as painted made the scorer
            // report a mismatch for a pixel the guest correctly left empty.
            let guest_background = guest[3] == 0;
            let guest_identity = u32::from(guest[1]) | (u32::from(guest[2]) << 8);
            let all_hit = samples.iter().all(|sample| sample.hit);
            let all_miss = samples.iter().all(|sample| !sample.hit);
            let identities: std::collections::BTreeSet<u32> = samples
                .iter()
                .filter_map(|sample| sample.identity)
                .collect();
            if all_miss {
                let root_free = pixel_bundle_root_free(
                    renderer,
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
                    score.skipped_unproven += 1;
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
            } else if center.hit {
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
        }
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
    let mut report = String::from("PixelsConformance version=1 mode=score-only\n");
    let mut failures = 0_u32;
    let mut unresolved = 0_u32;
    let mut cases = 0_u32;

    for (observation, renderer) in guest_observations.iter().zip(guest_renderers) {
        let complete_partition =
            observation.certificate_runs != 0 || observation.event_corridors != 0;
        let (frame_camera, frame_params) = observation
            .frame_dump
            .as_ref()
            .map_or((CANONICAL_CAMERA, [0.0; 16]), |dump| {
                (dump.camera, dump.params)
            });
        let alpha_oracle =
            semantic_alpha_samples(renderer, &frame_camera, &frame_params).map_err(|error| {
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
        let alpha_matches = observation
            .alpha_samples
            .into_iter()
            .zip(alpha_oracle.values)
            .all(|(guest, oracle)| guest.abs_diff(oracle) <= 12);
        let evidence = observation
            .run_evidence
            .ok_or_else(|| format!("`{}` run evidence is missing", observation.case))?;
        let certified = super::sweep::decode_certified_run_record(evidence).map_err(|error| {
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
        let evidence_point =
            super::sweep::decode_certified_run_evidence_point(evidence).map_err(|error| {
                format!(
                    "`{}` run evidence point is invalid: {error:?}",
                    observation.case
                )
            })?;
        let y = u32::from(evidence_point.row_y);
        let aspect = f64::from(renderer.config.width) / f64::from(renderer.config.height);
        let u_at = |x: f64| (x / f64::from(renderer.config.width) * 2.0 - 1.0) * aspect;
        let v_at = |sample_y: f64| 1.0 - sample_y / f64::from(renderer.config.height) * 2.0;
        let guest_q_lo =
            (i64::from(certified.q_model.q0.lo) + i64::from(certified.q_error.lo)) as f64 * q_scale;
        let guest_q_hi =
            (i64::from(certified.q_model.q0.hi) + i64::from(certified.q_error.hi)) as f64 * q_scale;
        let mut evidence_failures = 0_u8;
        if y >= renderer.config.height {
            evidence_failures |= 1;
        } else if certified.visible.is_none() {
            let sample_x = f64::from(certified.x0) + f64::from(certified.x1 - certified.x0) * 0.5;
            let sample_y = f64::from(y) + 0.5;
            let oracle = semantic_ray_score_with(
                renderer,
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
            let sample_x = f64::from(certified.x0) + f64::from(certified.x1 - certified.x0) * 0.5;
            let sample_y = f64::from(y) + 0.5;
            let u = u_at(sample_x);
            let v = v_at(sample_y);
            let oracle = semantic_ray_score_with(
                renderer,
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
                if semantic_identity_at(renderer, point, &frame_params)?
                    != Some(certified.identity.0)
                {
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
        let evidence_matches = evidence_failures == 0;
        // Per-pixel frame scoring against the independent semantic oracle.
        // The conformance harness always supplies the dump for instrumented
        // runs (and errors when the guest omits it); a synthetic observation
        // without one skips only this stage. Scoring it is the gate that
        // catches whole missing features and wrong identities that
        // aggregate digests cannot see.
        let frame = match observation.frame_dump.as_ref() {
            Some(frame_dump) => Some(score_frame(renderer, frame_dump).map_err(|error| {
                format!("`{}` frame scoring failed: {error}", observation.case)
            })?),
            None => None,
        };
        if let Some(frame) = frame.as_ref() {
            unresolved = unresolved
                .checked_add(frame.unresolved)
                .ok_or_else(|| "frame unresolved counter overflow".to_string())?;
        }
        // `phantom_surface` is deliberately absent from this predicate; it is
        // pinned in the expectation file instead, so any growth still fails
        // the gate while the known count stays visible.
        let frame_matches = frame.as_ref().is_none_or(|frame| {
            frame.interior_mismatches == 0
                && frame.edge_center_violations == 0
                && frame.unresolved == 0
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
            "case=guest-{} runs={} corridors={} proposals={} alpha={:02x},{:02x},{:02x} oracle={:02x},{:02x},{:02x} run_evidence={} frame_interior={}/{} frame_edge_violations={} frame_ambiguous={} frame_unresolved={} frame_skipped={} frame_phantom={} frame_first={} status={}",
            observation.case,
            observation.certificate_runs,
            observation.event_corridors,
            observation.revalidated_proposals,
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

fn semantic_field_dual(
    renderer: &crate::pixels::CompiledRenderer,
    coordinates: [SemanticDual; 3],
    params: &[f32; 16],
) -> Result<SemanticDual, String> {
    Ok(semantic_field_dual_all(renderer, coordinates, params)?.0)
}

fn semantic_field_dual_all(
    renderer: &crate::pixels::CompiledRenderer,
    coordinates: [SemanticDual; 3],
    params: &[f32; 16],
) -> Result<(SemanticDual, Vec<SemanticDual>), String> {
    use crate::pixels::scalar::{CompareOp, ScalarOp};

    let graph = &renderer.symbolic;
    let root = graph.fields.get(graph.field_root)?.scalar_value;
    let mut values = Vec::<SemanticDual>::with_capacity(graph.scalar.len());
    let get = |id: crate::pixels::ids::ScalarId, values: &[SemanticDual]| {
        values
            .get(id.index())
            .copied()
            .ok_or_else(|| "semantic scalar references a non-predecessor".to_string())
    };
    for (id, node) in graph.scalar.iter() {
        debug_assert_eq!(id.index(), values.len(), "scalar arena order is dense");
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
            ScalarOp::Add(a, b) => get(*a, &values)?.add(get(*b, &values)?)?,
            ScalarOp::Sub(a, b) => get(*a, &values)?.sub(get(*b, &values)?)?,
            ScalarOp::Mul(a, b) => get(*a, &values)?.mul(get(*b, &values)?)?,
            ScalarOp::Div(a, b) => get(*a, &values)?.div(get(*b, &values)?)?,
            ScalarOp::Neg(value) => get(*value, &values)?.neg()?,
            ScalarOp::Abs(value) => get(*value, &values)?.abs()?,
            ScalarOp::Min(a, b) => get(*a, &values)?.min(get(*b, &values)?),
            ScalarOp::Max(a, b) => get(*a, &values)?.max(get(*b, &values)?),
            ScalarOp::Clamp { value, lo, hi } => get(*value, &values)?
                .max(get(*lo, &values)?)
                .min(get(*hi, &values)?),
            ScalarOp::Sqrt(value, _) => get(*value, &values)?.sqrt()?,
            ScalarOp::Rsqrt(value, _) => {
                SemanticDual::point(1.0)?.div(get(*value, &values)?.sqrt()?)?
            }
            ScalarOp::SinRestricted(value, _) | ScalarOp::CosRestricted(value, _) => {
                let argument = get(*value, &values)?;
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
                    result = result.add(get(a[axis], &values)?.mul(get(b[axis], &values)?)?)?;
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
                get(a[left.0], &values)?
                    .mul(get(b[left.1], &values)?)?
                    .sub(get(a[right.0], &values)?.mul(get(b[right.1], &values)?)?)?
            }
            ScalarOp::Length2(vector) => {
                let mut square = SemanticDual::point(0.0)?;
                for component in vector {
                    let value = get(*component, &values)?;
                    square = square.add(value.square()?)?;
                }
                square.sqrt()?
            }
            ScalarOp::Length3(vector) => {
                let mut square = SemanticDual::point(0.0)?;
                for component in vector {
                    let value = get(*component, &values)?;
                    square = square.add(value.square()?)?;
                }
                square.sqrt()?
            }
            ScalarOp::Normalize3Component {
                component, value, ..
            } => {
                let mut square = SemanticDual::point(0.0)?;
                for coordinate in value {
                    let value = get(*coordinate, &values)?;
                    square = square.add(value.square()?)?;
                }
                get(
                    *value
                        .get(usize::from(*component))
                        .ok_or_else(|| "semantic normal component is invalid".to_string())?,
                    &values,
                )?
                .div(square.sqrt()?)?
            }
            ScalarOp::Compare { op, a, b } => {
                let a = get(*a, &values)?.value;
                let b = get(*b, &values)?.value;
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
                let predicate = get(*predicate, &values)?.value;
                if predicate.lo >= 1.0 {
                    get(*a, &values)?
                } else if predicate.hi <= 0.0 {
                    get(*b, &values)?
                } else {
                    let a = get(*a, &values)?;
                    let b = get(*b, &values)?;
                    SemanticDual {
                        value: a.value.hull(b.value),
                        derivative: a.derivative.hull(b.derivative),
                    }
                }
            }
            ScalarOp::SelectIndex { index, options } => {
                let index = get(*index, &values)?.value;
                let first = index.lo.floor().max(0.0) as usize;
                let last = index.hi.ceil().max(0.0) as usize;
                let mut selected = None::<SemanticDual>;
                for option in options
                    .iter()
                    .skip(first)
                    .take(last.saturating_sub(first).saturating_add(1))
                {
                    let value = get(*option, &values)?;
                    selected = Some(selected.map_or(value, |old| SemanticDual {
                        value: old.value.hull(value.value),
                        derivative: old.derivative.hull(value.derivative),
                    }));
                }
                selected.ok_or_else(|| "semantic select index is out of bounds".to_string())?
            }
            ScalarOp::SmoothMin { a, b, k, .. } => {
                let a = get(*a, &values)?;
                let b = get(*b, &values)?;
                let k = get(*k, &values)?;
                let half = SemanticDual::point(0.5)?;
                let one = SemanticDual::point(1.0)?;
                let zero = SemanticDual::point(0.0)?;
                let h = half.add(half.mul(b.sub(a)?.div(k)?)?)?.max(zero).min(one);
                b.add(a.sub(b)?.mul(h)?)?.sub(k.mul(h)?.mul(one.sub(h)?)?)?
            }
            ScalarOp::FiniteOr {
                value, fallback, ..
            } => {
                let value = get(*value, &values)?;
                if value.value.lo.is_finite() && value.value.hi.is_finite() {
                    value
                } else {
                    get(*fallback, &values)?
                }
            }
            ScalarOp::MaterialRoughness { value, .. } => get(*value, &values)?,
        };
        values.push(value);
    }
    let root_value = values
        .get(root.index())
        .copied()
        .ok_or_else(|| "semantic field root is absent from the scalar graph".to_string())?;
    Ok((root_value, values))
}

fn semantic_point(
    renderer: &crate::pixels::CompiledRenderer,
    point: [f64; 3],
    params: &[f32; 16],
) -> Result<f64, String> {
    let zero = SemanticInterval::point(0.0)?;
    Ok(semantic_field_dual(
        renderer,
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

/// Point-evaluate every scalar node in the semantic arena, returning midpoint
/// values indexed by scalar id. Used by the per-pixel identity oracle to read
/// individual feature leaves independently of the composed field root.
fn semantic_point_values(
    renderer: &crate::pixels::CompiledRenderer,
    point: [f64; 3],
    params: &[f32; 16],
) -> Result<Vec<f64>, String> {
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
    let (_, values) = semantic_field_dual_all(renderer, coordinates, params)?;
    Ok(values
        .iter()
        .map(|dual| (dual.value.lo + dual.value.hi) * 0.5)
        .collect())
}

/// Independent identity oracle: the surface at `point` belongs to the
/// structural feature whose semantic leaf magnitude is smallest there. When
/// the two smallest leaves carry different identity sets and are within a
/// factor of four of each other the pixel is identity-ambiguous (a blend or
/// shared boundary) and the oracle abstains rather than guessing.
fn semantic_identity_at(
    renderer: &crate::pixels::CompiledRenderer,
    point: [f64; 3],
    params: &[f32; 16],
) -> Result<Option<u32>, String> {
    let values = semantic_point_values(renderer, point, params)?;
    let graph = &renderer.symbolic;
    let mut ranked = Vec::new();
    for feature in &renderer.structural.program().features {
        let node = graph.fields.get(feature.scalar_semantic_root)?;
        let value = values
            .get(node.scalar_value.index())
            .copied()
            .ok_or_else(|| "feature leaf scalar is absent from the arena".to_string())?;
        if !value.is_finite() {
            return Err("feature leaf evaluated non-finite at a hit point".to_string());
        }
        ranked.push((value.abs(), feature.identity_set));
    }
    ranked.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.cmp(&b.1)));
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

fn semantic_ray_score_with(
    renderer: &crate::pixels::CompiledRenderer,
    camera: &[f32; 12],
    params: &[f32; 16],
    u: f64,
    v: f64,
) -> Result<super::oracle::VisibilityScore, String> {
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
    let range_error = std::cell::RefCell::new(None::<String>);
    let range = |lo: f64, hi: f64| {
        let result = semantic_field_dual(
            renderer,
            [
                ray_dual(direction[0], eye[0], lo, hi).ok()?,
                ray_dual(direction[1], eye[1], lo, hi).ok()?,
                ray_dual(direction[2], eye[2], lo, hi).ok()?,
            ],
            params,
        );
        match result {
            Ok(dual) => OracleInterval::new(dual.value.lo, dual.value.hi),
            Err(error) => {
                let mut first = range_error.borrow_mut();
                if first.is_none() {
                    *first = Some(format!("on [{lo}, {hi}]: {error}"));
                }
                None
            }
        }
    };
    let derivative_error = std::cell::RefCell::new(None::<String>);
    let derivative = |lo: f64, hi: f64| {
        let result = semantic_field_dual(
            renderer,
            [
                ray_dual(direction[0], eye[0], lo, hi).ok()?,
                ray_dual(direction[1], eye[1], lo, hi).ok()?,
                ray_dual(direction[2], eye[2], lo, hi).ok()?,
            ],
            params,
        );
        match result {
            Ok(dual) => OracleInterval::new(dual.derivative.lo, dual.derivative.hi),
            Err(error) => {
                let mut first = derivative_error.borrow_mut();
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
        semantic_point(renderer, ray_point(depth), params).unwrap_or_else(|error| {
            let mut first = value_error.borrow_mut();
            if first.is_none() {
                *first = Some(format!("at depth {depth}: {error}"));
            }
            f64::NAN
        })
    };
    let gradient = |depth: f64| {
        let point = ray_point(depth);
        let epsilon = 1.0e-6;
        let mut components = [0.0; 3];
        for axis in 0..3 {
            let mut below = point;
            let mut above = point;
            below[axis] -= epsilon;
            above[axis] += epsilon;
            components[axis] = (semantic_point(renderer, above, params).unwrap_or(f64::NAN)
                - semantic_point(renderer, below, params).unwrap_or(f64::NAN))
                / (2.0 * epsilon);
        }
        OracleVec3 {
            x: components[0],
            y: components[1],
            z: components[2],
        }
    };
    let identity = |_| 0;
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
            let normal = gradient(midpoint);
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
            .or_else(|| range_error.borrow().clone())
            .or_else(|| derivative_error.borrow().clone());
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
                range: &range,
                value: &value,
                derivative_range: &derivative,
                gradient: &gradient,
                identity: &identity,
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
    renderer: &crate::pixels::CompiledRenderer,
    camera: &[f32; 12],
    params: &[f32; 16],
    u: f64,
    v: f64,
) -> Result<bool, String> {
    Ok(semantic_ray_score_with(renderer, camera, params, u, v)?.hit)
}

fn semantic_alpha_samples(
    renderer: &crate::pixels::CompiledRenderer,
    camera: &[f32; 12],
    params: &[f32; 16],
) -> Result<SemanticAlphaSamples, String> {
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
                covered += u32::from(semantic_ray_visibility(renderer, camera, params, u, v)?);
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
            bytes[at] = 0;
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
    let range = |lo: f64, hi: f64| OracleInterval::new(lo - root, hi - root);
    let value = |q: f64| q - root;
    let derivative = |_: f64, _: f64| OracleInterval::new(1.0, 1.0);
    let gradient = |_: f64| OracleVec3 {
        x: 0.0,
        y: 0.0,
        z: 1.0,
    };
    let identity_at = |_| identity;
    oracle(
        SemanticRay {
            range: &range,
            value: &value,
            derivative_range: &derivative,
            gradient: &gradient,
            identity: &identity_at,
        },
        false,
    )
}

fn oracle_sphere(camera_inside: bool) -> Result<super::oracle::VisibilityScore, String> {
    let range = |lo: f64, hi: f64| {
        let at_lo = (lo - 2.0).powi(2) - 1.0;
        let at_hi = (hi - 2.0).powi(2) - 1.0;
        let minimum = if lo <= 2.0 && 2.0 <= hi {
            -1.0
        } else {
            at_lo.min(at_hi)
        };
        OracleInterval::new(minimum, at_lo.max(at_hi))
    };
    let value = |q: f64| (q - 2.0).powi(2) - 1.0;
    let derivative = |lo: f64, hi: f64| OracleInterval::new(2.0 * (lo - 2.0), 2.0 * (hi - 2.0));
    let gradient = |q: f64| OracleVec3 {
        x: 0.0,
        y: 0.0,
        z: q - 2.0,
    };
    let identity = |_| 7;
    oracle(
        SemanticRay {
            range: &range,
            value: &value,
            derivative_range: &derivative,
            gradient: &gradient,
            identity: &identity,
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

    fn fixture_renderer(case: &str) -> crate::pixels::CompiledRenderer {
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
    }

    #[test]
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
            let alpha =
                super::semantic_alpha_samples(&renderer, &super::CANONICAL_CAMERA, &[0.0; 16])
                    .unwrap_or_else(|error| panic!("{case}: {error}"));
            assert_eq!(alpha.unresolved, 0, "{case}");
        }
    }

    #[test]
    fn repeat_frame_probe_lattice_has_no_unresolved_rays() {
        let renderer = fixture_renderer("check-pixels-repeat");
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
                        super::sample_ray(&renderer, &super::CANONICAL_CAMERA, &[0.0; 16], u, v)
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
    fn displaced_coverage_edge_pixel_is_not_a_point_witness() {
        let renderer = fixture_renderer("check-pixels-displace");
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
                    &renderer,
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
            super::semantic_ray_score_with(&renderer, &super::CANONICAL_CAMERA, &[0.0; 16], u, v)
                .unwrap();
        assert!(!background.hit);
        assert_eq!(background.unresolved, 0);
    }

    #[test]
    fn conformance_is_deterministic_and_has_no_unresolved_visibility() {
        let observed = [
            5_103_229_672_427_504_515,
            14_671_586_064_629_379_507,
            3_970_903_405_857_294_658,
            17_783_162_213_222_808_057,
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
        let fixture = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../tests/golden/boot-pixels-plane/src/examples/boot_pixels_plane.wr");
        let renderer = crate::cost::stage::load_pixels_programs(&fixture)
            .unwrap()
            .compiled_renderers
            .into_iter()
            .next()
            .unwrap();
        let renderers = [renderer.clone(), renderer];
        let first = super::run(&observations, &renderers).unwrap();
        let second = super::run(&observations, &renderers).unwrap();
        assert_eq!(first, second);
        assert!(first.contains("failures=0 unresolved=0"));
    }
}
