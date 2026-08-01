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
//!
//! ## The join: [`MeasuredBlocks`]
//!
//! Item C landed the correspondence; item F
//! ([`super::footprint::HotBlocks`]) and item H
//! ([`super::branch::BlockCounts`]) each landed an injection point shaped
//! `(fn_key, word_block_index) -> …` and each defaulted to its
//! measurement-free variant. [`MeasuredBlocks`] is the one type that turns a
//! bridge plus a measured `<fn_key>#<block_index>` vector into both closures.
//! It lives here because this module already owns the MWIR ↔ word-block
//! correspondence; a second module for the join would be a layer for its own
//! sake (CLAUDE.md).
//!
//! **[`BlockObs::source`] is the whole reason the join is not a one-liner** —
//! see [`MeasuredBlocks::obs`].

use std::collections::BTreeMap;

use crate::codegen::{BlockSpan, CodegenProgram};
use crate::placement::PlacementTable;

use super::branch::{BlockCounts, BlockObs};
use super::score::{basic_block_ranges, block_schedule_lengths_with_counts};
use super::table::CostTable;

/// A resolved Lane 2 block: its word span in the scored program and the
/// `Σ s(b)` of the emitted-word blocks that span covers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BridgedBlock {
    pub fn_key: String,
    pub block_index: u32,
    pub word_start: usize,
    pub word_end: usize,
    /// Ordinal of the first emitted-word block this span covers, indexing
    /// [`basic_block_ranges`] over the fn. The span covers exactly
    /// `first_word_block .. first_word_block + word_blocks` — contiguous,
    /// because spans tile the fn's word range and every boundary is a block
    /// leader. This is what makes a Lane 2 count attributable to the
    /// `(fn_key, word_block_index)` identity `HotBlocks` and `BlockCounts`
    /// are keyed by. 0 for an empty span (`word_blocks == 0`).
    pub first_word_block: usize,
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
        placement: &PlacementTable,
    ) -> Result<Self, String> {
        Self::build_with_counts(program, spans, table, placement, &BlockCounts::Flat)
    }

    /// [`BlockBridge::build`] with an explicit per-block frequency source
    /// for the `s(b)` each span sums.
    ///
    /// **This is the second pass of the two-pass wiring.** `BridgedBlock`'s
    /// `cycles` is `Σ s(b)` over the span, and `s(b)` itself depends on the
    /// measured counts once item H's bias-derived mispredict is live. So the
    /// correspondence is built first under [`BlockCounts::Flat`] (a
    /// measured `f` cannot be resolved before the key → word-block map
    /// exists), [`MeasuredBlocks::resolve`] is taken from that, and then the
    /// per-span `cycles` are recomputed here under
    /// [`BlockCounts::Measured`]. Only the `cycles` differ between the two
    /// passes: the partition, the span boundaries and the fail-closed checks
    /// are frequency-independent, which is why the second pass cannot move
    /// coverage.
    pub fn build_with_counts(
        program: &CodegenProgram,
        spans: &[BlockSpan],
        table: &CostTable,
        placement: &PlacementTable,
        counts: &BlockCounts<'_>,
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
            let lengths =
                block_schedule_lengths_with_counts(fn_key, &f.code, table, placement, counts)?;
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
                            first_word_block: 0,
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
                        first_word_block: first,
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
        placement: &PlacementTable,
    ) -> Result<Self, String> {
        Self::build(program, &crate::codegen::block_spans(), table, placement)
    }

    /// [`BlockBridge::from_current_codegen`] at an explicit frequency
    /// source — the second pass of the two-pass wiring.
    pub fn from_current_codegen_with_counts(
        program: &CodegenProgram,
        table: &CostTable,
        placement: &PlacementTable,
        counts: &BlockCounts<'_>,
    ) -> Result<Self, String> {
        Self::build_with_counts(
            program,
            &crate::codegen::block_spans(),
            table,
            placement,
            counts,
        )
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

// ---------------------------------------------------------------------------
// The join: a measured vector -> item F's hotness and item H's bias
// ---------------------------------------------------------------------------

/// A measured `<fn_key>#<block_index>` vector resolved onto the scored
/// program's **emitted-word** blocks: the single source both
/// [`super::footprint::HotBlocks::Measured`] and
/// [`super::branch::BlockCounts::Measured`] are built from.
///
/// ## Why one type feeds both
///
/// Item F asks `is_hot(fn_key, word_block)`, item H asks
/// `obs(fn_key, word_block)`. Both are the identity
/// [`basic_block_ranges`] indexes and the identity a Lane 2 span resolves
/// to, so there is exactly one attribution to get right and it is done
/// once, here.
///
/// ## What is *not* measured is not charged, and not hot
///
/// A word block with no observation yields `None` from [`Self::obs`] and
/// `false` from [`Self::is_hot`]. That is decision 1609 read honestly (no
/// data, no charge) rather than a default: at item C's measured 13.4%
/// block coverage a 0.5-ratio default would put the full mispredict
/// penalty on ~87% of branches, and a "hot by default" would make the
/// measured footprint identical to the flat one.
///
/// **The direction is recorded rather than hidden:** treating an unmeasured
/// block as cold is an *under*-cost of hot text, so the measured per-core
/// budget is a floor, not a bound. Decision 1617 already tells item J to
/// read its veto off the flat row (`HotBlocks::All`, every block hot) until
/// coverage improves; the measured budget is the diagnostic beside it, not
/// a replacement for it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MeasuredBlocks {
    /// fn key → word-block ordinal → observation.
    obs: BTreeMap<String, BTreeMap<usize, BlockObs>>,
    /// Sidecar keys that resolved to a scored Lane 2 span.
    pub resolved_keys: u64,
    /// Sidecar keys naming a fn the scored closure does not contain. Not an
    /// error (see [`Resolved::UnknownFn`]); `compose` charges them at the
    /// program maximum and this join contributes no observation for them.
    pub unresolved_keys: u64,
    /// Emitted-word blocks that carry an observation.
    pub measured_word_blocks: u64,
    /// Of those, the ones whose count is non-zero — the hot-text set.
    pub hot_word_blocks: u64,
}

impl MeasuredBlocks {
    /// Resolve `counts` against `bridge`.
    ///
    /// Fails closed exactly where [`BlockBridge::lookup`] does: a malformed
    /// key, or a block index out of range for a fn that **is** scored
    /// (decision 1608). A key naming an unscored fn is counted in
    /// `unresolved_keys` and contributes nothing — dropping it here does
    /// **not** narrow the sidecar, because `compose` still charges the same
    /// key at the program maximum and freeze 1627 keeps the coverage
    /// denominator the whole scored set. This join changes no coverage
    /// number at all; it only decides which `s(b)` the resolved part is
    /// multiplied by and which blocks are hot text.
    pub fn resolve(
        bridge: &BlockBridge,
        counts: &BTreeMap<String, u64>,
    ) -> Result<MeasuredBlocks, String> {
        // The `source` id: the Lane 2 span's own ordinal in the bridge, and
        // deliberately **not** anything derived from the word block. See
        // `obs` for why this is the load-bearing line of the whole join.
        let source_of: BTreeMap<&str, u64> = bridge
            .blocks()
            .enumerate()
            .map(|(i, (k, _))| (k.as_str(), i as u64))
            .collect();

        let mut out = MeasuredBlocks::default();
        for (key, &count) in counts {
            match bridge.lookup(key)? {
                Resolved::Block(b) => {
                    out.resolved_keys += 1;
                    let source = *source_of.get(key.as_str()).ok_or_else(|| {
                        format!(
                            "bridge: key `{key}` resolved to a span the bridge does not \
                             enumerate (the two views of one bridge disagree)"
                        )
                    })?;
                    let per_fn = out.obs.entry(b.fn_key.clone()).or_default();
                    let lo = b.first_word_block;
                    let hi = lo + b.word_blocks as usize;
                    for wb in lo..hi {
                        if per_fn.insert(wb, BlockObs::new(count, source)).is_some() {
                            return Err(format!(
                                "bridge: fn `{}` word block {wb} is claimed by two Lane 2 \
                                 spans — the partitions disagree (decision 1608)",
                                b.fn_key
                            ));
                        }
                        out.measured_word_blocks += 1;
                        if count > 0 {
                            out.hot_word_blocks += 1;
                        }
                    }
                }
                Resolved::UnknownFn => out.unresolved_keys += 1,
            }
        }
        Ok(out)
    }

    /// Feeds [`super::footprint::HotBlocks::Measured`]: a block is hot text
    /// when its measured `f` is non-zero. Unmeasured is **not** hot (see the
    /// type doc).
    pub fn is_hot(&self, fn_key: &str, word_block: usize) -> bool {
        self.obs(fn_key, word_block).is_some_and(|o| o.count > 0)
    }

    /// Feeds [`super::branch::BlockCounts::Measured`].
    ///
    /// ## `source` is the Lane 2 span, never the word block
    ///
    /// This is the guard item H's model rests on, and getting it wrong
    /// inverts that item's headline finding rather than merely perturbing a
    /// number.
    ///
    /// Lane 2 ids are assigned over MWIR blocks; `s(b)` is computed over
    /// emitted-word blocks; **one MWIR block spans several word blocks**. So
    /// a conditional branch whose target and fallthrough both land inside a
    /// single Lane 2 span has *one* measurement behind both successors. If
    /// `source` were the word-block ordinal the two would look distinct, the
    /// ratio would read `count:count` — a perfect 50/50 — and the branch
    /// would be charged the **full** mispredict penalty. Every abort branch
    /// wrela emits has exactly that shape, so the emit-every-check doctrine
    /// would be priced at maximum mispredict instead of the zero item H
    /// measured.
    ///
    /// Setting `source` to the span's identity makes
    /// [`super::branch::BranchBias::from_observations`] see one datum and
    /// return `None`, which charges nothing.
    /// `unit:measured_blocks_source_is_the_span_so_an_abort_branch_charges_zero`
    /// fails if this line is ever changed to a word-block-derived id.
    pub fn obs(&self, fn_key: &str, word_block: usize) -> Option<BlockObs> {
        self.obs.get(fn_key)?.get(&word_block).copied()
    }

    /// Scored fns carrying at least one measured block.
    pub fn fn_count(&self) -> u64 {
        self.obs.len() as u64
    }

    /// Whether any observation resolved at all. A vector that resolves to
    /// nothing must behave exactly like the flat row rather than like a
    /// measurement of zero.
    pub fn is_empty(&self) -> bool {
        self.measured_word_blocks == 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::codegen::CodegenFn;
    use crate::cost::branch::{BranchTerms, branch_mispredict_charge};
    use crate::cost::rule::{CostRule, EmittedWord};
    use crate::cost::sweep::SweepPoint;

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
            ..Default::default()
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
        let bridge = BlockBridge::build(
            &prog,
            &[span(0, 0, 2), span(1, 2, 4)],
            &t,
            &PlacementTable::default(),
        )
        .expect("bridge");
        let b0 = match bridge.lookup("F.m#0").expect("lookup") {
            Resolved::Block(b) => b.clone(),
            other => panic!("expected a block, got {other:?}"),
        };
        assert_eq!((b0.word_start, b0.word_end, b0.word_blocks), (0, 2, 1));
        let all: u64 = crate::cost::block_schedule_lengths(
            "F.m",
            &prog.fns["F.m"].code,
            &t,
            &PlacementTable::default(),
        )
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
        let bridge = BlockBridge::build(&prog, &[span(0, 0, 4)], &t, &PlacementTable::default())
            .expect("bridge");
        let lens = crate::cost::block_schedule_lengths(
            "F.m",
            &prog.fns["F.m"].code,
            &t,
            &PlacementTable::default(),
        )
        .unwrap();
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
        let err = BlockBridge::build(
            &prog,
            &[span(0, 0, 1), span(1, 1, 4)],
            &table(),
            &PlacementTable::default(),
        )
        .expect_err("mid-block span start must fail closed");
        assert!(
            err.contains("not an emitted-word block leader"),
            "got: {err}"
        );
    }

    #[test]
    fn fail_closed_when_spans_do_not_tile_the_fn() {
        let prog = program(vec![word(CostRule::Alu), branch(), word(CostRule::Alu)]);
        let err = BlockBridge::build(
            &prog,
            &[span(0, 0, 2)],
            &table(),
            &PlacementTable::default(),
        )
        .expect_err("a short tiling must fail closed");
        assert!(err.contains("must tile"), "got: {err}");

        let err = BlockBridge::build(
            &prog,
            &[span(0, 0, 2), span(1, 3, 3)],
            &table(),
            &PlacementTable::default(),
        )
        .expect_err("a gap must fail closed");
        assert!(err.contains("must tile"), "got: {err}");
    }

    #[test]
    fn fail_closed_on_a_span_naming_an_unscored_fn() {
        let prog = program(vec![word(CostRule::Alu)]);
        let mut s = span(0, 0, 1);
        s.fn_key = "Other.m".to_string();
        let err = BlockBridge::build(&prog, &[s], &table(), &PlacementTable::default())
            .expect_err("unknown fn");
        assert!(err.contains("does not contain"), "got: {err}");
    }

    #[test]
    fn lookup_fails_closed_on_an_out_of_range_block_index() {
        let prog = program(vec![word(CostRule::Alu)]);
        let bridge = BlockBridge::build(
            &prog,
            &[span(0, 0, 1)],
            &table(),
            &PlacementTable::default(),
        )
        .expect("bridge");
        let err = bridge.lookup("F.m#7").expect_err("out of range");
        assert!(err.contains("out of range"), "got: {err}");
    }

    /// An unknown *fn* is not an error — it is an uncovered hit. The two
    /// closures genuinely differ (module doc / `Resolved::UnknownFn`).
    #[test]
    fn lookup_reports_an_unknown_fn_as_uncovered_not_an_error() {
        let prog = program(vec![word(CostRule::Alu)]);
        let bridge = BlockBridge::build(
            &prog,
            &[span(0, 0, 1)],
            &table(),
            &PlacementTable::default(),
        )
        .expect("bridge");
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

    /// **B4's own bridge oracle** — the exact thing M20 decision 1608 was
    /// protecting, asked of the transform that broke it the first time
    /// (plans/codegen-pareto-B.md B4, landed by
    /// plans/codegen-pareto-2.md item L, decision 1973).
    ///
    /// Item B's revert message was
    /// `` bridge: fn `Ledger.mark` block 0 ends at word 42 which is not an
    /// emitted-word block leader ``. The landed form must produce **no**
    /// such error, and it must do so while actually deleting words —
    /// otherwise this passes for the boring reason.
    ///
    /// What is asserted is block **identity**, not merely that the build
    /// succeeded: the same Lane 2 keys resolve, with the same ordinals and
    /// the same fns, before and after. Every Lane 2 block still means
    /// exactly what it meant; the emitted-word partition underneath it is
    /// one block coarser per fn, and that merge lands inside the final
    /// span, whose `word_end` is `code_len` either way.
    #[test]
    fn an_elided_branch_chain_still_resolves_its_lane_2_block_identity() {
        use crate::opts::{OptId, RELEASE_OPTS, apply_opts};

        let without: Vec<OptId> = RELEASE_OPTS
            .iter()
            .copied()
            .filter(|o| *o != OptId::BranchCleanup)
            .collect();
        let t = table();

        let build = |opts: &[OptId], case: &str| {
            apply_opts(opts);
            crate::codegen::set_block_bridge(true);
            let prog = crate::cost::codegen_cost_stage(&corpus(case)).expect("cost-stage codegen");
            let spans = crate::codegen::block_spans();
            crate::codegen::set_block_bridge(false);
            let bridge = BlockBridge::build(&prog, &spans, &t, &PlacementTable::default())
                .unwrap_or_else(|e| panic!("{case}: bridge must agree: {e}"));
            let words: usize = prog.fns.values().map(|f| f.code.len()).sum();
            let word_blocks: u64 = prog
                .fns
                .values()
                .map(|f| basic_block_ranges(&f.code).len() as u64)
                .sum();
            (bridge, words, word_blocks)
        };

        for case in ["boot-actors", "cost-runtime", "boot-hello"] {
            let (off, off_words, off_wb) = build(&without, case);
            let (on, on_words, on_wb) = build(RELEASE_OPTS, case);

            // Not vacuous: B4 really deleted branch words here.
            assert!(
                on_words < off_words,
                "{case}: B4 deleted nothing, so this oracle proves nothing \
                 ({off_words} vs {on_words} words)"
            );
            // And the emitted-word partition really did get coarser — the
            // merge decision 1608 fails closed on, happening.
            assert!(
                on_wb < off_wb,
                "{case}: no emitted-word blocks merged ({off_wb} vs {on_wb}), \
                 so the disagreement this rule guards is not being exercised"
            );

            // The Lane 2 identity is untouched, key for key.
            let keys_off: Vec<&String> = off.blocks().map(|(k, _)| k).collect();
            let keys_on: Vec<&String> = on.blocks().map(|(k, _)| k).collect();
            assert_eq!(
                keys_off, keys_on,
                "{case}: B4 changed which Lane 2 blocks exist"
            );
            assert_eq!(
                off.block_count, on.block_count,
                "{case}: Lane 2 block count moved"
            );
            for ((k, b_off), (_, b_on)) in off.blocks().zip(on.blocks()) {
                assert_eq!(
                    (b_off.fn_key.as_str(), b_off.block_index),
                    (b_on.fn_key.as_str(), b_on.block_index),
                    "{case}: block `{k}` changed identity"
                );
            }
        }
        crate::opts::apply_mode(crate::opts::CompileMode::Release);
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
            let bridge = BlockBridge::build(&prog, &spans, &t, &PlacementTable::default())
                .unwrap_or_else(|e| panic!("{case}: bridge must agree: {e}"));
            assert!(
                bridge.block_count > 0,
                "{case}: bridge must carry blocks (an empty bridge proves nothing)"
            );
            assert!(
                bridge.covered_word_blocks >= bridge.block_count,
                "{case}: a Lane 2 block spans one or more word blocks"
            );
            let report =
                crate::cost::score_program(&prog, &t, &crate::placement::PlacementTable::default())
                    .expect("score");
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

    // --- the join: MeasuredBlocks (plans/M20.md items C + F + H) ----------

    /// A real `CBZ` word: `byte_offset` is relative to the branch word.
    fn cbz(byte_offset: i32) -> EmittedWord {
        EmittedWord::new(
            crate::encode::enc_cbz(0, byte_offset, true),
            "cbz".to_string(),
            CostRule::Branch,
            None,
            &[0],
        )
    }

    fn alu() -> EmittedWord {
        EmittedWord::new(0, "alu".to_string(), CostRule::Alu, Some(1), &[0, 0])
    }

    /// The **abort shape**: a conditional branch whose target and
    /// fallthrough are two different emitted-word blocks.
    ///
    /// words: 0 alu | 1 cbz .+8 | 2 alu (fallthrough) | 3 alu (target) | 4 alu
    /// emitted-word blocks: (0,2) (2,3) (3,5) — the branch is the last word
    /// of block 0, its fallthrough is block 1 and its target is block 2.
    fn abort_shape() -> CodegenProgram {
        program(vec![alu(), cbz(8), alu(), alu(), alu()])
    }

    fn point() -> SweepPoint {
        SweepPoint::pinned(&table())
    }

    fn penalty() -> u64 {
        table()
            .branch_row("mispredict_penalty")
            .expect("[branch.mispredict_penalty]")
            .value
    }

    /// The mispredict charged to the one branch in `prog`'s single fn under
    /// `counts`.
    fn charge(prog: &CodegenProgram, counts: &BlockCounts<'_>) -> u64 {
        let code = &prog.fns["F.m"].code;
        let t = table();
        let terms = BranchTerms::compute("F.m", code, &t, &point(), counts).expect("terms");
        assert_eq!(terms.summary.branches, 1, "the shape must carry one branch");
        branch_mispredict_charge(penalty(), terms.bias_at(1))
    }

    /// **The `source` guard** (plans/M20.md item H's load-bearing clause).
    ///
    /// One MWIR block spans several emitted-word blocks, so an abort
    /// branch's target and fallthrough routinely sit inside a **single**
    /// Lane 2 span. Both then carry one measurement, which is one datum and
    /// not a ratio. If `MeasuredBlocks::obs` derived `source` from the word
    /// block instead of the span, the two would read as a measured 1000:1000
    /// — a perfect 50/50 — and the branch would be charged the **full**
    /// penalty. Every abort branch has that shape, so item H's headline
    /// finding (emit-every-check costs ~0 mispredict) would invert.
    ///
    /// The control at the bottom is what makes this unit an oracle rather
    /// than a tautology: the same program, same counts, with a
    /// word-block-derived source, is charged the full 14.
    #[test]
    fn measured_blocks_source_is_the_span_so_an_abort_branch_charges_zero() {
        let prog = abort_shape();
        let t = table();
        // One Lane 2 span over the whole fn: all three word blocks share it.
        let bridge = BlockBridge::build(&prog, &[span(0, 0, 5)], &t, &PlacementTable::default())
            .expect("bridge");
        let counts = BTreeMap::from([("F.m#0".to_string(), 1000u64)]);
        let mb = MeasuredBlocks::resolve(&bridge, &counts).expect("resolve");

        assert_eq!(mb.measured_word_blocks, 3, "one span, three word blocks");
        let (a, b, c) = (
            mb.obs("F.m", 0).expect("block 0"),
            mb.obs("F.m", 1).expect("block 1"),
            mb.obs("F.m", 2).expect("block 2"),
        );
        assert_eq!((a.count, b.count, c.count), (1000, 1000, 1000));
        assert_eq!(
            (a.source, b.source, c.source),
            (a.source, a.source, a.source),
            "every word block inside one Lane 2 span reports the SAME source — \
             it is one measurement, not three"
        );

        let obs_fn = |k: &str, w: usize| mb.obs(k, w);
        let measured = BlockCounts::Measured(&obs_fn);
        let terms = BranchTerms::compute("F.m", &prog.fns["F.m"].code, &t, &point(), &measured)
            .expect("terms");
        assert_eq!(terms.summary.biased, 0, "one datum is not a ratio");
        assert_eq!(terms.summary.no_data, 1);
        assert!(terms.bias_at(1).is_none());
        assert_eq!(
            charge(&prog, &measured),
            0,
            "an abort-shaped branch whose successors share one Lane 2 span must be \
             charged ZERO mispredict, not the full penalty"
        );

        // The control: derive `source` from the word block instead and the
        // same measurement becomes a fabricated 50/50 at the full penalty.
        // This is the bug the assertion above exists to catch.
        let by_word_block = |_: &str, w: usize| Some(BlockObs::new(1000, w as u64));
        assert_eq!(
            charge(&prog, &BlockCounts::Measured(&by_word_block)),
            penalty(),
            "a word-block-derived source would charge the full penalty — so the zero \
             above is the guard working, not an absent term"
        );

        // And end to end through `s(b)`: the span-sourced schedule is the
        // flat one, the word-block-sourced one is a whole penalty longer.
        let sum = |counts: &BlockCounts<'_>| -> u64 {
            block_schedule_lengths_with_counts(
                "F.m",
                &prog.fns["F.m"].code,
                &t,
                &PlacementTable::default(),
                counts,
            )
            .expect("schedule")
            .iter()
            .sum()
        };
        let flat = sum(&BlockCounts::Flat);
        assert_eq!(sum(&measured), flat);
        assert_eq!(
            sum(&BlockCounts::Measured(&by_word_block)),
            flat + penalty()
        );
    }

    /// Two **different** Lane 2 spans are two measurements, so the ratio is
    /// real: 99/1 costs ~0 and 50/50 costs the whole penalty.
    #[test]
    fn successors_in_different_spans_give_a_real_ratio() {
        let prog = abort_shape();
        let t = table();
        // span 0 covers word blocks 0-1 (branch + fallthrough), span 1
        // covers word block 2 (the target).
        let spans = [span(0, 0, 3), span(1, 3, 5)];
        let bridge =
            BlockBridge::build(&prog, &spans, &t, &PlacementTable::default()).expect("bridge");

        let at = |taken: u64, not_taken: u64| -> u64 {
            let counts = BTreeMap::from([
                ("F.m#0".to_string(), not_taken),
                ("F.m#1".to_string(), taken),
            ]);
            let mb = MeasuredBlocks::resolve(&bridge, &counts).expect("resolve");
            assert_ne!(
                mb.obs("F.m", 1).unwrap().source,
                mb.obs("F.m", 2).unwrap().source,
                "two spans are two measurements"
            );
            let f = |k: &str, w: usize| mb.obs(k, w);
            charge(&prog, &BlockCounts::Measured(&f))
        };

        let lopsided = at(99, 1);
        assert!(
            lopsided <= 1,
            "a 99/1 branch must cost ~0 of {}, got {lopsided}",
            penalty()
        );
        assert_eq!(
            at(50, 50),
            penalty(),
            "a measured 50/50 pays the whole thing"
        );
        assert_eq!(at(1_000_000, 0), 0, "never taken is exactly zero");
    }

    /// Item F's hot-text predicate under a measured `f`: `f > 0` is hot,
    /// `f = 0` is not, and **unmeasured is not hot either** — no data, no
    /// default hotness (decision 1609; the direction is recorded on
    /// `MeasuredBlocks` itself).
    #[test]
    fn a_block_with_zero_f_is_not_hot_text_and_an_unmeasured_one_is_not_hot_by_default() {
        let prog = abort_shape();
        let t = table();
        let bridge = BlockBridge::build(
            &prog,
            &[span(0, 0, 3), span(1, 3, 5)],
            &t,
            &PlacementTable::default(),
        )
        .expect("bridge");
        let counts = BTreeMap::from([("F.m#0".to_string(), 0u64), ("F.m#1".to_string(), 7u64)]);
        let mb = MeasuredBlocks::resolve(&bridge, &counts).expect("resolve");

        assert!(!mb.is_hot("F.m", 0), "f = 0 is not hot text");
        assert!(!mb.is_hot("F.m", 1), "f = 0 is not hot text");
        assert!(mb.is_hot("F.m", 2), "f > 0 is hot text");
        assert!(!mb.is_hot("F.m", 99), "an unmeasured block is not hot");
        assert!(!mb.is_hot("Other.m", 0), "an unmeasured fn is not hot");
        assert_eq!((mb.measured_word_blocks, mb.hot_word_blocks), (3, 1));

        // `HotBlocks::All` is the opposite end and every block is hot — the
        // flat row is a static-footprint row, unchanged by this wiring.
        let all = crate::cost::HotBlocks::All;
        for w in 0..3 {
            assert!(all.is_hot("F.m", w));
        }
    }

    /// Decision 1617: the join must not "fix" coverage. An unresolvable
    /// key is **counted** and contributes no observation — `compose` still
    /// charges it at the program maximum — and an out-of-range index on a
    /// scored fn still fails closed (decision 1608).
    #[test]
    fn resolve_counts_unknown_fns_and_fails_closed_on_an_out_of_range_index() {
        let prog = abort_shape();
        let bridge = BlockBridge::build(
            &prog,
            &[span(0, 0, 5)],
            &table(),
            &PlacementTable::default(),
        )
        .expect("bridge");

        let mb = MeasuredBlocks::resolve(
            &bridge,
            &BTreeMap::from([
                ("F.m#0".to_string(), 3u64),
                ("NotScored.m#4".to_string(), 900u64),
            ]),
        )
        .expect("an unknown fn is not an error");
        assert_eq!((mb.resolved_keys, mb.unresolved_keys), (1, 1));
        assert!(
            !mb.is_hot("NotScored.m", 0),
            "an unresolvable key contributes no observation to this program"
        );

        let err = MeasuredBlocks::resolve(&bridge, &BTreeMap::from([("F.m#9".to_string(), 1u64)]))
            .expect_err("an out-of-range index on a scored fn must fail closed");
        assert!(err.contains("out of range"), "got: {err}");
        assert!(
            MeasuredBlocks::resolve(&bridge, &BTreeMap::from([("F.m".to_string(), 1u64)])).is_err(),
            "a malformed key is a malformed sidecar, not a miss"
        );
    }

    /// A vector that resolves to nothing must behave **exactly** like the
    /// flat row, not like a measurement of zero: no bias anywhere, and the
    /// measured-pass `s(b)` identical to the flat one.
    #[test]
    fn a_vector_that_resolves_to_nothing_is_the_flat_row() {
        let prog = abort_shape();
        let t = table();
        let bridge = BlockBridge::build(&prog, &[span(0, 0, 5)], &t, &PlacementTable::default())
            .expect("bridge");
        let mb = MeasuredBlocks::resolve(
            &bridge,
            &BTreeMap::from([("Elsewhere.m#0".to_string(), 5u64)]),
        )
        .expect("resolve");
        assert!(mb.is_empty());
        let f = |k: &str, w: usize| mb.obs(k, w);
        let measured = BlockBridge::build_with_counts(
            &prog,
            &[span(0, 0, 5)],
            &t,
            &PlacementTable::default(),
            &BlockCounts::Measured(&f),
        )
        .expect("measured bridge");
        assert_eq!(
            measured, bridge,
            "an unresolved vector must leave every s(b) exactly where the flat pass put it"
        );
    }

    /// **The census of what the join actually reaches on the committed
    /// `boot-actors` vector** — the number nobody had before, kept as a
    /// number so a collapse to "clean about nothing" is visible in the
    /// output rather than hidden behind a green test.
    #[test]
    fn boot_actors_measured_join_census() {
        let (prog, spans, _) = bridge_codegen("boot-actors");
        let t = table();
        let place = PlacementTable::default();
        let flat_bridge = BlockBridge::build(&prog, &spans, &t, &place).expect("bridge");
        let f = crate::cost::freq::load_block_from_path(
            &crate::cost::repo_root().join("tests/golden/boot-actors/lane2-freq.txt"),
        )
        .expect("committed sidecar");
        let mb = MeasuredBlocks::resolve(&flat_bridge, &f.counts).expect("resolve");

        // Resolution: unchanged from item C's pinned finding — the join
        // resolves the same 81 keys and leaves the same 291 uncovered
        // (decision 1617: do not "fix" coverage).
        assert_eq!((mb.resolved_keys, mb.unresolved_keys), (81, 291));
        // **190 -> 188 at plans/codegen-pareto.md item C2** (decision
        // 1748), and the pair of numbers above is what makes that a safe
        // move rather than a coverage loss: `MaskCheck` replaces a
        // two-constant, two-compare, **two-abort** narrow range check with
        // a one-`TST`, one-abort one, so the closure has two fewer basic
        // blocks (`all_word_blocks` 333 -> 331) and both of them happened
        // to be measured. The join still resolves the same 81 keys and
        // still leaves the same 291 unresolved — the measurement explains
        // exactly what it explained before, in two fewer blocks. Attributed
        // by ablation: with `MaskCheck` alone removed from `RELEASE_OPTS`
        // every number here returns to its M20 value.
        //
        // **188 -> 187 at plans/codegen-pareto-2.md item L** (decision
        // 1975), and this is the number that shows B4 is boundary-
        // preserving rather than boundary-breaking. `BranchCleanup`
        // deletes each fn's trailing branch to the epilogue, which merges
        // the epilogue's word block into the last body block — a merge
        // that happens *inside* the final Lane 2 span, whose `word_end` is
        // `code_len` either way. So exactly one emitted-word block
        // disappears, and **`resolved_keys` / `unresolved_keys` /
        // `fn_count` do not move at all**: the same measurement still
        // explains the same Lane 2 blocks. Had the transform merged a
        // Lane 2 boundary, `BlockBridge::build` would have refused the
        // whole build (decision 1608 rule 4) rather than shifted a count.
        assert_eq!(
            mb.measured_word_blocks, 187,
            "emitted-word blocks the measurement reaches"
        );
        assert_eq!(
            mb.hot_word_blocks, 187,
            "the committed sidecar carries only non-zero hits, so every measured block is hot"
        );
        assert_eq!(mb.fn_count(), 14, "scored fns the measurement reaches");

        // Hot text under measured `f` vs under `W_flat`, in word blocks.
        let all_word_blocks: u64 = prog
            .fns
            .values()
            .map(|f| basic_block_ranges(&f.code).len() as u64)
            .sum();
        // **312 -> 310 at plans/codegen-pareto-2.md item J** (decision
        // 1934), and the pair above is again what makes it safe: item J
        // leaves the shared runtime closure alone (decision 1932), so the
        // two blocks it removes are in `boot-actors`' own actor methods,
        // `resolved_keys`/`unresolved_keys`/`measured_word_blocks`/
        // `fn_count` do not move at all, and the same measurement still
        // explains the same Lane 2 blocks. Had item J repartitioned a
        // runtime fn the sidecar names, `MeasuredBlocks::resolve` would
        // have refused the whole join (decision 1608) rather than shifted
        // a count — which is exactly what it did before decision 1932
        // existed, on `ascii_digit#21`.
        assert_eq!(
            all_word_blocks, 310,
            "emitted-word blocks in the scored closure — every one of them is hot under W_flat              (331 before item L's B4 deleted one trailing branch per fn, decision 1975)"
        );
        assert!(
            mb.hot_word_blocks < all_word_blocks,
            "the measured vector must exclude some blocks or item F's wiring is a no-op"
        );

        // Branch census under the measured vector: how many branches a real
        // measurement can actually bias, and what that costs.
        let obs_fn = |k: &str, w: usize| mb.obs(k, w);
        let counts = BlockCounts::Measured(&obs_fn);
        let point = point();
        let (mut biased, mut no_data, mut branches, mut mispredict) = (0u64, 0u64, 0u64, 0u64);
        for (key, cf) in &prog.fns {
            let terms =
                BranchTerms::compute(key, &cf.code, &t, &point, &counts).expect("branch terms");
            branches += terms.summary.branches;
            biased += terms.summary.biased;
            no_data += terms.summary.no_data;
            for w in 0..cf.code.len() {
                mispredict +=
                    branch_mispredict_charge(point.get("mispredict_penalty"), terms.bias_at(w));
            }
        }
        // 264 -> 243 at item L: B4 deletes 21 unconditional branch words
        // from this closure, one per fn whose body ends in a `Return`.
        // **243 -> 241 at item J** (decision 1934): constant propagation
        // resolves two `JumpIfFalse`es in `boot-actors`' own actor
        // methods whose conditions are compile-time known, and DCE
        // collects the arms they orphan. Neither is a branch the measured
        // vector biases — `biased` below does not move — because item J
        // never touches the runtime closure the vector covers.
        assert_eq!(branches, 241, "branch words in the scored closure");
        assert_eq!(
            biased, 14,
            "branches a real measured vector can bias — the number item J must not mistake \
             for a fine-grained ranking signal (decision 1617)"
        );
        assert_eq!(
            no_data, 103,
            "conditional branches with no usable ratio: unmeasured, or both successors \
             inside one Lane 2 span (the `source` guard)"
        );
        assert_eq!(
            mispredict, 145,
            "total bias-derived mispredict over the scored closure, in cycles"
        );

        // The flat row over the same program charges zero mispredict — the
        // control that makes the number above a measurement rather than a
        // constant.
        let (mut flat_biased, mut flat_mispredict) = (0u64, 0u64);
        for (key, cf) in &prog.fns {
            let terms = BranchTerms::compute(key, &cf.code, &t, &point, &BlockCounts::Flat)
                .expect("branch terms");
            flat_biased += terms.summary.biased;
            for w in 0..cf.code.len() {
                flat_mispredict +=
                    branch_mispredict_charge(point.get("mispredict_penalty"), terms.bias_at(w));
            }
        }
        assert_eq!((flat_biased, flat_mispredict), (0, 0));
    }
}
