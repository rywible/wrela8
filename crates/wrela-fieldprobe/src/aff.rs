//! Revised affine arithmetic over three primitive noise symbols.
//!
//! plans/graphics.md §2.1 rests on one claim: affine arithmetic is tight
//! enough to classify a screen tile without tracing a ray, where plain
//! interval arithmetic is not. This module is the instrument that decides
//! that claim, so its own soundness is the first thing the probe checks.
//!
//! # The form
//!
//! An affine form is
//!
//! ```text
//!     x̂ = c + d₀ε₀ + d₁ε₁ + d₂ε₂ + e·ε_e ,   εᵢ ∈ [-1, 1]
//! ```
//!
//! with exactly three *shared* symbols — ε₀ = u, ε₁ = v, ε₂ = t, the screen
//! tile's two axes and the ray parameter — plus one *unshared* error term
//! `e ≥ 0` that absorbs every nonlinear residue.
//!
//! Lumping all nonlinearity into a single unshared symbol is "revised"
//! affine arithmetic (Fryazinov / Pasko / Comninos, cited in §2.1's
//! reference list). It keeps precisely the correlation that matters — the
//! *linear* dependence on u, v, t — which is what makes `x·x − x` tight,
//! and, far more importantly here, what makes `a − b` tight inside a
//! `smin`. It gives up correlation between nonlinear residues, which is
//! worth little and would cost a growing symbol list.
//!
//! The payoff is that an `Aff` is 5 floats, `Copy`, and allocation-free, so
//! a tile classification over a few-hundred-op tape is cheap enough to run
//! over a whole screen at several subdivision depths.
//!
//! # Soundness
//!
//! Every operation must satisfy: for all ε ∈ [-1,1]³, the true value of the
//! expression at the corresponding point lies inside `[lo(), hi()]` of the
//! result. `tests::` and the probe's `selfcheck` lane both verify this by
//! sampling; a bound that ever fails containment is a bug in this file, not
//! a finding about the design.

/// The three shared noise symbols: tile-u, tile-v, ray-t.
pub const NSYM: usize = 3;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Aff {
    /// Center.
    pub c: f32,
    /// Coefficients on the shared symbols ε₀..ε₂.
    pub d: [f32; NSYM],
    /// Unshared error magnitude. Invariant: `e >= 0`.
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

    /// A form that is exactly the `i`-th symbol scaled to span `[lo, hi]`.
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

    /// An opaque interval with no correlation to anything.
    #[inline]
    pub fn opaque(lo: f32, hi: f32) -> Aff {
        Aff {
            c: 0.5 * (lo + hi),
            d: [0.0; NSYM],
            e: 0.5 * (hi - lo).abs(),
        }
    }

    /// Total radius: the half-width of the guaranteed enclosure.
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

    /// True when the enclosure is finite. A non-finite bound means a
    /// division by an interval spanning zero, or an overflow; the probe
    /// fails closed on it rather than reporting a flattering "no surface
    /// here" (CLAUDE.md: an unimplemented path errors loudly).
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

    /// Product of two forms.
    ///
    /// The exact product is `c₁c₂ + Σ(c₁d₂ᵢ + c₂d₁ᵢ)εᵢ + (Σd₁ᵢεᵢ)(Σd₂ᵢεᵢ)`.
    /// The quadratic tail is bounded by `rad(a)·rad(b)` and goes into `e`.
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
            // |c₁|·e₂ + |c₂|·e₁ covers the linear part of the unshared
            // terms; ra·rb covers the whole quadratic tail (and, being
            // computed from radii that already include e, re-covers those
            // cross terms — conservative, which is the safe direction).
            e: self.c.abs() * o.e + o.c.abs() * self.e + ra * rb,
        }
    }

    /// `x²`, with the exact Chebyshev form rather than `mul(self, self)`.
    ///
    /// Squaring is the hottest nonlinear op in any distance field
    /// (`length` is three of them), and the generic product rule is 2×
    /// looser here *and* fails to notice that a square is non-negative.
    ///
    /// With `x = c + p` where `|p| ≤ r`: `x² = c² + 2cp + p²`, and
    /// `p² ∈ [0, r²]`, so recentre by `r²/2` and carry `r²/2` as error.
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

    /// First-order Taylor enclosure of a smooth univariate `f`.
    ///
    /// `f(x) = f(c) + f'(c)(x−c) + f''(ξ)(x−c)²/2` gives a rigorous bound
    /// once `m2 ≥ |f''|` over `[c−r, c+r]`. The linear term keeps the
    /// correlation on u, v, t; the remainder joins `e`.
    #[inline]
    fn taylor1(self, fc: f32, dfc: f32, m2: f32) -> Aff {
        let r = self.rad();
        Aff {
            c: fc,
            d: [dfc * self.d[0], dfc * self.d[1], dfc * self.d[2]],
            e: dfc.abs() * self.e + 0.5 * m2 * r * r,
        }
    }

    /// `|x|`.
    ///
    /// When the input straddles zero this is the one genuinely non-smooth
    /// operation in the set, and it is also the load-bearing one: `min` and
    /// `max` are built from it, so its tightness *is* the tightness of CSG.
    /// Uses the exact Chebyshev line, which collapses to the identity (zero
    /// added error) as soon as the input is sign-definite — that collapse
    /// is what lets a tape prune.
    pub fn abs(self) -> Aff {
        let (lo, hi) = self.interval();
        if lo >= 0.0 {
            return self;
        }
        if hi <= 0.0 {
            return self.neg();
        }
        // Chebyshev line for |x| on [lo, hi], lo < 0 < hi:
        //   slope α = (hi + lo) / (hi - lo)
        //   max deviation from the chord M = -2·lo·hi / (hi - lo)
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

    /// Intersect the enclosure with a box the true value is known to lie in,
    /// **giving up the correlation** to do it.
    ///
    /// An affine form is a function of ε, not an interval: it asserts
    /// `truth(ε) ∈ [c + L(ε) − e, c + L(ε) + e]` for *every* ε. Neither
    /// shifting `c` nor shrinking `e` is a valid tightening — both change
    /// what the form claims at every individual ε, while the box says
    /// nothing about which ε the truth sits at. So the only sound moves are
    /// keep the form, or collapse to an opaque interval.
    ///
    /// This replaced a "free tightening" that was a real bug in this file,
    /// caught by `tests::operations_enclose_truth` before a single scene was
    /// measured: an `abs` clamped to `[0, max]` produced a `max` whose
    /// enclosure excluded the truth by ~0.3%. That error direction is the
    /// dangerous one — a too-tight enclosure reports geometry as *absent*,
    /// so it would have surfaced as a flattering interior-tile fraction and
    /// a flattering pruning result rather than as a crash.
    pub fn intersect_opaque(self, blo: f32, bhi: f32) -> Aff {
        let (lo, hi) = self.interval();
        if lo >= blo && hi <= bhi {
            return self;
        }
        let nlo = lo.max(blo);
        let nhi = hi.min(bhi);
        if !(nlo <= nhi) {
            // Enclosure and box disagree: trust the box, which is a fact
            // about the function, over an accumulated residue estimate.
            return Aff::opaque(blo, bhi);
        }
        Aff::opaque(nlo, nhi)
    }

    #[inline]
    pub fn min(self, o: Aff) -> Aff {
        // min(a,b) = ((a+b) - |a-b|) / 2, evaluated so that the difference
        // keeps its correlation. When a and b are separable this returns
        // one operand exactly.
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

    /// `sqrt(x)` for `x ≥ 0`.
    ///
    /// A negative low end can only come from an accumulated nonlinear
    /// residue — every caller squares first — so it is handled by falling
    /// back to the monotone interval bound, which is sound because
    /// `truth ∈ [0, hi]` implies `sqrt(truth) ∈ [0, sqrt(hi)]`. Clamping the
    /// *form* would not be (see [`Aff::intersect_opaque`]).
    pub fn sqrt(self) -> Aff {
        let (lo, hi) = self.interval();
        if hi <= 0.0 {
            return Aff::konst(0.0);
        }
        // `sqrt` is monotone, so this is always a valid enclosure and is the
        // fallback whenever the linear form is worse.
        let mono = Aff::opaque(lo.max(0.0).sqrt(), hi.sqrt());
        if lo <= 1e-9 {
            // |f''| = 1/(4x^{3/2}) is unbounded at 0; no linear form exists.
            return mono;
        }
        let fc = self.c.clamp(lo, hi).sqrt();
        let dfc = 0.5 / fc;
        let m2 = 0.25 / (lo * lo.sqrt());
        tighter(self.taylor1(fc, dfc, m2), mono)
    }

    /// `1/x`, for an enclosure that excludes zero. Returns a non-finite
    /// form otherwise so callers fail closed rather than silently
    /// continuing with a bound that does not enclose.
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
        // |f''| = 2/|x|³, maximised at the end nearest zero.
        let m2 = 2.0 / (m * m * m);
        let (rlo, rhi) = (1.0 / hi, 1.0 / lo);
        let mono = Aff::opaque(rlo.min(rhi), rlo.max(rhi));
        tighter(self.taylor1(fc, dfc, m2), mono)
    }

    /// The smallest opaque form containing both inputs. Used where a
    /// derivative genuinely is ambiguous — inside an `abs` straddling zero,
    /// or a `min` whose branches are not separable — so the ambiguity is
    /// carried rather than guessed.
    pub fn hull(self, o: Aff) -> Aff {
        let (a, b) = self.interval();
        let (c, d) = o.interval();
        Aff::opaque(a.min(c), b.max(d))
    }

    /// `sin(x)`. `|f''| ≤ 1` everywhere, so the Taylor bound is rigorous
    /// with no case analysis. The `[-1,1]` intersection only fires once the
    /// radius is wide enough that the quadratic remainder swamps the linear
    /// term — at which point the correlation it costs is worth nothing.
    pub fn sin(self) -> Aff {
        self.taylor1(self.c.sin(), self.c.cos(), 1.0)
            .intersect_opaque(-1.0, 1.0)
    }

    pub fn cos(self) -> Aff {
        self.taylor1(self.c.cos(), -self.c.sin(), 1.0)
            .intersect_opaque(-1.0, 1.0)
    }

    /// `clamp(x, 0, 1)`.
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
        // `min(max(x,0),1)` through the correlated forms keeps the useful
        // case tight. But the chain is only *sound*, not bounded: for a wide
        // input, `abs` decorrelates and the enclosure can come back at ±75
        // for a quantity whose true range is `[0,1]`. Intersecting with the
        // provable box afterwards costs the correlation that was already
        // gone and stops the blowup from being amplified by the surrounding
        // `smin` — where a `k` of 0.025 multiplies the operand difference by
        // 20 before it ever reaches this clamp.
        self.max(Aff::konst(0.0))
            .min(Aff::konst(1.0))
            .intersect_opaque(0.0, 1.0)
    }

    /// Domain repetition: `x − p·round(x/p)`.
    ///
    /// Exact (a pure translation, correlation intact) while the enclosure
    /// stays inside one cell; opaque `[-p/2, p/2]` the moment it straddles
    /// a cell boundary. That cliff is real, not an artefact — it is why
    /// §9.2's "domain repetition is nearly free" claim needs measuring
    /// rather than assuming.
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

/// Pick the tighter of a correlated linear enclosure and a plain monotone
/// one, both sound.
///
/// Taylor error scales with `|f''|·r²`, and for `sqrt` and `1/x` near zero
/// `|f''|` is unbounded — so a form that is narrow in `r` but sitting close
/// to the singularity produces an enclosure astronomically wider than the
/// function's own monotone range. This is not hypothetical: without this
/// guard the probe's mean enclosure overwidth came back at 5×10¹⁷, which is
/// a number no downstream test would have failed on, because an infinitely
/// wide bound never *excludes* anything — it just silently classifies every
/// tile as "boundary" and reports zero interior area as if that were a
/// finding about the design.
fn tighter(a: Aff, b: Aff) -> Aff {
    if a.rad() <= b.rad() && a.is_finite() {
        a
    } else {
        b
    }
}

/// Plain interval arithmetic, kept only so the report can state how much
/// affine arithmetic actually buys. §2.1 asserts plain IA "is too loose to
/// use"; that assertion is cheap to check and expensive to get wrong.
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
        // Exact only when the interval is narrow; otherwise the full range.
        if self.width() >= std::f32::consts::PI {
            return Iv::new(-1.0, 1.0);
        }
        let a = self.lo.sin();
        let b = self.hi.sin();
        let mut lo = a.min(b);
        let mut hi = a.max(b);
        // Check for an enclosed extremum.
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

    /// Deterministic sampler. No `rand`, no clock: a fixed xorshift so the
    /// probe's numbers are reproducible by construction.
    struct Rng(u32);
    impl Rng {
        fn next_f32(&mut self) -> f32 {
            let mut x = self.0;
            x ^= x << 13;
            x ^= x >> 17;
            x ^= x << 5;
            self.0 = x;
            // Uniform in [-1, 1].
            ((x >> 8) as f32 / 8_388_608.0) - 1.0
        }
    }

    /// Evaluate an affine form at a concrete point in symbol space.
    fn at(a: &Aff, eps: [f32; NSYM], ee: f32) -> f32 {
        a.c + a.d[0] * eps[0] + a.d[1] * eps[1] + a.d[2] * eps[2] + a.e * ee
    }

    /// Every operation must enclose the true value at every sampled point.
    /// This is the soundness gate: if it fails, every number the probe
    /// reports is meaningless.
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
            // The two operands' unshared terms are independent, so they get
            // independent samples.
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

            // sqrt needs a non-negative argument; square first.
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

    /// `rep` must enclose the true repetition, including across cells.
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

    /// The motivating example from §2.1: `x·x − x` over `[0,1]` is `[-1,1]`
    /// under plain interval arithmetic and must be far tighter here.
    #[test]
    fn affine_beats_interval_on_the_doc_example() {
        let x = Aff::sym(0, 0.0, 1.0);
        let (lo, hi) = x.square().sub(x).interval();
        let iv = Iv::new(0.0, 1.0);
        let ivr = iv.square().sub(iv);
        assert!((ivr.lo - -1.0).abs() < 1e-6 && (ivr.hi - 1.0).abs() < 1e-6);
        // True range is [-0.25, 0].
        assert!(lo >= -0.6 && hi <= 0.4, "aa range [{lo}, {hi}] not tight");
        assert!(hi - lo < 0.5 * ivr.width());
    }

    /// `min(a,b) − k/4 ≤ smin(a,b,k) ≤ min(a,b)`, with both ends attained.
    ///
    /// This is the bound `eval::smin_iv` uses in place of evaluating the
    /// formula. Locking it here because the formula-based enclosure was off
    /// by 35 absolute units on the melee scene, and the failure was silent —
    /// it produced a plausible-looking 0% interior fraction, not a crash.
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

    /// A separable `min` must return one operand exactly — no widening.
    /// This is the property tape pruning depends on.
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
