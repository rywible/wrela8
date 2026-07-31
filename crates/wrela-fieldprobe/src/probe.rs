//! The experiments. Every output here is a *count*.
//!
//! plans/graphics.md §16.1 splits the metrics into those that port off the
//! M4 proxy and those that do not. This module measures only the first kind:
//! tape lengths, area fractions, ray-length fractions, eval counts, hit
//! rates. It deliberately does not time anything — "achieved GFLOP/s as a
//! fraction of peak" is on §16.1's *do not port* list, and the counts→cycles
//! conversion belongs to the pinned `bench/a76-pi5.toml` table, not here.

use crate::aff::{Aff, Iv};
use crate::camera::Camera;
use crate::eval::{eval, eval_aff, eval_blend_active, eval_daff, eval_grad, eval_iv, DAff};
use crate::prune::{prune, Pruned};
use crate::scene::Scene;
use crate::tape::Tape;

/// Deterministic xorshift. No `rand`, no clock — CLAUDE.md's determinism
/// doctrine applies to the instrument as much as to the compiler, or the
/// numbers are not re-derivable.
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
    /// Uniform in `[0, 1)`.
    #[inline]
    pub fn unit(&mut self) -> f32 {
        (self.next_u32() >> 8) as f32 / 16_777_216.0
    }
    #[inline]
    pub fn range(&mut self, lo: f32, hi: f32) -> f32 {
        lo + (hi - lo) * self.unit()
    }
}

// ---------------------------------------------------------------------------
// Shared scratch, so an inner loop never allocates.
// ---------------------------------------------------------------------------

#[derive(Default)]
pub struct Scratch {
    pub f: Vec<f32>,
    pub g: Vec<(f32, [f32; 3])>,
    pub a: Vec<Aff>,
    pub av: Vec<Iv>,
    pub d: Vec<DAff>,
    pub i: Vec<Iv>,
}

// ---------------------------------------------------------------------------
// E1 — tile classification, pruning by depth, edge cells.
// ---------------------------------------------------------------------------

#[derive(Default, Clone)]
pub struct DepthStat {
    pub cells: u64,
    pub ops_sum: u64,
    pub weight_sum: u64,
    pub blends_sum: u64,
    pub ops_max: usize,
    /// Live op counts, kept so the report can quote a median rather than
    /// only a mean — a mean hides the tail that decides whether the L1
    /// budget in §5 holds.
    pub ops_hist: Vec<u32>,
}

#[derive(Default)]
pub struct ClassifyOut {
    pub per_depth: Vec<DepthStat>,
    /// Screen area (px²) by outcome.
    pub area_exterior: f64,
    pub area_interior: f64,
    pub area_unresolved: f64,
    pub area_edge: f64,
    /// Leaf cells carrying a silhouette or CSG seam.
    pub edge_cells: u64,
    pub leaf_px: f32,
    /// Cells the enclosure could not evaluate finitely — must be zero.
    pub nonfinite: u64,
    /// Interior leaf cells, kept for E5/E6.
    pub interior_cells: Vec<InteriorCell>,
    /// Why the interior certificate failed, by cause. §16.3's "instrument it
    /// like a fuzz lane": a low interior fraction is only a finding once you
    /// know which clause rejected it.
    pub fail_dt_straddles: u64,
    pub fail_faces: u64,
    /// Screen area that *is* a single smooth sheet, decided by marching
    /// rather than by proving.
    ///
    /// The certificate is a lower bound and a weak one: `min(m, 0)` inside a
    /// box SDF has an ambiguous derivative at the face, even though that
    /// non-smoothness cancels against the `max(q,0)` term in the sum. Fixing
    /// that needs fused box/cylinder primitives with analytic gradients.
    /// Until then, measuring the truth separately brackets the answer —
    /// certified area is what §2.1 delivers today, empirical area is what a
    /// perfect certificate could deliver, and the cost model reports both.
    pub area_interior_empirical: f64,
    /// Every leaf, with the tape that survived pruning there. The frame-cost
    /// model marches these directly, so its FLOP count is measured rather
    /// than extrapolated from a mean.
    pub leaves: Vec<Leaf>,
    /// Affine-domain op-evaluations spent on traversal (weight × evals).
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
    /// Pruned over the found slab: what the renderer executes there.
    pub tape: Tape,
    /// Pruned over `[t0, t_far]`: the fallback when the slab march misses
    /// and the ray must carry on to the next slab.
    pub tape_wide: Tape,
    pub t_far: f32,
    /// The `t` range the pruned tape is valid over. Outside it the tape is
    /// simply wrong — pruning deleted branches that provably lose *in this
    /// slab*, not everywhere — so a renderer marches the pruned tape only
    /// here, and so does the cost model.
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

/// Recursive tile classification with per-cell tape pruning.
///
/// The child recurses on the *pruned* tape, which is the whole point of
/// §2.2: the cost of `map()` falls with subdivision depth because the tape
/// shortens on the way down, and the shortening compounds.
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
    // Only the on-screen part of a cell counts. The base tiling is a whole
    // number of tiles and the screen usually is not (288/64 = 4.5), so
    // unclamped areas sum to 111% of the frame and every "fraction of screen
    // area" below is quietly inflated.
    let vx = (x0 + size).min(cam.w as f32) - x0;
    let vy = (y0 + size).min(cam.h as f32) - y0;
    if vx <= 0.0 || vy <= 0.0 {
        return;
    }
    let area = (vx * vy) as f64;
    let (u0, u1) = (cam.u_of(x0), cam.u_of(x0 + size));
    let (v0, v1) = (cam.v_of(y0 + size), cam.v_of(y0));

    // --- find the first t-slab that can contain a surface ------------------
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

    // --- prune over this cell ---------------------------------------------
    //
    // The pruning region is `[ta, t_hi]`, **not** the slab `[ta, tb]` that
    // was just found. A pruned tape is only valid over the region it was
    // pruned for: outside it, a deleted branch can be the true minimum. The
    // children recurse on this tape and sweep out to `t_hi`, so pruning to
    // the narrower slab lets a child prove "no surface here" with a tape
    // that no longer contains the surface.
    //
    // That was a real bug, and it is exactly the kind that flatters: it
    // reported 37.8% of the colonnade frame as provably empty, of which
    // 15,012 pixels (10% of the frame) were hit by the marcher. The
    // `exterior_hits` gate in `run_framecost` exists to catch it, and does.
    // Two prunings, because they answer two different questions.
    //
    // `pr_wide` is pruned over `[ta, t_hi]` — everything the children will
    // ever look at. Children recurse on it, because a tape is only valid
    // over the region it was pruned for and a child that sweeps past `tb`
    // with a slab-pruned tape can prove "empty" using a tape from which the
    // surface has been deleted. That bug reported 37.8% of the colonnade as
    // provably empty while the marcher hit 15,012 of those pixels; the
    // `exterior_hits` gate catches it.
    //
    // `pr_cell` is pruned over the found slab `[ta, tb]` alone. That is the
    // tape a renderer actually executes inside the cell, so it is what §2.2's
    // "pruned tape length by depth" means and what the frame cost charges.
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

    // --- interior certificate ---------------------------------------------
    // A cell is certifiably interior when the field is strictly monotone in
    // t across the whole wedge (no silhouette: ∂f/∂t ≠ 0), *and* the near
    // face is entirely outside while the far face is entirely inside. Then
    // every ray in the cell has exactly one crossing and the visible surface
    // is a single smooth sheet — §16.3's "resolved as interior, no ray
    // traced". Bisect t to give the face test a chance to succeed.
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
            // ∂f/∂t may vanish: a silhouette is possible here, so there is
            // no single sheet to certify.
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
            out.interior_cells.push(InteriorCell { x0, y0, size, t0: lo, t1: hi });
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
        // Narrow toward the crossing: keep the half that still brackets it.
        let mid = 0.5 * (lo + hi);
        let m = cam.slice(u0, u1, v0, v1, mid);
        eval_aff(&pr.tape, m, &mut s.a, &mut s.av);
        let mf = s.av[pr.tape.root as usize];
        if mf.lo <= 0.0 && mf.hi >= 0.0 {
            // The tile's own depth spread exceeds what bisection can
            // separate: the mid-depth screen slice is partly in front of the
            // surface and partly behind. Monotone in t, but not resolvable
            // as one flat slab — the certificate wants a *fitted* sheet, not
            // an axis-aligned one.
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

    // --- subdivide or give up ---------------------------------------------
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

    // Leaf, uncertified. Is it an edge (silhouette or CSG seam)?
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
    // Ground truth for §2.1's actual claim — "resolved from tile-corner
    // depths plus a Newton polish". March the corners and the centre; if
    // bilinear interpolation of the corner depths predicts the centre to
    // under half a pixel of parallax, the cell really is one smooth sheet,
    // whatever the certificate managed to prove.
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
    for (i, &(dx, dy)) in [(0.0, 0.0), (1.0, 0.0), (0.0, 1.0), (1.0, 1.0)].iter().enumerate() {
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

// ---------------------------------------------------------------------------
// E2/E3 — marching: evals per pixel, blend-band ray fraction.
// ---------------------------------------------------------------------------

pub struct MarchOut {
    pub rays: u64,
    pub hits: u64,
    pub evals: u64,
    pub steps_max: u32,
    /// Uniformly-sampled ray length inside a blend band, and the total.
    pub band_len: f64,
    pub total_len: f64,
    /// Same, restricted to rays that hit.
    pub band_len_hit: f64,
    pub total_len_hit: f64,
    pub band_samples: u64,
    pub band_active_samples: u64,
}

const HIT_EPS: f32 = 1e-4;
const MAX_STEPS: u32 = 192;

/// Naive sphere trace, the §1 baseline. No over-relaxation, no segment
/// tracing — §1's "pre-pruning landing zone" assumes the classical
/// amortisations, so the count reported here is the thing those
/// amortisations have to beat.
pub fn march(tape: &Tape, o: [f32; 3], d: [f32; 3], t_near: f32, t_far: f32, s: &mut Scratch) -> (Option<f32>, u32) {
    let mut t = t_near;
    let mut steps = 0;
    while t < t_far && steps < MAX_STEPS {
        let p = [o[0] + t * d[0], o[1] + t * d[1], o[2] + t * d[2]];
        let dist = eval(tape, p, &mut s.f);
        steps += 1;
        if dist < HIT_EPS * t.max(1.0) {
            return (Some(t), steps);
        }
        t += dist.max(HIT_EPS);
    }
    (None, steps)
}

/// March a subsampled grid and measure §16.3's blend-band ray fraction.
///
/// The band measurement uses *uniform* samples along the traversed segment,
/// not march steps: march steps bunch up near the surface, which is exactly
/// where blends live, so stepping-weighted sampling would inflate the
/// fraction and flatter §2.3's opposition.
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

// ---------------------------------------------------------------------------
// E4 — affine vs plain interval tightness.
// ---------------------------------------------------------------------------

pub struct TightOut {
    pub cells: u64,
    /// Cells affine arithmetic proves empty that interval arithmetic cannot.
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
                // The same wedge as an axis-aligned interval box: this is
                // the fairest available comparison, since plain IA has no
                // way to represent the wedge's correlation at all.
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

// ---------------------------------------------------------------------------
// E5 — continuation on the hit manifold vs marching.
// ---------------------------------------------------------------------------

pub struct ContinuationOut {
    pub samples: u64,
    pub converged: u64,
    pub diverged: u64,
    /// Eval-equivalents: an `eval_grad` counts as `GRAD_COST` evals.
    pub cont_evals: f64,
    pub march_evals: f64,
    pub max_err_px: f32,
}

/// Cost of one forward-mode gradient relative to one value evaluation.
///
/// Forward-mode over a 3-component dual carries four numbers where `eval`
/// carries one, but the value work is shared and the derivative of most ops
/// is one FMA. 3.0 is the conservative (continuation-unfriendly) end.
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

/// Walk the surface across a certified-interior cell instead of marching
/// each pixel from scratch.
///
/// `∂t/∂u = −F_u / F_t` with `F_t = ∇f·d̂` and `F_u = ∇f·(t ∂d̂/∂u)`, both
/// from one `eval_grad`. Predictor from the previous sample's screen-space
/// depth gradient, corrector by Newton in `t`.
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
        // Seed the walk with one honest march at the left edge.
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

            // Predictor: one gradient at the previous solution.
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

            // Corrector: up to two Newton steps in t.
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

            // Ground truth: an independent march for the same pixel.
            let (truth, msteps) = march(&sc.tape, cam.eye, dh2, sc.t_near, sc.t_far, s);
            o.march_evals += msteps as f64;
            o.samples += 1;
            match truth {
                Some(tt) if ok && (tn - tt).abs() < 2.0 * HIT_EPS * tt.max(1.0) + 1e-3 => {
                    o.converged += 1;
                    // Screen-space error of the reconstructed depth, in
                    // pixels of parallax at this depth.
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

// ---------------------------------------------------------------------------
// E6 — the reconstruction factor: how far apart may samples be?
// ---------------------------------------------------------------------------

pub struct ReconOut {
    pub cell_px: f32,
    pub tested: u64,
    pub passed: u64,
}

/// Fit a quadratic `t(x,y)` over a cell from a 3×3 sample grid and measure
/// the worst residual on a 7×7 check grid, in pixels of parallax.
///
/// This is the empirical stand-in for `eval_hess`-driven sample placement:
/// if a quadratic patch reconstructs the depth of an `N×N` block to under
/// half a pixel, the renderer needs 9 samples for `N²` pixels, and the
/// reconstruction factor is `N²/9`.
pub fn run_reconstruction_capped(
    sc: &Scene,
    cell_px: f32,
    cap: u64,
    s: &mut Scratch,
) -> ReconOut {
    let cam = &sc.cam;
    let mut o = ReconOut { cell_px, tested: 0, passed: 0 };
    let nx = (cam.w as f32 / cell_px) as u32;
    let ny = (cam.h as f32 / cell_px) as u32;
    // Deterministic stride rather than a random sample: a fixed lattice is
    // reproducible and cannot be re-rolled until it flatters.
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
            // 3×3 fit samples.
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
                    if let (Some(t), _) = march(&sc.tape, cam.eye, d, sc.t_near, sc.t_far, s)
                    {
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
                            rhs[r] += b[r] * t as f64;
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
            let tol = 0.5 * foot;
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
                    let e = (fit as f32 - truth).abs();
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

/// Gaussian elimination with partial pivoting on the 6×6 normal equations.
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

// ---------------------------------------------------------------------------
// E7 — reprojection hit rate and disocclusion area (§4.4).
// ---------------------------------------------------------------------------

pub struct ReprojOut {
    pub pixels: u64,
    pub hinted: u64,
    pub verified: u64,
    pub disoccluded: u64,
    pub tunnelled: u64,
}

/// Forward-scatter frame N−1's hit points into frame N, then check whether
/// starting the march at `t_hint − slack` reaches the same surface.
///
/// §4 promises a wrong hint costs performance and never correctness — but
/// only for static geometry, and only when `slack` covers the motion. This
/// counts how often the hint exists at all (the complement is disocclusion)
/// and how often it verifies.
pub fn run_reprojection(sc: &Scene, stride: u32, slack: f32, s: &mut Scratch) -> ReprojOut {
    let (c0, c1) = (&sc.cam, &sc.cam2);
    let w = (c1.w / stride) as usize;
    let h = (c1.h / stride) as usize;
    let mut hint = vec![f32::INFINITY; w * h];

    // Scatter.
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
                        let (ix, iy) = ((px as u32 / stride) as usize, (py as u32 / stride) as usize);
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

    // Verify.
    let mut o = ReprojOut { pixels: 0, hinted: 0, verified: 0, disoccluded: 0, tunnelled: 0 };
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

// ---------------------------------------------------------------------------
// Self-checks. Nothing above means anything until these pass.
// ---------------------------------------------------------------------------

pub struct SelfCheck {
    pub samples: u64,
    pub containment_failures: u64,
    pub prune_mismatches: u64,
    pub grad_max_rel_err: f32,
    pub grad_p99_rel_err: f32,
    /// Samples skipped because the field is not differentiable there — a
    /// CSG kink or a repetition boundary between the two difference points.
    /// Reported rather than hidden: it is the fraction of the scene where
    /// §2.5's Newton refinement has no gradient to use.
    pub grad_kink_skips: u64,
    pub grad_tested: u64,
    /// Mean (affine enclosure width) / (sampled true span). ≥ 1 by
    /// definition; how far above 1 is how loose the instrument is.
    pub mean_overwidth: f64,
    pub overwidth_n: u64,
}

/// Sample inside random wedges and verify: the affine enclosure contains the
/// truth, the pruned tape agrees with the full tape bit-for-bit, and the
/// analytic gradient matches a central difference.
///
/// The bit-identity clause is §7's `diff-eval` gate applied to the
/// instrument: a pruning bug that deletes a live branch would otherwise show
/// up as a spectacular §2.2 result.
pub fn run_selfcheck(sc: &Scene, cells: u32, pts: u32, rng: &mut Rng, s: &mut Scratch) -> SelfCheck {
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

        // Containment is checked against the *decision* interval — the
        // intersected one that pruning and classification actually read.
        let (lo, hi) = (r.lo, r.hi);
        let mut tmin = f32::INFINITY;
        let mut tmax = f32::NEG_INFINITY;
        for _ in 0..pts {
            // Sample the wedge in its own parameterisation, so the point is
            // guaranteed to be inside the region the enclosure covers.
            let u = rng.range(u0.min(u1), u0.max(u1));
            let v = rng.range(v0.min(v1), v0.max(v1));
            let t = rng.range(t0, t1);
            let d = cam.dir(u, v);
            let q = [cam.eye[0] + t * d[0], cam.eye[1] + t * d[1], cam.eye[2] + t * d[2]];

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
                // A central difference is only a valid check where the field
                // is differentiable. Across a CSG kink or a repetition
                // boundary the two-sided slope is genuinely neither
                // one-sided derivative, and comparing them measures nothing.
                // The second difference separates the cases: it is O(f''·ε²)
                // on a smooth patch and O(Δslope·ε) at a kink.
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
    // Median, not mean: enclosure width has a long tail, and a mean lets one
    // pathological wedge stand in for the instrument's typical tightness.
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

/// A tiny PPM of the scene, so the geometry can be eyeballed. Not an oracle
/// — but a probe whose scenes are not what their author thinks they are is
/// measuring nothing, and this is the cheapest way to notice.
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

// ---------------------------------------------------------------------------
// E8 — modelled frame cost, and the resolution it buys.
// ---------------------------------------------------------------------------

/// Cost of one affine-domain op relative to one scalar FLOP.
///
/// An `Aff` is five floats and the evaluator carries a plain interval beside
/// it, so an affine `add` is ~7 scalar ops and an affine `mul` ~18. 8.0 is a
/// deliberately unflattering single number for the mix; §16.1's discipline is
/// that this factor is *stated* and swept, not buried. `run_framecost`'s
/// output scales linearly in it and traversal is a minority of the frame, so
/// a 2× error here moves the final resolution by well under 20%.
const AA_OP_COST: f64 = 8.0;

/// §1's shading model, unchanged. These are the doc's own numbers and are
/// not what this probe is testing — replacing them with measurements is a
/// separate exercise (§8's lighting stack does not exist yet).
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
    /// Split out of `primary`: cost of carrying on past the cell's own slab
    /// with the weakly-pruned wide tape. A renderer that re-classified per
    /// slab would pay less than this, so it is the conservative end of the
    /// bracket, and it is reported separately rather than blended in.
    pub primary_fallback: f64,
    pub shadow: f64,
    pub ao_gi: f64,
    pub shade: f64,
    pub post: f64,
    /// Mean marching steps over pixels that had to march.
    pub mean_steps: f64,
    /// Pixels the classifier proved empty that the marcher nonetheless hits.
    /// Must be zero.
    pub exterior_hits: u64,
    /// Same frame, costed as if the interior certificate were perfect —
    /// every empirically-smooth cell resolved from corner depths instead of
    /// marched. The gap between this and `total()` is what a stronger §2.1
    /// certificate is worth.
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
    /// The optimistic end of the bracket: a renderer that re-classifies each
    /// slab as it advances, so a ray that misses in its cell never marches
    /// the weakly-pruned wide tape. It would pay more traversal than modelled
    /// here and less marching; this end charges neither, so the truth sits
    /// between `per_pixel()` and this.
    pub fn per_pixel_optimistic(&self) -> f64 {
        (self.total() - self.primary_fallback) / self.pixels.max(1) as f64
    }
}

/// Cost the frame by marching every pixel with the tape that actually
/// survived pruning in its own leaf.
///
/// This is the point of keeping the pruned tapes: §1's cost model multiplies
/// "evals per pixel" by "FLOP per eval" as two independent averages, and on a
/// pruned renderer they are strongly anti-correlated — the pixels that need
/// the most steps are the ones whose tape pruned the least. Multiplying the
/// means overstates the frame. This measures the product directly.
pub fn run_framecost(sc: &Scene, cl: &ClassifyOut, s: &mut Scratch) -> FrameCost {
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
        total_ideal: 0.0,
    };
    let mut covered = 0u64;
    let mut step_sum = 0u64;
    let mut ideal_primary = 0.0f64;
    // Empirically-smooth area is reported as a fraction; apply it as the
    // share of marched pixels a perfect certificate would have lifted.
    let total_area = (cam.w as f64) * (cam.h as f64);
    let liftable = ((cl.area_interior_empirical - cl.area_interior).max(0.0) / total_area)
        .clamp(0.0, 1.0);

    for lf in &cl.leaves {
        let w = lf.tape.weight() as f64;
        let x1 = (lf.x0 + lf.size).min(cam.w as f32);
        let y1 = (lf.y0 + lf.size).min(cam.h as f32);
        let mut x = lf.x0;
        while x < x1 {
            let mut y = lf.y0;
            while y < y1 {
                covered += 1;
                let d = cam.dir_at_pixel(x + 0.5, y + 0.5);
                // The renderer enters the leaf already knowing the slab that
                // brackets the surface — marching from `t_near` would charge
                // the frame for traversal the classifier already paid for.
                // Slab-local march with the tight tape; on a miss, carry on
                // to the far plane with the wide one. Both are charged.
                let (lo, hi) = (lf.t0.max(sc.t_near), lf.t1.min(sc.t_far));
                let (mut hit, steps) = march(&lf.tape, cam.eye, d, lo, hi, s);
                let mut extra = 0.0f64;
                if hit.is_none() && hi < lf.t_far {
                    let (h2, st2) = march(&lf.tape_wide, cam.eye, d, hi, lf.t_far, s);
                    hit = h2;
                    extra = st2 as f64 * lf.tape_wide.weight() as f64;
                    fc.primary_fallback += extra;
                }
                if lf.class == Class::Interior {
                    fc.interior_px += 1;
                    fc.primary += 2.0 * 3.0 * w + extra;
                    ideal_primary += 2.0 * 3.0 * w + extra;
                } else {
                    fc.marched_px += 1;
                    step_sum += steps as u64;
                    fc.primary += steps as f64 * w + extra;
                    // A lifted pixel costs the Newton polish instead.
                    ideal_primary += (1.0 - liftable) * (steps as f64 * w + extra)
                        + liftable * 2.0 * 3.0 * w;
                }
                if hit.is_some() {
                    fc.hits += 1;
                    fc.shadow += SHADOW_EVALS * w;
                    fc.ao_gi += AO_GI_EVALS * w;
                    fc.shade += SHADE_FLOP;
                }
                fc.post += POST_FLOP;
                y += 1.0;
            }
            x += 1.0;
        }
    }
    fc.exterior_px = fc.pixels.saturating_sub(covered);
    // Soundness gate. A pixel with no leaf was *proved* to contain no
    // surface; if the full tape finds one there, the enclosure lied and
    // every area fraction in this report is void.
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
                if march(&sc.tape, cam.eye, d, sc.t_near, sc.t_far, s).0.is_some() {
                    fc.exterior_hits += 1;
                }
            }
        }
    }
    // Exterior pixels still pay post-processing.
    fc.post += fc.exterior_px as f64 * POST_FLOP;
    fc.mean_steps = step_sum as f64 / fc.marched_px.max(1) as f64;
    fc.total_ideal =
        fc.traversal + ideal_primary + fc.shadow + fc.ao_gi + fc.shade + fc.post;
    fc
}
