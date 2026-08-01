//! The five evaluation modes over one tape.
//!
//! plans/graphics.md §6.2 is a table of derived programs emitted from one
//! source. The probe hand-writes the five it needs to answer §16.3, which is
//! the point: if these numbers only exist once FieldWir exists, they arrive
//! too late to change anything.
//!
//! | mode | what it stands in for | consumed by |
//! | --- | --- | --- |
//! | [`eval`] | `eval` | marching, ground truth |
//! | [`eval_grad`] | `eval_grad` | normals, Newton, continuation |
//! | [`eval_aff`] | `eval_range` | tile classification, pruning |
//! | [`eval_daff`] | `eval_range` ∘ ∂t | silhouette certification |
//! | [`eval_iv`] | — | the "plain IA is too loose" control |
//!
//! Five near-identical `match` blocks is the intended shape. CLAUDE.md
//! prefers long obvious files to a trait with five implementations, and the
//! duplication is what lets each mode carry its own derivative or enclosure
//! rule without a generic seam.

use crate::aff::{Aff, Iv};
use crate::tape::{Op, Tape};

// ---------------------------------------------------------------------------
// Mode 1: scalar. Ground truth for every other mode.
// ---------------------------------------------------------------------------

/// Polynomial smooth minimum.
///
/// The saturated cases return the winning operand **verbatim** rather than
/// evaluating the formula. That is not micro-optimisation: with `h == 1` the
/// expression reduces to `b + (a − b)`, which is not bit-identical to `a` in
/// floating point. §2.3 states that "`smin` deviates from plain `min` only
/// inside a blend band of width `k`", and §7 makes bit-identity between a
/// pruned tape and the full tape a **hard gate**. Written naively, those two
/// clauses contradict each other by one ulp on every pruned blend — which is
/// exactly what `run_selfcheck` reported (17 mismatches in 7680 samples)
/// before this branch existed.
#[inline]
fn smin_scalar(a: f32, b: f32, k: f32) -> f32 {
    let h = (0.5 + 0.5 * (b - a) / k).clamp(0.0, 1.0);
    if h >= 1.0 {
        return a;
    }
    if h <= 0.0 {
        return b;
    }
    b + (a - b) * h - k * h * (1.0 - h)
}

#[inline]
fn smax_scalar(a: f32, b: f32, k: f32) -> f32 {
    let h = (0.5 + 0.5 * (a - b) / k).clamp(0.0, 1.0);
    if h >= 1.0 {
        return a;
    }
    if h <= 0.0 {
        return b;
    }
    b + (a - b) * h + k * h * (1.0 - h)
}

/// Interval enclosure of a polynomial smooth minimum.
///
/// **Do not evaluate the formula.** `mix(b, a, h)` multiplies the operand
/// difference by an unsaturated `h ∈ [0,1]`, so in the interval domain a
/// blend between two operands 20 apart returns a span of 40 — and a chain of
/// six limb blends compounds it. Measured on the melee scene: a tile of pure
/// sky, 15 units above four figures, enclosed as `[-29.3, +8.0]`, i.e. "you
/// may be 29 units inside a character". That single looseness made *every*
/// tile in the frame report as boundary, so the classifier returned 0.00%
/// exterior on a scene that is 45% sky, and the tape pruned 412 ops to 319
/// where it should have collapsed to the ground plane.
///
/// The algebra gives the answer directly and tightly. `mix` is a convex
/// combination, so `mix ≥ min(a,b)`; the bulge `k·h(1−h)` is at most `k/4`;
/// and the polynomial smooth minimum is by construction never above `min`.
/// Both ends are attained (`a = b` hits the lower, `|a−b| ≥ k` the upper):
///
/// ```text
///     min(a,b) − k/4  ≤  smin(a,b,k)  ≤  min(a,b)
/// ```
#[inline]
fn smin_iv(x: Iv, y: Iv, k: f32) -> Iv {
    let m = x.min(y);
    Iv::new(m.lo - 0.25 * k, m.hi)
}

#[inline]
fn smax_iv(x: Iv, y: Iv, k: f32) -> Iv {
    let m = x.max(y);
    Iv::new(m.lo, m.hi + 0.25 * k)
}

/// Scalar-dual length: `∇|v| = v/|v|`, zero at the origin where it is
/// undefined (the SDF is still continuous there; only its gradient is not).
#[inline]
fn len_d(v: &[D]) -> D {
    let sq: f32 = v.iter().map(|x| x.v * x.v).sum();
    let l = sq.sqrt();
    let inv = if l > 1e-12 { 1.0 / l } else { 0.0 };
    let mut g = [0.0f32; 3];
    for k in 0..3 {
        g[k] = v.iter().map(|x| x.v * x.g[k]).sum::<f32>() * inv;
    }
    D { v: l, g }
}

#[inline]
fn len_iv(v: &[Iv]) -> Iv {
    let mut s = Iv::konst(0.0);
    for x in v {
        s = s.add(x.square());
    }
    s.sqrt()
}

/// Dual-affine length, and the reason [`crate::tape::Op::Len3`] is fused.
///
/// The chain rule through `sqrt` divides by `|v|`, which is *identically
/// zero* over any region inside a box — `length(max(q,0))` is the box SDF's
/// outside term. Lowered, that produced an unbounded derivative enclosure
/// and the interior certificate was refused across most of the frame.
///
/// Cauchy-Schwarz removes the division from the bound entirely:
///
/// ```text
///     |d|v|/dt|  =  |v · v'| / |v|  ≤  |v'|
/// ```
///
/// so wherever `|v|` cannot be bounded away from zero the derivative is
/// still enclosed by `±‖v'‖₂` — finite, tight, and independent of `|v|`.
/// Away from the origin the exact quotient is used, which stays sign-definite
/// and is what lets a cell on one face of a box certify as interior.
#[inline]
fn len_daff(v: &[DAff]) -> DAff {
    let mut sq = Aff::konst(0.0);
    for x in v {
        sq = sq.add(x.v.square());
    }
    let l = sq.sqrt();
    let inv = l.recip();
    if inv.is_finite() {
        let mut dot = Aff::konst(0.0);
        for x in v {
            dot = dot.add(x.v.mul(x.dt));
        }
        DAff {
            v: l,
            dt: dot.mul(inv),
        }
    } else {
        let b = v
            .iter()
            .map(|x| {
                let (lo, hi) = x.dt.interval();
                lo.abs().max(hi.abs())
            })
            .map(|m| m * m)
            .sum::<f32>()
            .sqrt();
        DAff {
            v: l,
            dt: Aff::opaque(-b, b),
        }
    }
}

pub fn eval(tape: &Tape, p: [f32; 3], scratch: &mut Vec<f32>) -> f32 {
    scratch.clear();
    scratch.resize(tape.ops.len(), 0.0);
    for i in 0..tape.ops.len() {
        let v = match tape.ops[i] {
            Op::X => p[0],
            Op::Y => p[1],
            Op::Z => p[2],
            Op::Const(v) => v,
            Op::Neg(a) => -scratch[a as usize],
            Op::Add(a, b) => scratch[a as usize] + scratch[b as usize],
            Op::Sub(a, b) => scratch[a as usize] - scratch[b as usize],
            Op::Mul(a, b) => scratch[a as usize] * scratch[b as usize],
            Op::Square(a) => scratch[a as usize] * scratch[a as usize],
            Op::Sqrt(a) => scratch[a as usize].max(0.0).sqrt(),
            Op::Abs(a) => scratch[a as usize].abs(),
            Op::Min(a, b) => scratch[a as usize].min(scratch[b as usize]),
            Op::Max(a, b) => scratch[a as usize].max(scratch[b as usize]),
            Op::SMin(a, b, k) => smin_scalar(scratch[a as usize], scratch[b as usize], k),
            Op::SMax(a, b, k) => smax_scalar(scratch[a as usize], scratch[b as usize], k),
            Op::Clamp01(a) => scratch[a as usize].clamp(0.0, 1.0),
            Op::Sin(a) => scratch[a as usize].sin(),
            Op::AddC(a, v) => scratch[a as usize] + v,
            Op::MulC(a, v) => scratch[a as usize] * v,
            Op::Rep(a, p) => {
                let x = scratch[a as usize];
                x - p * (x / p).round()
            }
            Op::Len2(a, b) => {
                let (x, y) = (scratch[a as usize], scratch[b as usize]);
                (x * x + y * y).sqrt()
            }
            Op::Len3(a, b, c) => {
                let (x, y, z) = (
                    scratch[a as usize],
                    scratch[b as usize],
                    scratch[c as usize],
                );
                (x * x + y * y + z * z).sqrt()
            }
        };
        scratch[i] = v;
    }
    scratch[tape.root as usize]
}

/// Count of blend nodes whose two operands are within `k` at this point.
///
/// This is §16.3's "ray-length fraction inside blend bands", sampled. A
/// blend node is *in band* exactly when the smooth minimum differs from the
/// plain minimum — i.e. `|a − b| < k` — which is precisely §2.3's condition
/// for "solve, do not march" to fail and marching to be forced.
pub fn eval_blend_active(tape: &Tape, p: [f32; 3], scratch: &mut Vec<f32>) -> (f32, u32) {
    let d = eval(tape, p, scratch);
    let mut n = 0;
    for op in &tape.ops {
        match *op {
            Op::SMin(a, b, k) | Op::SMax(a, b, k) => {
                if (scratch[a as usize] - scratch[b as usize]).abs() < k {
                    n += 1;
                }
            }
            _ => {}
        }
    }
    (d, n)
}

// ---------------------------------------------------------------------------
// Mode 2: scalar forward-mode dual, for ∇f.
// ---------------------------------------------------------------------------

#[derive(Clone, Copy)]
struct D {
    v: f32,
    g: [f32; 3],
}

impl D {
    #[inline]
    fn c(v: f32) -> D {
        D { v, g: [0.0; 3] }
    }
    #[inline]
    fn add(self, o: D) -> D {
        D {
            v: self.v + o.v,
            g: [self.g[0] + o.g[0], self.g[1] + o.g[1], self.g[2] + o.g[2]],
        }
    }
    #[inline]
    fn sub(self, o: D) -> D {
        D {
            v: self.v - o.v,
            g: [self.g[0] - o.g[0], self.g[1] - o.g[1], self.g[2] - o.g[2]],
        }
    }
    #[inline]
    fn scale(self, k: f32) -> D {
        D {
            v: self.v * k,
            g: [self.g[0] * k, self.g[1] * k, self.g[2] * k],
        }
    }
    #[inline]
    fn mul(self, o: D) -> D {
        D {
            v: self.v * o.v,
            g: [
                self.g[0] * o.v + self.v * o.g[0],
                self.g[1] * o.v + self.v * o.g[1],
                self.g[2] * o.v + self.v * o.g[2],
            ],
        }
    }
}

pub fn eval_grad(tape: &Tape, p: [f32; 3], scratch: &mut Vec<(f32, [f32; 3])>) -> (f32, [f32; 3]) {
    scratch.clear();
    scratch.resize(tape.ops.len(), (0.0, [0.0; 3]));
    let get = |s: &Vec<(f32, [f32; 3])>, i: u32| -> D {
        let (v, g) = s[i as usize];
        D { v, g }
    };
    for i in 0..tape.ops.len() {
        let r = match tape.ops[i] {
            Op::X => D {
                v: p[0],
                g: [1.0, 0.0, 0.0],
            },
            Op::Y => D {
                v: p[1],
                g: [0.0, 1.0, 0.0],
            },
            Op::Z => D {
                v: p[2],
                g: [0.0, 0.0, 1.0],
            },
            Op::Const(v) => D::c(v),
            Op::Neg(a) => get(scratch, a).scale(-1.0),
            Op::Add(a, b) => get(scratch, a).add(get(scratch, b)),
            Op::Sub(a, b) => get(scratch, a).sub(get(scratch, b)),
            Op::Mul(a, b) => get(scratch, a).mul(get(scratch, b)),
            Op::Square(a) => {
                let x = get(scratch, a);
                D {
                    v: x.v * x.v,
                    g: [2.0 * x.v * x.g[0], 2.0 * x.v * x.g[1], 2.0 * x.v * x.g[2]],
                }
            }
            Op::Sqrt(a) => {
                let x = get(scratch, a);
                let s = x.v.max(0.0).sqrt();
                let inv = if s > 1e-12 { 0.5 / s } else { 0.0 };
                D {
                    v: s,
                    g: [x.g[0] * inv, x.g[1] * inv, x.g[2] * inv],
                }
            }
            Op::Abs(a) => {
                let x = get(scratch, a);
                let s = if x.v < 0.0 { -1.0 } else { 1.0 };
                x.scale(s)
            }
            Op::Min(a, b) => {
                let (x, y) = (get(scratch, a), get(scratch, b));
                if x.v <= y.v { x } else { y }
            }
            Op::Max(a, b) => {
                let (x, y) = (get(scratch, a), get(scratch, b));
                if x.v >= y.v { x } else { y }
            }
            Op::SMin(a, b, k) => {
                let (x, y) = (get(scratch, a), get(scratch, b));
                smin_d(x, y, k)
            }
            Op::SMax(a, b, k) => {
                let (x, y) = (get(scratch, a), get(scratch, b));
                smin_d(x.scale(-1.0), y.scale(-1.0), k).scale(-1.0)
            }
            Op::Clamp01(a) => {
                let x = get(scratch, a);
                if x.v <= 0.0 {
                    D::c(0.0)
                } else if x.v >= 1.0 {
                    D::c(1.0)
                } else {
                    x
                }
            }
            Op::Sin(a) => {
                let x = get(scratch, a);
                let c = x.v.cos();
                D {
                    v: x.v.sin(),
                    g: [x.g[0] * c, x.g[1] * c, x.g[2] * c],
                }
            }
            Op::AddC(a, v) => {
                let x = get(scratch, a);
                D { v: x.v + v, g: x.g }
            }
            Op::MulC(a, v) => get(scratch, a).scale(v),
            Op::Rep(a, p) => {
                let x = get(scratch, a);
                // Derivative of x − p·round(x/p) is 1 wherever it is defined.
                D {
                    v: x.v - p * (x.v / p).round(),
                    g: x.g,
                }
            }
            Op::Len2(a, b) => len_d(&[get(scratch, a), get(scratch, b)]),
            Op::Len3(a, b, c) => len_d(&[get(scratch, a), get(scratch, b), get(scratch, c)]),
        };
        scratch[i] = (r.v, r.g);
    }
    scratch[tape.root as usize]
}

#[inline]
fn smin_d(a: D, b: D, k: f32) -> D {
    let t = b.sub(a).scale(0.5 / k);
    let raw = t.v + 0.5;
    // Saturated: the winner's value *and* derivative, verbatim. Same reason
    // as `smin_scalar` — outside the band a blend is its operand exactly.
    if raw >= 1.0 {
        return a;
    }
    if raw <= 0.0 {
        return b;
    }
    let h = D { v: raw, g: t.g };
    // b + (a−b)·h − k·h·(1−h)
    let lin = b.add(a.sub(b).mul(h));
    let one = D::c(1.0);
    let bulge = h.mul(one.sub(h)).scale(k);
    lin.sub(bulge)
}

// ---------------------------------------------------------------------------
// Mode 3: affine, per-slot. The enclosure §2.1 and §2.2 both run on.
// ---------------------------------------------------------------------------

/// Affine evaluation with a plain interval carried alongside and intersected
/// at every slot.
///
/// Affine arithmetic is not uniformly tighter than interval arithmetic, and
/// distance fields hit its worst case constantly. `min(x, 0)` — the `inside`
/// term of every box SDF — is *exact* under intervals: anything positive
/// collapses to `0`. Under affine arithmetic it routes through `abs`, whose
/// Chebyshev line leaves a symmetric error band that cannot collapse, so a
/// slot whose true width is 0.04 comes back at 1.34.
///
/// Measured on the colonnade scene before this function intersected: plain
/// IA proved 70% of cells empty against affine arithmetic's 27%, with AA
/// enclosures ~20× wider at the root. Reported as-is that would have read as
/// a refutation of §2.1, when it is really a refutation of implementing §2.1
/// without the interval every AA implementation in the literature carries.
///
/// Both enclosures are sound, so their intersection is sound — but the two
/// domains must propagate **independently**. Writing the intersection back
/// into the affine form collapses it to an opaque interval at the first slot
/// where IA wins, and from there the whole tape degenerates to plain IA:
/// measured, that produced an AA/IA width ratio of exactly 1.00× at every
/// slot, which is not agreement, it is the affine domain having been
/// switched off. So `out_aff` carries pure affine arithmetic with its
/// correlation intact, `out_iv` carries the per-slot intersection, and
/// decisions (pruning, classification) read `out_iv`.
pub fn eval_aff(tape: &Tape, p: [Aff; 3], out: &mut Vec<Aff>, out_iv: &mut Vec<Iv>) {
    out.clear();
    out.resize(tape.ops.len(), Aff::konst(0.0));
    out_iv.clear();
    out_iv.resize(tape.ops.len(), Iv::konst(0.0));
    for i in 0..tape.ops.len() {
        let iv = |j: u32, o: &Vec<Iv>| -> Iv { o[j as usize] };
        // The interval-domain value of this slot, computed from the already
        // intersected intervals of its inputs.
        let vi: Iv = match tape.ops[i] {
            Op::X => Iv::new(p[0].lo(), p[0].hi()),
            Op::Y => Iv::new(p[1].lo(), p[1].hi()),
            Op::Z => Iv::new(p[2].lo(), p[2].hi()),
            Op::Const(v) => Iv::konst(v),
            Op::Neg(a) => iv(a, out_iv).neg(),
            Op::Add(a, b) => iv(a, out_iv).add(iv(b, out_iv)),
            Op::Sub(a, b) => iv(a, out_iv).sub(iv(b, out_iv)),
            Op::Mul(a, b) => iv(a, out_iv).mul(iv(b, out_iv)),
            Op::Square(a) => iv(a, out_iv).square(),
            Op::Sqrt(a) => iv(a, out_iv).sqrt(),
            Op::Abs(a) => iv(a, out_iv).abs(),
            Op::Min(a, b) => iv(a, out_iv).min(iv(b, out_iv)),
            Op::Max(a, b) => iv(a, out_iv).max(iv(b, out_iv)),
            Op::SMin(a, b, k) => smin_iv(iv(a, out_iv), iv(b, out_iv), k),
            Op::SMax(a, b, k) => smax_iv(iv(a, out_iv), iv(b, out_iv), k),
            Op::Clamp01(a) => iv(a, out_iv).clamp01(),
            Op::Sin(a) => iv(a, out_iv).sin(),
            Op::AddC(a, v) => iv(a, out_iv).add(Iv::konst(v)),
            Op::MulC(a, v) => iv(a, out_iv).mul(Iv::konst(v)),
            Op::Rep(a, v) => iv(a, out_iv).rep(v),
            Op::Len2(a, b) => len_iv(&[iv(a, out_iv), iv(b, out_iv)]),
            Op::Len3(a, b, c) => len_iv(&[iv(a, out_iv), iv(b, out_iv), iv(c, out_iv)]),
        };
        let v = match tape.ops[i] {
            Op::X => p[0],
            Op::Y => p[1],
            Op::Z => p[2],
            Op::Const(v) => Aff::konst(v),
            Op::Neg(a) => out[a as usize].neg(),
            Op::Add(a, b) => out[a as usize].add(out[b as usize]),
            Op::Sub(a, b) => out[a as usize].sub(out[b as usize]),
            Op::Mul(a, b) => out[a as usize].mul(out[b as usize]),
            Op::Square(a) => out[a as usize].square(),
            Op::Sqrt(a) => out[a as usize].sqrt(),
            Op::Abs(a) => out[a as usize].abs(),
            Op::Min(a, b) => out[a as usize].min(out[b as usize]),
            Op::Max(a, b) => out[a as usize].max(out[b as usize]),
            Op::SMin(a, b, k) => smin_aff(out[a as usize], out[b as usize], k),
            Op::SMax(a, b, k) => smin_aff(out[a as usize].neg(), out[b as usize].neg(), k).neg(),
            Op::Clamp01(a) => out[a as usize].clamp01(),
            Op::Sin(a) => out[a as usize].sin(),
            Op::AddC(a, v) => out[a as usize].add_c(v),
            Op::MulC(a, v) => out[a as usize].mul_c(v),
            Op::Rep(a, v) => out[a as usize].rep(v),
            Op::Len2(a, b) => out[a as usize]
                .square()
                .add(out[b as usize].square())
                .sqrt(),
            Op::Len3(a, b, c) => out[a as usize]
                .square()
                .add(out[b as usize].square())
                .add(out[c as usize].square())
                .sqrt(),
        };
        out[i] = v;
        // The decision interval: the tighter of the two sound enclosures.
        let (alo, ahi) = v.interval();
        out_iv[i] = Iv::new(alo.max(vi.lo), ahi.min(vi.hi));
    }
}

/// Fused smooth-min enclosure.
///
/// The whole reason `SMin` is one op: `b − a` is formed once, as a single
/// affine form, so the correlation between the two operands survives into
/// `h`. Lowering this to primitives before enclosing would decorrelate the
/// difference and make every blend look like a blown bound — which would
/// have shown up as a (false) refutation of §2.2 on the character scene.
#[inline]
fn smin_aff(a: Aff, b: Aff, k: f32) -> Aff {
    let h = b.sub(a).mul_c(0.5 / k).add_c(0.5).clamp01();
    let lin = b.add(a.sub(b).mul(h));
    // h(1−h) ∈ [0, 1/4] for h ∈ [0,1]; pinning that keeps the bulge term
    // from re-widening what `clamp01` just bounded.
    let bulge = h
        .mul(Aff::konst(1.0).sub(h))
        .intersect_opaque(0.0, 0.25)
        .mul_c(k);
    lin.sub(bulge)
}

// ---------------------------------------------------------------------------
// Mode 4: affine + ∂/∂t. The silhouette certificate.
// ---------------------------------------------------------------------------

/// An affine value paired with an affine bound on its `t`-derivative.
///
/// §2.1 classifies a tile as *boundary*; that is not the same as *interior*
/// in §16.3's sense. A tile is certifiably interior only when its visible
/// sheet carries no silhouette, and the silhouette set is
/// `f = 0 ∧ ∂f/∂t = 0`. So the interior fraction needs a rigorous enclosure
/// of the directional derivative, not just of the value — which is why this
/// mode exists rather than being deferred with the rest of the gradient
/// work.
#[derive(Clone, Copy)]
pub struct DAff {
    pub v: Aff,
    pub dt: Aff,
}

impl DAff {
    #[inline]
    pub fn konst(v: f32) -> DAff {
        DAff {
            v: Aff::konst(v),
            dt: Aff::konst(0.0),
        }
    }
    #[inline]
    fn add(self, o: DAff) -> DAff {
        DAff {
            v: self.v.add(o.v),
            dt: self.dt.add(o.dt),
        }
    }
    #[inline]
    fn sub(self, o: DAff) -> DAff {
        DAff {
            v: self.v.sub(o.v),
            dt: self.dt.sub(o.dt),
        }
    }
    #[inline]
    fn mul_c(self, k: f32) -> DAff {
        DAff {
            v: self.v.mul_c(k),
            dt: self.dt.mul_c(k),
        }
    }
    #[inline]
    fn mul(self, o: DAff) -> DAff {
        DAff {
            v: self.v.mul(o.v),
            dt: self.dt.mul(o.v).add(self.v.mul(o.dt)),
        }
    }
}

/// Lipschitz bound used to cap the directional derivative.
///
/// `∂f/∂t = ∇f·d̂` with `|d̂| = 1`, so `|∂f/∂t| ≤ ‖∇f‖`, and §6.4 makes
/// `‖∇d‖ ≤ 1` a *diagnostic-enforced* property of every field — a field that
/// violates it makes the marcher overstep. The margin above 1 covers the
/// scenes' displacement terms, whose gradient contribution is bounded by
/// `Σ amplitude·frequency` by construction.
///
/// This is an assumption about the scenes, not a derived bound, so it is
/// named here rather than buried: if a scene were authored that broke it,
/// the interior certificate would be unsound rather than merely loose.
pub const LIPSCHITZ: f32 = 1.25;

/// Dual-affine evaluation.
///
/// `bounds` supplies the intersected value enclosures from [`eval_aff`] over
/// the same region and the same tape, so the value component inherits the
/// interval tightening without duplicating the interval domain here.
pub fn eval_daff(tape: &Tape, p: [DAff; 3], bounds: &[Iv], out: &mut Vec<DAff>) -> DAff {
    out.clear();
    out.resize(tape.ops.len(), DAff::konst(0.0));
    debug_assert_eq!(bounds.len(), tape.ops.len());
    for i in 0..tape.ops.len() {
        let r = match tape.ops[i] {
            Op::X => p[0],
            Op::Y => p[1],
            Op::Z => p[2],
            Op::Const(v) => DAff::konst(v),
            Op::Neg(a) => out[a as usize].mul_c(-1.0),
            Op::Add(a, b) => out[a as usize].add(out[b as usize]),
            Op::Sub(a, b) => out[a as usize].sub(out[b as usize]),
            Op::Mul(a, b) => out[a as usize].mul(out[b as usize]),
            Op::Square(a) => {
                let x = out[a as usize];
                DAff {
                    v: x.v.square(),
                    dt: x.v.mul(x.dt).mul_c(2.0),
                }
            }
            Op::Sqrt(a) => {
                let x = out[a as usize];
                let s = x.v.sqrt();
                // d/dt sqrt(v) = dt / (2·sqrt(v)); ambiguous as v → 0.
                let inv = s.recip();
                let dt = if inv.is_finite() {
                    x.dt.mul(inv).mul_c(0.5)
                } else {
                    let r = x.dt.rad() + x.dt.c.abs();
                    Aff::opaque(-r * 1e3, r * 1e3)
                };
                DAff { v: s, dt }
            }
            Op::Abs(a) => {
                let x = out[a as usize];
                let (lo, hi) = x.v.interval();
                if lo >= 0.0 {
                    x
                } else if hi <= 0.0 {
                    x.mul_c(-1.0)
                } else {
                    // Sign is not determined over this region, so neither is
                    // the derivative. Carry the ambiguity.
                    DAff {
                        v: x.v.abs(),
                        dt: x.dt.hull(x.dt.neg()),
                    }
                }
            }
            Op::Min(a, b) => sel_daff(out[a as usize], out[b as usize], true),
            Op::Max(a, b) => sel_daff(out[a as usize], out[b as usize], false),
            Op::SMin(a, b, k) => smin_daff(out[a as usize], out[b as usize], k),
            Op::SMax(a, b, k) => {
                smin_daff(out[a as usize].mul_c(-1.0), out[b as usize].mul_c(-1.0), k).mul_c(-1.0)
            }
            Op::Clamp01(a) => {
                let x = out[a as usize];
                let (lo, hi) = x.v.interval();
                if lo >= 0.0 && hi <= 1.0 {
                    x
                } else if lo >= 1.0 {
                    DAff::konst(1.0)
                } else if hi <= 0.0 {
                    DAff::konst(0.0)
                } else {
                    DAff {
                        v: x.v.clamp01(),
                        dt: x.dt.hull(Aff::konst(0.0)),
                    }
                }
            }
            Op::Sin(a) => {
                let x = out[a as usize];
                DAff {
                    v: x.v.sin(),
                    dt: x.dt.mul(x.v.cos()),
                }
            }
            Op::AddC(a, v) => {
                let x = out[a as usize];
                DAff {
                    v: x.v.add_c(v),
                    dt: x.dt,
                }
            }
            Op::MulC(a, v) => out[a as usize].mul_c(v),
            Op::Rep(a, v) => {
                let x = out[a as usize];
                DAff {
                    v: x.v.rep(v),
                    dt: x.dt,
                }
            }
            Op::Len2(a, b) => len_daff(&[out[a as usize], out[b as usize]]),
            Op::Len3(a, b, c) => len_daff(&[out[a as usize], out[b as usize], out[c as usize]]),
        };
        // The value enclosure tightens at every slot — `bounds[i]` is that
        // slot's own. The Lipschitz cap applies only at the root: `‖∇d‖ ≤ 1`
        // is a property of the *distance field*, not of every intermediate,
        // and a `Square` slot's derivative is legitimately unbounded.
        out[i] = DAff {
            v: r.v.intersect_opaque(bounds[i].lo, bounds[i].hi),
            dt: if i as u32 == tape.root {
                r.dt.intersect_opaque(-LIPSCHITZ, LIPSCHITZ)
            } else {
                r.dt
            },
        };
    }
    out[tape.root as usize]
}

/// `min`/`max` in the dual-affine domain.
///
/// When the branches are separable the derivative comes from the winner
/// exactly — that exactness is what makes an interior certificate possible
/// through a CSG tree. When they are not, the value still gets the tight
/// affine `min`, but the derivative degrades to the hull, which is the
/// honest answer: a tile straddling a CSG seam has no single sheet.
#[inline]
fn sel_daff(a: DAff, b: DAff, is_min: bool) -> DAff {
    let (alo, ahi) = a.v.interval();
    let (blo, bhi) = b.v.interval();
    if is_min {
        if ahi <= blo {
            return a;
        }
        if bhi <= alo {
            return b;
        }
        DAff {
            v: a.v.min(b.v),
            dt: a.dt.hull(b.dt),
        }
    } else {
        if alo >= bhi {
            return a;
        }
        if blo >= ahi {
            return b;
        }
        DAff {
            v: a.v.max(b.v),
            dt: a.dt.hull(b.dt),
        }
    }
}

#[inline]
fn smin_daff(a: DAff, b: DAff, k: f32) -> DAff {
    let t = b.sub(a).mul_c(0.5 / k);
    let raw_v = t.v.add_c(0.5);
    let (lo, hi) = raw_v.interval();
    let h = if lo >= 0.0 && hi <= 1.0 {
        DAff { v: raw_v, dt: t.dt }
    } else if lo >= 1.0 {
        DAff::konst(1.0)
    } else if hi <= 0.0 {
        DAff::konst(0.0)
    } else {
        DAff {
            v: raw_v.clamp01(),
            dt: t.dt.hull(Aff::konst(0.0)),
        }
    };
    let lin = b.add(a.sub(b).mul(h));
    let one = DAff::konst(1.0);
    let bulge = h.mul(one.sub(h)).mul_c(k);
    lin.sub(bulge)
}

// ---------------------------------------------------------------------------
// Mode 5: plain interval. The control for §2.1's "IA is too loose" claim.
// ---------------------------------------------------------------------------

pub fn eval_iv(tape: &Tape, p: [Iv; 3], out: &mut Vec<Iv>) -> Iv {
    out.clear();
    out.resize(tape.ops.len(), Iv::konst(0.0));
    for i in 0..tape.ops.len() {
        let v = match tape.ops[i] {
            Op::X => p[0],
            Op::Y => p[1],
            Op::Z => p[2],
            Op::Const(v) => Iv::konst(v),
            Op::Neg(a) => out[a as usize].neg(),
            Op::Add(a, b) => out[a as usize].add(out[b as usize]),
            Op::Sub(a, b) => out[a as usize].sub(out[b as usize]),
            Op::Mul(a, b) => out[a as usize].mul(out[b as usize]),
            Op::Square(a) => out[a as usize].square(),
            Op::Sqrt(a) => out[a as usize].sqrt(),
            Op::Abs(a) => out[a as usize].abs(),
            Op::Min(a, b) => out[a as usize].min(out[b as usize]),
            Op::Max(a, b) => out[a as usize].max(out[b as usize]),
            Op::SMin(a, b, k) => smin_iv(out[a as usize], out[b as usize], k),
            Op::SMax(a, b, k) => smax_iv(out[a as usize], out[b as usize], k),
            Op::Clamp01(a) => out[a as usize].clamp01(),
            Op::Sin(a) => out[a as usize].sin(),
            Op::AddC(a, v) => out[a as usize].add(Iv::konst(v)),
            Op::MulC(a, v) => out[a as usize].mul(Iv::konst(v)),
            Op::Rep(a, v) => out[a as usize].rep(v),
            Op::Len2(a, b) => len_iv(&[out[a as usize], out[b as usize]]),
            Op::Len3(a, b, c) => len_iv(&[out[a as usize], out[b as usize], out[c as usize]]),
        };
        out[i] = v;
    }
    out[tape.root as usize]
}
