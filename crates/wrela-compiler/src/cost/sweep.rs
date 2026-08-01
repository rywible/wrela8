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

use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet};

use super::table::CostTable;

thread_local! {
    /// `Some` while [`record_reads`] is running: the set of dimension names
    /// [`SweepPoint::get`] has been asked for. `None` — the ordinary case —
    /// costs one thread-local borrow per read and records nothing.
    static READS: RefCell<Option<BTreeSet<String>>> = const { RefCell::new(None) };
}

/// Run `f` while recording every dimension name read through
/// [`SweepPoint::get`], returning `f`'s value and that set.
///
/// **Why this exists.** Item J's sensitivity probe must decide which
/// dimensions can be held at their pinned value without dropping one
/// (decision 1604). "The total did not move when I nudged `d`" is evidence;
/// "the model never read `d` at all while scoring this program" is a
/// *reason* — a dimension no term reads cannot change a score at any point
/// of the box, whatever the other dimensions are doing. The probe excludes
/// only on the second, and cross-checks it against the first.
///
/// Nesting is a bug, not a mode: the inner call would silently steal the
/// outer one's set, so it panics.
pub fn record_reads<R>(f: impl FnOnce() -> R) -> (R, BTreeSet<String>) {
    READS.with(|c| {
        let mut slot = c.borrow_mut();
        assert!(
            slot.is_none(),
            "cost::sweep::record_reads is already recording on this thread"
        );
        *slot = Some(BTreeSet::new());
    });
    let value = f();
    let read = READS.with(|c| {
        c.borrow_mut()
            .take()
            .expect("record_reads slot vanished mid-run")
    });
    (value, read)
}

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
        READS.with(|c| {
            if let Some(set) = c.borrow_mut().as_mut() {
                set.insert(dim.to_string());
            }
        });
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

    /// Every joint constraint (`[sweep.*].le`) this point violates, as
    /// readable text — empty for a point the machine could be at
    /// (plans/codegen-pareto-2.md decision 1950).
    ///
    /// Reads the point's raw values, **not** through [`Self::get`]:
    /// feasibility is a property of the box, and recording it as a model
    /// read would make every dimension look live to item J's sensitivity
    /// probe.
    pub fn violations(&self, table: &CostTable) -> Vec<String> {
        let mut out = Vec::new();
        for dim in table.sweep_dimensions() {
            let row = match table.sweep(dim) {
                Some(r) => r,
                None => continue,
            };
            let Some(b) = &row.le else { continue };
            let v = self.values.get(dim).copied().unwrap_or(row.pinned);
            let m = self
                .values
                .get(&b.minuend)
                .copied()
                .unwrap_or_else(|| table.sweep(&b.minuend).map(|r| r.pinned).unwrap_or(0));
            let s = b
                .subtrahend
                .as_ref()
                .map(|d| {
                    self.values
                        .get(d)
                        .copied()
                        .unwrap_or_else(|| table.sweep(d).map(|r| r.pinned).unwrap_or(0))
                })
                .unwrap_or(0);
            let bound = b.bound_at(m, s);
            if v > bound {
                out.push(format!("{dim}={v} > {} = {bound}", b.expr()));
            }
        }
        out
    }

    /// True when every joint constraint holds — i.e. this is a point the
    /// machine could actually be at.
    pub fn is_feasible(&self, table: &CostTable) -> bool {
        self.violations(table).is_empty()
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
///
/// **Corners the machine cannot be at are not enumerated**
/// (plans/codegen-pareto-2.md decision 1950). The box is a product of
/// independent brackets, so two brackets over correlated quantities admit
/// points no silicon reaches — and one such point is enough for the ∀ gate
/// to veto a correct optimization, since the gate treats every corner as
/// equally admissible. Every `[sweep.*].le` constraint is checked against
/// the whole point (swept dimensions *and* the ones held pinned) and a
/// violating corner is dropped. Removing corners can only ever make a
/// candidate easier to land, which is why each constraint must carry its
/// `le_physics` and why the pinned point is asserted feasible at parse.
///
/// Fails closed on an empty result: a box with no admissible corner would
/// make the ∀ gate vacuously true, which is the one thing a gate must
/// never be.
pub fn endpoint_corners(table: &CostTable, dims: &[&str]) -> Vec<SweepPoint> {
    let out = endpoint_corners_unconstrained(table, dims);
    let total = out.len();
    let kept: Vec<SweepPoint> = out.into_iter().filter(|p| p.is_feasible(table)).collect();
    assert!(
        !kept.is_empty(),
        "the residual box has no feasible corner over [{}] of {total} enumerated — every \
         point violates a [sweep.*].le constraint, which would make the ∀ gate vacuous",
        dims.join(", ")
    );
    kept
}

/// [`endpoint_corners`] without the feasibility filter — the raw product
/// box. Exists so the units can show what the filter removes; **not**
/// public, because a caller that wanted the unfiltered box would be asking
/// the gate to rank at points the machine cannot be at.
fn endpoint_corners_unconstrained(table: &CostTable, dims: &[&str]) -> Vec<SweepPoint> {
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

    /// The read recorder sees exactly the dimensions asked for, and nothing
    /// leaks outside the recorded scope.
    #[test]
    fn record_reads_sees_exactly_the_dimensions_asked_for() {
        let table = load_default().expect("committed profile");
        let p = SweepPoint::pinned(&table);
        let (sum, read) = record_reads(|| p.get("l2_latency") + p.get("l3_latency"));
        assert!(sum > 0);
        assert_eq!(
            read.iter().map(String::as_str).collect::<Vec<_>>(),
            vec!["l2_latency", "l3_latency"]
        );
        // Outside the scope nothing is recorded, and a second scope starts
        // empty rather than accumulating.
        let _ = p.get("dram_latency");
        let ((), again) = record_reads(|| {});
        assert!(again.is_empty(), "recorder leaked across scopes: {again:?}");
    }

    // `#[should_panic]` first so `xtask check`'s `#[test]`-then-`fn` scan
    // can see this name (the same shape `cost::crosscore` uses).
    #[should_panic(expected = "already recording")]
    #[test]
    fn nested_recording_fails_closed() {
        let table = load_default().expect("committed profile");
        let p = SweepPoint::pinned(&table);
        let _ = record_reads(|| record_reads(|| p.get("l2_latency")));
    }

    // -----------------------------------------------------------------
    // Joint constraints (plans/codegen-pareto-2.md item K, decision 1950)
    // -----------------------------------------------------------------

    /// The `[sweep.divide_w_latency]` row plans/codegen-pareto.md decision
    /// 1749 could not add, with its constraint. This is the fixture the
    /// defect is stated over: item C found that independent `[5,20]` and
    /// `[5,12]` brackets put `(divide_x_latency = 5, divide_w_latency = 12)`
    /// inside the box, and refused to add the row because of it.
    ///
    /// It lives here rather than in `bench/a76-pi5.toml` because freeze
    /// 1630 admits a `[latency]` group only where the emitted stream
    /// contains it, and item K changes no emission (decision 1952). The row
    /// is written out in full so whoever lands C1's divide half or C4 can
    /// paste it, constraint included, without re-deriving the argument.
    const DIVIDE_W_ROW: &str = "\n\
        [sweep.divide_w_latency]\n\
        lo = 5\n\
        hi = 12\n\
        pinned = 12\n\
        pessimistic = \"hi\"\n\
        le = \"divide_x_latency\"\n\
        le_physics = \"one divider, one early-termination rule: a 32-bit divide \
        of a value cannot take more iterations than the 64-bit divide of the same \
        value, because it has strictly fewer significant bits to retire. \
        divide_w_latency > divide_x_latency describes no A76.\"\n\
        tier = \"T1\"\n\
        source = \"SOG 3.6 divide, W-form - 5-12 cycles with data-dependent early termination\"\n";

    fn table_text() -> String {
        std::fs::read_to_string(crate::cost::table::default_table_path())
            .expect("bench/a76-pi5.toml")
    }

    /// **K1's regression test.** No corner of the box may be a point the
    /// machine cannot be at — and the *same* box without the constraint
    /// enumerates exactly such a corner, which is the old behaviour.
    #[test]
    fn no_physically_impossible_divide_corner_is_enumerated() {
        let constrained =
            crate::cost::table::parse(&(table_text() + DIVIDE_W_ROW)).expect("constrained");
        let unconstrained = crate::cost::table::parse(
            &(table_text()
                + &DIVIDE_W_ROW
                    .lines()
                    .filter(|l| !l.starts_with("le"))
                    .collect::<Vec<_>>()
                    .join("\n")),
        )
        .expect("unconstrained");

        let dims = ["divide_x_latency", "divide_w_latency"];
        let loose = endpoint_corners_unconstrained(&unconstrained, &dims);
        assert_eq!(loose.len(), 4, "the raw product box is 2^2");
        let impossible: Vec<String> = loose
            .iter()
            .filter(|p| p.get("divide_w_latency") > p.get("divide_x_latency"))
            .map(|p| p.label_over(&dims))
            .collect();
        assert_eq!(
            impossible,
            vec!["divide_x_latency=5 divide_w_latency=12".to_string()],
            "the defect: the product box contains a corner where the 32-bit divide \
             is slower than the 64-bit one"
        );

        let tight = endpoint_corners(&constrained, &dims);
        assert_eq!(tight.len(), 3, "the impossible corner is dropped, no other");
        for p in &tight {
            assert!(
                p.get("divide_w_latency") <= p.get("divide_x_latency"),
                "enumerated a corner no divider can be at: {}",
                p.label_over(&dims)
            );
            assert!(p.is_feasible(&constrained));
        }
    }

    /// **The C4-shaped comparison.** A candidate that replaces a 64-bit
    /// divide with the 32-bit divide of the same value must not *rise* at
    /// any point of the box. Over the product box it rises at one corner
    /// and the ∀ gate refuses; over the constrained box there is no such
    /// corner and the substitution wins everywhere.
    ///
    /// Priced arithmetically rather than through `score_program` on
    /// purpose: no site emits a W-form divide (decision 1952), so a scored
    /// program could only assert this by pretending one does. The claim
    /// under test is about the *box*, and this is that claim with nothing
    /// else in it.
    #[test]
    fn the_c4_shaped_comparison_no_longer_vetoes_at_the_divide_lo_corner() {
        let constrained =
            crate::cost::table::parse(&(table_text() + DIVIDE_W_ROW)).expect("constrained");
        let unconstrained = crate::cost::table::parse(
            &(table_text()
                + &DIVIDE_W_ROW
                    .lines()
                    .filter(|l| !l.starts_with("le"))
                    .collect::<Vec<_>>()
                    .join("\n")),
        )
        .expect("unconstrained");
        let dims = ["divide_x_latency", "divide_w_latency"];
        // baseline: one X-form divide. candidate: the same divide at 32 bits.
        // The candidate "rises" exactly where its latency exceeds the
        // baseline's, which is the whole of the comparison for one word.
        let rose = |corners: Vec<SweepPoint>| -> Vec<String> {
            corners
                .into_iter()
                .filter(|p| p.get("divide_w_latency") > p.get("divide_x_latency"))
                .map(|p| p.label_over(&dims))
                .collect()
        };
        assert_eq!(
            rose(endpoint_corners_unconstrained(&unconstrained, &dims)),
            vec!["divide_x_latency=5 divide_w_latency=12".to_string()],
            "old behaviour: the candidate rises at the divide-lo corner, so the ∀ gate \
             vetoes a substitution that cannot be a loss on one divider"
        );
        assert!(
            rose(endpoint_corners(&constrained, &dims)).is_empty(),
            "the candidate must not rise anywhere in the constrained box"
        );
    }

    /// A held dimension is still checked: `endpoint_corners` restricted to
    /// one of a constraint's dimensions must not emit a corner the *pinned*
    /// value of the other makes impossible.
    #[test]
    fn a_constraint_is_checked_against_held_dimensions_too() {
        let t = load_default().expect("committed profile");
        // `snoop_cost <= dram_latency - l3_latency`, with dram and l3 held
        // pinned (347, 35 → bound 312). snoop's hi is exactly 312, so both
        // corners survive; sweeping dram too drops the ones that do not.
        let one = endpoint_corners(&t, &["snoop_cost"]);
        assert_eq!(one.len(), 2);
        let three = endpoint_corners(&t, &["snoop_cost", "dram_latency", "l3_latency"]);
        assert_eq!(
            three.len(),
            6,
            "2 of the 8 product corners are a remote load costing more than DRAM"
        );
        for p in &three {
            assert!(
                p.get("snoop_cost") <= p.get("dram_latency") - p.get("l3_latency"),
                "enumerated a remote load dearer than the DRAM path: {}",
                p.label_over(&["snoop_cost", "dram_latency", "l3_latency"])
            );
        }
    }

    /// The committed profile's own pinned point is feasible — the model may
    /// not be pinned at a point the machine cannot be at.
    #[test]
    fn the_committed_pinned_point_is_physically_realizable() {
        let t = load_default().expect("committed profile");
        let p = SweepPoint::pinned(&t);
        assert_eq!(p.violations(&t), Vec::<String>::new());
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
