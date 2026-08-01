use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet};

use super::table::CostTable;

thread_local! {
    static READS: RefCell<Option<BTreeSet<String>>> = const { RefCell::new(None) };
}

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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SweepPoint {
    values: BTreeMap<String, u64>,
}

impl SweepPoint {
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

    pub fn dimensions(&self) -> Vec<&str> {
        self.values.keys().map(String::as_str).collect()
    }

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

    pub fn is_feasible(&self, table: &CostTable) -> bool {
        self.violations(table).is_empty()
    }
}

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
        let untouched: Vec<&str> = dims.iter().skip(2).copied().collect();
        for c in &corners {
            for d in &untouched {
                assert_eq!(c.get(d), table.sweep(d).expect("row").pinned);
            }
        }
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
        assert!(
            label.starts_with(&format!("{}=", dims[0])),
            "label should lead with the first sorted dimension, got: {label}"
        );
    }

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
        let _ = p.get("dram_latency");
        let ((), again) = record_reads(|| {});
        assert!(again.is_empty(), "recorder leaked across scopes: {again:?}");
    }

    #[should_panic(expected = "already recording")]
    #[test]
    fn nested_recording_fails_closed() {
        let table = load_default().expect("committed profile");
        let p = SweepPoint::pinned(&table);
        let _ = record_reads(|| record_reads(|| p.get("l2_latency")));
    }

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

    #[test]
    fn a_constraint_is_checked_against_held_dimensions_too() {
        let t = load_default().expect("committed profile");
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
