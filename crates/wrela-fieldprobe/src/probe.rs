use crate::aff::{Aff, Iv};
use crate::camera::Camera;
use crate::eval::{DAff, eval, eval_aff, eval_blend_active, eval_daff, eval_grad, eval_iv};
use crate::prune::{Pruned, prune};
use crate::scene::Scene;
use crate::tape::Tape;

pub struct Rng(pub u32);

impl Rng {
    #[inline]
    pub fn next_u32(&mut self) -> u32 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        self.0 = x;
        x
    }
    #[inline]
    pub fn unit(&mut self) -> f32 {
        (self.next_u32() >> 8) as f32 / 16_777_216.0
    }
    #[inline]
    pub fn range(&mut self, lo: f32, hi: f32) -> f32 {
        lo + (hi - lo) * self.unit()
    }
}

#[derive(Default)]
pub struct Scratch {
    pub f: Vec<f32>,
    pub g: Vec<(f32, [f32; 3])>,
    pub a: Vec<Aff>,
    pub av: Vec<Iv>,
    pub d: Vec<DAff>,
    pub i: Vec<Iv>,
}

#[derive(Default, Clone)]
pub struct DepthStat {
    pub cells: u64,
    pub ops_sum: u64,
    pub weight_sum: u64,
    pub blends_sum: u64,
    pub ops_max: usize,
    pub ops_hist: Vec<u32>,
}

#[derive(Default)]
pub struct ClassifyOut {
    pub per_depth: Vec<DepthStat>,
    pub area_exterior: f64,
    pub area_interior: f64,
    pub area_unresolved: f64,
    pub area_edge: f64,
    pub edge_cells: u64,
    pub leaf_px: f32,
    pub nonfinite: u64,
    pub interior_cells: Vec<InteriorCell>,
    pub fail_dt_straddles: u64,
    pub fail_faces: u64,
    pub area_interior_empirical: f64,
    pub leaves: Vec<Leaf>,
    pub traversal_ops: u64,
}

#[derive(Clone, Copy, PartialEq)]
pub enum Class {
    Interior,
    Edge,
    Unresolved,
}

pub struct Leaf {
    pub x0: f32,
    pub y0: f32,
    pub size: f32,
    pub class: Class,
    pub tape: Tape,
    pub tape_wide: Tape,
    pub t_far: f32,
    pub t0: f32,
    pub t1: f32,
}

#[derive(Clone, Copy)]
pub struct InteriorCell {
    pub x0: f32,
    pub y0: f32,
    pub size: f32,
    pub t0: f32,
    pub t1: f32,
}

const NSLAB: u32 = 20;
const BISECT: u32 = 7;

#[allow(clippy::too_many_arguments)]
fn classify(
    cam: &Camera,
    full_len: usize,
    tape: &Tape,
    x0: f32,
    y0: f32,
    size: f32,
    t_lo: f32,
    t_hi: f32,
    depth: u32,
    max_depth: u32,
    s: &mut Scratch,
    out: &mut ClassifyOut,
) {
    let vx = (x0 + size).min(cam.w as f32) - x0;
    let vy = (y0 + size).min(cam.h as f32) - y0;
    if vx <= 0.0 || vy <= 0.0 {
        return;
    }
    let area = (vx * vy) as f64;
    let (u0, u1) = (cam.u_of(x0), cam.u_of(x0 + size));
    let (v0, v1) = (cam.v_of(y0 + size), cam.v_of(y0));

    let mut ta = t_lo;
    let mut tb = t_hi;
    let mut found = false;
    let ratio = (t_hi / t_lo).powf(1.0 / NSLAB as f32);
    let mut s0 = t_lo;
    for _ in 0..NSLAB {
        let s1 = (s0 * ratio).min(t_hi);
        let p = cam.wedge(u0, u1, v0, v1, s0, s1);
        eval_aff(tape, p, &mut s.a, &mut s.av);
        let r = s.av[tape.root as usize];
        if !(r.lo.is_finite() && r.hi.is_finite()) {
            out.nonfinite += 1;
            out.area_unresolved += area;
            return;
        }
        if r.lo <= 0.0 {
            ta = s0;
            tb = s1;
            found = true;
            break;
        }
        s0 = s1;
        if s0 >= t_hi {
            break;
        }
    }
    if !found {
        out.area_exterior += area;
        return;
    }

    let p = cam.wedge(u0, u1, v0, v1, ta, t_hi);
    eval_aff(tape, p, &mut s.a, &mut s.av);
    let pr_wide: Pruned = prune(tape, &s.av);
    let pc = cam.wedge(u0, u1, v0, v1, ta, tb);
    eval_aff(&pr_wide.tape, pc, &mut s.a, &mut s.av);
    let pr: Pruned = prune(&pr_wide.tape, &s.av);

    out.traversal_ops += tape.weight() as u64;
    let d = depth as usize;
    if out.per_depth.len() <= d {
        out.per_depth.resize(d + 1, DepthStat::default());
    }
    let st = &mut out.per_depth[d];
    st.cells += 1;
    st.ops_sum += pr.ops as u64;
    st.weight_sum += pr.weight as u64;
    st.blends_sum += pr.blends as u64;
    st.ops_max = st.ops_max.max(pr.ops);
    st.ops_hist.push(pr.ops as u32);
    let _ = full_len;

    let mut lo = ta;
    let mut hi = tb;
    let mut bounds: Vec<Aff> = Vec::new();
    let mut biv: Vec<Iv> = Vec::new();

    for _ in 0..BISECT {
        let wp = cam.wedge(u0, u1, v0, v1, lo, hi);
        eval_aff(&pr.tape, wp, &mut bounds, &mut biv);
        let dp = cam.wedge_daff(u0, u1, v0, v1, lo, hi);
        let r = eval_daff(&pr.tape, dp, &biv, &mut s.d);
        let (dlo, dhi) = r.dt.interval();
        if !r.dt.is_finite() || (dlo <= 0.0 && dhi >= 0.0) {
            if depth == max_depth {
                out.fail_dt_straddles += 1;
            }
            break;
        }
        let near = cam.slice(u0, u1, v0, v1, lo);
        eval_aff(&pr.tape, near, &mut s.a, &mut s.av);
        let nf = s.av[pr.tape.root as usize];
        let far = cam.slice(u0, u1, v0, v1, hi);
        eval_aff(&pr.tape, far, &mut s.a, &mut s.av);
        let ff = s.av[pr.tape.root as usize];
        let certified = if dhi < 0.0 {
            nf.lo > 0.0 && ff.hi < 0.0
        } else {
            nf.hi < 0.0 && ff.lo > 0.0
        };
        out.traversal_ops += 4 * pr.tape.weight() as u64;
        if certified {
            out.area_interior += area;
            out.area_interior_empirical += area;
            out.interior_cells.push(InteriorCell {
                x0,
                y0,
                size,
                t0: lo,
                t1: hi,
            });
            out.leaves.push(Leaf {
                x0,
                y0,
                size,
                class: Class::Interior,
                tape: pr.tape.clone(),
                tape_wide: pr_wide.tape.clone(),
                t_far: t_hi,
                t0: ta,
                t1: tb,
            });
            return;
        }
        let mid = 0.5 * (lo + hi);
        let m = cam.slice(u0, u1, v0, v1, mid);
        eval_aff(&pr.tape, m, &mut s.a, &mut s.av);
        let mf = s.av[pr.tape.root as usize];
        if mf.lo <= 0.0 && mf.hi >= 0.0 {
            if depth == max_depth {
                out.fail_faces += 1;
            }
            break;
        }
        let near_outside = if dhi < 0.0 { mf.lo > 0.0 } else { mf.hi < 0.0 };
        if near_outside {
            lo = mid;
        } else {
            hi = mid;
        }
    }

    if depth < max_depth {
        let hs = size * 0.5;
        for (dx, dy) in [(0.0, 0.0), (hs, 0.0), (0.0, hs), (hs, hs)] {
            classify(
                cam,
                full_len,
                &pr_wide.tape,
                x0 + dx,
                y0 + dy,
                hs,
                ta,
                t_hi,
                depth + 1,
                max_depth,
                s,
                out,
            );
        }
        return;
    }

    let wp = cam.wedge(u0, u1, v0, v1, ta, tb);
    let mut lb: Vec<Aff> = Vec::new();
    let mut lbi: Vec<Iv> = Vec::new();

    eval_aff(&pr.tape, wp, &mut lb, &mut lbi);
    let dp = cam.wedge_daff(u0, u1, v0, v1, ta, tb);
    let r = eval_daff(&pr.tape, dp, &lbi, &mut s.d);
    let (dlo, dhi) = r.dt.interval();
    let (vlo, vhi) = (lbi[pr.tape.root as usize].lo, lbi[pr.tape.root as usize].hi);
    let class = if vlo <= 0.0 && vhi >= 0.0 && dlo <= 0.0 && dhi >= 0.0 {
        out.area_edge += area;
        out.edge_cells += 1;
        Class::Edge
    } else {
        out.area_unresolved += area;
        Class::Unresolved
    };
    if empirically_smooth(cam, &pr.tape, x0, y0, size, ta, tb, s) {
        out.area_interior_empirical += area;
    }
    out.leaves.push(Leaf {
        x0,
        y0,
        size,
        class,
        tape: pr.tape.clone(),
        tape_wide: pr_wide.tape.clone(),
        t_far: t_hi,
        t0: ta,
        t1: tb,
    });
    out.leaf_px = size;
}

#[allow(clippy::too_many_arguments)]
fn empirically_smooth(
    cam: &Camera,
    tape: &Tape,
    x0: f32,
    y0: f32,
    size: f32,
    t0: f32,
    t1: f32,
    s: &mut Scratch,
) -> bool {
    let lo = (t0 * 0.5).max(1e-3);
    let hi = t1 * 2.0;
    let mut c = [0.0f32; 4];
    for (i, &(dx, dy)) in [(0.0, 0.0), (1.0, 0.0), (0.0, 1.0), (1.0, 1.0)]
        .iter()
        .enumerate()
    {
        let d = cam.dir_at_pixel(x0 + dx * size, y0 + dy * size);
        match march(tape, cam.eye, d, lo, hi, s).0 {
            Some(t) => c[i] = t,
            None => return false,
        }
    }
    let d = cam.dir_at_pixel(x0 + 0.5 * size, y0 + 0.5 * size);
    let mid = match march(tape, cam.eye, d, lo, hi, s).0 {
        Some(t) => t,
        None => return false,
    };
    let pred = 0.25 * (c[0] + c[1] + c[2] + c[3]);
    let foot = mid * 2.0 * cam.tan_half / cam.h as f32;
    (pred - mid).abs() <= 0.5 * foot
}

pub fn run_classify(sc: &Scene, max_depth: u32, base_tile: f32, s: &mut Scratch) -> ClassifyOut {
    let mut out = ClassifyOut::default();
    let full_len = sc.tape.len();
    let nx = (sc.cam.w as f32 / base_tile).ceil() as u32;
    let ny = (sc.cam.h as f32 / base_tile).ceil() as u32;
    for ty in 0..ny {
        for tx in 0..nx {
            classify(
                &sc.cam,
                full_len,
                &sc.tape,
                tx as f32 * base_tile,
                ty as f32 * base_tile,
                base_tile,
                sc.t_near,
                sc.t_far,
                0,
                max_depth,
                s,
                &mut out,
            );
        }
    }
    out
}

pub struct MarchOut {
    pub rays: u64,
    pub hits: u64,
    pub evals: u64,
    pub steps_max: u32,
    pub band_len: f64,
    pub total_len: f64,
    pub band_len_hit: f64,
    pub total_len_hit: f64,
    pub band_samples: u64,
    pub band_active_samples: u64,
}

const HIT_EPS: f32 = 1e-4;
const MAX_STEPS: u32 = 192;

const OVER_RELAX: f32 = 1.0;

pub static STEP_CAP_HITS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

pub fn march(
    tape: &Tape,
    o: [f32; 3],
    d: [f32; 3],
    t_near: f32,
    t_far: f32,
    s: &mut Scratch,
) -> (Option<f32>, u32) {
    let mut t = t_near;
    let mut steps = 0;
    let mut prev = 0.0f32;
    let mut relax = OVER_RELAX;
    while t < t_far && steps < MAX_STEPS {
        let p = [o[0] + t * d[0], o[1] + t * d[1], o[2] + t * d[2]];
        let dist = eval(tape, p, &mut s.f);
        steps += 1;
        if dist < HIT_EPS * t.max(1.0) {
            return (Some(t), steps);
        }
        let step = if relax > 1.0 && dist + prev < relax * prev {
            t -= (relax - 1.0) * prev;
            relax = 1.0;
            dist
        } else {
            dist * relax
        };
        prev = dist;
        t += step.max(HIT_EPS);
    }
    if steps >= MAX_STEPS {
        STEP_CAP_HITS.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    }
    (None, steps)
}

pub fn run_march(sc: &Scene, stride: u32, band_samples: u32, s: &mut Scratch) -> MarchOut {
    let mut o = MarchOut {
        rays: 0,
        hits: 0,
        evals: 0,
        steps_max: 0,
        band_len: 0.0,
        total_len: 0.0,
        band_len_hit: 0.0,
        total_len_hit: 0.0,
        band_samples: 0,
        band_active_samples: 0,
    };
    let cam = &sc.cam;
    let mut y = 0;
    while y < cam.h {
        let mut x = 0;
        while x < cam.w {
            let dir = cam.dir_at_pixel(x as f32 + 0.5, y as f32 + 0.5);
            let (hit, steps) = march(&sc.tape, cam.eye, dir, sc.t_near, sc.t_far, s);
            o.rays += 1;
            o.evals += steps as u64;
            o.steps_max = o.steps_max.max(steps);
            let end = hit.unwrap_or(sc.t_far);
            if hit.is_some() {
                o.hits += 1;
            }
            let seg = end - sc.t_near;
            let mut band = 0.0f64;
            for i in 0..band_samples {
                let f = (i as f32 + 0.5) / band_samples as f32;
                let t = sc.t_near + f * seg;
                let p = [
                    cam.eye[0] + t * dir[0],
                    cam.eye[1] + t * dir[1],
                    cam.eye[2] + t * dir[2],
                ];
                let (_, n) = eval_blend_active(&sc.tape, p, &mut s.f);
                o.band_samples += 1;
                if n > 0 {
                    o.band_active_samples += 1;
                    band += (seg / band_samples as f32) as f64;
                }
            }
            o.total_len += seg as f64;
            o.band_len += band;
            if hit.is_some() {
                o.total_len_hit += seg as f64;
                o.band_len_hit += band;
            }
            x += stride;
        }
        y += stride;
    }
    o
}

pub struct TightOut {
    pub cells: u64,
    pub aa_empty: u64,
    pub iv_empty: u64,
    pub width_ratio_sum: f64,
    pub width_ratio_n: u64,
}

pub fn run_tightness(sc: &Scene, tile: f32, s: &mut Scratch) -> TightOut {
    let mut o = TightOut {
        cells: 0,
        aa_empty: 0,
        iv_empty: 0,
        width_ratio_sum: 0.0,
        width_ratio_n: 0,
    };
    let cam = &sc.cam;
    let nx = (cam.w as f32 / tile).ceil() as u32;
    let ny = (cam.h as f32 / tile).ceil() as u32;
    let ratio = (sc.t_far / sc.t_near).powf(1.0 / NSLAB as f32);
    for ty in 0..ny {
        for tx in 0..nx {
            let x0 = tx as f32 * tile;
            let y0 = ty as f32 * tile;
            let (u0, u1) = (cam.u_of(x0), cam.u_of(x0 + tile));
            let (v0, v1) = (cam.v_of(y0 + tile), cam.v_of(y0));
            let mut t0 = sc.t_near;
            for _ in 0..NSLAB {
                let t1 = (t0 * ratio).min(sc.t_far);
                let p = cam.wedge(u0, u1, v0, v1, t0, t1);
                eval_aff(&sc.tape, p, &mut s.a, &mut s.av);
                let a = s.a[sc.tape.root as usize];
                let bx = [
                    Iv::new(p[0].lo(), p[0].hi()),
                    Iv::new(p[1].lo(), p[1].hi()),
                    Iv::new(p[2].lo(), p[2].hi()),
                ];
                let iv = eval_iv(&sc.tape, bx, &mut s.i);
                o.cells += 1;
                if a.lo() > 0.0 {
                    o.aa_empty += 1;
                }
                if iv.lo > 0.0 {
                    o.iv_empty += 1;
                }
                let aw = (a.hi() - a.lo()) as f64;
                let iw = iv.width() as f64;
                if aw > 1e-9 && iw.is_finite() && iw > 1e-9 {
                    o.width_ratio_sum += iw / aw;
                    o.width_ratio_n += 1;
                }
                t0 = t1;
                if t0 >= sc.t_far {
                    break;
                }
            }
        }
    }
    o
}

pub struct ContinuationOut {
    pub samples: u64,
    pub converged: u64,
    pub diverged: u64,
    pub cont_evals: f64,
    pub march_evals: f64,
    pub max_err_px: f32,
}

const GRAD_COST: f64 = 3.0;

fn dhat_du(cam: &Camera, u: f32, v: f32) -> ([f32; 3], [f32; 3], f32) {
    let raw = [
        cam.fwd[0] + u * cam.right[0] + v * cam.up[0],
        cam.fwd[1] + u * cam.right[1] + v * cam.up[1],
        cam.fwd[2] + u * cam.right[2] + v * cam.up[2],
    ];
    let l = (raw[0] * raw[0] + raw[1] * raw[1] + raw[2] * raw[2]).sqrt();
    let dh = [raw[0] / l, raw[1] / l, raw[2] / l];
    let dot = dh[0] * cam.right[0] + dh[1] * cam.right[1] + dh[2] * cam.right[2];
    let ddu = [
        (cam.right[0] - dh[0] * dot) / l,
        (cam.right[1] - dh[1] * dot) / l,
        (cam.right[2] - dh[2] * dot) / l,
    ];
    (dh, ddu, l)
}

pub fn run_continuation(sc: &Scene, cells: &[InteriorCell], s: &mut Scratch) -> ContinuationOut {
    let cam = &sc.cam;
    let mut o = ContinuationOut {
        samples: 0,
        converged: 0,
        diverged: 0,
        cont_evals: 0.0,
        march_evals: 0.0,
        max_err_px: 0.0,
    };
    for c in cells {
        let n = c.size.max(1.0) as u32;
        let v = cam.v_of(c.y0 + 0.5 * c.size);
        let u_seed = cam.u_of(c.x0 + 0.5);
        let d_seed = cam.dir(u_seed, v);
        let (hit, steps) = match march(&sc.tape, cam.eye, d_seed, c.t0, c.t1 * 1.05, s) {
            (Some(t), st) => (t, st),
            (None, st) => {
                o.diverged += 1;
                o.cont_evals += st as f64;
                continue;
            }
        };
        o.cont_evals += steps as f64;
        let mut t = hit;

        for i in 1..n {
            let px = c.x0 + i as f32 + 0.5;
            let u_prev = cam.u_of(px - 1.0);
            let u = cam.u_of(px);
            let du = u - u_prev;

            let (dh, ddu, _) = dhat_du(cam, u_prev, v);
            let p = [
                cam.eye[0] + t * dh[0],
                cam.eye[1] + t * dh[1],
                cam.eye[2] + t * dh[2],
            ];
            let (_, g) = eval_grad(&sc.tape, p, &mut s.g);
            o.cont_evals += GRAD_COST;
            let ft = g[0] * dh[0] + g[1] * dh[1] + g[2] * dh[2];
            let fu = t * (g[0] * ddu[0] + g[1] * ddu[1] + g[2] * ddu[2]);
            if ft.abs() < 1e-6 {
                o.diverged += 1;
                continue;
            }
            let mut tn = t - (fu / ft) * du;

            let (dh2, _, _) = dhat_du(cam, u, v);
            let mut ok = false;
            for _ in 0..2 {
                let q = [
                    cam.eye[0] + tn * dh2[0],
                    cam.eye[1] + tn * dh2[1],
                    cam.eye[2] + tn * dh2[2],
                ];
                let (f, g2) = eval_grad(&sc.tape, q, &mut s.g);
                o.cont_evals += GRAD_COST;
                let ft2 = g2[0] * dh2[0] + g2[1] * dh2[1] + g2[2] * dh2[2];
                if ft2.abs() < 1e-6 {
                    break;
                }
                tn -= f / ft2;
                if f.abs() < HIT_EPS * tn.max(1.0) {
                    ok = true;
                    break;
                }
            }

            let (truth, msteps) = march(&sc.tape, cam.eye, dh2, sc.t_near, sc.t_far, s);
            o.march_evals += msteps as f64;
            o.samples += 1;
            match truth {
                Some(tt) if ok && (tn - tt).abs() < 2.0 * HIT_EPS * tt.max(1.0) + 1e-3 => {
                    o.converged += 1;
                    let foot = tt * 2.0 * cam.tan_half / cam.h as f32;
                    let err = (tn - tt).abs() / foot.max(1e-9);
                    o.max_err_px = o.max_err_px.max(err);
                    t = tn;
                }
                _ => {
                    o.diverged += 1;
                    if let Some(tt) = truth {
                        t = tt;
                    }
                }
            }
        }
    }
    o
}

pub struct ReconOut {
    pub cell_px: f32,
    pub tested: u64,
    pub passed: u64,
}

#[derive(Clone, Copy, PartialEq)]
pub enum FitSpace {
    Depth,
    InverseDepth,
}

pub fn run_reconstruction_capped(
    sc: &Scene,
    cell_px: f32,
    cap: u64,
    space: FitSpace,
    tol_px: f32,
    s: &mut Scratch,
) -> ReconOut {
    let cam = &sc.cam;
    let mut o = ReconOut {
        cell_px,
        tested: 0,
        passed: 0,
    };
    let nx = (cam.w as f32 / cell_px) as u32;
    let ny = (cam.h as f32 / cell_px) as u32;
    let total = (nx as u64) * (ny as u64);
    let stride = ((total / cap.max(1)) as u32).max(1);
    let mut idx: u64 = 0;
    for ty in 0..ny {
        for tx in 0..nx {
            let this = idx;
            idx += 1;
            if this % stride as u64 != 0 {
                continue;
            }
            let x0 = tx as f32 * cell_px;
            let y0 = ty as f32 * cell_px;
            let mut a = [[0.0f64; 6]; 6];
            let mut rhs = [0.0f64; 6];
            let mut all_hit = true;
            let mut depth_ref = 0.0f32;
            for j in 0..3 {
                for i in 0..3 {
                    let fx = i as f32 * 0.5;
                    let fy = j as f32 * 0.5;
                    let px = x0 + fx * cell_px;
                    let py = y0 + fy * cell_px;
                    let d = cam.dir_at_pixel(px + 0.5, py + 0.5);
                    if let (Some(t), _) = march(&sc.tape, cam.eye, d, sc.t_near, sc.t_far, s) {
                        let b = [
                            1.0,
                            fx as f64,
                            fy as f64,
                            (fx * fx) as f64,
                            (fx * fy) as f64,
                            (fy * fy) as f64,
                        ];
                        let target = match space {
                            FitSpace::Depth => t as f64,
                            FitSpace::InverseDepth => 1.0 / t as f64,
                        };
                        for r in 0..6 {
                            for c in 0..6 {
                                a[r][c] += b[r] * b[c];
                            }
                            rhs[r] += b[r] * target;
                        }
                        depth_ref = t;
                    } else {
                        all_hit = false;
                    }
                }
            }
            if !all_hit {
                continue;
            }
            o.tested += 1;
            let coef = match solve6(a, rhs) {
                Some(c) => c,
                None => continue,
            };
            let foot = depth_ref * 2.0 * cam.tan_half / cam.h as f32;
            let tol = tol_px * foot;
            let mut worst = 0.0f32;
            let mut ok = true;
            for j in 0..7 {
                for i in 0..7 {
                    let fx = i as f32 / 6.0;
                    let fy = j as f32 / 6.0;
                    let px = x0 + fx * cell_px;
                    let py = y0 + fy * cell_px;
                    let d = cam.dir_at_pixel(px + 0.5, py + 0.5);
                    let truth = match march(&sc.tape, cam.eye, d, sc.t_near, sc.t_far, s) {
                        (Some(t), _) => t,
                        (None, _) => {
                            ok = false;
                            continue;
                        }
                    };
                    let fit = coef[0]
                        + coef[1] * fx as f64
                        + coef[2] * fy as f64
                        + coef[3] * (fx * fx) as f64
                        + coef[4] * (fx * fy) as f64
                        + coef[5] * (fy * fy) as f64;
                    let fit_t = match space {
                        FitSpace::Depth => fit,
                        FitSpace::InverseDepth => {
                            if fit.abs() < 1e-9 {
                                ok = false;
                                continue;
                            }
                            1.0 / fit
                        }
                    };
                    let e = (fit_t as f32 - truth).abs();
                    worst = worst.max(e);
                }
            }
            if ok && worst <= tol {
                o.passed += 1;
            }
        }
    }
    o
}

fn solve6(mut a: [[f64; 6]; 6], mut b: [f64; 6]) -> Option<[f64; 6]> {
    for c in 0..6 {
        let mut piv = c;
        for r in c + 1..6 {
            if a[r][c].abs() > a[piv][c].abs() {
                piv = r;
            }
        }
        if a[piv][c].abs() < 1e-12 {
            return None;
        }
        a.swap(c, piv);
        b.swap(c, piv);
        for r in c + 1..6 {
            let f = a[r][c] / a[c][c];
            for k in c..6 {
                a[r][k] -= f * a[c][k];
            }
            b[r] -= f * b[c];
        }
    }
    let mut x = [0.0; 6];
    for r in (0..6).rev() {
        let mut sum = b[r];
        for k in r + 1..6 {
            sum -= a[r][k] * x[k];
        }
        x[r] = sum / a[r][r];
    }
    Some(x)
}

pub struct ReprojOut {
    pub pixels: u64,
    pub hinted: u64,
    pub verified: u64,
    pub disoccluded: u64,
    pub tunnelled: u64,
}

pub fn run_reprojection(sc: &Scene, stride: u32, slack: f32, s: &mut Scratch) -> ReprojOut {
    let (c0, c1) = (&sc.cam, &sc.cam2);
    let w = (c1.w / stride) as usize;
    let h = (c1.h / stride) as usize;
    let mut hint = vec![f32::INFINITY; w * h];

    let mut y = 0;
    while y < c0.h {
        let mut x = 0;
        while x < c0.w {
            let d = c0.dir_at_pixel(x as f32 + 0.5, y as f32 + 0.5);
            if let (Some(t), _) = march(&sc.tape, c0.eye, d, sc.t_near, sc.t_far, s) {
                let p = [
                    c0.eye[0] + t * d[0],
                    c0.eye[1] + t * d[1],
                    c0.eye[2] + t * d[2],
                ];
                let r = [p[0] - c1.eye[0], p[1] - c1.eye[1], p[2] - c1.eye[2]];
                let zf = r[0] * c1.fwd[0] + r[1] * c1.fwd[1] + r[2] * c1.fwd[2];
                if zf > 1e-4 {
                    let xr = r[0] * c1.right[0] + r[1] * c1.right[1] + r[2] * c1.right[2];
                    let yu = r[0] * c1.up[0] + r[1] * c1.up[1] + r[2] * c1.up[2];
                    let u = xr / zf;
                    let v = yu / zf;
                    let px = (u / (c1.aspect * c1.tan_half) + 1.0) * 0.5 * c1.w as f32;
                    let py = (1.0 - v / c1.tan_half) * 0.5 * c1.h as f32;
                    if px >= 0.0 && py >= 0.0 {
                        let (ix, iy) =
                            ((px as u32 / stride) as usize, (py as u32 / stride) as usize);
                        if ix < w && iy < h {
                            let dist = (r[0] * r[0] + r[1] * r[1] + r[2] * r[2]).sqrt();
                            let c = &mut hint[iy * w + ix];
                            if dist < *c {
                                *c = dist;
                            }
                        }
                    }
                }
            }
            x += stride;
        }
        y += stride;
    }

    let mut o = ReprojOut {
        pixels: 0,
        hinted: 0,
        verified: 0,
        disoccluded: 0,
        tunnelled: 0,
    };
    for iy in 0..h {
        for ix in 0..w {
            let px = (ix as u32 * stride) as f32 + 0.5;
            let py = (iy as u32 * stride) as f32 + 0.5;
            let d = c1.dir_at_pixel(px, py);
            let truth = march(&sc.tape, c1.eye, d, sc.t_near, sc.t_far, s).0;
            o.pixels += 1;
            let hv = hint[iy * w + ix];
            if !hv.is_finite() {
                o.disoccluded += 1;
                continue;
            }
            o.hinted += 1;
            let start = (hv - slack).max(sc.t_near);
            let got = march(&sc.tape, c1.eye, d, start, sc.t_far, s).0;
            match (truth, got) {
                (Some(a), Some(b)) if (a - b).abs() < 3.0 * HIT_EPS * a.max(1.0) + 1e-3 => {
                    o.verified += 1
                }
                (None, None) => o.verified += 1,
                _ => o.tunnelled += 1,
            }
        }
    }
    o
}

pub struct SelfCheck {
    pub samples: u64,
    pub containment_failures: u64,
    pub prune_mismatches: u64,
    pub grad_max_rel_err: f32,
    pub grad_p99_rel_err: f32,
    pub grad_kink_skips: u64,
    pub grad_tested: u64,
    pub mean_overwidth: f64,
    pub overwidth_n: u64,
}

pub fn run_selfcheck(
    sc: &Scene,
    cells: u32,
    pts: u32,
    rng: &mut Rng,
    s: &mut Scratch,
) -> SelfCheck {
    let cam = &sc.cam;
    let mut o = SelfCheck {
        samples: 0,
        containment_failures: 0,
        prune_mismatches: 0,
        grad_max_rel_err: 0.0,
        grad_p99_rel_err: 0.0,
        grad_kink_skips: 0,
        grad_tested: 0,
        mean_overwidth: 0.0,
        overwidth_n: 0,
    };
    let mut full_scratch: Vec<f32> = Vec::new();
    let mut rel_errs: Vec<f32> = Vec::new();
    let mut overwidths: Vec<f64> = Vec::new();
    for _ in 0..cells {
        let size = [4.0f32, 8.0, 16.0, 32.0, 64.0][(rng.next_u32() % 5) as usize];
        let x0 = (rng.range(0.0, cam.w as f32 - size)).floor();
        let y0 = (rng.range(0.0, cam.h as f32 - size)).floor();
        let t0 = rng.range(sc.t_near, sc.t_far * 0.5);
        let t1 = t0 * rng.range(1.05, 2.5);
        let (u0, u1) = (cam.u_of(x0), cam.u_of(x0 + size));
        let (v0, v1) = (cam.v_of(y0 + size), cam.v_of(y0));

        let p = cam.wedge(u0, u1, v0, v1, t0, t1);
        eval_aff(&sc.tape, p, &mut s.a, &mut s.av);
        let r = s.av[sc.tape.root as usize];
        if !(r.lo.is_finite() && r.hi.is_finite()) {
            continue;
        }
        let pr = prune(&sc.tape, &s.av);

        let (lo, hi) = (r.lo, r.hi);
        let mut tmin = f32::INFINITY;
        let mut tmax = f32::NEG_INFINITY;
        for _ in 0..pts {
            let u = rng.range(u0.min(u1), u0.max(u1));
            let v = rng.range(v0.min(v1), v0.max(v1));
            let t = rng.range(t0, t1);
            let d = cam.dir(u, v);
            let q = [
                cam.eye[0] + t * d[0],
                cam.eye[1] + t * d[1],
                cam.eye[2] + t * d[2],
            ];

            let truth = eval(&sc.tape, q, &mut full_scratch);
            o.samples += 1;
            let slack = 1e-3 * (1.0 + truth.abs());
            if truth < lo - slack || truth > hi + slack {
                o.containment_failures += 1;
            }
            tmin = tmin.min(truth);
            tmax = tmax.max(truth);

            let pruned = eval(&pr.tape, q, &mut s.f);
            if pruned.to_bits() != truth.to_bits() {
                o.prune_mismatches += 1;
            }

            let (f0, g) = eval_grad(&sc.tape, q, &mut s.g);
            let eps = 1e-3f32;
            for k in 0..3 {
                let mut a = q;
                let mut b = q;
                a[k] += eps;
                b[k] -= eps;
                let fa = eval(&sc.tape, a, &mut full_scratch);
                let fb = eval(&sc.tape, b, &mut full_scratch);
                let d2 = (fa - 2.0 * f0 + fb).abs();
                if d2 > 1e-2 * eps {
                    o.grad_kink_skips += 1;
                    continue;
                }
                let fd = (fa - fb) / (2.0 * eps);
                let denom = fd.abs().max(g[k].abs()).max(0.05);
                let rel = (fd - g[k]).abs() / denom;
                if rel.is_finite() {
                    o.grad_tested += 1;
                    o.grad_max_rel_err = o.grad_max_rel_err.max(rel);
                    rel_errs.push(rel);
                }
            }
        }
        if tmax > tmin {
            overwidths.push(((hi - lo) / (tmax - tmin)) as f64);
            o.overwidth_n += 1;
        }
    }
    if !overwidths.is_empty() {
        overwidths.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        o.mean_overwidth = overwidths[overwidths.len() / 2];
    }
    if !rel_errs.is_empty() {
        rel_errs.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        o.grad_p99_rel_err = rel_errs[(rel_errs.len() * 99 / 100).min(rel_errs.len() - 1)];
    }
    o
}

pub fn debug_ppm(sc: &Scene, s: &mut Scratch) -> Vec<u8> {
    let cam = &sc.cam;
    let mut buf = format!("P6\n{} {}\n255\n", cam.w, cam.h).into_bytes();
    for y in 0..cam.h {
        for x in 0..cam.w {
            let d = cam.dir_at_pixel(x as f32 + 0.5, y as f32 + 0.5);
            let px = match march(&sc.tape, cam.eye, d, sc.t_near, sc.t_far, s) {
                (Some(t), _) => {
                    let p = [
                        cam.eye[0] + t * d[0],
                        cam.eye[1] + t * d[1],
                        cam.eye[2] + t * d[2],
                    ];
                    let (_, g) = eval_grad(&sc.tape, p, &mut s.g);
                    let l = (g[0] * g[0] + g[1] * g[1] + g[2] * g[2]).sqrt().max(1e-6);
                    let n = [g[0] / l, g[1] / l, g[2] / l];
                    let lit = (n[0] * 0.4 + n[1] * 0.8 + n[2] * 0.45).max(0.0) * 0.85 + 0.15;
                    let v = (lit.clamp(0.0, 1.0) * 255.0) as u8;
                    [v, v, v]
                }
                (None, _) => [24, 28, 36],
            };
            buf.extend_from_slice(&px);
        }
    }
    buf
}

const AA_OP_COST: f64 = 8.0;

const SHADE_FLOP: f64 = 800.0;
const POST_FLOP: f64 = 300.0;
const SHADOW_EVALS: f64 = 5.0;
const AO_GI_EVALS: f64 = 6.0;

pub struct FrameCost {
    pub pixels: u64,
    pub interior_px: u64,
    pub marched_px: u64,
    pub exterior_px: u64,
    pub hits: u64,
    pub traversal: f64,
    pub primary: f64,
    pub primary_fallback: f64,
    pub shadow: f64,
    pub ao_gi: f64,
    pub shade: f64,
    pub post: f64,
    pub mean_steps: f64,
    pub exterior_hits: u64,
    pub v_uops_lane: f64,
    pub total_ideal: f64,
}

impl FrameCost {
    pub fn total(&self) -> f64 {
        self.traversal + self.primary + self.shadow + self.ao_gi + self.shade + self.post
    }
    pub fn per_pixel(&self) -> f64 {
        self.total() / self.pixels.max(1) as f64
    }
    pub fn per_pixel_ideal(&self) -> f64 {
        self.total_ideal / self.pixels.max(1) as f64
    }
    pub fn per_pixel_optimistic(&self) -> f64 {
        (self.total() - self.primary_fallback) / self.pixels.max(1) as f64
    }
}
pub fn run_framecost(sc: &Scene, cl: &ClassifyOut, s: &mut Scratch) -> FrameCost {
    run_framecost_sw(sc, cl, &crate::tape::UopSweep::pessimistic(), s)
}

pub fn run_framecost_sw(
    sc: &Scene,
    cl: &ClassifyOut,
    sweep: &crate::tape::UopSweep,
    s: &mut Scratch,
) -> FrameCost {
    let cam = &sc.cam;
    let mut fc = FrameCost {
        pixels: (cam.w as u64) * (cam.h as u64),
        interior_px: 0,
        marched_px: 0,
        exterior_px: 0,
        hits: 0,
        traversal: cl.traversal_ops as f64 * AA_OP_COST,
        primary: 0.0,
        primary_fallback: 0.0,
        shadow: 0.0,
        ao_gi: 0.0,
        shade: 0.0,
        post: 0.0,
        mean_steps: 0.0,
        exterior_hits: 0,
        v_uops_lane: 0.0,
        total_ideal: 0.0,
    };
    let mut covered = 0u64;
    let mut step_sum = 0u64;
    let mut ideal_primary = 0.0f64;
    let total_area = (cam.w as f64) * (cam.h as f64);
    let liftable =
        ((cl.area_interior_empirical - cl.area_interior).max(0.0) / total_area).clamp(0.0, 1.0);
    let ratio = (sc.t_far / sc.t_near).powf(1.0 / NSLAB as f32);
    let mut aff = Vec::new();
    let mut ivs = Vec::new();

    for lf in &cl.leaves {
        let x1 = (lf.x0 + lf.size).min(cam.w as f32);
        let y1 = (lf.y0 + lf.size).min(cam.h as f32);
        let (u0, u1) = (cam.u_of(lf.x0), cam.u_of(lf.x0 + lf.size));
        let (v0, v1) = (cam.v_of(lf.y0 + lf.size), cam.v_of(lf.y0));

        let mut pend: Vec<(f32, f32)> = Vec::new();
        let mut x = lf.x0;
        while x < x1 {
            let mut y = lf.y0;
            while y < y1 {
                pend.push((x, y));
                y += 1.0;
            }
            x += 1.0;
        }
        let npix = pend.len() as u64;
        covered += npix;
        if lf.class == Class::Interior {
            fc.interior_px += npix;
        } else {
            fc.marched_px += npix;
        }
        fc.post += npix as f64 * POST_FLOP;

        let mut tape = lf.tape.clone();
        let mut lo = lf.t0.max(sc.t_near);
        let mut hi = lf.t1.min(sc.t_far);
        let mut first = true;

        while !pend.is_empty() && lo < sc.t_far && hi > lo {
            if !first {
                let p = cam.wedge(u0, u1, v0, v1, lo, hi);
                eval_aff(&lf.tape_wide, p, &mut aff, &mut ivs);
                fc.traversal += lf.tape_wide.weight() as f64 * AA_OP_COST;
                if ivs[lf.tape_wide.root as usize].lo > 0.0 {
                    lo = hi;
                    hi = (lo * ratio).min(sc.t_far);
                    continue;
                }
                tape = prune(&lf.tape_wide, &ivs).tape;
            }
            first = false;
            let w = tape.weight() as f64;
            let wu = tape.v_uops(sweep) as f64;

            let mut still = Vec::with_capacity(pend.len());
            for &(px, py) in &pend {
                let d = cam.dir_at_pixel(px + 0.5, py + 0.5);
                let (hit, steps) = march(&tape, cam.eye, d, lo, hi, s);
                let charged = if lf.class == Class::Interior {
                    6.0
                } else {
                    steps as f64
                };
                step_sum += steps as u64;
                fc.primary += charged * w;
                fc.v_uops_lane += charged * wu;
                ideal_primary += if lf.class == Class::Interior {
                    charged * w
                } else {
                    (1.0 - liftable) * charged * w + liftable * 6.0 * w
                };
                if hit.is_some() {
                    fc.hits += 1;
                    fc.shadow += SHADOW_EVALS * w;
                    fc.ao_gi += AO_GI_EVALS * w;
                    fc.shade += SHADE_FLOP;
                    fc.v_uops_lane += (SHADOW_EVALS + AO_GI_EVALS) * wu;
                    fc.v_uops_lane += SHADE_FLOP * 0.5;
                } else {
                    still.push((px, py));
                }
            }
            pend = still;
            lo = hi;
            hi = (lo * ratio).min(sc.t_far);
        }
    }

    fc.exterior_px = fc.pixels.saturating_sub(covered);
    fc.post += fc.exterior_px as f64 * POST_FLOP;
    fc.v_uops_lane += fc.pixels as f64 * POST_FLOP * 0.5;
    {
        let mut mask = vec![false; (cam.w as usize) * (cam.h as usize)];
        for lf in &cl.leaves {
            let x1 = (lf.x0 + lf.size).min(cam.w as f32) as usize;
            let y1 = (lf.y0 + lf.size).min(cam.h as f32) as usize;
            for yy in lf.y0 as usize..y1 {
                for xx in lf.x0 as usize..x1 {
                    mask[yy * cam.w as usize + xx] = true;
                }
            }
        }
        for yy in 0..cam.h as usize {
            for xx in 0..cam.w as usize {
                if mask[yy * cam.w as usize + xx] {
                    continue;
                }
                let d = cam.dir_at_pixel(xx as f32 + 0.5, yy as f32 + 0.5);
                if march(&sc.tape, cam.eye, d, sc.t_near, sc.t_far, s)
                    .0
                    .is_some()
                {
                    fc.exterior_hits += 1;
                }
            }
        }
    }
    fc.mean_steps = step_sum as f64 / fc.marched_px.max(1) as f64;
    fc.total_ideal = fc.traversal + ideal_primary + fc.shadow + fc.ao_gi + fc.shade + fc.post;
    fc
}

#[inline]
fn cosine(cam: &Camera, d: [f32; 3]) -> f32 {
    (d[0] * cam.fwd[0] + d[1] * cam.fwd[1] + d[2] * cam.fwd[2]).max(1e-6)
}

pub struct ReconAdaptive {
    pub samples: u64,
    pub pixels: u64,
    pub dense_px: u64,
    pub patches: Vec<(f32, u64)>,
}

impl ReconAdaptive {
    pub fn factor(&self) -> f64 {
        self.pixels as f64 / self.samples.max(1) as f64
    }
}

#[allow(clippy::too_many_arguments)]
fn recon_cell(
    sc: &Scene,
    x0: f32,
    y0: f32,
    size: f32,
    min_size: f32,
    tol_px: f32,
    s: &mut Scratch,
    out: &mut ReconAdaptive,
) {
    let cam = &sc.cam;
    let vx = (x0 + size).min(cam.w as f32) - x0;
    let vy = (y0 + size).min(cam.h as f32) - y0;
    if vx <= 0.0 || vy <= 0.0 {
        return;
    }
    let area = (vx * vy) as u64;

    let mut a = [[0.0f64; 6]; 6];
    let mut rhs = [0.0f64; 6];
    let mut all_hit = true;
    let mut depth_ref = 0.0f32;
    for j in 0..3 {
        for i in 0..3 {
            let (fx, fy) = (i as f32 * 0.5, j as f32 * 0.5);
            let d = cam.dir_at_pixel(x0 + fx * size, y0 + fy * size);
            match march(&sc.tape, cam.eye, d, sc.t_near, sc.t_far, s).0 {
                Some(t) => {
                    let z = t * cosine(cam, d);
                    let b = [
                        1.0,
                        fx as f64,
                        fy as f64,
                        (fx * fx) as f64,
                        (fx * fy) as f64,
                        (fy * fy) as f64,
                    ];
                    for r in 0..6 {
                        for c in 0..6 {
                            a[r][c] += b[r] * b[c];
                        }
                        rhs[r] += b[r] / z as f64;
                    }
                    depth_ref = t;
                }
                None => all_hit = false,
            }
        }
    }

    let mut ok = all_hit && depth_ref > 0.0;
    if ok {
        if let Some(coef) = solve6(a, rhs) {
            let tol = tol_px * depth_ref * 2.0 * cam.tan_half / cam.h as f32;
            'check: for j in 0..5 {
                for i in 0..5 {
                    let (fx, fy) = (i as f32 / 4.0, j as f32 / 4.0);
                    let d = cam.dir_at_pixel(x0 + fx * size, y0 + fy * size);
                    let truth = match march(&sc.tape, cam.eye, d, sc.t_near, sc.t_far, s).0 {
                        Some(t) => t,
                        None => {
                            ok = false;
                            break 'check;
                        }
                    };
                    let q = coef[0]
                        + coef[1] * fx as f64
                        + coef[2] * fy as f64
                        + coef[3] * (fx * fx) as f64
                        + coef[4] * (fx * fy) as f64
                        + coef[5] * (fy * fy) as f64;
                    if q.abs() < 1e-9 {
                        ok = false;
                        break 'check;
                    }
                    let t_fit = (1.0 / q) as f32 / cosine(cam, d);
                    if (t_fit - truth).abs() > tol {
                        ok = false;
                        break 'check;
                    }
                }
            }
        } else {
            ok = false;
        }
    }

    if ok {
        out.samples += 9;
        out.pixels += area;
        match out.patches.iter_mut().find(|(sz, _)| *sz == size) {
            Some((_, n)) => *n += 1,
            None => out.patches.push((size, 1)),
        }
        return;
    }
    if size <= min_size {
        out.samples += area;
        out.pixels += area;
        out.dense_px += area;
        return;
    }
    let h = size * 0.5;
    for (dx, dy) in [(0.0, 0.0), (h, 0.0), (0.0, h), (h, h)] {
        recon_cell(sc, x0 + dx, y0 + dy, h, min_size, tol_px, s, out);
    }
}

pub fn run_recon_adaptive(
    sc: &Scene,
    base: f32,
    min_size: f32,
    tol_px: f32,
    s: &mut Scratch,
) -> ReconAdaptive {
    let mut out = ReconAdaptive {
        samples: 0,
        pixels: 0,
        dense_px: 0,
        patches: Vec::new(),
    };
    let cam = &sc.cam;
    let nx = (cam.w as f32 / base).ceil() as u32;
    let ny = (cam.h as f32 / base).ceil() as u32;
    for ty in 0..ny {
        for tx in 0..nx {
            recon_cell(
                sc,
                tx as f32 * base,
                ty as f32 * base,
                base,
                min_size,
                tol_px,
                s,
                &mut out,
            );
        }
    }
    out.patches
        .sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
    out
}

pub struct EdgeCensus {
    pub pixels: u64,
    pub silhouette: u64,
    pub depth_step: u64,
    pub edge: u64,
}

pub fn run_edge_census(sc: &Scene, s: &mut Scratch) -> EdgeCensus {
    let cam = &sc.cam;
    let (w, h) = (cam.w as usize, cam.h as usize);
    let mut depth = vec![f32::INFINITY; w * h];
    for y in 0..h {
        for x in 0..w {
            let d = cam.dir_at_pixel(x as f32 + 0.5, y as f32 + 0.5);
            if let (Some(t), _) = march(&sc.tape, cam.eye, d, sc.t_near, sc.t_far, s) {
                depth[y * w + x] = t;
            }
        }
    }
    let mut o = EdgeCensus {
        pixels: (w * h) as u64,
        silhouette: 0,
        depth_step: 0,
        edge: 0,
    };
    for y in 0..h {
        for x in 0..w {
            let c = depth[y * w + x];
            let mut sil = false;
            let mut step = false;
            for (dx, dy) in [(-1i32, 0i32), (1, 0), (0, -1), (0, 1)] {
                let (nx, ny) = (x as i32 + dx, y as i32 + dy);
                if nx < 0 || ny < 0 || nx >= w as i32 || ny >= h as i32 {
                    continue;
                }
                let n = depth[ny as usize * w + nx as usize];
                match (c.is_finite(), n.is_finite()) {
                    (a, b) if a != b => sil = true,
                    (true, true) => {
                        if (c - n).abs() / c.min(n).max(1e-6) > 0.05 {
                            step = true;
                        }
                    }
                    _ => {}
                }
            }
            if sil {
                o.silhouette += 1;
            }
            if step {
                o.depth_step += 1;
            }
            if sil || step {
                o.edge += 1;
            }
        }
    }
    o
}

pub struct AtlasOut {
    pub cost: crate::atlas::TraceCost,
    pub pixels: u64,
    pub hits: u64,
    pub mismatches: u64,
    pub miss_atlas_none: u64,
    pub miss_atlas_extra: u64,
    pub miss_depth: u64,
    pub worst_dt: f32,
}

pub fn run_atlas(sc: &Scene, at: &crate::atlas::Atlas, s: &mut Scratch) -> AtlasOut {
    let cam = &sc.cam;
    let mut o = AtlasOut {
        cost: Default::default(),
        pixels: (cam.w as u64) * (cam.h as u64),
        hits: 0,
        mismatches: 0,
        miss_atlas_none: 0,
        miss_atlas_extra: 0,
        miss_depth: 0,
        worst_dt: 0.0,
    };
    for y in 0..cam.h {
        for x in 0..cam.w {
            let d = cam.dir_at_pixel(x as f32 + 0.5, y as f32 + 0.5);
            let got = crate::atlas::trace(
                at,
                &sc.tape,
                cam.eye,
                d,
                sc.t_near,
                sc.t_far,
                &mut o.cost,
                s,
            );
            let truth = march(&sc.tape, cam.eye, d, sc.t_near, sc.t_far, s).0;
            if got.is_some() {
                o.hits += 1;
            }
            match (got, truth) {
                (Some(a), Some(b)) if (a - b).abs() <= 3e-3 * b.max(1.0) => {}
                (None, None) => {}
                (Some(a), Some(b)) => {
                    o.mismatches += 1;
                    o.miss_depth += 1;
                    o.worst_dt = o.worst_dt.max((a - b).abs());
                }
                (None, Some(_)) => {
                    o.mismatches += 1;
                    o.miss_atlas_none += 1;
                }
                (Some(_), None) => {
                    o.mismatches += 1;
                    o.miss_atlas_extra += 1;
                }
            }
        }
    }
    o
}

pub struct FrameStat {
    pub deg: f32,
    pub primary_flop_px: f64,
    pub hit_rate: f64,
    pub hinted: f64,
    pub verified: f64,
}

pub fn run_motion(
    sc: &Scene,
    at: &crate::atlas::Atlas,
    stride: u32,
    s: &mut Scratch,
) -> Vec<FrameStat> {
    let mut out = Vec::new();
    let mut prev: Option<(&Camera, Vec<Option<f32>>)> = None;
    for (i, cam) in sc.path.iter().enumerate() {
        let mut cost: crate::atlas::TraceCost = Default::default();
        let w = (cam.w / stride) as usize;
        let h = (cam.h / stride) as usize;
        let mut depth = vec![None; w * h];
        let mut hits = 0u64;
        for iy in 0..h {
            for ix in 0..w {
                let px = (ix as u32 * stride) as f32 + 0.5;
                let py = (iy as u32 * stride) as f32 + 0.5;
                let d = cam.dir_at_pixel(px, py);
                let t = crate::atlas::trace(
                    at, &sc.tape, cam.eye, d, sc.t_near, sc.t_far, &mut cost, s,
                );
                if t.is_some() {
                    hits += 1;
                }
                depth[iy * w + ix] = t;
            }
        }
        let n = (w * h) as f64;

        let (mut hinted, mut verified) = (0.0, 0.0);
        if let Some((p0, pd)) = &prev {
            let mut hint = vec![f32::INFINITY; w * h];
            for iy in 0..h {
                for ix in 0..w {
                    let t = match pd[iy * w + ix] {
                        Some(t) => t,
                        None => continue,
                    };
                    let px = (ix as u32 * stride) as f32 + 0.5;
                    let py = (iy as u32 * stride) as f32 + 0.5;
                    let d0 = p0.dir_at_pixel(px, py);
                    let wp = [
                        p0.eye[0] + t * d0[0],
                        p0.eye[1] + t * d0[1],
                        p0.eye[2] + t * d0[2],
                    ];
                    let r = [wp[0] - cam.eye[0], wp[1] - cam.eye[1], wp[2] - cam.eye[2]];
                    let zf = r[0] * cam.fwd[0] + r[1] * cam.fwd[1] + r[2] * cam.fwd[2];
                    if zf <= 1e-4 {
                        continue;
                    }
                    let xr = r[0] * cam.right[0] + r[1] * cam.right[1] + r[2] * cam.right[2];
                    let yu = r[0] * cam.up[0] + r[1] * cam.up[1] + r[2] * cam.up[2];
                    let sx = (xr / zf / (cam.aspect * cam.tan_half) + 1.0) * 0.5 * cam.w as f32;
                    let sy = (1.0 - yu / zf / cam.tan_half) * 0.5 * cam.h as f32;
                    if sx < 0.0 || sy < 0.0 {
                        continue;
                    }
                    let (jx, jy) = ((sx as u32 / stride) as usize, (sy as u32 / stride) as usize);
                    if jx < w && jy < h {
                        let dist = (r[0] * r[0] + r[1] * r[1] + r[2] * r[2]).sqrt();
                        let c = &mut hint[jy * w + jx];
                        if dist < *c {
                            *c = dist;
                        }
                    }
                }
            }
            for iy in 0..h {
                for ix in 0..w {
                    let hv = hint[iy * w + ix];
                    if !hv.is_finite() {
                        continue;
                    }
                    hinted += 1.0;
                    if let Some(t) = depth[iy * w + ix] {
                        if (t - hv).abs() < 0.08 {
                            verified += 1.0;
                        }
                    }
                }
            }
        }

        let deg = match &prev {
            Some((p0, _)) => {
                let dot =
                    (p0.fwd[0] * cam.fwd[0] + p0.fwd[1] * cam.fwd[1] + p0.fwd[2] * cam.fwd[2])
                        .clamp(-1.0, 1.0);
                dot.acos().to_degrees()
            }
            None => 0.0,
        };
        out.push(FrameStat {
            deg,
            primary_flop_px: cost.flop() / n,
            hit_rate: hits as f64 / n,
            hinted: hinted / n,
            verified: if hinted > 0.0 { verified / hinted } else { 0.0 },
        });
        prev = Some((cam, depth));
        let _ = i;
    }
    out
}

pub struct EdgeRecon {
    pub pixels: u64,
    pub edge_px: u64,
    pub patch_samples: u64,
    pub edge_samples: u64,
    pub dense_samples: u64,
    pub patches: Vec<(u32, u64)>,
}

impl EdgeRecon {
    pub fn samples(&self) -> u64 {
        self.patch_samples + self.edge_samples + self.dense_samples
    }
    pub fn factor(&self) -> f64 {
        self.pixels as f64 / self.samples().max(1) as f64
    }
}

struct Field {
    w: usize,
    h: usize,
    depth: Vec<f32>,
    edge: Vec<bool>,
}

fn recon_edge_cell(
    f: &Field,
    cam: &Camera,
    x0: usize,
    y0: usize,
    size: usize,
    tol_px: f32,
    out: &mut EdgeRecon,
) {
    let x1 = (x0 + size).min(f.w);
    let y1 = (y0 + size).min(f.h);
    if x0 >= x1 || y0 >= y1 {
        return;
    }
    let mut smooth: Vec<(f32, f32, f32)> = Vec::new();
    let mut n_edge = 0u64;
    let mut n_miss = 0u64;
    let mut zref = 0.0f32;
    for y in y0..y1 {
        for x in x0..x1 {
            let i = y * f.w + x;
            if f.edge[i] {
                n_edge += 1;
                continue;
            }
            let t = f.depth[i];
            if !t.is_finite() {
                n_miss += 1;
                continue;
            }
            let d = cam.dir_at_pixel(x as f32 + 0.5, y as f32 + 0.5);
            let cz = (d[0] * cam.fwd[0] + d[1] * cam.fwd[1] + d[2] * cam.fwd[2]).max(1e-6);
            smooth.push((
                (x - x0) as f32 / size as f32,
                (y - y0) as f32 / size as f32,
                t * cz,
            ));
            zref = t;
        }
    }
    let _ = n_miss;
    if smooth.is_empty() {
        out.edge_samples += n_edge;
        return;
    }

    let mut ata = [[0.0f64; 6]; 6];
    let mut atb = [0.0f64; 6];
    for &(fx, fy, z) in &smooth {
        let b = [
            1.0,
            fx as f64,
            fy as f64,
            (fx * fx) as f64,
            (fx * fy) as f64,
            (fy * fy) as f64,
        ];
        for r in 0..6 {
            for c in 0..6 {
                ata[r][c] += b[r] * b[c];
            }
            atb[r] += b[r] / z as f64;
        }
    }
    let ok = if smooth.len() >= 6 {
        match solve6(ata, atb) {
            Some(coef) => {
                let tol = tol_px * zref * 2.0 * cam.tan_half / cam.h as f32;
                smooth.iter().all(|&(fx, fy, z)| {
                    let q = coef[0]
                        + coef[1] * fx as f64
                        + coef[2] * fy as f64
                        + coef[3] * (fx * fx) as f64
                        + coef[4] * (fx * fy) as f64
                        + coef[5] * (fy * fy) as f64;
                    q.abs() > 1e-9 && ((1.0 / q) as f32 - z).abs() <= tol
                })
            }
            None => false,
        }
    } else {
        false
    };

    if ok {
        out.edge_samples += n_edge;
        out.patch_samples += 9;
        let sz = size as u32;
        match out.patches.iter_mut().find(|(s, _)| *s == sz) {
            Some((_, n)) => *n += 1,
            None => out.patches.push((sz, 1)),
        }
        return;
    }
    if size <= 2 {
        out.edge_samples += n_edge;
        out.dense_samples += smooth.len() as u64;
        return;
    }
    let hs = size / 2;
    for (dx, dy) in [(0, 0), (hs, 0), (0, hs), (hs, hs)] {
        recon_edge_cell(f, cam, x0 + dx, y0 + dy, hs, tol_px, out);
    }
}

pub fn run_edge_recon(
    sc: &Scene,
    tol_px: f32,
    base: usize,
    s: &mut Scratch,
) -> (EdgeCensus, EdgeRecon) {
    let cam = &sc.cam;
    let (w, h) = (cam.w as usize, cam.h as usize);
    let mut depth = vec![f32::INFINITY; w * h];
    for y in 0..h {
        for x in 0..w {
            let d = cam.dir_at_pixel(x as f32 + 0.5, y as f32 + 0.5);
            if let (Some(t), _) = march(&sc.tape, cam.eye, d, sc.t_near, sc.t_far, s) {
                depth[y * w + x] = t;
            }
        }
    }
    let mut edge = vec![false; w * h];
    let mut cen = EdgeCensus {
        pixels: (w * h) as u64,
        silhouette: 0,
        depth_step: 0,
        edge: 0,
    };
    for y in 0..h {
        for x in 0..w {
            let c = depth[y * w + x];
            let (mut sil, mut stp) = (false, false);
            for (dx, dy) in [(-1i32, 0i32), (1, 0), (0, -1), (0, 1)] {
                let (nx, ny) = (x as i32 + dx, y as i32 + dy);
                if nx < 0 || ny < 0 || nx >= w as i32 || ny >= h as i32 {
                    continue;
                }
                let n = depth[ny as usize * w + nx as usize];
                match (c.is_finite(), n.is_finite()) {
                    (a, b) if a != b => sil = true,
                    (true, true) => {
                        if (c - n).abs() / c.min(n).max(1e-6) > 0.05 {
                            stp = true;
                        }
                    }
                    _ => {}
                }
            }
            if sil {
                cen.silhouette += 1;
            }
            if stp {
                cen.depth_step += 1;
            }
            if sil || stp {
                cen.edge += 1;
                edge[y * w + x] = true;
            }
        }
    }

    let f = Field { w, h, depth, edge };
    let mut out = EdgeRecon {
        pixels: (w * h) as u64,
        edge_px: cen.edge,
        patch_samples: 0,
        edge_samples: 0,
        dense_samples: 0,
        patches: Vec::new(),
    };
    let mut y = 0;
    while y < h {
        let mut x = 0;
        while x < w {
            recon_edge_cell(&f, cam, x, y, base, tol_px, &mut out);
            x += base;
        }
        y += base;
    }
    out.patches.sort_by(|a, b| b.0.cmp(&a.0));
    (cen, out)
}

pub struct LightBake {
    pub dims: [usize; 3],
    pub cell: f32,
    pub cells: u64,
    pub bake_rays: u64,
    pub bytes_f32: u64,
    pub tested: u64,
    pub mean_err: f64,
    pub p95_err: f32,
    pub max_err: f32,
}

const AO_RADIUS: f32 = 0.6;
const AO_RAYS: usize = 8;

fn ao_dirs() -> [[f32; 3]; AO_RAYS] {
    let mut d = [[0.0f32; 3]; AO_RAYS];
    let ga = std::f32::consts::PI * (3.0 - 5.0f32.sqrt());
    for i in 0..AO_RAYS {
        let z = 1.0 - 2.0 * (i as f32 + 0.5) / AO_RAYS as f32;
        let r = (1.0 - z * z).max(0.0).sqrt();
        let th = ga * i as f32;
        d[i] = [r * th.cos(), z, r * th.sin()];
    }
    d
}

fn ao_at(tape: &Tape, p: [f32; 3], dirs: &[[f32; 3]; AO_RAYS], s: &mut Scratch) -> f32 {
    let mut acc = 0.0;
    for d in dirs.iter() {
        let mut occ = 1.0f32;
        let mut t = 0.02f32;
        let mut steps = 0;
        while t < AO_RADIUS && steps < 24 {
            let q = [p[0] + t * d[0], p[1] + t * d[1], p[2] + t * d[2]];
            let f = eval(tape, q, &mut s.f);
            steps += 1;
            if f < 1e-3 {
                occ = t / AO_RADIUS;
                break;
            }
            t += f.max(0.01);
        }
        acc += occ.min(1.0);
    }
    acc / AO_RAYS as f32
}

pub fn run_light_bake(
    sc: &Scene,
    lo: [f32; 3],
    hi: [f32; 3],
    cell: f32,
    rng: &mut Rng,
    s: &mut Scratch,
) -> LightBake {
    let dims = [
        (((hi[0] - lo[0]) / cell).ceil() as usize + 1).max(2),
        (((hi[1] - lo[1]) / cell).ceil() as usize + 1).max(2),
        (((hi[2] - lo[2]) / cell).ceil() as usize + 1).max(2),
    ];
    let dirs = ao_dirs();
    let n = dims[0] * dims[1] * dims[2];
    let mut grid = vec![1.0f32; n];
    for iz in 0..dims[2] {
        for iy in 0..dims[1] {
            for ix in 0..dims[0] {
                let p = [
                    lo[0] + ix as f32 * cell,
                    lo[1] + iy as f32 * cell,
                    lo[2] + iz as f32 * cell,
                ];
                grid[(iz * dims[1] + iy) * dims[0] + ix] = ao_at(&sc.tape, p, &dirs, s);
            }
        }
    }

    let sample = |g: &Vec<f32>, p: [f32; 3]| -> f32 {
        let f = [
            ((p[0] - lo[0]) / cell).clamp(0.0, (dims[0] - 1) as f32 - 1e-3),
            ((p[1] - lo[1]) / cell).clamp(0.0, (dims[1] - 1) as f32 - 1e-3),
            ((p[2] - lo[2]) / cell).clamp(0.0, (dims[2] - 1) as f32 - 1e-3),
        ];
        let i = [f[0] as usize, f[1] as usize, f[2] as usize];
        let d = [f[0] - i[0] as f32, f[1] - i[1] as f32, f[2] - i[2] as f32];
        let at = |dx: usize, dy: usize, dz: usize| -> f32 {
            g[((i[2] + dz) * dims[1] + i[1] + dy) * dims[0] + i[0] + dx]
        };
        let l = |a: f32, b: f32, t: f32| a + (b - a) * t;
        let c00 = l(at(0, 0, 0), at(1, 0, 0), d[0]);
        let c10 = l(at(0, 1, 0), at(1, 1, 0), d[0]);
        let c01 = l(at(0, 0, 1), at(1, 0, 1), d[0]);
        let c11 = l(at(0, 1, 1), at(1, 1, 1), d[0]);
        l(l(c00, c10, d[1]), l(c01, c11, d[1]), d[2])
    };

    let cam = &sc.cam;
    let mut out = LightBake {
        dims,
        cell,
        cells: n as u64,
        bake_rays: n as u64 * AO_RAYS as u64,
        bytes_f32: (n * 4) as u64,
        tested: 0,
        mean_err: 0.0,
        p95_err: 0.0,
        max_err: 0.0,
    };
    let mut errs: Vec<f32> = Vec::new();
    for _ in 0..1500 {
        let px = rng.range(0.0, cam.w as f32);
        let py = rng.range(0.0, cam.h as f32);
        let d = cam.dir_at_pixel(px, py);
        let t = match march(&sc.tape, cam.eye, d, sc.t_near, sc.t_far, s).0 {
            Some(t) => t,
            None => continue,
        };
        let p = [
            cam.eye[0] + t * d[0],
            cam.eye[1] + t * d[1],
            cam.eye[2] + t * d[2],
        ];
        if p[0] < lo[0]
            || p[1] < lo[1]
            || p[2] < lo[2]
            || p[0] > hi[0]
            || p[1] > hi[1]
            || p[2] > hi[2]
        {
            continue;
        }
        let truth = ao_at(&sc.tape, p, &dirs, s);
        let got = sample(&grid, p);
        let e = (truth - got).abs();
        errs.push(e);
        out.mean_err += e as f64;
        out.max_err = out.max_err.max(e);
        out.tested += 1;
    }
    if out.tested > 0 {
        out.mean_err /= out.tested as f64;
        errs.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        out.p95_err = errs[(errs.len() * 95 / 100).min(errs.len() - 1)];
    }
    out
}

fn sun_vis(tape: &Tape, p: [f32; 3], sun: [f32; 3], k: f32, s: &mut Scratch) -> f32 {
    let mut vis = 1.0f32;
    let mut t = 0.03f32;
    let mut steps = 0;
    while t < 12.0 && steps < 48 {
        let q = [p[0] + t * sun[0], p[1] + t * sun[1], p[2] + t * sun[2]];
        let f = eval(tape, q, &mut s.f);
        steps += 1;
        if f < 1e-3 {
            return 0.0;
        }
        vis = vis.min(k * f / t);
        t += f.max(0.01);
    }
    vis.clamp(0.0, 1.0)
}

pub fn run_sun_bake(
    sc: &Scene,
    lo: [f32; 3],
    hi: [f32; 3],
    cell: f32,
    rng: &mut Rng,
    s: &mut Scratch,
) -> LightBake {
    let sun = {
        let v = [0.42f32, 0.86, 0.29];
        let l = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
        [v[0] / l, v[1] / l, v[2] / l]
    };
    let k = 12.0;
    let dims = [
        (((hi[0] - lo[0]) / cell).ceil() as usize + 1).max(2),
        (((hi[1] - lo[1]) / cell).ceil() as usize + 1).max(2),
        (((hi[2] - lo[2]) / cell).ceil() as usize + 1).max(2),
    ];
    let n = dims[0] * dims[1] * dims[2];
    let mut grid = vec![1.0f32; n];
    for iz in 0..dims[2] {
        for iy in 0..dims[1] {
            for ix in 0..dims[0] {
                let p = [
                    lo[0] + ix as f32 * cell,
                    lo[1] + iy as f32 * cell,
                    lo[2] + iz as f32 * cell,
                ];
                grid[(iz * dims[1] + iy) * dims[0] + ix] = sun_vis(&sc.tape, p, sun, k, s);
            }
        }
    }
    let sample = |g: &Vec<f32>, p: [f32; 3]| -> f32 {
        let f = [
            ((p[0] - lo[0]) / cell).clamp(0.0, (dims[0] - 1) as f32 - 1e-3),
            ((p[1] - lo[1]) / cell).clamp(0.0, (dims[1] - 1) as f32 - 1e-3),
            ((p[2] - lo[2]) / cell).clamp(0.0, (dims[2] - 1) as f32 - 1e-3),
        ];
        let i = [f[0] as usize, f[1] as usize, f[2] as usize];
        let d = [f[0] - i[0] as f32, f[1] - i[1] as f32, f[2] - i[2] as f32];
        let at = |dx: usize, dy: usize, dz: usize| -> f32 {
            g[((i[2] + dz) * dims[1] + i[1] + dy) * dims[0] + i[0] + dx]
        };
        let l = |a: f32, b: f32, t: f32| a + (b - a) * t;
        let c00 = l(at(0, 0, 0), at(1, 0, 0), d[0]);
        let c10 = l(at(0, 1, 0), at(1, 1, 0), d[0]);
        let c01 = l(at(0, 0, 1), at(1, 0, 1), d[0]);
        let c11 = l(at(0, 1, 1), at(1, 1, 1), d[0]);
        l(l(c00, c10, d[1]), l(c01, c11, d[1]), d[2])
    };

    let cam = &sc.cam;
    let mut out = LightBake {
        dims,
        cell,
        cells: n as u64,
        bake_rays: n as u64,
        bytes_f32: (n * 4) as u64,
        tested: 0,
        mean_err: 0.0,
        p95_err: 0.0,
        max_err: 0.0,
    };
    let mut errs: Vec<f32> = Vec::new();
    for _ in 0..1500 {
        let px = rng.range(0.0, cam.w as f32);
        let py = rng.range(0.0, cam.h as f32);
        let d = cam.dir_at_pixel(px, py);
        let t = match march(&sc.tape, cam.eye, d, sc.t_near, sc.t_far, s).0 {
            Some(t) => t,
            None => continue,
        };
        let p = [
            cam.eye[0] + t * d[0],
            cam.eye[1] + t * d[1],
            cam.eye[2] + t * d[2],
        ];
        if p[0] < lo[0]
            || p[1] < lo[1]
            || p[2] < lo[2]
            || p[0] > hi[0]
            || p[1] > hi[1]
            || p[2] > hi[2]
        {
            continue;
        }
        let (_, g) = eval_grad(&sc.tape, p, &mut s.g);
        let gl = (g[0] * g[0] + g[1] * g[1] + g[2] * g[2]).sqrt().max(1e-6);
        let ps = [
            p[0] + g[0] / gl * 0.5 * cell,
            p[1] + g[1] / gl * 0.5 * cell,
            p[2] + g[2] / gl * 0.5 * cell,
        ];
        let truth = sun_vis(&sc.tape, ps, sun, k, s);
        let got = sample(&grid, ps);
        let e = (truth - got).abs();
        errs.push(e);
        out.mean_err += e as f64;
        out.max_err = out.max_err.max(e);
        out.tested += 1;
    }
    if out.tested > 0 {
        out.mean_err /= out.tested as f64;
        errs.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        out.p95_err = errs[(errs.len() * 95 / 100).min(errs.len() - 1)];
    }
    out
}

pub struct DeformTemporal {
    pub pixels: u64,
    pub hinted: u64,
    pub verified: u64,
    pub tunnelled: u64,
    pub max_closing: f32,
    pub p99_closing: f32,
    pub mean_closing: f32,
}

pub fn run_deform_temporal(
    prev: &Scene,
    cur: &Scene,
    slack: f32,
    stride: u32,
    s: &mut Scratch,
) -> DeformTemporal {
    let cam = &cur.cam;
    let mut o = DeformTemporal {
        pixels: 0,
        hinted: 0,
        verified: 0,
        tunnelled: 0,
        max_closing: 0.0,
        p99_closing: 0.0,
        mean_closing: 0.0,
    };
    let mut closings: Vec<f32> = Vec::new();
    let mut y = 0;
    while y < cam.h {
        let mut x = 0;
        while x < cam.w {
            let d = cam.dir_at_pixel(x as f32 + 0.5, y as f32 + 0.5);
            o.pixels += 1;
            let p0 = march(&prev.tape, cam.eye, d, prev.t_near, prev.t_far, s).0;
            let truth = march(&cur.tape, cam.eye, d, cur.t_near, cur.t_far, s).0;
            if let (Some(a), Some(b)) = (p0, truth) {
                let closing = a - b;
                if closing > 0.0 {
                    closings.push(closing);
                    o.max_closing = o.max_closing.max(closing);
                }
            }
            let hv = match p0 {
                Some(t) => t,
                None => {
                    x += stride;
                    continue;
                }
            };
            o.hinted += 1;
            let start = (hv - slack).max(cur.t_near);
            let got = march(&cur.tape, cam.eye, d, start, cur.t_far, s).0;
            match (truth, got) {
                (Some(a), Some(b)) if (a - b).abs() < 3e-3 * a.max(1.0) => o.verified += 1,
                (None, None) => o.verified += 1,
                _ => o.tunnelled += 1,
            }
            x += stride;
        }
        y += stride;
    }
    if !closings.is_empty() {
        let n = closings.len();
        o.mean_closing = closings.iter().sum::<f32>() / n as f32;
        closings.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        o.p99_closing = closings[(n * 99 / 100).min(n - 1)];
    }
    o
}
