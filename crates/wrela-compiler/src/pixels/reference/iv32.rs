//! Checked dyadic interval arithmetic used by Pixels proof predicates.

use std::fmt;

pub const MIN_EXPONENT: i16 = -96;
pub const MAX_EXPONENT: i16 = 63;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NumericError {
    InvalidDomain,
    InvalidInterval,
    DomainMismatch,
    Overflow,
    NonFinite,
    DivisionByZero,
    UnsupportedShape,
    CapacityExceeded,
}

impl fmt::Display for NumericError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{self:?}")
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FixedDomain {
    pub exponent: i16,
    pub min_raw: i32,
    pub max_raw: i32,
}

impl FixedDomain {
    pub fn new(exponent: i16, min_raw: i32, max_raw: i32) -> Result<Self, NumericError> {
        let domain = Self {
            exponent,
            min_raw,
            max_raw,
        };
        domain.validate()?;
        Ok(domain)
    }

    pub const fn full(exponent: i16) -> Self {
        Self {
            exponent,
            min_raw: i32::MIN,
            max_raw: i32::MAX,
        }
    }

    pub fn validate(self) -> Result<(), NumericError> {
        if !(MIN_EXPONENT..=MAX_EXPONENT).contains(&self.exponent) || self.min_raw > self.max_raw {
            return Err(NumericError::InvalidDomain);
        }
        Ok(())
    }

    fn contains(self, value: i32) -> bool {
        self.min_raw <= value && value <= self.max_raw
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord)]
pub struct Iv32 {
    pub lo: i32,
    pub hi: i32,
}

impl Iv32 {
    pub fn new(lo: i32, hi: i32) -> Result<Self, NumericError> {
        if lo > hi {
            Err(NumericError::InvalidInterval)
        } else {
            Ok(Self { lo, hi })
        }
    }

    pub const fn point(raw: i32) -> Self {
        Self { lo: raw, hi: raw }
    }

    fn validate_in(self, domain: FixedDomain) -> Result<(), NumericError> {
        domain.validate()?;
        if self.lo > self.hi {
            return Err(NumericError::InvalidInterval);
        }
        if !domain.contains(self.lo) || !domain.contains(self.hi) {
            return Err(NumericError::DomainMismatch);
        }
        Ok(())
    }

    pub fn contains_zero(self) -> bool {
        self.lo <= 0 && self.hi >= 0
    }

    pub fn strict_positive(self) -> bool {
        self.lo > 0
    }

    pub fn strict_negative(self) -> bool {
        self.hi < 0
    }

    pub fn add(self, other: Self, domain: FixedDomain) -> Result<Self, NumericError> {
        self.validate_in(domain)?;
        other.validate_in(domain)?;
        checked_pair(
            i64::from(self.lo) + i64::from(other.lo),
            i64::from(self.hi) + i64::from(other.hi),
            domain,
        )
    }

    pub fn subtract(self, other: Self, domain: FixedDomain) -> Result<Self, NumericError> {
        self.validate_in(domain)?;
        other.validate_in(domain)?;
        checked_pair(
            i64::from(self.lo) - i64::from(other.hi),
            i64::from(self.hi) - i64::from(other.lo),
            domain,
        )
    }

    pub fn negate(self, domain: FixedDomain) -> Result<Self, NumericError> {
        self.validate_in(domain)?;
        checked_pair(-i64::from(self.hi), -i64::from(self.lo), domain)
    }

    pub fn multiply(self, other: Self, domain: FixedDomain) -> Result<Self, NumericError> {
        self.validate_in(domain)?;
        other.validate_in(domain)?;
        let products = [
            i64::from(self.lo) * i64::from(other.lo),
            i64::from(self.lo) * i64::from(other.hi),
            i64::from(self.hi) * i64::from(other.lo),
            i64::from(self.hi) * i64::from(other.hi),
        ];
        let lo = *products.iter().min().expect("four products");
        let hi = *products.iter().max().expect("four products");
        let (lo, hi) = rescale_product(lo, hi, domain.exponent)?;
        checked_pair(lo, hi, domain)
    }

    pub fn square(self, domain: FixedDomain) -> Result<Self, NumericError> {
        self.validate_in(domain)?;
        let magnitude = i64::from(self.lo)
            .unsigned_abs()
            .max(i64::from(self.hi).unsigned_abs());
        let high_product = i64::try_from(
            magnitude
                .checked_mul(magnitude)
                .ok_or(NumericError::Overflow)?,
        )
        .map_err(|_| NumericError::Overflow)?;
        let low_product = if self.contains_zero() {
            0
        } else {
            let magnitude = i64::from(self.lo)
                .unsigned_abs()
                .min(i64::from(self.hi).unsigned_abs());
            i64::try_from(
                magnitude
                    .checked_mul(magnitude)
                    .ok_or(NumericError::Overflow)?,
            )
            .map_err(|_| NumericError::Overflow)?
        };
        let (lo, hi) = rescale_product(low_product, high_product, domain.exponent)?;
        checked_pair(lo.max(0), hi.max(0), domain)
    }

    pub fn scale_pow2(self, shift: i16, domain: FixedDomain) -> Result<Self, NumericError> {
        self.validate_in(domain)?;
        let scale = |value: i32| -> Result<i64, NumericError> {
            let bits = u32::try_from(shift).map_err(|_| NumericError::Overflow)?;
            let factor = 1_i128.checked_shl(bits).ok_or(NumericError::Overflow)?;
            i64::try_from(
                i128::from(value)
                    .checked_mul(factor)
                    .ok_or(NumericError::Overflow)?,
            )
            .map_err(|_| NumericError::Overflow)
        };
        if shift >= 0 {
            checked_pair(scale(self.lo)?, scale(self.hi)?, domain)
        } else {
            let bits = u32::try_from(-i32::from(shift)).map_err(|_| NumericError::Overflow)?;
            checked_pair(
                floor_div_pow2(i64::from(self.lo), bits),
                ceil_div_pow2(i64::from(self.hi), bits),
                domain,
            )
        }
    }

    pub fn intersect(self, other: Self, domain: FixedDomain) -> Result<Option<Self>, NumericError> {
        self.validate_in(domain)?;
        other.validate_in(domain)?;
        let lo = self.lo.max(other.lo);
        let hi = self.hi.min(other.hi);
        Ok((lo <= hi).then_some(Self { lo, hi }))
    }

    pub fn hull(self, other: Self, domain: FixedDomain) -> Result<Self, NumericError> {
        self.validate_in(domain)?;
        other.validate_in(domain)?;
        Ok(Self {
            lo: self.lo.min(other.lo),
            hi: self.hi.max(other.hi),
        })
    }

    pub fn min(self, other: Self, domain: FixedDomain) -> Result<Self, NumericError> {
        self.validate_in(domain)?;
        other.validate_in(domain)?;
        Ok(Self {
            lo: self.lo.min(other.lo),
            hi: self.hi.min(other.hi),
        })
    }

    pub fn max(self, other: Self, domain: FixedDomain) -> Result<Self, NumericError> {
        self.validate_in(domain)?;
        other.validate_in(domain)?;
        Ok(Self {
            lo: self.lo.max(other.lo),
            hi: self.hi.max(other.hi),
        })
    }

    pub fn abs(self, domain: FixedDomain) -> Result<Self, NumericError> {
        self.validate_in(domain)?;
        if self.contains_zero() {
            checked_pair(
                0,
                i64::from(self.lo).abs().max(i64::from(self.hi).abs()),
                domain,
            )
        } else {
            checked_pair(
                i64::from(self.lo).abs().min(i64::from(self.hi).abs()),
                i64::from(self.lo).abs().max(i64::from(self.hi).abs()),
                domain,
            )
        }
    }

    pub fn affine(
        input: Self,
        scale: Self,
        bias: Self,
        domain: FixedDomain,
    ) -> Result<Self, NumericError> {
        input.multiply(scale, domain)?.add(bias, domain)
    }

    pub fn convert(
        self,
        source: FixedDomain,
        destination: FixedDomain,
    ) -> Result<Self, NumericError> {
        self.validate_in(source)?;
        destination.validate()?;
        let shift = source.exponent - destination.exponent;
        if shift >= 0 {
            let bits = u32::try_from(shift).map_err(|_| NumericError::Overflow)?;
            let factor = 1_i128.checked_shl(bits).ok_or(NumericError::Overflow)?;
            let lo = i128::from(self.lo)
                .checked_mul(factor)
                .ok_or(NumericError::Overflow)?;
            let hi = i128::from(self.hi)
                .checked_mul(factor)
                .ok_or(NumericError::Overflow)?;
            checked_pair_i128(lo, hi, destination)
        } else {
            let bits = u32::try_from(-shift).map_err(|_| NumericError::Overflow)?;
            checked_pair(
                floor_div_pow2(i64::from(self.lo), bits),
                ceil_div_pow2(i64::from(self.hi), bits),
                destination,
            )
        }
    }

    pub fn from_f32_bits(
        bits: u32,
        radius_raw: u32,
        domain: FixedDomain,
    ) -> Result<Self, NumericError> {
        domain.validate()?;
        let value = f32::from_bits(bits);
        if !value.is_finite() {
            return Err(NumericError::NonFinite);
        }
        let (numerator, binary_exponent) = exact_f32_integer(bits);
        let shift = binary_exponent - i32::from(domain.exponent);
        let (floor, ceil) = if shift >= 0 {
            let bits = u32::try_from(shift).map_err(|_| NumericError::Overflow)?;
            let factor = 1_i128.checked_shl(bits).ok_or(NumericError::Overflow)?;
            let exact = numerator
                .checked_mul(factor)
                .ok_or(NumericError::Overflow)?;
            (exact, exact)
        } else {
            let bits = u32::try_from(-shift).map_err(|_| NumericError::Overflow)?;
            (
                floor_div_pow2_i128(numerator, bits),
                ceil_div_pow2_i128(numerator, bits),
            )
        };
        let radius = i128::from(radius_raw);
        checked_pair_i128(floor - radius, ceil + radius, domain)
    }

    pub fn nearer_than(self, other: Self) -> bool {
        self.lo > other.hi
    }

    pub fn width_raw(self) -> Result<u32, NumericError> {
        u32::try_from(i64::from(self.hi) - i64::from(self.lo)).map_err(|_| NumericError::Overflow)
    }
}

fn exact_f32_integer(bits: u32) -> (i128, i32) {
    let negative = bits >> 31 != 0;
    let raw_exponent = ((bits >> 23) & 0xff) as i32;
    let fraction = bits & 0x7f_ffff;
    let (mantissa, exponent) = if raw_exponent == 0 {
        (i128::from(fraction), -149)
    } else {
        (
            i128::from((1_u32 << 23) | fraction),
            raw_exponent - 127 - 23,
        )
    };
    (if negative { -mantissa } else { mantissa }, exponent)
}

fn rescale_product(lo: i64, hi: i64, exponent: i16) -> Result<(i64, i64), NumericError> {
    if exponent <= 0 {
        let bits = u32::try_from(-exponent).map_err(|_| NumericError::Overflow)?;
        Ok((floor_div_pow2(lo, bits), ceil_div_pow2(hi, bits)))
    } else {
        let bits = u32::try_from(exponent).map_err(|_| NumericError::Overflow)?;
        let factor = 1_i128.checked_shl(bits).ok_or(NumericError::Overflow)?;
        let lo = i128::from(lo)
            .checked_mul(factor)
            .ok_or(NumericError::Overflow)?;
        let hi = i128::from(hi)
            .checked_mul(factor)
            .ok_or(NumericError::Overflow)?;
        Ok((
            i64::try_from(lo).map_err(|_| NumericError::Overflow)?,
            i64::try_from(hi).map_err(|_| NumericError::Overflow)?,
        ))
    }
}

fn floor_div_pow2(value: i64, bits: u32) -> i64 {
    if bits >= 63 {
        return if value < 0 { -1 } else { 0 };
    }
    value >> bits
}

fn ceil_div_pow2(value: i64, bits: u32) -> i64 {
    if bits == 0 {
        return value;
    }
    if bits >= 63 {
        return if value > 0 { 1 } else { 0 };
    }
    -((-value) >> bits)
}

fn floor_div_pow2_i128(value: i128, bits: u32) -> i128 {
    if bits >= 127 {
        return if value < 0 { -1 } else { 0 };
    }
    value >> bits
}

fn ceil_div_pow2_i128(value: i128, bits: u32) -> i128 {
    if bits == 0 {
        return value;
    }
    if bits >= 127 {
        return if value > 0 { 1 } else { 0 };
    }
    -((-value) >> bits)
}

fn checked_pair(lo: i64, hi: i64, domain: FixedDomain) -> Result<Iv32, NumericError> {
    checked_pair_i128(i128::from(lo), i128::from(hi), domain)
}

fn checked_pair_i128(lo: i128, hi: i128, domain: FixedDomain) -> Result<Iv32, NumericError> {
    let lo = i32::try_from(lo).map_err(|_| NumericError::Overflow)?;
    let hi = i32::try_from(hi).map_err(|_| NumericError::Overflow)?;
    if !domain.contains(lo) || !domain.contains(hi) {
        return Err(NumericError::Overflow);
    }
    Iv32::new(lo, hi)
}

#[cfg(test)]
mod tests {
    use super::*;

    const D: FixedDomain = FixedDomain::full(-4);

    #[test]
    fn checked_interval_arithmetic_is_outward() {
        assert_eq!(
            Iv32::new(-3, 7).unwrap().add(Iv32::new(2, 9).unwrap(), D),
            Ok(Iv32 { lo: -1, hi: 16 })
        );
        assert_eq!(
            Iv32::new(-3, 7)
                .unwrap()
                .multiply(Iv32::new(2, 9).unwrap(), D),
            Ok(Iv32 { lo: -2, hi: 4 })
        );
        assert_eq!(
            Iv32::new(-3, 7).unwrap().square(D),
            Ok(Iv32 { lo: 0, hi: 4 })
        );
    }

    #[test]
    fn conversion_rounds_outward_for_negative_values() {
        let coarse = FixedDomain::full(-2);
        let fine = FixedDomain::full(-4);
        assert_eq!(
            Iv32::new(-3, 7).unwrap().convert(coarse, fine),
            Ok(Iv32 { lo: -12, hi: 28 })
        );
        assert_eq!(
            Iv32::new(-3, 7).unwrap().convert(fine, coarse),
            Ok(Iv32 { lo: -1, hi: 2 })
        );
    }

    #[test]
    fn f32_conversion_uses_exact_bits_and_rejects_nonfinite() {
        assert_eq!(
            Iv32::from_f32_bits(0.5_f32.to_bits(), 1, D),
            Ok(Iv32 { lo: 7, hi: 9 })
        );
        assert_eq!(
            Iv32::from_f32_bits(f32::INFINITY.to_bits(), 0, D),
            Err(NumericError::NonFinite)
        );
        assert_eq!(
            Iv32::from_f32_bits(0, 0, FixedDomain::full(100)),
            Err(NumericError::InvalidDomain)
        );
        assert_eq!(
            Iv32::from_f32_bits(1, 0, FixedDomain::full(-96)),
            Ok(Iv32::new(0, 1).unwrap())
        );
    }

    #[test]
    fn every_i8_interval_and_point_pair_matches_integer_model() {
        let domain = FixedDomain {
            exponent: 0,
            min_raw: i32::MIN,
            max_raw: i32::MAX,
        };
        for lo in i8::MIN..=i8::MAX {
            for hi in lo..=i8::MAX {
                let a = Iv32::new(i32::from(lo), i32::from(hi)).unwrap();
                assert_eq!(a.negate(domain).unwrap().lo, -i32::from(hi));
                assert_eq!(
                    a.abs(domain).unwrap().hi,
                    i32::from(lo).abs().max(i32::from(hi).abs())
                );
                assert_eq!(a.square(domain).unwrap().lo >= 0, true);
                assert_eq!(a.hull(a, domain), Ok(a));
                assert_eq!(a.intersect(a, domain), Ok(Some(a)));
            }
        }
        for left in i8::MIN..=i8::MAX {
            for right in i8::MIN..=i8::MAX {
                let a = Iv32::point(i32::from(left));
                let b = Iv32::point(i32::from(right));
                assert_eq!(
                    a.add(b, domain),
                    Ok(Iv32::point(i32::from(left) + i32::from(right)))
                );
                assert_eq!(
                    a.subtract(b, domain),
                    Ok(Iv32::point(i32::from(left) - i32::from(right)))
                );
                assert_eq!(
                    a.multiply(b, domain),
                    Ok(Iv32::point(i32::from(left) * i32::from(right)))
                );
                assert_eq!(
                    a.min(b, domain),
                    Ok(Iv32::point(i32::from(left.min(right))))
                );
                assert_eq!(
                    a.max(b, domain),
                    Ok(Iv32::point(i32::from(left.max(right))))
                );
                assert_eq!(
                    a.hull(b, domain),
                    Iv32::new(i32::from(left.min(right)), i32::from(left.max(right)))
                );
                assert_eq!(a.intersect(b, domain), Ok((left == right).then_some(a)));
            }
        }
    }

    #[test]
    fn scale_pow2_covers_v1_exponent_boundaries() {
        let domain = FixedDomain::full(0);
        for shift in [-96, -63, 30, 31, 63] {
            let expected = match shift {
                -96 | -63 => Ok(Iv32::new(0, 1).unwrap()),
                30 => Ok(Iv32::new(0, 1 << 30).unwrap()),
                31 | 63 => Err(NumericError::Overflow),
                _ => unreachable!(),
            };
            assert_eq!(Iv32::new(0, 1).unwrap().scale_pow2(shift, domain), expected);
        }
        assert_eq!(Iv32::point(0).scale_pow2(63, domain), Ok(Iv32::point(0)));
    }

    #[test]
    fn restricted_domains_reject_results_outside_the_sealed_raw_range() {
        let domain = FixedDomain::new(0, -100, 100).unwrap();
        assert_eq!(
            Iv32::point(100).add(Iv32::point(1), domain),
            Err(NumericError::Overflow)
        );
        assert_eq!(
            Iv32::point(99).add(Iv32::point(1), domain),
            Ok(Iv32::point(100))
        );
        assert_eq!(
            FixedDomain::new(64, -100, 100),
            Err(NumericError::InvalidDomain)
        );
        assert_eq!(FixedDomain::new(0, 1, -1), Err(NumericError::InvalidDomain));
    }

    #[test]
    fn scale_pow2_supports_the_full_shared_i16_shift_domain() {
        let domain = FixedDomain::full(0);
        assert_eq!(
            Iv32::new(0, 1).unwrap().scale_pow2(-159, domain).unwrap(),
            Iv32::new(0, 1).unwrap()
        );
        assert_eq!(Iv32::point(0).scale_pow2(127, domain), Ok(Iv32::point(0)));
        assert_eq!(
            Iv32::point(0).scale_pow2(128, domain),
            Err(NumericError::Overflow)
        );
        assert_eq!(
            Iv32::point(-1).scale_pow2(31, domain),
            Ok(Iv32::point(i32::MIN))
        );
    }

    #[test]
    fn errors_never_saturate_or_wrap() {
        let point = Iv32::point(i32::MAX);
        assert_eq!(
            point.add(Iv32::point(1), FixedDomain::full(0)),
            Err(NumericError::Overflow)
        );
        assert_eq!(
            Iv32::point(i32::MIN).negate(FixedDomain::full(0)),
            Err(NumericError::Overflow)
        );
        assert_eq!(
            FixedDomain::full(MIN_EXPONENT - 1).validate(),
            Err(NumericError::InvalidDomain)
        );
        assert_eq!(
            Iv32::point(i32::MAX).scale_pow2(40, FixedDomain::full(0)),
            Err(NumericError::Overflow)
        );
        assert_eq!(
            Iv32::point(i32::MAX).convert(FixedDomain::full(40), FixedDomain::full(0)),
            Err(NumericError::Overflow)
        );
    }
}
