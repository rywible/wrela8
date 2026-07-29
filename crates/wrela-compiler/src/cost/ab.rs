//! Proxy A/B harness (plans/M18.md items H+J, decisions 1370–1374 /
//! 1385–1389; plans/M19.md item D / 1440–1449; integrity Item E).
//!
//! Rank two emissions by scoreboard total only — no wall time, no host
//! calibration. Capstone smoke: lower under `CompileMode::Release`
//! (BoundsElide on) vs `Dev` (elide off) via `opts::apply_mode`, then
//! score — release must rank strictly below dev.
//!
//! Corpus oracle (integrity E): every `tests/golden/cost-*/input.wr`
//! scored with BoundsElide on vs off; per-fn `on.proxy <= off.proxy`.
//!
//! Cost tags / scoreboard stay always-on in both modes (freeze 1408);
//! modes flip emission, not instrumentation.

use std::cmp::Ordering;

use crate::codegen::CodegenProgram;
use crate::opts::CompileMode;

use super::score::{CostReport, score_program};
use super::table::CostTable;

/// Options that label which emission was scored (mirrors
/// `opts::apply_mode` / `CompileMode`). Scoring itself ignores this —
/// cost instrumentation is always-on (freeze 1408); callers use it to
/// name which program they passed in.
#[derive(Debug, Clone, Copy)]
pub struct CostOpts {
    /// Mode used to produce the emission (`Release` ⇒ BoundsElide on;
    /// `Dev` ⇒ elide off).
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
    use crate::cost::stage::codegen_cost_stage;
    use crate::cost::table::{load_default, parse};
    use crate::opts::win::discover_cost_corpus;
    use crate::opts::{CompileMode, OptId, apply_mode, apply_opts};

    const TABLE: &str = r#"
version = 2
[ports]
alu = 2
mem = 2
max_issue_per_cycle = 2
branch_penalty = 3
mem_reuse_window = 8
mem_working_set_cap = 4
[latency]
alu = 1
load = 12
store = 2
branch = 1
call = 4
abort = 1
abort_val = 3
mov_wide = 1
mul = 3
sdiv = 12
udiv = 12
adrp = 1
barrier = 1
system = 1
neon = 1
[mem]
load_stack_hit = 1
load_stack_miss = 4
load_cold_hit = 4
load_cold_miss = 12
store_stack = 1
store_cold = 2
working_set_surcharge = 2
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

    /// Shared fixture: many literal `[T; N]` indices → cmp/abort_val
    /// words when BoundsElide is off.
    const BOUNDS_ELIDE_AB_SRC: &str = r#"
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

    fn lower_codegen_score(src: &str, mode: CompileMode) -> CostReport {
        use crate::codegen::codegen_program;
        use crate::lower::lower_program;
        use crate::mwir;
        use crate::sema;
        use crate::syntax::{lexer, parser};

        let tokens = lexer::lex(src).expect("lex");
        let module = parser::parse(tokens).expect("parse");
        let typed = sema::check_typed(&module, "<test>").expect("check");
        let layout = mwir::build_layout_ctx(&module, &Default::default()).expect("layout");
        let table = load_default().expect("wrela-cost-v1");

        apply_mode(mode);
        let mwir = lower_program(&typed).expect("lower");
        let prog = codegen_program(&mwir, &layout).expect("codegen");
        score_with_opts(&prog, &table, CostOpts { mode }).expect("score")
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
            mode: CompileMode::Release,
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

    /// plans/M18.md item J / M19 item D: elide-on (Release) ranks
    /// strictly below elide-off (Dev). Driven via `apply_mode`.
    #[test]
    fn bounds_elide_on_ranks_strictly_below_off() {
        let on = lower_codegen_score(BOUNDS_ELIDE_AB_SRC, CompileMode::Release);
        let off = lower_codegen_score(BOUNDS_ELIDE_AB_SRC, CompileMode::Dev);
        // Restore product default.
        apply_mode(CompileMode::Release);

        assert!(
            on.total_proxy_cycles < off.total_proxy_cycles,
            "elide-on {} must rank strictly below elide-off {}",
            on.total_proxy_cycles,
            off.total_proxy_cycles
        );
        assert_eq!(rank_cmp(&on, &off), Ordering::Less);
    }

    /// plans/M19.md item D: mode-named twin of the BoundsElide A/B
    /// oracle — `Release` ranks strictly below `Dev` on the same fixture.
    /// Cost instrumentation runs in both modes (freeze 1408).
    #[test]
    fn release_ranks_strictly_below_dev_on_bounds_elide_fixture() {
        let release = lower_codegen_score(BOUNDS_ELIDE_AB_SRC, CompileMode::Release);
        let dev = lower_codegen_score(BOUNDS_ELIDE_AB_SRC, CompileMode::Dev);
        apply_mode(CompileMode::Release);

        assert!(
            release.total_proxy_cycles < dev.total_proxy_cycles,
            "release {} must rank strictly below dev {}",
            release.total_proxy_cycles,
            dev.total_proxy_cycles
        );
        assert_eq!(rank_cmp(&release, &dev), Ordering::Less);
    }

    /// Integrity Item E: every `cost-*` golden, BoundsElide on vs off,
    /// per-fn proxy must be monotonic (`on <= off`). Fail closed if the
    /// corpus is empty. Isolates BoundsElide (NarrowImm held off).
    #[test]
    fn bounds_elide_corpus_per_fn_on_leq_off() {
        let corpus = discover_cost_corpus();
        assert!(
            !corpus.is_empty(),
            "cost corpus empty: expected tests/golden/cost-*/input.wr"
        );

        let table = load_default().expect("wrela-cost-v1");

        for path in &corpus {
            let case = path
                .parent()
                .and_then(|p| p.file_name())
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_else(|| path.display().to_string());

            apply_opts(&[OptId::BoundsElide]);
            let on_prog = codegen_cost_stage(path).unwrap_or_else(|e| {
                panic!("codegen BoundsElide-on {case}: {e}");
            });
            let on = score_with_opts(
                &on_prog,
                &table,
                CostOpts {
                    mode: CompileMode::Release,
                },
            )
            .unwrap_or_else(|e| panic!("score BoundsElide-on {case}: {e}"));

            apply_opts(&[]);
            let off_prog = codegen_cost_stage(path).unwrap_or_else(|e| {
                panic!("codegen BoundsElide-off {case}: {e}");
            });
            let off = score_with_opts(
                &off_prog,
                &table,
                CostOpts {
                    mode: CompileMode::Dev,
                },
            )
            .unwrap_or_else(|e| panic!("score BoundsElide-off {case}: {e}"));

            let off_by_key: BTreeMap<&str, u64> = off
                .fns
                .iter()
                .map(|f| (f.key.as_str(), f.proxy_cycles))
                .collect();
            assert_eq!(
                on.fns.len(),
                off.fns.len(),
                "{case}: fn count on={} off={}",
                on.fns.len(),
                off.fns.len()
            );
            for f_on in &on.fns {
                let off_proxy = off_by_key.get(f_on.key.as_str()).unwrap_or_else(|| {
                    panic!("{case}: fn {:?} present with BoundsElide on but missing off", f_on.key);
                });
                assert!(
                    f_on.proxy_cycles <= *off_proxy,
                    "{case} fn {:?}: BoundsElide-on proxy {} > off {}",
                    f_on.key,
                    f_on.proxy_cycles,
                    off_proxy
                );
            }
        }

        apply_mode(CompileMode::Release);
    }
}
