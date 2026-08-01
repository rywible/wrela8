//! Proxy A/B harness (plans/M18.md items H+J, decisions 1370–1374 /
//! 1385–1389; plans/M19.md item D / 1440–1449; integrity Item E).
//!
//! Rank two emissions by scoreboard total only — no wall time, no host
//! calibration.
//!
//! The three `BoundsElide` A/B oracles that used to live here — the
//! capstone smoke, its mode-named twin, and the per-fn corpus
//! monotonicity run — left with the opt's `RELEASE_OPTS` membership
//! (plans/codegen-pareto-2.md item L, decision 1970) and did **not**
//! come back when item N parked the opt (decision 1912). Two of them
//! asked `Release < Dev` on a fixture, which is simply false for an opt
//! the product does not ship, and the third re-scored the whole cost
//! corpus twice to assert a monotonicity that the transform's own shape
//! guarantees. What replaced them is one artifact-level oracle over the
//! opt *named explicitly* —
//! `opts::win::tests::parked_bounds_elide_still_transforms_and_is_still_flat_on_the_appliance`
//! — plus `diff-eval --with-opt BoundsElide`. Ranking two *opt lists* is
//! `opts::win::compare_opt_lists_over_box`'s job and always was.
//!
//! Cost tags / scoreboard stay always-on in both modes (freeze 1408);
//! modes flip emission, not instrumentation.

use std::cmp::Ordering;

use crate::codegen::CodegenProgram;
use crate::opts::CompileMode;
use crate::placement::PlacementTable;

use super::score::{CostReport, score_program};
use super::table::CostTable;

/// Options that label which emission was scored (mirrors
/// `opts::apply_mode` / `CompileMode`). Scoring itself ignores this —
/// cost instrumentation is always-on (freeze 1408); callers use it to
/// name which program they passed in.
#[derive(Debug, Clone, Copy)]
pub struct CostOpts {
    /// Mode used to produce the emission (`Release` ⇒ `RELEASE_OPTS`;
    /// `Dev` ⇒ every opt off).
    pub mode: CompileMode,
}

impl Default for CostOpts {
    fn default() -> Self {
        // Product default path = release (plans/M19.md item D).
        Self {
            mode: CompileMode::Release,
        }
    }
}

/// Score a program under opts. Scoring ignores opts except that
/// callers use opts to choose which program/emission to score;
/// instrumentation remains always-on (freeze 1408).
pub fn score_with_opts(
    program: &CodegenProgram,
    table: &CostTable,
    placement: &PlacementTable,
    _opts: CostOpts,
) -> Result<CostReport, String> {
    score_program(program, table, placement)
}

/// Compare two emissions: returns Ordering of a.total vs b.total (proxy rank).
pub fn rank_cmp(a: &CostReport, b: &CostReport) -> Ordering {
    a.total_proxy_cycles.cmp(&b.total_proxy_cycles)
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::*;
    use crate::codegen::CodegenFn;
    use crate::cost::rule::{CostRule, EmittedWord};
    use crate::cost::table::load_default;
    use crate::opts::CompileMode;

    /// plans/M20.md item D: score against the committed profile, not an
    /// inline v2 fixture.
    fn table() -> CostTable {
        load_default().expect("bench/a76-pi5.toml")
    }

    fn word(rule: CostRule, dst: Option<u8>, srcs: &[u8]) -> EmittedWord {
        EmittedWord::new(0, String::new(), rule, dst, srcs)
    }

    fn prog(key: &str, code: Vec<EmittedWord>) -> CodegenProgram {
        let mut fns = BTreeMap::new();
        fns.insert(
            key.to_string(),
            CodegenFn {
                frame_size: 0,
                code,
                relocs: Vec::new(),
            },
        );
        CodegenProgram {
            fns,
            rodata: Vec::new(),
            ..Default::default()
        }
    }

    #[test]
    fn identical_program_scored_twice_equal_totals() {
        let table = table();
        let p = prog(
            "f",
            vec![
                word(CostRule::Alu, Some(1), &[0, 0]),
                word(CostRule::Alu, Some(2), &[1, 1]),
            ],
        );
        let opts = CostOpts {
            mode: CompileMode::Release,
        };
        let place = PlacementTable::default();
        let a = score_with_opts(&p, &table, &place, opts).expect("first");
        let b = score_with_opts(&p, &table, &place, opts).expect("second");
        assert_eq!(a.total_proxy_cycles, b.total_proxy_cycles);
        assert_eq!(rank_cmp(&a, &b), Ordering::Equal);
    }

    #[test]
    fn dependent_chain_ranks_higher_than_independent_alus() {
        let table = table();
        // Program A: independent alus (same issue window).
        let a_prog = prog(
            "a",
            vec![
                word(CostRule::Alu, Some(1), &[0, 0]),
                word(CostRule::Alu, Some(2), &[3, 3]),
            ],
        );
        // Program B: dependent chain, same word count — longer schedule.
        let b_prog = prog(
            "b",
            vec![
                word(CostRule::Alu, Some(1), &[0, 0]),
                word(CostRule::Alu, Some(2), &[1, 1]),
            ],
        );
        let opts = CostOpts::default();
        let place = PlacementTable::default();
        let a = score_with_opts(&a_prog, &table, &place, opts).expect("a");
        let b = score_with_opts(&b_prog, &table, &place, opts).expect("b");
        assert!(
            b.total_proxy_cycles > a.total_proxy_cycles,
            "dependent {} should exceed independent {}",
            b.total_proxy_cycles,
            a.total_proxy_cycles
        );
        assert_eq!(rank_cmp(&a, &b), Ordering::Less);
        assert_eq!(rank_cmp(&b, &a), Ordering::Greater);
    }
}
