use crate::aff::{Aff, Iv};
use crate::tape::{Op, Tape};

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
    if raw >= 1.0 {
        return a;
    }
    if raw <= 0.0 {
        return b;
    }
    let h = D { v: raw, g: t.g };
    let lin = b.add(a.sub(b).mul(h));
    let one = D::c(1.0);
    let bulge = h.mul(one.sub(h)).scale(k);
    lin.sub(bulge)
}

pub fn eval_aff(tape: &Tape, p: [Aff; 3], out: &mut Vec<Aff>, out_iv: &mut Vec<Iv>) {
    out.clear();
    out.resize(tape.ops.len(), Aff::konst(0.0));
    out_iv.clear();
    out_iv.resize(tape.ops.len(), Iv::konst(0.0));
    for i in 0..tape.ops.len() {
        let iv = |j: u32, o: &Vec<Iv>| -> Iv { o[j as usize] };
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
        let (alo, ahi) = v.interval();
        out_iv[i] = Iv::new(alo.max(vi.lo), ahi.min(vi.hi));
    }
}

#[inline]
fn smin_aff(a: Aff, b: Aff, k: f32) -> Aff {
    let h = b.sub(a).mul_c(0.5 / k).add_c(0.5).clamp01();
    let lin = b.add(a.sub(b).mul(h));
    let bulge = h
        .mul(Aff::konst(1.0).sub(h))
        .intersect_opaque(0.0, 0.25)
        .mul_c(k);
    lin.sub(bulge)
}

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

pub const LIPSCHITZ: f32 = 1.25;

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
