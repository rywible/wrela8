//! Lane 2 **derived tables** (plans/codegen-pareto.md item A, decisions
//! 1720–1729).
//!
//! M20 item C committed one measured artifact: `lane2-freq.txt`, a
//! `<fn_key>#<block_index> = <count>` vector taken from the host DRAM
//! snapshot of a real boot ([`super::freq::BlockFreq`]). This module turns
//! that one vector into the three tables the codegen-Pareto plan keys off:
//!
//! 1. **per-loop measured trip counts** — decides whether unrolling is ever
//!    worth pulling in from the backlog;
//! 2. **per-block hot/cold classification** — item D's whole input;
//! 3. **per-fn call frequency** — the plan's deliverable (its consumer, F6,
//!    was cut at activation by decision 1770; nothing is built on it here).
//!
//! ## Why the derivation reads the sidecar and nothing else (decision 1720)
//!
//! The sidecar's key space is the `@test(runtime)` image's closure (2527
//! Lane 2 blocks). No in-compiler build reproduces that closure — the
//! cost-stage closure has 184 — so a CFG-based derivation would have to
//! rebuild and re-partition the test image inside a unit test. It does not
//! need to: every number below is *provable from the counts alone*. What
//! needs the program being compiled is only the fail-closed staleness check
//! ([`DerivedTables::check_against_spans`]) and item D's entry point
//! ([`layout_classes`]), and both take the spans they are handed.
//!
//! ## Determinism
//!
//! Every map is a `BTreeMap`, every table a `Vec` built in sorted key
//! order, and every ratio is carried as an integer (milli-trips) — there is
//! no float and no `HashMap` on any path that reaches an output.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use crate::codegen::BlockSpan;

use super::Fnv64;
use super::bridge::split_key;
use super::freq::{self, BlockFreq};

/// The one fn whose Lane 2 counts are an **instrumentation artifact**
/// rather than a measurement (decision 1724).
///
/// `__wrela_rt_primary_boot` contains the loop that zeroes the counter pool
/// itself:
///
/// ```text
/// @budget(bound=4096)
/// while zj < BLOCK_POOL_COUNT:
///     LANE2.hits[zj] = 0
/// ```
///
/// The loop's own blocks increment their counters on every iteration while
/// the same loop is walking `LANE2.hits[0..3072]` clearing them, so each of
/// its blocks ends holding *the number of iterations left after its own
/// global id was passed* — a function of where codegen happened to place
/// that block in the id space, not of the workload. Measured on
/// `boot-actors` those four blocks hold 986/984/983/981 and the fn totals
/// **3938 of the vector's 6647 hits (59.2%)**; its entry block `#0` is
/// absent from the sidecar entirely, because the zeroing loop wiped it
/// after it ran. Left in, this fn is the "hottest" code in the image and
/// every derived table is about the counter-clearing loop.
///
/// Excluded **by name**, the same shape `codegen::BLOCK_HIT_KEY` already
/// uses for the counter helper, and reported rather than hidden:
/// [`DerivedTables::artifact_hits`] carries the mass that was removed.
pub const COUNTER_CLEARING_KEY: &str = "__wrela_rt_primary_boot";

/// What the measurement says about one block's layout placement.
///
/// **`Unmeasured` is not `Cold`** (decision 1723). The sidecar carries
/// every non-zero counter from a pool that covers every assigned id, so a
/// block that is *absent from a fn the sidecar does name* really did
/// execute zero times — that is `Cold`, real evidence. A block in a fn the
/// sidecar never names has no evidence at all; calling it cold would sink
/// most of a program on nothing. Item D must leave `Unmeasured` blocks
/// exactly where they are.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum BlockClass {
    /// Measured, count ≥ 1.
    Hot,
    /// Measured, count 0 (absent from a fn the sidecar does name).
    Cold,
    /// No evidence — the sidecar does not name this fn at all.
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

/// One row of the per-block hot/cold table.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BlockRow {
    pub fn_key: String,
    pub block_index: u32,
    pub count: u64,
    pub class: BlockClass,
}

/// One row of the per-fn call-frequency table.
///
/// `calls` is `f(fn#0)` (decision 1721). Leader 0's span starts at word 0
/// of the fn (`codegen::record_spans`), so every **call** runs it. For an
/// async fn that is the count of *fresh* entries only: the dispatch header
/// branches straight to `state_flat_base[k]` on a resume, past block 0.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CallRow {
    pub fn_key: String,
    /// `f(fn#0)`, or `None` when block 0 is absent (see
    /// [`COUNTER_CLEARING_KEY`] — the only measured instance).
    pub calls: Option<u64>,
    /// Distinct blocks of this fn with a non-zero count.
    pub hot_blocks: u64,
    /// `Σ f(b)` over this fn.
    pub hits: u64,
}

/// One row of the per-loop trip-count table.
///
/// The grain is a **contiguous run of loop-resident block indices**, not a
/// proved natural loop — see decision 1722. A gap in the run (an abort
/// check's cold arm, say) splits one source loop into two rows; two
/// adjacent source loops with no gap between them merge into one. The row
/// is therefore a *lower* bound on how many loops there are and an exact
/// statement about the blocks it names.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoopRow {
    pub fn_key: String,
    pub first_block: u32,
    pub last_block: u32,
    /// Blocks in the run.
    pub blocks: u64,
    /// `f(fn#0)` — the denominator.
    pub calls: u64,
    /// `max f(b)` over the run.
    pub peak_count: u64,
    /// `peak_count / calls`, ×1000 (integer; no float on an output path).
    pub trips_milli: u64,
    /// `Σ f(b)` over the run.
    pub hits: u64,
}

impl LoopRow {
    /// `trips_milli` rendered as a fixed-point decimal.
    pub fn trips_text(&self) -> String {
        format!("{}.{:03}", self.trips_milli / 1000, self.trips_milli % 1000)
    }
}

/// The three tables, derived from one `lane2-freq.txt`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DerivedTables {
    pub workload: String,
    /// FNV-1a over the sorted `key=count` body — moves whenever the sidecar
    /// is edited or regenerated (decision 1725).
    pub sidecar_digest: u64,
    /// Per-block hot/cold, sorted by `(fn_key, block_index)`. Carries the
    /// measured (`Hot`) rows only; every other block of a named fn is
    /// `Cold` and every block of an unnamed fn is `Unmeasured`, both
    /// answered by [`DerivedTables::class_of`] without a row.
    pub blocks: Vec<BlockRow>,
    /// Per-loop trip counts, sorted by `(fn_key, first_block)`.
    pub loops: Vec<LoopRow>,
    /// Per-fn call frequency, sorted by `fn_key`.
    pub calls: Vec<CallRow>,
    /// `Σ f(b)` over the whole sidecar, artifact included.
    pub total_hits: u64,
    /// Of `total_hits`, the mass removed by decision 1724.
    pub artifact_hits: u64,
    /// Fns named by the sidecar, artifact excluded.
    pub measured_fns: u64,
    /// Keys the sidecar carries, artifact excluded.
    pub measured_keys: u64,
}

/// Derive the three tables from a parsed sidecar.
///
/// Fails closed on a malformed key (`split_key`) and on a sidecar with no
/// usable row left after decision 1724's exclusion — a vector that is
/// nothing but the counter-clearing artifact is not a measurement.
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
        // Decision 1722: a block cannot run more than once per invocation
        // without a back-edge, so `f(b) > f(fn#0)` *proves* loop residency
        // and `f(b)/f(fn#0)` is its measured per-call trip count. Needs no
        // CFG. Without an entry count there is no denominator and the fn
        // contributes no loop row.
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

/// Split a sorted, deduplicated index list into maximal `+1` runs.
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

/// What [`DerivedTables::check_against_spans`] proved about one program.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PartitionCheck {
    /// FNV-1a over the built program's `(fn_key, block_count)` pairs,
    /// restricted to the fns the sidecar names. Moves whenever a measured
    /// fn's block partition changes — the in-range drift the key checks
    /// below cannot see (decision 1725).
    pub partition_digest: u64,
    /// Fns named by both the sidecar and the built program.
    pub matched_fns: u64,
    /// Sidecar keys resolving to a block of a matched fn.
    pub matched_keys: u64,
    /// Fns the sidecar names that the built program does not contain. Not
    /// an error — the sidecar's closure is the `@test(runtime)` image and
    /// any other build is legitimately a different fn set.
    pub unmatched_fns: u64,
}

impl DerivedTables {
    /// Item D's classifier: the class of one block of the **built**
    /// program.
    ///
    /// `Hot` when the sidecar measured it non-zero; `Cold` when the sidecar
    /// names the fn but not this block (a real measured zero); `Unmeasured`
    /// when the sidecar does not name the fn at all.
    pub fn class_of(&self, fn_key: &str, block_index: u32) -> BlockClass {
        // `blocks` is sorted by `(fn_key, block_index)`, so both questions
        // are one binary search each.
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

    /// **Fail closed on a stale sidecar** (decision 1725).
    ///
    /// `spans` is the block partition of the program being compiled, as
    /// `codegen::block_spans()` records it under bridge mode. Three
    /// directions error, each with its own unit:
    ///
    /// 1. a sidecar key names a fn the built program *does* contain, at a
    ///    block index that fn does not have — the fn was recompiled into a
    ///    different shape and its measured counts no longer describe it;
    /// 2. no fn is named by both — the sidecar describes some other
    ///    program;
    /// 3. `spans` is empty — the caller did not build under bridge mode, so
    ///    there is nothing to check against and a silent pass would be a
    ///    fiction.
    ///
    /// What this **cannot** see is drift that leaves every measured fn's
    /// block count unchanged and every index in range. That is what
    /// [`PartitionCheck::partition_digest`] is for: it is a number a caller
    /// pins, and a pinned artifact that moves is this repo's staleness
    /// detector of record.
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

    /// The three tables as deterministic text — the review surface, and the
    /// source of the tables committed to `plans/codegen-pareto-A.md`.
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

/// Item D's hot/cold classification for one program.
///
/// The two states are deliberately distinct types rather than an empty map:
/// "no sidecar" and "a sidecar that measured nothing here" must not be the
/// same call for a layout pass.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LayoutClasses {
    /// No `lane2-freq.txt` next to the source. **Item D must leave the
    /// layout exactly as it found it** — every block is hot, nothing moves.
    /// This is the degradation, not a guess at coldness.
    Unmeasured,
    /// A sidecar that passed [`DerivedTables::check_against_spans`].
    Measured(Box<DerivedTables>, PartitionCheck),
}

impl LayoutClasses {
    /// The class of one block of the built program.
    ///
    /// [`LayoutClasses::Unmeasured`] answers [`BlockClass::Unmeasured`] for
    /// every block — item D reads that as "leave it where it is".
    pub fn class_of(&self, fn_key: &str, block_index: u32) -> BlockClass {
        match self {
            LayoutClasses::Unmeasured => BlockClass::Unmeasured,
            LayoutClasses::Measured(t, _) => t.class_of(fn_key, block_index),
        }
    }

    /// Whether any measurement is behind this classification at all.
    pub fn is_measured(&self) -> bool {
        matches!(self, LayoutClasses::Measured(..))
    }
}

/// **Item D's entry point.**
///
/// `source` is the compiled `input.wr` (the sidecar is its sibling,
/// `super::freq::sibling_block_freq_path`); `spans` is the block partition
/// of the program being compiled, from `codegen::block_spans()` under
/// bridge mode.
///
/// - No `source`, or no sidecar beside it → [`LayoutClasses::Unmeasured`].
///   Item D leaves layout unchanged. This is the **only** silent path, and
///   it is silent because "there is no measurement" is the ordinary case
///   for every non-boot-bearing program in the tree.
/// - A sidecar that is present but malformed, or stale against `spans` →
///   `Err`. A stale profile is a wrong profile and it must not lay out an
///   image (fail closed).
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

    /// Spans that cover every key of the committed sidecar exactly — the
    /// "fresh" partition every staleness unit perturbs.
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

    // --- the derivation itself -----------------------------------------

    #[test]
    fn derives_the_committed_boot_actors_vector() {
        let t = derive(&committed()).expect("derive");
        assert_eq!(t.workload, "boot-actors");
        assert_eq!(t.total_hits, 6647);
        // Decision 1724: the counter-clearing loop is 59.2% of the vector.
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
        // The number the unrolling pull-in decision rests on: no loop in
        // the workload runs more than 13 iterations per call, and the only
        // loop that runs 13 runs once in the whole boot.
        let t = derive(&committed()).expect("derive");
        let peak = t.loops.iter().max_by_key(|l| l.trips_milli).expect("loops");
        assert_eq!(peak.fn_key, "copy_bytes_range");
        assert_eq!(peak.trips_milli, 13_000);
        assert_eq!(peak.calls, 1);
        // The busiest loop by hit mass is the line-buffer copy, at 3.697.
        let busiest = t.loops.iter().max_by_key(|l| l.hits).expect("loops");
        assert_eq!(busiest.fn_key, "copy_line_buf_range");
        assert_eq!(busiest.trips_milli, 3_696); // 122*1000/33, floored
        assert_eq!(busiest.hits, 300);
        assert_eq!(busiest.trips_text(), "3.696");
    }

    #[test]
    fn call_freq_agrees_with_lane1_on_the_sync_methods() {
        // Decision 1721's oracle. Lane 1 counts **turns**; `f(fn#0)` counts
        // **fresh entries**. For a sync method the two must be identical;
        // for an async one they differ by exactly the suspension count, so
        // Lane 1 must never be *below* Lane 2's entry count.
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
        // Sync methods: exact agreement.
        assert_eq!(lane1.counts["Ledger.mark"], 3);
        assert_eq!(calls["Ledger.mark"], Some(3));
        assert_eq!(lane1.counts["Ledger.read_marks"], 1);
        assert_eq!(calls["Ledger.read_marks"], Some(1));
        // Async methods: one fresh entry, several turns.
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
        // Decision 1723. `Worker.slow` is measured at blocks 0,1,2,3,7,8,
        // 9,10,14 — so block 4 of that fn is a measured zero (cold), while
        // a fn the sidecar never names is unmeasured and must not move.
        let t = derive(&committed()).expect("derive");
        assert_eq!(t.class_of("Worker.slow", 0), BlockClass::Hot);
        assert_eq!(t.class_of("Worker.slow", 4), BlockClass::Cold);
        assert_eq!(t.class_of("Worker.slow", 999), BlockClass::Cold);
        assert_eq!(
            t.class_of("A.fn_the_sidecar_never_named", 0),
            BlockClass::Unmeasured
        );
        // The artifact fn is excluded, so it reads as unmeasured, never as
        // the hottest code in the image.
        assert_eq!(t.class_of(COUNTER_CLEARING_KEY, 13), BlockClass::Unmeasured);
    }

    // --- determinism ----------------------------------------------------

    #[test]
    fn derivation_is_byte_identical_across_runs_and_input_orders() {
        // Freeze 1714: this exercises the derivation, not a stub. Same
        // input twice -> identical render; and because every input map is
        // a `BTreeMap`, the insertion order of the parse cannot leak in —
        // re-parsing the same body with its lines shuffled must produce the
        // identical bytes.
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

    // --- fail closed -----------------------------------------------------

    #[test]
    fn missing_sidecar_degrades_to_unmeasured_never_to_a_guess() {
        // Item D's failure mode: no sidecar -> every block unmeasured ->
        // layout unchanged. Never `Cold`, which would sink the program.
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
        // Direction 3: the caller did not build under bridge mode. A pass
        // here would classify an image against nothing.
        let t = derive(&committed()).expect("derive");
        let err = t.check_against_spans(&[]).expect_err("empty partition");
        assert!(err.contains("set_block_bridge"), "got: {err}");

        let input = super::super::repo_root().join("tests/golden/boot-actors/input.wr");
        let err = layout_classes(Some(&input), &[]).expect_err("empty partition");
        assert!(err.contains("set_block_bridge"), "got: {err}");
    }

    #[test]
    fn a_stale_sidecar_fails_closed_on_a_shrunken_fn() {
        // Direction 1 — the realistic drift: a measured fn is recompiled
        // into fewer blocks, so its old block indices name blocks that no
        // longer exist. `Worker.slow` is measured up to block 14.
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
        // Direction 2: not one fn in common.
        let t = derive(&committed()).expect("derive");
        let err = t
            .check_against_spans(&[span("Stranger.turn", 0), span("Stranger.turn", 1)])
            .expect_err("different program");
        assert!(err.contains("different program"), "got: {err}");
    }

    #[test]
    fn in_range_drift_is_invisible_to_the_key_checks_and_visible_in_the_digest() {
        // The honest limit of decision 1725, pinned so it cannot be
        // forgotten: a partition change that keeps every measured index in
        // range passes the key checks, and the digest is what moves.
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
    fn print_the_committed_tables() {
        // The source of the tables in `plans/codegen-pareto-A.md`. Run with
        // `-- --nocapture` to regenerate them.
        let t = derive(&committed()).expect("derive");
        println!("{}", t.render());
    }
}
