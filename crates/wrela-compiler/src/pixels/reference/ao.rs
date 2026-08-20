//! Five-tap deterministic normal-distance ambient occlusion.

use super::interval::F64Interval;

pub const DISTANCE_FRACTIONS: [f64; 5] = [1.0 / 16.0, 1.0 / 8.0, 1.0 / 4.0, 1.0 / 2.0, 1.0];
pub const WEIGHTS: [f64; 5] = [0.40, 0.25, 0.16, 0.11, 0.08];

pub fn evaluate(
    radius: f64,
    strength: f64,
    mut distance_lower_bound: impl FnMut(f64) -> Result<F64Interval, String>,
) -> Result<F64Interval, String> {
    if !radius.is_finite()
        || radius <= 0.0
        || !strength.is_finite()
        || !(0.0..=1.0).contains(&strength)
    {
        return Err("P029: invalid sealed AO configuration".to_string());
    }
    let mut occlusion = F64Interval::point(0.0)?;
    for (fraction, weight) in DISTANCE_FRACTIONS.into_iter().zip(WEIGHTS) {
        let distance = radius * fraction;
        let bound = distance_lower_bound(distance)?;
        if !bound.lo.is_finite() || !bound.hi.is_finite() {
            return Err("P029: non-finite AO distance interval".to_string());
        }
        // The distance program returns a complete interval. Since occ is
        // decreasing in distance, reverse endpoints when mapping it.
        let occ_lo = ((distance - bound.hi.max(0.0)) / distance).clamp(0.0, 1.0);
        let occ_hi = ((distance - bound.lo.max(0.0)) / distance).clamp(0.0, 1.0);
        occlusion = occlusion.add_outward(F64Interval::new(occ_lo * weight, occ_hi * weight)?)?;
    }
    F64Interval::new(
        (1.0 - strength * occlusion.hi).clamp(0.0, 1.0),
        (1.0 - strength * occlusion.lo).clamp(0.0, 1.0),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn open_plane_is_exactly_unoccluded() {
        assert_eq!(
            evaluate(2.0, 1.0, |distance| F64Interval::point(distance)),
            Ok(F64Interval::point(1.0).unwrap())
        );
    }

    #[test]
    fn contact_darkens_and_interval_contains_endpoint_references() {
        let ao = evaluate(1.0, 0.75, |distance| F64Interval::new(0.0, distance * 0.5)).unwrap();
        assert!(ao.lo < 1.0 && ao.lo <= ao.hi && ao.hi <= 1.0);
    }
}
