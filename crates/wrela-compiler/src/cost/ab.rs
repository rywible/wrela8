//! Proxy A/B harness (plans/M18.md items H+J, decisions 1370–1374 /
//! 1385–1389).
//!
//! Rank two emissions by scoreboard total only — no wall time, no host
//! calibration. Capstone smoke: lower with `set_bounds_elide` on vs off,
//! then score — on must rank strictly below off.

use std::cmp::Ordering;

use crate::codegen::CodegenProgram;

use super::score::{score_program, CostReport};
use super::table::CostTable;

/// Options that select which emission to score (mirrors lower TLS / CLI
/// off-switches such as `--omit-dmb` / `--no-bounds-elide`).
#[derive(Debug, Clone, Copy, Default)]
pub struct CostOpts {
    /// When true, bounds-elide was enabled for the emission being scored
    /// (mirrors `lower::set_bounds_elide`). Scoring itself ignores this;
    /// callers use it to label which program they passed in.
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
    use crate::cost::table::{load_default, parse};

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

    /// plans/M18.md item J: elide-on ranks strictly below elide-off for a
    /// fixture with many literal `[T; N]` indices (cmp/abort_val words).
    #[test]
    fn bounds_elide_on_ranks_strictly_below_off() {
        use crate::codegen::codegen_program;
        use crate::lower::{lower_program, set_bounds_elide};
        use crate::mwir;
        use crate::sema;
        use crate::syntax::{lexer, parser};

        let src = r#"
module examples.cost_bounds_elide_ab

pub fn hot(a: [u64; 32]) -> u64:
    s0: u64 = a[0] +% a[1]
    s1: u64 = a[2] +% a[3]
    s2: u64 = a[4] +% a[5]
    s3: u64 = a[6] +% a[7]
    s4: u64 = a[8] +% a[9]
    s5: u64 = a[10] +% a[11]
    s6: u64 = a[12] +% a[13]
    s7: u64 = a[14] +% a[15]
    s8: u64 = a[16] +% a[17]
    s9: u64 = a[18] +% a[19]
    s10: u64 = a[20] +% a[21]
    s11: u64 = a[22] +% a[23]
    s12: u64 = a[24] +% a[25]
    s13: u64 = a[26] +% a[27]
    s14: u64 = a[28] +% a[29]
    s15: u64 = a[30] +% a[31]
    t0: u64 = s0 +% s1
    t1: u64 = s2 +% s3
    t2: u64 = s4 +% s5
    t3: u64 = s6 +% s7
    t4: u64 = s8 +% s9
    t5: u64 = s10 +% s11
    t6: u64 = s12 +% s13
    t7: u64 = s14 +% s15
    u0: u64 = t0 +% t1
    u1: u64 = t2 +% t3
    u2: u64 = t4 +% t5
    u3: u64 = t6 +% t7
    v0: u64 = u0 +% u1
    v1: u64 = u2 +% u3
    return v0 +% v1
"#;
        let tokens = lexer::lex(src).expect("lex");
        let module = parser::parse(tokens).expect("parse");
        let typed = sema::check_typed(&module, "<test>").expect("check");
        let layout = mwir::build_layout_ctx(&module, &Default::default()).expect("layout");
        let table = load_default().expect("wrela-cost-v1");

        set_bounds_elide(true);
        let mwir_on = lower_program(&typed).expect("lower on");
        let prog_on = codegen_program(&mwir_on, &layout).expect("codegen on");
        let on = score_with_opts(
            &prog_on,
            &table,
            CostOpts {
                bounds_elide: true,
            },
        )
        .expect("score on");

        set_bounds_elide(false);
        let mwir_off = lower_program(&typed).expect("lower off");
        let prog_off = codegen_program(&mwir_off, &layout).expect("codegen off");
        let off = score_with_opts(
            &prog_off,
            &table,
            CostOpts {
                bounds_elide: false,
            },
        )
        .expect("score off");

        set_bounds_elide(true);

        assert!(
            on.total_proxy_cycles < off.total_proxy_cycles,
            "elide-on {} must rank strictly below elide-off {}",
            on.total_proxy_cycles,
            off.total_proxy_cycles
        );
        assert_eq!(rank_cmp(&on, &off), Ordering::Less);
    }
}
