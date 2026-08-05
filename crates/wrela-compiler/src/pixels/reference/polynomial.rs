//! Allocation-bounded compiler reference for low-degree polynomial checks.

use super::super::reference::interval::{F64Interval, next_down, next_up};

const TRIM_EPSILON: f64 = 1.0e-13;

fn trim(mut polynomial: Vec<f64>) -> Vec<f64> {
    while polynomial.len() > 1
        && polynomial
            .last()
            .is_some_and(|coefficient| coefficient.abs() <= TRIM_EPSILON)
    {
        polynomial.pop();
    }
    polynomial
}

pub fn direct_sum(coefficients_ascending: &[f64], x: f64) -> f64 {
    coefficients_ascending
        .iter()
        .enumerate()
        .map(|(power, coefficient)| coefficient * x.powi(power as i32))
        .sum()
}

pub fn horner(coefficients_ascending: &[f64], x: f64) -> f64 {
    coefficients_ascending
        .iter()
        .rev()
        .fold(0.0, |value, coefficient| value * x + coefficient)
}

fn derivative(polynomial: &[f64]) -> Vec<f64> {
    if polynomial.len() <= 1 {
        return vec![0.0];
    }
    polynomial
        .iter()
        .enumerate()
        .skip(1)
        .map(|(power, coefficient)| *coefficient * power as f64)
        .collect()
}

fn remainder(dividend: &[f64], divisor: &[f64]) -> Result<Vec<f64>, String> {
    let divisor = trim(divisor.to_vec());
    let leading = *divisor
        .last()
        .ok_or_else(|| "pixels::reference::polynomial: empty divisor".to_string())?;
    if leading.abs() <= TRIM_EPSILON {
        return Err("pixels::reference::polynomial: zero leading divisor".to_string());
    }
    let mut remainder = trim(dividend.to_vec());
    while remainder.len() >= divisor.len()
        && remainder
            .last()
            .is_some_and(|value| value.abs() > TRIM_EPSILON)
    {
        let offset = remainder.len() - divisor.len();
        let scale = remainder[remainder.len() - 1] / leading;
        for (index, coefficient) in divisor.iter().enumerate() {
            remainder[offset + index] -= scale * coefficient;
        }
        remainder = trim(remainder);
    }
    Ok(remainder)
}

fn monic(polynomial: Vec<f64>) -> Result<Vec<f64>, String> {
    let polynomial = trim(polynomial);
    let leading = *polynomial
        .last()
        .ok_or_else(|| "pixels::reference::polynomial: empty polynomial".to_string())?;
    if leading.abs() <= TRIM_EPSILON {
        return Err("pixels::reference::polynomial: zero polynomial".to_string());
    }
    Ok(polynomial
        .into_iter()
        .map(|coefficient| coefficient / leading)
        .collect())
}

fn square_free(polynomial: &[f64]) -> Result<Vec<f64>, String> {
    let mut a = monic(polynomial.to_vec())?;
    let mut b = monic(derivative(&a))?;
    while b.len() > 1 || b[0].abs() > TRIM_EPSILON {
        let next = remainder(&a, &b)?;
        if next.len() == 1 && next[0].abs() <= TRIM_EPSILON {
            break;
        }
        a = b;
        b = monic(next)?;
    }
    let gcd = if b.len() == 1 && b[0].abs() <= TRIM_EPSILON {
        vec![1.0]
    } else {
        b
    };
    let quotient_degree = polynomial
        .len()
        .checked_sub(gcd.len())
        .ok_or_else(|| "pixels::reference::polynomial: gcd degree overflow".to_string())?;
    let mut quotient = vec![0.0; quotient_degree + 1];
    let mut work = polynomial.to_vec();
    let gcd_leading = *gcd
        .last()
        .ok_or_else(|| "pixels::reference::polynomial: empty gcd".to_string())?;
    for degree in (0..=quotient_degree).rev() {
        let source = degree + gcd.len() - 1;
        let scale = work[source] / gcd_leading;
        quotient[degree] = scale;
        for (index, coefficient) in gcd.iter().enumerate() {
            work[degree + index] -= scale * coefficient;
        }
    }
    Ok(trim(quotient))
}

fn sturm_sequence(polynomial: &[f64]) -> Result<Vec<Vec<f64>>, String> {
    let square_free = square_free(polynomial)?;
    let mut sequence = vec![square_free.clone(), derivative(&square_free)];
    while sequence.last().is_some_and(|value| value.len() > 1) {
        let length = sequence.len();
        let remainder = remainder(&sequence[length - 2], &sequence[length - 1])?;
        if remainder.iter().all(|value| value.abs() <= TRIM_EPSILON) {
            break;
        }
        sequence.push(remainder.into_iter().map(|value| -value).collect());
    }
    Ok(sequence)
}

fn sign_variations(sequence: &[Vec<f64>], x: f64) -> u32 {
    let mut previous = 0_i8;
    let mut variations = 0_u32;
    for polynomial in sequence {
        let value = horner(polynomial, x);
        let sign = if value > TRIM_EPSILON {
            1
        } else if value < -TRIM_EPSILON {
            -1
        } else {
            0
        };
        if sign == 0 {
            continue;
        }
        if previous != 0 && sign != previous {
            variations += 1;
        }
        previous = sign;
    }
    variations
}

fn roots_in(sequence: &[Vec<f64>], lo: f64, hi: f64) -> u32 {
    sign_variations(sequence, next_up(lo)).saturating_sub(sign_variations(sequence, next_down(hi)))
}

/// Floating-point Sturm bug-finder retained for differential tests only.
/// Production root-isolation metadata names `isolate_all_roots_bernstein`,
/// whose acceptance decisions use outward intervals and fail closed.
pub fn isolate_distinct_roots(
    coefficients_ascending: &[f64],
    domain: F64Interval,
    max_depth: u8,
) -> Result<Vec<F64Interval>, String> {
    if coefficients_ascending.len() < 2 || coefficients_ascending.len() > 9 {
        return Err(format!(
            "P004: field operation `projective root isolation` is not available in `AaaByteExact`: degree {} is outside 1..=8",
            coefficients_ascending.len().saturating_sub(1)
        ));
    }
    let sequence = sturm_sequence(coefficients_ascending)?;
    let mut pending = vec![(domain, 0_u8)];
    let mut roots = Vec::new();
    while let Some((interval, depth)) = pending.pop() {
        let count = roots_in(&sequence, interval.lo, interval.hi);
        if count == 0 {
            continue;
        }
        if count == 1 {
            roots.push(interval);
            continue;
        }
        if depth >= max_depth {
            return Err(format!(
                "pixels::reference::polynomial: {count} roots remain after depth {max_depth} in {interval:?}"
            ));
        }
        let midpoint = interval.lo + (interval.hi - interval.lo) * 0.5;
        if midpoint <= interval.lo || midpoint >= interval.hi {
            return Err(
                "pixels::reference::polynomial: isolation midpoint made no progress".to_string(),
            );
        }
        pending.push((F64Interval::new(midpoint, interval.hi)?, depth + 1));
        pending.push((F64Interval::new(interval.lo, midpoint)?, depth + 1));
    }
    roots.sort_by(|a, b| a.lo.total_cmp(&b.lo));
    Ok(roots)
}

fn interval_power(value: F64Interval, exponent: usize) -> Result<F64Interval, String> {
    let mut result = F64Interval::point(1.0)?;
    for _ in 0..exponent {
        result = result.mul_outward(value)?;
    }
    Ok(result)
}

fn power_coefficients_on_unit_interval(
    coefficients_ascending: &[F64Interval],
    domain: F64Interval,
) -> Result<Vec<F64Interval>, String> {
    let degree = coefficients_ascending.len().saturating_sub(1);
    let zero = F64Interval::point(0.0)?;
    let lo = F64Interval::point(domain.lo)?;
    let width = F64Interval::new(
        next_down(domain.hi - domain.lo),
        next_up(domain.hi - domain.lo),
    )?;
    let mut transformed = vec![zero; degree + 1];
    for (source_power, coefficient) in coefficients_ascending.iter().copied().enumerate() {
        for (target_power, target) in transformed.iter_mut().enumerate().take(source_power + 1) {
            let choose = F64Interval::point(binomial(source_power, target_power))?;
            let lo_power = interval_power(lo, source_power - target_power)?;
            let width_power = interval_power(width, target_power)?;
            let contribution = coefficient
                .mul_outward(choose)?
                .mul_outward(lo_power)?
                .mul_outward(width_power)?;
            *target = target.add_outward(contribution)?;
        }
    }
    Ok(transformed)
}

fn bernstein_range_on(
    coefficients_ascending: &[F64Interval],
    domain: F64Interval,
) -> Result<Vec<F64Interval>, String> {
    let transformed = power_coefficients_on_unit_interval(coefficients_ascending, domain)?;
    power_to_bernstein(
        &transformed,
        u8::try_from(transformed.len().saturating_sub(1))
            .map_err(|_| "pixels::reference::polynomial: Bernstein degree overflow".to_string())?,
    )
}

fn interval_horner(
    coefficients_ascending: &[F64Interval],
    value: f64,
) -> Result<F64Interval, String> {
    let value = F64Interval::point(value)?;
    coefficients_ascending
        .iter()
        .rev()
        .try_fold(F64Interval::point(0.0)?, |result, coefficient| {
            result.mul_outward(value)?.add_outward(*coefficient)
        })
}

fn strict_sign(value: F64Interval) -> i8 {
    if value.lo > 0.0 {
        1
    } else if value.hi < 0.0 {
        -1
    } else {
        0
    }
}

/// Certified all-root isolation using outward interval arithmetic and the
/// Bernstein convex-hull property.
///
/// A cell is discarded only when every Bernstein coefficient has one strict
/// sign. It is accepted only when the derivative Bernstein coefficients have
/// one strict sign and the outward endpoint evaluations have opposite signs.
/// A depth-exhausted cell whose hull can still contain zero is retained as an
/// ambiguity corridor. Thus tangent/even-multiplicity roots cannot disappear;
/// later event/tube verification must refine or preserve that corridor.
pub fn isolate_all_roots_bernstein(
    coefficients_ascending: &[F64Interval],
    domain: F64Interval,
    max_depth: u8,
    ambiguity_depth: u8,
) -> Result<Vec<F64Interval>, String> {
    if coefficients_ascending.len() < 2 || coefficients_ascending.len() > 9 {
        return Err(format!(
            "P004: field operation `certified projective root isolation` is not available in `AaaByteExact`: degree {} is outside 1..=8",
            coefficients_ascending.len().saturating_sub(1)
        ));
    }
    if ambiguity_depth == 0 || ambiguity_depth > max_depth {
        return Err(format!(
            "pixels::reference::polynomial: ambiguity depth {ambiguity_depth} is outside 1..={max_depth}"
        ));
    }
    let derivative = coefficients_ascending
        .iter()
        .copied()
        .enumerate()
        .skip(1)
        .map(|(power, coefficient)| coefficient.mul_outward(F64Interval::point(power as f64)?))
        .collect::<Result<Vec<_>, String>>()?;
    let mut pending = vec![(domain, 0_u8)];
    let mut roots = Vec::<(F64Interval, bool)>::new();
    while let Some((interval, depth)) = pending.pop() {
        let values = bernstein_range_on(coefficients_ascending, interval)?;
        let all_positive = values.iter().all(|value| value.lo > 0.0);
        let all_negative = values.iter().all(|value| value.hi < 0.0);
        if all_positive || all_negative {
            continue;
        }
        let derivative_values = bernstein_range_on(&derivative, interval)?;
        let monotone = derivative_values.iter().all(|value| value.lo > 0.0)
            || derivative_values.iter().all(|value| value.hi < 0.0);
        if monotone {
            let lo_sign = strict_sign(interval_horner(coefficients_ascending, interval.lo)?);
            let hi_sign = strict_sign(interval_horner(coefficients_ascending, interval.hi)?);
            if lo_sign != 0 && hi_sign != 0 {
                if lo_sign != hi_sign {
                    roots.push((interval, false));
                }
                continue;
            }
        }
        if depth >= ambiguity_depth || depth >= max_depth {
            roots.push((interval, true));
            continue;
        }
        let midpoint = interval.lo + (interval.hi - interval.lo) * 0.5;
        if midpoint <= interval.lo || midpoint >= interval.hi {
            roots.push((interval, true));
            continue;
        }
        pending.push((F64Interval::new(midpoint, interval.hi)?, depth + 1));
        pending.push((F64Interval::new(interval.lo, midpoint)?, depth + 1));
    }
    roots.sort_by(|a, b| a.0.lo.total_cmp(&b.0.lo));
    let mut merged = Vec::<(F64Interval, bool)>::new();
    for (interval, ambiguous) in roots {
        if let Some((previous, previous_ambiguous)) = merged.last_mut()
            && ambiguous
            && *previous_ambiguous
            && interval.lo <= previous.hi
        {
            *previous = previous.hull(interval);
            continue;
        }
        merged.push((interval, ambiguous));
    }
    let degree = coefficients_ascending.len() - 1;
    while merged.len() > degree {
        let merge_at = (0..merged.len() - 1)
            .min_by(|left, right| {
                let left_gap = merged[*left + 1].0.lo - merged[*left].0.hi;
                let right_gap = merged[*right + 1].0.lo - merged[*right].0.hi;
                left_gap.total_cmp(&right_gap)
            })
            .expect("more root corridors than a positive polynomial degree");
        let right = merged.remove(merge_at + 1).0;
        merged[merge_at].0 = merged[merge_at].0.hull(right);
        merged[merge_at].1 = true;
    }
    Ok(merged.into_iter().map(|(interval, _)| interval).collect())
}

fn binomial(n: usize, k: usize) -> f64 {
    if k > n {
        return 0.0;
    }
    let k = k.min(n - k);
    (0..k).fold(1.0, |value, index| {
        value * (n - index) as f64 / (index + 1) as f64
    })
}

/// Converts power coefficients on [0,1] to Bernstein coefficients of the
/// same fixed degree. Endpoint rounding is enclosed explicitly.
pub fn power_to_bernstein(
    coefficients_ascending: &[F64Interval],
    degree: u8,
) -> Result<Vec<F64Interval>, String> {
    let degree = usize::from(degree);
    if degree > 8 || coefficients_ascending.len() > degree + 1 {
        return Err(format!(
            "P004: field operation `Bernstein conversion` is not available in `AaaByteExact`: degree {degree}"
        ));
    }
    let zero = F64Interval::point(0.0)?;
    (0..=degree)
        .map(|i| {
            let mut value = zero;
            for k in 0..=i {
                let coefficient = coefficients_ascending.get(k).copied().unwrap_or(zero);
                let scale = F64Interval::point(binomial(i, k) / binomial(degree, k))?;
                value = value.add_outward(coefficient.mul_outward(scale)?)?;
            }
            Ok(value)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn horner_and_direct_sum_match_permanent_shapes() {
        for coefficients in [
            vec![1.0, -2.0],
            vec![3.0, -1.0, 0.5],
            vec![-2.0, 0.0, 1.0, -0.25, 0.03125],
        ] {
            for x in [-2.0, -0.25, 0.0, 0.75, 3.0] {
                let a = direct_sum(&coefficients, x);
                let b = horner(&coefficients, x);
                assert!((a - b).abs() <= f64::EPSILON * a.abs().max(1.0) * 8.0);
            }
        }
    }

    #[test]
    fn sturm_keeps_all_distinct_positive_roots_including_repeated_roots() {
        // (x - .3)^2 (x - 1.1) (x - 2.7)
        let polynomial = [0.2673, -2.124, 5.34, -4.4, 1.0];
        let roots =
            isolate_distinct_roots(&polynomial, F64Interval::new(0.01, 3.0).unwrap(), 16).unwrap();
        assert_eq!(roots.len(), 3);
        for (root, expected) in roots.iter().zip([0.3, 1.1, 2.7]) {
            assert!(
                root.contains(expected),
                "{root:?} does not contain {expected}"
            );
        }
    }

    #[test]
    fn bernstein_conversion_encloses_fixed_degree_coefficients() {
        let power = [
            F64Interval::point(1.0).unwrap(),
            F64Interval::point(-2.0).unwrap(),
            F64Interval::point(3.0).unwrap(),
        ];
        let bernstein = power_to_bernstein(&power, 2).unwrap();
        for step in 0..=64 {
            let x = f64::from(step) / 64.0;
            let source = 1.0 - 2.0 * x + 3.0 * x * x;
            let basis = [(1.0 - x) * (1.0 - x), 2.0 * x * (1.0 - x), x * x];
            let lo = bernstein
                .iter()
                .zip(basis)
                .map(|(coefficient, basis)| coefficient.lo * basis)
                .sum::<f64>();
            let hi = bernstein
                .iter()
                .zip(basis)
                .map(|(coefficient, basis)| coefficient.hi * basis)
                .sum::<f64>();
            assert!(lo <= source && source <= hi);
        }
    }
}
