//! Checked finite `f64` intervals for compile-time Pixels analysis.

use std::f64::consts::{FRAC_PI_2, PI, TAU};

#[cfg(test)]
use super::super::scalar::source_smooth_min;
use super::super::scalar::{source_cos, source_sin, source_sqrt};

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct F64Interval {
    pub lo: f64,
    pub hi: f64,
}

impl F64Interval {
    pub fn new(lo: f64, hi: f64) -> Result<Self, String> {
        if !lo.is_finite() || !hi.is_finite() {
            return Err(format!("non-finite interval endpoint [{lo},{hi}]"));
        }
        if lo > hi {
            return Err(format!("empty interval [{lo},{hi}]"));
        }
        Ok(Self { lo, hi })
    }

    pub fn point(value: f64) -> Result<Self, String> {
        Self::new(value, value)
    }

    pub fn hull(self, other: Self) -> Self {
        Self {
            lo: self.lo.min(other.lo),
            hi: self.hi.max(other.hi),
        }
    }

    pub fn intersect(self, other: Self) -> Option<Self> {
        Self::new(self.lo.max(other.lo), self.hi.min(other.hi)).ok()
    }

    pub fn contains(self, value: f64) -> bool {
        self.lo <= value && value <= self.hi
    }

    pub fn contains_zero(self) -> bool {
        self.contains(0.0)
    }

    pub fn abs_upper(self) -> f64 {
        self.lo.abs().max(self.hi.abs())
    }

    pub fn width(self) -> f64 {
        self.hi - self.lo
    }

    pub fn add_outward(self, other: Self) -> Result<Self, String> {
        outward(self.lo + other.lo, self.hi + other.hi)
    }

    pub fn sub_outward(self, other: Self) -> Result<Self, String> {
        outward(self.lo - other.hi, self.hi - other.lo)
    }

    pub fn mul_outward(self, other: Self) -> Result<Self, String> {
        let candidates = [
            self.lo * other.lo,
            self.lo * other.hi,
            self.hi * other.lo,
            self.hi * other.hi,
        ];
        candidates_outward(&candidates)
    }

    pub fn reciprocal(self) -> Result<Self, String> {
        if self.contains_zero() {
            return Err(format!("reciprocal interval contains zero: {self:?}"));
        }
        candidates_outward(&[1.0 / self.lo, 1.0 / self.hi])
    }

    pub fn div_outward(self, other: Self) -> Result<Self, String> {
        self.mul_outward(other.reciprocal()?)
    }

    pub fn neg(self) -> Self {
        Self {
            lo: -self.hi,
            hi: -self.lo,
        }
    }

    pub fn abs(self) -> Result<Self, String> {
        if self.contains_zero() {
            Self::new(0.0, next_up(self.abs_upper()))
        } else {
            candidates_outward(&[self.lo.abs(), self.hi.abs()])
        }
    }

    pub fn square(self) -> Result<Self, String> {
        if self.contains_zero() {
            F64Interval::new(0.0, next_up(self.abs_upper() * self.abs_upper()))
        } else {
            let mut result = candidates_outward(&[self.lo * self.lo, self.hi * self.hi])?;
            result.lo = result.lo.max(0.0);
            Ok(result)
        }
    }

    pub fn sqrt(self) -> Result<Self, String> {
        if self.lo < 0.0 {
            return Err(format!("sqrt domain reaches below zero: {self:?}"));
        }
        let mut result = outward(self.lo.sqrt(), self.hi.sqrt())?;
        result.lo = result.lo.max(0.0);
        Ok(result)
    }

    pub fn min(self, other: Self) -> Result<Self, String> {
        outward(self.lo.min(other.lo), self.hi.min(other.hi))
    }

    pub fn max(self, other: Self) -> Result<Self, String> {
        outward(self.lo.max(other.lo), self.hi.max(other.hi))
    }

    pub fn clamp(self, lo: Self, hi: Self) -> Result<Self, String> {
        if lo.hi > hi.lo {
            return Err(format!("clamp bounds can be reversed: lo={lo:?} hi={hi:?}"));
        }
        // `min(max(value, lo), hi)` is monotone in all three operands once
        // `lo <= hi` is proven for the complete product domain.
        let lower = self.lo.max(lo.lo).min(hi.lo);
        let upper = self.hi.max(lo.hi).min(hi.hi);
        outward(lower.min(upper), lower.max(upper))
    }

    pub fn sin(self) -> Result<Self, String> {
        trig_range(self, false)
    }

    pub fn cos(self) -> Result<Self, String> {
        trig_range(self, true)
    }

    /// Range of the authoritative source-language f32 addition. Inputs in a
    /// scalar tape are f32 values even though the compiler stores their
    /// enclosing endpoints as f64.
    pub fn add_f32(self, other: Self) -> Result<Self, String> {
        binary_f32_candidates(self, other, |a, b| a + b)
    }

    pub fn sub_f32(self, other: Self) -> Result<Self, String> {
        binary_f32_candidates(self, other, |a, b| a - b)
    }

    pub fn mul_f32(self, other: Self) -> Result<Self, String> {
        binary_f32_candidates(self, other, |a, b| a * b)
    }

    pub fn div_f32(self, other: Self) -> Result<Self, String> {
        if other.contains_zero() {
            return Err(format!("f32 division interval contains zero: {other:?}"));
        }
        binary_f32_candidates(self, other, |a, b| a / b)
    }

    pub fn abs_f32(self) -> Result<Self, String> {
        let (lo, hi) = enclosing_f32_endpoints(self)?;
        if lo <= 0.0 && hi >= 0.0 {
            f32_candidates(&[0.0, lo.abs(), hi.abs()])
        } else {
            f32_candidates(&[lo.abs(), hi.abs()])
        }
    }

    pub fn square_f32(self) -> Result<Self, String> {
        let (lo, hi) = enclosing_f32_endpoints(self)?;
        if lo <= 0.0 && hi >= 0.0 {
            f32_candidates(&[0.0, lo * lo, hi * hi])
        } else {
            f32_candidates(&[lo * lo, hi * hi])
        }
    }

    pub fn sqrt_source_f32(self) -> Result<Self, String> {
        if self.lo < 0.0 {
            return Err(format!("source sqrt domain reaches below zero: {self:?}"));
        }
        let (lo, hi) = enclosing_f32_endpoints(self)?;
        f32_candidates(&[source_sqrt(lo), source_sqrt(hi)])
    }

    pub fn rsqrt_source_f32(self) -> Result<Self, String> {
        if self.lo <= 0.0 {
            return Err(format!(
                "source reciprocal sqrt domain reaches zero: {self:?}"
            ));
        }
        let (lo, hi) = enclosing_f32_endpoints(self)?;
        let evaluate = |value| {
            let root = source_sqrt(value);
            if root <= 0.0 { 0.0 } else { 1.0 / root }
        };
        f32_candidates(&[evaluate(lo), evaluate(hi)])
    }

    pub fn min_f32(self, other: Self) -> Result<Self, String> {
        let (a_lo, a_hi) = enclosing_f32_endpoints(self)?;
        let (b_lo, b_hi) = enclosing_f32_endpoints(other)?;
        f32_candidates(&[a_lo.min(b_lo), a_hi.min(b_hi)])
    }

    pub fn max_f32(self, other: Self) -> Result<Self, String> {
        let (a_lo, a_hi) = enclosing_f32_endpoints(self)?;
        let (b_lo, b_hi) = enclosing_f32_endpoints(other)?;
        f32_candidates(&[a_lo.max(b_lo), a_hi.max(b_hi)])
    }

    pub fn clamp_f32(self, lo: Self, hi: Self) -> Result<Self, String> {
        if lo.hi > hi.lo {
            return Err(format!("clamp bounds can be reversed: lo={lo:?} hi={hi:?}"));
        }
        self.max_f32(lo)?.min_f32(hi)
    }

    pub fn sin_source_f32(self) -> Result<Self, String> {
        source_trig_range(self, false)
    }

    pub fn cos_source_f32(self) -> Result<Self, String> {
        source_trig_range(self, true)
    }

    /// Range the exact, rounded source implementation rather than the real
    /// smooth-min identity.  In particular, cancellation in the blend arm can
    /// round a tiny amount above `min(a, b)`.
    pub fn smooth_min_source_f32(self, other: Self, k: Self) -> Result<Self, String> {
        if k.lo <= 0.0 {
            return Err(format!("source smooth-min radius reaches zero: {k:?}"));
        }
        let half = Self::point(0.5)?;
        let one = Self::point(1.0)?;
        let h = half.add_f32(half.mul_f32(other.sub_f32(self)?)?.div_f32(k)?)?;
        let blend = other
            .add_f32(self.sub_f32(other)?.mul_f32(h)?)?
            .sub_f32(k.mul_f32(h)?.mul_f32(one.sub_f32(h)?)?)?;

        // Either endpoint branch or the blend branch may be selected for
        // values inside the input box.  Interval evaluation of the blend arm
        // follows the source operation order exactly.
        Ok(self.hull(other).hull(blend))
    }
}

pub fn next_down_f32(value: f32) -> f32 {
    if value.is_nan() || value == f32::NEG_INFINITY {
        return value;
    }
    if value == 0.0 {
        return -f32::from_bits(1);
    }
    let bits = value.to_bits();
    f32::from_bits(if value > 0.0 { bits - 1 } else { bits + 1 })
}

pub fn next_up_f32(value: f32) -> f32 {
    if value.is_nan() || value == f32::INFINITY {
        return value;
    }
    if value == 0.0 {
        return f32::from_bits(1);
    }
    let bits = value.to_bits();
    f32::from_bits(if value > 0.0 { bits + 1 } else { bits - 1 })
}

pub fn next_down(value: f64) -> f64 {
    if value.is_nan() || value == f64::NEG_INFINITY {
        return value;
    }
    if value == 0.0 {
        return -f64::from_bits(1);
    }
    let bits = value.to_bits();
    f64::from_bits(if value > 0.0 { bits - 1 } else { bits + 1 })
}

pub fn next_up(value: f64) -> f64 {
    if value.is_nan() || value == f64::INFINITY {
        return value;
    }
    if value == 0.0 {
        return f64::from_bits(1);
    }
    let bits = value.to_bits();
    f64::from_bits(if value > 0.0 { bits + 1 } else { bits - 1 })
}

fn outward(lo: f64, hi: f64) -> Result<F64Interval, String> {
    if !lo.is_finite() || !hi.is_finite() {
        return Err(format!(
            "interval operation produced non-finite endpoints [{lo},{hi}]"
        ));
    }
    F64Interval::new(next_down(lo), next_up(hi))
}

fn candidates_outward(candidates: &[f64]) -> Result<F64Interval, String> {
    if candidates.iter().any(|value| !value.is_finite()) {
        return Err(format!(
            "interval operation produced a non-finite candidate: {candidates:?}"
        ));
    }
    let lo = candidates.iter().copied().fold(f64::INFINITY, f64::min);
    let hi = candidates.iter().copied().fold(f64::NEG_INFINITY, f64::max);
    outward(lo, hi)
}

fn enclosing_f32_endpoints(interval: F64Interval) -> Result<(f32, f32), String> {
    if interval.lo < -(f32::MAX as f64) || interval.hi > f32::MAX as f64 {
        return Err(format!(
            "interval is outside the finite f32 scalar domain: {interval:?}"
        ));
    }
    let mut lo = interval.lo as f32;
    if f64::from(lo) > interval.lo {
        lo = next_down_f32(lo);
    }
    let mut hi = interval.hi as f32;
    if f64::from(hi) < interval.hi {
        hi = next_up_f32(hi);
    }
    if !lo.is_finite() || !hi.is_finite() {
        return Err(format!(
            "interval has no finite enclosing f32 endpoints: {interval:?}"
        ));
    }
    Ok((lo, hi))
}

fn f32_candidates(candidates: &[f32]) -> Result<F64Interval, String> {
    if candidates.iter().any(|value| !value.is_finite()) {
        return Err(format!(
            "f32 interval operation produced a non-finite candidate: {candidates:?}"
        ));
    }
    let lo = candidates.iter().copied().fold(f32::INFINITY, f32::min);
    let hi = candidates.iter().copied().fold(f32::NEG_INFINITY, f32::max);
    F64Interval::new(f64::from(lo), f64::from(hi))
}

fn binary_f32_candidates(
    a: F64Interval,
    b: F64Interval,
    operation: impl Fn(f32, f32) -> f32,
) -> Result<F64Interval, String> {
    let (a_lo, a_hi) = enclosing_f32_endpoints(a)?;
    let (b_lo, b_hi) = enclosing_f32_endpoints(b)?;
    f32_candidates(&[
        operation(a_lo, b_lo),
        operation(a_lo, b_hi),
        operation(a_hi, b_lo),
        operation(a_hi, b_hi),
    ])
}

fn source_sin_polynomial_range(x: F64Interval) -> Result<F64Interval, String> {
    let x2 = x.square_f32()?;
    let constant = |value: f32| F64Interval::point(f64::from(value));
    let mut polynomial = constant(-0.00000002505210838544172_f32)?;
    polynomial = constant(0.0000027557319223985893_f32)?.add_f32(x2.mul_f32(polynomial)?)?;
    polynomial = constant(-0.0001984126984126984_f32)?.add_f32(x2.mul_f32(polynomial)?)?;
    polynomial = constant(0.008333333333333333_f32)?.add_f32(x2.mul_f32(polynomial)?)?;
    polynomial = constant(-0.16666666666666666_f32)?.add_f32(x2.mul_f32(polynomial)?)?;
    x.mul_f32(constant(1.0)?.add_f32(x2.mul_f32(polynomial)?)?)
}

fn source_trig_range(interval: F64Interval, cosine: bool) -> Result<F64Interval, String> {
    let half_pi = F64Interval::point(f64::from(1.5707963267948966_f32))?;
    let interval = if cosine {
        interval.add_f32(half_pi)?
    } else {
        interval
    };
    let (lo, hi) = enclosing_f32_endpoints(interval)?;
    let two_pi = 6.283185307179586_f32;
    let pi = 3.141592653589793_f32;
    let half_pi = 1.5707963267948966_f32;
    let lo_cell = (lo / two_pi).trunc();
    let hi_cell = (hi / two_pi).trunc();
    // A folded source-sine branch is never wider than pi/2.  Checking this
    // before comparing truncated period cells is essential: positive values
    // on opposite sides of the `pi` fold (for example [0.1, 6.0]) both have
    // truncated period cell zero even though the interval contains an
    // extremum.  Wide or non-finite-width domains therefore take the complete
    // folded polynomial domain.  Narrow domains still use the exact source
    // modulo/fold branches below.
    let source_width = f64::from(hi) - f64::from(lo);
    let reduced = if source_width >= f64::from(half_pi) {
        F64Interval::new(f64::from(-half_pi), f64::from(half_pi))?
    } else if lo_cell == hi_cell {
        let adjust = |value: f32| {
            let mut reduced = value % two_pi;
            if reduced > pi {
                reduced -= two_pi;
            } else if reduced < -pi {
                reduced += two_pi;
            }
            if reduced > half_pi {
                (pi - reduced, 1_i8)
            } else if reduced < -half_pi {
                (-pi - reduced, -1_i8)
            } else {
                (reduced, 0_i8)
            }
        };
        let (reduced_lo, lo_branch) = adjust(lo);
        let (reduced_hi, hi_branch) = adjust(hi);
        if lo_branch == hi_branch {
            F64Interval::new(
                f64::from(reduced_lo.min(reduced_hi)),
                f64::from(reduced_lo.max(reduced_hi)),
            )?
        } else {
            F64Interval::new(f64::from(-half_pi), f64::from(half_pi))?
        }
    } else {
        F64Interval::new(f64::from(-half_pi), f64::from(half_pi))?
    };
    let polynomial = source_sin_polynomial_range(reduced)?;
    // Endpoint evaluation both catches the common narrow monotone case and
    // guards the interval implementation against accidental over-tightening.
    let endpoint_values = if cosine {
        [source_cos(lo), source_cos(hi)]
    } else {
        [source_sin(lo), source_sin(hi)]
    };
    Ok(polynomial.hull(f32_candidates(&endpoint_values)?))
}

pub fn source_trig_is_smooth_on(interval: F64Interval, cosine: bool) -> Result<bool, String> {
    let half_pi_interval = F64Interval::point(f64::from(1.5707963267948966_f32))?;
    let interval = if cosine {
        interval.add_f32(half_pi_interval)?
    } else {
        interval
    };
    let (lo, hi) = enclosing_f32_endpoints(interval)?;
    if lo == hi {
        return Ok(true);
    }
    let two_pi = 6.283185307179586_f32;
    let pi = 3.141592653589793_f32;
    let half_pi = 1.5707963267948966_f32;
    if f64::from(hi) - f64::from(lo) >= f64::from(half_pi) {
        return Ok(false);
    }
    let classify = |value: f32| {
        let period_cell = (value / two_pi).trunc().to_bits();
        let mut reduced = value % two_pi;
        if reduced > pi {
            reduced -= two_pi;
        } else if reduced < -pi {
            reduced += two_pi;
        }
        let fold = if reduced > half_pi {
            1_i8
        } else if reduced < -half_pi {
            -1_i8
        } else {
            0_i8
        };
        (period_cell, fold)
    };
    Ok(classify(lo) == classify(hi))
}

fn trig_range(interval: F64Interval, cosine: bool) -> Result<F64Interval, String> {
    let width = interval.width();
    if width >= TAU {
        return F64Interval::new(-1.0, 1.0);
    }
    let evaluate = |value: f64| if cosine { value.cos() } else { value.sin() };
    let mut lo = evaluate(interval.lo).min(evaluate(interval.hi));
    let mut hi = evaluate(interval.lo).max(evaluate(interval.hi));
    // Reduce the interval origin before looking for critical points. This
    // avoids the lossy `ceil((x-phase)/TAU)` calculation for large but finite
    // source arguments.
    let reduced_lo = interval.lo.rem_euclid(TAU);
    let reduced_hi = reduced_lo + width;
    let contains_reduced = |phase: f64| {
        (reduced_lo <= phase && phase <= reduced_hi)
            || (reduced_lo <= phase + TAU && phase + TAU <= reduced_hi)
    };
    let max_phase = if cosine { 0.0 } else { FRAC_PI_2 };
    let min_phase = if cosine { PI } else { 3.0 * FRAC_PI_2 };
    if contains_reduced(max_phase) {
        hi = 1.0;
    }
    if contains_reduced(min_phase) {
        lo = -1.0;
    }
    let mut result = outward(lo, hi)?;
    result.lo = result.lo.max(-1.0);
    result.hi = result.hi.min(1.0);
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn checked_construction_and_zero_reciprocal_fail_closed() {
        assert!(F64Interval::new(2.0, 1.0).is_err());
        assert!(F64Interval::new(f64::NAN, 1.0).is_err());
        assert!(F64Interval::new(-1.0, 1.0).unwrap().reciprocal().is_err());
    }

    #[test]
    fn arithmetic_contains_endpoint_results() {
        let a = F64Interval::new(-2.0, 3.0).unwrap();
        let b = F64Interval::new(4.0, 5.0).unwrap();
        let product = a.mul_outward(b).unwrap();
        for x in [-2.0, 0.0, 3.0] {
            for y in [4.0, 4.5, 5.0] {
                assert!(product.contains(x * y));
            }
        }
        assert!(a.square().unwrap().contains(9.0));
        assert!(a.abs().unwrap().contains(0.0));
    }

    #[test]
    fn trig_checks_critical_points_instead_of_always_using_unit_range() {
        let small = F64Interval::new(0.0, 0.1).unwrap().sin().unwrap();
        assert!(small.lo <= 0.0);
        assert!(small.hi < 0.2);
        assert_eq!(
            F64Interval::new(0.0, TAU).unwrap().cos().unwrap(),
            F64Interval::new(-1.0, 1.0).unwrap()
        );
        let large = 1.0e15 * TAU;
        let reduced = F64Interval::new(large, large + 0.5).unwrap().sin().unwrap();
        for sample in [large, large + 0.25, large + 0.5] {
            assert!(reduced.contains(sample.sin()));
        }
        assert!(reduced.lo >= -1.0 && reduced.hi <= 1.0);
    }

    #[test]
    fn source_trig_wide_same_truncated_cell_contains_internal_fold_extrema() {
        let interval = F64Interval::new(0.1, 6.0).unwrap();
        let range = interval.sin_source_f32().unwrap();
        let extremum = f64::from(source_sin(1.5707963267948966_f32));
        assert!(range.contains(extremum), "{range:?} misses {extremum}");
        assert!(!source_trig_is_smooth_on(interval, false).unwrap());

        let narrow = F64Interval::new(0.1, 0.2).unwrap();
        assert!(source_trig_is_smooth_on(narrow, false).unwrap());
    }

    #[test]
    fn analytic_edge_cases_keep_domains_and_signed_zero_conservative() {
        let negative_zero = F64Interval::point(-0.0).unwrap();
        assert_eq!(negative_zero.lo.to_bits(), (-0.0_f64).to_bits());
        assert!(negative_zero.abs().unwrap().contains(0.0));
        let subnormal = f64::from_bits(1);
        assert!(
            F64Interval::new(subnormal, subnormal * 2.0)
                .unwrap()
                .sqrt()
                .unwrap()
                .lo
                >= 0.0
        );
        assert!(
            F64Interval::new(-subnormal, subnormal)
                .unwrap()
                .reciprocal()
                .is_err()
        );
        assert!(
            F64Interval::new(1.0, 2.0)
                .unwrap()
                .clamp(
                    F64Interval::new(0.0, 3.0).unwrap(),
                    F64Interval::new(2.0, 4.0).unwrap()
                )
                .is_err()
        );
        let varying = F64Interval::point(-10.0)
            .unwrap()
            .clamp(
                F64Interval::new(0.0, 1.0).unwrap(),
                F64Interval::new(2.0, 3.0).unwrap(),
            )
            .unwrap();
        assert!(varying.contains(0.0));
        assert!(varying.contains(1.0));
    }

    #[test]
    fn deterministic_grid_is_contained_for_every_operation() {
        let intervals = [
            F64Interval::new(-10.0, -0.125).unwrap(),
            F64Interval::new(-1.0, 1.0).unwrap(),
            F64Interval::new(0.125, 10.0).unwrap(),
        ];
        let mut state = 0x9e37_79b9_7f4a_7c15_u64;
        for _ in 0..100_000 {
            state ^= state << 7;
            state ^= state >> 9;
            let a = intervals[(state as usize) % intervals.len()];
            state = state.wrapping_mul(0xd134_2543_de82_ef95);
            let b = intervals[(state as usize) % intervals.len()];
            let t = ((state >> 11) as f64) / ((u64::MAX >> 11) as f64);
            let x = a.lo + (a.hi - a.lo) * t;
            let y = b.lo + (b.hi - b.lo) * (1.0 - t);
            let x32 = x as f32;
            let y32 = y as f32;
            assert!(a.add_outward(b).unwrap().contains(x + y));
            assert!(a.sub_outward(b).unwrap().contains(x - y));
            assert!(a.mul_outward(b).unwrap().contains(x * y));
            assert!(a.abs().unwrap().contains(x.abs()));
            assert!(a.square().unwrap().contains(x * x));
            assert!(a.min(b).unwrap().contains(x.min(y)));
            assert!(a.max(b).unwrap().contains(x.max(y)));
            assert!(a.sin().unwrap().contains(x.sin()));
            assert!(a.cos().unwrap().contains(x.cos()));
            assert!(a.add_f32(b).unwrap().contains(f64::from(x32 + y32)));
            assert!(a.sub_f32(b).unwrap().contains(f64::from(x32 - y32)));
            assert!(a.mul_f32(b).unwrap().contains(f64::from(x32 * y32)));
            assert!(a.abs_f32().unwrap().contains(f64::from(x32.abs())));
            assert!(a.square_f32().unwrap().contains(f64::from(x32 * x32)));
            assert!(a.min_f32(b).unwrap().contains(f64::from(x32.min(y32))));
            assert!(a.max_f32(b).unwrap().contains(f64::from(x32.max(y32))));
            assert!(
                a.sin_source_f32()
                    .unwrap()
                    .contains(f64::from(source_sin(x32)))
            );
            assert!(
                a.cos_source_f32()
                    .unwrap()
                    .contains(f64::from(source_cos(x32)))
            );
            let positive = F64Interval::new(0.125, 10.0).unwrap();
            let divisor = 0.125 + 9.875 * t;
            let divisor32 = divisor as f32;
            assert!(a.div_outward(positive).unwrap().contains(x / divisor));
            assert!(positive.sqrt().unwrap().contains(divisor.sqrt()));
            assert!(
                a.div_f32(positive)
                    .unwrap()
                    .contains(f64::from(x32 / divisor32))
            );
            assert!(
                positive
                    .sqrt_source_f32()
                    .unwrap()
                    .contains(f64::from(source_sqrt(divisor32)))
            );
            assert!(
                a.smooth_min_source_f32(b, positive)
                    .unwrap()
                    .contains(f64::from(source_smooth_min(x32, y32, divisor32)))
            );
            let clamp_lo = F64Interval::point(-0.5).unwrap();
            let clamp_hi = F64Interval::point(0.5).unwrap();
            assert!(
                a.clamp(clamp_lo, clamp_hi)
                    .unwrap()
                    .contains(x.clamp(-0.5, 0.5))
            );
            let varying_lo = F64Interval::new(-0.75, -0.25).unwrap();
            let varying_hi = F64Interval::new(0.25, 0.75).unwrap();
            let low_value = varying_lo.lo + varying_lo.width() * t;
            let high_value = varying_hi.lo + varying_hi.width() * (1.0 - t);
            assert!(
                a.clamp(varying_lo, varying_hi)
                    .unwrap()
                    .contains(x.clamp(low_value, high_value))
            );
        }
        let three = F64Interval::point(3.0).unwrap();
        assert!(
            three
                .sin_source_f32()
                .unwrap()
                .contains(f64::from(source_sin(3.0)))
        );
    }

    #[test]
    fn source_smooth_min_contains_cancellation_above_real_minimum() {
        let a = F64Interval::point(f64::from(f32::from_bits(0x0000_0000))).unwrap();
        let b = F64Interval::point(f64::from(f32::from_bits(0x3f68_7c91))).unwrap();
        let k = F64Interval::point(f64::from(f32::from_bits(0x3f68_8000))).unwrap();
        let actual = source_smooth_min(a.lo as f32, b.lo as f32, k.lo as f32);
        assert_eq!(actual.to_bits(), 0x32e2_8000);
        assert!(
            a.smooth_min_source_f32(b, k)
                .unwrap()
                .contains(f64::from(actual))
        );
    }
}
