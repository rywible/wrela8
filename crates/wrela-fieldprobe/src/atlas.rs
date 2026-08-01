//! The baked atlas: an octree of certified analytic solves.
//!
//! plans/graphics.md §13 rejects bakes on three grounds — staleness,
//! dynamism, redundancy — and the first is the real one: "a second
//! representation that can drift from the truth". This module is what a bake
//! looks like when that objection is answered rather than dodged.
//!
//! Every cell stores a degree-2 polynomial proxy for the field *plus a
//! certified error bound* `ε`, derived at bake time from the same affine
//! machinery §2.1 runs. At trace time the proxy gives a closed-form root; the
//! true pruned tape then verifies it. A proxy that lies cannot produce a
//! wrong pixel — it can only produce a wasted verification and a fallback to
//! marching. So the bake is an *accelerator with a proof*, not a second
//! source of truth, and `run_atlas`'s soundness gate compares every pixel
//! against an independent march to keep it honest.
//!
//! Staleness dies for a structural reason as well: the image is recompiled
//! whole on every update ([01 §1](../../../docs/language/01-model.md)), so a
//! baked cell cannot outlive the expression it was baked from.
//!
//! What the atlas precomputes, in the order it matters:
//!
//! 1. **The pruning.** §2.2's tape reduction is a function of *region*, not
//!    of camera. The probe re-derives it every frame (10.8% of the measured
//!    frame); for static structure the answer never changes.
//! 2. **Empty space, aggregated.** A large node proved empty is skipped in
//!    one ray-box test, where sphere tracing pays a step per unbounding
//!    sphere. This is the answer to §2.5's "linear asymptotic crawl".
//! 3. **The solve.** §2.3 says "solve, do not march" but only for subtrees
//!    that are polynomial by construction. A certified quadratic proxy makes
//!    *every* cell polynomial, including inside `smin` blend bands — which is
//!    exactly where §16.2's worst-case scene spends its time.

use crate::aff::{Aff, Iv};
use crate::eval::{eval, eval_aff};
use crate::probe::{Scratch, march};
use crate::prune::prune;
use crate::tape::Tape;

#[derive(Clone, Copy, Debug)]
pub struct Aabb {
    pub lo: [f32; 3],
    pub hi: [f32; 3],
}

impl Aabb {
    #[inline]
    pub fn centre(&self) -> [f32; 3] {
        [
            0.5 * (self.lo[0] + self.hi[0]),
            0.5 * (self.lo[1] + self.hi[1]),
            0.5 * (self.lo[2] + self.hi[2]),
        ]
    }
    #[inline]
    pub fn half(&self) -> [f32; 3] {
        [
            0.5 * (self.hi[0] - self.lo[0]),
            0.5 * (self.hi[1] - self.lo[1]),
            0.5 * (self.hi[2] - self.lo[2]),
        ]
    }
    #[inline]
    pub fn diag(&self) -> f32 {
        let h = self.half();
        (h[0] * h[0] + h[1] * h[1] + h[2] * h[2]).sqrt()
    }
    pub fn child(&self, i: usize) -> Aabb {
        let c = self.centre();
        let mut lo = self.lo;
        let mut hi = self.hi;
        for k in 0..3 {
            if i & (1 << k) == 0 {
                hi[k] = c[k];
            } else {
                lo[k] = c[k];
            }
        }
        Aabb { lo, hi }
    }
    /// Slab test. Returns the entry/exit parameters clipped to `[t0, t1]`.
    #[inline]
    pub fn hit(&self, o: [f32; 3], inv: [f32; 3], t0: f32, t1: f32) -> Option<(f32, f32)> {
        let mut a = t0;
        let mut b = t1;
        for k in 0..3 {
            let x0 = (self.lo[k] - o[k]) * inv[k];
            let x1 = (self.hi[k] - o[k]) * inv[k];
            let (n, f) = if x0 <= x1 { (x0, x1) } else { (x1, x0) };
            a = a.max(n);
            b = b.min(f);
            if a > b {
                return None;
            }
        }
        Some((a, b))
    }
}

/// A degree-2 trivariate proxy in cell-local coordinates normalised to
/// `[-1,1]³`, with a certified sup-norm error bound.
#[derive(Clone)]
pub struct Proxy {
    /// `1, x, y, z, x², y², z², xy, xz, yz`
    pub c: [f32; 10],
    pub eps: f32,
    pub tape: u16,
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Kind {
    /// Provably contains no surface.
    Empty,
    /// Provably interior to a solid. Carries a tape so a ray that reaches it
    /// can refine back onto the surface instead of reporting the cell face:
    /// terminating at `t0` quantises the hit to the cell size, which showed
    /// up as 7,513 disagreements against ground truth.
    Full(u16),
    /// Certified quadratic proxy.
    Proxy(u32),
    /// Proxy did not certify at the depth cap: march the pruned tape here.
    Live(u16),
    /// First of eight children.
    Branch(u32),
}

pub struct Atlas {
    pub bounds: Aabb,
    pub nodes: Vec<Kind>,
    pub proxies: Vec<Proxy>,
    pub tapes: Vec<Tape>,
    // --- bake statistics -------------------------------------------------
    pub n_empty: u64,
    pub n_full: u64,
    pub n_proxy: u64,
    pub n_live: u64,
    pub n_branch: u64,
    pub deepest: u32,
    /// Field evaluations spent baking, so the bake's own cost is on record.
    pub bake_evals: u64,
}

impl Atlas {
    pub fn bytes(&self) -> usize {
        self.nodes.len() * 8
            + self.proxies.len() * std::mem::size_of::<Proxy>()
            + self.tapes.iter().map(|t| t.ops.len() * 16).sum::<usize>()
    }
}

/// Least squares over the normal equations, `N`×`N`, partial pivoting.
fn solve_n(mut a: Vec<Vec<f64>>, mut b: Vec<f64>) -> Option<Vec<f64>> {
    let n = b.len();
    for c in 0..n {
        let mut piv = c;
        for r in c + 1..n {
            if a[r][c].abs() > a[piv][c].abs() {
                piv = r;
            }
        }
        if a[piv][c].abs() < 1e-10 {
            return None;
        }
        a.swap(c, piv);
        b.swap(c, piv);
        for r in c + 1..n {
            let f = a[r][c] / a[c][c];
            for k in c..n {
                a[r][k] -= f * a[c][k];
            }
            b[r] -= f * b[c];
        }
    }
    let mut x = vec![0.0; n];
    for r in (0..n).rev() {
        let mut s = b[r];
        for k in r + 1..n {
            s -= a[r][k] * x[k];
        }
        x[r] = s / a[r][r];
    }
    Some(x)
}

#[inline]
fn basis(p: [f32; 3]) -> [f64; 10] {
    let (x, y, z) = (p[0] as f64, p[1] as f64, p[2] as f64);
    [1.0, x, y, z, x * x, y * y, z * z, x * y, x * z, y * z]
}

#[inline]
pub fn eval_proxy(c: &[f32; 10], p: [f32; 3]) -> f32 {
    let (x, y, z) = (p[0], p[1], p[2]);
    c[0] + c[1] * x
        + c[2] * y
        + c[3] * z
        + c[4] * x * x
        + c[5] * y * y
        + c[6] * z * z
        + c[7] * x * y
        + c[8] * x * z
        + c[9] * y * z
}

/// Fit a quadratic to the field over `bb` and bound its residual.
///
/// **Where the certificate actually lives.** The first version of this tried
/// to certify the proxy itself with a Lipschitz argument — sample the
/// residual on a grid of spacing `h`, inflate by `(L_f + L_p)·h√3/2`. That is
/// rigorous and it is useless: driving the inflation under ε needs sample
/// spacing ~ε/L, so an ε of 0.004 wants ~80 samples per axis *per cell*. It
/// certified zero cells out of 66,538.
///
/// The fix is to notice the certificate was in the wrong place. Correctness
/// never depended on the proxy: `Empty` and `Full` come from affine
/// arithmetic and are rigorous, and every proxy root is **verified against
/// the true pruned tape** before it is believed, with marching as the
/// fallback. So the proxy is a *seed*, and its residual bound is a quality
/// heuristic that decides whether seeding is worth it — not a soundness
/// property. What is certified is the thing that actually skips work: empty
/// space.
///
/// The residual is still Lipschitz-inflated, so `eps` remains an upper
/// bound; it is simply compared against a fraction of the cell diagonal
/// rather than an absolute tolerance.
fn fit_proxy(
    tape: &Tape,
    bb: &Aabb,
    fit_n: usize,
    chk_n: usize,
    s: &mut Scratch,
    evals: &mut u64,
) -> Option<([f32; 10], f32)> {
    let c = bb.centre();
    let h = bb.half();
    let to_world = |u: [f32; 3]| [c[0] + u[0] * h[0], c[1] + u[1] * h[1], c[2] + u[2] * h[2]];

    let mut ata = vec![vec![0.0f64; 10]; 10];
    let mut atb = vec![0.0f64; 10];
    let step = 2.0 / (fit_n - 1) as f32;
    for i in 0..fit_n {
        for j in 0..fit_n {
            for k in 0..fit_n {
                let u = [
                    -1.0 + i as f32 * step,
                    -1.0 + j as f32 * step,
                    -1.0 + k as f32 * step,
                ];
                let v = eval(tape, to_world(u), &mut s.f) as f64;
                *evals += 1;
                let b = basis(u);
                for r in 0..10 {
                    for cc in 0..10 {
                        ata[r][cc] += b[r] * b[cc];
                    }
                    atb[r] += b[r] * v;
                }
            }
        }
    }
    let sol = solve_n(ata, atb)?;
    let mut coef = [0.0f32; 10];
    for i in 0..10 {
        coef[i] = sol[i] as f32;
        if !coef[i].is_finite() {
            return None;
        }
    }

    // Residual sup-norm on a denser grid, then the Lipschitz inflation.
    let mut worst = 0.0f32;
    let cstep = 2.0 / (chk_n - 1) as f32;
    for i in 0..chk_n {
        for j in 0..chk_n {
            for k in 0..chk_n {
                let u = [
                    -1.0 + i as f32 * cstep,
                    -1.0 + j as f32 * cstep,
                    -1.0 + k as f32 * cstep,
                ];
                let w = to_world(u);
                let f = eval(tape, w, &mut s.f);
                *evals += 1;
                worst = worst.max((f - eval_proxy(&coef, u)).abs());
            }
        }
    }
    // The sampled residual, with no Lipschitz inflation.
    //
    // Inflating was correct when `eps` was load-bearing for correctness. It
    // is not any more — verification against the true tape is — and the
    // inflation term `(L_f + L_p)·h√3/2` is ~0.058 on a depth-8 cell whose
    // whole diagonal is 0.128, so it swamped the quantity it was inflating
    // and rejected 100% of proxies. An estimate is the right tool for a
    // seeding decision; a bound was the right tool for a claim nothing
    // downstream makes any more.
    Some((coef, worst))
}

pub struct BakeCfg {
    pub max_depth: u32,
    /// Accept a proxy when its residual is under this fraction of the cell
    /// diagonal **and** under `eps_abs`. Both are needed: a fraction alone
    /// let the *root* node accept a quadratic over the whole 38-unit scene
    /// box (residual 3 against a threshold of 4.9), which is scale-relative
    /// nonsense. The absolute cap is what ties acceptance to the thing that
    /// actually matters — whether the proxy root lands inside a Newton basin
    /// of the true surface.
    pub eps_frac: f32,
    pub eps_abs: f32,
    pub fit_n: usize,
    pub chk_n: usize,
}

impl Default for BakeCfg {
    fn default() -> Self {
        BakeCfg {
            max_depth: 11,
            eps_frac: 0.10,
            eps_abs: 0.01,
            fit_n: 4,
            chk_n: 6,
        }
    }
}

pub fn bake(tape: &Tape, bounds: Aabb, cfg: &BakeCfg, s: &mut Scratch) -> Atlas {
    let mut at = Atlas {
        bounds,
        nodes: Vec::new(),
        proxies: Vec::new(),
        tapes: Vec::new(),
        n_empty: 0,
        n_full: 0,
        n_proxy: 0,
        n_live: 0,
        n_branch: 0,
        deepest: 0,
        bake_evals: 0,
    };
    at.nodes.push(Kind::Empty);
    let root = bake_node(tape, bounds, 0, cfg, &mut at, s);
    at.nodes[0] = root;
    at
}

fn palette(at: &mut Atlas, t: &Tape) -> u16 {
    // Tape palettes are small because topology is comptime (§6.3): a scene
    // has finitely many distinct pruned tapes, and cells share them.
    for (i, e) in at.tapes.iter().enumerate() {
        if e.root == t.root && e.ops == t.ops {
            return i as u16;
        }
    }
    at.tapes.push(t.clone());
    (at.tapes.len() - 1) as u16
}

fn bake_node(
    tape: &Tape,
    bb: Aabb,
    depth: u32,
    cfg: &BakeCfg,
    at: &mut Atlas,
    s: &mut Scratch,
) -> Kind {
    at.deepest = at.deepest.max(depth);
    let c = bb.centre();
    let h = bb.half();
    let p = [
        Aff::sym(0, c[0] - h[0], c[0] + h[0]),
        Aff::sym(1, c[1] - h[1], c[1] + h[1]),
        Aff::sym(2, c[2] - h[2], c[2] + h[2]),
    ];
    let (mut aff, mut ivs): (Vec<Aff>, Vec<Iv>) = (Vec::new(), Vec::new());
    eval_aff(tape, p, &mut aff, &mut ivs);
    at.bake_evals += tape.weight() as u64;
    let r = ivs[tape.root as usize];
    if r.lo > 0.0 {
        at.n_empty += 1;
        return Kind::Empty;
    }
    let pr = prune(tape, &ivs);
    if r.hi < 0.0 {
        at.n_full += 1;
        return Kind::Full(palette(at, &pr.tape));
    }

    if depth < cfg.max_depth {
        if let Some((coef, eps)) =
            fit_proxy(&pr.tape, &bb, cfg.fit_n, cfg.chk_n, s, &mut at.bake_evals)
        {
            if eps <= cfg.eps_frac * bb.diag() && eps <= cfg.eps_abs {
                let t = palette(at, &pr.tape);
                at.proxies.push(Proxy {
                    c: coef,
                    eps,
                    tape: t,
                });
                at.n_proxy += 1;
                return Kind::Proxy((at.proxies.len() - 1) as u32);
            }
        }
        // Not certified at this size: subdivide.
        let base = at.nodes.len() as u32;
        for _ in 0..8 {
            at.nodes.push(Kind::Empty);
        }
        at.n_branch += 1;
        for i in 0..8 {
            let k = bake_node(&pr.tape, bb.child(i), depth + 1, cfg, at, s);
            at.nodes[base as usize + i] = k;
        }
        return Kind::Branch(base);
    }

    at.n_live += 1;
    Kind::Live(palette(at, &pr.tape))
}

// ---------------------------------------------------------------------------
// Trace
// ---------------------------------------------------------------------------

#[derive(Default, Clone, Copy)]
pub struct TraceCost {
    /// Ray-box tests plus child ordering.
    pub node_visits: u64,
    /// Closed-form quadratic solves.
    pub solves: u64,
    /// True-tape evaluations spent verifying a proxy root.
    pub verify_ops: u64,
    /// Marching inside cells the proxy could not certify.
    pub march_ops: u64,
    /// Proxy roots the true field disagreed with.
    pub verify_fail: u64,
    /// Rays that left the atlas and fell back to live marching.
    pub escaped: u64,
    /// Rays that reached a provably-interior cell. A primary ray should
    /// cross a boundary cell first, so this counts upstream misses.
    pub full_entries: u64,
}

/// FLOP charged per node visit: a 3-axis slab test plus child ordering.
pub const NODE_FLOP: f64 = 18.0;
/// FLOP charged for building and rooting the univariate quadratic.
pub const SOLVE_FLOP: f64 = 34.0;

impl TraceCost {
    pub fn flop(&self) -> f64 {
        self.node_visits as f64 * NODE_FLOP
            + self.solves as f64 * SOLVE_FLOP
            + self.verify_ops as f64
            + self.march_ops as f64
    }
}

/// March inside one cell with that cell's pruned tape.
///
/// Not [`crate::probe::march`], because the shared marcher accepts any
/// `dist < HIT_EPS·t` — including a large *negative* distance. Sampled
/// exactly on a cell boundary a pruned tape can return a large negative
/// (pruning is only valid strictly inside its cell), and the shared marcher
/// reads that as "hit, here", quantising the intersection to the cell face.
/// That single behaviour produced every one of the 4,086 depth disagreements
/// against ground truth, and it shrank with cell size (worst 1.41 world units
/// at depth 9, 0.22 at depth 11) exactly as a quantisation artefact should.
///
/// So: reject a start that is implausibly inside, refine it back onto the
/// surface, and require `|f|` small — not `f` small — to call it a hit.
fn march_cell(
    tape: &Tape,
    o: [f32; 3],
    d: [f32; 3],
    t0: f32,
    t1: f32,
    s: &mut Scratch,
) -> (Option<f32>, u32) {
    let mut t = t0;
    let mut steps = 0u32;
    while t <= t1 && steps < 96 {
        let q = [o[0] + t * d[0], o[1] + t * d[1], o[2] + t * d[2]];
        let f = eval(tape, q, &mut s.f);
        steps += 1;
        if f.abs() < 1e-4 * t.max(1.0) {
            return (Some(t), steps);
        }
        if f < 0.0 {
            // Overshot (or entered already inside): Newton back onto it.
            t += f;
            if t < t0 - 1e-3 {
                return (None, steps);
            }
            continue;
        }
        t += f.max(1e-5);
    }
    (None, steps)
}

/// Smallest root of `A t² + B t + C` in `[t0, t1]`, or `None`.
#[inline]
fn quad_root(a: f32, b: f32, c: f32, t0: f32, t1: f32) -> Option<f32> {
    if a.abs() < 1e-9 {
        if b.abs() < 1e-12 {
            return None;
        }
        let t = -c / b;
        return if t >= t0 && t <= t1 { Some(t) } else { None };
    }
    let disc = b * b - 4.0 * a * c;
    if disc < 0.0 {
        return None;
    }
    let sq = disc.sqrt();
    // Numerically stable pair.
    let q = -0.5 * (b + b.signum() * sq);
    let (r0, r1) = (
        q / a,
        if q.abs() > 1e-20 {
            c / q
        } else {
            f32::INFINITY
        },
    );
    let (lo, hi) = if r0 <= r1 { (r0, r1) } else { (r1, r0) };
    if lo >= t0 && lo <= t1 {
        Some(lo)
    } else if hi >= t0 && hi <= t1 {
        Some(hi)
    } else {
        None
    }
}

#[allow(clippy::too_many_arguments)]
fn trace_node(
    at: &Atlas,
    node: Kind,
    bb: Aabb,
    o: [f32; 3],
    d: [f32; 3],
    inv: [f32; 3],
    t0: f32,
    t1: f32,
    cost: &mut TraceCost,
    s: &mut Scratch,
) -> Option<f32> {
    cost.node_visits += 1;
    match node {
        Kind::Empty => None,
        // Inside a solid. The surface is behind the entry point, so refine
        // back onto it with the true tape rather than returning the cell
        // face — the face is only accurate to one cell width.
        Kind::Full(ti) => {
            cost.full_entries += 1;
            let tape = &at.tapes[ti as usize];
            let mut t = t0;
            for _ in 0..4 {
                let q = [o[0] + t * d[0], o[1] + t * d[1], o[2] + t * d[2]];
                let f = eval(tape, q, &mut s.f);
                cost.verify_ops += tape.weight() as u64;
                if f.abs() < 1e-4 * t.max(1.0) {
                    return Some(t);
                }
                t += f;
            }
            Some(t0)
        }
        Kind::Proxy(i) => {
            let px = &at.proxies[i as usize];
            let c = bb.centre();
            let h = bb.half();
            // Ray in cell-local normalised coordinates.
            let lo = [
                (o[0] - c[0]) / h[0],
                (o[1] - c[1]) / h[1],
                (o[2] - c[2]) / h[2],
            ];
            let ld = [d[0] / h[0], d[1] / h[1], d[2] / h[2]];
            let k = &px.c;
            let a = k[4] * ld[0] * ld[0]
                + k[5] * ld[1] * ld[1]
                + k[6] * ld[2] * ld[2]
                + k[7] * ld[0] * ld[1]
                + k[8] * ld[0] * ld[2]
                + k[9] * ld[1] * ld[2];
            let b = k[1] * ld[0]
                + k[2] * ld[1]
                + k[3] * ld[2]
                + 2.0 * k[4] * lo[0] * ld[0]
                + 2.0 * k[5] * lo[1] * ld[1]
                + 2.0 * k[6] * lo[2] * ld[2]
                + k[7] * (lo[0] * ld[1] + lo[1] * ld[0])
                + k[8] * (lo[0] * ld[2] + lo[2] * ld[0])
                + k[9] * (lo[1] * ld[2] + lo[2] * ld[1]);
            let cc = eval_proxy(k, lo);
            cost.solves += 1;

            // The true surface lies where the proxy is within ε of zero, so
            // bracket on ±ε rather than on the bare root.
            let tape = &at.tapes[px.tape as usize];
            let root = quad_root(a, b, cc - 0.0, t0, t1)
                .or_else(|| quad_root(a, b, cc - px.eps, t0, t1))
                .or_else(|| quad_root(a, b, cc + px.eps, t0, t1));
            if let Some(tr) = root {
                // Verify against the expression: two Newton steps on the
                // true pruned tape. The proxy is an accelerator, never an
                // authority.
                let mut t = tr;
                for _ in 0..3 {
                    // A pruned tape is only valid inside its own cell, so a
                    // Newton step that leaves the cell invalidates the very
                    // evaluation used to test convergence. Range-check first,
                    // then converge — the other order accepts roots found
                    // with a tape that does not describe the field there.
                    if t < t0 - 1e-4 || t > t1 + 1e-4 {
                        break;
                    }
                    let q = [o[0] + t * d[0], o[1] + t * d[1], o[2] + t * d[2]];
                    let f = eval(tape, q, &mut s.f);
                    cost.verify_ops += tape.weight() as u64;
                    if f.abs() < 1e-4 * t.max(1.0) {
                        return Some(t);
                    }
                    t += f;
                }
                cost.verify_fail += 1;
            }
            // Certificate held but the root was not in this cell, or the
            // verification wandered: fall back to marching, which is always
            // correct and is what the ε bound guarantees we can do.
            let (hit, steps) = march_cell(tape, o, d, t0, t1, s);
            cost.march_ops += steps as u64 * tape.weight() as u64;
            hit
        }
        Kind::Live(ti) => {
            let tape = &at.tapes[ti as usize];
            let (hit, steps) = march_cell(tape, o, d, t0, t1, s);
            cost.march_ops += steps as u64 * tape.weight() as u64;
            hit
        }
        Kind::Branch(base) => {
            // Visit children in ray order; the first hit wins.
            let mut order: [(f32, usize); 8] = [(f32::INFINITY, 0); 8];
            let mut n = 0;
            for i in 0..8 {
                let cb = bb.child(i);
                if let Some((a, b)) = cb.hit(o, inv, t0, t1) {
                    order[n] = (a, i);
                    n += 1;
                    let _ = b;
                }
            }
            order[..n].sort_by(|x, y| x.0.partial_cmp(&y.0).unwrap_or(std::cmp::Ordering::Equal));
            for &(_, i) in &order[..n] {
                let cb = bb.child(i);
                if let Some((a, b)) = cb.hit(o, inv, t0, t1) {
                    if let Some(t) = trace_node(
                        at,
                        at.nodes[base as usize + i],
                        cb,
                        o,
                        d,
                        inv,
                        a,
                        b,
                        cost,
                        s,
                    ) {
                        return Some(t);
                    }
                }
            }
            None
        }
    }
}

/// Trace a ray through the atlas. Falls back to the live tape outside it.
pub fn trace(
    at: &Atlas,
    full: &Tape,
    o: [f32; 3],
    d: [f32; 3],
    t_near: f32,
    t_far: f32,
    cost: &mut TraceCost,
    s: &mut Scratch,
) -> Option<f32> {
    let inv = [
        1.0 / if d[0].abs() < 1e-9 {
            1e-9f32.copysign(d[0])
        } else {
            d[0]
        },
        1.0 / if d[1].abs() < 1e-9 {
            1e-9f32.copysign(d[1])
        } else {
            d[1]
        },
        1.0 / if d[2].abs() < 1e-9 {
            1e-9f32.copysign(d[2])
        } else {
            d[2]
        },
    ];
    if let Some((a, b)) = at.bounds.hit(o, inv, t_near, t_far) {
        if let Some(t) = trace_node(at, at.nodes[0], at.bounds, o, d, inv, a, b, cost, s) {
            return Some(t);
        }
        // Cleared the atlas without hitting: geometry may still lie beyond it
        // (the ground plane runs to the horizon), so the live tape finishes
        // the ray. Counted, because an atlas that only covers the near field
        // is an atlas that flatters itself.
        if b < t_far {
            cost.escaped += 1;
            let (hit, steps) = march(full, o, d, b, t_far, s);
            cost.march_ops += steps as u64 * full.weight() as u64;
            return hit;
        }
        return None;
    }
    cost.escaped += 1;
    let (hit, steps) = march(full, o, d, t_near, t_far, s);
    cost.march_ops += steps as u64 * full.weight() as u64;
    hit
}
