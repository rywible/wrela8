use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use crate::codegen::BlockSpan;

use super::Fnv64;
use super::bridge::split_key;
use super::freq::{self, BlockFreq};

pub const COUNTER_CLEARING_KEY: &str = "__wrela_rt_primary_boot";

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum BlockClass {
    Hot,
    Cold,
    Unmeasured,
}

impl BlockClass {
    pub fn as_str(self) -> &'static str {
        match self {
            BlockClass::Hot => "hot",
            BlockClass::Cold => "cold",
            BlockClass::Unmeasured => "unmeasured",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BlockRow {
    pub fn_key: String,
    pub block_index: u32,
    pub count: u64,
    pub class: BlockClass,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CallRow {
    pub fn_key: String,
    pub calls: Option<u64>,
    pub hot_blocks: u64,
    pub hits: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoopRow {
    pub fn_key: String,
    pub first_block: u32,
    pub last_block: u32,
    pub blocks: u64,
    pub calls: u64,
    pub peak_count: u64,
    pub trips_milli: u64,
    pub hits: u64,
}

impl LoopRow {
    pub fn trips_text(&self) -> String {
        format!("{}.{:03}", self.trips_milli / 1000, self.trips_milli % 1000)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DerivedTables {
    pub workload: String,
    pub sidecar_digest: u64,
    pub blocks: Vec<BlockRow>,
    pub loops: Vec<LoopRow>,
    pub calls: Vec<CallRow>,
    pub total_hits: u64,
    pub artifact_hits: u64,
    pub measured_fns: u64,
    pub measured_keys: u64,
}

pub fn derive(freq: &BlockFreq) -> Result<DerivedTables, String> {
    let mut per_fn: BTreeMap<&str, BTreeMap<u32, u64>> = BTreeMap::new();
    let mut total_hits: u64 = 0;
    let mut artifact_hits: u64 = 0;
    let mut digest = Fnv64::new();
    for (key, &count) in &freq.counts {
        let (fn_key, idx) = split_key(key)?;
        digest.write(key.as_bytes());
        digest.write(b"=");
        digest.write(count.to_string().as_bytes());
        digest.write(b"\n");
        total_hits = total_hits.saturating_add(count);
        if fn_key == COUNTER_CLEARING_KEY {
            artifact_hits = artifact_hits.saturating_add(count);
            continue;
        }
        if per_fn
            .entry(fn_key)
            .or_default()
            .insert(idx, count)
            .is_some()
        {
            return Err(format!(
                "derive: duplicate block key `{key}` (the sidecar's own parse should have \
                 rejected it)"
            ));
        }
    }
    if per_fn.is_empty() {
        return Err(format!(
            "derive: `{}` carries no measured block outside `{COUNTER_CLEARING_KEY}` — a vector \
             that is only the counter-clearing artifact is not a measurement (decision 1724)",
            freq.workload
        ));
    }

    let mut blocks = Vec::new();
    let mut loops = Vec::new();
    let mut calls = Vec::new();
    let mut measured_keys: u64 = 0;
    for (&fn_key, per_block) in &per_fn {
        let hits: u64 = per_block.values().sum();
        let entry = per_block.get(&0).copied();
        measured_keys += per_block.len() as u64;
        calls.push(CallRow {
            fn_key: fn_key.to_string(),
            calls: entry,
            hot_blocks: per_block.len() as u64,
            hits,
        });
        for (&block_index, &count) in per_block {
            blocks.push(BlockRow {
                fn_key: fn_key.to_string(),
                block_index,
                count,
                class: BlockClass::Hot,
            });
        }
        let Some(entry) = entry.filter(|&e| e > 0) else {
            continue;
        };
        let resident: Vec<u32> = per_block
            .iter()
            .filter(|&(_, &c)| c > entry)
            .map(|(&b, _)| b)
            .collect();
        for run in contiguous_runs(&resident) {
            let peak_count = run.iter().map(|b| per_block[b]).max().unwrap_or(0);
            let hits: u64 = run.iter().map(|b| per_block[b]).sum();
            loops.push(LoopRow {
                fn_key: fn_key.to_string(),
                first_block: run[0],
                last_block: run[run.len() - 1],
                blocks: run.len() as u64,
                calls: entry,
                peak_count,
                trips_milli: peak_count.saturating_mul(1000) / entry,
                hits,
            });
        }
    }

    Ok(DerivedTables {
        workload: freq.workload.clone(),
        sidecar_digest: digest.finish(),
        blocks,
        loops,
        calls,
        total_hits,
        artifact_hits,
        measured_fns: per_fn.len() as u64,
        measured_keys,
    })
}

fn contiguous_runs(sorted: &[u32]) -> Vec<Vec<u32>> {
    let mut out: Vec<Vec<u32>> = Vec::new();
    for &b in sorted {
        match out.last_mut() {
            Some(run) if run[run.len() - 1] + 1 == b => run.push(b),
            _ => out.push(vec![b]),
        }
    }
    out
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PartitionCheck {
    pub partition_digest: u64,
    pub matched_fns: u64,
    pub matched_keys: u64,
    pub unmatched_fns: u64,
}

impl DerivedTables {
    pub fn class_of(&self, fn_key: &str, block_index: u32) -> BlockClass {
        let hit = self
            .blocks
            .binary_search_by(|r| (r.fn_key.as_str(), r.block_index).cmp(&(fn_key, block_index)))
            .is_ok();
        if hit {
            return BlockClass::Hot;
        }
        let named = self
            .calls
            .binary_search_by(|r| r.fn_key.as_str().cmp(fn_key))
            .is_ok();
        if named {
            BlockClass::Cold
        } else {
            BlockClass::Unmeasured
        }
    }

    pub fn check_against_spans(&self, spans: &[BlockSpan]) -> Result<PartitionCheck, String> {
        if spans.is_empty() {
            return Err(format!(
                "derive: `{}` cannot be checked against an empty block partition — build under \
                 `codegen::set_block_bridge(true)` first (fail closed, never assume fresh)",
                self.workload
            ));
        }
        let mut built: BTreeMap<&str, u32> = BTreeMap::new();
        for s in spans {
            let n = built.entry(s.fn_key.as_str()).or_insert(0);
            *n = (*n).max(s.block_index + 1);
        }

        let mut matched_fns = 0u64;
        let mut unmatched_fns = 0u64;
        let mut matched_keys = 0u64;
        let mut named: BTreeSet<&str> = BTreeSet::new();
        let mut digest = Fnv64::new();
        for row in &self.calls {
            named.insert(row.fn_key.as_str());
        }
        for fn_key in &named {
            match built.get(fn_key) {
                Some(&n) => {
                    matched_fns += 1;
                    digest.write(fn_key.as_bytes());
                    digest.write(b"=");
                    digest.write(n.to_string().as_bytes());
                    digest.write(b"\n");
                }
                None => unmatched_fns += 1,
            }
        }
        for row in &self.blocks {
            let Some(&n) = built.get(row.fn_key.as_str()) else {
                continue;
            };
            if row.block_index >= n {
                return Err(format!(
                    "derive: sidecar `{}` is stale — key `{}#{}` names block {} of fn `{}`, which \
                     the built program partitions into {n} block(s). The fn was recompiled into a \
                     different shape; re-run `cargo xtask gen-lane2-freq {}` (fail closed, never \
                     re-key by nearest index).",
                    self.workload,
                    row.fn_key,
                    row.block_index,
                    row.block_index,
                    row.fn_key,
                    self.workload
                ));
            }
            matched_keys += 1;
        }
        if matched_fns == 0 {
            return Err(format!(
                "derive: sidecar `{}` is stale — none of its {} measured fn(s) exists in the \
                 built program's {} fn(s), so it describes a different program entirely",
                self.workload,
                named.len(),
                built.len()
            ));
        }
        Ok(PartitionCheck {
            partition_digest: digest.finish(),
            matched_fns,
            matched_keys,
            unmatched_fns,
        })
    }

    pub fn render(&self) -> String {
        let mut s = String::new();
        s.push_str(&format!(
            "workload={} sidecar_digest={:#018x}\n\
             hits total={} artifact={} measured={}\n\
             fns={} keys={} loops={}\n",
            self.workload,
            self.sidecar_digest,
            self.total_hits,
            self.artifact_hits,
            self.total_hits - self.artifact_hits,
            self.measured_fns,
            self.measured_keys,
            self.loops.len(),
        ));
        s.push_str("\n# per-loop measured trip counts\n");
        s.push_str("| run | calls | blocks | peak f | trips | hits |\n");
        s.push_str("| --- | --- | --- | --- | --- | --- |\n");
        let mut loops: Vec<&LoopRow> = self.loops.iter().collect();
        loops.sort_by(|a, b| {
            b.trips_milli
                .cmp(&a.trips_milli)
                .then_with(|| b.hits.cmp(&a.hits))
                .then_with(|| (&a.fn_key, a.first_block).cmp(&(&b.fn_key, b.first_block)))
        });
        for l in loops {
            s.push_str(&format!(
                "| `{}#{}..{}` | {} | {} | {} | {} | {} |\n",
                l.fn_key,
                l.first_block,
                l.last_block,
                l.calls,
                l.blocks,
                l.peak_count,
                l.trips_text(),
                l.hits
            ));
        }
        s.push_str("\n# per-fn call frequency\n");
        s.push_str("| fn | calls | hot blocks | hits |\n");
        s.push_str("| --- | --- | --- | --- |\n");
        let mut calls: Vec<&CallRow> = self.calls.iter().collect();
        calls.sort_by(|a, b| {
            b.calls
                .cmp(&a.calls)
                .then_with(|| b.hits.cmp(&a.hits))
                .then_with(|| a.fn_key.cmp(&b.fn_key))
        });
        for c in calls {
            s.push_str(&format!(
                "| `{}` | {} | {} | {} |\n",
                c.fn_key,
                match c.calls {
                    Some(n) => n.to_string(),
                    None => "-".to_string(),
                },
                c.hot_blocks,
                c.hits
            ));
        }
        s.push_str("\n# per-block hot/cold (hot rows; every other block of a named fn is cold)\n");
        s.push_str("| block | f | class |\n| --- | --- | --- |\n");
        for b in &self.blocks {
            s.push_str(&format!(
                "| `{}#{}` | {} | {} |\n",
                b.fn_key,
                b.block_index,
                b.count,
                b.class.as_str()
            ));
        }
        s
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LayoutClasses {
    Unmeasured,
    Measured(Box<DerivedTables>, PartitionCheck),
}

impl LayoutClasses {
    pub fn class_of(&self, fn_key: &str, block_index: u32) -> BlockClass {
        match self {
            LayoutClasses::Unmeasured => BlockClass::Unmeasured,
            LayoutClasses::Measured(t, _) => t.class_of(fn_key, block_index),
        }
    }

    pub fn is_measured(&self) -> bool {
        matches!(self, LayoutClasses::Measured(..))
    }
}

pub fn layout_classes(source: Option<&Path>, spans: &[BlockSpan]) -> Result<LayoutClasses, String> {
    let Some(source) = source else {
        return Ok(LayoutClasses::Unmeasured);
    };
    let Some(path) = freq::sibling_block_freq_path(source) else {
        return Ok(LayoutClasses::Unmeasured);
    };
    let f = freq::load_block_from_path(&path)?;
    let tables = derive(&f)?;
    let check = tables.check_against_spans(spans)?;
    Ok(LayoutClasses::Measured(Box::new(tables), check))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn committed() -> BlockFreq {
        let path = super::super::repo_root().join("tests/golden/boot-actors/lane2-freq.txt");
        freq::load_block_from_path(&path).expect("committed sidecar")
    }

    fn span(fn_key: &str, block_index: u32) -> BlockSpan {
        BlockSpan {
            fn_key: fn_key.to_string(),
            block_index,
            id: block_index,
            word_start: 0,
            word_end: 0,
        }
    }

    fn fresh_spans(t: &DerivedTables) -> Vec<BlockSpan> {
        let mut arity: BTreeMap<&str, u32> = BTreeMap::new();
        for b in &t.blocks {
            let n = arity.entry(b.fn_key.as_str()).or_insert(0);
            *n = (*n).max(b.block_index + 1);
        }
        let mut out = Vec::new();
        for (fn_key, n) in arity {
            for i in 0..n {
                out.push(span(fn_key, i));
            }
        }
        out
    }

    #[test]
    fn derives_the_committed_boot_actors_vector() {
        let t = derive(&committed()).expect("derive");
        assert_eq!(t.workload, "boot-actors");
        assert_eq!(t.total_hits, 6647);
        assert_eq!(t.artifact_hits, 3938);
        assert_eq!(t.measured_fns, 67);
        assert_eq!(t.measured_keys, 364);
        assert_eq!(t.loops.len(), 23);
        assert!(
            !t.calls.iter().any(|c| c.fn_key == COUNTER_CLEARING_KEY),
            "the artifact fn must not appear in any derived table"
        );
    }

    #[test]
    fn the_peak_measured_trip_count_is_thirteen() {
        let t = derive(&committed()).expect("derive");
        let peak = t.loops.iter().max_by_key(|l| l.trips_milli).expect("loops");
        assert_eq!(peak.fn_key, "copy_bytes_range");
        assert_eq!(peak.trips_milli, 13_000);
        assert_eq!(peak.calls, 1);
        let busiest = t.loops.iter().max_by_key(|l| l.hits).expect("loops");
        assert_eq!(busiest.fn_key, "copy_line_buf_range");
        assert_eq!(busiest.trips_milli, 3_696);
        assert_eq!(busiest.hits, 300);
        assert_eq!(busiest.trips_text(), "3.696");
    }

    #[test]
    fn call_freq_agrees_with_lane1_on_the_sync_methods() {
        let t = derive(&committed()).expect("derive");
        let lane1 = freq::load_from_path(
            &super::super::repo_root().join("tests/golden/boot-actors/lane1-freq.txt"),
        )
        .expect("lane1");
        let calls: BTreeMap<&str, Option<u64>> = t
            .calls
            .iter()
            .map(|c| (c.fn_key.as_str(), c.calls))
            .collect();
        assert_eq!(lane1.counts["Ledger.mark"], 3);
        assert_eq!(calls["Ledger.mark"], Some(3));
        assert_eq!(lane1.counts["Ledger.read_marks"], 1);
        assert_eq!(calls["Ledger.read_marks"], Some(1));
        for (m, turns) in [
            ("Worker.slow", 3),
            ("Worker.quick", 2),
            ("Worker.report", 2),
        ] {
            assert_eq!(lane1.counts[m], turns, "lane1 turns for {m}");
            assert_eq!(calls[m], Some(1), "lane2 fresh entries for {m}");
        }
    }

    #[test]
    fn hot_cold_is_three_valued_and_unmeasured_is_not_cold() {
        let t = derive(&committed()).expect("derive");
        assert_eq!(t.class_of("Worker.slow", 0), BlockClass::Hot);
        assert_eq!(t.class_of("Worker.slow", 4), BlockClass::Cold);
        assert_eq!(t.class_of("Worker.slow", 999), BlockClass::Cold);
        assert_eq!(
            t.class_of("A.fn_the_sidecar_never_named", 0),
            BlockClass::Unmeasured
        );
        assert_eq!(t.class_of(COUNTER_CLEARING_KEY, 13), BlockClass::Unmeasured);
    }

    #[test]
    fn derivation_is_byte_identical_across_runs_and_input_orders() {
        let f = committed();
        let a = derive(&f).expect("a").render();
        let b = derive(&f).expect("b").render();
        assert_eq!(a, b);

        let mut lines: Vec<String> = f.counts.iter().map(|(k, v)| format!("{k}={v}")).collect();
        lines.reverse();
        let shuffled = format!("workload={}\n{}\n", f.workload, lines.join("\n"));
        let c = derive(&freq::parse_block(&shuffled).expect("reparse"))
            .expect("c")
            .render();
        assert_eq!(a, c, "input line order must not reach the output");

        let t = derive(&f).expect("t");
        assert!(
            t.blocks
                .windows(2)
                .all(|w| (&w[0].fn_key, w[0].block_index) < (&w[1].fn_key, w[1].block_index)),
            "blocks must be strictly sorted (class_of binary-searches them)"
        );
        assert!(
            t.calls.windows(2).all(|w| w[0].fn_key < w[1].fn_key),
            "calls must be strictly sorted"
        );
        assert_eq!(t.sidecar_digest, derive(&f).expect("again").sidecar_digest);
    }

    #[test]
    fn missing_sidecar_degrades_to_unmeasured_never_to_a_guess() {
        let none = super::super::repo_root().join("tests/golden/cost-arith/input.wr");
        assert!(freq::sibling_block_freq_path(&none).is_none());
        let lc =
            layout_classes(Some(&none), &[span("A.turn", 0)]).expect("no sidecar is not an error");
        assert_eq!(lc, LayoutClasses::Unmeasured);
        assert!(!lc.is_measured());
        assert_eq!(lc.class_of("A.turn", 0), BlockClass::Unmeasured);
        assert_eq!(
            layout_classes(None, &[span("A.turn", 0)]).expect("no source"),
            LayoutClasses::Unmeasured
        );
    }

    #[test]
    fn a_present_sidecar_over_an_unbuilt_partition_fails_closed() {
        let t = derive(&committed()).expect("derive");
        let err = t.check_against_spans(&[]).expect_err("empty partition");
        assert!(err.contains("set_block_bridge"), "got: {err}");

        let input = super::super::repo_root().join("tests/golden/boot-actors/input.wr");
        let err = layout_classes(Some(&input), &[]).expect_err("empty partition");
        assert!(err.contains("set_block_bridge"), "got: {err}");
    }

    #[test]
    fn a_stale_sidecar_fails_closed_on_a_shrunken_fn() {
        let t = derive(&committed()).expect("derive");
        let fresh = fresh_spans(&t);
        t.check_against_spans(&fresh)
            .expect("fresh partition is not stale");

        let stale: Vec<BlockSpan> = fresh
            .iter()
            .filter(|s| !(s.fn_key == "Worker.slow" && s.block_index >= 9))
            .cloned()
            .collect();
        let err = t.check_against_spans(&stale).expect_err("stale");
        assert!(err.contains("is stale"), "got: {err}");
        assert!(err.contains("Worker.slow"), "got: {err}");
        assert!(err.contains("gen-lane2-freq"), "got: {err}");
    }

    #[test]
    fn a_sidecar_for_a_different_program_fails_closed() {
        let t = derive(&committed()).expect("derive");
        let err = t
            .check_against_spans(&[span("Stranger.turn", 0), span("Stranger.turn", 1)])
            .expect_err("different program");
        assert!(err.contains("different program"), "got: {err}");
    }

    #[test]
    fn in_range_drift_is_invisible_to_the_key_checks_and_visible_in_the_digest() {
        let t = derive(&committed()).expect("derive");
        let fresh = fresh_spans(&t);
        let a = t.check_against_spans(&fresh).expect("fresh");
        let mut grown = fresh.clone();
        grown.push(span("Worker.slow", 15));
        let b = t.check_against_spans(&grown).expect("still in range");
        assert_eq!(
            a.matched_keys, b.matched_keys,
            "the key checks cannot see it"
        );
        assert_ne!(
            a.partition_digest, b.partition_digest,
            "the partition digest must"
        );
        assert_eq!(a.unmatched_fns, 0);
    }

    #[test]
    fn an_artifact_only_sidecar_is_not_a_measurement() {
        let f = freq::parse_block(&format!(
            "workload=w\n{COUNTER_CLEARING_KEY}#13=986\n{COUNTER_CLEARING_KEY}#14=984\n"
        ))
        .expect("parse");
        let err = derive(&f).expect_err("artifact only");
        assert!(err.contains("counter-clearing artifact"), "got: {err}");
    }

    #[test]
    fn layout_classes_over_a_real_bridge_mode_build() {
        let input = super::super::repo_root().join("tests/golden/boot-actors/input.wr");
        crate::opts::apply_mode(crate::opts::CompileMode::Release);
        crate::codegen::set_block_bridge(true);
        let _prog = super::super::codegen_cost_stage(&input).expect("cost-stage codegen");
        let spans = crate::codegen::block_spans();
        crate::codegen::set_block_bridge(false);
        assert!(!spans.is_empty(), "bridge mode must record a partition");

        let lc = layout_classes(Some(&input), &spans).expect("a fresh build is not stale");
        let LayoutClasses::Measured(t, check) = &lc else {
            panic!("boot-actors has a committed sidecar; this must be Measured");
        };
        assert!(lc.is_measured());
        assert_eq!(t.sidecar_digest, 0x4a53_6169_0b06_f87a);

        assert_eq!(check.matched_fns, 14);
        assert_eq!(check.unmatched_fns, 53);
        assert_eq!(check.matched_fns + check.unmatched_fns, t.measured_fns);
        assert_eq!(check.matched_keys, 81);
        assert!(
            check.matched_keys < t.measured_keys,
            "a closure that resolved every key would mean the two closures were the same program"
        );

        let mut hot = 0u64;
        let mut cold = 0u64;
        let mut unmeasured = 0u64;
        for s in &spans {
            match lc.class_of(&s.fn_key, s.block_index) {
                BlockClass::Hot => hot += 1,
                BlockClass::Cold => cold += 1,
                BlockClass::Unmeasured => unmeasured += 1,
            }
        }
        assert_eq!(hot + cold + unmeasured, spans.len() as u64);
        assert_eq!((hot, cold, unmeasured), (81, 83, 18));
        assert_eq!(hot, check.matched_keys, "every resolved key is a hot block");
        assert!(
            unmeasured > 0,
            "a closure the sidecar covers completely would not exercise the three-valued \
             classification at all, and this unit would stop being decision 1723's oracle"
        );
    }

    #[test]
    fn print_the_committed_tables() {
        let t = derive(&committed()).expect("derive");
        println!("{}", t.render());
    }
}
