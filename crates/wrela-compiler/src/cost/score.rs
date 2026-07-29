//! Register scoreboard over a `CodegenProgram` (plans/M18.md item E,
//! freeze 1311). Differential proxy-cycles only — no cache, no mispredict,
//! no A76 ports.

use std::collections::BTreeMap;

use crate::codegen::{CodegenFn, CodegenProgram};

use super::owner::classify_owner;
use super::rule::{CostRule, EmittedWord};
use super::table::CostTable;

/// Per-fn scoreboard result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FnCost {
    pub key: String,
    /// Owner bucket: `app` / `runtime` / `driver` (item G).
    pub owner: String,
    pub proxy_cycles: u64,
    /// `CostRule::as_str()` → count of words with that rule.
    pub terms: BTreeMap<String, u64>,
}

/// Whole-program proxy-cycle report.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CostReport {
    pub version: u64,
    pub digest: String,
    /// Copied from `CostTable` for the dump/report header (item D).
    pub alu_ports: u64,
    pub mem_ports: u64,
    pub max_issue_per_cycle: u64,
    pub branch_penalty: u64,
    pub mem_reuse_window: u64,
    pub mem_working_set_cap: u64,
    /// Sum of per-fn schedule lengths (compositionality; dump header states it).
    pub total_proxy_cycles: u64,
    /// Sum of fn `proxy_cycles` per owner bucket (`app` / `runtime` / `driver`).
    pub owner_totals: BTreeMap<String, u64>,
    /// Stable order: `BTreeMap` key order of `program.fns`.
    pub fns: Vec<FnCost>,
}

/// Score every function with a dumb in-order register scoreboard.
///
/// Per fn: `ready[0..32] = 0`; for each word in order, issue at
/// `max(ready[srcs…], next_issue_slot)`, retire at `start + latency[rule]`,
/// update `ready[dst]`, advance issue slots by `issue_width`. Fn total =
/// max retire time (0 if empty). Program total = sum of fn totals.
pub fn score_program(program: &CodegenProgram, table: &CostTable) -> Result<CostReport, String> {
    let mut fns = Vec::with_capacity(program.fns.len());
    let mut total_proxy_cycles = 0u64;
    let mut owner_totals = BTreeMap::from([
        ("app".to_string(), 0u64),
        ("runtime".to_string(), 0u64),
        ("driver".to_string(), 0u64),
    ]);

    for (key, f) in &program.fns {
        let (proxy_cycles, terms) = score_fn(f, table)?;
        total_proxy_cycles = total_proxy_cycles.saturating_add(proxy_cycles);
        let owner = classify_owner(key).to_string();
        *owner_totals.entry(owner.clone()).or_insert(0) += proxy_cycles;
        fns.push(FnCost {
            key: key.clone(),
            owner,
            proxy_cycles,
            terms,
        });
    }

    Ok(CostReport {
        version: table.version,
        digest: table.table_digest(),
        // Header knobs only — schedule algorithm rewrite is item C.
        alu_ports: table.alu_ports,
        mem_ports: table.mem_ports,
        max_issue_per_cycle: table.max_issue_per_cycle,
        branch_penalty: table.branch_penalty,
        mem_reuse_window: table.mem_reuse_window,
        mem_working_set_cap: table.mem_working_set_cap,
        total_proxy_cycles,
        owner_totals,
        fns,
    })
}

fn score_fn(f: &CodegenFn, table: &CostTable) -> Result<(u64, BTreeMap<String, u64>), String> {
    let mut terms: BTreeMap<String, u64> = BTreeMap::new();
    if f.code.is_empty() {
        return Ok((0, terms));
    }

    let mut ready = [0u64; 32];
    let iw = table.max_issue_per_cycle.max(1);
    // Current issue cycle and how many slots already used in it.
    let mut cycle = 0u64;
    let mut slots_used = 0u64;
    let mut max_retire = 0u64;

    for ew in &f.code {
        *terms.entry(ew.rule.as_str().to_string()).or_insert(0) += 1;

        let data_ready = src_ready(ew, &ready);
        if data_ready > cycle {
            cycle = data_ready;
            slots_used = 0;
        }
        let start = cycle;
        let latency = latency_checked(table, ew.rule)?;
        let finish = start.saturating_add(latency);
        if let Some(d) = ew.dst {
            let i = d as usize;
            if i < 32 {
                ready[i] = finish;
            }
        }
        max_retire = max_retire.max(finish);

        slots_used += 1;
        if slots_used >= iw {
            cycle = cycle.saturating_add(1);
            slots_used = 0;
        }
    }

    Ok((max_retire, terms))
}

fn src_ready(ew: &EmittedWord, ready: &[u64; 32]) -> u64 {
    let mut t = 0u64;
    for &s in ew.src_slice() {
        let i = s as usize;
        if i < 32 {
            t = t.max(ready[i]);
        }
    }
    t
}

fn latency_checked(table: &CostTable, rule: CostRule) -> Result<u64, String> {
    // `CostTable::latency` panics on a missing key; after `parse`/`load_*`
    // every `CostRule::ALL` key is present. Still fail closed if somehow not.
    Ok(table.latency(rule))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cost::rule::CostRule;
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
    fn dependent_chain_longer_than_independent_pair() {
        let table = parse(TABLE).expect("table");
        // r1 = r0+r0; r2 = r1+r1 — true dep through r1.
        let dependent = prog(
            "dep",
            vec![
                word(CostRule::Alu, Some(1), &[0, 0]),
                word(CostRule::Alu, Some(2), &[1, 1]),
            ],
        );
        // r1 = r0+r0; r2 = r3+r3 — independent, same issue window.
        let independent = prog(
            "indep",
            vec![
                word(CostRule::Alu, Some(1), &[0, 0]),
                word(CostRule::Alu, Some(2), &[3, 3]),
            ],
        );
        let dep = score_program(&dependent, &table).expect("dep");
        let indep = score_program(&independent, &table).expect("indep");
        assert!(
            dep.total_proxy_cycles > indep.total_proxy_cycles,
            "dependent {} should exceed independent {}",
            dep.total_proxy_cycles,
            indep.total_proxy_cycles
        );
    }

    #[test]
    fn eliding_load_use_shrinks_total() {
        let table = parse(TABLE).expect("table");
        // load r1 ← [r0]; alu r2 = r1+r1 — load-use dependence.
        let with_load = prog(
            "f",
            vec![
                word(CostRule::Load, Some(1), &[0]),
                word(CostRule::Alu, Some(2), &[1, 1]),
            ],
        );
        // Two independent alus (load elided / not on the critical path).
        let without_load = prog(
            "f",
            vec![
                word(CostRule::Alu, Some(1), &[0, 0]),
                word(CostRule::Alu, Some(2), &[3, 3]),
            ],
        );
        let a = score_program(&with_load, &table).expect("with");
        let b = score_program(&without_load, &table).expect("without");
        assert!(
            a.total_proxy_cycles > b.total_proxy_cycles,
            "load-use {} should exceed elided {}",
            a.total_proxy_cycles,
            b.total_proxy_cycles
        );
    }

    #[test]
    fn empty_fn_is_zero() {
        let table = parse(TABLE).expect("table");
        let p = prog("empty", Vec::new());
        let r = score_program(&p, &table).expect("score");
        assert_eq!(r.total_proxy_cycles, 0);
        assert_eq!(r.fns.len(), 1);
        assert_eq!(r.fns[0].proxy_cycles, 0);
        assert!(r.fns[0].terms.is_empty());
    }

    #[test]
    fn score_sets_owner_from_classify() {
        let table = parse(TABLE).expect("table");
        let code = vec![word(CostRule::Alu, Some(1), &[0, 0])];
        let mut fns = BTreeMap::new();
        fns.insert(
            "checked_add".to_string(),
            CodegenFn {
                frame_size: 0,
                code: code.clone(),
                relocs: Vec::new(),
            },
        );
        fns.insert(
            "__wrela_abort".to_string(),
            CodegenFn {
                frame_size: 0,
                code: code.clone(),
                relocs: Vec::new(),
            },
        );
        fns.insert(
            "BlkDriver.on_turn".to_string(),
            CodegenFn {
                frame_size: 0,
                code,
                relocs: Vec::new(),
            },
        );
        let p = CodegenProgram {
            fns,
            rodata: Vec::new(),
        };
        let r = score_program(&p, &table).expect("score");
        let by_key: BTreeMap<_, _> = r.fns.iter().map(|f| (f.key.as_str(), f)).collect();
        assert_eq!(by_key["checked_add"].owner, "app");
        assert_eq!(by_key["__wrela_abort"].owner, "runtime");
        assert_eq!(by_key["BlkDriver.on_turn"].owner, "driver");
        assert_eq!(r.owner_totals["app"], by_key["checked_add"].proxy_cycles);
        assert_eq!(
            r.owner_totals["runtime"],
            by_key["__wrela_abort"].proxy_cycles
        );
        assert_eq!(
            r.owner_totals["driver"],
            by_key["BlkDriver.on_turn"].proxy_cycles
        );
    }
}
