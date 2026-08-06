//! Interval normal reconstruction from inverse-depth sheets.

use super::iv32::{FixedDomain, Iv32, NumericError};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Iv3 {
    pub x: Iv32,
    pub y: Iv32,
    pub z: Iv32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NormalCone {
    /// Unnormalized direction enclosure. Normalization may only consume this
    /// record when `norm_squared.lo > 0`.
    pub direction: Iv3,
    pub norm_squared: Iv32,
}

pub fn reconstruct(
    u: Iv32,
    v: Iv32,
    q: Iv32,
    q_u: Iv32,
    q_v: Iv32,
    domain: FixedDomain,
) -> Result<Iv3, NumericError> {
    let z = q
        .subtract(u.multiply(q_u, domain)?, domain)?
        .subtract(v.multiply(q_v, domain)?, domain)?;
    Ok(Iv3 { x: q_u, y: q_v, z })
}

pub fn norm_squared(vector: Iv3, domain: FixedDomain) -> Result<Iv32, NumericError> {
    vector
        .x
        .square(domain)?
        .add(vector.y.square(domain)?, domain)?
        .add(vector.z.square(domain)?, domain)
}

pub fn normalization_is_safe(vector: Iv3, domain: FixedDomain) -> Result<bool, NumericError> {
    Ok(norm_squared(vector, domain)?.strict_positive())
}

pub fn normal_cone(
    u: Iv32,
    v: Iv32,
    q: Iv32,
    q_u: Iv32,
    q_v: Iv32,
    domain: FixedDomain,
) -> Result<Option<NormalCone>, NumericError> {
    let direction = reconstruct(u, v, q, q_u, q_v, domain)?;
    let norm_squared = norm_squared(direction, domain)?;
    Ok(norm_squared.strict_positive().then_some(NormalCone {
        direction,
        norm_squared,
    }))
}

pub fn cone_dot(
    normal: NormalCone,
    other_direction: Iv3,
    domain: FixedDomain,
) -> Result<Iv32, NumericError> {
    if !normal.norm_squared.strict_positive() {
        return Err(NumericError::InvalidDomain);
    }
    normalized_dot_q8(normal.direction, other_direction, domain)
}

/// Scale-invariant cosine enclosure in signed Q8 raw units.
pub fn normalized_dot_q8(a: Iv3, b: Iv3, domain: FixedDomain) -> Result<Iv32, NumericError> {
    validate_vector(a, domain)?;
    validate_vector(b, domain)?;
    let numerator = raw_dot(a, b)?;
    let norm_a = raw_norm_squared(a)?;
    let norm_b = raw_norm_squared(b)?;
    if norm_a.0 <= 0 || norm_b.0 <= 0 {
        return Err(NumericError::InvalidDomain);
    }
    let denominator_lo = floor_sqrt_i64(norm_a.0)
        .checked_mul(floor_sqrt_i64(norm_b.0))
        .ok_or(NumericError::Overflow)?;
    let denominator_hi = ceil_sqrt_i64(norm_a.1)
        .checked_mul(ceil_sqrt_i64(norm_b.1))
        .ok_or(NumericError::Overflow)?;
    if denominator_lo == 0 {
        return Err(NumericError::InvalidDomain);
    }
    let products = [
        (
            numerator.0.checked_mul(256).ok_or(NumericError::Overflow)?,
            denominator_lo,
        ),
        (
            numerator.0.checked_mul(256).ok_or(NumericError::Overflow)?,
            denominator_hi,
        ),
        (
            numerator.1.checked_mul(256).ok_or(NumericError::Overflow)?,
            denominator_lo,
        ),
        (
            numerator.1.checked_mul(256).ok_or(NumericError::Overflow)?,
            denominator_hi,
        ),
    ];
    let mut lo = i64::MAX;
    let mut hi = i64::MIN;
    for (value, denominator) in products {
        lo = lo.min(value.div_euclid(denominator));
        hi = hi.max(ceil_div_i64(value, denominator)?);
    }
    let lo = lo.max(-256);
    let hi = hi.min(256);
    Iv32::new(
        i32::try_from(lo).map_err(|_| NumericError::Overflow)?,
        i32::try_from(hi).map_err(|_| NumericError::Overflow)?,
    )
}

fn validate_vector(vector: Iv3, domain: FixedDomain) -> Result<(), NumericError> {
    domain.validate()?;
    for component in [vector.x, vector.y, vector.z] {
        if component.lo > component.hi {
            return Err(NumericError::InvalidInterval);
        }
        if component.lo < domain.min_raw || component.hi > domain.max_raw {
            return Err(NumericError::DomainMismatch);
        }
    }
    Ok(())
}

fn raw_dot(a: Iv3, b: Iv3) -> Result<(i64, i64), NumericError> {
    checked_raw_add(
        checked_raw_add(raw_product(a.x, b.x), raw_product(a.y, b.y))?,
        raw_product(a.z, b.z),
    )
}

fn raw_norm_squared(vector: Iv3) -> Result<(i64, i64), NumericError> {
    checked_raw_add(
        checked_raw_add(raw_square(vector.x), raw_square(vector.y))?,
        raw_square(vector.z),
    )
}

fn raw_product(a: Iv32, b: Iv32) -> (i64, i64) {
    let products = [
        i64::from(a.lo) * i64::from(b.lo),
        i64::from(a.lo) * i64::from(b.hi),
        i64::from(a.hi) * i64::from(b.lo),
        i64::from(a.hi) * i64::from(b.hi),
    ];
    (
        *products.iter().min().expect("four products"),
        *products.iter().max().expect("four products"),
    )
}

fn raw_square(value: Iv32) -> (i64, i64) {
    let low_magnitude = if value.contains_zero() {
        0
    } else {
        i64::from(value.lo).abs().min(i64::from(value.hi).abs())
    };
    let high_magnitude = i64::from(value.lo).abs().max(i64::from(value.hi).abs());
    (
        low_magnitude * low_magnitude,
        high_magnitude * high_magnitude,
    )
}

fn checked_raw_add(a: (i64, i64), b: (i64, i64)) -> Result<(i64, i64), NumericError> {
    Ok((
        a.0.checked_add(b.0).ok_or(NumericError::Overflow)?,
        a.1.checked_add(b.1).ok_or(NumericError::Overflow)?,
    ))
}

fn ceil_div_i64(value: i64, denominator: i64) -> Result<i64, NumericError> {
    value
        .checked_neg()
        .map(|negated| -negated.div_euclid(denominator))
        .ok_or(NumericError::Overflow)
}

fn floor_sqrt_i64(value: i64) -> i64 {
    debug_assert!(value >= 0);
    if value < 2 {
        return value;
    }
    let mut lo = 1_i64;
    let mut hi = value.min(3_037_000_499);
    while lo <= hi {
        let midpoint = lo + (hi - lo) / 2;
        if midpoint <= value / midpoint {
            lo = midpoint + 1;
        } else {
            hi = midpoint - 1;
        }
    }
    hi
}

fn ceil_sqrt_i64(value: i64) -> i64 {
    let floor = floor_sqrt_i64(value);
    if floor * floor == value {
        floor
    } else {
        floor + 1
    }
}

pub fn dot(a: Iv3, b: Iv3, domain: FixedDomain) -> Result<Iv32, NumericError> {
    a.x.multiply(b.x, domain)?
        .add(a.y.multiply(b.y, domain)?, domain)?
        .add(a.z.multiply(b.z, domain)?, domain)
}

#[cfg(test)]
mod tests {
    use super::*;

    const D: FixedDomain = FixedDomain::full(-8);

    #[test]
    fn plane_normal_is_exact_and_constant() {
        let normal = reconstruct(
            Iv32::point(0),
            Iv32::point(0),
            Iv32::point(256),
            Iv32::point(0),
            Iv32::point(0),
            D,
        )
        .unwrap();
        assert_eq!(
            normal,
            Iv3 {
                x: Iv32::point(0),
                y: Iv32::point(0),
                z: Iv32::point(256)
            }
        );
        assert_eq!(normalization_is_safe(normal, D), Ok(true));
    }

    #[test]
    fn zero_containing_norm_is_certificate_failure() {
        let uncertain = Iv3 {
            x: Iv32::new(-1, 1).unwrap(),
            y: Iv32::new(-1, 1).unwrap(),
            z: Iv32::new(-1, 1).unwrap(),
        };
        assert_eq!(normalization_is_safe(uncertain, D), Ok(false));
    }

    #[test]
    fn curved_inverse_depth_gradient_is_inside_the_cone() {
        let cone = normal_cone(
            Iv32::new(31, 33).unwrap(),
            Iv32::new(-17, -15).unwrap(),
            Iv32::new(250, 258).unwrap(),
            Iv32::new(10, 12).unwrap(),
            Iv32::new(-7, -5).unwrap(),
            D,
        )
        .unwrap()
        .expect("regular curved sheet");
        assert!(cone.direction.x.lo <= 11 && 11 <= cone.direction.x.hi);
        assert!(cone.direction.y.lo <= -6 && -6 <= cone.direction.y.hi);
        let exact_z = 254 - (32 * 11) / 256 - (-16 * -6) / 256;
        assert!(cone.direction.z.lo <= exact_z && exact_z <= cone.direction.z.hi);
    }

    #[test]
    fn normalized_dot_is_scale_invariant() {
        let direction = Iv3 {
            x: Iv32::point(3),
            y: Iv32::point(4),
            z: Iv32::point(0),
        };
        let scaled = Iv3 {
            x: Iv32::point(6),
            y: Iv32::point(8),
            z: Iv32::point(0),
        };
        assert_eq!(
            normalized_dot_q8(direction, direction, D),
            Ok(Iv32::point(256))
        );
        assert_eq!(
            normalized_dot_q8(direction, scaled, D),
            Ok(Iv32::point(256))
        );
        let zero = Iv3 {
            x: Iv32::point(0),
            y: Iv32::point(0),
            z: Iv32::point(0),
        };
        assert_eq!(
            normalized_dot_q8(direction, zero, D),
            Err(NumericError::InvalidDomain)
        );
    }
}
