use std::collections::BTreeMap;

use crate::codegen::{BlockSpan, CodegenProgram};
use crate::placement::PlacementTable;

use super::branch::{BlockCounts, BlockObs};
use super::score::{basic_block_ranges, block_schedule_lengths_with_counts};
use super::table::CostTable;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BridgedBlock {
    pub fn_key: String,
    pub block_index: u32,
    pub word_start: usize,
    pub word_end: usize,
    pub first_word_block: usize,
    pub word_blocks: u64,
    pub cycles: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BlockBridge {
    blocks: BTreeMap<String, BridgedBlock>,
    fns_with_spans: BTreeMap<String, u32>,
    pub block_count: u64,
    pub covered_word_blocks: u64,
    pub empty_spans: u64,
}

pub fn make_key(fn_key: &str, block_index: u32) -> String {
    format!("{fn_key}#{block_index}")
}

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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Resolved<'a> {
    Block(&'a BridgedBlock),
    UnknownFn,
}

impl BlockBridge {
    pub fn build(
        program: &CodegenProgram,
        spans: &[BlockSpan],
        table: &CostTable,
        placement: &PlacementTable,
    ) -> Result<Self, String> {
        Self::build_with_counts(program, spans, table, placement, &BlockCounts::Flat)
    }

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

    pub fn from_linked(
        linked: &crate::linked::LinkedProgram,
        table: &CostTable,
        placement: &PlacementTable,
    ) -> Result<Self, String> {
        Self::from_linked_with_counts(linked, table, placement, &BlockCounts::Flat)
    }

    pub fn from_linked_with_counts(
        linked: &crate::linked::LinkedProgram,
        table: &CostTable,
        placement: &PlacementTable,
        counts: &BlockCounts<'_>,
    ) -> Result<Self, String> {
        let program = CodegenProgram {
            fns: linked
                .fns
                .iter()
                .map(|(key, function)| {
                    (
                        key.clone(),
                        crate::codegen::CodegenFn {
                            frame_size: function.frame_size as usize,
                            code: function.code.clone(),
                            relocs: Vec::new(),
                            regions: Vec::new(),
                        },
                    )
                })
                .collect(),
            rodata: Vec::new(),
            conventions: BTreeMap::new(),
            origin_spans: Vec::new(),
        };
        let spans: Vec<BlockSpan> = linked
            .fns
            .values()
            .flat_map(|function| {
                function
                    .origin_word_ranges
                    .iter()
                    .map(|&(ordinal, start, end)| BlockSpan {
                        fn_key: function.key.clone(),
                        block_index: ordinal,
                        id: ordinal,
                        word_start: start,
                        word_end: end,
                    })
            })
            .collect();
        Self::build_with_counts(&program, &spans, table, placement, counts)
    }

    pub fn from_current_codegen(
        program: &CodegenProgram,
        table: &CostTable,
        placement: &PlacementTable,
    ) -> Result<Self, String> {
        Self::build(program, &crate::codegen::block_spans(), table, placement)
    }

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

    pub fn fn_count(&self) -> u64 {
        self.fns_with_spans.len() as u64
    }

    pub fn blocks(&self) -> impl Iterator<Item = (&String, &BridgedBlock)> {
        self.blocks.iter()
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MeasuredBlocks {
    obs: BTreeMap<String, BTreeMap<usize, BlockObs>>,
    pub resolved_keys: u64,
    pub unresolved_keys: u64,
    pub measured_word_blocks: u64,
    pub hot_word_blocks: u64,
}

impl MeasuredBlocks {
    pub fn resolve(
        bridge: &BlockBridge,
        counts: &BTreeMap<String, u64>,
    ) -> Result<MeasuredBlocks, String> {
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

    pub fn is_hot(&self, fn_key: &str, word_block: usize) -> bool {
        self.obs(fn_key, word_block).is_some_and(|o| o.count > 0)
    }

    pub fn obs(&self, fn_key: &str, word_block: usize) -> Option<BlockObs> {
        self.obs.get(fn_key)?.get(&word_block).copied()
    }

    pub fn fn_count(&self) -> u64 {
        self.obs.len() as u64
    }

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
        EmittedWord::gpr(0xd503_201f, "nop".to_string(), rule, None, &[])
    }

    fn branch() -> EmittedWord {
        EmittedWord::gpr(
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
                regions: Vec::new(),
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

    #[test]
    fn a_lane2_span_covers_the_set_of_word_blocks_inside_it() {
        let prog = program(vec![
            word(CostRule::Alu),
            branch(),
            word(CostRule::Alu),
            word(CostRule::Alu),
        ]);
        let t = table();
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

    fn corpus(case: &str) -> std::path::PathBuf {
        crate::cost::repo_root().join(format!("tests/golden/{case}/input.wr"))
    }

    fn bridge_codegen(case: &str) -> (CodegenProgram, Vec<BlockSpan>, u32) {
        crate::opts::apply_mode(crate::opts::CompileMode::Release);
        crate::codegen::set_block_bridge(true);
        let prog = crate::cost::codegen_cost_stage(&corpus(case)).expect("cost-stage codegen");
        let spans = crate::codegen::block_spans();
        let ids = crate::codegen::block_ids_assigned();
        crate::codegen::set_block_bridge(false);
        (prog, spans, ids)
    }

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

            assert!(
                on_words < off_words,
                "{case}: B4 deleted nothing, so this oracle proves nothing \
                 ({off_words} vs {on_words} words)"
            );
            assert!(
                on_wb < off_wb,
                "{case}: no emitted-word blocks merged ({off_wb} vs {on_wb}), \
                 so the disagreement this rule guards is not being exercised"
            );

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

    fn cbz(byte_offset: i32) -> EmittedWord {
        EmittedWord::gpr(
            crate::encode::enc_cbz(0, byte_offset, true),
            "cbz".to_string(),
            CostRule::Branch,
            None,
            &[0],
        )
    }

    fn alu() -> EmittedWord {
        EmittedWord::gpr(0, "alu".to_string(), CostRule::Alu, Some(1), &[0, 0])
    }

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

    fn charge(prog: &CodegenProgram, counts: &BlockCounts<'_>) -> u64 {
        let code = &prog.fns["F.m"].code;
        let t = table();
        let terms = BranchTerms::compute("F.m", code, &t, &point(), counts).expect("terms");
        assert_eq!(terms.summary.branches, 1, "the shape must carry one branch");
        branch_mispredict_charge(penalty(), terms.bias_at(1))
    }

    #[test]
    fn measured_blocks_source_is_the_span_so_an_abort_branch_charges_zero() {
        let prog = abort_shape();
        let t = table();
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

        let by_word_block = |_: &str, w: usize| Some(BlockObs::new(1000, w as u64));
        assert_eq!(
            charge(&prog, &BlockCounts::Measured(&by_word_block)),
            penalty(),
            "a word-block-derived source would charge the full penalty — so the zero \
             above is the guard working, not an absent term"
        );

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

    #[test]
    fn successors_in_different_spans_give_a_real_ratio() {
        let prog = abort_shape();
        let t = table();
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

        let all = crate::cost::HotBlocks::All;
        for w in 0..3 {
            assert!(all.is_hot("F.m", w));
        }
    }

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

        assert_eq!(mb.resolved_keys + mb.unresolved_keys, f.counts.len() as u64);
        assert!(mb.resolved_keys > 0);
        assert_eq!(
            mb.hot_word_blocks, mb.measured_word_blocks,
            "the committed sidecar carries only non-zero hits"
        );
        assert!(mb.fn_count() > 0);

        let all_word_blocks: u64 = prog
            .fns
            .values()
            .map(|f| basic_block_ranges(&f.code).len() as u64)
            .sum();
        // 304 -> 300: large aggregate copies became counted loops instead of an
        // unrolled load/store pair per word, which removes straight-line words
        // from the scored closure. Re-locked against the measurement, not
        // widened to accommodate it.
        assert_eq!(
            all_word_blocks, 300,
            "emitted-word blocks in the current scored closure"
        );
        assert!(
            mb.hot_word_blocks < all_word_blocks,
            "the measured vector must exclude some blocks or item F's wiring is a no-op"
        );

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
        assert!(branches > 0);
        assert!(biased > 0 && biased <= branches);
        assert!(no_data > 0 && no_data <= branches);
        assert!(mispredict > 0);

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
