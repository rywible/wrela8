//! Dual-port register scoreboard over a `CodegenProgram` (cost hard-cut
//! item C; integrity item L block-split). Differential proxy-cycles only —
//! path-insensitive, no real cache geometry, no mispredict model beyond
//! `branch_penalty`.
//!
//! Per-fn flat total = Σ s(b) over mechanical basic blocks (`f≡1`).

use std::collections::{BTreeMap, VecDeque};

use crate::codegen::{CodegenFn, CodegenProgram};

use super::owner::classify_owner;
use super::rule::{CostRule, EmittedWord, MemClass, MemRef, MEM_SP_REG};
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
    /// Equals the `flat` workload row (`f≡1`).
    pub total_proxy_cycles: u64,
    /// Sum of fn `proxy_cycles` per owner bucket (`app` / `runtime` / `driver`).
    pub owner_totals: BTreeMap<String, u64>,
    /// Stable order: `BTreeMap` key order of `program.fns`.
    pub fns: Vec<FnCost>,
    /// `workloads.toml` digest when multi-W compose ran; `None` if not attached.
    pub workloads_digest: Option<String>,
    /// Per-workload proxy totals (`flat` always after compose; measured W when `f` available).
    pub workload_totals: BTreeMap<String, u64>,
    /// Measured-W coverage: name → (matched_hits, total_hits).
    pub workload_coverage: BTreeMap<String, (u64, u64)>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Port {
    Alu,
    Mem,
}

/// One slot in the mem reuse window: stores push for working-set only
/// (`grants_hit = false`); loads push hit-eligible entries.
#[derive(Clone, Copy)]
struct WinEntry {
    mem: MemRef,
    grants_hit: bool,
}

/// Score every function with a dual-port in-order scoreboard.
///
/// Per fn: split the word stream into mechanical basic blocks (branch
/// targets + fallthrough), score each block from a cold scoreboard, and
/// take `proxy_cycles = Σ s(b)` (`f≡1` flat). Within a block: issue when
/// GPR deps, NZCV (when `FlagEffect::Read`), and post-branch
/// `control_ready` allow, the rule's port is free, and
/// `issues_this_cycle < max_issue_per_cycle`; retire at `start + latency`.
/// Program total = sum of fn totals.
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
        alu_ports: table.alu_ports,
        mem_ports: table.mem_ports,
        max_issue_per_cycle: table.max_issue_per_cycle,
        branch_penalty: table.branch_penalty,
        mem_reuse_window: table.mem_reuse_window,
        mem_working_set_cap: table.mem_working_set_cap,
        total_proxy_cycles,
        owner_totals,
        fns,
        workloads_digest: None,
        workload_totals: BTreeMap::new(),
        workload_coverage: BTreeMap::new(),
    })
}

/// Half-open word-index ranges `[start, end)` of mechanical basic blocks
/// in a codegen stream (integrity item L). Leaders: index 0, every
/// PC-relative branch target, and the fallthrough word after any
/// `CostRule::Branch`.
pub fn basic_block_ranges(code: &[EmittedWord]) -> Vec<(usize, usize)> {
    let n = code.len();
    if n == 0 {
        return Vec::new();
    }
    let mut leader = vec![false; n];
    leader[0] = true;
    for (i, ew) in code.iter().enumerate() {
        if ew.rule != CostRule::Branch {
            continue;
        }
        if i + 1 < n {
            leader[i + 1] = true;
        }
        if let Some(t) = branch_target_index(ew.word, i) {
            if t < n {
                leader[t] = true;
            }
        }
    }
    let mut starts: Vec<usize> = (0..n).filter(|&i| leader[i]).collect();
    starts.sort_unstable();
    starts.dedup();
    let mut ranges = Vec::with_capacity(starts.len());
    for (k, &start) in starts.iter().enumerate() {
        let end = starts.get(k + 1).copied().unwrap_or(n);
        ranges.push((start, end));
    }
    ranges
}

/// Scoreboard schedule length `s(b)` for each basic block, in layout order.
pub fn block_schedule_lengths(code: &[EmittedWord], table: &CostTable) -> Result<Vec<u64>, String> {
    let mut out = Vec::new();
    for (start, end) in basic_block_ranges(code) {
        let (s, _) = score_words(&code[start..end], table)?;
        out.push(s);
    }
    Ok(out)
}

fn score_fn(f: &CodegenFn, table: &CostTable) -> Result<(u64, BTreeMap<String, u64>), String> {
    let mut terms: BTreeMap<String, u64> = BTreeMap::new();
    if f.code.is_empty() {
        return Ok((0, terms));
    }
    let mut proxy_cycles = 0u64;
    for (start, end) in basic_block_ranges(&f.code) {
        let (s, block_terms) = score_words(&f.code[start..end], table)?;
        proxy_cycles = proxy_cycles.saturating_add(s);
        for (k, v) in block_terms {
            *terms.entry(k).or_insert(0) += v;
        }
    }
    Ok((proxy_cycles, terms))
}

/// Dual-port in-order scoreboard over a straight-line word slice.
/// Returns `(schedule_length, term_counts)`.
fn score_words(
    code: &[EmittedWord],
    table: &CostTable,
) -> Result<(u64, BTreeMap<String, u64>), String> {
    let mut terms: BTreeMap<String, u64> = BTreeMap::new();
    if code.is_empty() {
        return Ok((0, terms));
    }

    let mut ready = [0u64; 32];
    let mut flags_ready = 0u64;
    let mut control_ready = 0u64;
    let mut cycle = 0u64;
    let mut alu_used = 0u64;
    let mut mem_used = 0u64;
    let mut issues = 0u64;
    let mut max_retire = 0u64;
    let mut window: VecDeque<WinEntry> = VecDeque::new();
    let win_cap = table.mem_reuse_window.max(1) as usize;

    for ew in code {
        *terms.entry(ew.rule.as_str().to_string()).or_insert(0) += 1;
        check_mem_base_in_srcs(ew)?;

        let mut data_ready = src_ready(ew, &ready);
        data_ready = data_ready.max(control_ready);
        if ew.flags.reads() {
            data_ready = data_ready.max(flags_ready);
        }
        let port = port_for(ew.rule);

        // Advance to a cycle where deps, port, and global issue cap allow.
        loop {
            if data_ready > cycle {
                cycle = data_ready;
                alu_used = 0;
                mem_used = 0;
                issues = 0;
            }
            if can_issue(port, alu_used, mem_used, issues, table) {
                break;
            }
            cycle = cycle.saturating_add(1);
            alu_used = 0;
            mem_used = 0;
            issues = 0;
        }

        let start = cycle;
        let latency = word_latency(ew, table, &mut window, win_cap)?;
        let finish = start.saturating_add(latency);
        if let Some(d) = ew.dst {
            let i = d as usize;
            if i < 32 {
                ready[i] = finish;
            }
        }
        if ew.flags.writes() {
            flags_ready = finish;
        }
        if ew.rule == CostRule::Branch {
            // Fallthrough waits on lat + branch_penalty (already in finish).
            control_ready = finish;
        }
        max_retire = max_retire.max(finish);

        match port {
            Port::Alu => alu_used = alu_used.saturating_add(1),
            Port::Mem => mem_used = mem_used.saturating_add(1),
        }
        issues = issues.saturating_add(1);

        // Call clobber and SP writes (epoch) kill reuse — stack offsets
        // after an SP adjust are a different epoch (integrity item C).
        if ew.rule == CostRule::Call || ew.dst == Some(MEM_SP_REG) {
            window.clear();
        }
    }

    Ok((max_retire, terms))
}

/// PC-relative branch target as a word index, or `None` for register
/// branches / undecodable encodings.
fn branch_target_index(word: u32, from: usize) -> Option<usize> {
    let word_delta = if word & 0xFC00_0000 == 0x1400_0000 {
        // B: op=0, `0001_01 imm26`
        sign_extend(word & 0x03FF_FFFF, 26)
    } else if word & 0xFF00_0000 == 0x5400_0000 {
        // B.cond: `0101_0100 imm19 0 cond`
        sign_extend((word >> 5) & 0x7FFFF, 19)
    } else if word & 0x7E00_0000 == 0x3400_0000 {
        // CBZ / CBNZ: `sf 0110_10 op imm19 Rt`
        sign_extend((word >> 5) & 0x7FFFF, 19)
    } else {
        return None;
    };
    let target = from as i64 + word_delta;
    if target < 0 {
        None
    } else {
        Some(target as usize)
    }
}

fn sign_extend(value: u32, bits: u32) -> i64 {
    let shift = 32 - bits;
    ((value << shift) as i32 >> shift) as i64
}

/// Non-unique MemRef must list its base in `srcs` (integrity item C).
fn check_mem_base_in_srcs(ew: &EmittedWord) -> Result<(), String> {
    let Some(m) = ew.mem else {
        return Ok(());
    };
    m.require_base_in_srcs(ew.src_slice())
}

fn port_for(rule: CostRule) -> Port {
    match rule {
        // Adrp is address materialization (ALU-class); loads/stores alone
        // take the mem port (integrity item B).
        CostRule::Load | CostRule::Store => Port::Mem,
        _ => Port::Alu,
    }
}

fn can_issue(port: Port, alu_used: u64, mem_used: u64, issues: u64, table: &CostTable) -> bool {
    let port_free = match port {
        Port::Alu => alu_used < table.alu_ports,
        Port::Mem => mem_used < table.mem_ports,
    };
    port_free && issues < table.max_issue_per_cycle
}

fn word_latency(
    ew: &EmittedWord,
    table: &CostTable,
    window: &mut VecDeque<WinEntry>,
    win_cap: usize,
) -> Result<u64, String> {
    let mut lat = match ew.rule {
        CostRule::Branch => table
            .latency(CostRule::Branch)
            .saturating_add(table.branch_penalty),
        CostRule::Load => load_latency(ew.mem, table, window),
        CostRule::Store => store_latency(ew.mem, table, window, win_cap),
        other => table.latency(other),
    };

    if matches!(ew.rule, CostRule::Load) {
        if let Some(m) = ew.mem {
            push_window(
                window,
                win_cap,
                WinEntry {
                    mem: m,
                    grants_hit: true,
                },
            );
            lat = lat.saturating_add(working_set_surcharge(window, table));
        }
        // Missing MemRef: cold miss already charged; no window push.
    }

    if matches!(ew.rule, CostRule::Store) {
        // store_latency already invalidated + pushed; surcharge after.
        if ew.mem.is_some() {
            lat = lat.saturating_add(working_set_surcharge(window, table));
        }
    }

    Ok(lat)
}

fn load_latency(mem: Option<MemRef>, table: &CostTable, window: &VecDeque<WinEntry>) -> u64 {
    let Some(m) = mem else {
        return table.mem.load_cold_miss;
    };
    let hit = window.iter().any(|e| e.grants_hit && e.mem == m);
    match (m.class, hit) {
        (MemClass::Stack, true) => table.mem.load_stack_hit,
        (MemClass::Stack, false) => table.mem.load_stack_miss,
        (MemClass::Cold, true) => table.mem.load_cold_hit,
        (MemClass::Cold, false) => table.mem.load_cold_miss,
    }
}

fn store_latency(
    mem: Option<MemRef>,
    table: &CostTable,
    window: &mut VecDeque<WinEntry>,
    win_cap: usize,
) -> u64 {
    let Some(m) = mem else {
        return table.mem.store_cold;
    };
    // Invalidate all prior entries for this key, then push (WS only).
    window.retain(|e| e.mem != m);
    let lat = match m.class {
        MemClass::Stack => table.mem.store_stack,
        MemClass::Cold => table.mem.store_cold,
    };
    push_window(
        window,
        win_cap,
        WinEntry {
            mem: m,
            grants_hit: false,
        },
    );
    lat
}

fn push_window(window: &mut VecDeque<WinEntry>, win_cap: usize, entry: WinEntry) {
    window.push_back(entry);
    while window.len() > win_cap {
        window.pop_front();
    }
}

fn working_set_surcharge(window: &VecDeque<WinEntry>, table: &CostTable) -> u64 {
    let mut keys: Vec<(u8, u64)> = window
        .iter()
        .map(|e| {
            let class = match e.mem.class {
                MemClass::Stack => 0u8,
                MemClass::Cold => 1u8,
            };
            (class, e.mem.key)
        })
        .collect();
    keys.sort_unstable();
    keys.dedup();
    if (keys.len() as u64) > table.mem_working_set_cap {
        table.mem.working_set_surcharge
    } else {
        0
    }
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cost::rule::{CostRule, FlagEffect, MemRef, MEM_SP_REG};
    use crate::cost::table::parse;

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

    fn word_flags(rule: CostRule, dst: Option<u8>, srcs: &[u8], flags: FlagEffect) -> EmittedWord {
        word(rule, dst, srcs).with_flags(flags)
    }

    fn load_stack(dst: u8, offset: u64) -> EmittedWord {
        word(CostRule::Load, Some(dst), &[31]).with_mem(MemRef::stack(offset))
    }

    fn store_stack(offset: u64) -> EmittedWord {
        word(CostRule::Store, None, &[31, 0]).with_mem(MemRef::stack(offset))
    }

    fn load_cold_unique(dst: u8, seq: u64) -> EmittedWord {
        word(CostRule::Load, Some(dst), &[0]).with_mem(MemRef::cold_unique(seq))
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
        let dependent = prog(
            "dep",
            vec![
                word(CostRule::Alu, Some(1), &[0, 0]),
                word(CostRule::Alu, Some(2), &[1, 1]),
            ],
        );
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
        let with_load = prog(
            "f",
            vec![load_stack(1, 0), word(CostRule::Alu, Some(2), &[1, 1])],
        );
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

    #[test]
    fn report_copies_port_knobs_from_table() {
        let table = parse(TABLE).expect("table");
        let r = score_program(
            &prog("f", vec![word(CostRule::Alu, Some(1), &[0, 0])]),
            &table,
        )
        .expect("score");
        assert_eq!(r.version, 2);
        assert_eq!(r.alu_ports, 2);
        assert_eq!(r.mem_ports, 2);
        assert_eq!(r.max_issue_per_cycle, 2);
        assert_eq!(r.branch_penalty, 3);
        assert_eq!(r.mem_reuse_window, 8);
        assert_eq!(r.mem_working_set_cap, 4);
        assert_eq!(r.digest, table.table_digest());
    }

    /// max_issue=2 caps alu+mem: three independent mem ops need a second cycle.
    #[test]
    fn max_issue_caps_mem_dual_issue() {
        let table = parse(TABLE).expect("table");
        // Three independent cold loads: mem_ports=2 and max_issue=2 →
        // two issue at cycle 0, third at cycle 1. Retire = 1+12 = 13.
        let three = prog(
            "f",
            vec![
                load_cold_unique(1, 0),
                load_cold_unique(2, 1),
                load_cold_unique(3, 2),
            ],
        );
        let r = score_program(&three, &table).expect("score");
        assert_eq!(r.total_proxy_cycles, 13);
        // Two loads finish in one cycle: retire = 12.
        let two = prog("f", vec![load_cold_unique(1, 0), load_cold_unique(2, 1)]);
        let r2 = score_program(&two, &table).expect("score");
        assert_eq!(r2.total_proxy_cycles, 12);
        assert!(r.total_proxy_cycles > r2.total_proxy_cycles);
    }

    #[test]
    fn alu_and_mem_can_dual_issue_under_cap() {
        let table = parse(TABLE).expect("table");
        // Independent alu + cold load: both cycle 0; retire = max(1, 12) = 12.
        let mixed = prog(
            "f",
            vec![
                word(CostRule::Alu, Some(1), &[0, 0]),
                load_cold_unique(2, 0),
            ],
        );
        let r = score_program(&mixed, &table).expect("score");
        assert_eq!(r.total_proxy_cycles, 12);
    }

    #[test]
    fn branch_pays_penalty() {
        let table = parse(TABLE).expect("table");
        let p = prog("f", vec![word(CostRule::Branch, None, &[])]);
        let r = score_program(&p, &table).expect("score");
        // latency[branch]=1 + branch_penalty=3
        assert_eq!(r.total_proxy_cycles, 4);
    }

    #[test]
    fn abort_val_uses_table_latency() {
        let table = parse(TABLE).expect("table");
        let p = prog("f", vec![word(CostRule::AbortVal, None, &[0])]);
        let r = score_program(&p, &table).expect("score");
        assert_eq!(r.total_proxy_cycles, 3);
    }

    #[test]
    fn stack_hit_cheaper_than_miss() {
        let table = parse(TABLE).expect("table");
        // Chain through alu so the second load's finish is on the critical path.
        let hit = prog(
            "f",
            vec![
                load_stack(1, 8),
                word(CostRule::Alu, Some(1), &[1, 1]),
                load_stack(2, 8),
                word(CostRule::Alu, Some(3), &[2, 2]),
            ],
        );
        let miss = prog(
            "f",
            vec![
                load_stack(1, 8),
                word(CostRule::Alu, Some(1), &[1, 1]),
                load_stack(2, 16),
                word(CostRule::Alu, Some(3), &[2, 2]),
            ],
        );
        let a = score_program(&hit, &table).expect("hit");
        let b = score_program(&miss, &table).expect("miss");
        assert!(
            a.total_proxy_cycles < b.total_proxy_cycles,
            "stack hit schedule {} should beat miss {}",
            a.total_proxy_cycles,
            b.total_proxy_cycles
        );
    }

    #[test]
    fn store_kills_stack_hit() {
        let table = parse(TABLE).expect("table");
        // load → load same key hits; load → store → load same key misses.
        let reuse = prog(
            "f",
            vec![
                load_stack(1, 8),
                word(CostRule::Alu, Some(1), &[1, 1]),
                load_stack(2, 8),
                word(CostRule::Alu, Some(3), &[2, 2]),
            ],
        );
        let after_store = prog(
            "f",
            vec![
                load_stack(1, 8),
                word(CostRule::Alu, Some(1), &[1, 1]),
                store_stack(8),
                load_stack(2, 8),
                word(CostRule::Alu, Some(3), &[2, 2]),
            ],
        );
        let a = score_program(&reuse, &table).expect("reuse");
        let b = score_program(&after_store, &table).expect("store");
        assert!(
            b.total_proxy_cycles > a.total_proxy_cycles,
            "store-invalidated reload {} should exceed hit reload {}",
            b.total_proxy_cycles,
            a.total_proxy_cycles
        );
    }

    #[test]
    fn call_clears_mem_reuse_window() {
        let table = parse(TABLE).expect("table");
        let hit = prog(
            "f",
            vec![
                load_stack(1, 8),
                word(CostRule::Alu, Some(1), &[1, 1]),
                load_stack(2, 8),
                word(CostRule::Alu, Some(3), &[2, 2]),
            ],
        );
        let after_call = prog(
            "f",
            vec![
                load_stack(1, 8),
                word(CostRule::Alu, Some(1), &[1, 1]),
                word(CostRule::Call, Some(0), &[]),
                load_stack(2, 8),
                word(CostRule::Alu, Some(3), &[2, 2]),
            ],
        );
        let a = score_program(&hit, &table).expect("hit");
        let b = score_program(&after_call, &table).expect("call");
        assert!(
            b.total_proxy_cycles > a.total_proxy_cycles,
            "call-cleared reload {} should exceed hit {}",
            b.total_proxy_cycles,
            a.total_proxy_cycles
        );
    }

    #[test]
    fn cold_unique_always_misses() {
        let table = parse(TABLE).expect("table");
        // Distinct cold uniques never hit each other — each pays load_cold_miss.
        let two = prog(
            "f",
            vec![
                load_cold_unique(1, 0),
                word(CostRule::Alu, Some(1), &[1, 1]),
                load_cold_unique(2, 1),
                word(CostRule::Alu, Some(3), &[2, 2]),
            ],
        );
        let r = score_program(&two, &table).expect("score");
        // load@0→12, alu@12→13 dual with 2nd load@12→24, alu@24→25.
        assert_eq!(r.total_proxy_cycles, 25);
        // Contrast: repeating the same stack key would finish earlier on the
        // second load (hit=1 vs miss=12).
        let stack_hit = prog(
            "f",
            vec![
                load_stack(1, 8),
                word(CostRule::Alu, Some(1), &[1, 1]),
                load_stack(2, 8),
                word(CostRule::Alu, Some(3), &[2, 2]),
            ],
        );
        let h = score_program(&stack_hit, &table).expect("hit");
        assert!(
            r.total_proxy_cycles > h.total_proxy_cycles,
            "cold unique miss {} should exceed stack hit {}",
            r.total_proxy_cycles,
            h.total_proxy_cycles
        );
    }

    #[test]
    fn missing_memref_is_cold_miss() {
        let table = parse(TABLE).expect("table");
        let untagged = prog(
            "f",
            vec![word(CostRule::Load, Some(1), &[0])], // no MemRef
        );
        let r = score_program(&untagged, &table).expect("score");
        assert_eq!(r.total_proxy_cycles, 12); // load_cold_miss
    }

    #[test]
    fn working_set_surcharge_when_distinct_over_cap() {
        let table = parse(TABLE).expect("table");
        // cap=4: five distinct stack keys → surcharge on the 5th.
        let over = prog(
            "f",
            vec![
                load_stack(1, 0),
                load_stack(2, 8),
                load_stack(3, 16),
                load_stack(4, 24),
                // 5th: distinct; dual-issue with 4th at cycle 1 after first pair.
                load_stack(5, 32),
            ],
        );
        let under = prog(
            "f",
            vec![
                load_stack(1, 0),
                load_stack(2, 8),
                load_stack(3, 16),
                load_stack(4, 24),
                load_stack(5, 0), // reuse key 0 — still 4 distinct, no surcharge
            ],
        );
        let a = score_program(&over, &table).expect("over");
        let b = score_program(&under, &table).expect("under");
        assert!(
            a.total_proxy_cycles > b.total_proxy_cycles,
            "surcharge schedule {} should exceed under-cap {}",
            a.total_proxy_cycles,
            b.total_proxy_cycles
        );
    }

    /// Call clobber: consumer of x0 waits on call retire (dst=Some(0)).
    #[test]
    fn call_result_waits_on_call_retire() {
        let table = parse(TABLE).expect("table");
        let with_dep = prog(
            "f",
            vec![
                word(CostRule::Call, Some(0), &[]),
                word(CostRule::Alu, Some(1), &[0, 0]),
            ],
        );
        let r = score_program(&with_dep, &table).expect("score");
        // call@0 → finish 4 (x0 ready); alu use x0 @4 → 5.
        assert_eq!(r.total_proxy_cycles, 5);
        // Independent alu dual-issues with call; max retire is call's 4.
        let no_dep = prog(
            "f",
            vec![
                word(CostRule::Call, Some(0), &[]),
                word(CostRule::Alu, Some(1), &[2, 2]),
            ],
        );
        let b = score_program(&no_dep, &table).expect("score");
        assert_eq!(b.total_proxy_cycles, 4);
        assert!(r.total_proxy_cycles > b.total_proxy_cycles);
    }

    /// cmp (Write flags) → b.cond (Read flags) must serialize on NZCV.
    #[test]
    fn cmp_to_bcond_waits_on_flags() {
        let table = parse(TABLE).expect("table");
        let ordered = prog(
            "f",
            vec![
                word_flags(CostRule::Alu, None, &[1, 2], FlagEffect::Write),
                word_flags(CostRule::Branch, None, &[], FlagEffect::Read),
            ],
        );
        let r = score_program(&ordered, &table).expect("score");
        // cmp@0→1; branch waits on flags@1, lat+penalty → finish 1+4=5.
        assert_eq!(r.total_proxy_cycles, 5);
        // Without flag edge, branch issues at 0 (dual with cmp under max_issue=2)
        // and finishes at 4 — shorter than ordered.
        let unordered = prog(
            "f",
            vec![
                word(CostRule::Alu, None, &[1, 2]),
                word(CostRule::Branch, None, &[]),
            ],
        );
        let b = score_program(&unordered, &table).expect("score");
        assert_eq!(b.total_proxy_cycles, 4);
        assert!(r.total_proxy_cycles > b.total_proxy_cycles);
    }

    /// Mid-stream branch: followers wait on control-ready (lat+penalty).
    #[test]
    fn mid_stream_branch_delays_followers() {
        let table = parse(TABLE).expect("table");
        let mid = prog(
            "f",
            vec![
                word(CostRule::Branch, None, &[]),
                word(CostRule::Alu, Some(1), &[2, 2]),
            ],
        );
        let r = score_program(&mid, &table).expect("score");
        // branch@0 → finish 4 (= control_ready); alu issues at 4 → 5.
        assert_eq!(r.total_proxy_cycles, 5);
        // Trailing branch alone is still 4 (penalty only on max_retire).
        let alone = prog("f", vec![word(CostRule::Branch, None, &[])]);
        let a = score_program(&alone, &table).expect("score");
        assert_eq!(a.total_proxy_cycles, 4);
        assert!(r.total_proxy_cycles > a.total_proxy_cycles);
    }

    /// Adrp is ALU-port: dual-issues with an independent load under the cap.
    #[test]
    fn adrp_dual_issues_with_load() {
        let table = parse(TABLE).expect("table");
        let mixed = prog(
            "f",
            vec![word(CostRule::Adrp, Some(1), &[]), load_cold_unique(2, 0)],
        );
        let r = score_program(&mixed, &table).expect("score");
        // Both issue cycle 0 (alu+mem); retire = max(1, 12) = 12.
        assert_eq!(r.total_proxy_cycles, 12);
        // Two Adrps alone: alu_ports=2, max_issue=2 → both cycle 0, retire 1.
        let two_adrp = prog(
            "f",
            vec![
                word(CostRule::Adrp, Some(1), &[]),
                word(CostRule::Adrp, Some(2), &[]),
            ],
        );
        let a = score_program(&two_adrp, &table).expect("score");
        assert_eq!(a.total_proxy_cycles, 1);
    }

    /// SP write clears the mem reuse window (epoch), like Call.
    #[test]
    fn sp_dst_clears_mem_reuse_window() {
        let table = parse(TABLE).expect("table");
        let hit = prog(
            "f",
            vec![
                load_stack(1, 8),
                word(CostRule::Alu, Some(1), &[1, 1]),
                load_stack(2, 8),
                word(CostRule::Alu, Some(3), &[2, 2]),
            ],
        );
        let after_sp = prog(
            "f",
            vec![
                load_stack(1, 8),
                word(CostRule::Alu, Some(1), &[1, 1]),
                // SP adjust: dst=31 clears reuse window.
                word(CostRule::Alu, Some(MEM_SP_REG), &[MEM_SP_REG]),
                load_stack(2, 8),
                word(CostRule::Alu, Some(3), &[2, 2]),
            ],
        );
        let a = score_program(&hit, &table).expect("hit");
        let b = score_program(&after_sp, &table).expect("sp");
        assert!(
            b.total_proxy_cycles > a.total_proxy_cycles,
            "sp-cleared reload {} should exceed hit {}",
            b.total_proxy_cycles,
            a.total_proxy_cycles
        );
    }

    /// Store keeps other keys hittable; SP adjust then reload misses.
    #[test]
    fn store_then_sp_adjust_reload_misses() {
        let table = parse(TABLE).expect("table");
        // load key 8 → store other key → reload key 8 still hits.
        let after_store = prog(
            "f",
            vec![
                load_stack(1, 8),
                word(CostRule::Alu, Some(1), &[1, 1]),
                store_stack(16),
                load_stack(2, 8),
                word(CostRule::Alu, Some(3), &[2, 2]),
            ],
        );
        // Same, but SP adjust between store and reload clears the epoch.
        let after_sp = prog(
            "f",
            vec![
                load_stack(1, 8),
                word(CostRule::Alu, Some(1), &[1, 1]),
                store_stack(16),
                word(CostRule::Alu, Some(MEM_SP_REG), &[MEM_SP_REG]),
                load_stack(2, 8),
                word(CostRule::Alu, Some(3), &[2, 2]),
            ],
        );
        let a = score_program(&after_store, &table).expect("store");
        let b = score_program(&after_sp, &table).expect("sp");
        assert!(
            b.total_proxy_cycles > a.total_proxy_cycles,
            "store→sp→reload miss {} should exceed store-only hit {}",
            b.total_proxy_cycles,
            a.total_proxy_cycles
        );
    }

    /// Non-unique MemRef without base∈srcs fails closed.
    #[test]
    fn memref_base_not_in_srcs_fails_closed() {
        let table = parse(TABLE).expect("table");
        // Stack MemRef but srcs omit SP (31).
        let bad_stack = prog(
            "f",
            vec![word(CostRule::Load, Some(1), &[0]).with_mem(MemRef::stack(8))],
        );
        let err = score_program(&bad_stack, &table).expect_err("stack base");
        assert!(err.contains("base register"), "unexpected err: {err}");
        // Cold stable MemRef but srcs omit packed base 28.
        let bad_cold = prog(
            "f",
            vec![word(CostRule::Load, Some(1), &[0]).with_mem(MemRef::cold_stable(28, 16))],
        );
        let err = score_program(&bad_cold, &table).expect_err("cold base");
        assert!(err.contains("base register"), "unexpected err: {err}");
        // Cold unique: no base requirement — still scores.
        let unique = prog("f", vec![load_cold_unique(1, 0)]);
        assert!(score_program(&unique, &table).is_ok());
    }

    fn word_enc(enc: u32, rule: CostRule, dst: Option<u8>, srcs: &[u8]) -> EmittedWord {
        EmittedWord::new(enc, String::new(), rule, dst, srcs)
    }

    /// Leaders at 0, fallthrough after branch, and PC-relative targets.
    #[test]
    fn basic_blocks_split_on_branch_targets_and_fallthrough() {
        use crate::encode::{enc_b, enc_b_cond, Cond};
        // Layout: 0:alu  1:b.eq ->4  2:alu  3:b ->5  4:alu  5:alu
        let code = vec![
            word_enc(0, CostRule::Alu, Some(1), &[0]),
            word_enc(enc_b_cond(Cond::Eq, 12), CostRule::Branch, None, &[])
                .with_flags(FlagEffect::Read), // +3 words → idx 4
            word_enc(0, CostRule::Alu, Some(2), &[1]),
            word_enc(enc_b(8), CostRule::Branch, None, &[]), // +2 words → idx 5
            word_enc(0, CostRule::Alu, Some(3), &[0]),
            word_enc(0, CostRule::Alu, Some(4), &[0]),
        ];
        let ranges = basic_block_ranges(&code);
        assert_eq!(ranges, vec![(0, 2), (2, 4), (4, 5), (5, 6)]);
    }

    /// Under f≡1, Σ s(b) equals the per-fn schedule `score_fn` / dump flat uses.
    #[test]
    fn flat_equiv_block_sum_matches_fn_schedule() {
        use crate::encode::{enc_b, enc_b_cond, Cond};
        let table = parse(TABLE).expect("table");

        let cases: Vec<(&str, Vec<EmittedWord>)> = vec![
            ("empty", Vec::new()),
            (
                "straight",
                vec![
                    word(CostRule::Alu, Some(1), &[0, 0]),
                    word(CostRule::Alu, Some(2), &[1, 1]),
                    load_stack(3, 8),
                ],
            ),
            (
                "mid_branch",
                vec![
                    word(CostRule::Branch, None, &[]),
                    word(CostRule::Alu, Some(1), &[2, 2]),
                ],
            ),
            (
                "diamond",
                vec![
                    word_flags(CostRule::Alu, None, &[1, 2], FlagEffect::Write),
                    word_enc(enc_b_cond(Cond::Eq, 12), CostRule::Branch, None, &[])
                        .with_flags(FlagEffect::Read),
                    word_enc(0, CostRule::Alu, Some(3), &[0]),
                    word_enc(enc_b(8), CostRule::Branch, None, &[]),
                    word_enc(0, CostRule::Alu, Some(4), &[0]),
                    word_enc(0, CostRule::Alu, Some(5), &[0]),
                ],
            ),
        ];

        for (name, code) in cases {
            let p = prog("f", code.clone());
            let report = score_program(&p, &table).unwrap_or_else(|e| panic!("{name}: {e}"));
            let fn_sched = report.fns[0].proxy_cycles;
            let blocks = block_schedule_lengths(&code, &table)
                .unwrap_or_else(|e| panic!("{name} blocks: {e}"));
            let sum: u64 = blocks.iter().sum();
            assert_eq!(
                sum, fn_sched,
                "{name}: Σ s(b)={sum} (blocks {blocks:?}) != fn schedule {fn_sched}"
            );
            assert_eq!(report.total_proxy_cycles, fn_sched);
        }
    }
}
