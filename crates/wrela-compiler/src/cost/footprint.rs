use std::collections::{BTreeMap, BTreeSet};

use crate::codegen::{CodegenFn, CodegenProgram};
use crate::linked::LinkedProgram;
use crate::placement::PlacementTable;

use super::attr::{AttrTarget, classify_target};
use super::rule::MemRef;
use super::score::basic_block_ranges;
use super::sweep::SweepPoint;
use super::table::CostTable;

pub const PAGE_BYTES: u64 = 4096;

#[derive(Clone, Copy)]
pub enum HotBlocks<'a> {
    All,
    Measured(&'a dyn Fn(&str, usize) -> bool),
}

impl Default for HotBlocks<'_> {
    fn default() -> Self {
        HotBlocks::All
    }
}

impl HotBlocks<'_> {
    pub fn is_hot(&self, fn_key: &str, block_index: usize) -> bool {
        match self {
            HotBlocks::All => true,
            HotBlocks::Measured(f) => f(fn_key, block_index),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CoreBudget {
    pub n: usize,
    pub fetched_text_bytes: u64,
    pub executable_code_bytes: u64,
    pub l1i_bytes: u64,
    pub over_l1i_lines: u64,
    pub over_l2_lines: u64,
    pub over_l3_lines: u64,
    pub text_pages: u64,
    pub itlb_entries: u64,
    pub over_itlb_pages: u64,
    pub tlb_l2_entries: u64,
    pub over_tlb_l2_pages: u64,
    pub data_pages: u64,
    pub over_dtlb_pages: u64,
    pub over_data_tlb_l2_pages: u64,
    pub charge: u64,
}

impl CoreBudget {
    pub fn within_budget(&self) -> bool {
        self.over_l1i_lines == 0
            && self.over_l2_lines == 0
            && self.over_l3_lines == 0
            && self.over_itlb_pages == 0
            && self.over_tlb_l2_pages == 0
            && self.over_dtlb_pages == 0
            && self.over_data_tlb_l2_pages == 0
    }

    pub fn render(&self) -> String {
        self.render_line("Budget", String::new())
    }

    pub fn render_measured(&self, workload: &str) -> String {
        self.render_line("MeasuredBudget", format!("workload={workload} "))
    }

    fn render_line(&self, label: &str, prefix: String) -> String {
        format!(
            "{label} {prefix}n={} fetched_text_bytes={} executable_code_bytes={} \
             l1i_bytes={} over_l1i_lines={} over_l2_lines={} over_l3_lines={} \
             text_pages={} itlb_entries={} over_itlb_pages={} tlb_l2_entries={} \
             over_tlb_l2_pages={} data_pages={} over_dtlb_pages={} \
             over_data_tlb_l2_pages={} charge={}",
            self.n,
            self.fetched_text_bytes,
            self.executable_code_bytes,
            self.l1i_bytes,
            self.over_l1i_lines,
            self.over_l2_lines,
            self.over_l3_lines,
            self.text_pages,
            self.itlb_entries,
            self.over_itlb_pages,
            self.tlb_l2_entries,
            self.over_tlb_l2_pages,
            self.data_pages,
            self.over_dtlb_pages,
            self.over_data_tlb_l2_pages,
            self.charge,
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum DataPage {
    Stack(u64),
    Flow(u64, u64),
    Static(u64, u64),
    Mmio(u64, u64),
}

impl DataPage {
    fn of(m: MemRef) -> Option<DataPage> {
        match m.target {
            crate::cost::MemTarget::Stack { function, offset } => {
                if function == 0 {
                    Some(DataPage::Stack(offset / PAGE_BYTES))
                } else {
                    Some(DataPage::Flow(function, offset / PAGE_BYTES))
                }
            }
            crate::cost::MemTarget::FlowFrame { function, offset } => {
                Some(DataPage::Flow(function, offset / PAGE_BYTES))
            }
            crate::cost::MemTarget::Static { symbol, offset } => {
                Some(DataPage::Static(symbol, offset / PAGE_BYTES))
            }
            crate::cost::MemTarget::Mmio { device, offset } => {
                Some(DataPage::Mmio(device, offset / PAGE_BYTES))
            }
            crate::cost::MemTarget::Unknown { .. } => None,
        }
    }
}

fn geom(table: &CostTable, key: &str) -> u64 {
    table
        .geometry(key)
        .unwrap_or_else(|| {
            panic!("cost table: [geometry.{key}] is required by the footprint model")
        })
        .value
}

fn over_ways(lines: &BTreeSet<u64>, sets: u64, ways: u64) -> u64 {
    let sets = sets.max(1);
    let ways = ways.max(1);
    let mut per_set: BTreeMap<u64, u64> = BTreeMap::new();
    for &l in lines {
        *per_set.entry(l % sets).or_insert(0) += 1;
    }
    per_set.values().map(|&c| c.saturating_sub(ways)).sum()
}

pub fn compute(
    program: &CodegenProgram,
    table: &CostTable,
    point: &SweepPoint,
    placement: &PlacementTable,
    hot: HotBlocks<'_>,
) -> Result<Vec<CoreBudget>, String> {
    let fn_addresses: BTreeMap<String, u64> = program
        .fns
        .iter()
        .scan(0u64, |cursor, (key, f)| {
            let address = *cursor;
            *cursor = cursor.saturating_add((f.code.len() as u64) * 4);
            Some((key.clone(), address))
        })
        .collect();
    compute_at_addresses(program, table, point, placement, hot, &fn_addresses)
}

pub fn compute_linked(
    linked: &LinkedProgram,
    table: &CostTable,
    point: &SweepPoint,
    placement: &PlacementTable,
    hot: HotBlocks<'_>,
) -> Result<Vec<CoreBudget>, String> {
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
                },
            )
        })
        .collect();
    let addresses: BTreeMap<String, u64> = linked
        .fns
        .iter()
        .map(|(key, f)| (key.clone(), f.byte_address))
        .collect();
    let program = CodegenProgram {
        fns,
        rodata: Vec::new(),
        conventions: BTreeMap::new(),
        origin_spans: Vec::new(),
    };
    compute_at_addresses(&program, table, point, placement, hot, &addresses)
}

pub(crate) fn compute_at_addresses(
    program: &CodegenProgram,
    table: &CostTable,
    point: &SweepPoint,
    placement: &PlacementTable,
    hot: HotBlocks<'_>,
    fn_addresses: &BTreeMap<String, u64>,
) -> Result<Vec<CoreBudget>, String> {
    if placement.cores == 0 {
        return Ok(Vec::new());
    }
    let mut owned: Vec<Vec<&String>> = vec![Vec::new(); placement.cores];
    let mut shared: Vec<&String> = Vec::new();
    for key in program.fns.keys() {
        match classify_target(key, placement)? {
            AttrTarget::Core(n) => owned[n].push(key),
            AttrTarget::Shared => shared.push(key),
        }
    }

    let line_bytes = geom(table, "l1i_line_bytes").max(1);
    let l1i_bytes = geom(table, "l1i_bytes");
    let l1i_sets = (l1i_bytes / line_bytes / geom(table, "l1i_ways").max(1)).max(1);
    let l1i_ways = geom(table, "l1i_ways");
    let l2_bytes = geom(table, "l2_bytes");
    let l2_sets = (l2_bytes / line_bytes / geom(table, "l2_ways").max(1)).max(1);
    let l2_ways = geom(table, "l2_ways");
    let l3_bytes = point.get("effective_l3_bytes");
    let l3_ways = geom(table, "l3_ways");
    let l3_sets = (l3_bytes / line_bytes / l3_ways.max(1)).max(1);
    let itlb = geom(table, "itlb_l1_entries");
    let dtlb = geom(table, "dtlb_l1_entries");
    let tlb_l2 = geom(table, "tlb_l2_entries");
    let lat_l1d_hit = geom(table, "lat_l1d_hit");
    let lat_l2 = point.get("l2_latency");
    let lat_l3 = point.get("l3_latency");
    let walk = point.get("tlb_walk_cost");

    let mut out = Vec::with_capacity(placement.cores);
    for n in 0..placement.cores {
        let mut lines: BTreeSet<u64> = BTreeSet::new();
        let mut pages: BTreeSet<u64> = BTreeSet::new();
        let mut data: BTreeSet<DataPage> = BTreeSet::new();
        let mut executable_code_bytes = 0u64;
        for key in owned[n].iter().chain(shared.iter()) {
            let f = &program.fns[*key];
            let fn_address = *fn_addresses
                .get(*key)
                .ok_or_else(|| format!("footprint has no linked address for `{key}`"))?;
            let mut fn_hot_bytes = 0u64;
            for (bi, (start, end)) in basic_block_ranges(&f.code).into_iter().enumerate() {
                if !hot.is_hot(key, bi) {
                    continue;
                }
                let lo = fn_address + (start as u64) * 4;
                let hi = fn_address + (end as u64) * 4;
                if hi <= lo {
                    continue;
                }
                fn_hot_bytes = fn_hot_bytes.saturating_add(hi - lo);
                for l in (lo / line_bytes)..=((hi - 1) / line_bytes) {
                    lines.insert(l);
                }
                for p in (lo / PAGE_BYTES)..=((hi - 1) / PAGE_BYTES) {
                    pages.insert(p);
                }
                for ew in &f.code[start..end] {
                    if let Some(m) = ew.mem {
                        if let Some(p) = DataPage::of(m) {
                            data.insert(p);
                        }
                    }
                }
            }
            executable_code_bytes = executable_code_bytes.saturating_add(fn_hot_bytes);
        }
        // There is no hypothetical per-function packing in the linked
        // stream.  Keep the legacy fields as explicit zeroes for consumers
        // that have not migrated to fetched-text bytes yet.
        let over_l1i_lines = over_ways(&lines, l1i_sets, l1i_ways);
        let over_l2_lines = over_ways(&lines, l2_sets, l2_ways);
        let over_l3_lines = over_ways(&lines, l3_sets, l3_ways);
        let text_pages = pages.len() as u64;
        let data_pages = data.len() as u64;
        let over_itlb_pages = text_pages.saturating_sub(itlb);
        let over_dtlb_pages = data_pages.saturating_sub(dtlb);
        let (over_tlb_l2_pages, over_data_tlb_l2_pages) =
            unified_l2_tlb_overflow(text_pages, data_pages, tlb_l2);
        let charge = over_l1i_lines
            .saturating_mul(lat_l2.saturating_sub(lat_l1d_hit))
            .saturating_add(over_l2_lines.saturating_mul(lat_l3.saturating_sub(lat_l2)))
            .saturating_add(
                over_l3_lines.saturating_mul(point.get("dram_latency").saturating_sub(lat_l3)),
            )
            .saturating_add(
                over_itlb_pages
                    .saturating_add(over_tlb_l2_pages)
                    .saturating_add(over_dtlb_pages)
                    .saturating_add(over_data_tlb_l2_pages)
                    .saturating_mul(walk),
            );
        out.push(CoreBudget {
            n,
            fetched_text_bytes: (lines.len() as u64).saturating_mul(line_bytes),
            executable_code_bytes,
            l1i_bytes,
            over_l1i_lines,
            over_l2_lines,
            over_l3_lines,
            text_pages,
            itlb_entries: itlb,
            over_itlb_pages,
            tlb_l2_entries: tlb_l2,
            over_tlb_l2_pages,
            data_pages,
            over_dtlb_pages,
            over_data_tlb_l2_pages,
            charge,
        });
    }
    Ok(out)
}

fn unified_l2_tlb_overflow(text: u64, data: u64, entries: u64) -> (u64, u64) {
    let total = text.saturating_add(data);
    if total <= entries {
        return (0, 0);
    }
    let text_share =
        u64::try_from((u128::from(entries) * u128::from(text)) / u128::from(total.max(1)))
            .unwrap_or(u64::MAX)
            .min(text);
    let data_share = entries.saturating_sub(text_share).min(data);
    (text - text_share, data - data_share)
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::*;
    use crate::codegen::CodegenFn;
    use crate::cost::rule::{CostRule, EmittedWord, MEM_SP_REG};
    use crate::cost::table::load_default;
    use crate::eval::image::ImageDeclRef;
    use crate::placement::{PlacementEntry, PlacementSource};

    fn table() -> CostTable {
        load_default().expect("bench/a76-pi5.toml")
    }

    fn point(table: &CostTable) -> SweepPoint {
        SweepPoint::pinned(table)
    }

    fn alu() -> EmittedWord {
        EmittedWord::new(0, String::new(), CostRule::Alu, Some(1), &[0, 0])
    }

    fn load(offset: u64) -> EmittedWord {
        EmittedWord::new(0, String::new(), CostRule::Load, Some(1), &[MEM_SP_REG])
            .with_mem(MemRef::stack(offset))
    }

    #[test]
    fn the_measured_budget_line_is_labelled_and_names_its_workload() {
        let b = CoreBudget {
            n: 1,
            fetched_text_bytes: 7744,
            executable_code_bytes: 6420,
            l1i_bytes: 65536,
            over_l1i_lines: 0,
            over_l2_lines: 0,
            over_l3_lines: 0,
            text_pages: 3,
            itlb_entries: 48,
            over_itlb_pages: 0,
            tlb_l2_entries: 1280,
            over_tlb_l2_pages: 0,
            data_pages: 5,
            over_dtlb_pages: 0,
            over_data_tlb_l2_pages: 0,
            charge: 0,
        };
        assert!(
            b.render()
                .starts_with("Budget n=1 fetched_text_bytes=7744 ")
        );
        assert!(
            b.render_measured("boot-actors")
                .starts_with("MeasuredBudget workload=boot-actors n=1 fetched_text_bytes=7744 ")
        );
        assert_eq!(
            b.render().trim_start_matches("Budget "),
            b.render_measured("w")
                .trim_start_matches("MeasuredBudget workload=w "),
            "the two lines must carry identical fields, so only the hotness rule differs"
        );
    }

    fn straight(words: usize) -> CodegenFn {
        CodegenFn {
            frame_size: 0,
            code: (0..words).map(|_| alu()).collect(),
            relocs: Vec::new(),
        }
    }

    fn program(fns: &[(&str, CodegenFn)]) -> CodegenProgram {
        let mut map = BTreeMap::new();
        for (k, f) in fns {
            map.insert((*k).to_string(), f.clone());
        }
        CodegenProgram {
            fns: map,
            rodata: Vec::new(),
            ..Default::default()
        }
    }

    fn entry(id: ImageDeclRef, type_name: &str, core: usize) -> PlacementEntry {
        PlacementEntry {
            id,
            type_name: type_name.to_string(),
            core,
            source: PlacementSource::Explicit,
            work: 0,
            work_source: "unproved",
            bytes: 0,
            bytes_state: 0,
            bytes_mailbox: 0,
            bytes_pool: 0,
        }
    }

    #[test]
    fn two_actors_on_one_core_sum_footprint_and_split_cores_do_not() {
        let t = table();
        let p = point(&t);
        let prog = program(&[("A.turn", straight(64)), ("B.turn", straight(64))]);

        let together = PlacementTable {
            cores: 1,
            entries: vec![
                entry(ImageDeclRef::Actor(0), "A", 0),
                entry(ImageDeclRef::Actor(1), "B", 0),
            ],
        };
        let split = PlacementTable {
            cores: 2,
            entries: vec![
                entry(ImageDeclRef::Actor(0), "A", 0),
                entry(ImageDeclRef::Actor(1), "B", 1),
            ],
        };
        let one = compute(&prog, &t, &p, &together, HotBlocks::All).expect("together");
        let two = compute(&prog, &t, &p, &split, HotBlocks::All).expect("split");
        assert_eq!(one.len(), 1);
        assert_eq!(two.len(), 2);
        assert_eq!(two[0].fetched_text_bytes, 256);
        assert_eq!(two[1].fetched_text_bytes, 256);
        assert_eq!(
            one[0].fetched_text_bytes,
            two[0].fetched_text_bytes + two[1].fetched_text_bytes,
            "one core must hold both actors' text"
        );
        assert!(
            one[0].fetched_text_bytes > two[0].fetched_text_bytes,
            "the split must not sum"
        );
    }

    #[test]
    fn shared_text_counts_on_every_core() {
        let t = table();
        let p = point(&t);
        let prog = program(&[("A.turn", straight(64)), ("__wrela_abort", straight(64))]);
        let place = PlacementTable {
            cores: 2,
            entries: vec![entry(ImageDeclRef::Actor(0), "A", 0)],
        };
        let b = compute(&prog, &t, &p, &place, HotBlocks::All).expect("compute");
        assert_eq!(
            b[0].fetched_text_bytes, 512,
            "core 0 holds A.turn + the helper"
        );
        assert_eq!(b[1].fetched_text_bytes, 256, "core 1 holds only the helper");
    }

    #[test]
    fn hot_text_spanning_forty_nine_pages_costs_more_than_forty_eight() {
        let t = table();
        let p = point(&t);
        let words_per_page = (PAGE_BYTES / 4) as usize;
        let place = PlacementTable {
            cores: 1,
            entries: vec![entry(ImageDeclRef::Actor(0), "A", 0)],
        };
        let at = |pages: usize| -> CoreBudget {
            let prog = program(&[("A.turn", straight(pages * words_per_page))]);
            compute(&prog, &t, &p, &place, HotBlocks::All)
                .expect("compute")
                .remove(0)
        };
        let fit = at(48);
        let over = at(49);
        assert_eq!(fit.text_pages, 48);
        assert_eq!(over.text_pages, 49);
        assert_eq!(fit.over_itlb_pages, 0);
        assert_eq!(over.over_itlb_pages, 1);
        assert!(
            over.charge > fit.charge,
            "49 pages must cost more than 48: {} vs {}",
            over.charge,
            fit.charge
        );
        assert_eq!(fit.over_l1i_lines + 1024, fit.fetched_text_bytes / 64);
        assert_eq!(
            over.charge - fit.charge,
            (over.over_l1i_lines - fit.over_l1i_lines) * (11 - 4) + 58,
            "the extra page is one tlb_walk_cost on top of the L1I overflow"
        );
        assert!(!fit.within_budget(), "196 KiB of text does not fit the L1I");
        let lo = p.with("tlb_walk_cost", 0);
        let prog = program(&[("A.turn", straight(49 * words_per_page))]);
        let cheap = compute(&prog, &t, &lo, &place, HotBlocks::All).expect("compute");
        assert!(cheap[0].charge < over.charge, "tlb_walk_cost must be swept");
    }

    #[test]
    fn l1i_capacity_cliff_charges_the_l2_differential() {
        let t = table();
        let p = point(&t);
        let place = PlacementTable {
            cores: 1,
            entries: vec![entry(ImageDeclRef::Actor(0), "A", 0)],
        };
        let at = |words: usize| -> CoreBudget {
            let prog = program(&[("A.turn", straight(words))]);
            compute(&prog, &t, &p, &place, HotBlocks::All)
                .expect("compute")
                .remove(0)
        };
        let fit = at(16384);
        assert_eq!(fit.fetched_text_bytes, 65536);
        assert_eq!(fit.over_l1i_lines, 0);
        let over = at(16384 + 16);
        assert_eq!(over.fetched_text_bytes, 65536 + 64);
        assert_eq!(over.over_l1i_lines, 1);
        assert_eq!(
            over.charge - fit.charge,
            11 - 4,
            "an overflowing line is charged lat_l2 - lat_l1d_hit"
        );
        assert_eq!(over.over_l2_lines, 0, "64 KiB is far inside the 512 KiB L2");
    }

    #[test]
    fn w_flat_makes_every_block_hot_so_the_flat_row_is_static() {
        use crate::encode::{Cond, enc_b, enc_b_cond};
        let t = table();
        let p = point(&t);
        let code = vec![
            EmittedWord::new(0, String::new(), CostRule::Alu, Some(1), &[0]),
            EmittedWord::new(
                enc_b_cond(Cond::Eq, 12),
                String::new(),
                CostRule::Branch,
                None,
                &[],
            ),
            EmittedWord::new(0, String::new(), CostRule::Alu, Some(2), &[0]),
            EmittedWord::new(enc_b(8), String::new(), CostRule::Branch, None, &[]),
            EmittedWord::new(0, String::new(), CostRule::Alu, Some(3), &[0]),
            EmittedWord::new(0, String::new(), CostRule::Alu, Some(4), &[0]),
        ];
        let blocks = basic_block_ranges(&code);
        assert_eq!(blocks.len(), 4, "the fixture must have several blocks");
        let prog = program(&[(
            "A.turn",
            CodegenFn {
                frame_size: 0,
                code,
                relocs: Vec::new(),
            },
        )]);
        let place = PlacementTable {
            cores: 1,
            entries: vec![entry(ImageDeclRef::Actor(0), "A", 0)],
        };
        let flat = compute(&prog, &t, &p, &place, HotBlocks::All).expect("flat");
        for bi in 0..blocks.len() {
            assert!(HotBlocks::All.is_hot("A.turn", bi));
        }
        let all_hot = |_: &str, _: usize| true;
        let via_pred = compute(&prog, &t, &p, &place, HotBlocks::Measured(&all_hot)).expect("pred");
        assert_eq!(flat, via_pred);
        let only_first = |_: &str, bi: usize| bi == 0;
        let measured =
            compute(&prog, &t, &p, &place, HotBlocks::Measured(&only_first)).expect("measured");
        assert!(
            measured[0].fetched_text_bytes <= flat[0].fetched_text_bytes,
            "a colder f cannot span more text"
        );
    }

    #[test]
    fn two_orderings_of_the_same_blocks_score_differently() {
        use crate::encode::{Cond, enc_b_cond};
        let t = table();
        let p = point(&t);
        let place = PlacementTable {
            cores: 1,
            entries: vec![entry(ImageDeclRef::Actor(0), "A", 0)],
        };
        let body = || -> CodegenFn {
            CodegenFn {
                frame_size: 0,
                code: (0..32)
                    .map(|_| {
                        EmittedWord::new(
                            enc_b_cond(Cond::Eq, 4),
                            String::new(),
                            CostRule::Branch,
                            None,
                            &[],
                        )
                    })
                    .collect(),
                relocs: Vec::new(),
            }
        };
        let prog = program(&[("A.turn", body())]);
        let packed = |_: &str, bi: usize| bi < 16;
        let spread = |_: &str, bi: usize| bi % 2 == 0;
        let a = compute(&prog, &t, &p, &place, HotBlocks::Measured(&packed)).expect("packed");
        let b = compute(&prog, &t, &p, &place, HotBlocks::Measured(&spread)).expect("spread");

        assert_eq!(
            a[0].executable_code_bytes, b[0].executable_code_bytes,
            "same code runs"
        );
        assert_eq!(a[0].fetched_text_bytes, 64, "packed: one line fetched");
        assert_eq!(
            b[0].fetched_text_bytes, 128,
            "spread: two lines for the same code"
        );
        assert_eq!(
            b[0].charge, a[0].charge,
            "actual line membership is reported, not a hypothetical packing surcharge"
        );
        assert_eq!(a[0].over_l1i_lines, 0);
        assert_eq!(b[0].over_l1i_lines, 0);
    }

    #[test]
    fn the_flat_row_uses_actual_line_membership_by_construction() {
        use crate::encode::{Cond, enc_b, enc_b_cond};
        let t = table();
        let p = point(&t);
        let place = PlacementTable {
            cores: 1,
            entries: vec![entry(ImageDeclRef::Actor(0), "A", 0)],
        };
        let diamond = CodegenFn {
            frame_size: 0,
            code: vec![
                EmittedWord::new(0, String::new(), CostRule::Alu, Some(1), &[0]),
                EmittedWord::new(
                    enc_b_cond(Cond::Eq, 12),
                    String::new(),
                    CostRule::Branch,
                    None,
                    &[],
                ),
                EmittedWord::new(0, String::new(), CostRule::Alu, Some(2), &[0]),
                EmittedWord::new(enc_b(8), String::new(), CostRule::Branch, None, &[]),
                EmittedWord::new(0, String::new(), CostRule::Alu, Some(3), &[0]),
            ],
            relocs: Vec::new(),
        };
        let prog = program(&[
            ("A.turn", diamond),
            ("A.other", straight(17)),
            ("__wrela_helper", straight(64)),
        ]);
        let b = compute(&prog, &t, &p, &place, HotBlocks::All).expect("flat");
        assert!(b[0].fetched_text_bytes > 0, "the flat row has fetched text");
    }

    #[test]
    fn the_unified_l2_tlb_is_granted_once_and_not_per_axis() {
        let entries = table()
            .geometry("tlb_l2_entries")
            .expect("[geometry.tlb_l2_entries]")
            .value;
        assert_eq!(entries, 1280);

        let (t, d) = unified_l2_tlb_overflow(700, 700, entries);
        assert_eq!(t + d, 120, "700 + 700 pages over 1280 shared entries");

        assert_eq!(unified_l2_tlb_overflow(640, 640, entries), (0, 0));
        assert_eq!(unified_l2_tlb_overflow(1280, 0, entries), (0, 0));

        assert_eq!(unified_l2_tlb_overflow(1300, 0, entries), (20, 0));
        assert_eq!(unified_l2_tlb_overflow(0, 1300, entries), (0, 20));

        for text in [0u64, 1, 7, 640, 1279, 1280, 4000] {
            for data in [0u64, 1, 7, 640, 1279, 1280, 4000] {
                let (a, b) = unified_l2_tlb_overflow(text, data, entries);
                let want = (text + data).saturating_sub(entries);
                assert!(
                    a + b >= want && a + b <= want + 1,
                    "text={text} data={data}: {a}+{b} is not the unified overflow {want}"
                );
                assert!(a <= text && b <= data, "text={text} data={data}");
            }
        }
    }

    #[test]
    fn a_dead_word_never_lowers_a_footprint_or_a_tlb_charge() {
        let t = table();
        let p = point(&t);
        let place = PlacementTable {
            cores: 1,
            entries: vec![entry(ImageDeclRef::Actor(0), "A", 0)],
        };
        for words in [1usize, 15, 16, 17, 16384] {
            let base = compute(
                &program(&[("A.turn", straight(words))]),
                &t,
                &p,
                &place,
                HotBlocks::All,
            )
            .expect("base")
            .remove(0);
            let grown = compute(
                &program(&[("A.turn", straight(words + 1))]),
                &t,
                &p,
                &place,
                HotBlocks::All,
            )
            .expect("grown")
            .remove(0);
            assert!(
                grown.fetched_text_bytes >= base.fetched_text_bytes,
                "words={words}"
            );
            assert!(grown.text_pages >= base.text_pages, "words={words}");
            assert!(
                grown.over_itlb_pages >= base.over_itlb_pages,
                "words={words}"
            );
            assert!(grown.over_l1i_lines >= base.over_l1i_lines, "words={words}");
            assert!(grown.charge >= base.charge, "words={words}");
        }
    }

    #[test]
    fn data_side_page_span_comes_from_hot_block_memrefs() {
        let t = table();
        let p = point(&t);
        let place = PlacementTable {
            cores: 1,
            entries: vec![entry(ImageDeclRef::Actor(0), "A", 0)],
        };
        let code = vec![
            load(0),
            load(PAGE_BYTES),
            load(2 * PAGE_BYTES),
            EmittedWord::new(0, String::new(), CostRule::Load, Some(1), &[0])
                .with_mem(MemRef::cold_unique(0)),
        ];
        let prog = program(&[(
            "A.turn",
            CodegenFn {
                frame_size: 0,
                code,
                relocs: Vec::new(),
            },
        )]);
        let b = compute(&prog, &t, &p, &place, HotBlocks::All).expect("compute");
        assert_eq!(
            b[0].data_pages, 4,
            "three frame pages plus the named cold line"
        );
        assert_eq!(b[0].over_dtlb_pages, 0);
        assert_eq!(
            DataPage::of(MemRef::cold_unique(7)),
            Some(DataPage::Static(u64::MAX, 0))
        );
    }

    #[test]
    fn no_placement_means_no_budget_lines() {
        let t = table();
        let p = point(&t);
        let place = PlacementTable {
            entries: Vec::new(),
            cores: 0,
        };
        let b = compute(
            &program(&[("f", straight(4))]),
            &t,
            &p,
            &place,
            HotBlocks::All,
        )
        .expect("compute");
        assert!(b.is_empty());
    }
}
