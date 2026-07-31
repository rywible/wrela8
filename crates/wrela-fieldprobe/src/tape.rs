//! The field as a flat SSA tape, plus a builder for authoring scenes.
//!
//! plans/graphics.md §2.2 (tape pruning, after Keeter 2020) deletes losing
//! `min`/`max` branches *and everything feeding them*, which is a liveness
//! walk over a flat tape rather than a tree rewrite. So the tape, not a
//! tree, is the probe's representation: op counts here are the numbers §16.3
//! asks for.
//!
//! Every slot's operands have strictly smaller indices, so evaluation is one
//! forward pass and liveness is one backward pass.

use std::collections::BTreeMap;

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Op {
    /// The evaluation point's components.
    X,
    Y,
    Z,
    Const(f32),
    Neg(u32),
    Add(u32, u32),
    Sub(u32, u32),
    Mul(u32, u32),
    /// `x*x` — kept distinct from `Mul(a,a)` because the affine enclosure of
    /// a square is materially tighter (see `aff::Aff::square`).
    Square(u32),
    Sqrt(u32),
    Abs(u32),
    Min(u32, u32),
    Max(u32, u32),
    /// Fused polynomial smooth-min with blend width `k`.
    ///
    /// Fused rather than lowered for two reasons: the enclosure of the fused
    /// form is tighter than the enclosure of its decomposition, and §16.3's
    /// "ray-length fraction inside blend bands" is only measurable if the
    /// blend nodes still exist as nodes at march time.
    SMin(u32, u32, f32),
    SMax(u32, u32, f32),
    Clamp01(u32),
    Sin(u32),
    AddC(u32, f32),
    MulC(u32, f32),
    /// Domain repetition, `x - p*round(x/p)`.
    Rep(u32, f32),
    /// Fused `sqrt(a² + b²)`.
    Len2(u32, u32),
    /// Fused `sqrt(a² + b² + c²)`.
    ///
    /// Fused rather than lowered because the *derivative* of a length is
    /// where the naive chain falls apart. `d/dt sqrt(g)` is `g'/(2√g)`, and
    /// every box SDF evaluates `length(max(q,0))`, which is identically zero
    /// inside the box — so `√g → 0` and the enclosure of the derivative
    /// explodes, even though the true derivative is bounded (a distance
    /// field is 1-Lipschitz). Cauchy-Schwarz gives the bound directly:
    /// `|d|v|/dt| = |v·v'| / |v| ≤ |v'|`, with no reference to `|v|` at all.
    /// See `eval::eval_daff`.
    Len3(u32, u32, u32),
}

impl Op {
    /// Operand slots, for the liveness walk.
    pub fn inputs(&self) -> ([u32; 3], usize) {
        match *self {
            Op::X | Op::Y | Op::Z | Op::Const(_) => ([0, 0, 0], 0),
            Op::Neg(a)
            | Op::Square(a)
            | Op::Sqrt(a)
            | Op::Abs(a)
            | Op::Clamp01(a)
            | Op::Sin(a)
            | Op::AddC(a, _)
            | Op::MulC(a, _)
            | Op::Rep(a, _) => ([a, 0, 0], 1),
            Op::Add(a, b)
            | Op::Sub(a, b)
            | Op::Mul(a, b)
            | Op::Min(a, b)
            | Op::Max(a, b)
            | Op::SMin(a, b, _)
            | Op::SMax(a, b, _)
            | Op::Len2(a, b) => ([a, b, 0], 2),
            Op::Len3(a, b, c) => ([a, b, c], 3),
        }
    }

    /// Rough FLOP-equivalent weight on an A76 NEON pipe.
    ///
    /// Deliberately a *count* model, not a cycle model. §16.1 is explicit
    /// that counts port off the M4 proxy and timings do not, and that the
    /// counts→time conversion is the job of the pinned `bench/a76-pi5.toml`
    /// table. The probe stops at counts on purpose; wiring these into the
    /// cost table is a later, separately-reviewed step.
    pub fn weight(&self) -> u32 {
        match self {
            Op::X | Op::Y | Op::Z | Op::Const(_) => 0,
            Op::Neg(_)
            | Op::Add(..)
            | Op::Sub(..)
            | Op::Mul(..)
            | Op::Square(_)
            | Op::Abs(_)
            | Op::Min(..)
            | Op::Max(..)
            | Op::AddC(..)
            | Op::MulC(..) => 1,
            Op::Clamp01(_) => 2,
            Op::SMin(..) | Op::SMax(..) => 8,
            Op::Sqrt(_) => 10,
            Op::Len2(..) => 13,
            Op::Len3(..) => 15,
            Op::Sin(_) => 14,
            Op::Rep(..) => 4,
        }
    }

    /// V-pipe micro-ops this op costs **per packet**, for the port model.
    ///
    /// `bench/a76-pi5.toml` pins two FP/ASIMD pipes (`port_v0` = SOG
    /// pipeline 7, `port_v1` = pipeline 8, both T1) at `thru 1/1`, so the
    /// packet interpreter retires **2 V-uops per cycle** when it is
    /// register-resident and V-limited. That replaces §1's "assume ~30% of
    /// peak until measured" with a computed bound for this specific loop.
    ///
    /// The table's `[latency.neon]` is one coarse row for all FP/ASIMD
    /// ("kept as one coarse row per dimension inventory row 35 — no live
    /// emit site; do not expand"). It cannot distinguish `FMLA` from
    /// `FSQRT`, whose throughputs differ by an order of magnitude. So the
    /// ops that need a group the coarse row cannot express take a **sweep
    /// bracket** rather than a pinned value, per the over-cost rule: the
    /// pessimistic end is charged and the bracket is reported.
    pub fn v_uops(&self, sw: &UopSweep) -> f32 {
        match self {
            Op::X | Op::Y | Op::Z | Op::Const(_) => 0.0,
            // One data-processing uop each; the coarse T1 row covers these.
            Op::Neg(_)
            | Op::Add(..)
            | Op::Sub(..)
            | Op::Mul(..)
            | Op::Square(_)
            | Op::Abs(_)
            | Op::Min(..)
            | Op::Max(..)
            | Op::AddC(..)
            | Op::MulC(..) => 1.0,
            // FMIN + FMAX against two constants.
            Op::Clamp01(_) => 2.0,
            // Fused polynomial smin, all of it certain ops:
            // sub, mul_c, add_c, clamp(2), sub, mul, sub, mul, mul_c.
            Op::SMin(..) | Op::SMax(..) => 10.0,
            Op::Sqrt(_) => sw.sqrt,
            Op::Len2(..) => 2.0 + sw.sqrt,
            Op::Len3(..) => 3.0 + sw.sqrt,
            Op::Sin(_) => sw.sin,
            Op::Rep(..) => sw.rep,
        }
    }

    pub fn is_blend(&self) -> bool {
        matches!(self, Op::SMin(..) | Op::SMax(..))
    }

    fn key(&self) -> (u8, u32, u32, u32) {
        match *self {
            Op::X => (0, 0, 0, 0),
            Op::Y => (1, 0, 0, 0),
            Op::Z => (2, 0, 0, 0),
            Op::Const(v) => (3, v.to_bits(), 0, 0),
            Op::Neg(a) => (4, a, 0, 0),
            Op::Add(a, b) => (5, a, b, 0),
            Op::Sub(a, b) => (6, a, b, 0),
            Op::Mul(a, b) => (7, a, b, 0),
            Op::Square(a) => (8, a, 0, 0),
            Op::Sqrt(a) => (9, a, 0, 0),
            Op::Abs(a) => (10, a, 0, 0),
            Op::Min(a, b) => (11, a, b, 0),
            Op::Max(a, b) => (12, a, b, 0),
            Op::SMin(a, b, k) => (13, a, b, k.to_bits()),
            Op::SMax(a, b, k) => (14, a, b, k.to_bits()),
            Op::Clamp01(a) => (15, a, 0, 0),
            Op::Sin(a) => (16, a, 0, 0),
            Op::AddC(a, v) => (17, a, v.to_bits(), 0),
            Op::MulC(a, v) => (18, a, v.to_bits(), 0),
            Op::Rep(a, v) => (19, a, v.to_bits(), 0),
            Op::Len2(a, b) => (20, a, b, 0),
            Op::Len3(a, b, c) => (21, a, b, c),
        }
    }
}

/// The ASIMD groups `bench/a76-pi5.toml` does not yet resolve.
///
/// Each is a bracket, not a value. `opts-ladder.md` 9c records that M20
/// inventory **row 35's trigger condition has fired** — wrela now emits
/// FP/ASIMD, so the freeze that declined per-group rows "is satisfied by
/// adding them, not violated". Until those rows exist, these are the
/// dimensions the pixels work needs, stated as sweep brackets so the
/// uncertainty is data rather than a guess baked into a total.
#[derive(Clone, Copy, Debug)]
pub struct UopSweep {
    /// `FSQRT` (poorly pipelined) versus `FRSQRTE` + 2 Newton refinements.
    pub sqrt: f32,
    /// Range reduction plus a minimax polynomial.
    pub sin: f32,
    /// Reciprocal multiply, `FRINTN`, `FMLS`.
    pub rep: f32,
}

impl UopSweep {
    /// The pessimistic end of every bracket (over-cost rule, decision 1609).
    pub fn pessimistic() -> UopSweep {
        UopSweep { sqrt: 12.0, sin: 16.0, rep: 8.0 }
    }
    /// The optimistic end: rsqrt-chain `length`, a short minimax sine.
    pub fn optimistic() -> UopSweep {
        UopSweep { sqrt: 6.0, sin: 8.0, rep: 3.0 }
    }
}

#[derive(Clone, Debug)]
pub struct Tape {
    pub ops: Vec<Op>,
    pub root: u32,
}

impl Tape {
    pub fn len(&self) -> usize {
        self.ops.len()
    }

    /// Total weight of every op, whether or not it is live. The unpruned
    /// baseline that §16.3's "pruned tape length by depth" is measured
    /// against.
    pub fn weight(&self) -> u32 {
        self.ops.iter().map(|o| o.weight()).sum()
    }

    /// Total V-uops for one full evaluation of this tape, per packet.
    pub fn v_uops(&self, sw: &UopSweep) -> f32 {
        self.ops.iter().map(|o| o.v_uops(sw)).sum()
    }

    pub fn blend_count(&self) -> usize {
        self.ops.iter().filter(|o| o.is_blend()).count()
    }
}

/// Tape builder with common-subexpression elimination.
///
/// CSE is not an optimisation here, it is realism: a hand-written scene
/// without it would carry duplicate `length(p)` chains that pruning would
/// then get credit for deleting, and the tape-length numbers would flatter
/// §2.2. `BTreeMap` rather than `HashMap` per CLAUDE.md — this is an
/// output-touching path.
pub struct Builder {
    ops: Vec<Op>,
    cse: BTreeMap<(u8, u32, u32, u32), u32>,
}

#[allow(dead_code)]
impl Builder {
    pub fn new() -> Builder {
        Builder { ops: Vec::new(), cse: BTreeMap::new() }
    }

    pub fn push(&mut self, op: Op) -> u32 {
        let k = op.key();
        if let Some(&i) = self.cse.get(&k) {
            return i;
        }
        let i = self.ops.len() as u32;
        self.ops.push(op);
        self.cse.insert(k, i);
        i
    }

    pub fn finish(self, root: u32) -> Tape {
        Tape { ops: self.ops, root }
    }

    // --- leaves -----------------------------------------------------------

    pub fn point(&mut self) -> [u32; 3] {
        [self.push(Op::X), self.push(Op::Y), self.push(Op::Z)]
    }
    pub fn konst(&mut self, v: f32) -> u32 {
        self.push(Op::Const(v))
    }

    // --- scalar arithmetic ------------------------------------------------

    pub fn add(&mut self, a: u32, b: u32) -> u32 {
        self.push(Op::Add(a, b))
    }
    pub fn sub(&mut self, a: u32, b: u32) -> u32 {
        self.push(Op::Sub(a, b))
    }
    pub fn mul(&mut self, a: u32, b: u32) -> u32 {
        self.push(Op::Mul(a, b))
    }
    pub fn neg(&mut self, a: u32) -> u32 {
        self.push(Op::Neg(a))
    }
    pub fn sq(&mut self, a: u32) -> u32 {
        self.push(Op::Square(a))
    }
    pub fn sqrt(&mut self, a: u32) -> u32 {
        self.push(Op::Sqrt(a))
    }
    pub fn abs(&mut self, a: u32) -> u32 {
        self.push(Op::Abs(a))
    }
    pub fn min(&mut self, a: u32, b: u32) -> u32 {
        self.push(Op::Min(a, b))
    }
    pub fn max(&mut self, a: u32, b: u32) -> u32 {
        self.push(Op::Max(a, b))
    }
    pub fn smin(&mut self, a: u32, b: u32, k: f32) -> u32 {
        self.push(Op::SMin(a, b, k))
    }
    pub fn smax(&mut self, a: u32, b: u32, k: f32) -> u32 {
        self.push(Op::SMax(a, b, k))
    }
    pub fn sin(&mut self, a: u32) -> u32 {
        self.push(Op::Sin(a))
    }
    pub fn addc(&mut self, a: u32, v: f32) -> u32 {
        if v == 0.0 { a } else { self.push(Op::AddC(a, v)) }
    }
    pub fn mulc(&mut self, a: u32, v: f32) -> u32 {
        if v == 1.0 { a } else { self.push(Op::MulC(a, v)) }
    }
    pub fn rep(&mut self, a: u32, p: f32) -> u32 {
        self.push(Op::Rep(a, p))
    }
    pub fn maxc0(&mut self, a: u32) -> u32 {
        let z = self.konst(0.0);
        self.max(a, z)
    }
    pub fn minc0(&mut self, a: u32) -> u32 {
        let z = self.konst(0.0);
        self.min(a, z)
    }
    pub fn clampc(&mut self, a: u32, lo: f32, hi: f32) -> u32 {
        let l = self.konst(lo);
        let h = self.konst(hi);
        let t = self.max(a, l);
        self.min(t, h)
    }

    // --- vector helpers ---------------------------------------------------

    pub fn translate(&mut self, p: [u32; 3], t: [f32; 3]) -> [u32; 3] {
        [self.addc(p[0], -t[0]), self.addc(p[1], -t[1]), self.addc(p[2], -t[2])]
    }

    /// Rotation about Y by a comptime-known angle. §6.3's line — topology is
    /// comptime, parameters are runtime — means a scene's rotations are
    /// constants in the tape, exactly as they would be after FieldWir
    /// specialisation.
    pub fn rot_y(&mut self, p: [u32; 3], ang: f32) -> [u32; 3] {
        let (s, c) = ang.sin_cos();
        let xc = self.mulc(p[0], c);
        let zs = self.mulc(p[2], s);
        let x = self.sub(xc, zs);
        let xs = self.mulc(p[0], s);
        let zc = self.mulc(p[2], c);
        let z = self.add(xs, zc);
        [x, p[1], z]
    }

    pub fn rot_z(&mut self, p: [u32; 3], ang: f32) -> [u32; 3] {
        let (s, c) = ang.sin_cos();
        let xc = self.mulc(p[0], c);
        let ys = self.mulc(p[1], s);
        let x = self.sub(xc, ys);
        let xs = self.mulc(p[0], s);
        let yc = self.mulc(p[1], c);
        let y = self.add(xs, yc);
        [x, y, p[2]]
    }

    pub fn len3(&mut self, p: [u32; 3]) -> u32 {
        self.push(Op::Len3(p[0], p[1], p[2]))
    }

    pub fn len2(&mut self, a: u32, b: u32) -> u32 {
        self.push(Op::Len2(a, b))
    }

    // --- primitives (IQ's standard exact SDFs) ---------------------------

    pub fn sphere(&mut self, p: [u32; 3], r: f32) -> u32 {
        let l = self.len3(p);
        self.addc(l, -r)
    }

    pub fn plane_y(&mut self, p: [u32; 3], h: f32) -> u32 {
        self.addc(p[1], -h)
    }

    pub fn boxd(&mut self, p: [u32; 3], h: [f32; 3]) -> u32 {
        let ax = self.abs(p[0]);
        let ay = self.abs(p[1]);
        let az = self.abs(p[2]);
        let qx = self.addc(ax, -h[0]);
        let qy = self.addc(ay, -h[1]);
        let qz = self.addc(az, -h[2]);
        let mx = self.maxc0(qx);
        let my = self.maxc0(qy);
        let mz = self.maxc0(qz);
        let outside = self.len3([mx, my, mz]);
        let m1 = self.max(qy, qz);
        let m2 = self.max(qx, m1);
        let inside = self.minc0(m2);
        self.add(outside, inside)
    }

    pub fn round_box(&mut self, p: [u32; 3], h: [f32; 3], r: f32) -> u32 {
        let d = self.boxd(p, [h[0] - r, h[1] - r, h[2] - r]);
        self.addc(d, -r)
    }

    /// Capsule along Y, half-length `hl`, radius `r`.
    pub fn capsule_y(&mut self, p: [u32; 3], hl: f32, r: f32) -> u32 {
        let cy = self.clampc(p[1], -hl, hl);
        let qy = self.sub(p[1], cy);
        let l = self.len3([p[0], qy, p[2]]);
        self.addc(l, -r)
    }

    /// Cylinder along Y, radius `r`, half-height `hh`.
    pub fn cylinder_y(&mut self, p: [u32; 3], r: f32, hh: f32) -> u32 {
        let lxz = self.len2(p[0], p[2]);
        let dx = self.addc(lxz, -r);
        let ay = self.abs(p[1]);
        let dy = self.addc(ay, -hh);
        let mx = self.maxc0(dx);
        let my = self.maxc0(dy);
        let outside = self.len2(mx, my);
        let m = self.max(dx, dy);
        let inside = self.minc0(m);
        self.add(outside, inside)
    }

    /// Torus in the XZ plane: major radius `rr`, minor `r`.
    pub fn torus(&mut self, p: [u32; 3], rr: f32, r: f32) -> u32 {
        let lxz = self.len2(p[0], p[2]);
        let q = self.addc(lxz, -rr);
        let l = self.len2(q, p[1]);
        self.addc(l, -r)
    }

    // --- combinators ------------------------------------------------------

    pub fn union(&mut self, a: u32, b: u32) -> u32 {
        self.min(a, b)
    }
    pub fn inter(&mut self, a: u32, b: u32) -> u32 {
        self.max(a, b)
    }
    pub fn subtract(&mut self, a: u32, b: u32) -> u32 {
        let nb = self.neg(b);
        self.max(a, nb)
    }
    pub fn ssubtract(&mut self, a: u32, b: u32, k: f32) -> u32 {
        let nb = self.neg(b);
        self.smax(a, nb, k)
    }

    /// Band-limited displacement: a sum of `oct` sinusoidal octaves.
    ///
    /// A deliberate stand-in for §6.1's `fbm`, and an optimistic one — real
    /// hashed-gradient noise is piecewise and would enclose far worse under
    /// affine arithmetic. The report says so out loud, because a probe that
    /// quietly picks the friendly noise is measuring its own choice.
    pub fn displace_sin(
        &mut self,
        d: u32,
        p: [u32; 3],
        amp: f32,
        freq: f32,
        oct: u32,
        phase: f32,
    ) -> u32 {
        let mut acc = self.konst(0.0);
        let mut a = amp;
        let mut f = freq;
        for i in 0..oct {
            let ph = phase + i as f32 * 1.7;
            let sx = self.mulc(p[0], f);
            let sx = self.addc(sx, ph);
            let sx = self.sin(sx);
            let sy = self.mulc(p[1], f * 1.13);
            let sy = self.addc(sy, ph * 1.7);
            let sy = self.sin(sy);
            let sz = self.mulc(p[2], f * 0.87);
            let sz = self.addc(sz, ph * 2.3);
            let sz = self.sin(sz);
            let m = self.mul(sx, sy);
            let m = self.mul(m, sz);
            let m = self.mulc(m, a);
            acc = self.add(acc, m);
            a *= 0.5;
            f *= 2.0;
        }
        self.add(d, acc)
    }
}
