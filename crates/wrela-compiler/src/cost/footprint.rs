//! Per-core instruction-side footprint and TLB pressure (plans/M20.md
//! item F, decision 1603, `compiler.costs.percore-footprint`).
//!
//! **The instruction-side denominator is per core, not per image.** That is
//! the one place this model can be better than a general-purpose
//! compiler's, and it follows from sealed placement: `PlacementTable`
//! ([`crate::placement`]) fixes every actor and driver to a core, and
//! [`super::attr::classify_target`] already turns a scored fn key into
//! `Core(n)` or `Shared`. Two actors on one core **sum** their footprint;
//! the same two split across cores do not.
//!
//! This is the term that lets 04 §5 price code growth instead of refusing
//! it, so it has to be real: it is what stands in the retiring word veto's
//! place (freeze 1626, item J).
//!
//! ## What is hot text
//!
//! A block is hot text when its measured `f > 0`. The measured `f` bridge
//! is item C's, and this module does **not** depend on it: hotness arrives
//! as an injected [`HotBlocks`] parameter that defaults to
//! [`HotBlocks::All`]. Under `W_flat` (`f ≡ 1`) **every** block is hot,
//! which makes the flat row a **static-footprint** row — a degenerate case
//! asserted rather than assumed
//! (`unit:w_flat_makes_every_block_hot_so_the_flat_row_is_static`).
//!
//! ## Contribution
//!
//! A block's contribution is its word range **rounded to 64 B**. Each fn on
//! a core starts at the next 64 B boundary of that core's text region, so a
//! cold block sitting between two hot ones still occupies address space and
//! still inflates the page span — which is what makes the term move when
//! item C attaches a real `f`.
//!
//! ## Charges
//!
//! - **L1I**, 64 KiB 4-way over 64 B lines (256 sets): per-set overflow
//!   beyond 4 ways, charged `lat_l2 − lat_l1d_hit` each. Per-set rather
//!   than capacity-only so a scattered hot set can bind before capacity
//!   does; for contiguous text the two coincide.
//! - **L2**, 512 KiB 8-way (1024 sets): per-set overflow beyond 8 ways,
//!   charged `lat_l3 − lat_l2` each.
//! - **Density**, the order-sensitive term (plans/codegen-pareto-2.md item
//!   K3, decision 1955). Each fn starts at a 64 B boundary, so the fewest
//!   lines *any* intra-fn block ordering can occupy is
//!   `Σ_fn ⌈hot bytes of that fn / 64⌉`. A layout that occupies more than
//!   that floor is performing that many extra instruction-fetch line fills
//!   for bytes that never execute — the same event the L1I overflow term
//!   prices, for a different reason — so each slack line is charged the
//!   same `lat_l2 − lat_l1d_hit`. The floor itself is **not** charged:
//!   every layout pays it, and pricing it would be a second static
//!   footprint term. This is what makes the model see block *order* at all.
//!   Under [`HotBlocks::All`] a fn's hot bytes are all its bytes, so the
//!   floor equals the line count and the charge is identically zero — the
//!   flat row is order-invariant as a theorem rather than as an omission
//!   (`unit:the_flat_row_has_no_density_slack_by_construction`).
//! - **L1 TLB** (48 entries, I-side and D-side alike) and the **L2 TLB**
//!   (1280 entries): pages beyond each are charged one swept
//!   `tlb_walk_cost`. No source prices an L2-TLB *hit*, and the only
//!   magnitude the record brackets at all is the walk
//!   (`[sweep.tlb_walk_cost]`, derived from the T3 curve's tail), so a page
//!   that misses both levels pays two walks. Over-cost, and recorded here
//!   rather than invented as a second magnitude.
//!
//! The D-side gets the **TLB** terms only. Its cache capacity is already
//! charged per access by [`super::mem`], and charging it twice would be a
//! second overlapping footprint term — the exact defect item F is told to
//! avoid.
//!
//! ## Where the charge goes, and where it does not
//!
//! The per-core budget is **reported**, not folded into
//! `total_proxy_cycles`. 04 §5 makes the unified cost `Σ_b f(b)×s(b)` over
//! per-fn schedules and the dump asserts `Composition
//! sum_of_fn_schedules=1`; a whole-program per-core quantity is not a
//! per-fn schedule and adding it would break both. 04 §5 also makes the
//! budget a **hard constraint** ("veto if … any core's hot-text / I-TLB /
//! L2-TLB budget is exceeded"), not an addend — item J installs that veto.
//! `charge` is the priced magnitude the budget line reports so the term is
//! a number rather than a boolean.

use std::collections::{BTreeMap, BTreeSet};

use crate::codegen::CodegenProgram;
use crate::placement::PlacementTable;

use super::attr::{AttrTarget, classify_target};
use super::rule::{MemClass, MemRef};
use super::score::basic_block_ranges;
use super::sweep::SweepPoint;
use super::table::CostTable;

/// Translation granule the page-span terms are computed at.
///
/// 4 KiB is the smallest granule the A76 TLBs support (Core TRM: 4 KiB to
/// 32 MiB I-side, 4 KiB to 512 MiB D-side) and therefore the one that spans
/// the most pages — the over-cost direction (decision 1609). It is also the
/// granule the model's only TLB bracket assumes: `[sweep.tlb_walk_cost]`'s
/// source derives its range from a 64 MiB random read with **4 KiB** pages
/// overflowing the 1280-entry L2 TLB.
pub const PAGE_BYTES: u64 = 4096;

/// Which blocks count as hot text.
///
/// The injection point item C wires measured `f` into. `is_hot` is asked
/// `(fn_key, block_index)`, where `block_index` indexes
/// [`basic_block_ranges`] over that fn's emitted words in layout order —
/// which is exactly what item C's Lane-2 bridge resolves a block id to.
#[derive(Clone, Copy)]
pub enum HotBlocks<'a> {
    /// `W_flat` (`f ≡ 1`): every block is hot. The default, and the reason
    /// the flat row is a static-footprint row.
    All,
    /// Item C's measured `f`: the predicate answers `f(fn_key, block) > 0`.
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

/// One core's text and translation budget — the 04 §6 report line's
/// contents, plus the denominators that make it a *budget* rather than a
/// measurement.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CoreBudget {
    pub n: usize,
    /// Hot text: distinct 64 B lines × the line size.
    pub hot_text_bytes: u64,
    /// Bytes of hot text that are actually **code that runs** — the hot
    /// blocks' own spans, unrounded. `hot_text_bytes − hot_code_bytes` is
    /// fetched-and-never-executed (decision 1955).
    pub hot_code_bytes: u64,
    /// `Σ_fn ⌈that fn's hot bytes / 64⌉` — the fewest lines any intra-fn
    /// block ordering can occupy, since each fn starts 64 B-aligned.
    pub packing_floor_lines: u64,
    /// `lines − packing_floor_lines`: extra line fills this *ordering*
    /// costs. Zero under [`HotBlocks::All`] by construction.
    pub slack_lines: u64,
    /// `[geometry] l1i_bytes` — the denominator.
    pub l1i_bytes: u64,
    /// Σ per-set lines beyond the L1I's 4 ways.
    pub over_l1i_lines: u64,
    /// Σ per-set lines beyond the L2's 8 ways.
    pub over_l2_lines: u64,
    /// Distinct 4 KiB pages the hot text spans.
    pub text_pages: u64,
    /// `[geometry] itlb_l1_entries`.
    pub itlb_entries: u64,
    pub over_itlb_pages: u64,
    /// `[geometry] tlb_l2_entries` — the **unified** L2 TLB, shared by
    /// instruction and data translations. There is one such structure, so
    /// its overflow is computed once over `text_pages + data_pages`; see
    /// [`unified_l2_tlb_overflow`].
    pub tlb_l2_entries: u64,
    /// Text's attributed share of the unified L2 TLB overflow.
    pub over_tlb_l2_pages: u64,
    /// Distinct 4 KiB data pages the hot blocks' `MemRef`s span.
    pub data_pages: u64,
    pub over_dtlb_pages: u64,
    /// Data's attributed share of the unified L2 TLB overflow. This and
    /// `over_tlb_l2_pages` partition **one** overflow — they are not two
    /// independent budgets.
    pub over_data_tlb_l2_pages: u64,
    /// Priced magnitude of every overflow above, in proxy cycles.
    pub charge: u64,
}

impl CoreBudget {
    /// True when this core stays inside every budget — the shape item J's
    /// veto reads (04 §5).
    pub fn within_budget(&self) -> bool {
        self.over_l1i_lines == 0
            && self.over_l2_lines == 0
            && self.over_itlb_pages == 0
            && self.over_tlb_l2_pages == 0
            && self.over_dtlb_pages == 0
            && self.over_data_tlb_l2_pages == 0
    }

    /// The 04 §6 per-core text-and-translation budget line. One line per
    /// core, matching the `Core n=…` style of
    /// [`super::dump::append_core_block`].
    pub fn render(&self) -> String {
        self.render_line("Budget", String::new())
    }

    /// The same fields under a measured `f` (plans/M20.md item C's bridge
    /// wired into [`HotBlocks::Measured`]). Printed **beside** the flat
    /// `Budget` line, never instead of it: the flat line is the
    /// static-footprint row 04 §5's veto is argued against (decision 1617),
    /// and this one says how much of that text the measurement reached.
    pub fn render_measured(&self, workload: &str) -> String {
        self.render_line("MeasuredBudget", format!("workload={workload} "))
    }

    fn render_line(&self, label: &str, prefix: String) -> String {
        format!(
            "{label} {prefix}n={} hot_text_bytes={} hot_code_bytes={} slack_lines={} \
             l1i_bytes={} over_l1i_lines={} over_l2_lines={} \
             text_pages={} itlb_entries={} over_itlb_pages={} tlb_l2_entries={} \
             over_tlb_l2_pages={} data_pages={} over_dtlb_pages={} \
             over_data_tlb_l2_pages={} charge={}",
            self.n,
            self.hot_text_bytes,
            self.hot_code_bytes,
            self.slack_lines,
            self.l1i_bytes,
            self.over_l1i_lines,
            self.over_l2_lines,
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

/// A 4 KiB data page, identified the same way [`super::mem::LineId`] is:
/// per `MemRef` class, with the `Cold` base register a tag component rather
/// than arithmetic on the packed key.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum DataPage {
    Stack(u64),
    Cold(u8, u64),
}

impl DataPage {
    /// `None` for a `Cold` **unique** key: no page identity exists, so its
    /// translation pressure is unmodelled. An under-cost, recorded.
    fn of(m: MemRef) -> Option<DataPage> {
        match m.class {
            MemClass::Stack => Some(DataPage::Stack(m.key / PAGE_BYTES)),
            MemClass::Cold => {
                let base = m.base_reg()?;
                let imm = m.key & 0x0000_FFFF_FFFF_FFFF;
                Some(DataPage::Cold(base, imm / PAGE_BYTES))
            }
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

/// Σ per-set lines beyond `ways`, over `sets` sets.
fn over_ways(lines: &BTreeSet<u64>, sets: u64, ways: u64) -> u64 {
    let sets = sets.max(1);
    let ways = ways.max(1);
    let mut per_set: BTreeMap<u64, u64> = BTreeMap::new();
    for &l in lines {
        *per_set.entry(l % sets).or_insert(0) += 1;
    }
    per_set.values().map(|&c| c.saturating_sub(ways)).sum()
}

/// Per-core text/TLB budgets for `program` under `placement`, at `point`.
///
/// Fns that resolve to `Shared` (runtime helpers, abort paths, free
/// functions) count toward **every** core's budget: a helper any core can
/// call is text every core's L1I must hold. That is both accurate and the
/// over-cost direction. Per-core-owned fns count only for their own core,
/// which is what makes the two-actors-one-core oracle bite.
pub fn compute(
    program: &CodegenProgram,
    table: &CostTable,
    point: &SweepPoint,
    placement: &PlacementTable,
    hot: HotBlocks<'_>,
) -> Result<Vec<CoreBudget>, String> {
    if placement.cores == 0 {
        return Ok(Vec::new());
    }
    // Partition the scored fns by their sealed core, keeping BTreeMap key
    // order so a core's text layout is deterministic.
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
        // This core's text region: its own fns, then the shared text every
        // core must hold. Each fn starts at the next 64 B boundary.
        let mut at = 0u64;
        let mut hot_code_bytes = 0u64;
        let mut packing_floor_lines = 0u64;
        for key in owned[n].iter().chain(shared.iter()) {
            let f = &program.fns[*key];
            let fn_bytes = (f.code.len() as u64).saturating_mul(4);
            let mut fn_hot_bytes = 0u64;
            for (bi, (start, end)) in basic_block_ranges(&f.code).into_iter().enumerate() {
                if !hot.is_hot(key, bi) {
                    continue;
                }
                let lo = at + (start as u64) * 4;
                let hi = at + (end as u64) * 4;
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
            hot_code_bytes = hot_code_bytes.saturating_add(fn_hot_bytes);
            // Each fn is 64 B-aligned, so this is the fewest lines this
            // fn's hot blocks could occupy under *any* ordering of them.
            packing_floor_lines =
                packing_floor_lines.saturating_add(fn_hot_bytes.div_ceil(line_bytes));
            // Saturating like every other arithmetic on this path: a
            // pathological word count must clamp the synthetic address, not
            // panic partway through a scoring run.
            at = at.saturating_add(fn_bytes.div_ceil(line_bytes).saturating_mul(line_bytes));
        }
        let slack_lines = (lines.len() as u64).saturating_sub(packing_floor_lines);

        let over_l1i_lines = over_ways(&lines, l1i_sets, l1i_ways);
        let over_l2_lines = over_ways(&lines, l2_sets, l2_ways);
        let text_pages = pages.len() as u64;
        let data_pages = data.len() as u64;
        // The L1 TLBs are split — 48 instruction entries, 32 data entries —
        // so each is charged against its own axis. The L2 TLB is **one
        // unified structure**, so it is charged once against the combined
        // pressure; granting `tlb_l2` entries to text and another `tlb_l2`
        // to data would let 1280 text pages plus 1280 data pages cost zero
        // on a structure that holds 1280 in total.
        let over_itlb_pages = text_pages.saturating_sub(itlb);
        let over_dtlb_pages = data_pages.saturating_sub(dtlb);
        let (over_tlb_l2_pages, over_data_tlb_l2_pages) =
            unified_l2_tlb_overflow(text_pages, data_pages, tlb_l2);
        let charge = over_l1i_lines
            .saturating_add(slack_lines)
            .saturating_mul(lat_l2.saturating_sub(lat_l1d_hit))
            .saturating_add(over_l2_lines.saturating_mul(lat_l3.saturating_sub(lat_l2)))
            .saturating_add(
                over_itlb_pages
                    .saturating_add(over_tlb_l2_pages)
                    .saturating_add(over_dtlb_pages)
                    .saturating_add(over_data_tlb_l2_pages)
                    .saturating_mul(walk),
            );
        out.push(CoreBudget {
            n,
            hot_text_bytes: (lines.len() as u64).saturating_mul(line_bytes),
            hot_code_bytes,
            packing_floor_lines,
            slack_lines,
            l1i_bytes,
            over_l1i_lines,
            over_l2_lines,
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

/// Overflow of the **unified** L2 TLB, attributed to (text, data).
///
/// A76's L2 TLB is a single 1280-entry structure behind both L1 TLBs, so
/// the quantity that matters is `text + data - entries`, computed once.
/// The two returned numbers are an *attribution* of that one overflow for
/// the dump — they partition it, they are not two budgets. Capacity is
/// split in proportion to demand because the model has no residency order
/// to arbitrate with; the floor's remainder goes to text so the shares sum
/// to `entries` and the overflows sum to the overflow. The clamps can round
/// the total up by at most one page, which is 04 §5's safe direction.
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

    /// The measured line carries the same fields under a distinct label
    /// and names its workload, so a reader of a pinned dump can never
    /// mistake it for the flat `Budget` row (plans/M20.md items C + F).
    #[test]
    fn the_measured_budget_line_is_labelled_and_names_its_workload() {
        let b = CoreBudget {
            n: 1,
            hot_text_bytes: 7744,
            hot_code_bytes: 6420,
            packing_floor_lines: 106,
            slack_lines: 15,
            l1i_bytes: 65536,
            over_l1i_lines: 0,
            over_l2_lines: 0,
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
        assert!(b.render().starts_with("Budget n=1 hot_text_bytes=7744 "));
        assert!(
            b.render_measured("boot-actors")
                .starts_with("MeasuredBudget workload=boot-actors n=1 hot_text_bytes=7744 ")
        );
        assert_eq!(
            b.render().trim_start_matches("Budget "),
            b.render_measured("w")
                .trim_start_matches("MeasuredBudget workload=w "),
            "the two lines must carry identical fields, so only the hotness rule differs"
        );
    }

    /// A straight-line fn of `words` ALU words (one basic block).
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

    /// **Two actors on one core sum footprint; the same two split across
    /// cores do not.** The named oracle for decision 1603 — if this passes
    /// with a per-image denominator it passes by accident, so both the sum
    /// and the split are asserted against the same two fn bodies.
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
        // 64 words = 256 B = 4 lines each.
        assert_eq!(two[0].hot_text_bytes, 256);
        assert_eq!(two[1].hot_text_bytes, 256);
        assert_eq!(
            one[0].hot_text_bytes,
            two[0].hot_text_bytes + two[1].hot_text_bytes,
            "one core must hold both actors' text"
        );
        assert!(
            one[0].hot_text_bytes > two[0].hot_text_bytes,
            "the split must not sum"
        );
    }

    /// Shared text counts toward **every** core: a runtime helper any core
    /// can call is text every core's L1I must hold.
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
        assert_eq!(b[0].hot_text_bytes, 512, "core 0 holds A.turn + the helper");
        assert_eq!(b[1].hot_text_bytes, 256, "core 1 holds only the helper");
    }

    /// **Hot text spanning 49 pages costs more than 48**, which is the
    /// 48-entry L1 I-TLB's cliff. 48 pages is 196608 B of text; the 49th
    /// page overflows the TLB and is charged one swept walk.
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
        // Isolated to the I-TLB axis: 196 KiB of text blows the 64 KiB L1I
        // either way, so the *difference* between the two is the TLB term
        // alone — one page over 48, charged one swept walk.
        assert_eq!(fit.over_l1i_lines + 1024, fit.hot_text_bytes / 64);
        assert_eq!(
            over.charge - fit.charge,
            (over.over_l1i_lines - fit.over_l1i_lines) * (11 - 4) + 58,
            "the extra page is one tlb_walk_cost on top of the L1I overflow"
        );
        assert!(!fit.within_budget(), "196 KiB of text does not fit the L1I");
        // The walk cost is swept, not pinned: moving it to the bracket's low
        // end must move the charge.
        let lo = p.with("tlb_walk_cost", 0);
        let prog = program(&[("A.turn", straight(49 * words_per_page))]);
        let cheap = compute(&prog, &t, &lo, &place, HotBlocks::All).expect("compute");
        assert!(cheap[0].charge < over.charge, "tlb_walk_cost must be swept");
    }

    /// The L1I capacity cliff: 1024 lines fill the 64 KiB 4-way L1I
    /// exactly, and the next line overflows a set.
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
        // 64 KiB of text = 16384 words = 1024 lines.
        let fit = at(16384);
        assert_eq!(fit.hot_text_bytes, 65536);
        assert_eq!(fit.over_l1i_lines, 0);
        // One more line.
        let over = at(16384 + 16);
        assert_eq!(over.hot_text_bytes, 65536 + 64);
        assert_eq!(over.over_l1i_lines, 1);
        assert_eq!(
            over.charge - fit.charge,
            11 - 4,
            "an overflowing line is charged lat_l2 - lat_l1d_hit"
        );
        assert_eq!(over.over_l2_lines, 0, "64 KiB is far inside the 512 KiB L2");
    }

    /// Under `W_flat` (`f ≡ 1`) **every** block is hot, so the flat row is a
    /// static-footprint row: the budget it reports is the whole emitted stream,
    /// independent of any frequency vector. The degenerate case plans/M20.md
    /// item F requires asserted rather than assumed.
    #[test]
    fn w_flat_makes_every_block_hot_so_the_flat_row_is_static() {
        use crate::encode::{Cond, enc_b, enc_b_cond};
        let t = table();
        let p = point(&t);
        // Four blocks: a diamond, so hotness is a real choice.
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
        // Every block hot: `HotBlocks::All` is exactly `f ≡ 1`.
        for bi in 0..blocks.len() {
            assert!(HotBlocks::All.is_hot("A.turn", bi));
        }
        // And an all-hot predicate agrees with `All` exactly, so the default
        // really is the `f ≡ 1` policy and not a separate path.
        let all_hot = |_: &str, _: usize| true;
        let via_pred = compute(&prog, &t, &p, &place, HotBlocks::Measured(&all_hot)).expect("pred");
        assert_eq!(flat, via_pred);
        // A measured `f` that leaves one block cold is strictly smaller —
        // which is what makes the flat row the *static* row.
        let only_first = |_: &str, bi: usize| bi == 0;
        let measured =
            compute(&prog, &t, &p, &place, HotBlocks::Measured(&only_first)).expect("measured");
        assert!(
            measured[0].hot_text_bytes <= flat[0].hot_text_bytes,
            "a colder f cannot span more text"
        );
    }

    /// **K3's regression test: the term sees block order.** Two orderings
    /// of the *same* blocks, with the same blocks hot, must score
    /// differently — which is exactly what was impossible before decision
    /// 1955, and is why round 1's item D could not be ranked.
    ///
    /// The fixture is one fn of eight single-word blocks, four hot. Packed
    /// (hot first) they occupy one 64 B line; interleaved they straddle
    /// two, for the same four words of code that actually run. Same words,
    /// same hotness, different order, different charge.
    #[test]
    fn two_orderings_of_the_same_blocks_score_differently() {
        use crate::encode::{Cond, enc_b_cond};
        let t = table();
        let p = point(&t);
        let place = PlacementTable {
            cores: 1,
            entries: vec![entry(ImageDeclRef::Actor(0), "A", 0)],
        };
        // 32 blocks of one word each: a conditional branch is a block
        // terminator, so every word is its own block.
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
        // 32 words = 128 B = 2 lines. Sixteen hot blocks, 64 B of code.
        let packed = |_: &str, bi: usize| bi < 16; // hot blocks contiguous
        let spread = |_: &str, bi: usize| bi % 2 == 0; // hot blocks alternating
        let a = compute(&prog, &t, &p, &place, HotBlocks::Measured(&packed)).expect("packed");
        let b = compute(&prog, &t, &p, &place, HotBlocks::Measured(&spread)).expect("spread");

        assert_eq!(a[0].hot_code_bytes, b[0].hot_code_bytes, "same code runs");
        assert_eq!(
            a[0].packing_floor_lines, b[0].packing_floor_lines,
            "same floor: the floor is a property of how much runs, not of where"
        );
        assert_eq!(a[0].hot_text_bytes, 64, "packed: one line fetched");
        assert_eq!(
            b[0].hot_text_bytes, 128,
            "spread: two lines for the same code"
        );
        assert_eq!(a[0].slack_lines, 0);
        assert_eq!(b[0].slack_lines, 1);
        assert!(
            b[0].charge > a[0].charge,
            "the spread ordering must cost more: {} vs {}",
            b[0].charge,
            a[0].charge
        );
        assert_eq!(
            b[0].charge - a[0].charge,
            11 - 4,
            "one slack line is charged the same lat_l2 - lat_l1d_hit an overflowing \
             line is: it is the same extra fetch from L2, for a different reason"
        );
        // Neither side is anywhere near an overflow, which is the whole
        // point: the old term charged for overflow only and scored these
        // two identically at zero.
        assert_eq!(a[0].over_l1i_lines, 0);
        assert_eq!(b[0].over_l1i_lines, 0);
    }

    /// The flat row is order-invariant **as a theorem**, not as an
    /// omission: under `HotBlocks::All` a fn's hot bytes are all its bytes,
    /// so the packing floor equals the line count and the density charge is
    /// identically zero. This is why making the term order-sensitive does
    /// not by itself make block layout rankable — the column the forall
    /// gate reads cannot see order, and cannot be made to.
    #[test]
    fn the_flat_row_has_no_density_slack_by_construction() {
        use crate::encode::{Cond, enc_b, enc_b_cond};
        let t = table();
        let p = point(&t);
        let place = PlacementTable {
            cores: 1,
            entries: vec![entry(ImageDeclRef::Actor(0), "A", 0)],
        };
        // A diamond, a straight line, and an odd-length body, on one core.
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
        assert_eq!(b[0].slack_lines, 0, "the flat row has no slack: {:?}", b[0]);
        assert_eq!(
            b[0].hot_text_bytes / 64,
            b[0].packing_floor_lines,
            "under f = 1 the fetched line set *is* the packing floor"
        );
    }

    /// **The L2 TLB is one structure, not two.** Text and data pressure
    /// share its 1280 entries, so 700 text pages plus 700 data pages is
    /// 120 pages of real pressure — not zero on both axes, which is what
    /// charging each against the full 1280 produced.
    #[test]
    fn the_unified_l2_tlb_is_granted_once_and_not_per_axis() {
        let entries = table()
            .geometry("tlb_l2_entries")
            .expect("[geometry.tlb_l2_entries]")
            .value;
        assert_eq!(entries, 1280);

        // The case the split budgets missed entirely.
        let (t, d) = unified_l2_tlb_overflow(700, 700, entries);
        assert_eq!(t + d, 120, "700 + 700 pages over 1280 shared entries");

        // Under the shared capacity nothing is charged on either axis —
        // this is why every committed golden's two fields stay 0.
        assert_eq!(unified_l2_tlb_overflow(640, 640, entries), (0, 0));
        assert_eq!(unified_l2_tlb_overflow(1280, 0, entries), (0, 0));

        // One-sided pressure still lands wholly on the axis that caused it.
        assert_eq!(unified_l2_tlb_overflow(1300, 0, entries), (20, 0));
        assert_eq!(unified_l2_tlb_overflow(0, 1300, entries), (0, 20));

        // The attribution partitions one overflow: the shares sum to it,
        // and rounding is never in the under-cost direction.
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

    /// Monotonicity: a dead word never lowers a footprint or a TLB charge.
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
            assert!(grown.hot_text_bytes >= base.hot_text_bytes, "words={words}");
            assert!(grown.text_pages >= base.text_pages, "words={words}");
            assert!(
                grown.over_itlb_pages >= base.over_itlb_pages,
                "words={words}"
            );
            assert!(grown.over_l1i_lines >= base.over_l1i_lines, "words={words}");
            assert!(grown.charge >= base.charge, "words={words}");
        }
    }

    /// The D side gets the TLB terms, keyed off the hot blocks' `MemRef`s,
    /// and a `Cold` unique key contributes no page (no identity to count).
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
        assert_eq!(b[0].data_pages, 3, "three frame pages; the unique key none");
        assert_eq!(b[0].over_dtlb_pages, 0);
        assert_eq!(DataPage::of(MemRef::cold_unique(7)), None);
    }

    /// No image (`cores == 0`) means no per-core denominator, so there is no
    /// budget line to print rather than a fabricated one.
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
