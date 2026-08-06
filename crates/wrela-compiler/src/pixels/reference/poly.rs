//! Fixed-capacity low-degree polynomial verifier kernels.

use super::iv32::{FixedDomain, Iv32, NumericError};

pub const MAX_DEGREE: usize = 8;
pub const MAX_COEFFICIENTS: usize = MAX_DEGREE + 1;
pub const MAX_SPARSE_TERMS: usize = 32;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Polynomial {
    pub coefficients: [Iv32; MAX_COEFFICIENTS],
    pub degree: u8,
}

impl Polynomial {
    pub fn new(coefficients: &[Iv32]) -> Result<Self, NumericError> {
        if coefficients.is_empty() || coefficients.len() > MAX_COEFFICIENTS {
            return Err(NumericError::UnsupportedShape);
        }
        let mut stored = [Iv32::point(0); MAX_COEFFICIENTS];
        stored[..coefficients.len()].copy_from_slice(coefficients);
        Ok(Self {
            coefficients: stored,
            degree: u8::try_from(coefficients.len() - 1)
                .map_err(|_| NumericError::UnsupportedShape)?,
        })
    }

    pub fn evaluate(self, x: Iv32, domain: FixedDomain) -> Result<Iv32, NumericError> {
        self.coefficients[..=usize::from(self.degree)]
            .iter()
            .rev()
            .try_fold(Iv32::point(0), |accumulator, coefficient| {
                accumulator.multiply(x, domain)?.add(*coefficient, domain)
            })
    }

    pub fn derivative(self, domain: FixedDomain) -> Result<Self, NumericError> {
        if self.degree == 0 {
            return Self::new(&[Iv32::point(0)]);
        }
        let mut coefficients = [Iv32::point(0); MAX_COEFFICIENTS];
        for power in 1..=usize::from(self.degree) {
            coefficients[power - 1] = multiply_integer(self.coefficients[power], power, domain)?;
        }
        Self::new(&coefficients[..usize::from(self.degree)])
    }

    pub fn to_bernstein(self, domain: FixedDomain) -> Result<Bernstein, NumericError> {
        let degree = usize::from(self.degree);
        let mut coefficients = [Iv32::point(0); MAX_COEFFICIENTS];
        for (k, output) in coefficients.iter_mut().enumerate().take(degree + 1) {
            let mut lo = Rational::zero();
            let mut hi = Rational::zero();
            for i in 0..=k {
                let ratio =
                    Rational::new(i128::from(binomial(k, i)), i128::from(binomial(degree, i)))?;
                lo = lo
                    .checked_add(ratio.checked_mul_integer(i128::from(self.coefficients[i].lo))?)?;
                hi = hi
                    .checked_add(ratio.checked_mul_integer(i128::from(self.coefficients[i].hi))?)?;
            }
            *output = interval_from_rationals(lo, hi, domain)?;
        }
        Ok(Bernstein {
            coefficients,
            degree: self.degree,
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Bernstein {
    pub coefficients: [Iv32; MAX_COEFFICIENTS],
    pub degree: u8,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CoefficientSign {
    StrictPositive,
    StrictNegative,
    Mixed,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SignVariation {
    Exact(u8),
    Inconclusive,
}

impl Bernstein {
    pub fn sign(self) -> CoefficientSign {
        let coefficients = &self.coefficients[..=usize::from(self.degree)];
        if coefficients.iter().all(|coefficient| coefficient.lo > 0) {
            CoefficientSign::StrictPositive
        } else if coefficients.iter().all(|coefficient| coefficient.hi < 0) {
            CoefficientSign::StrictNegative
        } else {
            CoefficientSign::Mixed
        }
    }

    pub fn sign_variations(self) -> SignVariation {
        let mut previous = 0_i8;
        let mut variations = 0_u8;
        for coefficient in &self.coefficients[..=usize::from(self.degree)] {
            let sign = if coefficient.lo > 0 {
                1
            } else if coefficient.hi < 0 {
                -1
            } else if coefficient.lo == 0 && coefficient.hi == 0 {
                continue;
            } else {
                return SignVariation::Inconclusive;
            };
            if previous != 0 && sign != previous {
                variations += 1;
            }
            previous = sign;
        }
        SignVariation::Exact(variations)
    }

    /// Exact de Casteljau subdivision at `1/2`, rounded outward in raw units.
    pub fn subdivide_half(self, domain: FixedDomain) -> Result<(Self, Self), NumericError> {
        self.subdivide_at(1, 2, domain)
    }

    /// De Casteljau subdivision at `numerator / denominator`, rounded
    /// outward in raw units. The rational split must match the q-cell split
    /// exactly; using `1/2` for an odd-width integer cell can relabel a root
    /// with an interval that does not contain it.
    pub fn subdivide_at(
        self,
        numerator: u64,
        denominator: u64,
        domain: FixedDomain,
    ) -> Result<(Self, Self), NumericError> {
        domain.validate()?;
        if numerator == 0 || numerator >= denominator {
            return Err(NumericError::InvalidDomain);
        }
        let degree = usize::from(self.degree);
        let mut triangle = [[Iv32::point(0); MAX_COEFFICIENTS]; MAX_COEFFICIENTS];
        triangle[0] = self.coefficients;
        for row in 1..=degree {
            for column in 0..=degree - row {
                triangle[row][column] = bernstein_lerp_ratio(
                    triangle[row - 1][column],
                    triangle[row - 1][column + 1],
                    numerator,
                    denominator,
                    domain,
                )?;
            }
        }
        let mut left = [Iv32::point(0); MAX_COEFFICIENTS];
        let mut right = [Iv32::point(0); MAX_COEFFICIENTS];
        for row in 0..=degree {
            left[row] = triangle[row][0];
            right[degree - row] = triangle[row][degree - row];
        }
        Ok((
            Self {
                coefficients: left,
                degree: self.degree,
            },
            Self {
                coefficients: right,
                degree: self.degree,
            },
        ))
    }
}

pub fn bernstein_lerp_ratio(
    a: Iv32,
    b: Iv32,
    numerator: u64,
    denominator: u64,
    domain: FixedDomain,
) -> Result<Iv32, NumericError> {
    if numerator > i64::MAX as u64
        || denominator > i64::MAX as u64
        || numerator == 0
        || numerator >= denominator
    {
        return Err(NumericError::InvalidDomain);
    }
    a.intersect(a, domain)?;
    b.intersect(b, domain)?;
    let numerator = i128::from(numerator);
    let denominator = i128::from(denominator);
    let complement = denominator - numerator;
    let low_numerator = i128::from(a.lo) * complement + i128::from(b.lo) * numerator;
    let high_numerator = i128::from(a.hi) * complement + i128::from(b.hi) * numerator;
    let lo = low_numerator.div_euclid(denominator);
    let hi = -(-high_numerator).div_euclid(denominator);
    let result = Iv32::new(
        i32::try_from(lo).map_err(|_| NumericError::Overflow)?,
        i32::try_from(hi).map_err(|_| NumericError::Overflow)?,
    )?;
    result
        .intersect(result, domain)?
        .ok_or(NumericError::DomainMismatch)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SparseTerm {
    pub coefficient: Iv32,
    pub powers: [u8; 4],
}

pub fn evaluate_sparse(
    terms: &[SparseTerm],
    variables: [Iv32; 4],
    domain: FixedDomain,
) -> Result<Iv32, NumericError> {
    if terms.len() > MAX_SPARSE_TERMS
        || terms.iter().any(|term| {
            term.powers
                .iter()
                .any(|power| usize::from(*power) > MAX_DEGREE)
        })
    {
        return Err(NumericError::UnsupportedShape);
    }
    let mut sum = Iv32::point(0);
    for term in terms {
        let mut value = term.coefficient;
        for (variable, power) in variables.into_iter().zip(term.powers) {
            for _ in 0..power {
                value = value.multiply(variable, domain)?;
            }
        }
        sum = sum.add(value, domain)?;
    }
    Ok(sum)
}

pub fn taylor_with_remainder(
    polynomial: Polynomial,
    x: Iv32,
    remainder: Iv32,
    domain: FixedDomain,
) -> Result<Iv32, NumericError> {
    polynomial.evaluate(x, domain)?.add(remainder, domain)
}

/// Checked generated composition for the sealed univariate schedules. The
/// result is `outer(candidate(x))`; shapes whose composed degree exceeds eight
/// fail explicitly so callers can take the interval/Taylor fallback.
pub fn compose_univariate(
    outer: Polynomial,
    candidate: Polynomial,
    domain: FixedDomain,
) -> Result<Polynomial, NumericError> {
    if usize::from(outer.degree) * usize::from(candidate.degree) > MAX_DEGREE {
        return Err(NumericError::UnsupportedShape);
    }
    let mut result = Polynomial::new(&[Iv32::point(0)])?;
    for coefficient in outer.coefficients[..=usize::from(outer.degree)]
        .iter()
        .rev()
    {
        result = multiply_polynomials(result, candidate, domain)?;
        result.coefficients[0] = result.coefficients[0].add(*coefficient, domain)?;
    }
    Ok(result)
}

fn multiply_polynomials(
    left: Polynomial,
    right: Polynomial,
    domain: FixedDomain,
) -> Result<Polynomial, NumericError> {
    let degree = usize::from(left.degree) + usize::from(right.degree);
    if degree > MAX_DEGREE {
        return Err(NumericError::UnsupportedShape);
    }
    let mut coefficients = [Iv32::point(0); MAX_COEFFICIENTS];
    for left_power in 0..=usize::from(left.degree) {
        for right_power in 0..=usize::from(right.degree) {
            let product =
                left.coefficients[left_power].multiply(right.coefficients[right_power], domain)?;
            coefficients[left_power + right_power] =
                coefficients[left_power + right_power].add(product, domain)?;
        }
    }
    Polynomial::new(&coefficients[..=degree])
}

/// `c + bu*u + bv*v + buu*u² + buv*u*v + bvv*v²` over `[0,1]²`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Quadratic2 {
    pub c: i32,
    pub bu: i32,
    pub bv: i32,
    pub buu: i32,
    pub buv: i32,
    pub bvv: i32,
}

pub fn exact_quadratic_range(
    quadratic: Quadratic2,
    domain: FixedDomain,
) -> Result<Iv32, NumericError> {
    domain.validate()?;
    let needs_interior_schedule = quadratic.buv != 0 || (quadratic.buu != 0 && quadratic.bvv != 0);
    if needs_interior_schedule
        && [
            quadratic.c,
            quadratic.bu,
            quadratic.bv,
            quadratic.buu,
            quadratic.buv,
            quadratic.bvv,
        ]
        .into_iter()
        .any(|coefficient| !(-512..=512).contains(&coefficient))
    {
        // The scalar guest uses i64 proof arithmetic for the coupled
        // stationary-point schedule. Share its sealed envelope explicitly.
        return Err(NumericError::Overflow);
    }
    let mut candidates = [Rational::zero(); 9];
    let mut count = 0_usize;
    for (u, v) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
        push_candidate(
            &mut candidates,
            &mut count,
            evaluate_quadratic(quadratic, Rational::integer(u), Rational::integer(v))?,
        )?;
    }

    let determinant = 4_i128
        .checked_mul(i128::from(quadratic.buu))
        .and_then(|value| value.checked_mul(i128::from(quadratic.bvv)))
        .and_then(|value| value.checked_sub(i128::from(quadratic.buv) * i128::from(quadratic.buv)))
        .ok_or(NumericError::Overflow)?;
    if determinant != 0 {
        let u = Rational::new(
            i128::from(quadratic.buv) * i128::from(quadratic.bv)
                - 2_i128 * i128::from(quadratic.bvv) * i128::from(quadratic.bu),
            determinant,
        )?;
        let v = Rational::new(
            i128::from(quadratic.buv) * i128::from(quadratic.bu)
                - 2_i128 * i128::from(quadratic.buu) * i128::from(quadratic.bv),
            determinant,
        )?;
        if u.in_unit() && v.in_unit() {
            push_candidate(
                &mut candidates,
                &mut count,
                evaluate_quadratic(quadratic, u, v)?,
            )?;
        }
    }

    for (derivative_constant, quadratic_coefficient, fixed_u, boundary_is_one) in [
        (i128::from(quadratic.bv), quadratic.bvv, true, false),
        (
            i128::from(quadratic.bv)
                .checked_add(i128::from(quadratic.buv))
                .ok_or(NumericError::Overflow)?,
            quadratic.bvv,
            true,
            true,
        ),
        (i128::from(quadratic.bu), quadratic.buu, false, false),
        (
            i128::from(quadratic.bu)
                .checked_add(i128::from(quadratic.buv))
                .ok_or(NumericError::Overflow)?,
            quadratic.buu,
            false,
            true,
        ),
    ] {
        if quadratic_coefficient != 0 {
            let free = Rational::new(
                derivative_constant
                    .checked_neg()
                    .ok_or(NumericError::Overflow)?,
                2_i128 * i128::from(quadratic_coefficient),
            )?;
            if free.in_unit() {
                let (u, v) = if fixed_u {
                    (Rational::integer(boundary_is_one as i32), free)
                } else {
                    (free, Rational::integer(boundary_is_one as i32))
                };
                push_candidate(
                    &mut candidates,
                    &mut count,
                    evaluate_quadratic(quadratic, u, v)?,
                )?;
            }
        }
    }

    let values = &candidates[..count];
    let mut lo = *values.first().ok_or(NumericError::UnsupportedShape)?;
    let mut hi = lo;
    for value in values.iter().copied().skip(1) {
        if value.checked_cmp(lo)?.is_lt() {
            lo = value;
        }
        if value.checked_cmp(hi)?.is_gt() {
            hi = value;
        }
    }
    interval_from_rationals(lo, hi, domain)
}

fn evaluate_quadratic(q: Quadratic2, u: Rational, v: Rational) -> Result<Rational, NumericError> {
    Rational::integer(q.c)
        .checked_add(u.checked_mul_integer(i128::from(q.bu))?)?
        .checked_add(v.checked_mul_integer(i128::from(q.bv))?)?
        .checked_add(u.checked_mul(u)?.checked_mul_integer(i128::from(q.buu))?)?
        .checked_add(u.checked_mul(v)?.checked_mul_integer(i128::from(q.buv))?)?
        .checked_add(v.checked_mul(v)?.checked_mul_integer(i128::from(q.bvv))?)
}

fn push_candidate(
    candidates: &mut [Rational; 9],
    count: &mut usize,
    value: Rational,
) -> Result<(), NumericError> {
    let slot = candidates
        .get_mut(*count)
        .ok_or(NumericError::CapacityExceeded)?;
    *slot = value;
    *count += 1;
    Ok(())
}

fn multiply_integer(
    interval: Iv32,
    value: usize,
    domain: FixedDomain,
) -> Result<Iv32, NumericError> {
    let value = i64::try_from(value).map_err(|_| NumericError::Overflow)?;
    checked_raw_interval(
        i64::from(interval.lo)
            .checked_mul(value)
            .ok_or(NumericError::Overflow)?,
        i64::from(interval.hi)
            .checked_mul(value)
            .ok_or(NumericError::Overflow)?,
        domain,
    )
}

pub fn bernstein_average(a: Iv32, b: Iv32, domain: FixedDomain) -> Result<Iv32, NumericError> {
    let low = i64::from(a.lo) + i64::from(b.lo);
    let high = i64::from(a.hi) + i64::from(b.hi);
    checked_raw_interval(
        low.div_euclid(2),
        high.div_euclid(2) + i64::from(high % 2 != 0),
        domain,
    )
}

fn checked_raw_interval(lo: i64, hi: i64, domain: FixedDomain) -> Result<Iv32, NumericError> {
    domain.validate()?;
    let lo = i32::try_from(lo).map_err(|_| NumericError::Overflow)?;
    let hi = i32::try_from(hi).map_err(|_| NumericError::Overflow)?;
    if lo < domain.min_raw || hi > domain.max_raw {
        return Err(NumericError::Overflow);
    }
    Iv32::new(lo, hi)
}

fn interval_from_rationals(
    lo: Rational,
    hi: Rational,
    domain: FixedDomain,
) -> Result<Iv32, NumericError> {
    let low = lo.floor()?;
    let high = hi.ceil()?;
    let low = i64::try_from(low).map_err(|_| NumericError::Overflow)?;
    let high = i64::try_from(high).map_err(|_| NumericError::Overflow)?;
    checked_raw_interval(low, high, domain)
}

const fn binomial(n: usize, k: usize) -> u64 {
    if k > n {
        return 0;
    }
    let k = if k < n - k { k } else { n - k };
    let mut result = 1_u64;
    let mut i = 0_usize;
    while i < k {
        result = result * (n - i) as u64 / (i + 1) as u64;
        i += 1;
    }
    result
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Rational {
    numerator: i128,
    denominator: i128,
}

impl Rational {
    const fn zero() -> Self {
        Self {
            numerator: 0,
            denominator: 1,
        }
    }

    fn integer(value: impl Into<i128>) -> Self {
        Self {
            numerator: value.into(),
            denominator: 1,
        }
    }

    fn new(numerator: i128, denominator: i128) -> Result<Self, NumericError> {
        if denominator == 0 {
            return Err(NumericError::DivisionByZero);
        }
        let (numerator, denominator) = if denominator < 0 {
            (
                numerator.checked_neg().ok_or(NumericError::Overflow)?,
                denominator.checked_neg().ok_or(NumericError::Overflow)?,
            )
        } else {
            (numerator, denominator)
        };
        let divisor = gcd(numerator.unsigned_abs(), denominator.unsigned_abs()) as i128;
        Ok(Self {
            numerator: numerator / divisor,
            denominator: denominator / divisor,
        })
    }

    fn checked_add(self, other: Self) -> Result<Self, NumericError> {
        Self::new(
            self.numerator
                .checked_mul(other.denominator)
                .and_then(|left| {
                    other
                        .numerator
                        .checked_mul(self.denominator)
                        .and_then(|right| left.checked_add(right))
                })
                .ok_or(NumericError::Overflow)?,
            self.denominator
                .checked_mul(other.denominator)
                .ok_or(NumericError::Overflow)?,
        )
    }

    fn checked_mul(self, other: Self) -> Result<Self, NumericError> {
        Self::new(
            self.numerator
                .checked_mul(other.numerator)
                .ok_or(NumericError::Overflow)?,
            self.denominator
                .checked_mul(other.denominator)
                .ok_or(NumericError::Overflow)?,
        )
    }

    fn checked_mul_integer(self, value: i128) -> Result<Self, NumericError> {
        Self::new(
            self.numerator
                .checked_mul(value)
                .ok_or(NumericError::Overflow)?,
            self.denominator,
        )
    }

    fn in_unit(self) -> bool {
        self.numerator >= 0 && self.numerator <= self.denominator
    }

    fn floor(self) -> Result<i128, NumericError> {
        Ok(self.numerator.div_euclid(self.denominator))
    }

    fn ceil(self) -> Result<i128, NumericError> {
        let negated = self.numerator.checked_neg().ok_or(NumericError::Overflow)?;
        negated
            .div_euclid(self.denominator)
            .checked_neg()
            .ok_or(NumericError::Overflow)
    }

    fn checked_cmp(self, other: Self) -> Result<std::cmp::Ordering, NumericError> {
        let left = self
            .numerator
            .checked_mul(other.denominator)
            .ok_or(NumericError::Overflow)?;
        let right = other
            .numerator
            .checked_mul(self.denominator)
            .ok_or(NumericError::Overflow)?;
        Ok(left.cmp(&right))
    }
}

const fn gcd(mut a: u128, mut b: u128) -> u128 {
    while b != 0 {
        let remainder = a % b;
        a = b;
        b = remainder;
    }
    if a == 0 { 1 } else { a }
}

#[cfg(test)]
mod tests {
    use super::*;

    const D: FixedDomain = FixedDomain::full(0);

    #[test]
    fn horner_and_derivative_are_checked() {
        let polynomial =
            Polynomial::new(&[Iv32::point(1), Iv32::point(2), Iv32::point(3)]).unwrap();
        assert_eq!(polynomial.evaluate(Iv32::point(2), D), Ok(Iv32::point(17)));
        assert_eq!(
            polynomial.derivative(D).unwrap().coefficients[..2],
            [Iv32::point(2), Iv32::point(6)]
        );
    }

    #[test]
    fn bernstein_subdivision_preserves_endpoints_and_sign() {
        let polynomial =
            Polynomial::new(&[Iv32::point(1), Iv32::point(2), Iv32::point(1)]).unwrap();
        let bernstein = polynomial.to_bernstein(D).unwrap();
        assert_eq!(bernstein.sign(), CoefficientSign::StrictPositive);
        let (left, right) = bernstein.subdivide_half(D).unwrap();
        assert_eq!(left.coefficients[0], bernstein.coefficients[0]);
        assert_eq!(
            right.coefficients[usize::from(right.degree)],
            bernstein.coefficients[usize::from(bernstein.degree)]
        );
    }

    #[test]
    fn sign_variation_is_fail_closed_on_uncertain_coefficient() {
        let mut coefficients = [Iv32::point(1); MAX_COEFFICIENTS];
        coefficients[1] = Iv32::new(-1, 1).unwrap();
        let polynomial = Bernstein {
            coefficients,
            degree: 2,
        };
        assert_eq!(polynomial.sign_variations(), SignVariation::Inconclusive);
    }

    #[test]
    fn quadratic_range_finds_edge_extremum_missed_by_corners_and_center() {
        // (4u - 1)^2 has its minimum at u=1/4.
        let range = exact_quadratic_range(
            Quadratic2 {
                c: 1,
                bu: -8,
                bv: 0,
                buu: 16,
                buv: 0,
                bvv: 0,
            },
            D,
        )
        .unwrap();
        assert_eq!(range, Iv32 { lo: 0, hi: 9 });
    }

    #[test]
    fn quadratic_range_rejects_a_coupled_schedule_outside_the_shared_i64_envelope() {
        assert_eq!(
            exact_quadratic_range(
                Quadratic2 {
                    c: 0,
                    bu: 0,
                    bv: 1_500_000_000,
                    buu: 0,
                    buv: 1_500_000_000,
                    bvv: -2_000_000_000,
                },
                FixedDomain::full(0),
            ),
            Err(NumericError::Overflow)
        );
    }

    #[test]
    fn quadratic_range_contains_dense_deterministic_grid() {
        let cases = [
            Quadratic2 {
                c: 7,
                bu: -11,
                bv: 5,
                buu: 9,
                buv: -13,
                bvv: 6,
            },
            Quadratic2 {
                c: -3,
                bu: 4,
                bv: -8,
                buu: 0,
                buv: 7,
                bvv: 0,
            },
        ];
        for quadratic in cases {
            let range = exact_quadratic_range(quadratic, D).unwrap();
            for u in 0..=64_i64 {
                for v in 0..=64_i64 {
                    let denominator = 64_i128 * 64;
                    let numerator = i128::from(quadratic.c) * denominator
                        + i128::from(quadratic.bu) * i128::from(u) * 64
                        + i128::from(quadratic.bv) * i128::from(v) * 64
                        + i128::from(quadratic.buu) * i128::from(u * u)
                        + i128::from(quadratic.buv) * i128::from(u * v)
                        + i128::from(quadratic.bvv) * i128::from(v * v);
                    assert!(i128::from(range.lo) * denominator <= numerator);
                    assert!(numerator <= i128::from(range.hi) * denominator);
                }
            }
        }
    }

    #[test]
    fn sparse_shape_is_bounded() {
        let terms = vec![
            SparseTerm {
                coefficient: Iv32::point(1),
                powers: [0; 4],
            };
            MAX_SPARSE_TERMS + 1
        ];
        assert_eq!(
            evaluate_sparse(&terms, [Iv32::point(0); 4], D),
            Err(NumericError::UnsupportedShape)
        );
    }

    #[test]
    fn checked_composition_contains_direct_evaluation() {
        let outer = Polynomial::new(&[Iv32::point(1), Iv32::point(2), Iv32::point(1)]).unwrap();
        let candidate = Polynomial::new(&[Iv32::point(1), Iv32::point(1)]).unwrap();
        let composed = compose_univariate(outer, candidate, D).unwrap();
        assert_eq!(
            composed.coefficients[..=2],
            [Iv32::point(4), Iv32::point(4), Iv32::point(1)]
        );
        assert_eq!(
            composed.evaluate(Iv32::point(2), D),
            outer.evaluate(candidate.evaluate(Iv32::point(2), D).unwrap(), D)
        );
    }
}
