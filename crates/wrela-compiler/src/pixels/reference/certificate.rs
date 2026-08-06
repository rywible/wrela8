//! Ordered monotone-tube and scalar Krawczyk certificate predicates.

use super::iv32::{FixedDomain, Iv32, NumericError};
use super::poly::{Bernstein, CoefficientSign};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RootCertMethod {
    BernsteinFaces,
    MonotoneTube,
    Krawczyk,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CompositionShape {
    Plane,
    Sphere,
    Torus,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RootCertificate {
    pub correction: Iv32,
    pub derivative_margin: Iv32,
    pub face_margin: Iv32,
    pub contraction_margin: Iv32,
    pub method: RootCertMethod,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RunCertificateInput {
    pub composition_shape: Option<CompositionShape>,
    pub lower_coefficients: Bernstein,
    pub upper_coefficients: Bernstein,
    pub lower_face: Iv32,
    pub upper_face: Iv32,
    pub derivative: Iv32,
    pub reciprocal_derivative: Iv32,
    pub residual: Iv32,
    pub correction: Iv32,
    pub domain: FixedDomain,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CertifiedRun {
    pub certificate: RootCertificate,
    pub composition_shape: Option<CompositionShape>,
}

/// Apply the sealed certificate policy in preference order. An inconclusive
/// coefficient hull is an ordinary fallthrough to the interval tube; it does
/// not alter the derivative or face tolerances supplied by the caller.
pub fn certify_run(input: RunCertificateInput) -> Result<Option<CertifiedRun>, NumericError> {
    if input.composition_shape.is_some()
        && let Some(certificate) = bernstein_faces(
            input.lower_coefficients,
            input.upper_coefficients,
            input.derivative,
        )
    {
        return Ok(Some(CertifiedRun {
            certificate,
            composition_shape: input.composition_shape,
        }));
    }
    if let Some(certificate) = monotone_tube(input.lower_face, input.upper_face, input.derivative) {
        return Ok(Some(CertifiedRun {
            certificate,
            composition_shape: input.composition_shape,
        }));
    }
    Ok(krawczyk(
        input.reciprocal_derivative,
        input.residual,
        input.derivative,
        input.correction,
        input.domain,
    )?
    .map(|certificate| CertifiedRun {
        certificate,
        composition_shape: input.composition_shape,
    }))
}

pub fn bernstein_faces(
    lower: Bernstein,
    upper: Bernstein,
    derivative: Iv32,
) -> Option<RootCertificate> {
    let lower_sign = lower.sign();
    let upper_sign = upper.sign();
    if !opposite_signs(lower_sign, upper_sign)
        || !(derivative.strict_positive() || derivative.strict_negative())
    {
        return None;
    }
    Some(RootCertificate {
        correction: Iv32::point(0),
        derivative_margin: strict_margin(derivative),
        face_margin: coefficient_margin(lower, upper),
        contraction_margin: Iv32::point(0),
        method: RootCertMethod::BernsteinFaces,
    })
}

pub fn bernstein_faces_subdivided(
    lower: Bernstein,
    upper: Bernstein,
    derivative: Iv32,
    domain: FixedDomain,
    maximum_depth: u8,
) -> Result<Option<RootCertificate>, NumericError> {
    if maximum_depth > 8 {
        return Err(NumericError::UnsupportedShape);
    }
    if !(derivative.strict_positive() || derivative.strict_negative()) {
        return Ok(None);
    }
    let mut stack = [(lower, upper, 0_u8); 64];
    let mut stack_len = 1_usize;
    let mut minimum_margin = i32::MAX;
    while stack_len != 0 {
        stack_len -= 1;
        let (lower, upper, depth) = stack[stack_len];
        if opposite_signs(lower.sign(), upper.sign()) {
            minimum_margin = minimum_margin.min(coefficient_margin(lower, upper).lo);
            continue;
        }
        if depth >= maximum_depth || stack_len + 2 > stack.len() {
            return Ok(None);
        }
        let (lower_left, lower_right) = lower.subdivide_half(domain)?;
        let (upper_left, upper_right) = upper.subdivide_half(domain)?;
        stack[stack_len] = (lower_right, upper_right, depth + 1);
        stack[stack_len + 1] = (lower_left, upper_left, depth + 1);
        stack_len += 2;
    }
    Ok(Some(RootCertificate {
        correction: Iv32::point(0),
        derivative_margin: strict_margin(derivative),
        face_margin: Iv32::point(minimum_margin),
        contraction_margin: Iv32::point(0),
        method: RootCertMethod::BernsteinFaces,
    }))
}

pub fn monotone_tube(
    lower_face: Iv32,
    upper_face: Iv32,
    derivative: Iv32,
) -> Option<RootCertificate> {
    if !((lower_face.strict_negative() && upper_face.strict_positive())
        || (lower_face.strict_positive() && upper_face.strict_negative()))
        || !(derivative.strict_positive() || derivative.strict_negative())
    {
        return None;
    }
    Some(RootCertificate {
        correction: Iv32::point(0),
        derivative_margin: strict_margin(derivative),
        face_margin: Iv32::point(
            lower_face
                .lo
                .unsigned_abs()
                .min(lower_face.hi.unsigned_abs())
                .min(upper_face.lo.unsigned_abs())
                .min(upper_face.hi.unsigned_abs()) as i32,
        ),
        contraction_margin: Iv32::point(0),
        method: RootCertMethod::MonotoneTube,
    })
}

pub fn krawczyk(
    reciprocal_derivative: Iv32,
    residual: Iv32,
    derivative: Iv32,
    correction: Iv32,
    domain: FixedDomain,
) -> Result<Option<RootCertificate>, NumericError> {
    if correction.lo >= correction.hi || derivative.contains_zero() {
        return Ok(None);
    }
    let one = Iv32::from_f32_bits(1.0_f32.to_bits(), 0, domain)?;
    let minus_a_r = reciprocal_derivative
        .multiply(residual, domain)?
        .negate(domain)?;
    let contraction = one.subtract(reciprocal_derivative.multiply(derivative, domain)?, domain)?;
    let image = minus_a_r.add(contraction.multiply(correction, domain)?, domain)?;
    if image.lo <= correction.lo || image.hi >= correction.hi {
        return Ok(None);
    }
    let margin = (i64::from(image.lo) - i64::from(correction.lo))
        .min(i64::from(correction.hi) - i64::from(image.hi));
    Ok(Some(RootCertificate {
        correction: image,
        derivative_margin: strict_margin(derivative),
        face_margin: Iv32::point(0),
        contraction_margin: Iv32::point(i32::try_from(margin).map_err(|_| NumericError::Overflow)?),
        method: RootCertMethod::Krawczyk,
    }))
}

fn opposite_signs(a: CoefficientSign, b: CoefficientSign) -> bool {
    matches!(
        (a, b),
        (
            CoefficientSign::StrictNegative,
            CoefficientSign::StrictPositive
        ) | (
            CoefficientSign::StrictPositive,
            CoefficientSign::StrictNegative
        )
    )
}

fn strict_margin(interval: Iv32) -> Iv32 {
    Iv32::point(if interval.strict_positive() {
        interval.lo
    } else if interval.strict_negative() {
        interval.hi.saturating_neg()
    } else {
        0
    })
}

fn coefficient_margin(lower: Bernstein, upper: Bernstein) -> Iv32 {
    let lower = &lower.coefficients[..=usize::from(lower.degree)];
    let upper = &upper.coefficients[..=usize::from(upper.degree)];
    let margin = lower
        .iter()
        .chain(upper)
        .map(|coefficient| {
            coefficient
                .lo
                .unsigned_abs()
                .min(coefficient.hi.unsigned_abs())
        })
        .min()
        .unwrap_or(0);
    Iv32::point(i32::try_from(margin).unwrap_or(i32::MAX))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn monotone_plane_accepts_and_grazing_rejects() {
        assert!(
            monotone_tube(
                Iv32::new(-3, -1).unwrap(),
                Iv32::new(1, 4).unwrap(),
                Iv32::new(2, 3).unwrap()
            )
            .is_some()
        );
        assert!(
            monotone_tube(
                Iv32::new(-3, -1).unwrap(),
                Iv32::new(1, 4).unwrap(),
                Iv32::new(-1, 1).unwrap()
            )
            .is_none()
        );
    }

    #[test]
    fn failed_contraction_is_false_not_error() {
        let domain = FixedDomain::full(-4);
        assert_eq!(
            krawczyk(
                Iv32::point(16),
                Iv32::new(-1, 1).unwrap(),
                Iv32::new(15, 17).unwrap(),
                Iv32::new(-2, 2).unwrap(),
                domain
            ),
            Ok(None)
        );
    }

    #[test]
    fn coefficient_faces_can_decide_after_bounded_subdivision() {
        let domain = FixedDomain::full(0);
        let mut lower = [Iv32::point(-4); crate::pixels::reference::poly::MAX_COEFFICIENTS];
        let mut upper = [Iv32::point(4); crate::pixels::reference::poly::MAX_COEFFICIENTS];
        lower[1] = Iv32::point(0);
        upper[1] = Iv32::point(0);
        let lower = Bernstein {
            coefficients: lower,
            degree: 2,
        };
        let upper = Bernstein {
            coefficients: upper,
            degree: 2,
        };
        assert!(
            bernstein_faces_subdivided(lower, upper, Iv32::point(1), domain, 4)
                .unwrap()
                .is_some()
        );
    }

    #[test]
    fn ordered_policy_prefers_coefficients_then_falls_through_without_widening() {
        let make = |values: [Iv32; 3]| {
            let mut coefficients = [Iv32::point(0); super::super::poly::MAX_COEFFICIENTS];
            coefficients[..3].copy_from_slice(&values);
            Bernstein {
                coefficients,
                degree: 2,
            }
        };
        let base = RunCertificateInput {
            composition_shape: Some(CompositionShape::Plane),
            lower_coefficients: make([Iv32::point(-4); 3]),
            upper_coefficients: make([Iv32::point(4); 3]),
            lower_face: Iv32::point(-4),
            upper_face: Iv32::point(4),
            derivative: Iv32::point(2),
            reciprocal_derivative: Iv32::point(1),
            residual: Iv32::point(0),
            correction: Iv32::new(-2, 2).unwrap(),
            domain: FixedDomain::full(0),
        };
        assert!(monotone_tube(base.lower_face, base.upper_face, base.derivative).is_some());
        let tier_zero = certify_run(base).unwrap().unwrap();
        assert_eq!(tier_zero.certificate.method, RootCertMethod::BernsteinFaces);
        assert_eq!(tier_zero.composition_shape, Some(CompositionShape::Plane));

        let mut inconclusive = base;
        inconclusive.lower_coefficients =
            make([Iv32::point(-4), Iv32::new(-1, 1).unwrap(), Iv32::point(-4)]);
        let tier_one = certify_run(inconclusive).unwrap().unwrap();
        assert_eq!(tier_one.certificate.method, RootCertMethod::MonotoneTube);
        assert_eq!(tier_one.certificate.face_margin, Iv32::point(4));

        let mut tier_two = base;
        tier_two.composition_shape = None;
        tier_two.lower_face = Iv32::point(1);
        tier_two.upper_face = Iv32::point(2);
        tier_two.derivative = Iv32::point(1);
        let tier_two = certify_run(tier_two).unwrap().unwrap();
        assert_eq!(tier_two.certificate.method, RootCertMethod::Krawczyk);
    }
}
