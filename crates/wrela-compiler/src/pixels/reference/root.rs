//! Bounded, fail-closed Bernstein root isolation.

use super::iv32::{FixedDomain, Iv32, NumericError};
use super::poly::{Bernstein, CoefficientSign, Polynomial, SignVariation, taylor_with_remainder};

pub const MAX_ROOTS: usize = 16;
const MAX_STACK: usize = 128;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RootReason {
    DegenerateZero,
    DepthExhausted,
    NoProgress,
    Numeric,
    CapacityRoots,
    CapacitySheets,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RootInterval {
    pub interval: Iv32,
    pub unique: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RootOutcome {
    Roots {
        roots: [RootInterval; MAX_ROOTS],
        count: u16,
    },
    CertifiedNone,
    Unresolved(RootReason),
}

/// One exact midpoint refinement of a sign bracket. This is also the scalar
/// correspondence primitive used by the generated guest vector lane.
pub fn bracket_step(
    lo: i32,
    hi: i32,
    lo_negative: bool,
    midpoint_negative: bool,
) -> Result<RootInterval, NumericError> {
    if lo >= hi {
        return Err(NumericError::InvalidInterval);
    }
    let midpoint = lo
        .checked_add(hi.checked_sub(lo).ok_or(NumericError::Overflow)? / 2)
        .ok_or(NumericError::Overflow)?;
    Ok(if lo_negative == midpoint_negative {
        RootInterval {
            interval: Iv32::new(midpoint, hi)?,
            unique: true,
        }
    } else {
        RootInterval {
            interval: Iv32::new(lo, midpoint)?,
            unique: true,
        }
    })
}

#[derive(Clone, Copy)]
pub enum RootEvaluator {
    Bernstein(Bernstein),
    IntervalTaylor {
        polynomial: Polynomial,
        remainder: Iv32,
    },
}

#[derive(Clone, Copy)]
struct Cell {
    q: Iv32,
    evaluator: RootEvaluator,
    depth: u8,
}

pub fn isolate_all_roots(
    polynomial: Bernstein,
    q_domain: Iv32,
    domain: FixedDomain,
    tolerance_raw: u32,
    max_depth: u8,
    root_capacity: u16,
) -> Result<RootOutcome, NumericError> {
    isolate_roots(
        RootEvaluator::Bernstein(polynomial),
        q_domain,
        domain,
        tolerance_raw,
        max_depth,
        root_capacity,
    )
}

pub fn isolate_roots(
    evaluator: RootEvaluator,
    q_domain: Iv32,
    domain: FixedDomain,
    tolerance_raw: u32,
    max_depth: u8,
    root_capacity: u16,
) -> Result<RootOutcome, NumericError> {
    if tolerance_raw == 0
        || root_capacity == 0
        || usize::from(root_capacity) > MAX_ROOTS
        || max_depth == 0
        || max_depth > 15
    {
        return Err(NumericError::InvalidDomain);
    }
    if q_domain.lo >= q_domain.hi {
        return Err(NumericError::InvalidInterval);
    }
    if matches!(evaluator, RootEvaluator::Bernstein(polynomial) if polynomial.degree == 0) {
        return Err(NumericError::UnsupportedShape);
    }
    if evaluator_is_identically_zero(evaluator) {
        return Ok(RootOutcome::Unresolved(RootReason::DegenerateZero));
    }
    q_domain.intersect(q_domain, domain)?;
    let mut stack = [Cell {
        q: q_domain,
        evaluator,
        depth: 0,
    }; MAX_STACK];
    let mut stack_len = 1_usize;
    let mut roots = [RootInterval {
        interval: Iv32::point(0),
        unique: false,
    }; MAX_ROOTS];
    let mut count = 0_usize;

    while stack_len != 0 {
        stack_len -= 1;
        let cell = stack[stack_len];
        let (variation, lo_zero, hi_zero) = match cell.evaluator {
            RootEvaluator::Bernstein(polynomial) => {
                if matches!(
                    polynomial.sign(),
                    CoefficientSign::StrictPositive | CoefficientSign::StrictNegative
                ) {
                    continue;
                }
                let degree = usize::from(polynomial.degree);
                (
                    polynomial.sign_variations(),
                    polynomial.coefficients[0] == Iv32::point(0),
                    polynomial.coefficients[degree] == Iv32::point(0),
                )
            }
            RootEvaluator::IntervalTaylor {
                polynomial,
                remainder,
            } => {
                let range = match taylor_with_remainder(polynomial, cell.q, remainder, domain) {
                    Ok(range) => range,
                    Err(_) => return Ok(RootOutcome::Unresolved(RootReason::Numeric)),
                };
                if range.lo > 0 || range.hi < 0 {
                    continue;
                }
                (SignVariation::Inconclusive, false, false)
            }
        };
        if variation == SignVariation::Exact(0) {
            for endpoint in [
                lo_zero.then_some(cell.q.lo),
                hi_zero
                    .then_some(cell.q.hi)
                    .filter(|_| cell.q.hi != cell.q.lo),
            ]
            .into_iter()
            .flatten()
            {
                if !record_root(
                    &mut roots,
                    &mut count,
                    root_capacity,
                    RootInterval {
                        interval: Iv32::point(endpoint),
                        unique: false,
                    },
                )? {
                    return Ok(RootOutcome::Unresolved(RootReason::CapacityRoots));
                }
            }
            continue;
        }
        let width = cell.q.width_raw()?;
        // A tolerance-width cell is a complete root enclosure only when the
        // Bernstein variation bound proves that it contains at most one
        // root. Collapsing Exact(n > 1) to one corridor would lose roots.
        let endpoint_roots = u8::from(lo_zero) + u8::from(hi_zero && cell.q.hi != cell.q.lo);
        let terminal_is_complete = match variation {
            SignVariation::Exact(root_count) => root_count.saturating_add(endpoint_roots) <= 1,
            // An inconclusive tolerance cell is retained as one non-unique
            // corridor enclosing every root in the cell. It does not claim
            // an exact root count.
            SignVariation::Inconclusive => true,
        };
        if width <= tolerance_raw && terminal_is_complete {
            let unique = variation == SignVariation::Exact(1) && endpoint_roots == 0;
            if !record_root(
                &mut roots,
                &mut count,
                root_capacity,
                RootInterval {
                    interval: cell.q,
                    unique,
                },
            )? {
                return Ok(RootOutcome::Unresolved(RootReason::CapacityRoots));
            }
            continue;
        }
        if cell.depth >= max_depth {
            return Ok(RootOutcome::Unresolved(RootReason::DepthExhausted));
        }
        let midpoint_i64 =
            i64::from(cell.q.lo) + (i64::from(cell.q.hi) - i64::from(cell.q.lo)).div_euclid(2);
        let midpoint = i32::try_from(midpoint_i64).map_err(|_| NumericError::Overflow)?;
        if midpoint <= cell.q.lo || midpoint >= cell.q.hi {
            return Ok(RootOutcome::Unresolved(RootReason::NoProgress));
        }
        let (left_evaluator, right_evaluator) = match cell.evaluator {
            RootEvaluator::Bernstein(polynomial) => {
                let numerator = u64::try_from(i64::from(midpoint) - i64::from(cell.q.lo))
                    .map_err(|_| NumericError::Overflow)?;
                let denominator = u64::try_from(i64::from(cell.q.hi) - i64::from(cell.q.lo))
                    .map_err(|_| NumericError::Overflow)?;
                let (left, right) = polynomial.subdivide_at(numerator, denominator, domain)?;
                (
                    RootEvaluator::Bernstein(left),
                    RootEvaluator::Bernstein(right),
                )
            }
            RootEvaluator::IntervalTaylor {
                polynomial,
                remainder,
            } => (
                RootEvaluator::IntervalTaylor {
                    polynomial,
                    remainder,
                },
                RootEvaluator::IntervalTaylor {
                    polynomial,
                    remainder,
                },
            ),
        };
        if stack_len + 2 > MAX_STACK {
            return Ok(RootOutcome::Unresolved(RootReason::CapacitySheets));
        }
        // Higher q is nearer. Push the far half first so the near half is
        // processed first by the LIFO stack.
        stack[stack_len] = Cell {
            q: Iv32::new(cell.q.lo, midpoint)?,
            evaluator: left_evaluator,
            depth: cell.depth + 1,
        };
        stack[stack_len + 1] = Cell {
            q: Iv32::new(midpoint, cell.q.hi)?,
            evaluator: right_evaluator,
            depth: cell.depth + 1,
        };
        stack_len += 2;
    }

    if count == 0 {
        return Ok(RootOutcome::CertifiedNone);
    }
    roots[..count].sort_by(|a, b| b.interval.lo.cmp(&a.interval.lo));
    let mut merged = [RootInterval {
        interval: Iv32::point(0),
        unique: false,
    }; MAX_ROOTS];
    let mut merged_count = 0_usize;
    for root in roots[..count].iter().copied() {
        if let Some(previous) = merged.get_mut(merged_count.saturating_sub(1))
            && previous.interval == root.interval
        {
            previous.unique &= root.unique;
            continue;
        }
        merged[merged_count] = root;
        merged_count += 1;
    }
    Ok(RootOutcome::Roots {
        roots: merged,
        count: u16::try_from(merged_count).map_err(|_| NumericError::CapacityExceeded)?,
    })
}

pub fn bernstein_is_identically_zero(polynomial: Bernstein) -> bool {
    polynomial.coefficients[..=usize::from(polynomial.degree)]
        .iter()
        .all(|coefficient| *coefficient == Iv32::point(0))
}

fn evaluator_is_identically_zero(evaluator: RootEvaluator) -> bool {
    match evaluator {
        RootEvaluator::Bernstein(polynomial) => bernstein_is_identically_zero(polynomial),
        RootEvaluator::IntervalTaylor {
            polynomial,
            remainder,
        } => {
            remainder == Iv32::point(0)
                && polynomial.coefficients[..=usize::from(polynomial.degree)]
                    .iter()
                    .all(|coefficient| *coefficient == Iv32::point(0))
        }
    }
}

fn record_root(
    roots: &mut [RootInterval; MAX_ROOTS],
    count: &mut usize,
    capacity: u16,
    candidate: RootInterval,
) -> Result<bool, NumericError> {
    if let Some(previous) = roots.get_mut((*count).saturating_sub(1))
        && previous.interval == candidate.interval
    {
        previous.unique &= candidate.unique;
        return Ok(true);
    }
    if *count >= usize::from(capacity) {
        return Ok(false);
    }
    roots[*count] = candidate;
    *count += 1;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pixels::reference::poly::Polynomial;

    const D: FixedDomain = FixedDomain::full(-8);

    fn isolate(coefficients: &[i32]) -> RootOutcome {
        let polynomial = Polynomial::new(
            &coefficients
                .iter()
                .copied()
                .map(|value| Iv32::point(value.checked_mul(256).unwrap()))
                .collect::<Vec<_>>(),
        )
        .unwrap()
        .to_bernstein(D)
        .unwrap();
        isolate_all_roots(polynomial, Iv32::new(0, 256).unwrap(), D, 1, 12, 16).unwrap()
    }

    #[test]
    fn plane_and_no_root_are_complete() {
        let outcome = isolate(&[-1, 2]);
        let RootOutcome::Roots { count, .. } = outcome else {
            panic!("plane root must isolate: {outcome:?}");
        };
        assert_eq!(count, 1);
        assert_eq!(isolate(&[1, 1]), RootOutcome::CertifiedNone);
    }

    #[test]
    fn tangent_is_a_corridor_never_a_miss() {
        let outcome = isolate(&[1, -2, 1]);
        let RootOutcome::Roots { roots, count } = outcome else {
            panic!("tangent root must remain represented: {outcome:?}");
        };
        assert!(count >= 1);
        assert!(roots[..usize::from(count)].iter().any(|root| !root.unique));
    }

    #[test]
    fn torus_like_multi_root_and_close_roots_are_complete() {
        for coefficients in [&[24, -250, 875, -1250, 625][..], &[20, -90, 100][..]] {
            let outcome = isolate(coefficients);
            let RootOutcome::Roots { roots, count } = outcome else {
                panic!("multi-root polynomial must isolate or retain corridors: {outcome:?}");
            };
            assert!(count >= 2);
            assert!(
                roots[..usize::from(count)]
                    .windows(2)
                    .all(|pair| pair[0].interval.lo >= pair[1].interval.lo)
            );
        }
    }

    #[test]
    fn positive_leaf_sublevel_boundary_is_not_confused_with_a_leaf_zero() {
        assert_eq!(isolate(&[1, 1]), RootOutcome::CertifiedNone);
        let RootOutcome::Roots { count, .. } = isolate(&[-1, 1]) else {
            panic!("leaf-support boundary must isolate");
        };
        assert!(count >= 1);
    }

    #[test]
    fn distinct_roots_straddling_a_midpoint_are_never_merged() {
        let outcome = isolate(&[262_143, -1_048_576, 1_048_576]);
        let RootOutcome::Roots { roots, count } = outcome else {
            panic!("close roots must remain explicit: {outcome:?}");
        };
        assert_eq!(count, 2);
        assert!(roots[0].unique && roots[1].unique);
        assert_ne!(roots[0].interval, roots[1].interval);
    }

    #[test]
    fn tolerance_cell_with_multiple_variations_is_never_one_complete_root() {
        let mut coefficients = [Iv32::point(0); super::super::poly::MAX_COEFFICIENTS];
        coefficients[..3].copy_from_slice(&[Iv32::point(3), Iv32::point(-5), Iv32::point(3)]);
        let polynomial = Bernstein {
            coefficients,
            degree: 2,
        };
        let outcome = isolate_all_roots(
            polynomial,
            Iv32::new(0, 1).unwrap(),
            FixedDomain::full(0),
            1,
            1,
            16,
        )
        .unwrap();
        assert_eq!(outcome, RootOutcome::Unresolved(RootReason::NoProgress));
    }

    #[test]
    fn endpoint_root_plus_interior_variation_is_never_one_unique_root() {
        let mut coefficients = [Iv32::point(0); super::super::poly::MAX_COEFFICIENTS];
        coefficients[..3].copy_from_slice(&[Iv32::point(0), Iv32::point(-1), Iv32::point(1)]);
        let outcome = isolate_all_roots(
            Bernstein {
                coefficients,
                degree: 2,
            },
            Iv32::new(0, 1).unwrap(),
            FixedDomain::full(0),
            1,
            1,
            16,
        )
        .unwrap();
        assert_eq!(outcome, RootOutcome::Unresolved(RootReason::NoProgress));
    }

    #[test]
    fn odd_width_domain_uses_the_exact_integer_split_ratio() {
        let mut coefficients = [Iv32::point(0); super::super::poly::MAX_COEFFICIENTS];
        coefficients[0] = Iv32::point(-2);
        coefficients[1] = Iv32::point(3);
        let outcome = isolate_all_roots(
            Bernstein {
                coefficients,
                degree: 1,
            },
            Iv32::new(0, 3).unwrap(),
            FixedDomain::full(0),
            1,
            8,
            16,
        )
        .unwrap();
        let RootOutcome::Roots { roots, count } = outcome else {
            panic!("odd-width root must remain enclosed: {outcome:?}");
        };
        assert!(count >= 1);
        assert!(
            roots[..usize::from(count)]
                .iter()
                .any(|root| root.interval.lo <= 1 && root.interval.hi >= 2)
        );
    }

    #[test]
    fn capacity_failure_never_returns_a_partial_complete_list() {
        let polynomial = Polynomial::new(&[Iv32::point(0), Iv32::point(-1), Iv32::point(1)])
            .unwrap()
            .to_bernstein(D)
            .unwrap();
        assert!(matches!(
            isolate_all_roots(polynomial, Iv32::new(0, 256).unwrap(), D, 1, 12, 1).unwrap(),
            RootOutcome::Unresolved(RootReason::CapacityRoots)
        ));
    }

    #[test]
    fn identically_zero_polynomial_is_an_unresolved_corridor() {
        let polynomial = Polynomial::new(&[Iv32::point(0), Iv32::point(0), Iv32::point(0)])
            .unwrap()
            .to_bernstein(D)
            .unwrap();
        assert_eq!(
            isolate_all_roots(polynomial, Iv32::new(0, 256).unwrap(), D, 1, 12, 16),
            Ok(RootOutcome::Unresolved(RootReason::DegenerateZero))
        );
    }

    #[test]
    fn interval_taylor_fallback_is_complete_without_claiming_uniqueness() {
        let polynomial = Polynomial::new(&[Iv32::point(-1), Iv32::point(1)]).unwrap();
        let outcome = isolate_roots(
            RootEvaluator::IntervalTaylor {
                polynomial,
                remainder: Iv32::point(0),
            },
            Iv32::new(0, 2).unwrap(),
            FixedDomain::full(0),
            1,
            4,
            16,
        )
        .unwrap();
        let RootOutcome::Roots { roots, count } = outcome else {
            panic!("Taylor fallback must retain the crossing: {outcome:?}");
        };
        assert!(count >= 1);
        assert!(roots[..usize::from(count)].iter().all(|root| !root.unique));

        let excluded = Polynomial::new(&[Iv32::point(5)]).unwrap();
        assert_eq!(
            isolate_roots(
                RootEvaluator::IntervalTaylor {
                    polynomial: excluded,
                    remainder: Iv32::new(-1, 1).unwrap(),
                },
                Iv32::new(0, 2).unwrap(),
                FixedDomain::full(0),
                1,
                4,
                16,
            ),
            Ok(RootOutcome::CertifiedNone)
        );
    }

    #[test]
    fn interval_taylor_numeric_failure_is_not_a_negative_proof() {
        let polynomial = Polynomial::new(&[Iv32::point(0), Iv32::point(i32::MAX)]).unwrap();
        assert_eq!(
            isolate_roots(
                RootEvaluator::IntervalTaylor {
                    polynomial,
                    remainder: Iv32::point(0),
                },
                Iv32::new(i32::MAX - 1, i32::MAX).unwrap(),
                FixedDomain::full(0),
                1,
                4,
                16,
            ),
            Ok(RootOutcome::Unresolved(RootReason::Numeric))
        );
    }
}
