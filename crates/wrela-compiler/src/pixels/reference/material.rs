//! Deterministic material and centered-moment interval helpers.

use super::iv32::{FixedDomain, Iv32, NumericError};
use super::normal::Iv3;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MaterialBounds {
    pub base_color: Iv3,
    pub roughness: Iv32,
    pub metallic: Iv32,
    pub emission: Iv3,
    pub opacity: Iv32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CenteredMoments {
    pub mean: Iv32,
    pub first: Iv32,
    pub second: Iv32,
}

pub fn affine_moments(
    center: Iv32,
    slope: Iv32,
    centered_first: Iv32,
    centered_second: Iv32,
    domain: FixedDomain,
) -> Result<CenteredMoments, NumericError> {
    Ok(CenteredMoments {
        mean: center.add(slope.multiply(centered_first, domain)?, domain)?,
        first: slope.multiply(centered_second, domain)?,
        second: Iv32::point(0),
    })
}

pub fn quadratic_moments(
    center: Iv32,
    slope: Iv32,
    curvature: Iv32,
    centered_first: Iv32,
    centered_second: Iv32,
    domain: FixedDomain,
) -> Result<CenteredMoments, NumericError> {
    let half = Iv32::from_f32_bits(0.5_f32.to_bits(), 1, domain)?;
    let quadratic = curvature
        .multiply(centered_second, domain)?
        .multiply(half, domain)?;
    Ok(CenteredMoments {
        mean: center
            .add(slope.multiply(centered_first, domain)?, domain)?
            .add(quadratic, domain)?,
        first: slope.multiply(centered_second, domain)?,
        second: curvature,
    })
}

pub fn low_rank_residual_accepts(
    supplied_residual: Iv32,
    arithmetic_radius: i32,
    budget_raw: i32,
    domain: FixedDomain,
) -> Result<bool, NumericError> {
    if arithmetic_radius < 0 || budget_raw < 0 {
        return Err(NumericError::InvalidDomain);
    }
    let residual = supplied_residual.abs(domain)?;
    let total = i64::from(residual.hi) + i64::from(arithmetic_radius);
    Ok(total <= i64::from(budget_raw))
}

pub fn footprint_bound(du: Iv32, dv: Iv32, domain: FixedDomain) -> Result<Iv32, NumericError> {
    du.abs(domain)?.max(dv.abs(domain)?, domain)
}

pub fn evaluate_affine_scalar(
    center: Iv32,
    du: Iv32,
    dv: Iv32,
    offset_u: Iv32,
    offset_v: Iv32,
    domain: FixedDomain,
) -> Result<Iv32, NumericError> {
    center
        .add(du.multiply(offset_u, domain)?, domain)?
        .add(dv.multiply(offset_v, domain)?, domain)
}

pub fn evaluate_affine_vector(
    center: Iv3,
    du: Iv3,
    dv: Iv3,
    offset_u: Iv32,
    offset_v: Iv32,
    domain: FixedDomain,
) -> Result<Iv3, NumericError> {
    Ok(Iv3 {
        x: evaluate_affine_scalar(center.x, du.x, dv.x, offset_u, offset_v, domain)?,
        y: evaluate_affine_scalar(center.y, du.y, dv.y, offset_u, offset_v, domain)?,
        z: evaluate_affine_scalar(center.z, du.z, dv.z, offset_u, offset_v, domain)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn proposal_residual_never_becomes_a_premise() {
        let domain = FixedDomain::full(-8);
        assert_eq!(
            low_rank_residual_accepts(Iv32::new(-2, 2).unwrap(), 1, 3, domain),
            Ok(true)
        );
        assert_eq!(
            low_rank_residual_accepts(Iv32::new(-3, 3).unwrap(), 1, 3, domain),
            Ok(false)
        );
        assert_eq!(
            low_rank_residual_accepts(
                Iv32::new(i32::MIN, 0).unwrap(),
                0,
                i32::MAX,
                FixedDomain::full(0),
            ),
            Err(NumericError::Overflow)
        );
        assert_eq!(
            low_rank_residual_accepts(
                Iv32::new(0, i32::MAX).unwrap(),
                i32::MAX,
                i32::MAX,
                FixedDomain::full(0),
            ),
            Ok(false)
        );
    }

    #[test]
    fn scalar_vector_and_moment_bounds_share_checked_arithmetic() {
        let domain = FixedDomain::full(-8);
        let offset = Iv32::new(-8, 8).unwrap();
        let scalar = evaluate_affine_scalar(
            Iv32::point(128),
            Iv32::point(16),
            Iv32::point(-8),
            offset,
            offset,
            domain,
        )
        .unwrap();
        assert!(scalar.lo <= 128 && scalar.hi >= 128);
        let moments = quadratic_moments(
            Iv32::point(128),
            Iv32::point(16),
            Iv32::point(4),
            Iv32::point(0),
            Iv32::point(8),
            domain,
        )
        .unwrap();
        assert!(moments.mean.lo <= moments.mean.hi);
    }
}
