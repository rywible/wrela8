use std::collections::BTreeMap;

use super::rule::{CostRule, EmittedWord};
use super::score::{basic_block_ranges, branch_target_index};
use super::sweep::SweepPoint;
use super::table::CostTable;

pub const FRONTEND_SWEEP_DIM: &str = "range_cross_penalty";

pub const FN_ENTRY_ALIGN_BYTES: u64 = 4;

const WORD_BYTES: u64 = 4;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BlockObs {
    pub count: u64,
    pub source: u64,
}

impl BlockObs {
    pub fn new(count: u64, source: u64) -> BlockObs {
        BlockObs { count, source }
    }
}

#[derive(Clone, Copy)]
pub enum BlockCounts<'a> {
    Flat,
    Measured(&'a dyn Fn(&str, usize) -> Option<BlockObs>),
}

impl Default for BlockCounts<'_> {
    fn default() -> Self {
        BlockCounts::Flat
    }
}

impl BlockCounts<'_> {
    pub fn obs(&self, fn_key: &str, block_index: usize) -> Option<BlockObs> {
        match self {
            BlockCounts::Flat => None,
            BlockCounts::Measured(f) => f(fn_key, block_index),
        }
    }

    pub fn is_measured(&self) -> bool {
        matches!(self, BlockCounts::Measured(_))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BranchBias {
    taken: u64,
    not_taken: u64,
}

impl BranchBias {
    pub fn from_observations(taken: Option<BlockObs>, not_taken: Option<BlockObs>) -> Option<Self> {
        let t = taken?;
        let n = not_taken?;
        if t.source == n.source {
            return None;
        }
        if t.count == 0 && n.count == 0 {
            return None;
        }
        Some(BranchBias {
            taken: t.count,
            not_taken: n.count,
        })
    }

    pub fn from_distinct_counts(taken: u64, not_taken: u64) -> Option<Self> {
        Self::from_observations(
            Some(BlockObs::new(taken, 0)),
            Some(BlockObs::new(not_taken, 1)),
        )
    }

    pub fn taken(&self) -> u64 {
        self.taken
    }

    pub fn not_taken(&self) -> u64 {
        self.not_taken
    }

    pub fn total(&self) -> u64 {
        self.taken.saturating_add(self.not_taken)
    }

    pub fn unpredictability(&self) -> (u64, u64) {
        (
            2u64.saturating_mul(self.taken.min(self.not_taken)),
            self.total(),
        )
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct BranchTerms {
    bias: BTreeMap<usize, BranchBias>,
    frontend: BTreeMap<usize, u64>,
    pub summary: BranchSummary,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct BranchSummary {
    pub branches: u64,
    pub unconditional: u64,
    pub unresolved: u64,
    pub biased: u64,
    pub no_data: u64,
    pub dense_excess: u64,
    pub loop_crossings: u64,
    pub worst_residue: u64,
}

impl BranchTerms {
    pub fn flat(
        fn_key: &str,
        code: &[EmittedWord],
        table: &CostTable,
        point: &SweepPoint,
    ) -> Result<BranchTerms, String> {
        BranchTerms::compute(fn_key, code, table, point, &BlockCounts::Flat)
    }

    pub fn compute(
        fn_key: &str,
        code: &[EmittedWord],
        table: &CostTable,
        point: &SweepPoint,
        counts: &BlockCounts<'_>,
    ) -> Result<BranchTerms, String> {
        let mut out = BranchTerms::default();
        if code.is_empty() {
            return Ok(out);
        }
        out.bias = bias_per_branch(fn_key, code, counts, &mut out.summary)?;
        let fe = frontend_charges(code, table, point, &mut out.summary, 0)?;
        out.frontend = fe;
        Ok(out)
    }

    pub fn compute_at_address(
        fn_key: &str,
        code: &[EmittedWord],
        table: &CostTable,
        point: &SweepPoint,
        counts: &BlockCounts<'_>,
        fn_address: u64,
    ) -> Result<BranchTerms, String> {
        let mut out = BranchTerms::default();
        if code.is_empty() {
            return Ok(out);
        }
        out.bias = bias_per_branch(fn_key, code, counts, &mut out.summary)?;
        out.frontend = frontend_charges(code, table, point, &mut out.summary, fn_address)?;
        Ok(out)
    }

    pub fn bias_at(&self, w: usize) -> Option<BranchBias> {
        self.bias.get(&w).copied()
    }

    pub fn frontend_at(&self, w: usize) -> u64 {
        self.frontend.get(&w).copied().unwrap_or(0)
    }

    pub fn total_frontend_charge(&self) -> u64 {
        self.frontend.values().copied().sum()
    }
}

pub fn branch_mispredict_charge(penalty: u64, bias: Option<BranchBias>) -> u64 {
    let Some(b) = bias else {
        return 0;
    };
    let (num, den) = b.unpredictability();
    if den == 0 || num == 0 || penalty == 0 {
        return 0;
    }
    let scaled = (u128::from(penalty) * u128::from(num)).div_ceil(u128::from(den));
    u64::try_from(scaled).unwrap_or(u64::MAX)
}

fn is_unconditional_b(word: u32) -> bool {
    word & 0xFC00_0000 == 0x1400_0000
}

const BR_REG_MASK: u32 = 0xFFFF_FC1F;
const BR_REG_BR: u32 = 0xD61F_0000;
const BR_REG_BLR: u32 = 0xD63F_0000;

fn is_indirect_branch_register(word: u32) -> bool {
    let masked = word & BR_REG_MASK;
    masked == BR_REG_BR || masked == BR_REG_BLR
}

fn block_of(ranges: &[(usize, usize)], n: usize) -> Vec<usize> {
    let mut out = vec![0usize; n];
    for (k, (start, end)) in ranges.iter().enumerate() {
        for w in *start..*end {
            out[w] = k;
        }
    }
    out
}

fn bias_per_branch(
    fn_key: &str,
    code: &[EmittedWord],
    counts: &BlockCounts<'_>,
    summary: &mut BranchSummary,
) -> Result<BTreeMap<usize, BranchBias>, String> {
    let n = code.len();
    let ranges = basic_block_ranges(code);
    let block = block_of(&ranges, n);
    let mut out = BTreeMap::new();
    for (start, end) in &ranges {
        let _ = start;
        let i = end.saturating_sub(1);
        if code[i].rule != CostRule::Branch {
            continue;
        }
        summary.branches += 1;
        if is_unconditional_b(code[i].word) {
            summary.unconditional += 1;
            continue;
        }
        if is_indirect_branch_register(code[i].word) {
            return Err(format!(
                "{fn_key}: word {i} is a computed branch (`BR`/`BLR`, {:#010x}); \
                 no source prices one, so its mispredict charge is undecided",
                code[i].word
            ));
        }
        let fallthrough = i + 1;
        let Some(target) = branch_target_index(code[i].word, i) else {
            summary.unresolved += 1;
            continue;
        };
        if target >= n || fallthrough >= n {
            summary.unresolved += 1;
            continue;
        }
        if block[target] == block[fallthrough] {
            summary.no_data += 1;
            continue;
        }
        match BranchBias::from_observations(
            counts.obs(fn_key, block[target]),
            counts.obs(fn_key, block[fallthrough]),
        ) {
            Some(b) => {
                summary.biased += 1;
                out.insert(i, b);
            }
            None => summary.no_data += 1,
        }
    }
    Ok(out)
}

fn fetch_region_bytes(table: &CostTable) -> Result<u64, String> {
    let row = |k: &str| -> Result<u64, String> {
        table
            .branch_row(k)
            .map(|r| r.value)
            .ok_or_else(|| format!("cost table: [branch.{k}] is required by SOG §4.8's terms"))
    };
    let target = row("target_align_bytes")?;
    let entry = row("entry_align_bytes")?;
    let loop_fit = row("loop_fit_bytes")?;
    if target != entry || entry != loop_fit {
        return Err(format!(
            "cost table: SOG §4.8's fetch region is one size, but [branch] says \
             target_align_bytes={target} entry_align_bytes={entry} loop_fit_bytes={loop_fit}"
        ));
    }
    if target == 0 || target % WORD_BYTES != 0 {
        return Err(format!(
            "cost table: [branch] fetch region {target} is not a positive multiple of {WORD_BYTES}"
        ));
    }
    Ok(target)
}

fn frontend_charges(
    code: &[EmittedWord],
    table: &CostTable,
    point: &SweepPoint,
    summary: &mut BranchSummary,
    fn_address: u64,
) -> Result<BTreeMap<usize, u64>, String> {
    let region = fetch_region_bytes(table)?;
    let max_branches = table
        .branch_row("max_branches_per_32b")
        .map(|r| r.value)
        .ok_or_else(|| {
            "cost table: [branch.max_branches_per_32b] is required by SOG §4.8's density rule"
                .to_string()
        })?;
    let loop_fit = region;
    let penalty = point.get(FRONTEND_SWEEP_DIM);

    let branches: Vec<usize> = (0..code.len())
        .filter(|&i| is_branch_class(code[i].rule))
        .collect();
    let mut loops: Vec<(usize, usize)> = Vec::new();
    for &i in branches
        .iter()
        .filter(|&&i| code[i].rule == CostRule::Branch)
    {
        if let Some(t) = branch_target_index(code[i].word, i) {
            if t <= i && (i - t + 1) as u64 * WORD_BYTES <= loop_fit {
                loops.push((i, t));
            }
        }
    }

    let (residue, excess, crossings) = if fn_address == 0 {
        // Closure-only callers have no final address.  Preserve the old
        // stress diagnostic there; linked scoring always supplies an address.
        let residues = region / FN_ENTRY_ALIGN_BYTES;
        let mut best: Option<(u64, u64, u64)> = None;
        for step in 0..residues {
            let r = step * FN_ENTRY_ALIGN_BYTES;
            let excess = density_excess(&branches, r, region, max_branches);
            let crossings = loop_crossings(&loops, r, region);
            let total = excess.saturating_add(crossings);
            let better = match best {
                None => true,
                Some((_, e, c)) => total > e.saturating_add(c),
            };
            if better {
                best = Some((r, excess, crossings));
            }
        }
        best.unwrap_or((0, 0, 0))
    } else {
        let residue = fn_address % region;
        (
            residue,
            density_excess_at(&branches, fn_address, region, max_branches),
            loop_crossings_at(&loops, fn_address, region),
        )
    };
    summary.worst_residue = residue;
    summary.dense_excess = excess;
    summary.loop_crossings = crossings;

    let mut out: BTreeMap<usize, u64> = BTreeMap::new();
    let mut per_region: BTreeMap<u64, Vec<usize>> = BTreeMap::new();
    for &i in &branches {
        per_region
            .entry(region_of(i, residue, region))
            .or_default()
            .push(i);
    }
    for (_, members) in per_region {
        let over = (members.len() as u64).saturating_sub(max_branches);
        if over > 0 {
            let last = *members.last().expect("non-empty region bucket");
            *out.entry(last).or_insert(0) += over.saturating_mul(penalty);
        }
    }
    for &(i, t) in &loops {
        if region_of(i, residue, region) != region_of(t, residue, region) {
            *out.entry(i).or_insert(0) += penalty;
        }
    }
    Ok(out)
}

fn region_of(w: usize, r: u64, region: u64) -> u64 {
    (r + w as u64 * WORD_BYTES) / region
}

pub fn is_branch_class(rule: CostRule) -> bool {
    matches!(
        rule,
        CostRule::Branch | CostRule::Call | CostRule::Abort | CostRule::AbortVal
    )
}

fn density_excess(branches: &[usize], r: u64, region: u64, max_branches: u64) -> u64 {
    let mut per: BTreeMap<u64, u64> = BTreeMap::new();
    for &i in branches {
        *per.entry(region_of(i, r, region)).or_insert(0) += 1;
    }
    per.values().map(|&c| c.saturating_sub(max_branches)).sum()
}

fn loop_crossings(loops: &[(usize, usize)], r: u64, region: u64) -> u64 {
    loops
        .iter()
        .filter(|(i, t)| region_of(*i, r, region) != region_of(*t, r, region))
        .count() as u64
}

fn density_excess_at(branches: &[usize], address: u64, region: u64, max_branches: u64) -> u64 {
    let mut per: BTreeMap<u64, u64> = BTreeMap::new();
    for &i in branches {
        *per.entry((address + (i as u64) * WORD_BYTES) / region)
            .or_insert(0) += 1;
    }
    per.values().map(|&c| c.saturating_sub(max_branches)).sum()
}

fn loop_crossings_at(loops: &[(usize, usize)], address: u64, region: u64) -> u64 {
    loops
        .iter()
        .filter(|(i, t)| {
            (address + (*i as u64) * WORD_BYTES) / region
                != (address + (*t as u64) * WORD_BYTES) / region
        })
        .count() as u64
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::*;
    use crate::codegen::{CodegenFn, CodegenProgram};
    use crate::cost::footprint::HotBlocks;
    use crate::cost::rule::EmittedWord;
    use crate::cost::score::{CostReport, score_program_at_with_hot};
    use crate::cost::table::{CostTable, load_default};
    use crate::encode;
    use crate::placement::PlacementTable;

    fn table() -> CostTable {
        load_default().expect("bench/a76-pi5.toml")
    }

    fn placement() -> PlacementTable {
        PlacementTable {
            entries: Vec::new(),
            cores: 0,
        }
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

    fn frontend_penalty() -> u64 {
        point().get(FRONTEND_SWEEP_DIM)
    }

    fn alu(dst: u8) -> EmittedWord {
        EmittedWord::gpr(0, String::new(), CostRule::Alu, Some(dst), &[0, 0])
    }

    fn bl_rule(rule: CostRule) -> EmittedWord {
        EmittedWord::gpr(encode::enc_bl(0), String::new(), rule, None, &[])
    }

    fn cbz(byte_offset: i32) -> EmittedWord {
        EmittedWord::gpr(
            encode::enc_cbz(0, byte_offset, true),
            String::new(),
            CostRule::Branch,
            None,
            &[0],
        )
    }

    fn b(byte_offset: i32) -> EmittedWord {
        EmittedWord::gpr(
            encode::enc_b(byte_offset),
            String::new(),
            CostRule::Branch,
            None,
            &[],
        )
    }

    fn prog(key: &str, code: Vec<EmittedWord>) -> CodegenProgram {
        let mut fns = BTreeMap::new();
        fns.insert(
            key.to_string(),
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

    fn score_flat(p: &CodegenProgram) -> CostReport {
        let t = table();
        score_program_at_with_hot(
            p,
            &t,
            &placement(),
            &SweepPoint::pinned(&t),
            HotBlocks::All,
            BlockCounts::Flat,
        )
        .expect("score")
    }

    fn terms(code: &[EmittedWord], counts: &BlockCounts<'_>) -> BranchTerms {
        let t = table();
        BranchTerms::compute("F.m", code, &t, &SweepPoint::pinned(&t), counts).expect("terms")
    }

    #[test]
    fn a_99_to_1_branch_is_charged_about_zero_and_a_50_50_the_full_penalty() {
        let p = penalty();
        assert_eq!(p, 14, "the profile pins the pessimistic end of 11-14");
        let biased = BranchBias::from_distinct_counts(99, 1).expect("bias");
        let even = BranchBias::from_distinct_counts(50, 50).expect("bias");
        let lopsided = branch_mispredict_charge(p, Some(biased));
        let coin = branch_mispredict_charge(p, Some(even));
        assert_eq!(coin, p, "a 50/50 branch pays the whole penalty");
        assert!(
            lopsided <= 1,
            "a 99/1 branch must be charged ~0, got {lopsided} of {p}"
        );
        assert!(lopsided < coin);
        assert_eq!(
            branch_mispredict_charge(p, BranchBias::from_distinct_counts(0, 1000)),
            0
        );
        assert_eq!(
            branch_mispredict_charge(p, BranchBias::from_distinct_counts(1000, 0)),
            0
        );
    }

    #[test]
    fn the_charge_is_symmetric_and_monotone_in_unpredictability() {
        let p = penalty();
        let mut prev = 0u64;
        for taken in 0..=50u64 {
            let c =
                branch_mispredict_charge(p, BranchBias::from_distinct_counts(taken, 100 - taken));
            assert!(c >= prev, "charge fell as the branch got less predictable");
            assert_eq!(
                c,
                branch_mispredict_charge(p, BranchBias::from_distinct_counts(100 - taken, taken)),
                "the charge must not depend on which arm is the majority"
            );
            prev = c;
        }
        assert_eq!(prev, p, "the 50/50 end is the full penalty");
    }

    #[test]
    fn an_abort_branch_at_one_in_a_million_is_charged_about_zero() {
        let p = penalty();
        let bias = BranchBias::from_distinct_counts(1, 1_000_000).expect("bias");
        let charge = branch_mispredict_charge(p, Some(bias));
        assert!(
            charge <= 1,
            "an always-not-taken check branch must cost ~0 mispredict, got {charge} of {p}"
        );
        for penalty in [11u64, 14] {
            assert_eq!(
                branch_mispredict_charge(penalty, BranchBias::from_distinct_counts(0, 1_000_000)),
                0
            );
        }
    }

    #[test]
    fn no_data_charges_zero_and_is_structurally_distinct_from_a_measured_half() {
        let p = penalty();
        assert_eq!(branch_mispredict_charge(p, None), 0);
        assert_eq!(
            branch_mispredict_charge(p, BranchBias::from_distinct_counts(1, 1)),
            p
        );
        let flat = BlockCounts::Flat;
        assert!(!flat.is_measured());
        for block in 0..8 {
            assert!(flat.obs("F.m", block).is_none());
        }
        let one = BlockObs::new(7, 3);
        assert!(BranchBias::from_observations(Some(one), Some(one)).is_none());
        assert!(
            BranchBias::from_observations(Some(BlockObs::new(7, 3)), Some(BlockObs::new(7, 4)))
                .is_some(),
            "two distinct measurements of 7 and 7 are a real 50/50"
        );
        assert!(BranchBias::from_observations(Some(one), None).is_none());
        assert!(BranchBias::from_observations(None, Some(one)).is_none());
    }

    #[test]
    fn the_flat_row_charges_zero_mispredict() {
        let code = vec![cbz(8), alu(1), alu(2)];
        let flat = terms(&code, &BlockCounts::Flat);
        assert_eq!(flat.summary.branches, 1);
        assert_eq!(flat.summary.biased, 0);
        assert_eq!(flat.summary.no_data, 1);
        assert!(flat.bias_at(0).is_none());

        let per_block = |_: &str, b: usize| Some(BlockObs::new(1, b as u64));
        let measured = terms(&code, &BlockCounts::Measured(&per_block));
        assert_eq!(measured.summary.biased, 1);
        let bias = measured.bias_at(0).expect("measured bias");
        assert_eq!(branch_mispredict_charge(penalty(), Some(bias)), penalty());

        let flat_total = score_flat(&prog("F.m", code)).total_proxy_cycles;
        assert!(flat_total > 0);
        assert_eq!(
            flat_total,
            score_flat(&prog("F.m", vec![cbz(8), alu(1), alu(2)])).total_proxy_cycles
        );
    }

    #[test]
    fn an_unmeasured_branch_charges_nothing_at_all() {
        let code = vec![cbz(8), alu(1), alu(2)];
        let sparse = |_: &str, b: usize| (b == 0).then(|| BlockObs::new(9, 0));
        let t = terms(&code, &BlockCounts::Measured(&sparse));
        assert_eq!(t.summary.biased, 0);
        assert_eq!(t.summary.no_data, 1);
        assert_eq!(branch_mispredict_charge(penalty(), t.bias_at(0)), 0);
    }

    #[test]
    fn an_unconditional_branch_is_perfectly_predicted_not_merely_unmeasured() {
        let code = vec![b(8), alu(1), alu(2)];
        let per_block = |_: &str, bi: usize| Some(BlockObs::new(1, bi as u64));
        let t = terms(&code, &BlockCounts::Measured(&per_block));
        assert_eq!(t.summary.branches, 1);
        assert_eq!(t.summary.unconditional, 1);
        assert_eq!(t.summary.biased, 0);
        assert_eq!(t.summary.no_data, 0, "not a want-of-data case");
        assert_eq!(branch_mispredict_charge(penalty(), t.bias_at(0)), 0);
    }

    #[test]
    fn a_dead_word_never_lowers_the_mispredict_charge() {
        let code = vec![cbz(8), alu(1), alu(2)];
        let per_block = |_: &str, bi: usize| Some(BlockObs::new(bi as u64 + 1, bi as u64));
        let before = terms(&code, &BlockCounts::Measured(&per_block));
        let mut grown = code.clone();
        grown.push(EmittedWord::gpr(
            0,
            String::new(),
            CostRule::Alu,
            Some(20),
            &[21, 21],
        ));
        let after = terms(&grown, &BlockCounts::Measured(&per_block));
        let b0 = branch_mispredict_charge(penalty(), before.bias_at(0));
        let a0 = branch_mispredict_charge(penalty(), after.bias_at(0));
        assert!(
            a0 >= b0,
            "a dead word lowered the mispredict charge {b0} -> {a0}"
        );
    }

    #[test]
    fn five_branches_in_one_32b_region_cost_more_than_four() {
        let four = vec![cbz(4), cbz(4), cbz(4), cbz(4), alu(1)];
        let five = vec![cbz(4), cbz(4), cbz(4), cbz(4), cbz(4), alu(1)];
        let t4 = terms(&four, &BlockCounts::Flat);
        let t5 = terms(&five, &BlockCounts::Flat);
        assert_eq!(t4.summary.dense_excess, 0, "four per region is the limit");
        assert_eq!(t5.summary.dense_excess, 1);
        assert_eq!(t4.total_frontend_charge(), 0);
        assert_eq!(t5.total_frontend_charge(), frontend_penalty());
        assert!(
            score_flat(&prog("F.m", five)).total_proxy_cycles
                > score_flat(&prog("F.m", four)).total_proxy_cycles
        );
    }

    #[test]
    fn branches_more_than_four_per_region_apart_are_never_dense() {
        let mut code = Vec::new();
        for k in 0..4 {
            code.push(cbz(4));
            code.push(alu(k + 1));
        }
        code.push(alu(9));
        let t = terms(&code, &BlockCounts::Flat);
        assert_eq!(t.summary.dense_excess, 0);
        assert_eq!(t.total_frontend_charge(), 0);
    }

    #[test]
    fn a_loop_crossing_the_32b_window_costs_more_than_one_inside_it() {
        let inside = vec![alu(1), b(0), alu(2)];
        let mut crossing = vec![alu(1)];
        for k in 0..7u8 {
            crossing.push(alu(k + 2));
        }
        crossing.push(cbz(-28));
        crossing.push(alu(9));
        let ti = terms(&inside, &BlockCounts::Flat);
        let tc = terms(&crossing, &BlockCounts::Flat);
        assert_eq!(ti.summary.loop_crossings, 0, "a 4 B loop always fits");
        assert_eq!(tc.summary.loop_crossings, 1);
        assert_eq!(tc.total_frontend_charge(), frontend_penalty());
        assert!(tc.total_frontend_charge() > ti.total_frontend_charge());
    }

    #[test]
    fn a_loop_bigger_than_the_region_is_outside_the_rule() {
        let mut code = vec![alu(1)];
        for k in 0..8u8 {
            code.push(alu(k + 2));
        }
        code.push(cbz(-32));
        let t = terms(&code, &BlockCounts::Flat);
        assert_eq!(t.summary.loop_crossings, 0);
        assert_eq!(t.total_frontend_charge(), 0);
    }

    #[test]
    fn the_frontend_charge_is_exactly_the_two_decided_terms() {
        let cases: Vec<Vec<EmittedWord>> = vec![
            vec![cbz(8), alu(1), alu(2)],
            vec![cbz(4), cbz(4), cbz(4), cbz(4), cbz(4), alu(1)],
            vec![alu(1), alu(2), cbz(-4), alu(3)],
            vec![b(8), alu(1), alu(2)],
        ];
        for code in cases {
            let t = terms(&code, &BlockCounts::Flat);
            assert_eq!(
                t.total_frontend_charge(),
                t.summary
                    .dense_excess
                    .saturating_add(t.summary.loop_crossings)
                    .saturating_mul(frontend_penalty()),
                "an undecidable §4.8 term charged something"
            );
        }
    }

    #[test]
    fn unaligned_target_and_entry_are_undecidable_and_charge_nothing() {
        let odd = vec![cbz(8), alu(1), alu(2), alu(3)];
        let mut aligned = vec![cbz(32)];
        for k in 0..8u8 {
            aligned.push(alu(k + 1));
        }
        let a = terms(&odd, &BlockCounts::Flat);
        let b_ = terms(&aligned, &BlockCounts::Flat);
        assert_eq!(a.total_frontend_charge(), 0);
        assert_eq!(b_.total_frontend_charge(), 0);
        assert_eq!(
            table().branch_row("region_bytes").expect("row").value,
            2 << 20
        );
    }

    #[test]
    fn a_padding_word_can_lower_the_density_charge_and_that_is_real() {
        let dense = vec![
            cbz(4),
            alu(1),
            cbz(4),
            alu(2),
            cbz(4),
            alu(3),
            cbz(4),
            cbz(4),
            alu(4),
        ];
        let before = terms(&dense, &BlockCounts::Flat);
        assert_eq!(before.summary.dense_excess, 1);
        let mut padded = dense.clone();
        padded.insert(7, alu(20));
        let after = terms(&padded, &BlockCounts::Flat);
        assert_eq!(
            after.summary.dense_excess, 0,
            "the padding word is supposed to break the dense region"
        );
        assert!(
            after.total_frontend_charge() < before.total_frontend_charge(),
            "this is the non-monotonicity item L must account for: {} -> {}",
            before.total_frontend_charge(),
            after.total_frontend_charge()
        );
    }

    #[test]
    fn call_and_abort_words_count_toward_the_density_rule() {
        let code = vec![
            cbz(4),
            bl_rule(CostRule::Abort),
            cbz(4),
            bl_rule(CostRule::AbortVal),
            cbz(4),
            bl_rule(CostRule::Call),
            alu(1),
            alu(2),
        ];
        let t = terms(&code, &BlockCounts::Flat);
        assert_eq!(
            t.summary.dense_excess, 2,
            "six branch instructions in one region is two over the limit of four"
        );
        assert_eq!(t.total_frontend_charge(), 2 * frontend_penalty());

        let control = vec![
            cbz(4),
            alu(3),
            cbz(4),
            alu(4),
            cbz(4),
            alu(5),
            alu(1),
            alu(2),
        ];
        assert_eq!(terms(&control, &BlockCounts::Flat).summary.dense_excess, 0);
    }

    #[test]
    fn the_density_set_is_exactly_the_profiles_b_pipe_rules() {
        let t = table();
        for &rule in CostRule::ALL {
            if rule.is_crosscore() {
                assert!(!is_branch_class(rule), "{rule:?} has no [latency] row");
                continue;
            }
            let row = t
                .latency_row(rule.as_str())
                .unwrap_or_else(|| panic!("[latency.{}] is required", rule.as_str()));
            let on_b = row.ports.split(',').any(|p| p.trim() == "B");
            assert_eq!(
                is_branch_class(rule),
                on_b,
                "{rule:?} is on ports {:?} but is_branch_class says {}",
                row.ports,
                is_branch_class(rule)
            );
        }
    }

    #[test]
    fn a_computed_branch_fails_closed_and_a_ret_does_not() {
        let t = table();
        let p = point();
        let br = vec![
            alu(1),
            EmittedWord::gpr(
                encode::enc_br(9),
                String::new(),
                CostRule::Branch,
                None,
                &[9],
            ),
        ];
        let err = BranchTerms::compute("F.m", &br, &t, &p, &BlockCounts::Flat)
            .expect_err("a computed branch must not be priced silently");
        assert!(
            err.contains("computed branch"),
            "the refusal must name what it refused: {err}"
        );

        let ret = vec![
            alu(1),
            EmittedWord::gpr(
                encode::enc_ret(30),
                String::new(),
                CostRule::Branch,
                None,
                &[],
            ),
        ];
        let terms = BranchTerms::compute("F.m", &ret, &t, &p, &BlockCounts::Flat).expect("ret");
        assert_eq!(terms.summary.unresolved, 1);
        assert_eq!(terms.summary.biased, 0);
        assert_eq!(branch_mispredict_charge(penalty(), terms.bias_at(1)), 0);
        assert!(
            is_indirect_branch_register(encode::enc_br(9))
                && !is_indirect_branch_register(encode::enc_ret(30)),
            "the decoder must split BR from RET"
        );
    }

    #[test]
    fn the_fetch_region_comes_from_the_profile_and_agrees_across_its_rows() {
        let t = table();
        assert_eq!(fetch_region_bytes(&t).expect("region"), 32);
        assert_eq!(t.branch_row("max_branches_per_32b").expect("row").value, 4);
        assert!(
            t.sweep(FRONTEND_SWEEP_DIM).is_some(),
            "[sweep.{FRONTEND_SWEEP_DIM}] must exist for the §4.8 terms to have a magnitude"
        );
        assert_eq!(t.sweep(FRONTEND_SWEEP_DIM).expect("row").pinned, 6);
        assert_eq!(t.sweep(FRONTEND_SWEEP_DIM).expect("row").lo, 1);
    }

    #[test]
    fn the_frontend_magnitude_moves_with_the_sweep() {
        let t = table();
        let code = vec![cbz(4), cbz(4), cbz(4), cbz(4), cbz(4), alu(1)];
        let hi = BranchTerms::compute(
            "F.m",
            &code,
            &t,
            &SweepPoint::pinned(&t).with(FRONTEND_SWEEP_DIM, 6),
            &BlockCounts::Flat,
        )
        .expect("hi");
        let lo = BranchTerms::compute(
            "F.m",
            &code,
            &t,
            &SweepPoint::pinned(&t).with(FRONTEND_SWEEP_DIM, 1),
            &BlockCounts::Flat,
        )
        .expect("lo");
        assert_eq!(hi.total_frontend_charge(), 6);
        assert_eq!(lo.total_frontend_charge(), 1);
    }

    #[test]
    fn branch_terms_census_over_the_cost_corpus() {
        use crate::cost::stage::codegen_cost_stage_with_placement;
        use crate::opts::win::discover_cost_corpus;

        let t = table();
        let p = SweepPoint::pinned(&t);
        let fe = p.get(FRONTEND_SWEEP_DIM);
        let pen = penalty();
        let corpus = discover_cost_corpus();
        assert!(!corpus.is_empty(), "cost corpus empty");

        let per_block = |_: &str, b: usize| Some(BlockObs::new(1, b as u64));

        let mut census = BranchSummary::default();
        let mut charge_flat = 0u64;
        let mut worst_case_mispredict = 0u64;
        let mut cases = 0u64;
        for path in &corpus {
            let case = path
                .parent()
                .unwrap()
                .file_name()
                .unwrap()
                .to_string_lossy()
                .to_string();
            cases += 1;
            let (program, _place) = codegen_cost_stage_with_placement(path)
                .unwrap_or_else(|e| panic!("codegen {case}: {e}"));
            let mut case_max = 0u64;
            for (key, f) in &program.fns {
                let flat = BranchTerms::compute(key, &f.code, &t, &p, &BlockCounts::Flat)
                    .unwrap_or_else(|e| panic!("{case}/{key}: {e}"));
                let s = flat.summary;
                assert_eq!(
                    s.branches,
                    s.unconditional + s.unresolved + s.biased + s.no_data,
                    "{case}/{key}: a branch fell outside every reason"
                );
                assert_eq!(
                    s.biased, 0,
                    "{case}/{key}: the flat row produced a bias — `f ≡ 1` is not bias information"
                );
                assert_eq!(
                    flat.total_frontend_charge(),
                    (s.dense_excess + s.loop_crossings) * fe,
                    "{case}/{key}: an undecidable §4.8 term charged something"
                );
                for (i, ew) in f.code.iter().enumerate() {
                    if ew.rule != CostRule::Branch || is_unconditional_b(ew.word) {
                        continue;
                    }
                    assert!(
                        !is_indirect_branch_register(ew.word),
                        "{case}/{key}: word {i} is a computed branch that reached the census"
                    );
                }
                census.branches += s.branches;
                census.unconditional += s.unconditional;
                census.unresolved += s.unresolved;
                census.no_data += s.no_data;
                census.dense_excess += s.dense_excess;
                census.loop_crossings += s.loop_crossings;
                charge_flat += flat.total_frontend_charge();

                let measured =
                    BranchTerms::compute(key, &f.code, &t, &p, &BlockCounts::Measured(&per_block))
                        .unwrap_or_else(|e| panic!("{case}/{key}: {e}"));
                census.biased += measured.summary.biased;
                for w in 0..f.code.len() {
                    let c = branch_mispredict_charge(pen, measured.bias_at(w));
                    case_max += c;
                }
            }
            worst_case_mispredict += case_max;
            println!("  {case}: worst-case mispredict cycles = {case_max}");
        }
        println!(
            "branch census over {cases} cost-* cases: branches={} unconditional={} \
             unresolved={} no_data(flat)={} biased(all-blocks-measured)={} \
             dense_excess={} loop_crossings={} frontend_cycles={} \
             worst_case_mispredict_cycles={}",
            census.branches,
            census.unconditional,
            census.unresolved,
            census.no_data,
            census.biased,
            census.dense_excess,
            census.loop_crossings,
            charge_flat,
            worst_case_mispredict,
        );
        assert!(census.branches > 0, "no branch words in the cost corpus");
    }

    #[test]
    fn the_mispredict_penalty_moves_with_the_sweep() {
        let bias = BranchBias::from_distinct_counts(1, 1).expect("bias");
        assert_eq!(branch_mispredict_charge(11, Some(bias)), 11);
        assert_eq!(branch_mispredict_charge(14, Some(bias)), 14);
        let t = table();
        for p in [
            SweepPoint::pinned(&t).with("mispredict_penalty", 11),
            SweepPoint::pinned(&t).with("mispredict_penalty", 14),
        ] {
            assert_eq!(
                branch_mispredict_charge(p.get("mispredict_penalty"), Some(bias)),
                p.get("mispredict_penalty")
            );
        }
    }
}
