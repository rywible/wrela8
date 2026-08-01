use crate::atlas::Aabb;
use crate::camera::Camera;
use crate::tape::{Builder, Tape};

fn whip_path(base: &Camera, target: [f32; 3], n: usize, peak_deg: f32) -> Vec<Camera> {
    let r = {
        let d = [
            base.eye[0] - target[0],
            base.eye[1] - target[1],
            base.eye[2] - target[2],
        ];
        (d[0] * d[0] + d[2] * d[2]).sqrt().max(1e-3)
    };
    let a0 = (base.eye[2] - target[2]).atan2(base.eye[0] - target[0]);
    let mut out = Vec::with_capacity(n);
    let mut ang = 0.0f32;
    for i in 0..n {
        let u = i as f32 / (n - 1).max(1) as f32;
        let rate = peak_deg * (1.0 - (std::f32::consts::TAU * u).cos()) * 0.5;
        ang += rate.to_radians();
        let a = a0 + ang;
        let eye = [
            target[0] + r * a.cos(),
            base.eye[1],
            target[2] + r * a.sin(),
        ];
        out.push(Camera::look_at(eye, target, base.fov_deg, base.w, base.h));
    }
    out
}

pub struct Scene {
    pub name: &'static str,
    pub tape: Tape,
    pub cam: Camera,
    pub cam2: Camera,
    pub t_near: f32,
    pub t_far: f32,
    pub spec: &'static str,
    pub bounds: Aabb,
    pub path: Vec<Camera>,
}

pub fn colonnade(w: u32, h: u32) -> Scene {
    colonnade_amp(w, h, 0.02, "colonnade")
}

pub fn colonnade_flat(w: u32, h: u32) -> Scene {
    colonnade_amp(w, h, 0.0, "colonnade-flat")
}

fn colonnade_amp(w: u32, h: u32, amp: f32, name: &'static str) -> Scene {
    let mut b = Builder::new();
    let p = b.point();

    let ground = b.plane_y(p, 0.0);
    let ground = if amp > 0.0 {
        b.displace_sin(ground, p, amp, 1.7, 2, 0.0)
    } else {
        ground
    };

    let rx = b.rep(p[0], 2.0);
    let cp = [rx, p[1], p[2]];
    let shaft = b.cylinder_y(cp, 0.22, 1.4);
    let base_p = b.translate(cp, [0.0, -1.4, 0.0]);
    let base = b.round_box(base_p, [0.34, 0.12, 0.34], 0.03);
    let cap_p = b.translate(cp, [0.0, 1.4, 0.0]);
    let cap = b.round_box(cap_p, [0.34, 0.14, 0.34], 0.03);
    let col = b.smin(shaft, base, 0.03);
    let col = b.smin(col, cap, 0.03);
    let arch_p = b.translate(cp, [0.0, 1.74, 0.0]);
    let arch = b.round_box(arch_p, [1.05, 0.16, 0.30], 0.02);
    let col = b.smin(col, arch, 0.025);
    let bay_slab = b.boxd(p, [8.0, 4.0, 0.45]);
    let colonnade = b.max(col, bay_slab);

    let wall_p = b.translate(p, [0.0, 1.6, -3.2]);
    let wall = b.round_box(wall_p, [5.0, 1.7, 0.22], 0.02);
    let win_x = b.rep(wall_p[0], 2.4);
    let win_p = [win_x, wall_p[1], wall_p[2]];
    let win = b.round_box(win_p, [0.55, 0.9, 0.5], 0.05);
    let wall = b.ssubtract(wall, win, 0.02);

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

    let cam = Camera::look_at([1.1, 1.55, 5.4], [0.0, 1.15, 0.0], 55.0, w, h);
    Scene {
        name,
        tape,
        cam,
        cam2: Camera::look_at([1.16, 1.55, 5.31], [0.02, 1.15, 0.0], 55.0, w, h),
        bounds: Aabb {
            lo: [-26.0, -25.0, -26.0],
            hi: [26.0, 27.0, 26.0],
        },
        path: whip_path(&cam, [0.0, 1.15, 0.0], 16, 9.0),
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

pub fn melee(w: u32, h: u32) -> Scene {
    melee_at(w, h, 0.35)
}

pub fn melee_at(w: u32, h: u32, swing: f32) -> Scene {
    let s_curve = 0.5 - 0.5 * (std::f32::consts::PI * swing).cos();
    melee_build(w, h, s_curve)
}

fn melee_build(w: u32, h: u32, sw: f32) -> Scene {
    let mut b = Builder::new();
    let p = b.point();

    let ground = b.plane_y(p, 0.0);

    let mut bodies: Vec<u32> = Vec::new();
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

        let arm_l_p = b.translate(fp, [-0.28, 1.18, 0.0]);
        let arm_l_p = b.rot_z(arm_l_p, 0.5 + 0.35 * ph.sin());
        let arm_l = b.capsule_y(arm_l_p, 0.24, 0.075);

        let swing_ang = if i == 0 {
            -0.35 - 1.75 * sw
        } else {
            -0.5 - 0.4 * (ph * 1.3 + sw * 0.6).sin()
        };
        let arm_r_p = b.translate(fp, [0.28, 1.18, 0.0]);
        let arm_r_p = b.rot_z(arm_r_p, swing_ang);
        let arm_r = b.capsule_y(arm_r_p, 0.24, 0.075);

        let leg_l_p = b.translate(fp, [-0.11, 0.42, 0.0]);
        let leg_l_p = b.rot_z(leg_l_p, 0.12 * ph.cos());
        let leg_l = b.capsule_y(leg_l_p, 0.34, 0.088);

        let leg_r_p = b.translate(fp, [0.11, 0.42, 0.0]);
        let leg_r_p = b.rot_z(leg_r_p, -0.12 * ph.cos());
        let leg_r = b.capsule_y(leg_r_p, 0.34, 0.088);

        let body = b.smin(torso, head, 0.08);
        let body = b.smin(body, arm_l, 0.08);
        let body = b.smin(body, arm_r, 0.08);
        let body = b.smin(body, leg_l, 0.08);
        let body = b.smin(body, leg_r, 0.08);

        let body = if i == 0 {
            let hand_p = b.translate(fp, [0.42, 1.25, 0.10]);
            let hand_p = b.rot_z(hand_p, swing_ang - 0.8);
            let blade = b.round_box(hand_p, [0.035, 0.46, 0.011], 0.008);
            let rate = (std::f32::consts::PI * sw).sin();
            let swept_p = b.translate(fp, [0.30, 1.22, 0.0]);
            let swept_p = b.rot_z(swept_p, swing_ang * 0.5);
            let swept = b.torus(swept_p, 0.44, 0.020 + 0.03 * rate);
            let arc = b.smin(blade, swept, 0.06);
            b.smin(body, arc, 0.05)
        } else {
            body
        };
        bodies.push(body);
    }

    let mut cluster = bodies[0];
    for &x in &bodies[1..] {
        cluster = b.union(cluster, x);
    }

    let world = b.union(ground, cluster);
    let tape = b.finish(world);

    let cam = Camera::look_at([1.55, 1.35, 2.35], [0.0, 1.05, -0.35], 60.0, w, h);
    Scene {
        name: "melee",
        tape,
        cam,
        cam2: Camera::look_at([2.02, 1.38, 1.98], [0.0, 1.05, -0.35], 60.0, w, h),
        bounds: Aabb {
            lo: [-19.0, -18.0, -19.0],
            hi: [19.0, 20.0, 19.0],
        },
        path: whip_path(&cam, [0.0, 1.05, -0.35], 16, 16.0),
        t_near: 0.05,
        t_far: 18.0,
        spec: "4-figure smin cluster (k=0.08 limbs), swung blade + swept-volume \
               torus (k=0.05-0.06), bodies hard-unioned, 14deg camera whip",
    }
}
