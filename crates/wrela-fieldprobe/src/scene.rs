//! The two scenes plans/graphics.md §16.2 demands, and their specs.
//!
//! **The specs are written before the geometry, on purpose.** These numbers
//! are only as honest as two hand-authored scenes written by someone who
//! wants the answer to be yes. The mitigation is to fix the budget — op
//! count, blend count, blend width, screen coverage — from what the design
//! actually claims, build to it, and print the realised statistics next to
//! every result so a reader can judge whether the answer was bought.
//!
//! If the hard-surface scene comes back at 3% blend-band fraction, the first
//! question is not "§2.3 is a pillar" but "does this scene have enough
//! blends in it to be a game".
//!
//! # Known optimism, stated up front
//!
//! - Displacement is a sum of sinusoids, not hashed-gradient `fbm`
//!   (`Builder::displace_sin`). Sinusoids are smooth and band-limited, so
//!   they enclose far better under affine arithmetic than real noise would.
//!   Every displaced-surface number here is therefore an upper bound on how
//!   well the real thing behaves.
//! - Both scenes are static. §4's temporal machinery is exercised only by
//!   the camera-pose pair, not by deforming geometry.
//! - No instance BVH: every scene is one monolithic tape, which is the
//!   pessimistic choice for tape length and the optimistic one for culling.

use crate::camera::Camera;
use crate::tape::{Builder, Tape};

pub struct Scene {
    pub name: &'static str,
    pub tape: Tape,
    pub cam: Camera,
    /// Second camera pose, for §4.4's reprojection measurement. Scene B's is
    /// a hard whip; scene A's is a slow dolly.
    pub cam2: Camera,
    pub t_near: f32,
    pub t_far: f32,
    pub spec: &'static str,
}

/// Scene A — "colonnade".
///
/// §16.2 case 1: *hard-surface architecture with smoothed seams, moderate
/// instance count, a static camera path. Tests the optimistic case.*
///
/// Budget, fixed before construction:
/// - 8-column colonnade by domain repetition, period 2.0
/// - every seam smoothed at k ≤ 0.03 (architecture-grade fillets)
/// - one wall with three smoothly-subtracted window openings
/// - a stepped platform, hard unions only
/// - ground displaced 2 octaves, amplitude 0.02 — inside the §6.4 margin
/// - target 200–450 ops, ≥ 8 blend nodes, subject covering ≳ 50% of screen
pub fn colonnade(w: u32, h: u32) -> Scene {
    colonnade_amp(w, h, 0.02, "colonnade")
}

/// The same scene with displacement switched off.
///
/// A control, not a scene. The interior certificate needs `∂f/∂t ≠ 0` over a
/// cell, and on a grazing surface `∂f/∂t` is small — a displacement whose own
/// slope is comparable to it makes the enclosure straddle zero and the
/// certificate is refused. §9.3 calls mid-frequency displacement a *band* to
/// populate; this pair measures what that band costs §2.1. Without a control
/// the rejection rate is a number with two candidate explanations.
pub fn colonnade_flat(w: u32, h: u32) -> Scene {
    colonnade_amp(w, h, 0.0, "colonnade-flat")
}

fn colonnade_amp(w: u32, h: u32, amp: f32, name: &'static str) -> Scene {
    let mut b = Builder::new();
    let p = b.point();

    // Ground: displaced plane.
    let ground = b.plane_y(p, 0.0);
    let ground = if amp > 0.0 { b.displace_sin(ground, p, amp, 1.7, 2, 0.0) } else { ground };

    // Colonnade: repeat along X with period 2.0, eight bays.
    let rx = b.rep(p[0], 2.0);
    let cp = [rx, p[1], p[2]];
    let shaft = b.cylinder_y(cp, 0.22, 1.4);
    let base_p = b.translate(cp, [0.0, -1.4, 0.0]);
    let base = b.round_box(base_p, [0.34, 0.12, 0.34], 0.03);
    let cap_p = b.translate(cp, [0.0, 1.4, 0.0]);
    let cap = b.round_box(cap_p, [0.34, 0.14, 0.34], 0.03);
    let col = b.smin(shaft, base, 0.03);
    let col = b.smin(col, cap, 0.03);
    // Architrave spanning the bays.
    let arch_p = b.translate(cp, [0.0, 1.74, 0.0]);
    let arch = b.round_box(arch_p, [1.05, 0.16, 0.30], 0.02);
    let col = b.smin(col, arch, 0.025);
    // Clip the repetition to eight bays with a slab.
    let bay_slab = b.boxd(p, [8.0, 4.0, 0.45]);
    let colonnade = b.max(col, bay_slab);

    // Back wall with three windows.
    let wall_p = b.translate(p, [0.0, 1.6, -3.2]);
    let wall = b.round_box(wall_p, [5.0, 1.7, 0.22], 0.02);
    let win_x = b.rep(wall_p[0], 2.4);
    let win_p = [win_x, wall_p[1], wall_p[2]];
    let win = b.round_box(win_p, [0.55, 0.9, 0.5], 0.05);
    let wall = b.ssubtract(wall, win, 0.02);

    // Stepped platform: hard unions, no fillet.
    let s0 = b.translate(p, [0.0, 0.06, 1.6]);
    let s0 = b.boxd(s0, [4.0, 0.06, 0.5]);
    let s1 = b.translate(p, [0.0, 0.18, 2.1]);
    let s1 = b.boxd(s1, [4.0, 0.06, 0.5]);
    let s2 = b.translate(p, [0.0, 0.30, 2.6]);
    let s2 = b.boxd(s2, [4.0, 0.06, 0.5]);
    let steps = b.union(s0, s1);
    let steps = b.union(steps, s2);

    let world = b.union(ground, colonnade);
    let world = b.union(world, wall);
    let world = b.union(world, steps);
    let tape = b.finish(world);

    Scene {
        name,
        tape,
        cam: Camera::look_at([1.1, 1.55, 5.4], [0.0, 1.15, 0.0], 55.0, w, h),
        // Slow dolly: the optimistic temporal case.
        cam2: Camera::look_at([1.16, 1.55, 5.31], [0.02, 1.15, 0.0], 55.0, w, h),
        t_near: 0.05,
        t_far: 24.0,
        spec: if amp > 0.0 {
            "8-bay colonnade, filleted seams k<=0.03, 3 subtracted windows, \
             2-octave ground displacement, static camera"
        } else {
            "same colonnade, ground displacement OFF — the control for whether \
             displacement is what refuses the interior certificate"
        },
    }
}

/// Scene B — "melee".
///
/// §16.2 case 2: *a `smin` character cluster mid-swing, camera whipping.
/// This one scene stresses three assumptions simultaneously — `smin`
/// clusters do not prune, blend bands force marching, and a whipping camera
/// is reprojection's weakest case. This is the scene that will tell the
/// truth.*
///
/// Budget, fixed before construction:
/// - 4 figures within a 2.5-unit cluster, limbs blended at k = 0.08
/// - bodies unioned to each other hard (separate solids, per §10.1)
/// - one swung blade plus its §10.3 swept-volume torus segment, k ≈ 0.06
/// - target 400–700 ops, ≥ 24 blend nodes
/// - camera close enough that figures fill the frame; second pose is a 14°
///   whip, which is ~2.5× a fast human flick at 30 Hz
pub fn melee(w: u32, h: u32) -> Scene {
    let mut b = Builder::new();
    let p = b.point();

    let ground = b.plane_y(p, 0.0);

    let mut bodies: Vec<u32> = Vec::new();
    // (x, z, facing, phase) — clustered, overlapping silhouettes.
    let figs = [
        (0.0f32, 0.0f32, 0.35f32, 0.0f32),
        (0.85, -0.55, -0.9, 1.1),
        (-0.75, -0.35, 1.4, 2.2),
        (0.15, -1.25, 2.6, 3.0),
    ];
    for (i, &(fx, fz, face, ph)) in figs.iter().enumerate() {
        let fp = b.translate(p, [fx, 0.0, fz]);
        let fp = b.rot_y(fp, face);

        let torso_p = b.translate(fp, [0.0, 1.05, 0.0]);
        let torso = b.capsule_y(torso_p, 0.26, 0.19);

        let head_p = b.translate(fp, [0.0, 1.52, 0.0]);
        let head = b.sphere(head_p, 0.15);

        // Arms swing out of phase, so the blend bands overlap differently
        // per figure — the point of the cluster.
        let arm_l_p = b.translate(fp, [-0.28, 1.18, 0.0]);
        let arm_l_p = b.rot_z(arm_l_p, 0.5 + 0.35 * ph.sin());
        let arm_l = b.capsule_y(arm_l_p, 0.24, 0.075);

        let arm_r_p = b.translate(fp, [0.28, 1.18, 0.0]);
        let arm_r_p = b.rot_z(arm_r_p, -0.5 - 0.4 * (ph * 1.3).sin());
        let arm_r = b.capsule_y(arm_r_p, 0.24, 0.075);

        let leg_l_p = b.translate(fp, [-0.11, 0.42, 0.0]);
        let leg_l_p = b.rot_z(leg_l_p, 0.12 * ph.cos());
        let leg_l = b.capsule_y(leg_l_p, 0.34, 0.088);

        let leg_r_p = b.translate(fp, [0.11, 0.42, 0.0]);
        let leg_r_p = b.rot_z(leg_r_p, -0.12 * ph.cos());
        let leg_r = b.capsule_y(leg_r_p, 0.34, 0.088);

        // Generous limb blends: this is what §16.2 says will not prune.
        let body = b.smin(torso, head, 0.08);
        let body = b.smin(body, arm_l, 0.08);
        let body = b.smin(body, arm_r, 0.08);
        let body = b.smin(body, leg_l, 0.08);
        let body = b.smin(body, leg_r, 0.08);

        let body = if i == 0 {
            // The swing: blade, plus the §10.3 swept volume as a torus
            // segment — the smear frame, one primitive.
            let hand_p = b.translate(fp, [0.42, 1.25, 0.10]);
            let hand_p = b.rot_z(hand_p, -1.15);
            let blade = b.round_box(hand_p, [0.035, 0.46, 0.011], 0.008);
            let swept_p = b.translate(fp, [0.30, 1.22, 0.0]);
            let swept_p = b.rot_z(swept_p, -0.55);
            let swept = b.torus(swept_p, 0.44, 0.020);
            let arc = b.smin(blade, swept, 0.06);
            b.smin(body, arc, 0.05)
        } else {
            body
        };
        bodies.push(body);
    }

    // Bodies do not blend into each other — separate solids (§10.1).
    let mut cluster = bodies[0];
    for &x in &bodies[1..] {
        cluster = b.union(cluster, x);
    }

    let world = b.union(ground, cluster);
    let tape = b.finish(world);

    Scene {
        name: "melee",
        tape,
        cam: Camera::look_at([1.55, 1.35, 2.35], [0.0, 1.05, -0.35], 60.0, w, h),
        // 14° whip about the subject in one 30 Hz frame.
        cam2: Camera::look_at([2.02, 1.38, 1.98], [0.0, 1.05, -0.35], 60.0, w, h),
        t_near: 0.05,
        t_far: 18.0,
        spec: "4-figure smin cluster (k=0.08 limbs), swung blade + swept-volume \
               torus (k=0.05-0.06), bodies hard-unioned, 14deg camera whip",
    }
}
