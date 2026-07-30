//! The block bridge (plans/M20.md item C, decision 1608) — **proved, not
//! assumed**.
//!
//! Lane 2 block ids are assigned over **MWIR** instructions
//! (`codegen::assign_mwir_block_ids` / `assign_flat_block_ids`); `s(b)` is
//! computed over **emitted-word** ranges (`cost::score::basic_block_ranges`).
//! Two partitions of the same fn. Codegen's two-pass emission already
//! prefix-sums `word_offsets[mwir_idx] → starting word index`, and that is
//! the whole bridge: `codegen::set_block_bridge` records a tiling
//! `BlockSpan` per Lane 2 block from it, without emitting a single counter
//! word, and this module checks the two partitions agree before any cost is
//! attributed.
//!
//! **Every disagreement is an error.** Never attribute by nearest offset:
//! a Lane 2 block whose `word_start` is not an emitted-word block leader, a
//! fn whose spans do not tile its word range, and a sidecar key whose block
//! index is out of range for a scored fn all fail closed. The one thing
//! that is *not* an error is a sidecar key naming a fn the scored closure
//! does not contain — see `lookup`.
//!
//! ## A Lane 2 block spans a *set* of emitted-word blocks
//!
//! Emitted code has strictly more blocks than MWIR does: every checked
//! operation emits an abort-check branch, so one MWIR-level straight line
//! becomes several emitted-word blocks. A Lane 2 span therefore maps to a
//! **set** of word-blocks and its cost is `Σ s(b)` over that set. Measured
//! on `boot-actors`' cost-stage closure, the mean is well above 1 — the
//! reason nearest-offset attribution (forbidden by 1608) would be wrong
//! even when it looked plausible.

use std::collections::BTreeMap;

use crate::codegen::{BlockSpan, CodegenProgram};

use super::score::{basic_block_ranges, block_schedule_lengths};
use super::table::CostTable;

/// A resolved Lane 2 block: its word span in the scored program and the
/// `Σ s(b)` of the emitted-word blocks that span covers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BridgedBlock {
    pub fn_key: String,
    pub block_index: u32,
    pub word_start: usize,
    pub word_end: usize,
    /// How many emitted-word basic blocks this span covers.
    pub word_blocks: u64,
    /// `Σ s(b)` over those emitted-word blocks — the cost one measured hit
    /// on this block buys.
    pub cycles: u64,
}

/// The checked MWIR ↔ emitted-word correspondence for one scored program.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BlockBridge {
    /// `<fn_key>#<block_index>` → resolved block.
    blocks: BTreeMap<String, BridgedBlock>,
    /// Scored fns that carry Lane 2 spans at all (the instrumented set).
    fns_with_spans: BTreeMap<String, u32>,
    /// Lane 2 blocks bridged.
    pub block_count: u64,
    /// Emitted-word blocks covered by some span.
    pub covered_word_blocks: u64,
    /// Spans covering zero emitted words (a MWIR leader whose instruction
    /// emitted nothing). Legal, priced at 0, counted so it is visible.
    pub empty_spans: u64,
}

/// The sidecar / snapshot key for one Lane 2 block.
pub fn make_key(fn_key: &str, block_index: u32) -> String {
    format!("{fn_key}#{block_index}")
}

/// Split `<fn_key>#<block_index>`. Fail closed: a key with no `#`, an
/// empty fn part, or a non-`u32` index is a malformed sidecar, not a miss.
pub fn split_key(key: &str) -> Result<(&str, u32), String> {
    let (fn_key, idx) = key
        .rsplit_once('#')
        .ok_or_else(|| format!("block key `{key}`: expected <fn_key>#<block_index>"))?;
    if fn_key.is_empty() {
        return Err(format!("block key `{key}`: empty fn key"));
    }
    let idx: u32 = idx
        .parse()
        .map_err(|_| format!("block key `{key}`: block index must be u32, got `{idx}`"))?;
    Ok((fn_key, idx))
}

/// What a sidecar key resolved to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Resolved<'a> {
    /// The key names a scored fn and an in-range block: charge `f × cycles`.
    Block(&'a BridgedBlock),
    /// The key names a fn the **scored** closure does not contain.
    ///
    /// Not an error, and this is the one place item C's plan text had to
    /// bend to a measured fact: the `@test(runtime)` image the guest boots
    /// and the cost-stage closure `--stage=cost` scores are two different
    /// programs (`boot-actors`: 2527 Lane 2 blocks vs 184). Test-harness,
    /// boot-init and unreached-stdlib fns exist only in the former, so a
    /// sidecar generated from a real boot *necessarily* names fns the
    /// scored program has never heard of. Those hits are **uncovered** —
    /// charged at the program maximum by `compose`, never dropped — which
    /// is the coverage rule, not a bridge failure. A key naming a fn that
    /// *is* scored but a block index out of its range is a genuine
    /// partition disagreement and errors instead.
    UnknownFn,
}

impl BlockBridge {
    /// Build and check the bridge for `program` from the spans
    /// `codegen::set_block_bridge` recorded while emitting it.
    ///
    /// Fail-closed directions, each with its own unit:
    /// 1. a span names a fn not in `program.fns`;
    /// 2. a fn's spans are not ordered / contiguous;
    /// 3. a fn's spans do not start at word 0 or do not end at `code.len()`
    ///    (they must **tile** the fn's word range);
    /// 4. a span boundary is not an emitted-word block leader.
    pub fn build(
        program: &CodegenProgram,
        spans: &[BlockSpan],
        table: &CostTable,
    ) -> Result<Self, String> {
        let mut by_fn: BTreeMap<&str, Vec<&BlockSpan>> = BTreeMap::new();
        for s in spans {
            by_fn.entry(s.fn_key.as_str()).or_default().push(s);
        }

        let mut blocks = BTreeMap::new();
        let mut fns_with_spans = BTreeMap::new();
        let mut covered_word_blocks = 0u64;
        let mut empty_spans = 0u64;

        for (fn_key, fn_spans) in by_fn {
            let f = program.fns.get(fn_key).ok_or_else(|| {
                format!(
                    "bridge: Lane 2 span names fn `{fn_key}`, which the scored program does not \
                     contain (the two partitions must describe the same program)"
                )
            })?;
            let ranges = basic_block_ranges(&f.code);
            let lengths = block_schedule_lengths(&f.code, table)?;
            if ranges.len() != lengths.len() {
                return Err(format!(
                    "bridge: internal error: fn `{fn_key}` has {} emitted-word block(s) but {} \
                     schedule length(s)",
                    ranges.len(),
                    lengths.len()
                ));
            }
            // word start index -> (word block ordinal)
            let leader_of: BTreeMap<usize, usize> = ranges
                .iter()
                .enumerate()
                .map(|(k, &(start, _))| (start, k))
                .collect();

            let code_len = f.code.len();
            let mut expect_start = 0usize;
            for (n, span) in fn_spans.iter().enumerate() {
                if span.block_index as usize != n {
                    return Err(format!(
                        "bridge: fn `{fn_key}` block ordinals out of order: expected {n}, got {}",
                        span.block_index
                    ));
                }
                if span.word_start != expect_start {
                    return Err(format!(
                        "bridge: fn `{fn_key}` block {n} starts at word {} but the previous block \
                         ended at {expect_start} — spans must tile the fn's word range",
                        span.word_start
                    ));
                }
                if span.word_end < span.word_start || span.word_end > code_len {
                    return Err(format!(
                        "bridge: fn `{fn_key}` block {n} span {}..{} is out of range for {code_len} \
                         emitted word(s)",
                        span.word_start, span.word_end
                    ));
                }
                expect_start = span.word_end;
            }
            if expect_start != code_len {
                return Err(format!(
                    "bridge: fn `{fn_key}` spans cover words 0..{expect_start} of {code_len} — \
                     spans must tile the fn's word range"
                ));
            }

            for (n, span) in fn_spans.iter().enumerate() {
                if span.word_start == span.word_end {
                    empty_spans += 1;
                    blocks.insert(
                        make_key(fn_key, n as u32),
                        BridgedBlock {
                            fn_key: fn_key.to_string(),
                            block_index: n as u32,
                            word_start: span.word_start,
                            word_end: span.word_end,
                            word_blocks: 0,
                            cycles: 0,
                        },
                    );
                    continue;
                }
                let first = *leader_of.get(&span.word_start).ok_or_else(|| {
                    format!(
                        "bridge: fn `{fn_key}` block {n} starts at word {} which is not an \
                         emitted-word block leader (decision 1608: never attribute by nearest \
                         offset)",
                        span.word_start
                    )
                })?;
                // `word_end` must be a leader too, or the fn's end.
                if span.word_end != code_len && !leader_of.contains_key(&span.word_end) {
                    return Err(format!(
                        "bridge: fn `{fn_key}` block {n} ends at word {} which is not an \
                         emitted-word block leader (decision 1608: never attribute by nearest \
                         offset)",
                        span.word_end
                    ));
                }
                let mut cycles = 0u64;
                let mut word_blocks = 0u64;
                for k in first..ranges.len() {
                    if ranges[k].0 >= span.word_end {
                        break;
                    }
                    cycles = cycles.saturating_add(lengths[k]);
                    word_blocks += 1;
                }
                covered_word_blocks += word_blocks;
                blocks.insert(
                    make_key(fn_key, n as u32),
                    BridgedBlock {
                        fn_key: fn_key.to_string(),
                        block_index: n as u32,
                        word_start: span.word_start,
                        word_end: span.word_end,
                        word_blocks,
                        cycles,
                    },
                );
            }
            fns_with_spans.insert(fn_key.to_string(), fn_spans.len() as u32);
        }

        let block_count = blocks.len() as u64;
        Ok(Self {
            blocks,
            fns_with_spans,
            block_count,
            covered_word_blocks,
            empty_spans,
        })
    }

    /// Build from the spans the current thread's most recent bridge-mode
    /// codegen recorded (`codegen::block_spans`).
    pub fn from_current_codegen(
        program: &CodegenProgram,
        table: &CostTable,
    ) -> Result<Self, String> {
        Self::build(program, &crate::codegen::block_spans(), table)
    }

    /// Resolve one sidecar key. `Err` for a malformed key or an
    /// out-of-range block index on a fn that **is** scored.
    pub fn lookup(&self, key: &str) -> Result<Resolved<'_>, String> {
        let (fn_key, idx) = split_key(key)?;
        match self.blocks.get(key) {
            Some(b) => Ok(Resolved::Block(b)),
            None => match self.fns_with_spans.get(fn_key) {
                Some(&n) => Err(format!(
                    "bridge: sidecar key `{key}` names block {idx} of fn `{fn_key}`, which has \
                     {n} Lane 2 block(s) — out of range (decision 1608: fail closed, never \
                     attribute by nearest offset)"
                )),
                None => Ok(Resolved::UnknownFn),
            },
        }
    }

    /// Scored fns carrying Lane 2 blocks.
    pub fn fn_count(&self) -> u64 {
        self.fns_with_spans.len() as u64
    }

    /// Every bridged block, in key order.
    pub fn blocks(&self) -> impl Iterator<Item = (&String, &BridgedBlock)> {
        self.blocks.iter()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::codegen::CodegenFn;
    use crate::cost::rule::{CostRule, EmittedWord};

    fn word(rule: CostRule) -> EmittedWord {
        EmittedWord::new(0xd503_201f, "nop".to_string(), rule, None, &[])
    }

    /// `b .+4` — a real PC-relative branch so `basic_block_ranges` sees a
    /// leader at the following word.
    fn branch() -> EmittedWord {
        EmittedWord::new(
            crate::encode::enc_b(4),
            "b .+4".to_string(),
            CostRule::Branch,
            None,
            &[],
        )
    }

    fn program(code: Vec<EmittedWord>) -> CodegenProgram {
        let mut fns = BTreeMap::new();
        fns.insert(
            "F.m".to_string(),
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

    fn span(idx: u32, start: usize, end: usize) -> BlockSpan {
        BlockSpan {
            fn_key: "F.m".to_string(),
            block_index: idx,
            id: idx,
            word_start: start,
            word_end: end,
        }
    }

    fn table() -> CostTable {
        crate::cost::load_default().expect("committed table")
    }

    /// Two Lane 2 blocks over a stream whose branch splits the first one
    /// into two emitted-word blocks: the span maps to a **set**.
    #[test]
    fn a_lane2_span_covers_the_set_of_word_blocks_inside_it() {
        // words: 0 alu, 1 branch, 2 alu | 3 alu (second Lane 2 block)
        let prog = program(vec![
            word(CostRule::Alu),
            branch(),
            word(CostRule::Alu),
            word(CostRule::Alu),
        ]);
        let t = table();
        // emitted-word leaders: 0 (entry), 2 (fallthrough after branch).
        let ranges = basic_block_ranges(&prog.fns["F.m"].code);
        assert_eq!(ranges, vec![(0, 2), (2, 4)]);
        let bridge =
            BlockBridge::build(&prog, &[span(0, 0, 2), span(1, 2, 4)], &t).expect("bridge");
        let b0 = match bridge.lookup("F.m#0").expect("lookup") {
            Resolved::Block(b) => b.clone(),
            other => panic!("expected a block, got {other:?}"),
        };
        assert_eq!((b0.word_start, b0.word_end, b0.word_blocks), (0, 2, 1));
        let all: u64 = block_schedule_lengths(&prog.fns["F.m"].code, &t)
            .unwrap()
            .iter()
            .sum();
        let bridged: u64 = bridge.blocks().map(|(_, b)| b.cycles).sum();
        assert_eq!(
            bridged, all,
            "tiling spans must account for every word block's s(b)"
        );
    }

    /// One Lane 2 block over a stream with two emitted-word blocks: the
    /// span's cost is the **sum** of both, not one of them.
    #[test]
    fn one_lane2_block_sums_several_word_blocks() {
        let prog = program(vec![
            word(CostRule::Alu),
            branch(),
            word(CostRule::Alu),
            word(CostRule::Alu),
        ]);
        let t = table();
        let bridge = BlockBridge::build(&prog, &[span(0, 0, 4)], &t).expect("bridge");
        let lens = block_schedule_lengths(&prog.fns["F.m"].code, &t).unwrap();
        assert_eq!(lens.len(), 2, "two emitted-word blocks");
        match bridge.lookup("F.m#0").unwrap() {
            Resolved::Block(b) => {
                assert_eq!(b.word_blocks, 2);
                assert_eq!(b.cycles, lens[0] + lens[1]);
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn fail_closed_on_a_span_start_that_is_not_a_word_block_leader() {
        // Leaders are 0 and 2; a span starting at word 1 is mid-block.
        let prog = program(vec![
            word(CostRule::Alu),
            branch(),
            word(CostRule::Alu),
            word(CostRule::Alu),
        ]);
        let err = BlockBridge::build(&prog, &[span(0, 0, 1), span(1, 1, 4)], &table())
            .expect_err("mid-block span start must fail closed");
        assert!(
            err.contains("not an emitted-word block leader"),
            "got: {err}"
        );
    }

    #[test]
    fn fail_closed_when_spans_do_not_tile_the_fn() {
        let prog = program(vec![word(CostRule::Alu), branch(), word(CostRule::Alu)]);
        let err = BlockBridge::build(&prog, &[span(0, 0, 2)], &table())
            .expect_err("a short tiling must fail closed");
        assert!(err.contains("must tile"), "got: {err}");

        let err = BlockBridge::build(&prog, &[span(0, 0, 2), span(1, 3, 3)], &table())
            .expect_err("a gap must fail closed");
        assert!(err.contains("must tile"), "got: {err}");
    }

    #[test]
    fn fail_closed_on_a_span_naming_an_unscored_fn() {
        let prog = program(vec![word(CostRule::Alu)]);
        let mut s = span(0, 0, 1);
        s.fn_key = "Other.m".to_string();
        let err = BlockBridge::build(&prog, &[s], &table()).expect_err("unknown fn");
        assert!(err.contains("does not contain"), "got: {err}");
    }

    #[test]
    fn lookup_fails_closed_on_an_out_of_range_block_index() {
        let prog = program(vec![word(CostRule::Alu)]);
        let bridge = BlockBridge::build(&prog, &[span(0, 0, 1)], &table()).expect("bridge");
        let err = bridge.lookup("F.m#7").expect_err("out of range");
        assert!(err.contains("out of range"), "got: {err}");
    }

    /// An unknown *fn* is not an error — it is an uncovered hit. The two
    /// closures genuinely differ (module doc / `Resolved::UnknownFn`).
    #[test]
    fn lookup_reports_an_unknown_fn_as_uncovered_not_an_error() {
        let prog = program(vec![word(CostRule::Alu)]);
        let bridge = BlockBridge::build(&prog, &[span(0, 0, 1)], &table()).expect("bridge");
        assert_eq!(
            bridge.lookup("NotScored.m#0").expect("no error"),
            Resolved::UnknownFn
        );
    }

    #[test]
    fn split_key_fails_closed_on_malformed_keys() {
        assert!(split_key("F.m").is_err());
        assert!(split_key("#3").is_err());
        assert!(split_key("F.m#x").is_err());
        assert_eq!(split_key("F.m#12").unwrap(), ("F.m", 12));
    }

    // --- real-program oracles (item C's own oracle: a green compose test
    // that never exercises the bridge does not satisfy this clause) ------

    fn corpus(case: &str) -> std::path::PathBuf {
        crate::cost::repo_root().join(format!("tests/golden/{case}/input.wr"))
    }

    /// Emit `case`'s cost-stage closure under bridge mode and return
    /// (program, spans, ids assigned).
    fn bridge_codegen(case: &str) -> (CodegenProgram, Vec<BlockSpan>, u32) {
        crate::opts::apply_mode(crate::opts::CompileMode::Release);
        crate::codegen::set_block_bridge(true);
        let prog = crate::cost::codegen_cost_stage(&corpus(case)).expect("cost-stage codegen");
        let spans = crate::codegen::block_spans();
        let ids = crate::codegen::block_ids_assigned();
        crate::codegen::set_block_bridge(false);
        (prog, spans, ids)
    }

    /// **The bridge-agreement oracle** (decision 1608), on real corpus
    /// closures rather than a synthetic stream: the two partitions agree.
    ///
    /// Every Lane 2 block maps to exactly one emitted-word block leader,
    /// every fn's spans tile its word range, and — the part that proves no
    /// `s(b)` is lost or double-counted — the Σ of bridged block costs for a
    /// fn equals that fn's own scored `proxy_cycles`.
    #[test]
    fn bridge_agrees_with_the_scored_partition_on_real_closures() {
        let t = table();
        for case in ["boot-actors", "cost-runtime", "cost-arith", "boot-hello"] {
            let (prog, spans, _) = bridge_codegen(case);
            let bridge = BlockBridge::build(&prog, &spans, &t)
                .unwrap_or_else(|e| panic!("{case}: bridge must agree: {e}"));
            assert!(
                bridge.block_count > 0,
                "{case}: bridge must carry blocks (an empty bridge proves nothing)"
            );
            assert!(
                bridge.covered_word_blocks >= bridge.block_count,
                "{case}: a Lane 2 block spans one or more word blocks"
            );
            let report = crate::cost::score_program(&prog, &t).expect("score");
            let mut per_fn: BTreeMap<&str, u64> = BTreeMap::new();
            for (_, b) in bridge.blocks() {
                *per_fn.entry(b.fn_key.as_str()).or_insert(0) += b.cycles;
            }
            for f in &report.fns {
                if let Some(&summed) = per_fn.get(f.key.as_str()) {
                    assert_eq!(
                        summed, f.proxy_cycles,
                        "{case}: fn `{}`: Σ bridged block s(b) must equal the fn's own scored \
                         schedule (tiling, no loss, no double count)",
                        f.key
                    );
                }
            }
        }
    }

    /// Bridge mode must not change one emitted word — otherwise the bridge
    /// would describe a program the cost model is not scoring, which is the
    /// exact corruption `--block-count` causes (5 words per leader).
    #[test]
    fn block_bridge_mode_leaves_the_word_stream_byte_identical() {
        crate::opts::apply_mode(crate::opts::CompileMode::Release);
        crate::codegen::set_block_bridge(false);
        crate::codegen::set_block_count(false);
        let plain = crate::cost::codegen_cost_stage(&corpus("boot-actors")).expect("plain");

        let (bridged, spans, _) = bridge_codegen("boot-actors");
        assert!(!spans.is_empty(), "bridge mode must record spans");

        assert_eq!(
            plain.fns.keys().collect::<Vec<_>>(),
            bridged.fns.keys().collect::<Vec<_>>(),
            "bridge mode must not change the emitted fn set"
        );
        for (key, p) in &plain.fns {
            let b = &bridged.fns[key];
            let pw: Vec<u32> = p.code.iter().map(|w| w.word).collect();
            let bw: Vec<u32> = b.code.iter().map(|w| w.word).collect();
            assert_eq!(pw, bw, "fn `{key}`: bridge mode changed the word stream");
            assert_eq!(p.frame_size, b.frame_size, "fn `{key}`: frame size moved");
        }

        // And the control: `--block-count` *does* change it, so the test
        // above is not vacuous.
        crate::codegen::set_block_count(true);
        let counted = crate::cost::codegen_cost_stage(&corpus("boot-actors")).expect("counted");
        crate::codegen::set_block_count(false);
        let plain_words: usize = plain.fns.values().map(|f| f.code.len()).sum();
        let counted_words: usize = counted.fns.values().map(|f| f.code.len()).sum();
        assert!(
            counted_words > plain_words,
            "`--block-count` must grow the word stream (else the byte-identity \
             assertion above is vacuous): {plain_words} vs {counted_words}"
        );
    }

    /// The generator's correctness rests on this: the id → `fn_key#idx`
    /// assignment is identical across two runs of the same closure.
    #[test]
    fn the_id_to_block_key_map_is_deterministic_across_runs() {
        let (_, a, ids_a) = bridge_codegen("boot-actors");
        let (_, b, ids_b) = bridge_codegen("boot-actors");
        assert_eq!(ids_a, ids_b, "id count must be stable");
        assert_eq!(a, b, "the whole span/id map must be stable across runs");
        let keys: Vec<(u32, String)> = a
            .iter()
            .map(|s| (s.id, make_key(&s.fn_key, s.block_index)))
            .collect();
        let mut seen: BTreeMap<u32, &String> = BTreeMap::new();
        for (id, key) in &keys {
            assert!(
                seen.insert(*id, key).is_none(),
                "id {id} assigned to two block keys"
            );
        }
        assert_eq!(seen.len(), ids_a as usize, "every id must map to one key");
    }

    /// Bridge mode assigns the *same* ids `--block-count` assigns, which is
    /// what makes an offline id → key translation from a boot legitimate.
    #[test]
    fn bridge_mode_assigns_the_same_ids_as_block_count() {
        let (_, bridge_spans, bridge_ids) = bridge_codegen("boot-actors");

        crate::opts::apply_mode(crate::opts::CompileMode::Release);
        crate::codegen::set_block_count(true);
        crate::codegen::set_block_bridge(true);
        let _ = crate::cost::codegen_cost_stage(&corpus("boot-actors")).expect("both modes");
        let both_spans = crate::codegen::block_spans();
        let both_ids = crate::codegen::block_ids_assigned();
        crate::codegen::set_block_count(false);
        crate::codegen::set_block_bridge(false);

        assert_eq!(bridge_ids, both_ids, "id count must not depend on emission");
        let ids_only = |v: &[BlockSpan]| -> Vec<(u32, String, u32)> {
            v.iter()
                .map(|s| (s.id, s.fn_key.clone(), s.block_index))
                .collect()
        };
        assert_eq!(
            ids_only(&bridge_spans),
            ids_only(&both_spans),
            "the id -> fn_key#idx map must be identical with and without counter emission"
        );
    }
}
