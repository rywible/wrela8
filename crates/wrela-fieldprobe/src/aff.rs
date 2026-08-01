pub const NSYM: usize = 3;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Aff {
    pub c: f32,
    pub d: [f32; NSYM],
    pub e: f32,
}

impl Aff {
    #[inline]
    pub fn konst(c: f32) -> Aff {
        Aff {
            c,
            d: [0.0; NSYM],
            e: 0.0,
        }
    }

    #[inline]
    pub fn sym(i: usize, lo: f32, hi: f32) -> Aff {
        let mut d = [0.0; NSYM];
        d[i] = 0.5 * (hi - lo);
        Aff {
            c: 0.5 * (lo + hi),
            d,
            e: 0.0,
        }
    }

    #[inline]
    pub fn opaque(lo: f32, hi: f32) -> Aff {
        Aff {
            c: 0.5 * (lo + hi),
            d: [0.0; NSYM],
            e: 0.5 * (hi - lo).abs(),
        }
    }

    #[inline]
    pub fn rad(&self) -> f32 {
        self.d[0].abs() + self.d[1].abs() + self.d[2].abs() + self.e
    }

    #[inline]
    pub fn lo(&self) -> f32 {
        self.c - self.rad()
    }

    #[inline]
    pub fn hi(&self) -> f32 {
        self.c + self.rad()
    }

    #[inline]
    pub fn interval(&self) -> (f32, f32) {
        let r = self.rad();
        (self.c - r, self.c + r)
    }

    #[inline]
    pub fn is_finite(&self) -> bool {
        self.c.is_finite() && self.d.iter().all(|x| x.is_finite()) && self.e.is_finite()
    }

    #[inline]
    pub fn neg(self) -> Aff {
        Aff {
            c: -self.c,
            d: [-self.d[0], -self.d[1], -self.d[2]],
            e: self.e,
        }
    }

    #[inline]
    pub fn add(self, o: Aff) -> Aff {
        Aff {
            c: self.c + o.c,
            d: [self.d[0] + o.d[0], self.d[1] + o.d[1], self.d[2] + o.d[2]],
            e: self.e + o.e,
        }
    }

    #[inline]
    pub fn sub(self, o: Aff) -> Aff {
        self.add(o.neg())
    }

    #[inline]
    pub fn add_c(self, k: f32) -> Aff {
        Aff {
            c: self.c + k,
            d: self.d,
            e: self.e,
        }
    }

    #[inline]
    pub fn mul_c(self, k: f32) -> Aff {
        Aff {
            c: self.c * k,
            d: [self.d[0] * k, self.d[1] * k, self.d[2] * k],
            e: self.e * k.abs(),
        }
    }

    pub fn mul(self, o: Aff) -> Aff {
        let ra = self.rad();
        let rb = o.rad();
        Aff {
            c: self.c * o.c,
            d: [
                self.c * o.d[0] + o.c * self.d[0],
                self.c * o.d[1] + o.c * self.d[1],
                self.c * o.d[2] + o.c * self.d[2],
            ],
            e: self.c.abs() * o.e + o.c.abs() * self.e + ra * rb,
        }
    }

    pub fn square(self) -> Aff {
        let r = self.rad();
        let half = 0.5 * r * r;
        Aff {
            c: self.c * self.c + half,
            d: [
                2.0 * self.c * self.d[0],
                2.0 * self.c * self.d[1],
                2.0 * self.c * self.d[2],
            ],
            e: 2.0 * self.c.abs() * self.e + half,
        }
    }

    #[inline]
    fn taylor1(self, fc: f32, dfc: f32, m2: f32) -> Aff {
        let r = self.rad();
        Aff {
            c: fc,
            d: [dfc * self.d[0], dfc * self.d[1], dfc * self.d[2]],
            e: dfc.abs() * self.e + 0.5 * m2 * r * r,
        }
    }

    pub fn abs(self) -> Aff {
        let (lo, hi) = self.interval();
        if lo >= 0.0 {
            return self;
        }
        if hi <= 0.0 {
            return self.neg();
        }
        let w = hi - lo;
        if w <= 0.0 || !w.is_finite() {
            return Aff::opaque(0.0, hi.max(-lo));
        }
        let alpha = (hi + lo) / w;
        let m = -2.0 * lo * hi / w;
        let half = 0.5 * m;
        let mut r = self.mul_c(alpha).add_c(half);
        r.e += half;
        r
    }

    pub fn intersect_opaque(self, blo: f32, bhi: f32) -> Aff {
        let (lo, hi) = self.interval();
        if lo >= blo && hi <= bhi {
            return self;
        }
        let nlo = lo.max(blo);
        let nhi = hi.min(bhi);
        if !(nlo <= nhi) {
            return Aff::opaque(blo, bhi);
        }
        Aff::opaque(nlo, nhi)
    }

    #[inline]
    pub fn min(self, o: Aff) -> Aff {
        let s = self.add(o);
        let d = self.sub(o).abs();
        s.sub(d).mul_c(0.5)
    }

    #[inline]
    pub fn max(self, o: Aff) -> Aff {
        let s = self.add(o);
        let d = self.sub(o).abs();
        s.add(d).mul_c(0.5)
    }

    pub fn sqrt(self) -> Aff {
        let (lo, hi) = self.interval();
        if hi <= 0.0 {
            return Aff::konst(0.0);
        }
        let mono = Aff::opaque(lo.max(0.0).sqrt(), hi.sqrt());
        if lo <= 1e-9 {
            return mono;
        }
        let fc = self.c.clamp(lo, hi).sqrt();
        let dfc = 0.5 / fc;
        let m2 = 0.25 / (lo * lo.sqrt());
        tighter(self.taylor1(fc, dfc, m2), mono)
    }

    pub fn recip(self) -> Aff {
        let (lo, hi) = self.interval();
        if lo <= 0.0 && hi >= 0.0 {
            return Aff {
                c: f32::NAN,
                d: [0.0; NSYM],
                e: f32::INFINITY,
            };
        }
        let m = lo.abs().min(hi.abs());
        let fc = 1.0 / self.c;
        let dfc = -fc * fc;
        let m2 = 2.0 / (m * m * m);
        let (rlo, rhi) = (1.0 / hi, 1.0 / lo);
        let mono = Aff::opaque(rlo.min(rhi), rlo.max(rhi));
        tighter(self.taylor1(fc, dfc, m2), mono)
    }

    pub fn hull(self, o: Aff) -> Aff {
        let (a, b) = self.interval();
        let (c, d) = o.interval();
        Aff::opaque(a.min(c), b.max(d))
    }

    pub fn sin(self) -> Aff {
        self.taylor1(self.c.sin(), self.c.cos(), 1.0)
            .intersect_opaque(-1.0, 1.0)
    }

    pub fn cos(self) -> Aff {
        self.taylor1(self.c.cos(), -self.c.sin(), 1.0)
            .intersect_opaque(-1.0, 1.0)
    }

    pub fn clamp01(self) -> Aff {
        let (lo, hi) = self.interval();
        if lo >= 0.0 && hi <= 1.0 {
            return self;
        }
        if lo >= 1.0 {
            return Aff::konst(1.0);
        }
        if hi <= 0.0 {
            return Aff::konst(0.0);
        }
        self.max(Aff::konst(0.0))
            .min(Aff::konst(1.0))
            .intersect_opaque(0.0, 1.0)
    }

    pub fn rep(self, p: f32) -> Aff {
        if !(p > 0.0) || !p.is_finite() {
            return self;
        }
        let (lo, hi) = self.interval();
        let klo = (lo / p).round();
        let khi = (hi / p).round();
        if klo == khi && klo.is_finite() {
            return self.add_c(-p * klo);
        }
        Aff::opaque(-0.5 * p, 0.5 * p)
    }
}

fn tighter(a: Aff, b: Aff) -> Aff {
    if a.rad() <= b.rad() && a.is_finite() {
        a
    } else {
        b
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Iv {
    pub lo: f32,
    pub hi: f32,
}

impl Iv {
    #[inline]
    pub fn new(lo: f32, hi: f32) -> Iv {
        Iv { lo, hi }
    }
    #[inline]
    pub fn konst(v: f32) -> Iv {
        Iv { lo: v, hi: v }
    }
    #[inline]
    pub fn width(&self) -> f32 {
        self.hi - self.lo
    }
    #[inline]
    pub fn neg(self) -> Iv {
        Iv::new(-self.hi, -self.lo)
    }
    #[inline]
    pub fn add(self, o: Iv) -> Iv {
        Iv::new(self.lo + o.lo, self.hi + o.hi)
    }
    #[inline]
    pub fn sub(self, o: Iv) -> Iv {
        Iv::new(self.lo - o.hi, self.hi - o.lo)
    }
    pub fn mul(self, o: Iv) -> Iv {
        let a = self.lo * o.lo;
        let b = self.lo * o.hi;
        let c = self.hi * o.lo;
        let d = self.hi * o.hi;
        Iv::new(a.min(b).min(c).min(d), a.max(b).max(c).max(d))
    }
    pub fn square(self) -> Iv {
        if self.lo >= 0.0 {
            Iv::new(self.lo * self.lo, self.hi * self.hi)
        } else if self.hi <= 0.0 {
            Iv::new(self.hi * self.hi, self.lo * self.lo)
        } else {
            let m = self.lo.abs().max(self.hi.abs());
            Iv::new(0.0, m * m)
        }
    }
    pub fn abs(self) -> Iv {
        if self.lo >= 0.0 {
            self
        } else if self.hi <= 0.0 {
            self.neg()
        } else {
            Iv::new(0.0, self.lo.abs().max(self.hi.abs()))
        }
    }
    #[inline]
    pub fn min(self, o: Iv) -> Iv {
        Iv::new(self.lo.min(o.lo), self.hi.min(o.hi))
    }
    #[inline]
    pub fn max(self, o: Iv) -> Iv {
        Iv::new(self.lo.max(o.lo), self.hi.max(o.hi))
    }
    pub fn sqrt(self) -> Iv {
        Iv::new(self.lo.max(0.0).sqrt(), self.hi.max(0.0).sqrt())
    }
    pub fn sin(self) -> Iv {
        if self.width() >= std::f32::consts::PI {
            return Iv::new(-1.0, 1.0);
        }
        let a = self.lo.sin();
        let b = self.hi.sin();
        let mut lo = a.min(b);
        let mut hi = a.max(b);
        let two_pi = std::f32::consts::TAU;
        let half_pi = std::f32::consts::FRAC_PI_2;
        let k0 = ((self.lo - half_pi) / two_pi).ceil();
        if half_pi + k0 * two_pi <= self.hi {
            hi = 1.0;
        }
        let k1 = ((self.lo + half_pi) / two_pi).ceil();
        if -half_pi + k1 * two_pi <= self.hi {
            lo = -1.0;
        }
        Iv::new(lo, hi)
    }
    pub fn cos(self) -> Iv {
        Iv::new(
            self.lo + std::f32::consts::FRAC_PI_2,
            self.hi + std::f32::consts::FRAC_PI_2,
        )
        .sin()
    }
    pub fn clamp01(self) -> Iv {
        Iv::new(self.lo.clamp(0.0, 1.0), self.hi.clamp(0.0, 1.0))
    }
    #[inline]
    pub fn clamp_to(self, lo: f32, hi: f32) -> Iv {
        Iv::new(self.lo.clamp(lo, hi), self.hi.clamp(lo, hi))
    }
    pub fn rep(self, p: f32) -> Iv {
        if !(p > 0.0) {
            return self;
        }
        let klo = (self.lo / p).round();
        let khi = (self.hi / p).round();
        if klo == khi {
            Iv::new(self.lo - p * klo, self.hi - p * klo)
        } else {
            Iv::new(-0.5 * p, 0.5 * p)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Rng(u32);
    impl Rng {
        fn next_f32(&mut self) -> f32 {
            let mut x = self.0;
            x ^= x << 13;
            x ^= x >> 17;
            x ^= x << 5;
            self.0 = x;
            ((x >> 8) as f32 / 8_388_608.0) - 1.0
        }
    }

    fn at(a: &Aff, eps: [f32; NSYM], ee: f32) -> f32 {
        a.c + a.d[0] * eps[0] + a.d[1] * eps[1] + a.d[2] * eps[2] + a.e * ee
    }

    #[test]
    fn operations_enclose_truth() {
        let mut rng = Rng(0x1234_5678);
        for _ in 0..20_000 {
            let x = Aff {
                c: rng.next_f32() * 3.0,
                d: [rng.next_f32(), rng.next_f32() * 0.5, rng.next_f32() * 0.25],
                e: rng.next_f32().abs() * 0.1,
            };
            let y = Aff {
                c: rng.next_f32() * 3.0,
                d: [rng.next_f32() * 0.3, rng.next_f32(), rng.next_f32() * 0.7],
                e: rng.next_f32().abs() * 0.1,
            };
            let eps = [rng.next_f32(), rng.next_f32(), rng.next_f32()];
            let (ex, ey) = (rng.next_f32(), rng.next_f32());
            let xv = at(&x, eps, ex);
            let yv = at(&y, eps, ey);

            let cases: [(Aff, f32, &str); 10] = [
                (x.add(y), xv + yv, "add"),
                (x.sub(y), xv - yv, "sub"),
                (x.mul(y), xv * yv, "mul"),
                (x.square(), xv * xv, "square"),
                (x.abs(), xv.abs(), "abs"),
                (x.min(y), xv.min(yv), "min"),
                (x.max(y), xv.max(yv), "max"),
                (x.sin(), xv.sin(), "sin"),
                (x.cos(), xv.cos(), "cos"),
                (x.clamp01(), xv.clamp(0.0, 1.0), "clamp01"),
            ];
            for (form, truth, name) in cases {
                let (lo, hi) = form.interval();
                let slack = 1e-4 * (1.0 + truth.abs());
                assert!(
                    truth >= lo - slack && truth <= hi + slack,
                    "{name}: {truth} outside [{lo}, {hi}]"
                );
            }

            let s = x.square();
            let sv = xv * xv;
            let (lo, hi) = s.sqrt().interval();
            let slack = 1e-3 * (1.0 + sv.abs().sqrt());
            assert!(
                sv.sqrt() >= lo - slack && sv.sqrt() <= hi + slack,
                "sqrt: {} outside [{lo}, {hi}]",
                sv.sqrt()
            );
        }
    }

    #[test]
    fn rep_encloses_truth() {
        let mut rng = Rng(0x9e37_79b9);
        for _ in 0..20_000 {
            let x = Aff {
                c: rng.next_f32() * 5.0,
                d: [rng.next_f32() * 2.0, 0.0, 0.0],
                e: rng.next_f32().abs() * 0.05,
            };
            let p = 0.3 + rng.next_f32().abs() * 2.0;
            let eps = [rng.next_f32(), 0.0, 0.0];
            let ee = rng.next_f32();
            let xv = at(&x, eps, ee);
            let truth = xv - p * (xv / p).round();
            let (lo, hi) = x.rep(p).interval();
            assert!(
                truth >= lo - 1e-4 && truth <= hi + 1e-4,
                "rep({p}): {truth} outside [{lo}, {hi}]"
            );
        }
    }

    #[test]
    fn affine_beats_interval_on_the_doc_example() {
        let x = Aff::sym(0, 0.0, 1.0);
        let (lo, hi) = x.square().sub(x).interval();
        let iv = Iv::new(0.0, 1.0);
        let ivr = iv.square().sub(iv);
        assert!((ivr.lo - -1.0).abs() < 1e-6 && (ivr.hi - 1.0).abs() < 1e-6);
        assert!(lo >= -0.6 && hi <= 0.4, "aa range [{lo}, {hi}] not tight");
        assert!(hi - lo < 0.5 * ivr.width());
    }

    #[test]
    fn smin_lies_between_min_and_min_minus_quarter_k() {
        let mut rng = Rng(0xa5a5_1234);
        for _ in 0..50_000 {
            let a = rng.next_f32() * 10.0;
            let b = rng.next_f32() * 10.0;
            let k = 0.001 + rng.next_f32().abs() * 0.5;
            let h = (0.5 + 0.5 * (b - a) / k).clamp(0.0, 1.0);
            let s = if h >= 1.0 {
                a
            } else if h <= 0.0 {
                b
            } else {
                b + (a - b) * h - k * h * (1.0 - h)
            };
            let m = a.min(b);
            assert!(s <= m + 1e-5, "smin {s} above min {m}");
            assert!(
                s >= m - 0.25 * k - 1e-5,
                "smin {s} below min-k/4 {}",
                m - 0.25 * k
            );
        }
    }

    #[test]
    fn separable_min_is_exact() {
        let a = Aff::sym(0, 5.0, 6.0);
        let b = Aff::sym(1, 1.0, 2.0);
        let m = a.min(b);
        let (lo, hi) = m.interval();
        assert!(
            (lo - 1.0).abs() < 1e-5 && (hi - 2.0).abs() < 1e-5,
            "[{lo}, {hi}]"
        );
    }
}
