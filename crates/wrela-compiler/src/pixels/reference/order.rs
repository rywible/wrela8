//! Deterministic q-order predicates for small active root lists.

use super::iv32::{Iv32, NumericError};

pub const MAX_ORDERED_ROOTS: usize = 64;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OrderRelation {
    Nearer,
    Farther,
    Corridor,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct OrderedRoot {
    pub q: Iv32,
    pub feature_id: u32,
    pub orientation: i8,
}

pub fn compare(a: Iv32, b: Iv32) -> OrderRelation {
    if a.lo > b.hi {
        OrderRelation::Nearer
    } else if b.lo > a.hi {
        OrderRelation::Farther
    } else {
        OrderRelation::Corridor
    }
}

pub fn adjacent_slack(roots: &[OrderedRoot]) -> Result<i32, NumericError> {
    if roots.len() > MAX_ORDERED_ROOTS {
        return Err(NumericError::CapacityExceeded);
    }
    if roots.len() < 2 {
        return Ok(i32::MAX);
    }
    let mut minimum = i64::from(i32::MAX);
    for pair in roots.windows(2) {
        if compare(pair[0].q, pair[1].q) != OrderRelation::Nearer {
            return Ok(0);
        }
        minimum = minimum.min(i64::from(pair[0].q.lo) - i64::from(pair[1].q.hi));
    }
    i32::try_from(minimum).map_err(|_| NumericError::Overflow)
}

/// Forms interval-overlap components, orders disjoint components from near to
/// far, and applies the feature/orientation storage key only within a complete
/// component. Connected overlap is intentionally transitive: if A overlaps B
/// and B overlaps C, none of A/B/C has strict visibility authority over the
/// others even when A and C are disjoint.
pub fn insertion_sort(roots: &mut [OrderedRoot]) -> Result<(), NumericError> {
    if roots.len() > MAX_ORDERED_ROOTS {
        return Err(NumericError::CapacityExceeded);
    }
    // First establish a total near-to-far endpoint order so connected
    // components can be discovered independently of the input permutation.
    for index in 1..roots.len() {
        let value = roots[index];
        let mut destination = index;
        while destination != 0 && endpoint_before(value, roots[destination - 1]) {
            roots[destination] = roots[destination - 1];
            destination -= 1;
        }
        roots[destination] = value;
    }
    let mut component_start = 0;
    while component_start < roots.len() {
        let mut component_end = component_start + 1;
        let mut component_min_lo = roots[component_start].q.lo;
        while component_end < roots.len() && roots[component_end].q.hi >= component_min_lo {
            component_min_lo = component_min_lo.min(roots[component_end].q.lo);
            component_end += 1;
        }
        for index in component_start + 1..component_end {
            let value = roots[index];
            let mut destination = index;
            while destination > component_start && feature_before(value, roots[destination - 1]) {
                roots[destination] = roots[destination - 1];
                destination -= 1;
            }
            roots[destination] = value;
        }
        component_start = component_end;
    }
    Ok(())
}

fn endpoint_before(a: OrderedRoot, b: OrderedRoot) -> bool {
    (
        a.q.lo,
        a.q.hi,
        std::cmp::Reverse(a.feature_id),
        std::cmp::Reverse(a.orientation),
    ) > (
        b.q.lo,
        b.q.hi,
        std::cmp::Reverse(b.feature_id),
        std::cmp::Reverse(b.orientation),
    )
}

fn feature_before(a: OrderedRoot, b: OrderedRoot) -> bool {
    (
        a.feature_id,
        a.orientation,
        std::cmp::Reverse(a.q.lo),
        std::cmp::Reverse(a.q.hi),
    ) < (
        b.feature_id,
        b.orientation,
        std::cmp::Reverse(b.q.lo),
        std::cmp::Reverse(b.q.hi),
    )
}

pub fn all_pairs_agree_with_adjacent(roots: &[OrderedRoot]) -> bool {
    for left in 0..roots.len() {
        for right in left + 1..roots.len() {
            if compare(roots[left].q, roots[right].q) != OrderRelation::Nearer {
                return false;
            }
        }
    }
    adjacent_slack(roots).is_ok_and(|slack| slack > 0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn close_intervals_remain_a_corridor() {
        let a = Iv32::new(10, 12).unwrap();
        let b = Iv32::new(11, 13).unwrap();
        assert_eq!(compare(a, b), OrderRelation::Corridor);
    }

    #[test]
    fn adjacent_order_implies_all_pairs() {
        let mut roots = [
            OrderedRoot {
                q: Iv32::new(0, 1).unwrap(),
                feature_id: 0,
                orientation: 1,
            },
            OrderedRoot {
                q: Iv32::new(8, 9).unwrap(),
                feature_id: 1,
                orientation: 1,
            },
            OrderedRoot {
                q: Iv32::new(4, 5).unwrap(),
                feature_id: 2,
                orientation: -1,
            },
        ];
        insertion_sort(&mut roots).unwrap();
        assert!(all_pairs_agree_with_adjacent(&roots));
        assert_eq!(adjacent_slack(&roots), Ok(3));
    }

    #[test]
    fn overlap_components_are_permutation_invariant() {
        let a = OrderedRoot {
            q: Iv32::new(0, 2).unwrap(),
            feature_id: 0,
            orientation: 1,
        };
        let b = OrderedRoot {
            q: Iv32::new(1, 10).unwrap(),
            feature_id: 1,
            orientation: -1,
        };
        let c = OrderedRoot {
            q: Iv32::new(9, 11).unwrap(),
            feature_id: 2,
            orientation: 1,
        };
        let far = OrderedRoot {
            q: Iv32::new(-4, -3).unwrap(),
            feature_id: 3,
            orientation: 1,
        };
        let mut forward = [a, b, c, far];
        let mut reverse = [far, c, b, a];
        insertion_sort(&mut forward).unwrap();
        insertion_sort(&mut reverse).unwrap();
        assert_eq!(forward, reverse);
        assert_eq!(forward, [a, b, c, far]);
    }
}
