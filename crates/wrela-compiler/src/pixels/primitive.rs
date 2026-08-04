//! Closed fused-feature vocabulary for primitive decomposition.

use super::ids::ScalarId;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FeatureKind {
    Plane,
    Quadric,
    Quartic,
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum AnalyticPredicate {
    Always,
    Sphere {
        center: [ScalarId; 3],
        radius: ScalarId,
    },
    BoxFace {
        axis: u8,
        sign: i8,
        center: [ScalarId; 3],
        half: [ScalarId; 3],
    },
    RoundBoxFace {
        axis: u8,
        sign: i8,
        center: [ScalarId; 3],
        half: [ScalarId; 3],
        radius: ScalarId,
    },
    RoundBoxEdge {
        axis: u8,
        signs: [i8; 2],
        center: [ScalarId; 3],
        half: [ScalarId; 3],
        radius: ScalarId,
    },
    RoundBoxCorner {
        signs: [i8; 3],
        center: [ScalarId; 3],
        half: [ScalarId; 3],
        radius: ScalarId,
    },
    SegmentSide {
        a: [ScalarId; 3],
        b: [ScalarId; 3],
        radius_a: ScalarId,
        radius_b: ScalarId,
    },
    SegmentCap {
        endpoint: u8,
        a: [ScalarId; 3],
        b: [ScalarId; 3],
        radius: ScalarId,
        hemisphere: bool,
    },
    TorusDomain {
        center: [ScalarId; 3],
        axis: [ScalarId; 3],
        major: ScalarId,
        minor: ScalarId,
    },
}

impl AnalyticPredicate {
    /// Number of analytic equality families forming this validity domain's
    /// boundary. Shared inequalities contribute one generator per equality;
    /// the feature equation itself is counted separately.
    pub fn boundary_generator_count(&self) -> u32 {
        match self {
            Self::Always | Self::Sphere { .. } | Self::TorusDomain { .. } => 0,
            Self::BoxFace { .. } | Self::RoundBoxFace { .. } | Self::RoundBoxEdge { .. } => 4,
            Self::RoundBoxCorner { .. } => 3,
            Self::SegmentSide { .. } => 2,
            Self::SegmentCap { .. } => 1,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct PredicateProgram {
    pub constraints: Vec<AnalyticPredicate>,
    pub shared_boundary: bool,
}

impl PredicateProgram {
    pub fn boundary_generator_count(&self) -> Result<u32, String> {
        self.constraints
            .iter()
            .try_fold(0_u32, |count, constraint| {
                count
                    .checked_add(constraint.boundary_generator_count())
                    .ok_or_else(|| {
                        "pixels::primitive: validity boundary count overflow".to_string()
                    })
            })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OrientationProgram {
    Outward,
    Inward,
    DeformedOutward,
    DeformedInward,
}

/// Production reference interpretation of the analytic feature equation and
/// its validity domain. Projective lowering consumes the same coefficients;
/// keeping this executable interpretation outside `#[cfg(test)]` gives the
/// structural verifier and differential tests one authoritative predicate.
pub(crate) fn contains_boundary_point(
    predicate: &AnalyticPredicate,
    point: [f64; 3],
    scalar: &impl Fn(ScalarId) -> Result<f64, String>,
    tolerance: f64,
) -> Result<bool, String> {
    let vector = |ids: &[ScalarId; 3]| -> Result<[f64; 3], String> {
        Ok([scalar(ids[0])?, scalar(ids[1])?, scalar(ids[2])?])
    };
    let sub = |a: [f64; 3], b: [f64; 3]| [a[0] - b[0], a[1] - b[1], a[2] - b[2]];
    let dot = |a: [f64; 3], b: [f64; 3]| a[0] * b[0] + a[1] * b[1] + a[2] * b[2];
    let norm = |value: [f64; 3]| dot(value, value).sqrt();
    let close = |value: f64| value.abs() <= tolerance;
    let signed_region = |value: f64, sign: i8| {
        if sign < 0 {
            value <= tolerance
        } else {
            value >= -tolerance
        }
    };

    Ok(match predicate {
        AnalyticPredicate::Always => true,
        AnalyticPredicate::Sphere { center, radius } => {
            close(norm(sub(point, vector(center)?)) - scalar(*radius)?)
        }
        AnalyticPredicate::BoxFace {
            axis,
            sign,
            center,
            half,
        } => {
            let center = vector(center)?;
            let half = vector(half)?;
            let local = sub(point, center);
            let axis = usize::from(*axis);
            close(local[axis] - f64::from(*sign) * half[axis])
                && (0..3)
                    .filter(|component| *component != axis)
                    .all(|component| local[component].abs() <= half[component] + tolerance)
        }
        AnalyticPredicate::RoundBoxFace {
            axis,
            sign,
            center,
            half,
            radius,
        } => {
            let center = vector(center)?;
            let half = vector(half)?;
            let radius = scalar(*radius)?;
            let local = sub(point, center);
            let axis = usize::from(*axis);
            close(local[axis] - f64::from(*sign) * (half[axis] + radius))
                && (0..3)
                    .filter(|component| *component != axis)
                    .all(|component| local[component].abs() <= half[component] + tolerance)
        }
        AnalyticPredicate::RoundBoxEdge {
            axis,
            signs,
            center,
            half,
            radius,
        } => {
            let center = vector(center)?;
            let half = vector(half)?;
            let radius = scalar(*radius)?;
            let local = sub(point, center);
            let axis = usize::from(*axis);
            let radial = (0..3)
                .filter(|component| *component != axis)
                .collect::<Vec<_>>();
            let offsets = [
                local[radial[0]] - f64::from(signs[0]) * half[radial[0]],
                local[radial[1]] - f64::from(signs[1]) * half[radial[1]],
            ];
            local[axis].abs() <= half[axis] + tolerance
                && signed_region(offsets[0], signs[0])
                && signed_region(offsets[1], signs[1])
                && close(offsets[0].hypot(offsets[1]) - radius)
        }
        AnalyticPredicate::RoundBoxCorner {
            signs,
            center,
            half,
            radius,
        } => {
            let center = vector(center)?;
            let half = vector(half)?;
            let radius = scalar(*radius)?;
            let local = sub(point, center);
            let offsets = std::array::from_fn(|component| {
                local[component] - f64::from(signs[component]) * half[component]
            });
            (0..3).all(|component| signed_region(offsets[component], signs[component]))
                && close(norm(offsets) - radius)
        }
        AnalyticPredicate::SegmentSide {
            a,
            b,
            radius_a,
            radius_b,
        } => {
            let a = vector(a)?;
            let b = vector(b)?;
            let axis = sub(b, a);
            let axis_squared = dot(axis, axis);
            if axis_squared <= 0.0 {
                false
            } else {
                let pa = sub(point, a);
                let t = dot(pa, axis) / axis_squared;
                let radial = sub(
                    point,
                    [a[0] + axis[0] * t, a[1] + axis[1] * t, a[2] + axis[2] * t],
                );
                let radius = scalar(*radius_a)? + (scalar(*radius_b)? - scalar(*radius_a)?) * t;
                t >= -tolerance && t <= 1.0 + tolerance && close(norm(radial) - radius)
            }
        }
        AnalyticPredicate::SegmentCap {
            endpoint,
            a,
            b,
            radius,
            hemisphere,
        } => {
            let a = vector(a)?;
            let b = vector(b)?;
            let axis = sub(b, a);
            let axis_squared = dot(axis, axis);
            if axis_squared <= 0.0 {
                false
            } else {
                let t = dot(sub(point, a), axis) / axis_squared;
                let endpoint_point = if *endpoint == 0 { a } else { b };
                if *hemisphere {
                    ((*endpoint == 0 && t <= tolerance) || (*endpoint == 1 && t >= 1.0 - tolerance))
                        && close(norm(sub(point, endpoint_point)) - scalar(*radius)?)
                } else {
                    let radial = sub(
                        point,
                        [a[0] + axis[0] * t, a[1] + axis[1] * t, a[2] + axis[2] * t],
                    );
                    close(t - f64::from(*endpoint)) && norm(radial) <= scalar(*radius)? + tolerance
                }
            }
        }
        AnalyticPredicate::TorusDomain {
            center,
            axis,
            major,
            minor,
        } => {
            let q = sub(point, vector(center)?);
            let axis = vector(axis)?;
            let axis_length = norm(axis);
            if axis_length <= 0.0 {
                false
            } else {
                let axial = dot(q, axis) / axis_length;
                let q2 = dot(q, q);
                let major = scalar(*major)?;
                let minor = scalar(*minor)?;
                let implicit = (q2 + major * major - minor * minor).powi(2)
                    - 4.0 * major * major * (q2 - axial * axial);
                implicit.abs() <= tolerance * (1.0 + q2 + major * major + minor * minor).powi(2)
            }
        }
    })
}
