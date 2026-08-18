use std::collections::BTreeMap;

use crate::codegen::{CodegenFn, CodegenProgram};
use crate::placement::PlacementTable;

use super::branch::{BlockCounts, BranchTerms};
use super::footprint::{self, CoreBudget, HotBlocks};
use super::mem::{MemLevel, MemState};
use super::owner::classify_owner;
use super::rule::{CostRule, EmittedWord, MemClass, REG_BANK_COUNT, Reg};
use super::sweep::SweepPoint;
use super::table::{CostTable, LatRow, pipe_range};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FnCost {
    pub key: String,
    pub owner: String,
    pub frame_bytes: u64,
    pub proxy_cycles: u64,
    pub words: u64,
    pub terms: BTreeMap<String, u64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CostReport {
    pub version: u64,
    pub digest: String,
    pub provenance: String,
    pub provenance_summary: String,
    pub profile: String,
    pub pipelines: u64,
    pub dispatch_mops: u64,
    pub dispatch_uops: u64,
    pub reorder_window: u64,
    pub total_proxy_cycles: u64,
    pub schedule_cycles: u64,
    pub footprint_cycles: u64,
    pub rank_cycles: u64,
    pub total_words: u64,
    pub sync_frame_max_bytes: u64,
    pub async_frame_total_bytes: u64,
    pub owner_totals: BTreeMap<String, u64>,
    pub fns: Vec<FnCost>,
    pub workloads_digest: Option<String>,
    pub workload_totals: BTreeMap<String, u64>,
    pub workload_coverage: BTreeMap<String, (u64, u64)>,
    pub workload_validation_bounds: BTreeMap<String, u64>,
    pub footprint: Vec<CoreBudget>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PortClass {
    B,
    I,
    M,
    L,
    V,
}

const PORT_CLASS_COUNT: usize = 5;

impl PortClass {
    fn index(self) -> usize {
        match self {
            PortClass::B => 0,
            PortClass::I => 1,
            PortClass::M => 2,
            PortClass::L => 3,
            PortClass::V => 4,
        }
    }
}

const PORT_LETTERS: &[(&str, &str, PortClass)] = &[
    ("B", "port_b", PortClass::B),
    ("I", "port_i", PortClass::I),
    ("M", "port_m", PortClass::M),
    ("D", "port_d", PortClass::V),
    ("V0", "port_v0", PortClass::V),
    ("V1", "port_v1", PortClass::V),
    ("L", "port_l", PortClass::L),
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Uop {
    pipes: u32,
    class: PortClass,
}

#[derive(Debug, Clone)]
struct Machine {
    pipes: usize,
    dispatch_mops: u64,
    dispatch_uops: u64,
    caps: [u64; PORT_CLASS_COUNT],
    window: usize,
    letters: Vec<(&'static str, u32, PortClass)>,
}

impl Machine {
    fn from_table(table: &CostTable) -> Result<Machine, String> {
        let pipes = table.pipelines() as usize;
        let mut letters = Vec::with_capacity(PORT_LETTERS.len());
        for (letter, row_key, class) in PORT_LETTERS {
            let row = table.pipeline_row(row_key).ok_or_else(|| {
                format!("cost table: [pipelines.{row_key}] is required by the port map")
            })?;
            let spec = row.text.as_deref().ok_or_else(|| {
                format!("cost table: [pipelines.{row_key}] must be a pipe-range string")
            })?;
            let (lo, hi) = pipe_range(spec).ok_or_else(|| {
                format!("cost table: [pipelines.{row_key}] `{spec}` is not a pipe range")
            })?;
            let mut mask = 0u32;
            for p in lo..=hi {
                if p as usize > pipes || p >= 32 {
                    return Err(format!(
                        "cost table: [pipelines.{row_key}] names pipeline {p} outside 1..={pipes}"
                    ));
                }
                mask |= 1u32 << p;
            }
            letters.push((*letter, mask, *class));
        }
        let cap = |key: &str| -> Result<u64, String> {
            table
                .pipeline_row(key)
                .map(|r| r.value)
                .ok_or_else(|| format!("cost table: [pipelines.{key}] is required"))
        };
        let mask_of = |letter: &str| -> u32 {
            letters
                .iter()
                .find(|(l, _, _)| *l == letter)
                .map(|(_, m, _)| *m)
                .unwrap_or(0)
        };
        let l_pipes = u64::from(mask_of("L").count_ones()).max(1);
        let v_pipes = u64::from(mask_of("D").count_ones()).max(1);
        let mut caps = [0u64; PORT_CLASS_COUNT];
        caps[PortClass::B.index()] = cap("cap_b")?;
        caps[PortClass::I.index()] = cap("cap_s")?;
        caps[PortClass::M.index()] = cap("cap_m")?;
        caps[PortClass::L.index()] = cap("cap_l_each")?.saturating_mul(l_pipes);
        caps[PortClass::V.index()] = cap("cap_v_each")?.saturating_mul(v_pipes);
        Ok(Machine {
            pipes,
            dispatch_mops: table.dispatch_mops(),
            dispatch_uops: table.dispatch_uops(),
            caps,
            window: table.reorder_window() as usize,
            letters,
        })
    }

    fn uops_for(&self, ports: &str) -> Result<Vec<Uop>, String> {
        let mut names: Vec<&str> = Vec::new();
        for part in ports.split(',') {
            let t = part.trim();
            if t.is_empty() {
                return Err(format!("cost table: empty port letter in `{ports}`"));
            }
            names.push(t);
        }
        let alt_v = names.contains(&"V0") && names.contains(&"V1");
        let mut out = Vec::with_capacity(names.len());
        let mut v_done = false;
        for name in &names {
            if alt_v && (*name == "V0" || *name == "V1") {
                if v_done {
                    continue;
                }
                v_done = true;
                let pipes = self.mask("V0")? | self.mask("V1")?;
                out.push(Uop {
                    pipes,
                    class: PortClass::V,
                });
                continue;
            }
            let (_, pipes, class) = *self
                .letters
                .iter()
                .find(|(l, _, _)| l == name)
                .ok_or_else(|| format!("cost table: unknown port letter `{name}` in `{ports}`"))?;
            out.push(Uop { pipes, class });
        }
        if out.is_empty() {
            return Err(format!("cost table: `{ports}` names no pipeline"));
        }
        Ok(out)
    }

    fn mask(&self, letter: &str) -> Result<u32, String> {
        self.letters
            .iter()
            .find(|(l, _, _)| *l == letter)
            .map(|(_, m, _)| *m)
            .ok_or_else(|| format!("cost table: unknown port letter `{letter}`"))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct CrossExtra {
    pub extra_cycles: u64,
    pub serializes_window: bool,
}

pub use super::branch::BranchBias;

fn mem_access(ew: &EmittedWord, state: &mut MemState) -> super::mem::MemVerdict {
    state.access(ew)
}

fn crosscore_extra(
    fn_key: &str,
    ew: &EmittedWord,
    table: &CostTable,
    point: &SweepPoint,
    placement: &PlacementTable,
) -> CrossExtra {
    super::crosscore::charge(fn_key, ew, table, point, placement)
}

fn branch_mispredict_charge(
    table: &CostTable,
    point: &SweepPoint,
    bias: Option<BranchBias>,
) -> u64 {
    let _ = table;
    super::branch::branch_mispredict_charge(point.get("mispredict_penalty"), bias)
}

fn alignment_penalty(
    ew: &EmittedWord,
    table: &CostTable,
    point: &SweepPoint,
) -> Result<u64, String> {
    let is_load = ew.rule.is_load();
    if !is_load && !ew.rule.is_store() {
        return Ok(0);
    }
    let width = u64::from(ew.access_bytes);
    let Some(m) = ew.mem else {
        return Ok(0);
    };
    if width == 0 || m.class != MemClass::Stack {
        return Ok(0);
    }
    let (row_key, dim) = if is_load {
        ("load_line_bytes", "load_line_cross_penalty")
    } else {
        ("store_boundary_bytes", "store_boundary_cross_penalty")
    };
    let boundary = table
        .align(row_key)
        .ok_or_else(|| format!("cost table: [align.{row_key}] is required by SOG §4.5's terms"))?
        .value;
    if crosses_boundary(m.key, width, boundary, crate::codegen::FRAME_SP_ALIGN_BYTES) {
        return Ok(point.get(dim));
    }
    Ok(0)
}

fn crosses_boundary(off: u64, width: u64, boundary: u64, sp_align: u64) -> bool {
    if boundary == 0 || width == 0 {
        return false;
    }
    let step = sp_align.max(1);
    let mut k = 0u64;
    loop {
        if (k.wrapping_add(off) % boundary) + width > boundary {
            return true;
        }
        k += step;
        if k >= boundary {
            return false;
        }
    }
}

pub fn score_program(
    program: &CodegenProgram,
    table: &CostTable,
    placement: &PlacementTable,
) -> Result<CostReport, String> {
    score_program_at(program, table, placement, &SweepPoint::pinned(table))
}

pub fn score_linked_program(
    linked: &crate::linked::LinkedProgram,
    table: &CostTable,
    placement: &PlacementTable,
) -> Result<CostReport, String> {
    crate::cost::audit::audit_linked(linked)?;
    let fn_words: u64 = linked.fns.values().map(|f| f.code.len() as u64).sum();
    if fn_words != linked.executable_words() {
        return Err(format!(
            "linked cost word reconciliation failed: functions={fn_words} sections={}",
            linked.executable_words()
        ));
    }
    let fns: BTreeMap<String, CodegenFn> = linked
        .fns
        .iter()
        .map(|(key, f)| {
            (
                key.clone(),
                CodegenFn {
                    frame_size: f.frame_size as usize,
                    code: f.code.clone(),
                    relocs: Vec::new(),
                    regions: Vec::new(),
                },
            )
        })
        .collect();
    let program = CodegenProgram {
        fns,
        rodata: Vec::new(),
        conventions: BTreeMap::new(),
        origin_spans: Vec::new(),
    };
    let point = SweepPoint::pinned(table);
    let ctx = ScoreCtx::new(table)?;
    let addresses: BTreeMap<String, u64> = linked
        .fns
        .iter()
        .map(|(key, f)| (key.clone(), f.byte_address))
        .collect();
    let totals = score_program_core_with_addresses(
        &program,
        table,
        placement,
        &point,
        &ctx,
        HotBlocks::All,
        BlockCounts::Flat,
        true,
        Some(&addresses),
    )?;
    for function in &totals.fns {
        let counted_words: u64 = function
            .terms
            .iter()
            .filter(|(rule, _)| CostRule::from_str(rule).is_some())
            .map(|(_, count)| *count)
            .sum();
        if counted_words != function.words {
            return Err(format!(
                "linked cost terms for `{}` count {counted_words} instructions, expected {}",
                function.key, function.words
            ));
        }
    }
    Ok(CostReport {
        version: table.version,
        digest: table.table_digest(),
        provenance: table.provenance_digest(),
        provenance_summary: table.provenance_summary(),
        profile: table.profile_name().to_string(),
        pipelines: table.pipelines(),
        dispatch_mops: table.dispatch_mops(),
        dispatch_uops: table.dispatch_uops(),
        reorder_window: table.reorder_window(),
        total_proxy_cycles: totals.total_proxy_cycles,
        schedule_cycles: totals.schedule_cycles,
        footprint_cycles: totals.footprint_cycles,
        rank_cycles: totals.rank_cycles,
        total_words: totals.total_words,
        sync_frame_max_bytes: linked.sync_frame_max_bytes,
        async_frame_total_bytes: linked.async_frame_total_bytes,
        owner_totals: totals.owner_totals,
        fns: totals.fns,
        workloads_digest: None,
        workload_totals: BTreeMap::new(),
        workload_coverage: BTreeMap::new(),
        workload_validation_bounds: BTreeMap::new(),
        footprint: totals.footprint,
    })
}

pub fn score_program_at(
    program: &CodegenProgram,
    table: &CostTable,
    placement: &PlacementTable,
    point: &SweepPoint,
) -> Result<CostReport, String> {
    score_program_at_with_hot(
        program,
        table,
        placement,
        point,
        HotBlocks::All,
        BlockCounts::Flat,
    )
}

pub fn score_program_at_with_hot(
    program: &CodegenProgram,
    table: &CostTable,
    placement: &PlacementTable,
    point: &SweepPoint,
    hot: HotBlocks<'_>,
    counts: BlockCounts<'_>,
) -> Result<CostReport, String> {
    let ctx = ScoreCtx::new(table)?;
    let totals = score_program_core(program, table, placement, point, &ctx, hot, counts, true)?;

    Ok(CostReport {
        version: table.version,
        digest: table.table_digest(),
        provenance: table.provenance_digest(),
        provenance_summary: table.provenance_summary(),
        profile: table.profile_name().to_string(),
        pipelines: table.pipelines(),
        dispatch_mops: table.dispatch_mops(),
        dispatch_uops: table.dispatch_uops(),
        reorder_window: table.reorder_window(),
        total_proxy_cycles: totals.total_proxy_cycles,
        schedule_cycles: totals.schedule_cycles,
        footprint_cycles: totals.footprint_cycles,
        rank_cycles: totals.rank_cycles,
        total_words: totals.total_words,
        sync_frame_max_bytes: program
            .fns
            .values()
            .map(|f| f.frame_size as u64)
            .max()
            .unwrap_or(0),
        async_frame_total_bytes: 0,
        owner_totals: totals.owner_totals,
        fns: totals.fns,
        workloads_digest: None,
        workload_totals: BTreeMap::new(),
        workload_coverage: BTreeMap::new(),
        workload_validation_bounds: BTreeMap::new(),
        footprint: totals.footprint,
    })
}

pub struct ScoreCtx {
    machine: Machine,
    uops: Vec<Result<Vec<Uop>, String>>,
    timing: Vec<(u64, u64, bool, bool)>,
    lat: Vec<LatSpec>,
}

struct LatSpec {
    row: Option<(u64, Option<String>, Option<String>)>,
    folded: u64,
}

impl ScoreCtx {
    pub fn new(table: &CostTable) -> Result<ScoreCtx, String> {
        let machine = Machine::from_table(table)?;
        let n = CostRule::ALL.len();
        let mut uops: Vec<Result<Vec<Uop>, String>> = (0..n)
            .map(|_| {
                Err(
                    "cost model: a CostRule missing from `CostRule::ALL` reached the \
                     scoreboard; the rule census and the port map disagree"
                        .to_string(),
                )
            })
            .collect();
        let mut timing = vec![(1u64, 0u64, false, false); n];
        let mut lat = Vec::with_capacity(n);
        lat.resize_with(n, || LatSpec {
            row: None,
            folded: 0,
        });
        for rule in CostRule::ALL {
            let i = *rule as usize;
            uops[i] = ports_for(*rule, table).and_then(|p| machine.uops_for(p));
            let row = timing_row(*rule, table);
            timing[i] = (
                row.map(|r| r.thru_den.div_ceil(r.thru_num).max(1))
                    .unwrap_or(1),
                row.map(|r| r.m_pipe_stall).unwrap_or(0),
                row.map(|r| r.m_pipe_block).unwrap_or(false),
                row.is_some_and(|r| r.thru_num == 1),
            );
            lat[i] = LatSpec {
                row: table
                    .latency_row(rule.as_str())
                    .map(|r| (r.lat, r.sweep.clone(), r.sweep_add.clone())),
                folded: table.latency(*rule),
            };
        }
        Ok(ScoreCtx {
            machine,
            uops,
            timing,
            lat,
        })
    }

    fn uops(&self, rule: CostRule) -> Result<&[Uop], String> {
        match &self.uops[rule as usize] {
            Ok(u) => Ok(u),
            Err(e) => Err(e.clone()),
        }
    }

    fn timing(&self, rule: CostRule) -> (u64, u64, bool, bool) {
        self.timing[rule as usize]
    }

    fn rule_latency(&self, rule: CostRule, point: &SweepPoint) -> u64 {
        let spec = &self.lat[rule as usize];
        match &spec.row {
            Some((lat, sweep, sweep_add)) => {
                let mut l = match sweep {
                    Some(dim) => point.get(dim),
                    None => *lat,
                };
                if let Some(dim) = sweep_add {
                    l = l.saturating_add(point.get(dim));
                }
                l
            }
            None => spec.folded,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScoreTotals {
    pub total_proxy_cycles: u64,
    pub schedule_cycles: u64,
    pub footprint_cycles: u64,
    pub rank_cycles: u64,
    pub total_words: u64,
    pub footprint: Vec<CoreBudget>,
    pub ordering: Option<super::crosscore::OrderingCounts>,
}

pub fn score_totals_at(
    program: &CodegenProgram,
    table: &CostTable,
    placement: &PlacementTable,
    point: &SweepPoint,
    ctx: &ScoreCtx,
    want_ordering: bool,
) -> Result<ScoreTotals, String> {
    let core = score_program_core(
        program,
        table,
        placement,
        point,
        ctx,
        HotBlocks::All,
        BlockCounts::Flat,
        want_ordering,
    )?;
    Ok(ScoreTotals {
        total_proxy_cycles: core.total_proxy_cycles,
        schedule_cycles: core.schedule_cycles,
        footprint_cycles: core.footprint_cycles,
        rank_cycles: core.rank_cycles,
        total_words: core.total_words,
        footprint: core.footprint,
        ordering: want_ordering.then(|| super::crosscore::ordering_word_counts_of(&core.fns)),
    })
}

struct ProgramCore {
    total_proxy_cycles: u64,
    schedule_cycles: u64,
    footprint_cycles: u64,
    rank_cycles: u64,
    total_words: u64,
    owner_totals: BTreeMap<String, u64>,
    fns: Vec<FnCost>,
    footprint: Vec<CoreBudget>,
}

#[allow(clippy::too_many_arguments)]
fn score_program_core(
    program: &CodegenProgram,
    table: &CostTable,
    placement: &PlacementTable,
    point: &SweepPoint,
    ctx: &ScoreCtx,
    hot: HotBlocks<'_>,
    counts: BlockCounts<'_>,
    want_fns: bool,
) -> Result<ProgramCore, String> {
    score_program_core_with_addresses(
        program, table, placement, point, ctx, hot, counts, want_fns, None,
    )
}

#[allow(clippy::too_many_arguments)]
fn score_program_core_with_addresses(
    program: &CodegenProgram,
    table: &CostTable,
    placement: &PlacementTable,
    point: &SweepPoint,
    ctx: &ScoreCtx,
    hot: HotBlocks<'_>,
    counts: BlockCounts<'_>,
    want_fns: bool,
    addresses: Option<&BTreeMap<String, u64>>,
) -> Result<ProgramCore, String> {
    let mut fns = Vec::with_capacity(if want_fns { program.fns.len() } else { 0 });
    let mut total_proxy_cycles = 0u64;
    let mut owner_totals = BTreeMap::from([
        ("app".to_string(), 0u64),
        ("runtime".to_string(), 0u64),
        ("driver".to_string(), 0u64),
    ]);

    let mut total_words = 0u64;
    for (key, f) in &program.fns {
        let fn_address = addresses.and_then(|map| map.get(key).copied());
        let (proxy_cycles, terms) = score_fn(
            key, f, table, placement, point, ctx, &counts, want_fns, fn_address,
        )?;
        let words = f.code.len() as u64;
        total_proxy_cycles = total_proxy_cycles.saturating_add(proxy_cycles);
        total_words = total_words.saturating_add(words);
        if want_fns {
            let owner = classify_owner(key).to_string();
            *owner_totals.entry(owner.clone()).or_insert(0) += proxy_cycles;
            fns.push(FnCost {
                key: key.clone(),
                owner,
                frame_bytes: f.frame_size as u64,
                proxy_cycles,
                words,
                terms,
            });
        }
    }

    let footprint = match addresses {
        Some(addresses) => {
            footprint::compute_at_addresses(program, table, point, placement, hot, addresses)?
        }
        None => footprint::compute(program, table, point, placement, hot)?,
    };
    let schedule_cycles = total_proxy_cycles;
    let footprint_cycles: u64 = footprint.iter().map(|b| b.charge).sum();
    let rank_cycles = schedule_cycles.saturating_add(footprint_cycles);
    Ok(ProgramCore {
        total_proxy_cycles: rank_cycles,
        schedule_cycles,
        footprint_cycles,
        rank_cycles,
        total_words,
        owner_totals,
        fns,
        footprint,
    })
}

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

pub fn block_schedule_lengths(
    fn_key: &str,
    code: &[EmittedWord],
    table: &CostTable,
    placement: &PlacementTable,
) -> Result<Vec<u64>, String> {
    block_schedule_lengths_with_counts(fn_key, code, table, placement, &BlockCounts::Flat)
}

pub fn block_schedule_lengths_with_counts(
    fn_key: &str,
    code: &[EmittedWord],
    table: &CostTable,
    placement: &PlacementTable,
    counts: &BlockCounts<'_>,
) -> Result<Vec<u64>, String> {
    let ctx = ScoreCtx::new(table)?;
    let point = SweepPoint::pinned(table);
    let branch_terms = BranchTerms::compute(fn_key, code, table, &point, counts)?;
    let mut out = Vec::new();
    for (start, end) in basic_block_ranges(code) {
        let (s, _) = score_words(
            fn_key,
            &code[start..end],
            start,
            table,
            placement,
            &point,
            &ctx,
            &branch_terms,
            true,
        )?;
        out.push(s);
    }
    Ok(out)
}

/// Returns the modeled rule and memory-class census for each emitted-word
/// basic block under the same pinned point used by the proxy schedule.
/// Physical validation consumes this to compare PMU behavior with the model;
/// it is evidence only and never feeds a cost constant.
pub fn block_term_counts_with_counts(
    fn_key: &str,
    code: &[EmittedWord],
    table: &CostTable,
    placement: &PlacementTable,
    counts: &BlockCounts<'_>,
) -> Result<Vec<BTreeMap<String, u64>>, String> {
    let ctx = ScoreCtx::new(table)?;
    let point = SweepPoint::pinned(table);
    let branch_terms = BranchTerms::compute(fn_key, code, table, &point, counts)?;
    basic_block_ranges(code)
        .into_iter()
        .map(|(start, end)| {
            score_words(
                fn_key,
                &code[start..end],
                start,
                table,
                placement,
                &point,
                &ctx,
                &branch_terms,
                true,
            )
            .map(|(_, terms)| terms)
        })
        .collect()
}

/// Returns a deliberately serialized, all-upper-endpoint cycle bound for
/// each emitted-word basic block. This is physical-validation evidence, not
/// an optimizer input: it makes no overlap assumption and changes no pinned
/// proxy ranking.
pub fn block_serial_lengths_with_counts(
    fn_key: &str,
    code: &[EmittedWord],
    table: &CostTable,
    placement: &PlacementTable,
    counts: &BlockCounts<'_>,
) -> Result<Vec<u64>, String> {
    let ctx = ScoreCtx::new(table)?;
    let point = SweepPoint::upper(table);
    let branch_terms = BranchTerms::compute(fn_key, code, table, &point, counts)?;
    basic_block_ranges(code)
        .into_iter()
        .map(|(start, end)| {
            serial_words(
                fn_key,
                &code[start..end],
                start,
                table,
                placement,
                &point,
                &ctx,
                &branch_terms,
            )
        })
        .collect()
}

#[allow(clippy::too_many_arguments)]
fn serial_words(
    fn_key: &str,
    code: &[EmittedWord],
    word_base: usize,
    table: &CostTable,
    placement: &PlacementTable,
    point: &SweepPoint,
    ctx: &ScoreCtx,
    branch_terms: &BranchTerms,
) -> Result<u64, String> {
    let mut mem = MemState::new(table, point);
    let mut cycles = 0u64;
    for (i, ew) in code.iter().enumerate() {
        check_mem_base_in_srcs(ew)?;
        let exec_lat = ctx.rule_latency(ew.rule, point);
        let mut lat = match ew.rule {
            CostRule::Branch => exec_lat
                .saturating_add(branch_mispredict_charge(
                    table,
                    point,
                    branch_terms.bias_at(word_base + i),
                ))
                .saturating_add(branch_terms.frontend_at(word_base + i)),
            r if r.is_load() || r.is_store() => {
                let verdict = mem_access(ew, &mut mem);
                match verdict.level {
                    MemLevel::L1dHit | MemLevel::Unresolved => verdict.latency.max(exec_lat),
                    _ => verdict.latency,
                }
            }
            _ => exec_lat,
        };
        let cross = crosscore_extra(fn_key, ew, table, point, placement);
        lat = lat.saturating_add(cross.extra_cycles);
        lat = lat.saturating_add(alignment_penalty(ew, table, point)?);
        cycles = cycles.saturating_add(lat.max(1));
        if ew.rule == CostRule::Call {
            mem.call_boundary();
        } else if ew.rule == CostRule::Barrier {
            mem.barrier();
        }
    }
    Ok(cycles)
}

fn score_fn(
    key: &str,
    f: &CodegenFn,
    table: &CostTable,
    placement: &PlacementTable,
    point: &SweepPoint,
    ctx: &ScoreCtx,
    counts: &BlockCounts<'_>,
    want_terms: bool,
    fn_address: Option<u64>,
) -> Result<(u64, BTreeMap<String, u64>), String> {
    let mut terms: BTreeMap<String, u64> = BTreeMap::new();
    if f.code.is_empty() {
        return Ok((0, terms));
    }
    let branch_terms = match fn_address {
        Some(address) => {
            BranchTerms::compute_at_address(key, &f.code, table, point, counts, address)?
        }
        None => BranchTerms::compute(key, &f.code, table, point, counts)?,
    };
    let mut proxy_cycles = 0u64;
    for (start, end) in basic_block_ranges(&f.code) {
        let (s, block_terms) = score_words(
            key,
            &f.code[start..end],
            start,
            table,
            placement,
            point,
            ctx,
            &branch_terms,
            want_terms,
        )?;
        proxy_cycles = proxy_cycles.saturating_add(s);
        for (k, v) in block_terms {
            *terms.entry(k).or_insert(0) += v;
        }
    }
    Ok((proxy_cycles, terms))
}

#[allow(clippy::too_many_arguments)]
fn score_words(
    fn_key: &str,
    code: &[EmittedWord],
    word_base: usize,
    table: &CostTable,
    placement: &PlacementTable,
    point: &SweepPoint,
    ctx: &ScoreCtx,
    branch_terms: &BranchTerms,
    want_terms: bool,
) -> Result<(u64, BTreeMap<String, u64>), String> {
    let mut terms: BTreeMap<String, u64> = BTreeMap::new();
    if code.is_empty() {
        return Ok((0, terms));
    }

    let mut mem = MemState::new(table, point);
    let mut pipe_free = vec![0u64; ctx.machine.pipes + 1];
    let mut unit_free = [0u64; PORT_CLASS_COUNT];
    // Some table rows deliberately allow fewer operations than their port
    // group has physical pipes. In particular, scalar FP loads may use both
    // L pipes while a Q load/store is sealed at one operation per cycle.
    // Per-rule readiness represents that row throughput instead of silently
    // widening Q accesses to the generic two-AGU rate.
    let mut rule_free = vec![0u64; CostRule::ALL.len()];
    let mut block_free = 0u64;
    // Readiness is per bank: `x0` and `v0` are different registers, so one
    // 32-entry table made every FP definition look like it clobbered the
    // general register of the same number (P8R.1).
    let mut ready = [[0u64; 32]; REG_BANK_COUNT];
    let mut flags_ready = 0u64;
    let mut sp_ready = 0u64;
    let mut control_ready = 0u64;
    let mut serial_until = 0u64;

    let mut disp_cycle = 0u64;
    let mut disp_mops = 0u64;
    let mut disp_uops = 0u64;
    let mut disp_class = [0u64; PORT_CLASS_COUNT];

    let mut retire = vec![0u64; code.len()];
    let mut max_retire = 0u64;

    for (i, ew) in code.iter().enumerate() {
        if want_terms {
            *terms.entry(ew.rule.as_str().to_string()).or_insert(0) += 1;
        }
        check_mem_base_in_srcs(ew)?;

        let uops = ctx.uops(ew.rule)?;
        let (hold, m_stall, blocks, one_per_interval) = ctx.timing(ew.rule);
        let occupancy = hold.saturating_add(m_stall);
        let exec_lat = ctx.rule_latency(ew.rule, point);
        let cross = crosscore_extra(fn_key, ew, table, point, placement);

        let mut min_disp = disp_cycle;
        if i >= ctx.machine.window {
            min_disp = min_disp.max(retire[i - ctx.machine.window]);
        }
        if min_disp > disp_cycle {
            disp_cycle = min_disp;
            disp_mops = 0;
            disp_uops = 0;
            disp_class = [0u64; PORT_CLASS_COUNT];
        }
        let mut want_class = [0u64; PORT_CLASS_COUNT];
        for u in uops {
            want_class[u.class.index()] += 1;
        }
        if uops.len() as u64 > ctx.machine.dispatch_uops {
            return Err(format!(
                "cost model: `{}` expands to {} uops, above [pipelines] dispatch_uops {}",
                ew.rule.as_str(),
                uops.len(),
                ctx.machine.dispatch_uops
            ));
        }
        for (c, want) in want_class.iter().enumerate() {
            if *want > ctx.machine.caps[c] {
                return Err(format!(
                    "cost model: `{}` wants {want} uops of one dispatch class, above its \
                     [pipelines] cap {}",
                    ew.rule.as_str(),
                    ctx.machine.caps[c]
                ));
            }
        }
        loop {
            let fits = disp_mops < ctx.machine.dispatch_mops
                && disp_uops + uops.len() as u64 <= ctx.machine.dispatch_uops
                && want_class
                    .iter()
                    .enumerate()
                    .all(|(c, want)| disp_class[c] + want <= ctx.machine.caps[c]);
            if fits {
                break;
            }
            disp_cycle = disp_cycle.saturating_add(1);
            disp_mops = 0;
            disp_uops = 0;
            disp_class = [0u64; PORT_CLASS_COUNT];
        }
        let dispatch = disp_cycle;
        disp_mops += 1;
        disp_uops += uops.len() as u64;
        for (c, want) in want_class.iter().enumerate() {
            disp_class[c] += want;
        }

        let mut base_ready = dispatch
            .max(src_ready(ew, &ready))
            .max(control_ready)
            .max(serial_until);
        if ew.flags.reads() {
            base_ready = base_ready.max(flags_ready);
        }
        let reads_sp = matches!(ew.mem.map(|m| m.class), Some(MemClass::Stack))
            || crate::encode::reads_sp(ew.word);
        if reads_sp {
            base_ready = base_ready.max(sp_ready);
        }
        if blocks {
            base_ready = base_ready.max(block_free);
        }
        if cross.serializes_window {
            base_ready = base_ready.max(max_retire);
        }
        if one_per_interval {
            base_ready = base_ready.max(rule_free[ew.rule as usize]);
        }
        let mut issue = base_ready;
        for u in uops {
            let earliest = base_ready.max(unit_free[u.class.index()]);
            let (pipe, at) = earliest_pipe(u.pipes, earliest, &pipe_free)?;
            pipe_free[pipe] = at.saturating_add(1);
            if occupancy > 1 {
                let until = at.saturating_add(occupancy);
                let slot = &mut unit_free[u.class.index()];
                *slot = (*slot).max(until);
            }
            issue = issue.max(at);
        }
        if blocks {
            block_free = block_free.max(issue.saturating_add(exec_lat));
        }
        if one_per_interval {
            rule_free[ew.rule as usize] = issue.saturating_add(occupancy.max(1));
        }

        let mut lat = match ew.rule {
            CostRule::Branch => exec_lat
                .saturating_add(branch_mispredict_charge(
                    table,
                    point,
                    branch_terms.bias_at(word_base + i),
                ))
                .saturating_add(branch_terms.frontend_at(word_base + i)),
            r if r.is_load() || r.is_store() => {
                let verdict = mem_access(ew, &mut mem);
                if want_terms {
                    let name = match verdict.level {
                        MemLevel::L1dHit => "l1_hit",
                        MemLevel::Forwarded => "forwarded",
                        MemLevel::Unresolved => "unresolved",
                        other => other.as_str(),
                    };
                    *terms.entry(format!("Mem level={name}")).or_insert(0) += 1;
                }
                match verdict.level {
                    // The hierarchy supplies the L1 floor, while the
                    // operation-specific row supplies width/bank completion
                    // latency (notably fp_load_q's extra cycle).
                    MemLevel::L1dHit | MemLevel::Unresolved => verdict.latency.max(exec_lat),
                    _ => verdict.latency,
                }
            }
            _ => exec_lat,
        };
        lat = lat.saturating_add(cross.extra_cycles);
        lat = lat.saturating_add(alignment_penalty(ew, table, point)?);

        let finish = issue.saturating_add(lat);
        retire[i] = finish;
        if let Some(d) = ew.dst {
            if !d.is_sp() {
                if let Some(slot) = ready[d.bank.index()].get_mut(usize::from(d.num)) {
                    *slot = finish;
                }
            }
        }
        if ew.dst.is_some_and(Reg::is_sp) {
            sp_ready = finish;
        }
        if ew.flags.writes() {
            flags_ready = finish;
        }
        if ew.rule == CostRule::Branch || ew.rule == CostRule::Call {
            // A returning call is a control boundary for the caller.  Its
            // dynamic callee schedule is composed separately, but following
            // caller instructions cannot issue before the call completes.
            control_ready = finish;
        }
        if cross.serializes_window {
            serial_until = serial_until.max(finish);
        }
        max_retire = max_retire.max(finish);

        if ew.rule == CostRule::Call {
            mem.call_boundary();
        } else if ew.rule == CostRule::Barrier {
            mem.barrier();
        }
    }

    Ok((max_retire, terms))
}

fn earliest_pipe(pipes: u32, from: u64, pipe_free: &[u64]) -> Result<(usize, u64), String> {
    let mut best: Option<(usize, u64)> = None;
    for p in 1..pipe_free.len() {
        if pipes & (1u32 << p) == 0 {
            continue;
        }
        let at = from.max(pipe_free[p]);
        match best {
            Some((_, b)) if b <= at => {}
            _ => best = Some((p, at)),
        }
    }
    best.ok_or_else(|| "cost model: uop names no eligible pipeline".to_string())
}

fn timing_row(rule: CostRule, table: &CostTable) -> Option<&LatRow> {
    let key = match rule {
        CostRule::Load | CostRule::LoadAcquire => "load",
        CostRule::Store | CostRule::StoreRelease => "store",
        _ => rule.as_str(),
    };
    table.latency_row(key)
}

fn ports_for(rule: CostRule, table: &CostTable) -> Result<&str, String> {
    match rule {
        CostRule::Barrier => return Ok("L"),
        CostRule::System => return Ok("I"),
        _ => {}
    }
    timing_row(rule, table)
        .map(|row| row.ports.as_str())
        .ok_or_else(|| {
            format!(
                "cost table: no [latency] row governs `{}` (ports unknown)",
                rule.as_str()
            )
        })
}

pub(super) fn branch_target_index(word: u32, from: usize) -> Option<usize> {
    let word_delta = if word & 0xFC00_0000 == 0x1400_0000 {
        sign_extend(word & 0x03FF_FFFF, 26)
    } else if word & 0xFF00_0000 == 0x5400_0000 {
        sign_extend((word >> 5) & 0x7FFFF, 19)
    } else if word & 0x7E00_0000 == 0x3400_0000 {
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

fn check_mem_base_in_srcs(ew: &EmittedWord) -> Result<(), String> {
    let Some(m) = ew.mem else {
        return Ok(());
    };
    m.require_base_in_srcs(ew.src_slice())
}

fn src_ready(ew: &EmittedWord, ready: &[[u64; 32]; REG_BANK_COUNT]) -> u64 {
    let mut t = 0u64;
    for &s in ew.src_slice() {
        // Synthetic model fixtures may name a number outside the architected
        // 0..=31 range; such an operand carries no dependency, exactly as it
        // did before operands were banked.
        if let Some(slot) = ready[s.bank.index()].get(usize::from(s.num)) {
            t = t.max(*slot);
        }
    }
    t
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cost::rule::{CostRule, FlagEffect, MEM_SP_REG, MemRef};

    fn table() -> CostTable {
        crate::cost::table::load_default().expect("bench/a76-pi5.toml")
    }

    fn placement() -> PlacementTable {
        PlacementTable {
            entries: Vec::new(),
            cores: 0,
        }
    }

    fn score(p: &CodegenProgram) -> CostReport {
        let table = table();
        score_program(p, &table, &placement()).expect("score")
    }

    fn total(p: &CodegenProgram) -> u64 {
        score(p).total_proxy_cycles
    }

    fn word(rule: CostRule, dst: Option<u8>, srcs: &[u8]) -> EmittedWord {
        EmittedWord::gpr(0, String::new(), rule, dst, srcs)
    }

    fn banked_word(rule: CostRule, dst: Option<Reg>, srcs: &[Reg]) -> EmittedWord {
        EmittedWord::banked(0, String::new(), rule, dst, srcs)
    }

    fn word_flags(rule: CostRule, dst: Option<u8>, srcs: &[u8], flags: FlagEffect) -> EmittedWord {
        word(rule, dst, srcs).with_flags(flags)
    }

    fn load_stack(dst: u8, offset: u64) -> EmittedWord {
        word(CostRule::Load, Some(dst), &[31]).with_mem(MemRef::stack(offset))
    }

    fn load_stack_after(dst: u8, offset: u64, dep: u8) -> EmittedWord {
        word(CostRule::Load, Some(dst), &[31, dep]).with_mem(MemRef::stack(offset))
    }

    fn store_stack(offset: u64) -> EmittedWord {
        word(CostRule::Store, None, &[31, 0]).with_mem(MemRef::stack(offset))
    }

    fn load_cold_unique(dst: u8, seq: u64) -> EmittedWord {
        word(CostRule::Load, Some(dst), &[0]).with_mem(MemRef::cold_unique(seq))
    }

    fn load_cold_unique_after(dst: u8, seq: u64, dep: u8) -> EmittedWord {
        word(CostRule::Load, Some(dst), &[0, dep]).with_mem(MemRef::cold_unique(seq))
    }

    fn fp_load(rule: CostRule, dst: u8, seq: u64) -> EmittedWord {
        banked_word(rule, Some(Reg::fp(dst)), &[Reg::gpr(0)]).with_mem(MemRef::cold_unique(seq))
    }

    fn fp_store(rule: CostRule, src: u8, seq: u64) -> EmittedWord {
        banked_word(rule, None, &[Reg::gpr(0), Reg::fp(src)]).with_mem(MemRef::cold_unique(seq))
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

    #[test]
    fn port_map_comes_from_the_profile() {
        let table = table();
        let m = Machine::from_table(&table).expect("machine");
        assert_eq!(m.pipes, 8);
        assert_eq!(m.dispatch_mops, 4);
        assert_eq!(m.dispatch_uops, 8);
        assert_eq!(m.window, 128);
        assert_eq!(m.mask("I").unwrap().count_ones(), 3);
        assert_eq!(m.mask("M").unwrap(), 1 << 4);
        assert_eq!(m.mask("L").unwrap(), (1 << 5) | (1 << 6));
        assert_eq!(m.mask("D").unwrap(), (1 << 7) | (1 << 8));
        assert_eq!(m.mask("B").unwrap(), 1 << 1);
        assert_eq!(
            m.mask("V0").unwrap() | m.mask("V1").unwrap(),
            m.mask("D").unwrap()
        );
        assert_eq!(
            m.mask("B").unwrap().count_ones()
                + m.mask("I").unwrap().count_ones()
                + m.mask("L").unwrap().count_ones()
                + m.mask("D").unwrap().count_ones(),
            m.dispatch_uops as u32
        );
        assert_eq!(m.caps[PortClass::B.index()], 2);
        assert_eq!(m.caps[PortClass::I.index()], 4);
        assert_eq!(m.caps[PortClass::M.index()], 2);
        assert_eq!(m.caps[PortClass::L.index()], 4);
        assert_eq!(m.caps[PortClass::V.index()], 4);
    }

    #[test]
    fn store_is_one_mop_two_uops_and_v0_v1_is_one() {
        let table = table();
        let m = Machine::from_table(&table).expect("machine");
        let store = m
            .uops_for(ports_for(CostRule::Store, &table).unwrap())
            .unwrap();
        assert_eq!(store.len(), 2, "store address uop + store data uop");
        assert_eq!(store[0].class, PortClass::L);
        assert_eq!(store[1].class, PortClass::V);
        assert_eq!(store[1].pipes, m.mask("D").unwrap());
        let call = m
            .uops_for(ports_for(CostRule::Call, &table).unwrap())
            .unwrap();
        assert_eq!(call.len(), 2);
        assert_eq!(call[0].class, PortClass::I);
        assert_eq!(call[1].class, PortClass::B);
        let asimd = m
            .uops_for(ports_for(CostRule::AsimdInt, &table).unwrap())
            .unwrap();
        assert_eq!(asimd.len(), 1, "`V0,V1` lists alternatives for one uop");
        assert_eq!(asimd[0].pipes, m.mask("D").unwrap());
        let alu = m
            .uops_for(ports_for(CostRule::Alu, &table).unwrap())
            .unwrap();
        assert_eq!(alu.len(), 1);
        assert_eq!(alu[0].pipes.count_ones(), 3, "I is three pipes");
    }

    #[test]
    fn three_alu_issue_in_one_cycle_and_a_fourth_waits() {
        let alu = |d: u8| word(CostRule::Alu, Some(d), &[0, 0]);
        let three = prog("f", vec![alu(1), alu(2), alu(3)]);
        assert_eq!(
            total(&three),
            1,
            "three independent ALU ops must all retire at cycle 1"
        );
        let four = prog("f", vec![alu(1), alu(2), alu(3), alu(4)]);
        assert_eq!(
            total(&four),
            2,
            "the fourth ALU op has no I pipe at cycle 0 (port_i = pipes 2-4)"
        );
        let five = prog("f", vec![alu(1), alu(2), alu(3), alu(4), alu(5)]);
        assert_eq!(total(&five), 2, "the fifth joins the fourth at cycle 1");
    }

    #[test]
    fn two_loads_issue_in_one_cycle_but_not_three() {
        let two = prog("f", vec![load_cold_unique(1, 0), load_cold_unique(2, 1)]);
        assert_eq!(
            total(&two),
            4,
            "both resolved loads retire at the L1 rank floor"
        );
        let three = prog(
            "f",
            vec![
                load_cold_unique(1, 0),
                load_cold_unique(2, 1),
                load_cold_unique(3, 2),
            ],
        );
        assert_eq!(total(&three), 5, "the third load waits a cycle for an AGU");
    }

    #[test]
    fn scalar_and_q_fp_memory_use_distinct_latency_and_throughput_rows() {
        let scalar_loads = prog(
            "f",
            vec![
                fp_load(CostRule::FpLoad, 0, 0),
                fp_load(CostRule::FpLoad, 1, 1),
            ],
        );
        let q_loads = prog(
            "f",
            vec![
                fp_load(CostRule::FpLoadQ, 0, 0),
                fp_load(CostRule::FpLoadQ, 1, 1),
            ],
        );
        assert_eq!(total(&scalar_loads), 5, "two scalar FP loads dual-issue");
        assert_eq!(
            total(&q_loads),
            7,
            "Q loads have six-cycle completion and only one may issue per cycle"
        );

        let scalar_stores = prog(
            "f",
            vec![
                fp_store(CostRule::FpStore, 0, 0),
                fp_store(CostRule::FpStore, 1, 1),
            ],
        );
        let q_stores = prog(
            "f",
            vec![
                fp_store(CostRule::FpStoreQ, 0, 0),
                fp_store(CostRule::FpStoreQ, 1, 1),
            ],
        );
        assert_eq!(total(&scalar_stores), 1, "two scalar FP stores dual-issue");
        assert_eq!(total(&q_stores), 2, "Q stores are sealed at one per cycle");
    }

    /// Register number 0 in each bank, used both ways round.
    ///
    /// Before P8R.1 the scheduler kept one 32-entry ready table, so a `v0`
    /// definition and an `x0` definition wrote the same slot: an `x0`
    /// consumer waited on an unrelated vector result (an invented
    /// dependency), and a `v0` consumer could be satisfied by an integer
    /// definition (a hidden one). Both directions are checked here.
    #[test]
    fn a_vector_and_a_general_register_of_the_same_number_are_independent() {
        // `fmul s0, s1, s2` is 3 cycles; `add x1, x0, x0` is 1.
        let fp_def = || banked_word(CostRule::FpMul, Some(Reg::fp(0)), &[Reg::fp(1), Reg::fp(2)]);
        let gpr_def = || word(CostRule::Alu, Some(Reg::gpr(0).num), &[1, 2]);

        // An `x0` consumer must not wait for the `v0` definition.
        let invented = prog("f", vec![fp_def(), word(CostRule::Alu, Some(3), &[0, 0])]);
        let baseline = prog("f", vec![fp_def(), word(CostRule::Alu, Some(3), &[4, 4])]);
        assert_eq!(
            total(&invented),
            total(&baseline),
            "an x0 consumer must not depend on a v0 definition"
        );

        // A `v0` consumer must still wait for the `v0` definition, and must
        // not be satisfied by an `x0` definition instead.
        let real = prog(
            "f",
            vec![
                fp_def(),
                banked_word(
                    CostRule::FpAddSub,
                    Some(Reg::fp(3)),
                    &[Reg::fp(0), Reg::fp(0)],
                ),
            ],
        );
        let hidden = prog(
            "f",
            vec![
                gpr_def(),
                banked_word(
                    CostRule::FpAddSub,
                    Some(Reg::fp(3)),
                    &[Reg::fp(0), Reg::fp(0)],
                ),
            ],
        );
        assert!(
            total(&real) > total(&hidden),
            "a v0 consumer must wait for the v0 definition ({} vs {})",
            total(&real),
            total(&hidden)
        );
    }

    #[test]
    fn store_data_uop_contends_with_asimd() {
        // Operand-free on purpose: this measures V-pipe occupancy against
        // the store data uop, not a dependency chain.
        let neon = || banked_word(CostRule::AsimdInt, None, &[]);
        // `asimd_int` is latency 2 (P8R.1 replaced the coarse `neon` row's
        // pessimistic 4 with the SOG's ASIMD basic-arithmetic group).
        let two_neon = prog("f", vec![neon(), neon()]);
        assert_eq!(total(&two_neon), 2, "both V pipes free: lat 2 from cycle 0");
        let with_store = prog("f", vec![store_stack(8), neon(), neon()]);
        assert_eq!(
            total(&with_store),
            3,
            "the store's data uop holds a V pipe, so one ASIMD op slips to cycle 1"
        );
        let with_alu = prog(
            "f",
            vec![word(CostRule::Alu, Some(1), &[0, 0]), neon(), neon()],
        );
        assert_eq!(total(&with_alu), 2);
    }

    #[test]
    fn dispatch_mops_bind_before_the_ports() {
        let alu = |d: u8| word(CostRule::Alu, Some(d), &[0, 0]);
        let five = prog(
            "f",
            vec![
                alu(1),
                alu(2),
                alu(3),
                store_stack(8),
                word(CostRule::Branch, None, &[]),
            ],
        );
        assert_eq!(
            total(&five),
            2,
            "the 5th Mop cannot dispatch at cycle 0 (dispatch_mops = 4)"
        );
        let four = prog(
            "f",
            vec![
                alu(1),
                alu(2),
                store_stack(8),
                word(CostRule::Branch, None, &[]),
            ],
        );
        assert_eq!(total(&four), 1, "four Mops / five uops fit one cycle");
    }

    #[test]
    fn mul_high_then_madd_shows_the_three_cycle_m_pipe_block() {
        let smulh = word(CostRule::MulHigh, Some(1), &[2, 3]);
        let madd = word(CostRule::Mul, Some(4), &[5, 6]);
        let after_high = prog("f", vec![smulh.clone(), madd.clone()]);
        assert_eq!(
            total(&after_high),
            11,
            "SMULH holds pipe M for 4 (thru 1/4) + 3 (note 5); MADD issues 7, lat 4"
        );
        let after_mul = prog("f", vec![madd.clone(), smulh.clone()]);
        assert_eq!(
            total(&after_mul),
            10,
            "MUL holds pipe M for 3 (thru 1/3) + 2 (note 4); SMULH issues 5, lat 5"
        );
        assert_eq!(total(&prog("f", vec![smulh])), 5);
        assert_eq!(total(&prog("f", vec![madd])), 4);
    }

    #[test]
    fn divide_blocks_a_subsequent_divide() {
        let sdiv = |d: u8| word(CostRule::Sdiv, Some(d), &[10, 11]);
        let udiv = |d: u8| word(CostRule::Udiv, Some(d), &[10, 11]);
        assert_eq!(
            total(&prog("f", vec![sdiv(1)])),
            20,
            "pinned pessimistic 20"
        );
        assert_eq!(
            total(&prog("f", vec![sdiv(1), sdiv(2)])),
            40,
            "the second divide cannot start until the first completes"
        );
        assert_eq!(
            total(&prog("f", vec![sdiv(1), udiv(2)])),
            40,
            "the block is on pipe M, not on one [latency] row"
        );
        assert_eq!(
            total(&prog(
                "f",
                vec![sdiv(1), word(CostRule::Alu, Some(2), &[0, 0])]
            )),
            20
        );
    }

    #[test]
    fn consumer_of_a_divide_waits_on_it() {
        let dependent = prog(
            "f",
            vec![
                word(CostRule::Udiv, Some(3), &[1, 2]),
                word(CostRule::Alu, Some(4), &[3, 3]),
            ],
        );
        let independent = prog(
            "f",
            vec![
                word(CostRule::Udiv, Some(3), &[1, 2]),
                word(CostRule::Alu, Some(4), &[8, 8]),
            ],
        );
        assert_eq!(total(&dependent), 21, "consumer issues at the divide's 20");
        assert_eq!(total(&independent), 20);
        let untagged = prog(
            "f",
            vec![
                word(CostRule::Udiv, None, &[]),
                word(CostRule::Alu, Some(4), &[3, 3]),
            ],
        );
        assert_eq!(
            total(&untagged),
            20,
            "no dst ⇒ no edge ⇒ the consumer is free"
        );
        assert!(total(&dependent) > total(&untagged));
    }

    #[test]
    fn dependence_chain_longer_than_the_window_does_not_reorder() {
        fn stream(n: usize) -> Vec<EmittedWord> {
            let mut code = vec![load_cold_unique(1, 0)];
            for j in 1..n {
                if j % 4 == 0 {
                    code.push(banked_word(CostRule::AsimdInt, None, &[]));
                } else {
                    code.push(word(CostRule::Alu, None, &[0]));
                }
            }
            code
        }
        let inside = prog("f", stream(128));
        assert_eq!(total(&inside), 33, "nothing crosses a 128-entry window");
        let across = prog("f", stream(129));
        assert_eq!(
            total(&across),
            34,
            "word 128 cannot cross the 128-entry reorder window"
        );
    }

    #[test]
    fn nzcv_raw_edge_is_charged_like_a_gpr_dependence() {
        let flags = prog(
            "f",
            vec![
                word_flags(CostRule::Alu, None, &[1, 2], FlagEffect::Write),
                word_flags(CostRule::Branch, None, &[], FlagEffect::Read),
            ],
        );
        let gpr = prog(
            "f",
            vec![
                word(CostRule::Alu, Some(1), &[0, 0]),
                word(CostRule::Branch, None, &[1]),
            ],
        );
        assert_eq!(
            total(&flags),
            total(&gpr),
            "cmp -> b.cond must cost what an equivalent GPR dependence costs: \
             flags {} vs gpr {}",
            total(&flags),
            total(&gpr)
        );
        let independent = prog(
            "f",
            vec![
                word(CostRule::Alu, None, &[1, 2]),
                word(CostRule::Branch, None, &[]),
            ],
        );
        assert!(
            total(&flags) > total(&independent),
            "the flag edge must lengthen the schedule: {} vs independent {}",
            total(&flags),
            total(&independent)
        );
    }

    #[test]
    fn renaming_removes_war_and_waw_flag_hazards_but_not_the_raw_edge() {
        const NZCV: u8 = 20;

        let dep_live = prog(
            "f",
            vec![
                word_flags(CostRule::Alu, None, &[9, 10], FlagEffect::Write),
                word_flags(CostRule::Alu, Some(11), &[], FlagEffect::Read),
                word_flags(CostRule::Alu, None, &[11, 12], FlagEffect::Write),
                word_flags(CostRule::Alu, Some(13), &[], FlagEffect::Read),
            ],
        );
        let dep_restored = prog(
            "f",
            vec![
                word(CostRule::Alu, Some(NZCV), &[9, 10]),
                word(CostRule::Alu, Some(11), &[NZCV]),
                word(CostRule::Alu, Some(NZCV), &[11, 12]),
                word(CostRule::Alu, Some(13), &[NZCV]),
            ],
        );
        assert_eq!(
            total(&dep_restored),
            4,
            "the GPR counterfactual serializes all four words"
        );
        assert_eq!(
            total(&dep_live),
            total(&dep_restored),
            "a dependent flag chain is a RAW chain and must cost the same as \
             its GPR counterfactual: live {} vs {}",
            total(&dep_live),
            total(&dep_restored)
        );

        let waw_live = prog(
            "f",
            vec![
                word_flags(CostRule::Alu, None, &[1, 2], FlagEffect::Write),
                word_flags(CostRule::Alu, None, &[3, 4], FlagEffect::Write),
                word_flags(CostRule::Alu, None, &[5, 6], FlagEffect::Write),
                word_flags(CostRule::Alu, None, &[7, 8], FlagEffect::Write),
            ],
        );
        let waw_restored = prog(
            "f",
            vec![
                word(CostRule::Alu, Some(NZCV), &[1, 2]),
                word(CostRule::Alu, Some(NZCV), &[3, 4]),
                word(CostRule::Alu, Some(NZCV), &[5, 6]),
                word(CostRule::Alu, Some(NZCV), &[7, 8]),
            ],
        );
        assert!(
            total(&waw_live) <= total(&waw_restored),
            "renaming must remove the WAW hazard: live {} should not exceed \
             the single-register counterfactual {}",
            total(&waw_live),
            total(&waw_restored)
        );

        let raw_live = prog(
            "f",
            vec![
                word_flags(CostRule::Alu, None, &[1, 2], FlagEffect::Read),
                word_flags(CostRule::Alu, None, &[3, 4], FlagEffect::Read),
                word_flags(CostRule::Alu, None, &[5, 6], FlagEffect::Read),
                word_flags(CostRule::Alu, None, &[7, 8], FlagEffect::Read),
            ],
        );
        let raw_restored = prog(
            "f",
            vec![
                word(CostRule::Alu, None, &[1, NZCV]),
                word(CostRule::Alu, None, &[3, NZCV]),
                word(CostRule::Alu, None, &[5, NZCV]),
                word(CostRule::Alu, None, &[7, NZCV]),
            ],
        );
        assert_eq!(
            total(&raw_live),
            total(&raw_restored),
            "flag reads with no writer wait on nothing: {} vs {}",
            total(&raw_live),
            total(&raw_restored)
        );

        assert!(total(&dep_live) > total(&waw_live));
    }

    #[test]
    fn sp_raw_edge_holds_while_an_xzr_read_waits_on_nothing() {
        let sp_then_load = prog(
            "f",
            vec![
                word(CostRule::Alu, Some(MEM_SP_REG), &[MEM_SP_REG]),
                load_stack(1, 0),
                word(CostRule::Alu, Some(2), &[1, 1]),
            ],
        );
        const FAKE_SP: u8 = 20;
        let as_gpr = prog(
            "f",
            vec![
                word(CostRule::Alu, Some(FAKE_SP), &[FAKE_SP]),
                word(CostRule::Load, Some(1), &[MEM_SP_REG, FAKE_SP]).with_mem(MemRef::stack(0)),
                word(CostRule::Alu, Some(2), &[1, 1]),
            ],
        );
        assert_eq!(
            total(&sp_then_load),
            total(&as_gpr),
            "an SP write must serialize a later stack access exactly as the \
             equivalent GPR dependence does: {} vs {}",
            total(&sp_then_load),
            total(&as_gpr)
        );
        let without_sp = prog(
            "f",
            vec![load_stack(1, 0), word(CostRule::Alu, Some(2), &[1, 1])],
        );
        assert!(
            total(&sp_then_load) > total(&without_sp),
            "the SP write must cost a cycle on the critical path: {} vs {}",
            total(&sp_then_load),
            total(&without_sp)
        );

        let xzr_reader = prog(
            "f",
            vec![
                word(CostRule::Alu, Some(MEM_SP_REG), &[MEM_SP_REG]),
                word(CostRule::Store, None, &[MEM_SP_REG, MEM_SP_REG])
                    .with_mem(MemRef::cold_stable(MEM_SP_REG, 64)),
                word(CostRule::Alu, Some(3), &[4, 5]),
            ],
        );
        let xzr_without_sp = prog(
            "f",
            vec![
                word(CostRule::Store, None, &[MEM_SP_REG, MEM_SP_REG])
                    .with_mem(MemRef::cold_stable(MEM_SP_REG, 64)),
                word(CostRule::Alu, Some(3), &[4, 5]),
            ],
        );
        assert_eq!(
            total(&xzr_reader),
            total(&xzr_without_sp),
            "a word reading register 31 as XZR must not inherit the SP edge: \
             {} vs {}",
            total(&xzr_reader),
            total(&xzr_without_sp)
        );

        let hit = prog(
            "f",
            vec![
                load_stack(1, 8),
                word(CostRule::Alu, Some(1), &[1, 1]),
                load_stack_after(2, 8, 1),
            ],
        );
        let epoch = prog(
            "f",
            vec![
                load_stack(1, 8),
                word(CostRule::Alu, Some(1), &[1, 1]),
                word(CostRule::Alu, Some(MEM_SP_REG), &[MEM_SP_REG]),
                load_stack_after(2, 8, 1),
            ],
        );
        assert_eq!(
            total(&epoch),
            total(&hit),
            "an SP write changes the frame identity but does not evict L1D"
        );
        let fresh_after_sp = prog(
            "f",
            vec![
                load_stack(1, 8),
                word(CostRule::Alu, Some(1), &[1, 1]),
                word(CostRule::Alu, Some(MEM_SP_REG), &[MEM_SP_REG]),
                load_stack_after(2, 4096, 1),
            ],
        );
        let fresh_no_sp = prog(
            "f",
            vec![
                load_stack(1, 8),
                word(CostRule::Alu, Some(1), &[1, 1]),
                load_stack_after(2, 4096, 1),
            ],
        );
        assert_eq!(
            total(&fresh_after_sp),
            total(&fresh_no_sp),
            "an SP write may not cost anything where there was no reuse to lose"
        );
    }

    fn stack_load_of_width(dst: u8, offset: u64, width_bytes: u8) -> EmittedWord {
        use crate::encode;
        let enc = match width_bytes {
            1 => encode::enc_ldrb_imm(dst, MEM_SP_REG, 0),
            2 => encode::enc_ldrh_imm(dst, MEM_SP_REG, 0),
            4 => encode::enc_ldr_w_imm(dst, MEM_SP_REG, 0),
            8 => encode::enc_ldr_x_imm(dst, MEM_SP_REG, 0),
            w => panic!("no encoder for a {w}-byte load"),
        };
        EmittedWord::gpr(enc, String::new(), CostRule::Load, Some(dst), &[MEM_SP_REG])
            .with_mem(MemRef::stack(offset))
    }

    fn stack_store_of_width(offset: u64, width_bytes: u8) -> EmittedWord {
        use crate::encode;
        let enc = match width_bytes {
            1 => encode::enc_strb_imm(0, MEM_SP_REG, 0),
            2 => encode::enc_strh_imm(0, MEM_SP_REG, 0),
            4 => encode::enc_str_w_imm(0, MEM_SP_REG, 0),
            8 => encode::enc_str_x_imm(0, MEM_SP_REG, 0),
            w => panic!("no encoder for a {w}-byte store"),
        };
        EmittedWord::gpr(enc, String::new(), CostRule::Store, None, &[MEM_SP_REG, 0])
            .with_mem(MemRef::stack(offset))
    }

    #[test]
    fn access_width_is_tagged_from_the_emitted_word() {
        assert_eq!(stack_load_of_width(1, 0, 8).access_bytes, 8);
        assert_eq!(stack_load_of_width(1, 0, 4).access_bytes, 4);
        assert_eq!(stack_load_of_width(1, 0, 2).access_bytes, 2);
        assert_eq!(stack_load_of_width(1, 0, 1).access_bytes, 1);
        assert_eq!(stack_store_of_width(0, 8).access_bytes, 8);
        assert_eq!(stack_store_of_width(0, 1).access_bytes, 1);
        assert_eq!(word(CostRule::Alu, Some(1), &[0, 0]).access_bytes, 0);
        assert_eq!(load_stack(1, 0).access_bytes, 0, "synthetic word=0 stream");
    }

    #[test]
    fn a_store_straddling_the_16_b_boundary_costs_more() {
        let table = table();
        let place = placement();
        let point = SweepPoint::pinned(&table);
        let aligned = prog("f", vec![stack_store_of_width(8, 8)]);
        let straddle = prog("f", vec![stack_store_of_width(12, 8)]);
        let a = score_program(&aligned, &table, &place)
            .expect("aligned")
            .total_proxy_cycles;
        let s = score_program(&straddle, &table, &place)
            .expect("straddle")
            .total_proxy_cycles;
        assert!(
            s > a,
            "a 16 B-straddling store {s} must cost more than an aligned one {a}"
        );
        assert_eq!(
            s - a,
            point.get("store_boundary_cross_penalty"),
            "the charge is exactly the swept penalty"
        );
        let lo = point.with("store_boundary_cross_penalty", 1);
        let cheap = score_program_at(&straddle, &table, &place, &lo)
            .expect("lo")
            .total_proxy_cycles;
        assert!(
            cheap < s,
            "store_boundary_cross_penalty must reach the schedule: {cheap} vs {s}"
        );
        let narrow = prog("f", vec![stack_store_of_width(12, 4)]);
        assert_eq!(
            score_program(&narrow, &table, &place)
                .expect("narrow")
                .total_proxy_cycles,
            a,
            "a 4-byte store at offset 12 ends exactly on the boundary"
        );
    }

    #[test]
    fn a_load_straddling_the_64_b_line_costs_more_than_one_inside_it() {
        let table = table();
        let place = placement();
        let point = SweepPoint::pinned(&table);
        let inside = prog("f", vec![stack_load_of_width(1, 56, 8)]);
        let across = prog("f", vec![stack_load_of_width(1, 60, 8)]);
        let i = score_program(&inside, &table, &place)
            .expect("inside")
            .total_proxy_cycles;
        let x = score_program(&across, &table, &place)
            .expect("across")
            .total_proxy_cycles;
        assert!(
            x > i,
            "a line-crossing load {x} must cost more than one inside the line {i}"
        );
        assert_eq!(x - i, point.get("load_line_cross_penalty"));
        let lo = point.with("load_line_cross_penalty", 1);
        assert!(
            score_program_at(&across, &table, &place, &lo)
                .expect("lo")
                .total_proxy_cycles
                < x,
            "load_line_cross_penalty must reach the schedule"
        );
    }

    #[test]
    fn crosses_boundary_quantifies_over_every_permitted_sp() {
        let sp = crate::codegen::FRAME_SP_ALIGN_BYTES;
        assert_eq!(sp, 16);
        for off in (0u64..256).step_by(8) {
            for w in [1u64, 2, 4, 8] {
                assert!(
                    !crosses_boundary(off, w, 16, sp),
                    "off {off} width {w} must not cross 16 B"
                );
                assert!(
                    !crosses_boundary(off, w, 64, sp),
                    "off {off} width {w} must not cross 64 B"
                );
            }
        }
        assert!(crosses_boundary(12, 8, 16, sp));
        assert!(!crosses_boundary(12, 4, 16, sp));
        assert!(crosses_boundary(15, 2, 16, sp));
        assert!(crosses_boundary(12, 8, 64, sp), "some permitted sp crosses");
        assert!(
            !crosses_boundary(12, 8, 64, 64),
            "with sp pinned mod 64 the verdict is exact and there is no crossing"
        );
        assert!(crosses_boundary(60, 8, 64, sp));
        assert!(crosses_boundary(60, 8, 64, 64));
        assert!(!crosses_boundary(60, 0, 64, sp));
        assert!(!crosses_boundary(60, 8, 0, sp));
    }

    #[test]
    fn a_cold_base_is_not_decided_and_charges_nothing() {
        let table = table();
        let point = SweepPoint::pinned(&table);
        let stack = stack_store_of_width(12, 8);
        assert_eq!(
            alignment_penalty(&stack, &table, &point).expect("stack"),
            point.get("store_boundary_cross_penalty")
        );
        let cold = EmittedWord::gpr(
            crate::encode::enc_str_x_imm(0, 28, 0),
            String::new(),
            CostRule::Store,
            None,
            &[28, 0],
        )
        .with_mem(MemRef::cold_stable(28, 12));
        assert_eq!(cold.access_bytes, 8, "the width is still known");
        assert_eq!(
            alignment_penalty(&cold, &table, &point).expect("cold"),
            0,
            "an unproven base alignment charges nothing (decision 1611)"
        );
        let unique = EmittedWord::gpr(
            crate::encode::enc_str_x_imm(0, 9, 0),
            String::new(),
            CostRule::Store,
            None,
            &[9],
        )
        .with_mem(MemRef::cold_unique(0));
        assert_eq!(
            alignment_penalty(&unique, &table, &point).expect("unique"),
            0
        );
    }

    #[test]
    fn stack_accesses_never_straddle_either_boundary_on_the_corpus() {
        use crate::cost::stage::codegen_cost_stage_with_placement;
        use crate::opts::win::discover_cost_corpus;

        let table = table();
        let point = SweepPoint::pinned(&table);
        let corpus = discover_cost_corpus();
        assert!(!corpus.is_empty(), "cost corpus empty");

        let mut stack = 0u64;
        let mut cold = 0u64;
        let mut charged = 0u64;
        let mut untagged_ordered = 0u64;
        for path in &corpus {
            let case = path
                .parent()
                .unwrap()
                .file_name()
                .unwrap()
                .to_string_lossy();
            let (program, _place) = codegen_cost_stage_with_placement(path)
                .unwrap_or_else(|e| panic!("codegen {case}: {e}"));
            for (key, f) in &program.fns {
                for ew in &f.code {
                    if !ew.rule.is_load() && !ew.rule.is_store() {
                        assert_eq!(
                            ew.access_bytes,
                            0,
                            "{case}/{key}: non-memory `{}` carries a width: {}",
                            ew.rule.as_str(),
                            ew.text
                        );
                        continue;
                    }
                    assert!(
                        ew.access_bytes > 0,
                        "{case}/{key}: `{}` has no transfer width — extend \
                         encode::access_width_bytes for it: {}",
                        ew.rule.as_str(),
                        ew.text
                    );
                    assert!(
                        ew.access_bytes <= crate::codegen::FRAME_SLOT_BYTES as u8,
                        "{case}/{key}: {}-byte access is wider than a frame slot: {}",
                        ew.access_bytes,
                        ew.text
                    );
                    let Some(m) = ew.mem else {
                        assert!(
                            ew.rule.is_crosscore(),
                            "{case}/{key}: emitted `{}` carries no MemRef — tag the \
                             emit site, or the memory model scores it as Unresolved \
                             and §4.5 declines to decide it: {}",
                            ew.rule.as_str(),
                            ew.text
                        );
                        untagged_ordered += 1;
                        continue;
                    };
                    match m.class {
                        MemClass::Stack => {
                            stack += 1;
                            assert_eq!(
                                m.key % u64::from(ew.access_bytes),
                                0,
                                "{case}/{key}: frame offset {} is not a multiple of \
                                 the {}-byte width: {}",
                                m.key,
                                ew.access_bytes,
                                ew.text
                            );
                        }
                        MemClass::Cold => cold += 1,
                    }
                    if alignment_penalty(ew, &table, &point).expect("align") > 0 {
                        charged += 1;
                    }
                }
            }
        }
        assert!(stack > 0, "corpus has no Stack accesses to decide");
        assert_eq!(
            charged, 0,
            "the fixed frame produced {charged} straddling accesses — the \
             §4.5 terms are supposed to be unreachable by construction"
        );
        eprintln!(
            "SOG §4.5 reach over the cost-* corpus: {stack} Stack accesses decided \
             (0 straddling), {cold} Cold accesses not decided (decision 1611), \
             {untagged_ordered} ordered accesses (`LDAR`/`STLR`) carrying no MemRef \
             at all (plans/M20.md item M)"
        );
    }

    #[test]
    fn dependent_chain_longer_than_independent_pair() {
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
        assert!(total(&dependent) > total(&independent));
    }

    #[test]
    fn eliding_load_use_shrinks_total() {
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
        assert!(total(&with_load) > total(&without_load));
    }

    #[test]
    fn empty_fn_is_zero() {
        let p = prog("empty", Vec::new());
        let r = score(&p);
        assert_eq!(r.total_proxy_cycles, 0);
        assert_eq!(r.total_words, 0);
        assert_eq!(r.fns.len(), 1);
        assert_eq!(r.fns[0].proxy_cycles, 0);
        assert_eq!(r.fns[0].words, 0);
        assert!(r.fns[0].terms.is_empty());
    }

    #[test]
    fn words_equals_word_count_and_term_sum() {
        let p = prog(
            "f",
            vec![
                word(CostRule::Alu, Some(1), &[0, 0]),
                load_stack(2, 8),
                word(CostRule::Branch, None, &[]),
                word(CostRule::Alu, Some(3), &[2, 2]),
            ],
        );
        let r = score(&p);
        assert_eq!(r.fns[0].words, 4);
        assert_eq!(r.total_words, 4);
        let term_sum: u64 = r.fns[0]
            .terms
            .iter()
            .filter(|(name, _)| !name.starts_with("Mem level="))
            .map(|(_, count)| *count)
            .sum();
        assert_eq!(
            term_sum, r.fns[0].words,
            "Σ instruction terms must equal words"
        );
    }

    #[test]
    fn footprint_growth_scales_through_reuse_distance() {
        fn chain(offsets: &[u64]) -> Vec<EmittedWord> {
            let mut code = Vec::new();
            for (i, &off) in offsets.iter().enumerate() {
                let dst = (i as u8) + 1;
                if i == 0 {
                    code.push(load_stack(dst, off));
                } else {
                    code.push(load_stack_after(dst, off, i as u8));
                }
                code.push(word(CostRule::Alu, Some(dst), &[dst, dst]));
            }
            code
        }
        let one_line = total(&prog("f", chain(&[0, 8, 16, 24, 32, 40])));
        let six_lines = total(&prog("f", chain(&[0, 64, 128, 192, 256, 320])));
        assert_eq!(
            six_lines, one_line,
            "frame homes start resident; cache capacity is priced by footprint, not a synthetic trace"
        );
        let five = total(&prog("f", chain(&[0, 64, 128, 192, 256])));
        let four = total(&prog("f", chain(&[0, 64, 128, 192])));
        assert_eq!(
            six_lines - five,
            five - four,
            "each extra line costs the same compulsory differential"
        );
        assert_eq!(
            five - four,
            5,
            "one additional resident stack access and its dependency"
        );
    }

    #[test]
    fn dead_word_never_lowers_schedule() {
        use crate::encode::{Cond, enc_b, enc_b_cond};

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
                "mem_heavy",
                vec![
                    load_stack(1, 0),
                    load_stack(2, 8),
                    load_stack(3, 16),
                    load_stack(4, 24),
                    load_stack(5, 32),
                ],
            ),
            (
                "mpipe",
                vec![
                    word(CostRule::MulHigh, Some(1), &[2, 3]),
                    word(CostRule::Mul, Some(4), &[5, 6]),
                    word(CostRule::Sdiv, Some(7), &[8, 9]),
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

        let dead = word(CostRule::Alu, Some(20), &[21, 21]);
        for (name, code) in cases {
            let base = score(&prog("f", code.clone()));
            let mut grown = code.clone();
            grown.push(dead.clone());
            let after = score(&prog("f", grown));
            assert!(
                after.total_proxy_cycles >= base.total_proxy_cycles,
                "{name}: dead word lowered schedule {} -> {}",
                base.total_proxy_cycles,
                after.total_proxy_cycles
            );
            assert_eq!(
                after.total_words,
                base.total_words + 1,
                "{name}: words must count the added dead word"
            );
        }
    }

    #[test]
    fn score_sets_owner_from_classify() {
        let code = vec![word(CostRule::Alu, Some(1), &[0, 0])];
        let mut fns = BTreeMap::new();
        for key in ["checked_add", "__wrela_abort", "BlkDriver.on_turn"] {
            fns.insert(
                key.to_string(),
                CodegenFn {
                    frame_size: 0,
                    code: code.clone(),
                    relocs: Vec::new(),
                    regions: Vec::new(),
                },
            );
        }
        let p = CodegenProgram {
            fns,
            rodata: Vec::new(),
            ..Default::default()
        };
        let r = score(&p);
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
    fn report_copies_profile_identity_from_table() {
        let table = table();
        let r = score(&prog("f", vec![word(CostRule::Alu, Some(1), &[0, 0])]));
        assert_eq!(r.version, 3);
        assert_eq!(r.profile, "a76-pi5");
        assert_eq!(r.pipelines, 8);
        assert_eq!(r.dispatch_mops, 4);
        assert_eq!(r.dispatch_uops, 8);
        assert_eq!(r.reorder_window, 128);
        assert_eq!(r.digest, table.table_digest());
        assert_eq!(r.provenance, table.provenance_digest());
        assert_eq!(r.provenance_summary, table.provenance_summary());
    }

    #[test]
    fn alu_and_mem_can_dual_issue_under_cap() {
        let mixed = prog(
            "f",
            vec![
                word(CostRule::Alu, Some(1), &[0, 0]),
                load_cold_unique(2, 0),
            ],
        );
        assert_eq!(total(&mixed), 4);
    }

    #[test]
    fn branch_charges_the_mispredict_penalty_scaled_by_measured_bias() {
        let table = table();
        assert_eq!(
            total(&prog("f", vec![word(CostRule::Branch, None, &[])])),
            1,
            "no bias information ⇒ latency only"
        );
        let point = SweepPoint::pinned(&table);
        assert_eq!(branch_mispredict_charge(&table, &point, None), 0);
        assert_eq!(
            table.branch_row("mispredict_penalty").unwrap().value,
            14,
            "the profile pins the pessimistic end of the 11-14 bracket"
        );
        let even = BranchBias::from_distinct_counts(5, 5).expect("bias");
        assert_eq!(branch_mispredict_charge(&table, &point, Some(even)), 14);
        assert_eq!(
            branch_mispredict_charge(&table, &point.with("mispredict_penalty", 11), Some(even)),
            11
        );
        let skewed = BranchBias::from_distinct_counts(1, 999).expect("bias");
        assert!(branch_mispredict_charge(&table, &point, Some(skewed)) <= 1);
    }

    #[test]
    fn abort_val_uses_table_latency() {
        assert_eq!(
            total(&prog("f", vec![word(CostRule::AbortVal, None, &[0])])),
            1
        );
    }

    #[test]
    fn stack_hit_cheaper_than_a_different_line() {
        let hit = prog(
            "f",
            vec![
                load_stack(1, 8),
                word(CostRule::Alu, Some(1), &[1, 1]),
                load_stack_after(2, 8, 1),
                word(CostRule::Alu, Some(3), &[2, 2]),
            ],
        );
        let same_line = prog(
            "f",
            vec![
                load_stack(1, 8),
                word(CostRule::Alu, Some(1), &[1, 1]),
                load_stack_after(2, 16, 1),
                word(CostRule::Alu, Some(3), &[2, 2]),
            ],
        );
        let other_line = prog(
            "f",
            vec![
                load_stack(1, 8),
                word(CostRule::Alu, Some(1), &[1, 1]),
                load_stack_after(2, 128, 1),
                word(CostRule::Alu, Some(3), &[2, 2]),
            ],
        );
        assert_eq!(
            total(&hit),
            total(&same_line),
            "offsets 8 and 16 are one 64 B line"
        );
        assert_eq!(
            total(&hit),
            total(&other_line),
            "stack homes are initially resident; footprint capacity is separate"
        );
    }

    #[test]
    fn a_store_no_longer_invalidates_its_reload() {
        let reuse = prog(
            "f",
            vec![
                load_stack(1, 8),
                word(CostRule::Alu, Some(1), &[1, 1]),
                load_stack_after(2, 8, 1),
                word(CostRule::Alu, Some(3), &[2, 2]),
            ],
        );
        let after_store = prog(
            "f",
            vec![
                load_stack(1, 8),
                word(CostRule::Alu, Some(1), &[1, 1]),
                store_stack(8),
                load_stack_after(2, 8, 1),
                word(CostRule::Alu, Some(3), &[2, 2]),
            ],
        );
        assert_eq!(
            total(&after_store),
            total(&reuse),
            "the reload forwards at lat_l1d_hit, so the store is off the \
             critical path rather than an invalidation"
        );
        let table = table();
        let lo = SweepPoint::pinned(&table).with("store_to_load_forwarding", 1);
        let cheap = score_program_at(&after_store, &table, &placement(), &lo).expect("lo");
        assert!(
            cheap.total_proxy_cycles < total(&after_store),
            "forwarding must be swept end to end: {} vs {}",
            cheap.total_proxy_cycles,
            total(&after_store)
        );
    }

    #[test]
    fn call_clears_mem_reuse_window() {
        let hit = prog(
            "f",
            vec![
                load_stack(1, 8),
                word(CostRule::Alu, Some(1), &[1, 1]),
                load_stack_after(2, 8, 1),
                word(CostRule::Alu, Some(3), &[2, 2]),
            ],
        );
        let after_call = prog(
            "f",
            vec![
                load_stack(1, 8),
                word(CostRule::Alu, Some(1), &[1, 1]),
                word(CostRule::Call, Some(0), &[]),
                load_stack_after(2, 8, 1),
                word(CostRule::Alu, Some(3), &[2, 2]),
            ],
        );
        assert!(
            total(&after_call) >= total(&hit),
            "a call remains a control boundary but does not flush L1D: {} vs {}",
            total(&after_call),
            total(&hit)
        );
        assert_eq!(
            score(&after_call).fns[0].terms.get("Mem level=l1_hit"),
            score(&hit).fns[0].terms.get("Mem level=l1_hit")
        );
    }

    #[test]
    fn cold_unique_uses_the_same_removal_safe_rank_floor() {
        let two = prog(
            "f",
            vec![
                load_cold_unique(1, 0),
                word(CostRule::Alu, Some(1), &[1, 1]),
                load_cold_unique_after(2, 1, 1),
                word(CostRule::Alu, Some(3), &[2, 2]),
            ],
        );
        assert_eq!(total(&two), 10);
        let stack_hit = prog(
            "f",
            vec![
                load_stack(1, 8),
                word(CostRule::Alu, Some(1), &[1, 1]),
                load_stack_after(2, 8, 1),
                word(CostRule::Alu, Some(3), &[2, 2]),
            ],
        );
        assert_eq!(
            total(&two),
            total(&stack_hit),
            "resolved static and frame targets share the removal-safe L1 floor"
        );
    }

    #[test]
    fn missing_memref_is_cold_miss() {
        let untagged = prog("f", vec![word(CostRule::Load, Some(1), &[0])]);
        assert_eq!(total(&untagged), 4);
    }

    #[test]
    fn five_way_conflict_is_priced_only_by_the_footprint_term() {
        const SET_STRIDE: u64 = 256 * 64;
        let serial = |offsets: &[u64]| -> CodegenProgram {
            let mut code = Vec::new();
            for (i, &off) in offsets.iter().enumerate() {
                let dst = (i as u8) + 1;
                if i == 0 {
                    code.push(load_stack(dst, off));
                } else {
                    code.push(load_stack_after(dst, off, i as u8));
                }
                code.push(word(CostRule::Alu, Some(dst), &[dst, dst]));
            }
            code.push(load_stack_after(20, offsets[0], offsets.len() as u8));
            prog("f", code)
        };
        let conflict: Vec<u64> = (0..5).map(|k| k * SET_STRIDE).collect();
        let spread: Vec<u64> = (0..5).map(|k| k * 64).collect();
        let a = total(&serial(&conflict));
        let b = total(&serial(&spread));
        assert_eq!(
            a, b,
            "block rank must not infer a cache trace from static source order"
        );
    }

    #[test]
    fn call_result_waits_on_call_retire() {
        let with_dep = prog(
            "f",
            vec![
                word(CostRule::Call, Some(0), &[]),
                word(CostRule::Alu, Some(1), &[0, 0]),
            ],
        );
        assert_eq!(total(&with_dep), 2);
        let no_dep = prog(
            "f",
            vec![
                word(CostRule::Call, Some(0), &[]),
                word(CostRule::Alu, Some(1), &[2, 2]),
            ],
        );
        assert_eq!(total(&no_dep), 2);
    }

    #[test]
    fn mid_stream_branch_delays_followers() {
        let mid = prog(
            "f",
            vec![
                word(CostRule::Branch, None, &[]),
                word(CostRule::Alu, Some(1), &[2, 2]),
            ],
        );
        assert_eq!(total(&mid), 2);
        assert_eq!(
            total(&prog("f", vec![word(CostRule::Branch, None, &[])])),
            1
        );
    }

    #[test]
    fn adrp_dual_issues_with_load() {
        let mixed = prog(
            "f",
            vec![word(CostRule::Adrp, Some(1), &[]), load_cold_unique(2, 0)],
        );
        assert_eq!(total(&mixed), 4);
        let two_adrp = prog(
            "f",
            vec![
                word(CostRule::Adrp, Some(1), &[]),
                word(CostRule::Adrp, Some(2), &[]),
            ],
        );
        assert_eq!(total(&two_adrp), 1);
    }

    #[test]
    fn sp_dst_clears_mem_reuse_window() {
        let hit = prog(
            "f",
            vec![
                load_stack(1, 8),
                word(CostRule::Alu, Some(1), &[1, 1]),
                load_stack_after(2, 8, 1),
                word(CostRule::Alu, Some(3), &[2, 2]),
            ],
        );
        let after_sp = prog(
            "f",
            vec![
                load_stack(1, 8),
                word(CostRule::Alu, Some(1), &[1, 1]),
                word(CostRule::Alu, Some(MEM_SP_REG), &[MEM_SP_REG]),
                load_stack_after(2, 8, 1),
                word(CostRule::Alu, Some(3), &[2, 2]),
            ],
        );
        assert_eq!(
            total(&after_sp),
            total(&hit),
            "SP adjustment does not flush the modeled data cache"
        );
    }

    #[test]
    fn store_then_sp_adjust_reload_misses() {
        let after_store = prog(
            "f",
            vec![
                load_stack(1, 8),
                word(CostRule::Alu, Some(1), &[1, 1]),
                store_stack(16),
                load_stack_after(2, 8, 1),
                word(CostRule::Alu, Some(3), &[2, 2]),
            ],
        );
        let after_sp = prog(
            "f",
            vec![
                load_stack(1, 8),
                word(CostRule::Alu, Some(1), &[1, 1]),
                store_stack(16),
                word(CostRule::Alu, Some(MEM_SP_REG), &[MEM_SP_REG]),
                load_stack_after(2, 8, 1),
                word(CostRule::Alu, Some(3), &[2, 2]),
            ],
        );
        assert_eq!(
            total(&after_sp),
            total(&after_store),
            "SP adjustment does not evict a forwarded/cache-resident line"
        );
    }

    #[test]
    fn memref_base_not_in_srcs_fails_closed() {
        let table = table();
        let place = placement();
        let bad_stack = prog(
            "f",
            vec![word(CostRule::Load, Some(1), &[0]).with_mem(MemRef::stack(8))],
        );
        let err = score_program(&bad_stack, &table, &place).expect_err("stack base");
        assert!(err.contains("base register"), "unexpected err: {err}");
        let bad_cold = prog(
            "f",
            vec![word(CostRule::Load, Some(1), &[0]).with_mem(MemRef::cold_stable(28, 16))],
        );
        let err = score_program(&bad_cold, &table, &place).expect_err("cold base");
        assert!(err.contains("base register"), "unexpected err: {err}");
        let unique = prog("f", vec![load_cold_unique(1, 0)]);
        assert!(score_program(&unique, &table, &place).is_ok());
    }

    fn word_enc(enc: u32, rule: CostRule, dst: Option<u8>, srcs: &[u8]) -> EmittedWord {
        EmittedWord::gpr(enc, String::new(), rule, dst, srcs)
    }

    #[test]
    fn basic_blocks_split_on_branch_targets_and_fallthrough() {
        use crate::encode::{Cond, enc_b, enc_b_cond};
        let code = vec![
            word_enc(0, CostRule::Alu, Some(1), &[0]),
            word_enc(enc_b_cond(Cond::Eq, 12), CostRule::Branch, None, &[])
                .with_flags(FlagEffect::Read),
            word_enc(0, CostRule::Alu, Some(2), &[1]),
            word_enc(enc_b(8), CostRule::Branch, None, &[]),
            word_enc(0, CostRule::Alu, Some(3), &[0]),
            word_enc(0, CostRule::Alu, Some(4), &[0]),
        ];
        let ranges = basic_block_ranges(&code);
        assert_eq!(ranges, vec![(0, 2), (2, 4), (4, 5), (5, 6)]);
    }

    #[test]
    fn flat_equiv_block_sum_matches_fn_schedule() {
        use crate::encode::{Cond, enc_b, enc_b_cond};
        let table = table();
        let place = placement();

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
            let report =
                score_program(&p, &table, &place).unwrap_or_else(|e| panic!("{name}: {e}"));
            let fn_sched = report.fns[0].proxy_cycles;
            let blocks = block_schedule_lengths("f", &code, &table, &place)
                .unwrap_or_else(|e| panic!("{name} blocks: {e}"));
            let sum: u64 = blocks.iter().sum();
            assert_eq!(
                sum, fn_sched,
                "{name}: Σ s(b)={sum} (blocks {blocks:?}) != fn schedule {fn_sched}"
            );
            assert_eq!(report.total_proxy_cycles, fn_sched);
        }
    }

    #[test]
    fn every_cost_rule_scores() {
        let table = table();
        let place = placement();
        for &rule in CostRule::ALL {
            let ew = if rule.is_load() || rule.is_store() {
                word(rule, Some(1), &[31]).with_mem(MemRef::stack(0))
            } else {
                word(rule, Some(1), &[2, 3])
            };
            let r = score_program(&prog("f", vec![ew]), &table, &place)
                .unwrap_or_else(|e| panic!("{}: {e}", rule.as_str()));
            assert!(
                r.total_proxy_cycles >= 1,
                "{} scored {} — every emitted word occupies at least one cycle",
                rule.as_str(),
                r.total_proxy_cycles
            );
        }
    }

    #[test]
    fn pinned_point_scoring_matches_explicit_point() {
        let table = table();
        let place = placement();
        let p = prog(
            "f",
            vec![
                word(CostRule::Sdiv, Some(1), &[2, 3]),
                load_stack(4, 8),
                word(CostRule::LoadAcquire, Some(5), &[31]).with_mem(MemRef::stack(16)),
            ],
        );
        let pinned = score_program(&p, &table, &place).expect("pinned");
        let point = SweepPoint::pinned(&table);
        let explicit = score_program_at(&p, &table, &place, &point).expect("explicit");
        assert_eq!(pinned, explicit);
        let lo = point.with("divide_x_latency", 5);
        let cheap = score_program_at(&p, &table, &place, &lo).expect("lo");
        assert!(
            cheap.total_proxy_cycles < pinned.total_proxy_cycles,
            "divide_x_latency must reach the schedule: {} vs {}",
            cheap.total_proxy_cycles,
            pinned.total_proxy_cycles
        );
    }
}
