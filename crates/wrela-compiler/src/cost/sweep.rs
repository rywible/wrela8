//! A point in the residual-uncertainty box (plans/M20.md decision 1604).
//!
//! The pinned model is one point of this box — every dimension at its
//! **pessimistic end** (decision 1609). The land gate scores both sides at
//! every point that can matter and vetoes on any rank flip, naming the
//! flipping point (item J).
//!
//! **There is deliberately no `∃`-form predicate here** (freeze 1624). This
//! module hands out points and corners; it never answers "does this win
//! somewhere", because that question is a search for a flattering
//! assumption rather than a gate. Nothing in this file may grow a
//! `wins_at_any_point` shape.
//!
//! Consumers: the memory model (item F), the cross-core model (item G), the
//! branch model (item H), and the ∀ sweep itself (item J). Item L asserts
//! that each pinned value really is its bracket's pessimistic end.

use std::collections::BTreeMap;

use super::table::CostTable;

/// One assignment of every residual-uncertainty dimension to a value.
///
/// Constructed only from a `CostTable`'s `[sweep]` section, so a point can
/// never name a dimension the committed profile does not declare.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SweepPoint {
    /// Dimension name → value at this point. Always the full dimension
    /// set; a partial point would let a consumer read a default.
    values: BTreeMap<String, u64>,
}

impl SweepPoint {
    /// The pinned point: every dimension at its `pinned` value, which the
    /// table parser has already asserted is that bracket's pessimistic end.
    /// This is the point every pinned dump and golden is scored at
    /// (freeze 1625).
    pub fn pinned(table: &CostTable) -> SweepPoint {
        let mut values = BTreeMap::new();
        for dim in table.sweep_dimensions() {
            let row = table.sweep(dim).unwrap_or_else(|| {
                panic!("sweep dimension `{dim}` vanished between listing and read")
            });
            values.insert(dim.to_string(), row.pinned);
        }
        SweepPoint { values }
    }

    /// Value of `dim` at this point.
    ///
    /// Panics on an undeclared dimension rather than returning a default:
    /// a model term that reads a dimension the profile does not declare is
    /// a term with no provenance, and silently substituting 0 would make it
    /// a discount (decision 1609).
    pub fn get(&self, dim: &str) -> u64 {
        *self.values.get(dim).unwrap_or_else(|| {
            panic!(
                "sweep point has no dimension `{dim}` (declared: {})",
                self.dimensions().join(", ")
            )
        })
    }

    /// Every dimension name at this point, sorted.
    pub fn dimensions(&self) -> Vec<&str> {
        self.values.keys().map(String::as_str).collect()
    }

    /// A copy with `dim` moved to `value`. Panics on an undeclared
    /// dimension, for the same reason `get` does.
    pub fn with(&self, dim: &str, value: u64) -> SweepPoint {
        if !self.values.contains_key(dim) {
            panic!(
                "cannot set undeclared sweep dimension `{dim}` (declared: {})",
                self.dimensions().join(", ")
            );
        }
        let mut values = self.values.clone();
        values.insert(dim.to_string(), value);
        SweepPoint { values }
    }

    /// Stable one-line rendering, sorted by dimension: this is what a veto
    /// prints when it **names the flipping point** (04 §5 requires every
    /// veto reason that fires to be reported).
    pub fn label(&self) -> String {
        let mut out = String::new();
        for (i, (k, v)) in self.values.iter().enumerate() {
            if i > 0 {
                out.push(' ');
            }
            out.push_str(&format!("{k}={v}"));
        }
        out
    }

    /// Short rendering over `dims` only — the readable form when a case is
    /// sensitive to two dimensions out of seventeen.
    pub fn label_over(&self, dims: &[&str]) -> String {
        let mut out = String::new();
        for (i, d) in dims.iter().enumerate() {
            if i > 0 {
                out.push(' ');
            }
            out.push_str(&format!("{d}={}", self.get(d)));
        }
        out
    }
}

/// Every endpoint corner of the box restricted to `dims`, with all other
/// dimensions held at their pinned value: `2^dims.len()` points, in a
/// deterministic order (lo before hi, first named dimension varying
/// slowest).
///
/// Restricting to `dims` is **not** dropping a dimension (which decision
/// 1604 forbids): the caller's contract is that a dimension left out has
/// been shown not to move either side's total, so no corner over it can
/// flip a rank. Item J owns that sensitivity probe and must state it.
pub fn endpoint_corners(table: &CostTable, dims: &[&str]) -> Vec<SweepPoint> {
    let base = SweepPoint::pinned(table);
    let mut brackets = Vec::with_capacity(dims.len());
    for d in dims {
        let row = table
            .sweep(d)
            .unwrap_or_else(|| panic!("no sweep dimension `{d}` in the committed profile"));
        brackets.push((*d, row.lo, row.hi));
    }
    let mut out = Vec::with_capacity(1usize << dims.len().min(20));
    let total = 1u64 << dims.len().min(63);
    for mask in 0..total {
        let mut p = base.clone();
        for (bit, (d, lo, hi)) in brackets.iter().enumerate() {
            // Bit set → the high end. First named dimension is the most
            // significant bit, so it varies slowest and the printed order
            // reads the way the caller named it.
            let shift = brackets.len() - 1 - bit;
            let hi_end = (mask >> shift) & 1 == 1;
            p = p.with(d, if hi_end { *hi } else { *lo });
        }
        out.push(p);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cost::table::load_default;

    #[test]
    fn pinned_point_matches_every_committed_pinned_value() {
        let table = load_default().expect("committed profile");
        let p = SweepPoint::pinned(&table);
        let dims = table.sweep_dimensions();
        assert!(
            !dims.is_empty(),
            "the committed profile declares no sweep box"
        );
        assert_eq!(p.dimensions(), dims);
        for d in &dims {
            assert_eq!(
                p.get(d),
                table.sweep(d).expect("row").pinned,
                "pinned point disagrees with the table on `{d}`"
            );
        }
    }

    /// The pinned point is the pessimistic corner: every dimension sits at
    /// the end its own row names as over-costing. This is decision 1609's
    /// shape checked through the point type, not only through the parser.
    #[test]
    fn pinned_point_is_the_pessimistic_corner() {
        let table = load_default().expect("committed profile");
        let p = SweepPoint::pinned(&table);
        for d in table.sweep_dimensions() {
            let row = table.sweep(d).expect("row");
            let want = match row.pessimistic {
                crate::cost::table::End::Lo => row.lo,
                crate::cost::table::End::Hi => row.hi,
            };
            assert_eq!(p.get(d), want, "`{d}` is not pinned at its pessimistic end");
        }
    }

    #[test]
    fn corners_are_two_to_the_k_and_deterministic() {
        let table = load_default().expect("committed profile");
        let dims = table.sweep_dimensions();
        let two: Vec<&str> = dims.iter().take(2).copied().collect();
        let corners = endpoint_corners(&table, &two);
        assert_eq!(corners.len(), 4);
        let again = endpoint_corners(&table, &two);
        assert_eq!(corners, again, "corner enumeration must be deterministic");
        // Untouched dimensions stay pinned at every corner.
        let untouched: Vec<&str> = dims.iter().skip(2).copied().collect();
        for c in &corners {
            for d in &untouched {
                assert_eq!(c.get(d), table.sweep(d).expect("row").pinned);
            }
        }
        // First named dimension varies slowest: lo, lo, hi, hi.
        let a = two[0];
        let (lo_a, hi_a) = {
            let r = table.sweep(a).expect("row");
            (r.lo, r.hi)
        };
        assert_eq!(
            [
                corners[0].get(a),
                corners[1].get(a),
                corners[2].get(a),
                corners[3].get(a)
            ],
            [lo_a, lo_a, hi_a, hi_a]
        );
    }

    #[test]
    fn empty_dims_is_the_pinned_point_alone() {
        let table = load_default().expect("committed profile");
        let corners = endpoint_corners(&table, &[]);
        assert_eq!(corners.len(), 1);
        assert_eq!(corners[0], SweepPoint::pinned(&table));
    }

    #[test]
    fn label_is_stable_and_sorted() {
        let table = load_default().expect("committed profile");
        let p = SweepPoint::pinned(&table);
        let label = p.label();
        assert_eq!(label, p.label());
        let dims = p.dimensions();
        // Sorted order: the first dimension's name leads the label.
        assert!(
            label.starts_with(&format!("{}=", dims[0])),
            "label should lead with the first sorted dimension, got: {label}"
        );
    }

    #[test]
    #[should_panic(expected = "no dimension `not_a_dimension`")]
    fn reading_an_undeclared_dimension_fails_closed() {
        let table = load_default().expect("committed profile");
        SweepPoint::pinned(&table).get("not_a_dimension");
    }

    #[test]
    #[should_panic(expected = "cannot set undeclared sweep dimension")]
    fn setting_an_undeclared_dimension_fails_closed() {
        let table = load_default().expect("committed profile");
        SweepPoint::pinned(&table).with("not_a_dimension", 1);
    }
}
