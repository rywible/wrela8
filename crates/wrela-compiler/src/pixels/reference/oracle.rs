//! Independent f64/interval all-root visibility oracle.
//!
//! This module consumes source-semantic callbacks only. It deliberately does
//! not import the sweep, projective-program, certificate, or telemetry modules.

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Interval {
    pub lo: f64,
    pub hi: f64,
}

impl Interval {
    pub fn new(lo: f64, hi: f64) -> Option<Self> {
        (lo.is_finite() && hi.is_finite() && lo <= hi).then_some(Self { lo, hi })
    }

    pub fn contains_zero(self) -> bool {
        self.lo <= 0.0 && self.hi >= 0.0
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Vec3 {
    pub x: f64,
    pub y: f64,
    pub z: f64,
}

impl Vec3 {
    fn normalized(self) -> Option<Self> {
        let length = (self.x * self.x + self.y * self.y + self.z * self.z).sqrt();
        if !length.is_finite() || length <= 0.0 {
            return None;
        }
        Some(Self {
            x: self.x / length,
            y: self.y / length,
            z: self.z / length,
        })
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct OracleRoot {
    pub t: Interval,
    pub identity: u32,
    pub orientation: i8,
    pub normal: Vec3,
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct OracleCell {
    lo: f64,
    hi: f64,
    depth: u8,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OracleError {
    InvalidDomain,
    CapacityExceeded,
    NonFinite,
    Unresolved,
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct CoverageCell {
    lo: f64,
    hi: f64,
    depth: u8,
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct CoverageScore {
    pub coverage: Interval,
    pub unresolved_cells: u32,
}

/// Independently bound the covered fraction of a one-dimensional event cell.
///
/// A negative predicate is covered. Cells whose interval still contains zero
/// at the fixed depth contribute `[0,width]` and are reported as unresolved;
/// callers must reject unresolved flagship controls rather than treating them
/// as misses.
pub fn event_coverage(
    range: &dyn Fn(f64, f64) -> Option<Interval>,
    lo: f64,
    hi: f64,
    tolerance: f64,
    max_depth: u8,
    stack: &mut [CoverageCell],
) -> Result<CoverageScore, OracleError> {
    if !lo.is_finite()
        || !hi.is_finite()
        || !tolerance.is_finite()
        || lo >= hi
        || tolerance <= 0.0
        || stack.is_empty()
    {
        return Err(OracleError::InvalidDomain);
    }
    stack[0] = CoverageCell { lo, hi, depth: 0 };
    let mut stack_len = 1_usize;
    let mut covered_lo = 0.0_f64;
    let mut covered_hi = 0.0_f64;
    let mut unresolved_cells = 0_u32;
    while stack_len != 0 {
        stack_len -= 1;
        let cell = stack[stack_len];
        let predicate = range(cell.lo, cell.hi).ok_or(OracleError::NonFinite)?;
        let width = cell.hi - cell.lo;
        // A zero-valued endpoint has measure zero, so a cell whose whole
        // range is non-positive or non-negative has exact area ownership.
        if predicate.hi <= 0.0 {
            covered_lo += width;
            covered_hi += width;
            continue;
        }
        if predicate.lo >= 0.0 {
            continue;
        }
        if width <= tolerance {
            covered_hi += width;
            unresolved_cells = unresolved_cells
                .checked_add(1)
                .ok_or(OracleError::CapacityExceeded)?;
            continue;
        }
        if cell.depth >= max_depth {
            covered_hi += width;
            unresolved_cells = unresolved_cells
                .checked_add(1)
                .ok_or(OracleError::CapacityExceeded)?;
            continue;
        }
        let midpoint = cell.lo + width * 0.5;
        if midpoint <= cell.lo || midpoint >= cell.hi || stack_len + 2 > stack.len() {
            return Err(OracleError::CapacityExceeded);
        }
        stack[stack_len] = CoverageCell {
            lo: midpoint,
            hi: cell.hi,
            depth: cell.depth + 1,
        };
        stack[stack_len + 1] = CoverageCell {
            lo: cell.lo,
            hi: midpoint,
            depth: cell.depth + 1,
        };
        stack_len += 2;
    }
    let scale = 1.0 / (hi - lo);
    let coverage =
        Interval::new(covered_lo * scale, covered_hi * scale).ok_or(OracleError::NonFinite)?;
    Ok(CoverageScore {
        coverage,
        unresolved_cells,
    })
}

pub struct SemanticRay<'a> {
    pub range: &'a dyn Fn(f64, f64) -> Option<Interval>,
    pub value: &'a dyn Fn(f64) -> f64,
    pub derivative_range: &'a dyn Fn(f64, f64) -> Option<Interval>,
    pub gradient: &'a dyn Fn(f64) -> Vec3,
    pub identity: &'a dyn Fn(f64) -> u32,
}

pub fn isolate_all_roots(
    ray: SemanticRay<'_>,
    near_t: f64,
    far_t: f64,
    tolerance: f64,
    max_depth: u8,
    stack: &mut [OracleCell],
    roots: &mut [OracleRoot],
) -> Result<usize, OracleError> {
    if !near_t.is_finite()
        || !far_t.is_finite()
        || !tolerance.is_finite()
        || near_t >= far_t
        || tolerance <= 0.0
        || stack.is_empty()
    {
        return Err(OracleError::InvalidDomain);
    }
    stack[0] = OracleCell {
        lo: near_t,
        hi: far_t,
        depth: 0,
    };
    let mut stack_len = 1_usize;
    let mut root_count = 0_usize;
    while stack_len != 0 {
        stack_len -= 1;
        let cell = stack[stack_len];
        let range = (ray.range)(cell.lo, cell.hi).ok_or(OracleError::NonFinite)?;
        if !range.contains_zero() {
            continue;
        }
        let derivative = (ray.derivative_range)(cell.lo, cell.hi).ok_or(OracleError::NonFinite)?;
        let unique = derivative.lo > 0.0 || derivative.hi < 0.0;
        if cell.hi - cell.lo <= tolerance {
            let midpoint = cell.lo + (cell.hi - cell.lo) * 0.5;
            let value = (ray.value)(midpoint);
            if !value.is_finite() {
                return Err(OracleError::NonFinite);
            }
            let normal = (ray.gradient)(midpoint)
                .normalized()
                .ok_or(OracleError::Unresolved)?;
            let identity = (ray.identity)(midpoint);
            // A nonsmooth/min-max semantic field can have a conservative
            // derivative interval spanning zero even at a simple crossing.
            // Terminal endpoint signs still prove the crossing orientation
            // directly and take precedence over the derivative fallback.
            let lo_value = (ray.value)(cell.lo);
            let hi_value = (ray.value)(cell.hi);
            if !lo_value.is_finite() || !hi_value.is_finite() {
                return Err(OracleError::NonFinite);
            }
            let orientation = if (lo_value > 0.0 && hi_value <= 0.0)
                || (lo_value >= 0.0 && hi_value < 0.0)
            {
                1
            } else if (lo_value < 0.0 && hi_value >= 0.0) || (lo_value <= 0.0 && hi_value > 0.0) {
                -1
            } else if derivative.hi < 0.0 {
                1
            } else if derivative.lo > 0.0 {
                -1
            } else {
                0
            };
            if root_count != 0 && roots[root_count - 1].t.hi >= cell.lo {
                roots[root_count - 1].t.hi = roots[root_count - 1].t.hi.max(cell.hi);
                if roots[root_count - 1].identity != identity {
                    return Err(OracleError::Unresolved);
                }
                let previous_orientation = roots[root_count - 1].orientation;
                roots[root_count - 1].orientation = if previous_orientation == 0 {
                    orientation
                } else if orientation == 0 || previous_orientation == orientation {
                    previous_orientation
                } else if unique {
                    orientation
                } else {
                    0
                };
                continue;
            }
            let Some(slot) = roots.get_mut(root_count) else {
                return Err(OracleError::CapacityExceeded);
            };
            *slot = OracleRoot {
                t: Interval {
                    lo: cell.lo,
                    hi: cell.hi,
                },
                identity,
                orientation,
                normal,
            };
            root_count += 1;
            continue;
        }
        if cell.depth >= max_depth {
            return Err(OracleError::Unresolved);
        }
        let midpoint = cell.lo + (cell.hi - cell.lo) * 0.5;
        if midpoint <= cell.lo || midpoint >= cell.hi || stack_len + 2 > stack.len() {
            return Err(OracleError::CapacityExceeded);
        }
        // Push the farther half first so LIFO processing and output are
        // deterministically front-to-back in increasing q.
        stack[stack_len] = OracleCell {
            lo: midpoint,
            hi: cell.hi,
            depth: cell.depth + 1,
        };
        stack[stack_len + 1] = OracleCell {
            lo: cell.lo,
            hi: midpoint,
            depth: cell.depth + 1,
        };
        stack_len += 2;
    }
    Ok(root_count)
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct VisibilityScore {
    pub hit: bool,
    pub identity: u32,
    pub t: Interval,
    pub normal: Vec3,
    pub unresolved: u32,
}

pub fn first_boundary(roots: &[OracleRoot], camera_inside: bool) -> VisibilityScore {
    let expected_orientation = if camera_inside { -1 } else { 1 };
    let root = roots
        .iter()
        .find(|root| root.orientation == expected_orientation)
        .or_else(|| roots.first());
    root.map_or_else(VisibilityScore::default, |root| VisibilityScore {
        hit: true,
        identity: root.identity,
        t: root.t,
        normal: root.normal,
        unresolved: u32::from(root.orientation == 0),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn independent_oracle_finds_sphere_entry_and_exit() {
        // (q - 2)^2 - 1 = 0, roots 1 and 3.
        let range = |lo: f64, hi: f64| {
            let at_lo = (lo - 2.0).powi(2) - 1.0;
            let at_hi = (hi - 2.0).powi(2) - 1.0;
            let minimum = if lo <= 2.0 && 2.0 <= hi {
                -1.0
            } else {
                at_lo.min(at_hi)
            };
            Interval::new(minimum, at_lo.max(at_hi))
        };
        let value = |t: f64| (t - 2.0).powi(2) - 1.0;
        let derivative = |lo: f64, hi: f64| Interval::new(2.0 * (lo - 2.0), 2.0 * (hi - 2.0));
        let gradient = |t: f64| Vec3 {
            x: 0.0,
            y: 0.0,
            z: t - 2.0,
        };
        let identity = |_| 7;
        let mut stack = [OracleCell::default(); 64];
        let mut roots = [OracleRoot::default(); 4];
        let count = isolate_all_roots(
            SemanticRay {
                range: &range,
                value: &value,
                derivative_range: &derivative,
                gradient: &gradient,
                identity: &identity,
            },
            0.0,
            4.0,
            1.0 / 1024.0,
            14,
            &mut stack,
            &mut roots,
        )
        .unwrap();
        assert_eq!(count, 2);
        assert!(roots[0].t.lo <= 1.0 && roots[0].t.hi >= 1.0);
        assert!(roots[1].t.lo <= 3.0 && roots[1].t.hi >= 3.0);
        assert_eq!(first_boundary(&roots[..count], false).identity, 7);
        assert_eq!(first_boundary(&roots[..count], true).orientation(), -1);
    }

    #[test]
    fn terminal_endpoint_signs_orient_a_crossing_with_mixed_derivative_bounds() {
        let range = |lo: f64, hi: f64| Interval::new(0.5 - hi, 0.5 - lo);
        let value = |t: f64| 0.5 - t;
        let derivative = |_: f64, _: f64| Interval::new(-1.0, 1.0);
        let gradient = |_| Vec3 {
            x: 0.0,
            y: 0.0,
            z: -1.0,
        };
        let identity = |_| 9;
        let mut stack = [OracleCell::default(); 64];
        let mut roots = [OracleRoot::default(); 4];
        let count = isolate_all_roots(
            SemanticRay {
                range: &range,
                value: &value,
                derivative_range: &derivative,
                gradient: &gradient,
                identity: &identity,
            },
            0.0,
            1.0,
            1.0 / 1024.0,
            14,
            &mut stack,
            &mut roots,
        )
        .unwrap();
        let score = first_boundary(&roots[..count], false);
        assert_eq!(count, 1);
        assert!(score.hit);
        assert_eq!(score.identity, 9);
        assert_eq!(score.unresolved, 0);
    }

    #[test]
    fn adjacent_unoriented_terminal_cell_preserves_proved_crossing_orientation() {
        let range = |_: f64, _: f64| Interval::new(-1.0, 1.0);
        let value = |t: f64| {
            if t < 0.5 {
                1.0
            } else if t == 0.5 {
                0.0
            } else {
                -1.0
            }
        };
        let derivative = |_: f64, _: f64| Interval::new(-1.0, 1.0);
        let gradient = |_: f64| Vec3 {
            x: 1.0,
            y: 0.0,
            z: 0.0,
        };
        let identity = |_: f64| 0;
        let mut stack = [OracleCell::default(); 8];
        let mut roots = [OracleRoot::default(); 4];
        let count = isolate_all_roots(
            SemanticRay {
                range: &range,
                value: &value,
                derivative_range: &derivative,
                gradient: &gradient,
                identity: &identity,
            },
            0.0,
            1.0,
            0.5,
            2,
            &mut stack,
            &mut roots,
        )
        .unwrap();
        assert_eq!(count, 1);
        assert_eq!(roots[0].orientation, 1);
    }

    #[test]
    fn unresolved_is_explicit_instead_of_becoming_a_miss() {
        let range = |_: f64, _: f64| Interval::new(-1.0, 1.0);
        let value = |_| 0.0;
        let derivative = |_: f64, _: f64| Interval::new(-1.0, 1.0);
        let gradient = |_| Vec3 {
            x: 1.0,
            y: 0.0,
            z: 0.0,
        };
        let identity = |_| 1;
        assert_eq!(
            isolate_all_roots(
                SemanticRay {
                    range: &range,
                    value: &value,
                    derivative_range: &derivative,
                    gradient: &gradient,
                    identity: &identity,
                },
                0.0,
                1.0,
                0.01,
                1,
                &mut [OracleCell::default(); 4],
                &mut [OracleRoot::default(); 4],
            ),
            Err(OracleError::Unresolved)
        );
    }

    #[test]
    fn independent_event_coverage_encloses_the_linear_edge() {
        let range = |lo: f64, hi: f64| Interval::new(lo - 0.375, hi - 0.375);
        let mut stack = [CoverageCell::default(); 64];
        let score = event_coverage(&range, 0.0, 1.0, 1.0 / 65_536.0, 16, &mut stack).unwrap();
        assert!(score.coverage.lo <= 0.375);
        assert!(score.coverage.hi >= 0.375);
        assert!(score.coverage.hi - score.coverage.lo <= 2.0 / 65_536.0);
        assert_eq!(score.unresolved_cells, 0);
    }

    #[test]
    fn tolerance_terminal_non_dyadic_event_is_reported_unresolved() {
        let range = |lo: f64, hi: f64| Interval::new(lo - 1.0 / 3.0, hi - 1.0 / 3.0);
        let mut stack = [CoverageCell::default(); 64];
        let score = event_coverage(&range, 0.0, 1.0, 1.0 / 1024.0, 16, &mut stack).unwrap();
        assert!(score.coverage.lo < 1.0 / 3.0);
        assert!(score.coverage.hi > 1.0 / 3.0);
        assert!(score.unresolved_cells > 0);
    }

    trait ScoreOrientation {
        fn orientation(self) -> i8;
    }

    impl ScoreOrientation for VisibilityScore {
        fn orientation(self) -> i8 {
            if self.normal.z > 0.0 { -1 } else { 1 }
        }
    }
}
