//! Multi-W cost compose (integrity Item J; block grain in plans/M20.md
//! item C): flat row + measured `Σ f×s` at **method** or **block** grain.
//!
//! - `flat` = today's program total (`f≡1`, sum of per-fn schedules).
//! - Named W with a `lane1-freq.txt` vector: `Σ_fn f(fn) × s(fn)`.
//! - Named W with a `lane2-freq.txt` vector: `Σ_b f(b) × s(b)`, where a
//!   sidecar key's `s` is the sum of `s(b)` over the emitted-word blocks
//!   its Lane 2 span covers (`cost::bridge`). 04 §5 makes this the
//!   requirement: a method-grain sum prices a hot loop body the same as the
//!   function containing it.
//! - Pure cost-* cases without an `f` vector emit the flat row alone.
//!
//! Both grains share the coverage rule unchanged (04 §5): an uncovered
//! measured hit is **charged** at `uncovered_charge`, never dropped, and
//! freeze 1627 keeps the denominator the whole scored set rather than the
//! instrumented or resolvable subset.

use std::collections::BTreeMap;
use std::path::Path;

use super::bridge::{BlockBridge, Resolved};
use super::freq::{self, BlockFreq, MethodFreq};
use super::score::{CostReport, FnCost};
use super::workload::{self, FLAT_NAME, WorkloadSet};

/// Workloads.toml set plus optional measured frequency vectors.
#[derive(Debug, Clone)]
pub struct WorkloadAttach {
    pub set: WorkloadSet,
    /// Workload name → method-grain counts (`lane1-freq.txt`).
    pub frequencies: BTreeMap<String, BTreeMap<String, u64>>,
    /// Workload name → block-grain counts (`lane2-freq.txt`), keyed
    /// `<fn_key>#<block_index>`.
    pub block_frequencies: BTreeMap<String, BTreeMap<String, u64>>,
    /// The checked MWIR ↔ emitted-word correspondence for the scored
    /// program. Required whenever `block_frequencies` is non-empty —
    /// compose fails closed rather than silently falling back to method
    /// grain or dropping the row.
    pub bridge: Option<BlockBridge>,
}

impl WorkloadAttach {
    /// Load `bench/workloads.toml` and any sidecar next to `source`:
    /// `lane1-freq.txt` (method grain) and/or `lane2-freq.txt` (block
    /// grain, which additionally needs the bridge for `program`).
    ///
    /// The bridge is built from the spans the current thread's most recent
    /// bridge-mode codegen recorded (`codegen::set_block_bridge`), so the
    /// caller must have emitted `program` under bridge mode. If it did not,
    /// a `lane2-freq.txt` case fails closed on an empty bridge rather than
    /// pricing blocks against a program nobody checked.
    pub fn load_default_for(
        source: Option<&Path>,
        program: &crate::codegen::CodegenProgram,
        table: &super::table::CostTable,
        placement: &crate::placement::PlacementTable,
    ) -> Result<Self, String> {
        let set = workload::load_default()?;
        let mut frequencies = BTreeMap::new();
        let mut block_frequencies = BTreeMap::new();
        let mut bridge = None;
        if let Some(path) = source {
            if let Some(freq_path) = freq::sibling_freq_path(path) {
                let f = freq::load_from_path(&freq_path)?;
                if set.weight(&f.workload).is_none() {
                    return Err(format!(
                        "freq: workload `{}` not in workloads.toml",
                        f.workload
                    ));
                }
                frequencies.insert(f.workload, f.counts);
            }
            if let Some(freq_path) = freq::sibling_block_freq_path(path) {
                let f = freq::load_block_from_path(&freq_path)?;
                if set.weight(&f.workload).is_none() {
                    return Err(format!(
                        "block freq: workload `{}` not in workloads.toml",
                        f.workload
                    ));
                }
                bridge = Some(BlockBridge::from_current_codegen(
                    program, table, placement,
                )?);
                block_frequencies.insert(f.workload, f.counts);
            }
        }
        Ok(Self {
            set,
            frequencies,
            block_frequencies,
            bridge,
        })
    }

    /// Test helper: set + one method-grain vector.
    pub fn from_parts(set: WorkloadSet, freq: MethodFreq) -> Self {
        let mut frequencies = BTreeMap::new();
        frequencies.insert(freq.workload, freq.counts);
        Self {
            set,
            frequencies,
            block_frequencies: BTreeMap::new(),
            bridge: None,
        }
    }

    /// Test helper: set + one block-grain vector and its bridge.
    pub fn from_block_parts(set: WorkloadSet, freq: BlockFreq, bridge: BlockBridge) -> Self {
        let mut block_frequencies = BTreeMap::new();
        block_frequencies.insert(freq.workload, freq.counts);
        Self {
            set,
            frequencies: BTreeMap::new(),
            block_frequencies,
            bridge: Some(bridge),
        }
    }

    /// Which grain priced workload `name` — the dump prints this so the
    /// both-sidecars rule (see `attach_workloads`) is never silent.
    /// `None` for `flat` and for any name this attach did not measure.
    pub fn grain_of(&self, name: &str) -> Option<&'static str> {
        if self.block_frequencies.contains_key(name) {
            Some("block")
        } else if self.frequencies.contains_key(name) {
            Some("method")
        } else {
            None
        }
    }
}

/// Attach multi-W rows onto a scored report (mutates in place).
///
/// Always writes `flat = total_proxy_cycles`. For each measured frequency
/// whose name is in `attach.set`, writes `Σ f×s` and coverage
/// `(matched_hits, total_hits)`.
///
/// **Grain selection.** A workload with a block-grain vector composes at
/// block grain; one with only a method-grain vector composes at method
/// grain. When a case commits **both** sidecars naming the same workload —
/// which `boot-actors` does — **block grain wins** and the method-grain
/// vector for that name is ignored: 04 §5 makes block grain the requirement
/// and a Lane 2 vector is strictly more information about the same run. The
/// choice is not silent: the dump prints `grain=block` / `grain=method` on
/// the row, so which vector priced a workload is on the review surface.
///
/// `Err` when a block-grain vector is present without a bridge, or when the
/// bridge rejects one of its keys (decision 1608 — never attribute by
/// nearest offset).
pub fn attach_workloads(report: &mut CostReport, attach: &WorkloadAttach) -> Result<(), String> {
    report.workloads_digest = Some(attach.set.digest());
    report.workload_totals.clear();
    report.workload_coverage.clear();

    report
        .workload_totals
        .insert(FLAT_NAME.to_string(), report.total_proxy_cycles);

    for (name, counts) in &attach.frequencies {
        if attach.set.weight(name).is_none() {
            continue;
        }
        if attach.block_frequencies.contains_key(name) {
            // Block grain wins (doc comment above).
            continue;
        }
        let (cycles, matched, total) = method_grain_fxs(&report.fns, counts);
        report.workload_totals.insert(name.clone(), cycles);
        report
            .workload_coverage
            .insert(name.clone(), (matched, total));
    }

    for (name, counts) in &attach.block_frequencies {
        if attach.set.weight(name).is_none() {
            continue;
        }
        let bridge = attach.bridge.as_ref().ok_or_else(|| {
            format!(
                "compose: workload `{name}` has a block-grain `f` vector but no block bridge — \
                 the scored program must be emitted under `codegen::set_block_bridge` (decision \
                 1608: the bridge is proved, not assumed)"
            )
        })?;
        let m = block_grain_fxs(&report.fns, bridge, counts)?;
        report.workload_totals.insert(name.clone(), m.cycles);
        report
            .workload_coverage
            .insert(name.clone(), (m.matched, m.total));
    }
    Ok(())
}

/// What one block-grain compose measured, reported honestly rather than
/// collapsed to a total (plans/M20.md item C's coverage finding).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct BlockGrainMeasure {
    /// `Σ_b f(b) × s(b)`, uncovered keys charged at `uncovered_charge`.
    pub cycles: u64,
    /// Hit mass whose key resolved to a scored block.
    pub matched: u64,
    /// Total hit mass in the sidecar.
    pub total: u64,
    /// Sidecar keys that resolved to a scored block.
    pub resolved_keys: u64,
    /// Sidecar keys naming a fn the scored closure does not contain.
    pub unresolved_keys: u64,
    /// Of `cycles`, the part contributed by uncovered keys.
    pub uncovered_cycles: u64,
}

/// `Σ_b f(b)×s(b)` at block grain. A sidecar key's `s` is the sum of
/// `s(b)` over the emitted-word blocks its Lane 2 span covers.
///
/// **Uncovered hits are charged, not dropped** — identically to the
/// method-grain path, and for the identical reason (`uncovered_charge`'s
/// own doc): a key that vanishes from the scored set must cost, or the
/// ruler rewards measuring less. Freeze 1627: the coverage denominator is
/// the whole scored set, never the resolvable subset, so a key naming a fn
/// only the test image contains lands in `unresolved_keys` and is charged.
pub fn block_grain_fxs(
    fns: &[FnCost],
    bridge: &BlockBridge,
    freq: &BTreeMap<String, u64>,
) -> Result<BlockGrainMeasure, String> {
    let charge = uncovered_charge(fns);
    let mut m = BlockGrainMeasure::default();
    for (key, &f) in freq {
        m.total = m.total.saturating_add(f);
        match bridge.lookup(key)? {
            Resolved::Block(b) => {
                m.cycles = m.cycles.saturating_add(f.saturating_mul(b.cycles));
                m.matched = m.matched.saturating_add(f);
                m.resolved_keys += 1;
            }
            Resolved::UnknownFn => {
                let c = f.saturating_mul(charge);
                m.cycles = m.cycles.saturating_add(c);
                m.uncovered_cycles = m.uncovered_cycles.saturating_add(c);
                m.unresolved_keys += 1;
            }
        }
    }
    Ok(m)
}

/// `Σ f(fn)×s(fn)` over method keys present in both `freq` and scored fns.
/// Coverage: matched hit sum / total hit sum in `freq`.
///
/// **Uncovered hits are charged, not dropped.** A measured key with no
/// scored fn is priced at `uncovered_charge` — the maximum per-fn schedule
/// in the program (see `uncovered_charge`). Contributing 0 would make any
/// transform that removes a method key from the scored set (rename,
/// outline, specialize-and-clone, or the handler fusion 04 §5's actor
/// as-if explicitly permits) delete that key's whole `f×s` mass and read
/// as a strict improvement — a proxy win bought by measuring less. 04 §5's
/// soundness rule says prefer over-cost when unsure, so the unknown is
/// priced at the most expensive thing in the program.
pub fn method_grain_fxs(
    fns: &[FnCost],
    freq: &BTreeMap<String, u64>,
) -> (
    u64, /* cycles */
    u64, /* matched */
    u64, /* total */
) {
    let mut by_key: BTreeMap<&str, u64> = BTreeMap::new();
    for f in fns {
        by_key.insert(f.key.as_str(), f.proxy_cycles);
    }
    let charge = uncovered_charge(fns);
    let mut cycles = 0u64;
    let mut matched = 0u64;
    let mut total = 0u64;
    for (key, &f) in freq {
        total = total.saturating_add(f);
        match by_key.get(key.as_str()) {
            Some(&s) => {
                cycles = cycles.saturating_add(f.saturating_mul(s));
                matched = matched.saturating_add(f);
            }
            None => cycles = cycles.saturating_add(f.saturating_mul(charge)),
        }
    }
    (cycles, matched, total)
}

/// Over-cost floor for a measured hit with no scored fn: the maximum
/// per-fn schedule length in the program (0 for an empty program — there
/// is nothing to charge). Deliberately pessimistic and self-contained:
/// no cross-run state, and losing coverage can never look cheap.
pub fn uncovered_charge(fns: &[FnCost]) -> u64 {
    fns.iter().map(|f| f.proxy_cycles).max().unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cost::freq::parse as parse_freq;
    use crate::cost::score::FnCost;
    use crate::cost::workload::parse as parse_workloads;

    fn fn_cost(key: &str, cycles: u64) -> FnCost {
        FnCost {
            key: key.to_string(),
            owner: "app".to_string(),
            proxy_cycles: cycles,
            words: cycles,
            terms: BTreeMap::new(),
        }
    }

    fn bare_report(fns: Vec<FnCost>) -> CostReport {
        let total: u64 = fns.iter().map(|f| f.proxy_cycles).sum();
        let words: u64 = fns.iter().map(|f| f.words).sum();
        CostReport {
            version: 3,
            digest: "t".to_string(),
            provenance: "test-prov".to_string(),
            provenance_summary: "T1=1 T2=0 T3=0 T4=0 T5=0 rows=1".to_string(),
            profile: "a76-pi5".to_string(),
            pipelines: 8,
            dispatch_mops: 4,
            dispatch_uops: 8,
            reorder_window: 128,
            total_proxy_cycles: total,
            total_words: words,
            owner_totals: BTreeMap::new(),
            fns,
            workloads_digest: None,
            workload_totals: BTreeMap::new(),
            workload_coverage: BTreeMap::new(),
            footprint: Vec::new(),
        }
    }

    #[test]
    fn flat_row_equals_total_and_boot_actors_fxs() {
        let fns = vec![
            fn_cost("Ledger.mark", 88),
            fn_cost("Ledger.read_marks", 41),
            fn_cost("Worker.slow", 833),
            fn_cost("Worker.quick", 475),
            fn_cost("Worker.report", 463),
            fn_cost("__wrela_abort", 142),
        ];
        let mut report = bare_report(fns);
        let set = parse_workloads(
            r#"
[flat]
weight = 1
[boot-actors]
weight = 10
"#,
        )
        .expect("workloads");
        let freq = parse_freq(
            r#"
workload=boot-actors
Ledger.mark=3
Ledger.read_marks=1
Worker.slow=3
Worker.quick=2
Worker.report=2
"#,
        )
        .expect("freq");
        let attach = WorkloadAttach::from_parts(set, freq);
        attach_workloads(&mut report, &attach).expect("attach");

        assert_eq!(report.workload_totals["flat"], report.total_proxy_cycles);
        // 3*88 + 1*41 + 3*833 + 2*475 + 2*463 = 264+41+2499+950+926 = 4680
        assert_eq!(report.workload_totals["boot-actors"], 4680);
        assert_eq!(report.workload_coverage["boot-actors"], (11, 11));
        assert!(report.workloads_digest.is_some());
    }

    #[test]
    fn uncovered_hits_reduce_coverage() {
        let mut report = bare_report(vec![fn_cost("Foo.bar", 10)]);
        let set = parse_workloads("[flat]\nweight = 1\n[w]\nweight = 1\n").unwrap();
        let freq = parse_freq("workload=w\nFoo.bar=2\nMissing.m=3\n").unwrap();
        attach_workloads(&mut report, &WorkloadAttach::from_parts(set, freq)).expect("attach");
        // 2×10 matched + 3×10 uncovered (charged at max fn schedule = 10).
        assert_eq!(report.workload_totals["w"], 50);
        assert_eq!(report.workload_coverage["w"], (2, 5));
    }

    /// The fusion/rename hole: dropping a hot method key from the scored
    /// set must **raise** `Σ f×s`, never lower it.
    #[test]
    fn vanished_hot_key_raises_total_and_drops_coverage() {
        let set = parse_workloads("[flat]\nweight = 1\n[w]\nweight = 1\n").unwrap();
        let freq_text = "workload=w\nHot.method=5\nCold.method=1\n";

        let mut before = bare_report(vec![fn_cost("Hot.method", 100), fn_cost("Cold.method", 10)]);
        attach_workloads(
            &mut before,
            &WorkloadAttach::from_parts(set.clone(), parse_freq(freq_text).unwrap()),
        )
        .expect("attach");

        // Same program with `Hot.method` fused away under a new key.
        let mut after = bare_report(vec![
            fn_cost("Hot.method$fused", 100),
            fn_cost("Cold.method", 10),
        ]);
        attach_workloads(
            &mut after,
            &WorkloadAttach::from_parts(set, parse_freq(freq_text).unwrap()),
        )
        .expect("attach");

        assert_eq!(before.workload_coverage["w"], (6, 6));
        assert_eq!(after.workload_coverage["w"], (1, 6));
        assert!(
            after.workload_totals["w"] >= before.workload_totals["w"],
            "losing coverage must not look cheap: {} -> {}",
            before.workload_totals["w"],
            after.workload_totals["w"]
        );
    }

    #[test]
    fn uncovered_charge_is_max_fn_schedule() {
        let fns = vec![fn_cost("a", 3), fn_cost("b", 41), fn_cost("c", 7)];
        assert_eq!(uncovered_charge(&fns), 41);
        assert_eq!(uncovered_charge(&[]), 0);
    }

    // --- block grain (plans/M20.md item C) -------------------------------

    use crate::cost::bridge::BlockBridge;
    use crate::cost::freq::parse_block;

    fn table() -> crate::cost::table::CostTable {
        crate::cost::load_default().expect("committed table")
    }

    /// Emit `case`'s cost-stage closure under bridge mode; return the
    /// scored report and its checked bridge.
    fn scored_with_bridge(case: &str) -> (CostReport, BlockBridge) {
        let p = crate::cost::repo_root().join(format!("tests/golden/{case}/input.wr"));
        crate::opts::apply_mode(crate::opts::CompileMode::Release);
        crate::codegen::set_block_bridge(true);
        let prog = crate::cost::codegen_cost_stage(&p).expect("cost-stage codegen");
        crate::codegen::set_block_bridge(false);
        let t = table();
        let report =
            crate::cost::score_program(&prog, &t, &crate::placement::PlacementTable::default())
                .expect("score");
        let bridge = BlockBridge::from_current_codegen(
            &prog,
            &t,
            &crate::placement::PlacementTable::default(),
        )
        .expect("bridge");
        (report, bridge)
    }

    /// **The point of block grain.** A block executed `f` times contributes
    /// `f × s(b)`, where method grain would charge the whole enclosing fn
    /// once — pricing a hot loop body the same as the function around it.
    #[test]
    fn a_hot_block_contributes_f_times_s_not_s() {
        let (report, bridge) = scored_with_bridge("boot-actors");
        // Pick a real resolved block with non-zero cost.
        let (key, block) = bridge
            .blocks()
            .find(|(_, b)| b.cycles > 0)
            .map(|(k, b)| (k.clone(), b.clone()))
            .expect("some scored block costs something");

        let once: BTreeMap<String, u64> = BTreeMap::from([(key.clone(), 1)]);
        let hot: BTreeMap<String, u64> = BTreeMap::from([(key.clone(), 1000)]);
        let a = block_grain_fxs(&report.fns, &bridge, &once).expect("once");
        let b = block_grain_fxs(&report.fns, &bridge, &hot).expect("hot");
        assert_eq!(a.cycles, block.cycles, "one hit charges s(b) exactly");
        assert_eq!(
            b.cycles,
            block.cycles * 1000,
            "1000 hits charge 1000 × s(b) — not s(b), and not the enclosing fn's schedule"
        );

        // And the same block priced at method grain is insensitive to `f`
        // *within* the fn: the fn's whole schedule, once per call.
        let fn_schedule = report
            .fns
            .iter()
            .find(|f| f.key == block.fn_key)
            .expect("the block's fn is scored")
            .proxy_cycles;
        assert!(
            fn_schedule >= block.cycles,
            "a block is part of its fn: {} vs {}",
            block.cycles,
            fn_schedule
        );
        assert!(
            b.cycles > fn_schedule,
            "a block hit 1000 times must outweigh one whole-fn charge (block grain is not \
             method grain with extra steps): {} vs {fn_schedule}",
            b.cycles
        );
    }

    /// An uncovered block hit is **charged** at the program maximum, never
    /// dropped (04 §5 coverage honesty; freeze 1627 keeps the denominator
    /// the whole scored set rather than the resolvable subset).
    #[test]
    fn an_uncovered_block_hit_is_charged_at_the_program_maximum() {
        let (report, bridge) = scored_with_bridge("boot-actors");
        let charge = uncovered_charge(&report.fns);
        assert!(charge > 0);
        let f = BTreeMap::from([("NotInTheScoredClosure.m#0".to_string(), 7u64)]);
        let m = block_grain_fxs(&report.fns, &bridge, &f).expect("uncovered is not an error");
        assert_eq!(m.resolved_keys, 0);
        assert_eq!(m.unresolved_keys, 1);
        assert_eq!(m.matched, 0);
        assert_eq!(m.total, 7);
        assert_eq!(m.cycles, 7 * charge, "charged, not dropped");
        assert_eq!(m.uncovered_cycles, m.cycles);
    }

    /// A key naming a **scored** fn with an out-of-range block index is a
    /// partition disagreement and must error (decision 1608).
    #[test]
    fn an_out_of_range_block_index_on_a_scored_fn_errors() {
        let (report, bridge) = scored_with_bridge("boot-actors");
        let some_fn = bridge
            .blocks()
            .next()
            .map(|(_, b)| b.fn_key.clone())
            .expect("a bridged fn");
        let f = BTreeMap::from([(format!("{some_fn}#100000"), 1u64)]);
        let err = block_grain_fxs(&report.fns, &bridge, &f).expect_err("must fail closed");
        assert!(err.contains("out of range"), "got: {err}");
    }

    /// A block-grain vector with no bridge must fail closed, not silently
    /// fall back to method grain or drop the row.
    #[test]
    fn a_block_vector_without_a_bridge_fails_closed() {
        let mut report = bare_report(vec![fn_cost("F.m", 10)]);
        let set = parse_workloads("[flat]\nweight = 1\n[w]\nweight = 1\n").unwrap();
        let attach = WorkloadAttach {
            set,
            frequencies: BTreeMap::new(),
            block_frequencies: BTreeMap::from([(
                "w".to_string(),
                BTreeMap::from([("F.m#0".to_string(), 1u64)]),
            )]),
            bridge: None,
        };
        let err = attach_workloads(&mut report, &attach).expect_err("no bridge");
        assert!(err.contains("no block bridge"), "got: {err}");
    }

    /// `boot-actors` commits **both** sidecars under one workload name.
    /// Block grain wins, and the choice is visible (`grain_of`).
    #[test]
    fn block_grain_wins_when_a_case_commits_both_sidecars() {
        let (mut report, bridge) = scored_with_bridge("boot-actors");
        let dir = crate::cost::repo_root().join("tests/golden/boot-actors");
        let m1 = crate::cost::freq::load_from_path(&dir.join("lane1-freq.txt")).expect("lane1");
        let m2 =
            crate::cost::freq::load_block_from_path(&dir.join("lane2-freq.txt")).expect("lane2");
        assert_eq!(m1.workload, m2.workload, "same workload name in both");
        let set = crate::cost::workload::load_default().expect("workloads");
        let attach = WorkloadAttach {
            set,
            frequencies: BTreeMap::from([(m1.workload.clone(), m1.counts.clone())]),
            block_frequencies: BTreeMap::from([(m2.workload.clone(), m2.counts.clone())]),
            bridge: Some(bridge.clone()),
        };
        attach_workloads(&mut report, &attach).expect("attach");
        assert_eq!(attach.grain_of("boot-actors"), Some("block"));

        let block_only = block_grain_fxs(&report.fns, &bridge, &m2.counts).expect("block");
        assert_eq!(
            report.workload_totals["boot-actors"], block_only.cycles,
            "the committed row must be the block-grain one"
        );
        let (method_cycles, _, _) = method_grain_fxs(&report.fns, &m1.counts);
        assert_ne!(
            block_only.cycles, method_cycles,
            "the two grains differ on this case, so which one won is observable"
        );
    }

    /// **The coverage finding, pinned as numbers** (plans/M20.md item C).
    ///
    /// Decision 1607 worried that an app-only Lane 2 vector would collapse
    /// coverage. Widening to `runtime`/`driver` (item B) fixed *that*, but
    /// the measurement says the dominant effect is a different one: the
    /// `@test(runtime)` image the sidecar is measured on has 2527 Lane 2
    /// blocks while the cost-stage closure `--stage=cost` scores has 184, so
    /// most measured hit-blocks name fns the scored program does not contain
    /// and are charged at the program maximum. The row is therefore
    /// dominated by the uncovered term. That is recorded, not fixed by
    /// narrowing the sidecar — narrowing it would be the "reward measuring
    /// less" failure 04 §5 forbids.
    ///
    /// This unit exists to make the numbers move loudly if they ever do.
    #[test]
    fn boot_actors_block_grain_coverage_is_pinned_and_dominated_by_uncovered_charge() {
        let (report, bridge) = scored_with_bridge("boot-actors");
        let f = crate::cost::freq::load_block_from_path(
            &crate::cost::repo_root().join("tests/golden/boot-actors/lane2-freq.txt"),
        )
        .expect("committed sidecar");
        let m = block_grain_fxs(&report.fns, &bridge, &f.counts).expect("compose");

        assert_eq!(bridge.block_count, 184, "scored-closure Lane 2 blocks");
        assert_eq!(f.counts.len(), 372, "measured non-zero hit blocks");
        assert_eq!(m.resolved_keys, 81, "measured hit-blocks that resolve");
        assert_eq!(m.unresolved_keys, 291, "measured hit-blocks that do not");
        assert_eq!((m.matched, m.total), (893, 6647), "hit mass matched/total");
        assert_eq!(m.resolved_keys + m.unresolved_keys, f.counts.len() as u64);
        // 99.6% of the measured total is uncovered charge. Asserted as a
        // ratio rather than a raw total so items E/F/H can move `s(b)`
        // without touching this finding.
        assert!(
            m.uncovered_cycles * 100 / m.cycles >= 99,
            "the uncovered term dominates this row: {} of {}",
            m.uncovered_cycles,
            m.cycles
        );
    }

    /// The `f` vector this case commits was measured on the test image, and
    /// every key in it must at least *parse* and resolve to one of the two
    /// legal outcomes — never to a bridge error.
    #[test]
    fn every_committed_boot_actors_key_resolves_or_is_uncovered() {
        let (_, bridge) = scored_with_bridge("boot-actors");
        let text = std::fs::read_to_string(
            crate::cost::repo_root().join("tests/golden/boot-actors/lane2-freq.txt"),
        )
        .expect("sidecar");
        let f = parse_block(&text).expect("parse");
        for key in f.counts.keys() {
            bridge
                .lookup(key)
                .unwrap_or_else(|e| panic!("committed key `{key}` must not error: {e}"));
        }
    }
}
