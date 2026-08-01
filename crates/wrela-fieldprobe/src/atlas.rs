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

#[derive(Clone)]
pub struct Proxy {
    pub c: [f32; 10],
    pub eps: f32,
    pub tape: u16,
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Kind {
    Empty,
    Full(u16),
    Proxy(u32),
    Live(u16),
    Branch(u32),
}

pub struct Atlas {
    pub bounds: Aabb,
    pub nodes: Vec<Kind>,
    pub proxies: Vec<Proxy>,
    pub tapes: Vec<Tape>,
    pub n_empty: u64,
    pub n_full: u64,
    pub n_proxy: u64,
    pub n_live: u64,
    pub n_branch: u64,
    pub deepest: u32,
    pub bake_evals: u64,
}

impl Atlas {
    pub fn bytes(&self) -> usize {
        self.nodes.len() * 8
            + self.proxies.len() * std::mem::size_of::<Proxy>()
            + self.tapes.iter().map(|t| t.ops.len() * 16).sum::<usize>()
    }
}

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
    Some((coef, worst))
}

pub struct BakeCfg {
    pub max_depth: u32,
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

#[derive(Default, Clone, Copy)]
pub struct TraceCost {
    pub node_visits: u64,
    pub solves: u64,
    pub verify_ops: u64,
    pub march_ops: u64,
    pub verify_fail: u64,
    pub escaped: u64,
    pub full_entries: u64,
}

pub const NODE_FLOP: f64 = 18.0;
pub const SOLVE_FLOP: f64 = 34.0;

impl TraceCost {
    pub fn flop(&self) -> f64 {
        self.node_visits as f64 * NODE_FLOP
            + self.solves as f64 * SOLVE_FLOP
            + self.verify_ops as f64
            + self.march_ops as f64
    }
}

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

            let tape = &at.tapes[px.tape as usize];
            let root = quad_root(a, b, cc - 0.0, t0, t1)
                .or_else(|| quad_root(a, b, cc - px.eps, t0, t1))
                .or_else(|| quad_root(a, b, cc + px.eps, t0, t1));
            if let Some(tr) = root {
                let mut t = tr;
                for _ in 0..3 {
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
