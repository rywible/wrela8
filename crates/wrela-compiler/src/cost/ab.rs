//! Proxy A/B harness (plans/M18.md item H, decisions 1370–1374).
//!
//! Rank two emissions by scoreboard total only — no wall time, no host
//! calibration. After item I lands, a bounds-elide on/off compare will
//! live here or in item J; until then tests use hand-built programs.

use std::cmp::Ordering;

use crate::codegen::CodegenProgram;

use super::score::{score_program, CostReport};
use super::table::CostTable;

/// Options that select which emission to score (mirrors lower TLS / CLI
/// off-switches such as `--omit-dmb`).
#[derive(Debug, Clone, Copy, Default)]
pub struct CostOpts {
    /// When true, bounds-elide is enabled for ranking compare (mirrors lower TLS).
    /// For H before I wires lower: tests use hand-built CodegenPrograms; this flag
    /// is recorded for J to use.
    pub bounds_elide: bool,
}

/// Score a program under opts. For M18, scoring ignores opts except that
/// callers use opts to choose which program/emission to score.
pub fn score_with_opts(
    program: &CodegenProgram,
    table: &CostTable,
    _opts: CostOpts,
) -> Result<CostReport, String> {
    score_program(program, table)
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
    use crate::cost::table::parse;

    const TABLE: &str = r#"
version = 1
issue_width = 4
[latency]
alu = 1
load = 4
store = 1
branch = 1
call = 1
abort = 1
abort_val = 1
mov_wide = 1
mul = 3
sdiv = 12
udiv = 12
adrp = 1
barrier = 1
system = 1
neon = 1
"#;

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
        }
    }

    #[test]
    fn identical_program_scored_twice_equal_totals() {
        let table = parse(TABLE).expect("table");
        let p = prog(
            "f",
            vec![
                word(CostRule::Alu, Some(1), &[0, 0]),
                word(CostRule::Alu, Some(2), &[1, 1]),
            ],
        );
        let opts = CostOpts {
            bounds_elide: true,
        };
        let a = score_with_opts(&p, &table, opts).expect("first");
        let b = score_with_opts(&p, &table, opts).expect("second");
        assert_eq!(a.total_proxy_cycles, b.total_proxy_cycles);
        assert_eq!(rank_cmp(&a, &b), Ordering::Equal);
    }

    #[test]
    fn dependent_chain_ranks_higher_than_independent_alus() {
        let table = parse(TABLE).expect("table");
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
        let a = score_with_opts(&a_prog, &table, opts).expect("a");
        let b = score_with_opts(&b_prog, &table, opts).expect("b");
        assert!(
            b.total_proxy_cycles > a.total_proxy_cycles,
            "dependent {} should exceed independent {}",
            b.total_proxy_cycles,
            a.total_proxy_cycles
        );
        assert_eq!(rank_cmp(&a, &b), Ordering::Less);
        assert_eq!(rank_cmp(&b, &a), Ordering::Greater);
    }

    /// After item I, a bounds-elide on/off test will live here or in item J
    /// (plans/M18.md H→J). Until then, manual CodegenProgram pairs above
    /// exercise `score_with_opts` / `rank_cmp` without waiting on lower.
    #[test]
    fn bounds_elide_ab_deferred_until_item_i_or_j() {
        let _ = CostOpts {
            bounds_elide: false,
        };
        // Documented placeholder: item I wires lower TLS; item J (or an
        // expanded test here) will score elide-on vs elide-off emissions.
    }
}
