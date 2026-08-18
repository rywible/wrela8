use std::collections::BTreeMap;
use std::path::Path;

use super::branch::BlockCounts;
use super::bridge::{BlockBridge, MeasuredBlocks, Resolved};
use super::footprint::{self, CoreBudget, HotBlocks};
use super::freq::{self, BlockFreq, MethodFreq};
use super::score::{CostReport, FnCost};
use super::sweep::SweepPoint;
use super::workload::{self, FLAT_NAME, WorkloadSet};

#[derive(Debug, Clone)]
pub struct WorkloadAttach {
    pub set: WorkloadSet,
    pub frequencies: BTreeMap<String, BTreeMap<String, u64>>,
    pub block_frequencies: BTreeMap<String, BTreeMap<String, u64>>,
    pub bridge: Option<BlockBridge>,
    pub measured_footprint: BTreeMap<String, Vec<CoreBudget>>,
}

impl WorkloadAttach {
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
        let mut measured_footprint = BTreeMap::new();
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
                let spans = crate::codegen::block_spans();
                let flat = BlockBridge::build(program, &spans, table, placement)?;
                let measured = MeasuredBlocks::resolve(&flat, &f.counts)?;
                let obs_fn = |k: &str, b: usize| measured.obs(k, b);
                let counts = BlockCounts::Measured(&obs_fn);
                bridge = Some(BlockBridge::build_with_counts(
                    program, &spans, table, placement, &counts,
                )?);
                let hot_fn = |k: &str, b: usize| measured.is_hot(k, b);
                measured_footprint.insert(
                    f.workload.clone(),
                    footprint::compute(
                        program,
                        table,
                        &SweepPoint::pinned(table),
                        placement,
                        HotBlocks::Measured(&hot_fn),
                    )?,
                );
                block_frequencies.insert(f.workload, f.counts);
            }
        }
        Ok(Self {
            set,
            frequencies,
            block_frequencies,
            bridge,
            measured_footprint,
        })
    }

    pub fn load_default_for_linked(
        source: Option<&Path>,
        linked: &crate::linked::LinkedProgram,
        table: &super::table::CostTable,
        placement: &crate::placement::PlacementTable,
    ) -> Result<Self, String> {
        let set = workload::load_default()?;
        let mut out = Self {
            set,
            frequencies: BTreeMap::new(),
            block_frequencies: BTreeMap::new(),
            bridge: None,
            measured_footprint: BTreeMap::new(),
        };
        let Some(source) = source else { return Ok(out) };
        let source = std::fs::canonicalize(source)
            .map_err(|error| format!("workload source {}: {error}", source.display()))?;
        let matching: Vec<String> = out
            .set
            .names()
            .filter(|name| *name != FLAT_NAME)
            .filter_map(|name| {
                let mapped = out.set.source_path(name)?;
                let mapped = std::fs::canonicalize(mapped).ok()?;
                (mapped == source).then(|| name.to_string())
            })
            .collect();
        for name in matching {
            let mapped = out
                .set
                .source_path(&name)
                .ok_or_else(|| format!("workload `{name}` has no source mapping"))?;
            if let Some(freq_path) = freq::sibling_freq_path(&mapped) {
                let measured = freq::load_from_path(&freq_path)?;
                if measured.workload != name {
                    return Err(format!(
                        "workload source `{name}` has method sidecar for `{}`",
                        measured.workload
                    ));
                }
                out.frequencies.insert(name.clone(), measured.counts);
            }
            if let Some(freq_path) = freq::sibling_block_freq_path(&mapped) {
                let measured_freq = freq::load_block_from_path(&freq_path)?;
                if measured_freq.workload != name {
                    return Err(format!(
                        "workload source `{name}` has block sidecar for `{}`",
                        measured_freq.workload
                    ));
                }
                let flat = BlockBridge::from_linked(linked, table, placement)?;
                let measured = MeasuredBlocks::resolve(&flat, &measured_freq.counts)?;
                if measured.unresolved_keys != 0 {
                    let unknown: Vec<&str> = measured_freq
                        .counts
                        .keys()
                        .filter_map(|key| match flat.lookup(key) {
                            Ok(Resolved::UnknownFn) => Some(key.as_str()),
                            _ => None,
                        })
                        .collect();
                    return Err(format!(
                        "workload `{name}` is unrankable: {} measured key(s) do not resolve in the linked image: {}",
                        measured.unresolved_keys,
                        unknown.join(",")
                    ));
                }
                let obs_fn = |key: &str, block: usize| measured.obs(key, block);
                let counts = BlockCounts::Measured(&obs_fn);
                out.bridge = Some(BlockBridge::from_linked_with_counts(
                    linked, table, placement, &counts,
                )?);
                let hot_fn = |key: &str, block: usize| measured.is_hot(key, block);
                out.measured_footprint.insert(
                    name.clone(),
                    footprint::compute_linked(
                        linked,
                        table,
                        &SweepPoint::pinned(table),
                        placement,
                        HotBlocks::Measured(&hot_fn),
                    )?,
                );
                out.block_frequencies.insert(name, measured_freq.counts);
            }
        }
        Ok(out)
    }

    pub fn from_parts(set: WorkloadSet, freq: MethodFreq) -> Self {
        let mut frequencies = BTreeMap::new();
        frequencies.insert(freq.workload, freq.counts);
        Self {
            set,
            frequencies,
            block_frequencies: BTreeMap::new(),
            bridge: None,
            measured_footprint: BTreeMap::new(),
        }
    }

    pub fn from_block_parts(set: WorkloadSet, freq: BlockFreq, bridge: BlockBridge) -> Self {
        let mut block_frequencies = BTreeMap::new();
        block_frequencies.insert(freq.workload, freq.counts);
        Self {
            set,
            frequencies: BTreeMap::new(),
            block_frequencies,
            bridge: Some(bridge),
            measured_footprint: BTreeMap::new(),
        }
    }

    pub fn measured_budget(&self, name: &str) -> Option<&[CoreBudget]> {
        self.measured_footprint.get(name).map(|v| v.as_slice())
    }

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

pub fn attach_workloads(report: &mut CostReport, attach: &WorkloadAttach) -> Result<(), String> {
    report.workloads_digest = Some(attach.set.digest());
    report.workload_totals.clear();
    report.workload_coverage.clear();
    report.workload_validation_bounds.clear();

    report
        .workload_totals
        .insert(FLAT_NAME.to_string(), report.total_proxy_cycles);

    for (name, counts) in &attach.frequencies {
        if attach.set.weight(name).is_none() {
            continue;
        }
        if attach.block_frequencies.contains_key(name) {
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
        let footprint_cycles = attach
            .measured_budget(name)
            .unwrap_or(&[])
            .iter()
            .map(|budget| budget.charge)
            .sum::<u64>();
        report
            .workload_totals
            .insert(name.clone(), m.cycles.saturating_add(footprint_cycles));
        report
            .workload_coverage
            .insert(name.clone(), (m.matched, m.total));
        report
            .workload_validation_bounds
            .insert(name.clone(), m.serial_cycles);
    }
    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct BlockGrainMeasure {
    pub cycles: u64,
    pub serial_cycles: u64,
    pub matched: u64,
    pub total: u64,
    pub resolved_keys: u64,
    pub unresolved_keys: u64,
    pub uncovered_cycles: u64,
}

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
                m.serial_cycles = m
                    .serial_cycles
                    .saturating_add(f.saturating_mul(b.serial_cycles));
                m.matched = m.matched.saturating_add(f);
                m.resolved_keys += 1;
            }
            Resolved::UnknownFn => {
                let c = f.saturating_mul(charge);
                m.cycles = m.cycles.saturating_add(c);
                m.serial_cycles = m.serial_cycles.saturating_add(c);
                m.uncovered_cycles = m.uncovered_cycles.saturating_add(c);
                m.unresolved_keys += 1;
            }
        }
    }
    Ok(m)
}

pub fn method_grain_fxs(fns: &[FnCost], freq: &BTreeMap<String, u64>) -> (u64, u64, u64) {
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
            frame_bytes: 0,
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
            schedule_cycles: total,
            footprint_cycles: 0,
            rank_cycles: total,
            total_words: words,
            sync_frame_max_bytes: 0,
            async_frame_total_bytes: 0,
            owner_totals: BTreeMap::new(),
            fns,
            workloads_digest: None,
            workload_totals: BTreeMap::new(),
            workload_coverage: BTreeMap::new(),
            workload_validation_bounds: BTreeMap::new(),
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
        assert_eq!(report.workload_totals["w"], 50);
        assert_eq!(report.workload_coverage["w"], (2, 5));
    }

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

    use crate::cost::bridge::BlockBridge;
    use crate::cost::freq::parse_block;

    fn table() -> crate::cost::table::CostTable {
        crate::cost::load_default().expect("committed table")
    }

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

    #[test]
    fn a_hot_block_contributes_f_times_s_not_s() {
        let (report, bridge) = scored_with_bridge("boot-actors");
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
            measured_footprint: BTreeMap::new(),
        };
        let err = attach_workloads(&mut report, &attach).expect_err("no bridge");
        assert!(err.contains("no block bridge"), "got: {err}");
    }

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
            measured_footprint: BTreeMap::new(),
        };
        attach_workloads(&mut report, &attach).expect("attach");
        assert_eq!(attach.grain_of("boot-actors"), Some("block"));

        let block_only = block_grain_fxs(&report.fns, &bridge, &m2.counts).expect("block");
        let measured_footprint = attach
            .measured_budget("boot-actors")
            .unwrap_or(&[])
            .iter()
            .map(|budget| budget.charge)
            .sum::<u64>();
        assert_eq!(
            report.workload_totals["boot-actors"],
            block_only.cycles + measured_footprint,
            "the committed row uses block schedule plus its actual hot footprint"
        );
        let (method_cycles, _, _) = method_grain_fxs(&report.fns, &m1.counts);
        assert_ne!(
            block_only.cycles, method_cycles,
            "the two grains differ on this case, so which one won is observable"
        );
    }

    #[test]
    fn legacy_closure_composition_labels_unresolved_production_keys_unrankable() {
        let (report, bridge) = scored_with_bridge("boot-actors");
        let f = crate::cost::freq::load_block_from_path(
            &crate::cost::repo_root().join("tests/golden/boot-actors/lane2-freq.txt"),
        )
        .expect("committed sidecar");
        let m = block_grain_fxs(&report.fns, &bridge, &f.counts).expect("compose");

        assert!(bridge.block_count > 0);
        assert_eq!(f.counts.len(), 271, "production-window non-zero blocks");
        assert!(m.resolved_keys > 0);
        assert!(
            m.unresolved_keys > 0,
            "the closure diagnostic is intentionally unrankable"
        );
        assert_eq!(m.total, 18278);
        assert_eq!(m.resolved_keys + m.unresolved_keys, f.counts.len() as u64);
        assert!(m.uncovered_cycles > 0);
    }

    fn attached(case: &str) -> (CostReport, WorkloadAttach, crate::placement::PlacementTable) {
        let p = crate::cost::repo_root().join(format!("tests/golden/{case}/input.wr"));
        crate::opts::apply_mode(crate::opts::CompileMode::Release);
        crate::codegen::set_block_bridge(true);
        let (prog, placement) =
            crate::cost::codegen_cost_stage_with_placement(&p).expect("cost-stage codegen");
        crate::codegen::set_block_bridge(false);
        let t = table();
        let mut report = crate::cost::score_program(&prog, &t, &placement).expect("score");
        let attach =
            WorkloadAttach::load_default_for(Some(&p), &prog, &t, &placement).expect("attach");
        attach_workloads(&mut report, &attach).expect("attach workloads");
        (report, attach, placement)
    }

    #[test]
    fn the_measured_row_carries_bias_and_the_flat_row_does_not() {
        let (report, attach, placement) = attached("boot-actors");
        let t = table();

        let (prog, _) = {
            let p = crate::cost::repo_root().join("tests/golden/boot-actors/input.wr");
            crate::opts::apply_mode(crate::opts::CompileMode::Release);
            crate::codegen::set_block_bridge(true);
            let r = crate::cost::codegen_cost_stage_with_placement(&p).expect("codegen");
            crate::codegen::set_block_bridge(false);
            r
        };
        let flat = crate::cost::score_program_at_with_hot(
            &prog,
            &t,
            &placement,
            &SweepPoint::pinned(&t),
            HotBlocks::All,
            BlockCounts::Flat,
        )
        .expect("flat score");
        assert_eq!(report.total_proxy_cycles, flat.total_proxy_cycles);
        assert_eq!(
            report.workload_totals["flat"], report.total_proxy_cycles,
            "the flat row IS cost(P, W_flat)"
        );
        assert_eq!(
            report.footprint, flat.footprint,
            "the printed `Budget` line stays the static-footprint row (HotBlocks::All)"
        );

        let spans = crate::codegen::block_spans();
        let flat_bridge = BlockBridge::build(&prog, &spans, &t, &placement).expect("flat bridge");
        let counts = &attach.block_frequencies["boot-actors"];
        let under_flat_s = block_grain_fxs(&report.fns, &flat_bridge, counts).expect("flat s");
        let measured = block_grain_fxs(
            &report.fns,
            attach.bridge.as_ref().expect("measured bridge"),
            counts,
        )
        .expect("measured s");
        let measured_footprint = attach
            .measured_budget("boot-actors")
            .unwrap_or(&[])
            .iter()
            .map(|budget| budget.charge)
            .sum::<u64>();
        assert_eq!(
            report.workload_totals["boot-actors"],
            measured.cycles + measured_footprint
        );
        assert!(
            measured.cycles > under_flat_s.cycles,
            "wiring the measured counts into s(b) must actually move the measured row: \
             {} vs {}",
            under_flat_s.cycles,
            measured.cycles
        );

        assert_eq!(report.workload_coverage["boot-actors"], (16742, 18278));
        assert_eq!(
            (measured.matched, measured.total),
            (under_flat_s.matched, under_flat_s.total)
        );
        assert_eq!(
            measured.resolved_keys + measured.unresolved_keys,
            counts.len() as u64
        );
        assert_eq!(attach.grain_of("boot-actors"), Some("block"));
    }

    #[test]
    fn the_measured_per_core_budget_excludes_cold_text() {
        let (report, attach, _) = attached("boot-actors");
        let flat = &report.footprint;
        let measured = attach
            .measured_budget("boot-actors")
            .expect("a block-grain workload has a measured budget");
        assert_eq!(flat.len(), measured.len(), "one budget per core, both rows");
        assert!(!flat.is_empty(), "boot-actors has a sealed placement");
        for (f, m) in flat.iter().zip(measured.iter()) {
            assert_eq!(f.n, m.n);
            assert!(
                m.fetched_text_bytes < f.fetched_text_bytes,
                "core {}: the measured vector must exclude some text ({} vs {})",
                f.n,
                m.fetched_text_bytes,
                f.fetched_text_bytes
            );
            assert!(m.fetched_text_bytes > 0, "core {}: and not all of it", f.n);
            assert_eq!(f.l1i_bytes, 65536);
        }
        assert!(attach.measured_budget("flat").is_none());
    }

    #[test]
    fn a_case_with_no_block_vector_is_untouched_by_the_wiring() {
        let (report, attach, _) = attached("cost-branchy");
        assert!(attach.bridge.is_none());
        assert!(attach.measured_footprint.is_empty());
        assert!(attach.block_frequencies.is_empty());
        // Re-measured 2026-08-07 with `AdrAddressing` parked.
        assert_eq!(report.total_proxy_cycles, 50, "the pinned flat total");
        assert_eq!(report.workload_totals["flat"], 50);
        assert_eq!(report.workload_totals.len(), 1, "flat row only");
    }

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
