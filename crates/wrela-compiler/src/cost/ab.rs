use std::cmp::Ordering;

use crate::codegen::CodegenProgram;
use crate::opts::CompileMode;
use crate::placement::PlacementTable;

use super::score::{CostReport, score_program};
use super::table::CostTable;

#[derive(Debug, Clone, Copy)]
pub struct CostOpts {
    pub mode: CompileMode,
}

impl Default for CostOpts {
    fn default() -> Self {
        Self {
            mode: CompileMode::Release,
        }
    }
}

pub fn score_with_opts(
    program: &CodegenProgram,
    table: &CostTable,
    placement: &PlacementTable,
    _opts: CostOpts,
) -> Result<CostReport, String> {
    score_program(program, table, placement)
}

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
        let a_prog = prog(
            "a",
            vec![
                word(CostRule::Alu, Some(1), &[0, 0]),
                word(CostRule::Alu, Some(2), &[3, 3]),
            ],
        );
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
