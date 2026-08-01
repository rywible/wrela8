//! The A76 scoreboard over a `CodegenProgram` (plans/M20.md item E,
//! decisions 1600 / 1606 / 1609, `compiler.costs.a76-model`).
//!
//! **A scoreboard with a real port map, not a cycle simulator** (decision
//! 1606 / freeze 1622). There is no predictor state machine, no cache-line
//! state machine, no coherency protocol, no ROB occupancy model, and none
//! may be grown here. What there is:
//!
//! - **Eight pipelines** (SOG §2.1), each accepting one uop per cycle,
//!   read out of `[pipelines] port_*` rather than hardcoded: 1 = Branch,
//!   2-4 = Integer (2 and 3 single-cycle, 4 single/multi-cycle), 5-6 =
//!   the load/store AGUs, 7-8 = FP/ASIMD, which also carry **store data**.
//!   The port letters map onto them exactly as the profile says: `I` =
//!   pipes 2-4 (basic ALU throughput **3**), `M` = pipe 4 only, `L` =
//!   pipes 5-6, `D` = pipes 7-8, `B` = pipe 1, `V0`/`V1` = pipes 7/8.
//! - **In-order dispatch** (SOG §4.1): `disp[i] >= disp[i-1]`, at most
//!   `dispatch_mops` Mops and `dispatch_uops` uops per cycle, with the
//!   per-class caps applied at the dispatch cycle. Oversubscription
//!   resolves oldest-to-youngest — which in-order dispatch gives for
//!   free, so it is stated here rather than machined.
//! - **Out-of-order issue** inside a **bounded reorder depth**: `issue[i]`
//!   is the earliest cycle at or after `max(disp[i], data_ready[i])` at
//!   which some eligible pipe has a free slot. Nothing forces
//!   `issue[i] >= issue[i-1]`, so a younger independent uop naturally
//!   issues ahead of an older stalled one — that is the whole
//!   out-of-order model and it needs no reordering pass. The 128-entry
//!   window is one line: instruction `i` cannot **dispatch** until
//!   instruction `i - reorder_window` has **retired** (decision 1606 —
//!   a bounded depth, never a simulated ROB).
//! - `retire[i] = issue[i] + latency[i]`, and `s(b)` is the maximum
//!   retire over the block, preserving the pre-M20 `max_retire` meaning.
//! - **Fractional throughput** from `thru_num`/`thru_den`: a rate at or
//!   above 1/cycle is already expressed by the pipe count (3 I pipes *is*
//!   throughput 3), and a rate below 1/cycle **holds the functional
//!   unit** for `ceil(thru_den / thru_num)` cycles. Never rounded —
//!   rounding 1/3 to 1 would be a discount (decision 1609).
//! - **M-pipe blocking**: `m_pipe_stall` extends the M unit's hold
//!   against any other M uop (`mul_high` 3, X-form `mul` 2 — SOG §3.6
//!   notes 4-5); `m_pipe_block` additionally blocks a later uop of the
//!   same blocking kind until this one **completes** (divide, note 1).
//! - **The store address/data split** (SOG §3.10): a store is one Mop
//!   that becomes **two uops** — an address uop on an `L` pipe and a data
//!   uop on a `V` pipe — which is why store-heavy streams contend with
//!   FP/ASIMD. `latency.store`'s `ports = "L,D"` is how the table says
//!   it; the Mop-vs-uop distinction is real in the dispatch accounting
//!   (4 Mops/cycle but 8 uops/cycle).
//! - **NZCV and SP carry their true (RAW) dependence; register 31 as XZR
//!   carries none.** See the correction note below — this is the one place
//!   plans/M20.md's own text was factually wrong about the machine.
//!
//! ## Correction: what register renaming actually removes
//!
//! plans/M20.md item I reads SOG §4.10 note 1 ("NZCV and SP are fully
//! renamed") as licence to delete the scoreboard's flag and SP edges, and
//! item E did so. **That inference does not hold.** Renaming eliminates
//! *false* dependences — WAR (a writer waiting on an earlier reader) and
//! WAW (two writers serializing) — by giving each definition its own
//! physical register. It does nothing to a **RAW** dependence: `cmp`
//! computes NZCV and `b.cond` / `cset` consume it, and no renaming lets a
//! consumer read a value before its producer produced it. The same holds
//! for `sub sp, sp, #N` followed by a load off the new SP.
//!
//! This model never had a WAR or WAW flag stall to remove — a flag write
//! simply overwrote the producer time and a write after a read cost
//! nothing — so §4.10 note 1 licensed **no change here at all**, and the
//! deletion removed a correct edge. Its direction was **under-cost**, the
//! one direction 04 §5 forbids, worth −75 of 3895 proxy-cycles.
//!
//! What item I was right about is narrower and is kept: register number 31
//! is **XZR** in almost every group and **SP** only in add/subtract
//! (immediate), so the pre-M20 model's `ready[31]` edge invented a
//! dependence on every `str xzr` word — a constant with no producer. So
//! the two registers are now tracked apart: `flags_ready` and `sp_ready`
//! carry real RAW edges, `ready[31]` stays permanently zero, and a word
//! reads SP only when its address is a proven `MemClass::Stack` slot or its
//! encoding names register 31 as an add/sub-immediate `Rn`
//! ([`crate::encode::reads_sp`]).
//!
//! Three terms are **provisional seams**, each owned by a following item
//! and each a separate function so those items merge cleanly:
//! [`mem_access_latency`] (item F, now live), [`crosscore_extra`] (item G,
//! now live), [`branch_mispredict_charge`] (item H).
//!
//! Per-fn flat total = Σ s(b) over mechanical basic blocks (`f≡1`).
//!
//! **Known over-cost directions**, recorded because decision 1609 makes
//! the direction part of the model rather than an accident:
//! - `pipe_free[p]` is one scalar per pipe walked in program order, so an
//!   older uop that issues late leaves its pipe unavailable at earlier
//!   cycles to a younger uop processed after it. Over-cost, and the price
//!   of a scoreboard rather than a simulator.
//! - `acc_lat` (the later-arriving accumulator operand of `MADD` /
//!   `SMULH`) is parsed but not used: the tags do not say which source is
//!   the accumulator, so every consumer waits the full latency.
//!   Over-cost.
//! - A divide's pipe-M hold comes from the **pinned** `thru_den/thru_num`
//!   while its dependence latency comes from the **swept**
//!   `divide_x_latency`. The profile does not sweep throughput, so at the
//!   bracket's low end the hold still charges 20. Over-cost.

use std::collections::BTreeMap;

use crate::codegen::{CodegenFn, CodegenProgram};
use crate::placement::PlacementTable;

use super::branch::{BlockCounts, BranchTerms};
use super::footprint::{self, CoreBudget, HotBlocks};
use super::mem::MemState;
use super::owner::classify_owner;
// `MemClass` is read here only by item I's §4.5 alignment term: crossing is
// statically decidable for a proven SP-relative slot and not for a `Cold`
// base whose alignment is not a fact the model has (decision 1615). The
// cache model itself lives in `super::mem` (item F).
use super::rule::{CostRule, EmittedWord, MEM_SP_REG, MemClass};
use super::sweep::SweepPoint;
use super::table::{CostTable, LatRow, pipe_range};

/// Per-fn scoreboard result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FnCost {
    pub key: String,
    /// Owner bucket: `app` / `runtime` / `driver` (item G).
    pub owner: String,
    pub proxy_cycles: u64,
    /// Emitted machine words in this fn (= Σ `terms`). A **reported
    /// column** under 04 §5 as rewritten by plans/M20.md item A; the hard
    /// constraint that replaces the veto is the per-core hot-text budget
    /// (items F / J).
    pub words: u64,
    /// `CostRule::as_str()` → count of words with that rule.
    pub terms: BTreeMap<String, u64>,
}

/// Whole-program proxy-cycle report.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CostReport {
    pub version: u64,
    pub digest: String,
    /// Provenance digest over the tier mix (freeze 1629).
    pub provenance: String,
    /// Human-readable tier mix for the dump's own summary line.
    pub provenance_summary: String,
    /// Copied from `CostTable` for the dump/report header. These are the
    /// facts the model is now actually built on — the pipeline set, the
    /// SOG §4.1 dispatch constraints, and the bounded reorder depth — and
    /// the scoreboard reads the same rows (plans/M20.md item E). The v2
    /// port / reuse-window fields are gone.
    pub profile: String,
    pub pipelines: u64,
    pub dispatch_mops: u64,
    pub dispatch_uops: u64,
    pub reorder_window: u64,
    /// Sum of per-fn schedule lengths (compositionality; dump header states it).
    /// Equals the `flat` workload row (`f≡1`).
    pub total_proxy_cycles: u64,
    /// Σ per-fn `words` — the program's static footprint proxy.
    pub total_words: u64,
    /// Sum of fn `proxy_cycles` per owner bucket (`app` / `runtime` / `driver`).
    pub owner_totals: BTreeMap<String, u64>,
    /// Stable order: `BTreeMap` key order of `program.fns`.
    pub fns: Vec<FnCost>,
    /// `workloads.toml` digest when multi-W compose ran; `None` if not attached.
    pub workloads_digest: Option<String>,
    /// Per-workload proxy totals (`flat` always after compose; measured W when `f` available).
    pub workload_totals: BTreeMap<String, u64>,
    /// Measured-W coverage: name → (matched_hits, total_hits).
    pub workload_coverage: BTreeMap<String, (u64, u64)>,
    /// Per-core text and translation budget, one entry per core in
    /// `0..placement.cores` (empty when there is no image, so no per-core
    /// denominator exists). 04 §6 prints one line per entry; 04 §5 makes it
    /// the hard constraint that replaces the word veto (plans/M20.md item F,
    /// decision 1603). **Not** folded into `total_proxy_cycles` — see
    /// [`super::footprint`] for why.
    pub footprint: Vec<CoreBudget>,
}

// ---------------------------------------------------------------------------
// The port map and the dispatch classes
// ---------------------------------------------------------------------------

/// SOG §4.1's dispatch classes — the granularity its per-cycle caps are
/// stated at. `D` (store *data*) and `V0`/`V1` are all FP/ASIMD pipes, so
/// they share the V cap.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PortClass {
    B,
    /// "S" in §4.1 — the single-cycle integer cap, which the `I` letter
    /// (pipes 2-4) and the `M` letter (pipe 4) both dispatch through; `M`
    /// carries its own tighter cap on top.
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

/// Port letter → the `[pipelines]` row naming its pipes, and the dispatch
/// class its cap is counted against. Nothing here restates a pipe number:
/// the profile's `port_*` rows are the only source.
const PORT_LETTERS: &[(&str, &str, PortClass)] = &[
    ("B", "port_b", PortClass::B),
    ("I", "port_i", PortClass::I),
    ("M", "port_m", PortClass::M),
    ("D", "port_d", PortClass::V),
    ("V0", "port_v0", PortClass::V),
    ("V1", "port_v1", PortClass::V),
    ("L", "port_l", PortClass::L),
];

/// One dispatched uop: the pipes it may issue on, and the class whose
/// dispatch cap and functional-unit hold it counts against.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Uop {
    /// Bit `p` set ⇒ pipeline `p` is eligible.
    pipes: u32,
    class: PortClass,
}

/// The machine's port map and dispatch constraints, read out of
/// `[pipelines]` once per scored fn.
#[derive(Debug, Clone)]
struct Machine {
    /// Number of pipelines (`[pipelines] count`); pipes are 1-based.
    pipes: usize,
    dispatch_mops: u64,
    dispatch_uops: u64,
    /// Per-class dispatch cap, indexed by `PortClass::index`.
    caps: [u64; PORT_CLASS_COUNT],
    /// Bounded reorder depth in instructions (decision 1606).
    window: usize,
    /// Letter → (pipe mask, class), from `PORT_LETTERS`.
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
        // Per-class caps. `cap_l_each` / `cap_v_each` are stated **per
        // pipe** by §4.1 while dispatch has not chosen a pipe yet, so the
        // class cap is the per-pipe cap times that class's pipe count.
        // The L/V port capacity (one uop per pipe per cycle) binds well
        // before either aggregate does; the aggregate is the honest
        // reading of a per-pipe rule at class grain.
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

    /// Expand a `ports` string into the uops one Mop becomes at dispatch.
    ///
    /// One uop per port **letter**, with one documented exception: the
    /// pair `V0,V1` is how the SOG's FP/ASIMD rows list the two V pipes as
    /// *alternatives* for a single uop, whereas `L,D` (store address +
    /// store data, §3.10) and `I,B` (`BL` writing the link register and
    /// redirecting fetch, §3.3) name distinct classes the Mop needs at the
    /// same time. There is no way to tell the two readings apart
    /// syntactically, so the alternative pair is named here.
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

// ---------------------------------------------------------------------------
// Seam payload types (items F / G / H)
// ---------------------------------------------------------------------------

/// What a cross-core term adds to one word.
///
/// `serializes_window` is the structural half of SOG §4.10: an in-order
/// system-register access may not be reordered against anything else in
/// the window. It is a **live** scheduler term (nothing later than a
/// serializing word may issue before it retires) with a `false` default,
/// so item G turns it on by returning `true` rather than by teaching the
/// scheduler something new.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct CrossExtra {
    pub extra_cycles: u64,
    pub serializes_window: bool,
}

/// Measured taken/not-taken for one branch, live at item H.
///
/// Re-exported here because this is the path the rest of the tree already
/// names it by; the type, its private fields, and the reason `None` is not a
/// measured 0.5 all live with the model in [`super::branch`].
pub use super::branch::BranchBias;

// ---------------------------------------------------------------------------
// The four seams
// ---------------------------------------------------------------------------

/// **Item F** owns this. Reuse distance at 64 B line grain over real
/// capacities and associativity (4-way L1D ⇒ a conflict verdict, not only
/// a capacity verdict), L2 strict inclusivity of L1D, and the store buffer
/// with store-to-load forwarding.
///
/// The model itself lives in [`super::mem`] — this is the seam that hands
/// one emitted word to it. `lat_l1d_hit` is T1 and unbracketed so it is
/// read pinned; every bracketed leaf (`l2_latency` / `l3_latency` /
/// `dram_latency`), the swept effective L3 **size**, and the T5
/// `store_to_load_forwarding` latency all come through `point`, so item J's
/// ∀ sweep genuinely moves the memory model. Returns the whole memory
/// latency of this word and advances `state` in **program** order.
fn mem_access_latency(ew: &EmittedWord, state: &mut MemState) -> u64 {
    state.access(ew).latency
}

/// **Item G's seam.** Placement-derived local-vs-remote snoop
/// classification (decision 1603: `PlacementTable::core_of` seals every
/// actor and driver to a core, so whether a load reads a line another core
/// wrote is a *static* fact), the `DMB` charge, the system-register flush
/// charge, and the swept `LoadAcquire` / `StoreRelease` increments — every
/// one of them swept and none of them pinnable (decision 1602).
///
/// Live: the model is [`crate::cost::crosscore`] — barrier ordering plus
/// its swept `dmb_cost`, the placement-classified snoop above the memory
/// path, §4.10's in-order/flush system-register halves, the swept ordered
/// accesses, and row 20's "charge the branch, never a handler body". This
/// body is the seam; the model lives in one file so item G's reasoning and
/// its oracles sit together.
fn crosscore_extra(
    fn_key: &str,
    ew: &EmittedWord,
    table: &CostTable,
    point: &SweepPoint,
    placement: &PlacementTable,
) -> CrossExtra {
    super::crosscore::charge(fn_key, ew, table, point, placement)
}

/// **Item H's seam.** The mispredict charge, scaled by measured per-block
/// branch bias: a well-predicted branch costs ~0 whatever `[branch]
/// mispredict_penalty` is, and a near-50/50 one costs the full swept
/// penalty. Zero under `W_flat` and zero for an unmeasured branch, where
/// there is no bias information — no data, no charge (decision 1609).
///
/// Live: the model is [`crate::cost::branch`], which also owns SOG §4.8's
/// front-end terms and records which of them a per-fn word stream can
/// decide. `bias` arrives as `Option`, and `None` returns before the
/// penalty is read at all — a measured 0.5 (the **full** penalty) and no
/// data (zero) are as far apart as this term can put them, which is the
/// point. This body is the seam; the reasoning and its oracles sit together
/// in one file.
///
/// The penalty magnitude is read through `point` (`[sweep]
/// mispredict_penalty`, 11-14 pinned at 14), never pinned here: it is T4.
fn branch_mispredict_charge(
    table: &CostTable,
    point: &SweepPoint,
    bias: Option<BranchBias>,
) -> u64 {
    let _ = table;
    super::branch::branch_mispredict_charge(point.get("mispredict_penalty"), bias)
}

/// **Item I** owns this. SOG §4.5's two boundaries: a load crossing a
/// 64 B cache line (`[align] load_line_bytes`), a store crossing a 16 B
/// boundary (`[align] store_boundary_bytes`). The **mechanism** is T1 and
/// pinned in `[align]`; the **magnitudes** are unpriced by §4.5 and so are
/// read through `point` from `[sweep] load_line_cross_penalty` /
/// `store_boundary_cross_penalty` — never pinned here (item D tiered them
/// T5).
///
/// **What is decidable, and what is not** (plans/M20.md item I, decision
/// 1611). Two facts make a crossing verdict possible:
///
/// 1. the **access width**, read off the emitted word at construction
///    (`EmittedWord::access_bytes`), and
/// 2. the **base register's alignment**, which the model has only for
///    `MemClass::Stack`: the base is `sp`, and this ABI keeps it congruent
///    to 0 modulo [`codegen::FRAME_SP_ALIGN_BYTES`]. A `Cold` base holds a
///    runtime-computed address whose alignment is not a fact this model
///    has at all.
///
/// So a Stack access is decided exactly, over every `sp` the ABI permits
/// (`crosses_boundary` quantifies over the residues rather than assuming
/// `sp ≡ 0 (mod 64)` — an unknown `sp` is a residual uncertainty and
/// decision 1609 rounds it toward over-costing). An undeclared width or a
/// `Cold` base is **not decided**, and charges nothing; decision 1611
/// records why, and that it is the one place in this model where an
/// unmodelled incidence is not rounded up.
///
/// On the emitted stream this term is therefore **live but unexercised**:
/// `encode.rs`'s `scaled_offset` asserts every immediate offset is a
/// multiple of its own transfer width, `LDAR`/`STLR` have no offset at
/// all, and every width is ≤ [`codegen::FRAME_SLOT_BYTES`], so every Stack
/// access is naturally aligned and neither boundary can be straddled. That
/// is a result about the spill-everything frame, not an accident, and
/// `stack_accesses_never_straddle_either_boundary` measures it over the
/// whole `cost-*` corpus rather than asserting it.
fn alignment_penalty(
    ew: &EmittedWord,
    table: &CostTable,
    point: &SweepPoint,
) -> Result<u64, String> {
    let is_load = ew.rule.is_load();
    if !is_load && !ew.rule.is_store() {
        return Ok(0);
    }
    // Not decided: no width fact, or a base whose alignment is unknown.
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
    // Fail closed: a v3 table always carries both `[align]` rows, so a
    // missing one means the profile and this term disagree about what
    // §4.5 says — never a silent 0, which would be a discount.
    let boundary = table
        .align(row_key)
        .ok_or_else(|| format!("cost table: [align.{row_key}] is required by SOG §4.5's terms"))?
        .value;
    if crosses_boundary(m.key, width, boundary, crate::codegen::FRAME_SP_ALIGN_BYTES) {
        return Ok(point.get(dim));
    }
    Ok(0)
}

/// Does a `width`-byte access at frame offset `off` straddle an aligned
/// `boundary`-byte block, for **any** `sp` this ABI permits?
///
/// `sp` itself is unknown; all that is known is `sp ≡ 0 (mod sp_align)`.
/// So the absolute address is `k + off` for some `k` in
/// `0, sp_align, 2·sp_align, …` below `boundary`, and the term charges if
/// *any* of those positions crosses — the over-cost reading of an unknown
/// base (decision 1609). When `boundary ≤ sp_align` the residue set is
/// `{0}` and the verdict is exact.
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

// ---------------------------------------------------------------------------
// Program / fn / block entry points
// ---------------------------------------------------------------------------

/// Score every function at the **pinned** point of the residual box
/// (freeze 1625: pinned dumps stay single-valued at `a76-pi5`).
pub fn score_program(
    program: &CodegenProgram,
    table: &CostTable,
    placement: &PlacementTable,
) -> Result<CostReport, String> {
    score_program_at(program, table, placement, &SweepPoint::pinned(table))
}

/// Score every function at an explicit point of the residual box — item
/// J's ∀ sweep entry. There is deliberately no per-point *win* predicate
/// here (freeze 1624); this only scores.
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

/// Score at `point` with explicit measured-`f` inputs — the two places
/// block-grain frequency enters the model, and the only ones.
///
/// - `hot`: the per-core footprint term's hot-text predicate (item F,
///   decision 1603). `HotBlocks::All` is `W_flat` (`f ≡ 1`), which makes the
///   flat row a **static-footprint** row.
/// - `counts`: per-block frequency for the branch model (item H).
///   `BlockCounts::Flat` carries no counts at all, so the flat row charges
///   **zero** mispredict — `f ≡ 1` is not bias information, and a naive
///   ratio over it would be 0.5, i.e. the full penalty everywhere, which is
///   backwards (see [`super::branch`]).
///
/// Both arrive as injected parameters rather than as a dependency on item
/// C's bridge, so the branch and footprint terms are testable and mergeable
/// without it. Item C wires the measured sides here, from the same
/// `<fn_key>#<block_index>` sidecar: `HotBlocks::Measured(&|k, b| f(k, b) >
/// 0)` and `BlockCounts::Measured(&|k, b| …)` returning the block's count
/// together with the identity of the Lane 2 span it was measured on.
/// Everything else about scoring is unchanged, so wiring measured `f` in
/// reshapes nothing.
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
        total_words: totals.total_words,
        owner_totals: totals.owner_totals,
        fns: totals.fns,
        workloads_digest: None,
        workload_totals: BTreeMap::new(),
        workload_coverage: BTreeMap::new(),
        footprint: totals.footprint,
    })
}

// ---------------------------------------------------------------------------
// The lean scoring path (plans/codegen-pareto-2.md item R, decisions
// 1960–1963)
// ---------------------------------------------------------------------------

/// Table-derived scoring constants, built **once** and reused at every
/// point of a sweep (decision 1960).
///
/// [`score_program_at`] is called once per corner of the residual box —
/// 21 cases × up to 12 288 corners × 2 sides — and every one of those
/// calls used to rebuild [`Machine`] out of the profile's `[pipelines]`
/// rows: five string pipe-range parses and a `Vec` of port letters, none
/// of which any sweep dimension can move. Nothing here reads a
/// [`SweepPoint`], which is the whole reason it is safe to hoist: a
/// `ScoreCtx` built from a table scores every point of that table's box.
///
/// This is the same category of fix as `super::branch`'s hoisted
/// `penalty()` — a per-call reconstruction of a constant, in a loop whose
/// only job is to vary something else.
pub struct ScoreCtx {
    machine: Machine,
}

impl ScoreCtx {
    pub fn new(table: &CostTable) -> Result<ScoreCtx, String> {
        Ok(ScoreCtx {
            machine: Machine::from_table(table)?,
        })
    }
}

/// Everything a ∀-sweep corner reads out of a scoring, and nothing else
/// (decision 1961).
///
/// [`CostReport`] carries three table digests — each of which formats
/// every row of the profile into a `String` and hashes it — a per-fn
/// [`FnCost`] with a cloned key, an owner `String` and a term map, and the
/// profile header fields. The sweep keeps cycles, words, budgets and the
/// ordering counts, and throws the rest away at every corner. This is the
/// part it keeps.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScoreTotals {
    pub total_proxy_cycles: u64,
    pub total_words: u64,
    pub footprint: Vec<CoreBudget>,
    /// Per-`(fn, ordering rule)` emitted-word counts, present only when
    /// the caller asked for them.
    ///
    /// **Ordering counts are counts of emitted words, so they are
    /// identical at every point of the box** — the same fact
    /// `opts::win::refuse_at_point`'s `check_ordering` flag already rests
    /// on. Computing them requires the per-rule term map, which is one
    /// `String` allocation per emitted word (~22 000 of them on the
    /// flagship image), so a corner that will not read them does not build
    /// them. `None` means "not computed", never "no ordering words": a
    /// caller that asked for them and got `None` is a bug, and
    /// `refuse_at_point` fails closed on it rather than reading an empty
    /// map as an absence of barriers.
    pub ordering: Option<super::crosscore::OrderingCounts>,
}

/// Score at `point` for the totals a sweep reads, skipping the report
/// (decision 1961).
///
/// Identical arithmetic to [`score_program_at`] — the same
/// [`score_program_core`] over the same words in the same order — with
/// the report assembly and, when `want_ordering` is false, the per-rule
/// term maps left unbuilt. `ctx` is the hoisted table constant; passing
/// one built from a *different* table than `table` would be a caller bug
/// the type system does not catch, so every caller builds it from the
/// table it then scores against.
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
        total_words: core.total_words,
        footprint: core.footprint,
        ordering: want_ordering.then(|| super::crosscore::ordering_word_counts_of(&core.fns)),
    })
}

/// What [`score_program_core`] produces: the report's body, before the
/// header.
struct ProgramCore {
    total_proxy_cycles: u64,
    total_words: u64,
    owner_totals: BTreeMap<String, u64>,
    /// Empty when `want_fns` was false.
    fns: Vec<FnCost>,
    footprint: Vec<CoreBudget>,
}

/// The one scoring loop. Both entry points above go through it, so the
/// lean path cannot drift from the reported one: `want_fns` decides
/// whether the per-fn rows and their term maps are *materialized*, never
/// what is *computed*.
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
    let mut fns = Vec::with_capacity(if want_fns { program.fns.len() } else { 0 });
    let mut total_proxy_cycles = 0u64;
    let mut owner_totals = BTreeMap::from([
        ("app".to_string(), 0u64),
        ("runtime".to_string(), 0u64),
        ("driver".to_string(), 0u64),
    ]);

    let mut total_words = 0u64;
    for (key, f) in &program.fns {
        let (proxy_cycles, terms) = score_fn(
            key,
            f,
            table,
            placement,
            point,
            &ctx.machine,
            &counts,
            want_fns,
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
                proxy_cycles,
                words,
                terms,
            });
        }
    }

    Ok(ProgramCore {
        total_proxy_cycles,
        total_words,
        owner_totals,
        fns,
        footprint: footprint::compute(program, table, point, placement, hot)?,
    })
}

/// Half-open word-index ranges `[start, end)` of mechanical basic blocks
/// in a codegen stream (integrity item L). Leaders: index 0, every
/// PC-relative branch target, and the fallthrough word after any
/// `CostRule::Branch`.
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

/// Scoreboard schedule length `s(b)` for each basic block, in layout
/// order — the block-grain compose input (item C).
///
/// Flat: no measured per-block frequency, so no mispredict is charged here
/// (item H's "no data, no charge"). Item C's `Σ f(b)×s(b)` becomes
/// bias-aware by calling [`block_schedule_lengths_with_counts`] with the
/// same vector it is weighting by, so the `s(b)` it multiplies and the fn
/// total it is checked against are scored at the same point.
pub fn block_schedule_lengths(
    fn_key: &str,
    code: &[EmittedWord],
    table: &CostTable,
    placement: &PlacementTable,
) -> Result<Vec<u64>, String> {
    block_schedule_lengths_with_counts(fn_key, code, table, placement, &BlockCounts::Flat)
}

/// [`block_schedule_lengths`] with an explicit per-block frequency source.
pub fn block_schedule_lengths_with_counts(
    fn_key: &str,
    code: &[EmittedWord],
    table: &CostTable,
    placement: &PlacementTable,
    counts: &BlockCounts<'_>,
) -> Result<Vec<u64>, String> {
    let machine = Machine::from_table(table)?;
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
            &machine,
            &branch_terms,
            true,
        )?;
        out.push(s);
    }
    Ok(out)
}

fn score_fn(
    key: &str,
    f: &CodegenFn,
    table: &CostTable,
    placement: &PlacementTable,
    point: &SweepPoint,
    machine: &Machine,
    counts: &BlockCounts<'_>,
    want_terms: bool,
) -> Result<(u64, BTreeMap<String, u64>), String> {
    let mut terms: BTreeMap<String, u64> = BTreeMap::new();
    if f.code.is_empty() {
        return Ok((0, terms));
    }
    // Both branch halves are properties of the whole word stream — §4.8's
    // fetch regions span block boundaries and a branch's successors are
    // other blocks — so they are computed once per fn, before scheduling.
    let branch_terms = BranchTerms::compute(key, &f.code, table, point, counts)?;
    let mut proxy_cycles = 0u64;
    for (start, end) in basic_block_ranges(&f.code) {
        let (s, block_terms) = score_words(
            key,
            &f.code[start..end],
            start,
            table,
            placement,
            point,
            machine,
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

// ---------------------------------------------------------------------------
// The scoreboard
// ---------------------------------------------------------------------------

/// Dispatch / issue / retire over a straight-line word slice.
/// Returns `(schedule_length, term_counts)`.
///
/// `word_base` is this slice's first word index **inside its fn**, which is
/// how the per-fn branch terms (bias and SOG §4.8) are keyed: both are
/// whole-stream properties, so they are computed once and looked up here.
#[allow(clippy::too_many_arguments)]
fn score_words(
    fn_key: &str,
    code: &[EmittedWord],
    word_base: usize,
    table: &CostTable,
    placement: &PlacementTable,
    point: &SweepPoint,
    machine: &Machine,
    branch_terms: &BranchTerms,
    // `want_terms`: build the per-rule word-count map. It is one `String`
    // allocation per emitted word and the only thing that reads it is the
    // ordering census, which is point-invariant — so the ∀ sweep asks for
    // it once per case rather than once per corner (item R, decision
    // 1961). It is an output, never an input: no branch of the scoreboard
    // below reads `terms`.
    want_terms: bool,
) -> Result<(u64, BTreeMap<String, u64>), String> {
    let mut terms: BTreeMap<String, u64> = BTreeMap::new();
    if code.is_empty() {
        return Ok((0, terms));
    }

    let mut mem = MemState::new(table, point);
    // Per-pipe next-free cycle. Index 0 unused (pipes are 1-based).
    let mut pipe_free = vec![0u64; machine.pipes + 1];
    // Functional-unit hold per dispatch class — only ever non-zero for a
    // class whose group holds its unit for more than one cycle (pipe M).
    let mut unit_free = [0u64; PORT_CLASS_COUNT];
    // A `m_pipe_block` group (divide) blocks the next such group until it
    // **completes** (SOG §3.6 note 1).
    let mut block_free = 0u64;
    let mut ready = [0u64; 32];
    // NZCV's producer time. A **RAW** edge: renaming removes WAR/WAW, not
    // the fact that a flag consumer cannot read a value its producer has
    // not computed. Kept out of `ready` because register number 31 already
    // means two things and NZCV is not a GPR at all.
    let mut flags_ready = 0u64;
    // SP's producer time, tracked separately from `ready[31]` for exactly
    // that reason: 31 is SP in the add/sub-immediate group and XZR nearly
    // everywhere else, and only one of the two has a producer.
    let mut sp_ready = 0u64;
    let mut control_ready = 0u64;
    // SOG §4.10: an in-order system access is not reordered against the
    // rest of the window (item G sets `serializes_window` to turn this on).
    let mut serial_until = 0u64;

    // In-order dispatch state.
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

        let row = timing_row(ew.rule, table);
        let uops = machine.uops_for(ports_for(ew.rule, table)?)?;
        // Throughput below one per cycle holds the functional unit; at or
        // above one per cycle the pipe count already expresses it.
        let hold = row
            .map(|r| r.thru_den.div_ceil(r.thru_num).max(1))
            .unwrap_or(1);
        let m_stall = row.map(|r| r.m_pipe_stall).unwrap_or(0);
        let blocks = row.map(|r| r.m_pipe_block).unwrap_or(false);
        let occupancy = hold.saturating_add(m_stall);
        // Execution latency, read through the point wherever the row is
        // bracketed (today: the divide's 5-20).
        let exec_lat = rule_latency(ew.rule, table, point);
        // Computed before issue because its `serializes_window` half is an
        // *ordering* constraint, not a latency: item G turns it on by
        // returning `true` here and needs no scheduler change.
        let cross = crosscore_extra(fn_key, ew, table, point, placement);

        // --- dispatch: in-order, capped, and bounded by the reorder depth
        let mut min_disp = disp_cycle;
        if i >= machine.window {
            min_disp = min_disp.max(retire[i - machine.window]);
        }
        if min_disp > disp_cycle {
            disp_cycle = min_disp;
            disp_mops = 0;
            disp_uops = 0;
            disp_class = [0u64; PORT_CLASS_COUNT];
        }
        let mut want_class = [0u64; PORT_CLASS_COUNT];
        for u in &uops {
            want_class[u.class.index()] += 1;
        }
        // Fail closed rather than spin: a Mop that can never fit means the
        // profile's caps and the port strings disagree.
        if uops.len() as u64 > machine.dispatch_uops {
            return Err(format!(
                "cost model: `{}` expands to {} uops, above [pipelines] dispatch_uops {}",
                ew.rule.as_str(),
                uops.len(),
                machine.dispatch_uops
            ));
        }
        for (c, want) in want_class.iter().enumerate() {
            if *want > machine.caps[c] {
                return Err(format!(
                    "cost model: `{}` wants {want} uops of one dispatch class, above its \
                     [pipelines] cap {}",
                    ew.rule.as_str(),
                    machine.caps[c]
                ));
            }
        }
        loop {
            let fits = disp_mops < machine.dispatch_mops
                && disp_uops + uops.len() as u64 <= machine.dispatch_uops
                && want_class
                    .iter()
                    .enumerate()
                    .all(|(c, want)| disp_class[c] + want <= machine.caps[c]);
            if fits {
                break;
            }
            // Oversubscription resolves oldest-to-youngest (SOG §4.1);
            // in-order dispatch gives that for free, so the younger Mop
            // simply slips to the next cycle.
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

        // --- issue: out of order, earliest eligible slot
        let mut base_ready = dispatch
            .max(src_ready(ew, &ready))
            .max(control_ready)
            .max(serial_until);
        if ew.flags.reads() {
            base_ready = base_ready.max(flags_ready);
        }
        // A word reads SP either because its address is a **proven**
        // SP-relative slot (`MemClass::Stack`) or because it is an
        // add/sub-immediate naming register 31 as `Rn` (`sub sp, sp, #N`,
        // `add x0, sp, #0`). A word that merely carries 31 in `srcs`
        // without either of those is reading **XZR** and waits on nothing —
        // that false edge, on every `str xzr` word, is the one thing item I
        // was right to delete.
        let reads_sp = matches!(ew.mem.map(|m| m.class), Some(MemClass::Stack))
            || crate::encode::reads_sp(ew.word);
        if reads_sp {
            base_ready = base_ready.max(sp_ready);
        }
        if blocks {
            base_ready = base_ready.max(block_free);
        }
        if cross.serializes_window {
            // SOG §4.10's in-order registers are not reordered in *either*
            // direction: nothing already in the window may still be in
            // flight when this word issues.
            base_ready = base_ready.max(max_retire);
        }
        let mut issue = base_ready;
        for u in &uops {
            // **Waiting on the hold is unconditional; only setting one is
            // not.** A functional unit an earlier uop is holding is busy
            // for every uop of that class, not just for the ones that
            // would take a hold themselves. Gating the wait on this uop's
            // own `occupancy > 1` let an occupancy-1 uop issue straight
            // through a live hold — inert today, because every M-pipe row
            // in the profile has occupancy above 1, and live the moment one
            // does not (a W-form multiply-accumulate is the obvious next
            // one). `unit_free` is only ever written under `occupancy > 1`
            // below, so a class nothing holds stays at 0 and this max is a
            // no-op there.
            let earliest = base_ready.max(unit_free[u.class.index()]);
            let (pipe, at) = earliest_pipe(u.pipes, earliest, &pipe_free)?;
            // Each pipeline accepts one uop per cycle (SOG §2.1).
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

        // --- latency: table + the four seams
        let mut lat = match ew.rule {
            // Item H: the bias-derived mispredict charge, plus the static
            // SOG §4.8 front-end incidences attributed to this branch word.
            // Both land on a branch, so both flow into this block's `s(b)`
            // and therefore into `Σ f(b)×s(b)` without a second addend.
            CostRule::Branch => exec_lat
                .saturating_add(branch_mispredict_charge(
                    table,
                    point,
                    branch_terms.bias_at(word_base + i),
                ))
                .saturating_add(branch_terms.frontend_at(word_base + i)),
            r if r.is_load() || r.is_store() => mem_access_latency(ew, &mut mem),
            _ => exec_lat,
        };
        lat = lat.saturating_add(cross.extra_cycles);
        lat = lat.saturating_add(alignment_penalty(ew, table, point)?);

        let finish = issue.saturating_add(lat);
        retire[i] = finish;
        if let Some(d) = ew.dst {
            let d = d as usize;
            // Register number 31 is skipped here on purpose, and the reason
            // is **not** renaming — it is that 31 is two different
            // registers. As an ordinary destination it is **XZR**, a
            // constant with no producer, so a write to it is discarded and
            // recording a ready time would invent a dependence the machine
            // cannot have. As an add/sub-immediate destination it is **SP**,
            // whose true dependence is tracked by `sp_ready` below.
            if d < 32 && d != MEM_SP_REG as usize {
                ready[d] = finish;
            }
        }
        // **SP's true dependence, restored** (see the module doc's
        // correction note). A tagged `dst == 31` is a genuine SP write —
        // stores carry `dst: None`, and codegen never names XZR as a
        // destination, which is also why this same tag drives the
        // frame-epoch invalidation below.
        if ew.dst == Some(MEM_SP_REG) {
            sp_ready = finish;
        }
        // **The NZCV data edge, restored.** Renaming removes WAR and WAW
        // hazards; it does not remove a RAW one. `cmp` produces NZCV and
        // `b.cond` / `cset` consume it, and no amount of renaming lets a
        // consumer read a value before its producer computed it. This model
        // has never had a WAR or WAW flag stall to remove — a write simply
        // overwrites `flags_ready` and a write after a read costs nothing —
        // so SOG §4.10 note 1 licenses no change here at all.
        if ew.flags.writes() {
            flags_ready = finish;
        }
        if ew.rule == CostRule::Branch {
            control_ready = finish;
        }
        if cross.serializes_window {
            serial_until = serial_until.max(finish);
        }
        max_retire = max_retire.max(finish);

        // The other half of an SP write, and the half that stays: a new
        // frame epoch ends memory reuse (stack offsets after an SP adjust
        // name different bytes). Independent of the rename above.
        if ew.rule == CostRule::Call || ew.dst == Some(MEM_SP_REG) {
            mem.clear();
        }
    }

    Ok((max_retire, terms))
}

/// Earliest cycle at or after `from` at which one of `pipes` is free, and
/// which pipe that is (lowest index breaks ties). This is the whole
/// out-of-order rule: no ordering against the previous uop is imposed.
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

/// The `[latency]` row whose ports / throughput / M-pipe terms govern
/// `rule`. The ordered accesses (`LDAR` / `STLR`) are `[crosscore]`
/// increments **on** `[latency.load]` / `[latency.store]`, so they take
/// their plain twin's row; `barrier` / `system` have no row at all.
fn timing_row(rule: CostRule, table: &CostTable) -> Option<&LatRow> {
    let key = if rule.is_load() {
        "load"
    } else if rule.is_store() {
        "store"
    } else {
        rule.as_str()
    };
    table.latency_row(key)
}

/// Port-letter string for `rule`, straight off the governing `[latency]`
/// row. An unknown letter fails closed in `Machine::uops_for`, so there is
/// no second list of legal port strings here to drift from the profile.
fn ports_for(rule: CostRule, table: &CostTable) -> Result<&str, String> {
    // `barrier` and `system` are T5 **by absence** — the SOG has no
    // barrier and no system-register instruction entry anywhere, so there
    // is no row to read a port letter from. `DMB` is a "special memory"
    // op, which SOG §2.1 puts on pipes 5-6 (the L letter); `BRK` has no
    // documented pipe at all, so it is charged against the integer pipes:
    // the over-cost direction for a word that certainly occupies *some*
    // slot. Item G owns their magnitudes.
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

/// Execution latency for `rule` at `point`: the row's `lat`, read through
/// the sweep dimension it names when it is bracketed (today only the
/// divide's 5-20), plus any swept residual the row declares
/// (`call_overhead`). Equals `CostTable::latency` at the pinned point.
fn rule_latency(rule: CostRule, table: &CostTable, point: &SweepPoint) -> u64 {
    match table.latency_row(rule.as_str()) {
        Some(row) => {
            let mut lat = match &row.sweep {
                Some(dim) => point.get(dim),
                None => row.lat,
            };
            if let Some(dim) = &row.sweep_add {
                lat = lat.saturating_add(point.get(dim));
            }
            lat
        }
        // `barrier` / `system` / the ordered accesses: priced by
        // `[crosscore]`, which `CostTable` has already folded in.
        None => table.latency(rule),
    }
}

/// PC-relative branch target as a word index, or `None` for register
/// branches / undecodable encodings.
/// Word index a PC-relative branch targets, or `None` for a word this
/// module cannot decode as one (`BR`, `RET`, a synthetic zero word).
/// `pub(super)` so item H's branch model reads **this** CFG rather than
/// building a second one.
pub(super) fn branch_target_index(word: u32, from: usize) -> Option<usize> {
    let word_delta = if word & 0xFC00_0000 == 0x1400_0000 {
        // B: op=0, `0001_01 imm26`
        sign_extend(word & 0x03FF_FFFF, 26)
    } else if word & 0xFF00_0000 == 0x5400_0000 {
        // B.cond: `0101_0100 imm19 0 cond`
        sign_extend((word >> 5) & 0x7FFFF, 19)
    } else if word & 0x7E00_0000 == 0x3400_0000 {
        // CBZ / CBNZ: `sf 0110_10 op imm19 Rt`
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

/// Non-unique MemRef must list its base in `srcs` (integrity item C).
fn check_mem_base_in_srcs(ew: &EmittedWord) -> Result<(), String> {
    let Some(m) = ew.mem else {
        return Ok(());
    };
    m.require_base_in_srcs(ew.src_slice())
}

fn src_ready(ew: &EmittedWord, ready: &[u64; 32]) -> u64 {
    let mut t = 0u64;
    for &s in ew.src_slice() {
        let i = s as usize;
        if i < 32 {
            t = t.max(ready[i]);
        }
    }
    t
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cost::rule::{CostRule, FlagEffect, MEM_SP_REG, MemRef};

    /// These units score against the **committed** `bench/a76-pi5.toml`,
    /// so a profile edit that breaks the scoreboard shows up here rather
    /// than only in a golden.
    fn table() -> CostTable {
        crate::cost::table::load_default().expect("bench/a76-pi5.toml")
    }

    /// No image is involved in a synthetic word stream, so there is no
    /// placement to consult. Item G's terms are silent here by
    /// construction, which is exactly what an empty table says.
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
        EmittedWord::new(0, String::new(), rule, dst, srcs)
    }

    fn word_flags(rule: CostRule, dst: Option<u8>, srcs: &[u8], flags: FlagEffect) -> EmittedWord {
        word(rule, dst, srcs).with_flags(flags)
    }

    fn load_stack(dst: u8, offset: u64) -> EmittedWord {
        word(CostRule::Load, Some(dst), &[31]).with_mem(MemRef::stack(offset))
    }

    /// A stack load that also reads `dep`, so it cannot issue before
    /// `dep`'s producer retires. The extra source is legal: the MemRef
    /// check requires the base to be *present* in `srcs`, not alone.
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

    fn prog(key: &str, code: Vec<EmittedWord>) -> CodegenProgram {
        let mut fns = BTreeMap::new();
        fns.insert(
            key.to_string(),
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

    // --- the port map -------------------------------------------------------

    /// SOG §2.1 / §4.1 as the profile states them, read back through the
    /// map the scheduler actually uses. If `[pipelines]` and this map ever
    /// disagree the port model is scoring a machine nobody described.
    #[test]
    fn port_map_comes_from_the_profile() {
        let table = table();
        let m = Machine::from_table(&table).expect("machine");
        assert_eq!(m.pipes, 8);
        assert_eq!(m.dispatch_mops, 4);
        assert_eq!(m.dispatch_uops, 8);
        assert_eq!(m.window, 128);
        // I = pipes 2-4 (three of them), M = pipe 4 alone, L = 5-6,
        // D = V0 ∪ V1 = 7-8, B = 1.
        assert_eq!(m.mask("I").unwrap().count_ones(), 3);
        assert_eq!(m.mask("M").unwrap(), 1 << 4);
        assert_eq!(m.mask("L").unwrap(), (1 << 5) | (1 << 6));
        assert_eq!(m.mask("D").unwrap(), (1 << 7) | (1 << 8));
        assert_eq!(m.mask("B").unwrap(), 1 << 1);
        assert_eq!(
            m.mask("V0").unwrap() | m.mask("V1").unwrap(),
            m.mask("D").unwrap()
        );
        // Total port capacity per cycle = 1 B + 3 I + 2 L + 2 V = 8,
        // exactly `dispatch_uops`, so the global uop cap never binds on
        // its own — see `dispatch_mops_bind_before_the_ports`.
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

    /// A store is **one Mop, two uops** (SOG §3.10); `BL` likewise takes
    /// I and B; an FP/ASIMD op's `V0,V1` is one uop with two alternatives,
    /// not two uops.
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
        let neon = m
            .uops_for(ports_for(CostRule::Neon, &table).unwrap())
            .unwrap();
        assert_eq!(neon.len(), 1, "`V0,V1` lists alternatives for one uop");
        assert_eq!(neon[0].pipes, m.mask("D").unwrap());
        let alu = m
            .uops_for(ports_for(CostRule::Alu, &table).unwrap())
            .unwrap();
        assert_eq!(alu.len(), 1);
        assert_eq!(alu[0].pipes.count_ones(), 3, "I is three pipes");
    }

    // --- issue width --------------------------------------------------------

    /// **Three** independent ALU ops issue in one cycle — the pre-M20
    /// two-port model could not, and a fourth still waits because there
    /// are exactly three I pipes (2-4).
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

    /// Two AGUs cap mem issue: two independent loads issue together, a
    /// third does not (`port_l` = pipes 5-6).
    #[test]
    fn two_loads_issue_in_one_cycle_but_not_three() {
        let two = prog("f", vec![load_cold_unique(1, 0), load_cold_unique(2, 1)]);
        assert_eq!(total(&two), 35, "both at cycle 0, retire = lat_l3");
        let three = prog(
            "f",
            vec![
                load_cold_unique(1, 0),
                load_cold_unique(2, 1),
                load_cold_unique(3, 2),
            ],
        );
        assert_eq!(total(&three), 36, "the third load waits a cycle for an AGU");
    }

    /// The store data uop takes a **V** pipe, so a store contends with
    /// FP/ASIMD — a contention the pre-M20 two-port model could not
    /// express at all. Two independent NEON ops fill both V pipes; adding
    /// a store pushes one of them out by a cycle.
    #[test]
    fn store_data_uop_contends_with_neon() {
        let neon = || word(CostRule::Neon, None, &[]);
        let two_neon = prog("f", vec![neon(), neon()]);
        assert_eq!(total(&two_neon), 4, "both V pipes free: lat 4 from cycle 0");
        let with_store = prog("f", vec![store_stack(8), neon(), neon()]);
        assert_eq!(
            total(&with_store),
            5,
            "the store's data uop holds a V pipe, so one NEON op slips to cycle 1"
        );
        // Control: the same three Mops with the store replaced by an ALU
        // op (which takes an I pipe) leave both V pipes free.
        let with_alu = prog(
            "f",
            vec![word(CostRule::Alu, Some(1), &[0, 0]), neon(), neon()],
        );
        assert_eq!(total(&with_alu), 4);
    }

    /// The cap that genuinely binds first is `dispatch_mops` = 4.
    ///
    /// Recorded because the plan's phrasing ("the dispatch cap binds
    /// before the port cap on a B-heavy block") does not survive contact
    /// with the profile: `cap_b` = 2 can never bind, because there is one
    /// B pipe and it accepts one uop per cycle; and the 8-uop cap can
    /// never bind on its own, because total port capacity per cycle is
    /// 1 + 3 + 2 + 2 = 8 exactly. Five single-uop Mops of mixed class have
    /// the ports to issue together and still take two cycles to get in.
    #[test]
    fn dispatch_mops_bind_before_the_ports() {
        // 3 ALU (I) + 1 store (L + V) + 1 branch (B): 5 Mops, 6 uops, and
        // every one of them has a free pipe at cycle 0. All latency 1, so
        // the schedule length is exactly the last dispatch cycle + 1.
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
        // Four of the same Mops all dispatch and issue at cycle 0.
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

    // --- M-pipe blocking ----------------------------------------------------

    /// `SMULH` then `MADD`: SOG §3.6 note 5 holds pipe M **3 extra**
    /// cycles beyond the group's 1/4 throughput, so the `MADD` issues at
    /// 4 + 3 = 7 and retires at 11. The differential against an X-form
    /// `MUL` first (1/3 throughput + note 4's 2 extra ⇒ issue at 5,
    /// retire 9) is what proves both the throughput reciprocal and the
    /// stall are live rather than one standing in for the other.
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
        // Each alone, to separate the hold from the latency.
        assert_eq!(total(&prog("f", vec![smulh])), 5);
        assert_eq!(total(&prog("f", vec![madd])), 4);
    }

    /// A divide blocks a subsequent divide until it completes (SOG §3.6
    /// note 1), across the `sdiv` / `udiv` rows alike, while an
    /// independent ALU op is untouched because it has two other I pipes.
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
        // An independent ALU op still issues at cycle 0 (I pipes 2-3).
        assert_eq!(
            total(&prog(
                "f",
                vec![sdiv(1), word(CostRule::Alu, Some(2), &[0, 0])]
            )),
            20
        );
    }

    /// A consumer of a divide result waits on it. This is the schedule
    /// side of the `codegen.rs` divide-tag fix (the emit site declared
    /// neither `dst` nor `srcs`, so a 20-cycle divide created **no**
    /// dependence edge at all — an under-cost 04 §5 forbids).
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
        // And an untagged divide (the pre-fix shape) is exactly the
        // under-cost the fix removes: no edge, so no wait.
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

    // --- the bounded reorder window ----------------------------------------

    /// A dependence chain longer than the 128-entry window does not
    /// reorder across it: instruction `i` cannot dispatch until
    /// instruction `i - 128` has retired (decision 1606).
    ///
    /// Shape: a 35-cycle cold load at index 0, then a stream tuned so that
    /// dispatch (4 Mops/cycle) is the only limit — three ALU ops and one
    /// FP/ASIMD op per cycle, which is 3 I pipes plus a V pipe. Word 128
    /// would dispatch at cycle 32 on caps alone; the window holds it to
    /// the load's retire at 35.
    #[test]
    fn dependence_chain_longer_than_the_window_does_not_reorder() {
        fn stream(n: usize) -> Vec<EmittedWord> {
            let mut code = vec![load_cold_unique(1, 0)];
            for j in 1..n {
                // One non-I Mop per group of four keeps issue at the
                // dispatch rate instead of the 3-wide I port rate.
                if j % 4 == 0 {
                    code.push(word(CostRule::Neon, None, &[]));
                } else {
                    code.push(word(CostRule::Alu, None, &[0]));
                }
            }
            code
        }
        // 128 words: no index reaches the window, so the load's retire
        // (35) is the schedule.
        let inside = prog("f", stream(128));
        assert_eq!(total(&inside), 35, "nothing crosses a 128-entry window");
        // 129 words: word 128 waits for word 0 to retire (35), then its
        // own FP/ASIMD latency of 4.
        let across = prog("f", stream(129));
        assert_eq!(
            total(&across),
            39,
            "word 128 dispatches at retire[0] = 35, not at its natural cycle 32"
        );
    }

    // --- NZCV de-serialization ---------------------------------------------

    /// A flag **RAW** edge costs exactly what the equivalent GPR dependence
    /// costs. Renaming (SOG §4.10 note 1) removes WAR and WAW hazards; it
    /// cannot let `b.cond` read NZCV before `cmp` computed it. See this
    /// module's correction note — plans/M20.md read note 1 as licence to
    /// delete this edge, which was an under-cost.
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
        // And it is a real edge, not port pressure: the same two words with
        // no flag relationship issue together on their two distinct pipes.
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

    /// **The precise statement of what renaming does**, and the oracle that
    /// keeps this model from drifting back to either error.
    ///
    /// Each shape is scored against a counterfactual in which the flag
    /// register's role is played by **one GPR**, which is exactly a model
    /// that treats NZCV as a single architectural register. Renaming's real
    /// effect is that the two agree on the **RAW** shape and diverge on the
    /// **WAW** and read-with-no-producer shapes, where a single-register
    /// model would invent a hazard renaming eliminates.
    #[test]
    fn renaming_removes_war_and_waw_flag_hazards_but_not_the_raw_edge() {
        /// A scratch GPR standing in for the flag register in the
        /// counterfactual. Not `31` — that encoding is SP/XZR and carries
        /// its own de-serialization (see the SP unit below).
        const NZCV: u8 = 20;

        // Dependent: `cmp` (writes NZCV) → `cset` (reads it, writes a GPR)
        // → `cmp` of that GPR → `cset`. This is the shape `cost-branchy`
        // actually emits, and it is the *only* way a flag chain can be
        // serial here: `FlagEffect` is write-or-read, never both, so a run
        // of bare flag writes is independent however long it is.
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

        // WAW shape: four flag **writes** and no read. A single-register
        // model has a WAW hazard here; a renamed one does not. This is the
        // half SOG §4.10 note 1 genuinely licenses.
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

        // Read-with-no-producer shape: four flag reads and no writer. There
        // is nothing to wait for, in either model.
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

        // The RAW chain really is the expensive shape, so the agreement
        // above is not a saturated-port coincidence.
        assert!(total(&dep_live) > total(&waw_live));
    }

    /// SP is renamed too (SOG §4.10 note 1), and that is a **different
    /// mechanism** from the frame-epoch reuse invalidation an SP write also
    /// triggers. Conflating them would silently restore a stale-reuse bug,
    /// so both halves are asserted here, separately:
    ///
    /// * **SP's RAW edge** — an SP write *does* serialize a later SP-based
    ///   access, because `sub sp, sp, #N` produces the address the load
    ///   needs. Renaming removes WAR/WAW, not this.
    /// * **register 31 as XZR** — a word that merely names 31 without an
    ///   SP-relative address or an add/sub-immediate `Rn` is reading a
    ///   constant with no producer, and waits on nothing. That false edge,
    ///   on every `str xzr` word, is what the pre-M20 model got wrong.
    /// * **frame-epoch invalidation** — `MemState::clear` still fires, so a
    ///   reload of the same frame offset after an SP adjust misses where it
    ///   would otherwise have hit.
    #[test]
    fn sp_raw_edge_holds_while_an_xzr_read_waits_on_nothing() {
        // --- half 1: the SP write serializes a later stack access.
        // `sub sp, sp, #n` then a stack load of a *fresh* key, so the
        // memory verdict is identical across the comparison and only the
        // register edge can differ.
        let sp_then_load = prog(
            "f",
            vec![
                word(CostRule::Alu, Some(MEM_SP_REG), &[MEM_SP_REG]),
                load_stack(1, 0),
                word(CostRule::Alu, Some(2), &[1, 1]),
            ],
        );
        // The counterfactual: the same shape with SP's role played by an
        // ordinary GPR the load also reads. A correct model agrees with it,
        // because this dependence is real.
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
        // And the edge is load-bearing: deleting the SP write shortens it.
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

        // --- half 1b: register 31 read as XZR waits on nothing. A store
        // whose data register is 31 (`str xzr, [sp, #n]`) names 31 in
        // `srcs` but has no SP-relative *value* dependence beyond its own
        // base, so the SP write must not gate an unrelated later consumer.
        let xzr_reader = prog(
            "f",
            vec![
                word(CostRule::Alu, Some(MEM_SP_REG), &[MEM_SP_REG]),
                // Reads 31 twice (base + data) but is not itself SP-based
                // for the purposes of the false edge: a `Cold` address.
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

        // --- half 2: the frame epoch still ends. Same offset reloaded
        // after the SP adjust must miss, which is a *memory* verdict and
        // must survive the de-serialization above.
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
        assert!(
            total(&epoch) > total(&hit),
            "the SP write must still clear the reuse window: epoch {} should \
             exceed same-epoch reuse {}",
            total(&epoch),
            total(&hit)
        );
        // The two mechanisms are independent: the epoch clear costs the
        // *memory* difference (L2 miss vs L1 hit), not a register edge —
        // a load of a **different** key after the same SP write is
        // unaffected, because it was never going to hit.
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

    // --- SOG §4.5 alignment penalties (item I) ------------------------------

    /// A stack load whose `word` fixes the transfer width and whose
    /// `MemRef` fixes the frame offset.
    ///
    /// The two are deliberately allowed to disagree here: `codegen.rs`
    /// cannot emit a `width`-byte access at an offset that is not a
    /// multiple of `width` (`encode::scaled_offset` asserts it), so the
    /// straddling witnesses below are **synthetic by necessity** — which
    /// is the finding, not a workaround. The scoreboard never reads a
    /// load's immediate out of `word`, only its size field.
    fn stack_load_of_width(dst: u8, offset: u64, width_bytes: u8) -> EmittedWord {
        use crate::encode;
        let enc = match width_bytes {
            1 => encode::enc_ldrb_imm(dst, MEM_SP_REG, 0),
            2 => encode::enc_ldrh_imm(dst, MEM_SP_REG, 0),
            4 => encode::enc_ldr_w_imm(dst, MEM_SP_REG, 0),
            8 => encode::enc_ldr_x_imm(dst, MEM_SP_REG, 0),
            w => panic!("no encoder for a {w}-byte load"),
        };
        EmittedWord::new(enc, String::new(), CostRule::Load, Some(dst), &[MEM_SP_REG])
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
        EmittedWord::new(enc, String::new(), CostRule::Store, None, &[MEM_SP_REG, 0])
            .with_mem(MemRef::stack(offset))
    }

    /// The width tag is a projection of the emitted word, so an emitted
    /// load/store always has one and nothing else does.
    #[test]
    fn access_width_is_tagged_from_the_emitted_word() {
        assert_eq!(stack_load_of_width(1, 0, 8).access_bytes, 8);
        assert_eq!(stack_load_of_width(1, 0, 4).access_bytes, 4);
        assert_eq!(stack_load_of_width(1, 0, 2).access_bytes, 2);
        assert_eq!(stack_load_of_width(1, 0, 1).access_bytes, 1);
        assert_eq!(stack_store_of_width(0, 8).access_bytes, 8);
        assert_eq!(stack_store_of_width(0, 1).access_bytes, 1);
        // Not a load/store shape: no width, and the alignment term must
        // treat that as undecided rather than as aligned.
        assert_eq!(word(CostRule::Alu, Some(1), &[0, 0]).access_bytes, 0);
        assert_eq!(load_stack(1, 0).access_bytes, 0, "synthetic word=0 stream");
    }

    /// SOG §4.5, store side: a store straddling a 16 B boundary costs
    /// strictly more than an aligned one, and the magnitude comes from the
    /// **sweep box** rather than a pin.
    #[test]
    fn a_store_straddling_the_16_b_boundary_costs_more() {
        let table = table();
        let place = placement();
        let point = SweepPoint::pinned(&table);
        // Offset 8, width 8: [8, 16) — inside one 16 B block.
        let aligned = prog("f", vec![stack_store_of_width(8, 8)]);
        // Offset 12, width 8: [12, 20) — straddles the boundary at 16.
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
        // Read through the point, never pinned: the bracket's low end must
        // move the schedule.
        let lo = point.with("store_boundary_cross_penalty", 1);
        let cheap = score_program_at(&straddle, &table, &place, &lo)
            .expect("lo")
            .total_proxy_cycles;
        assert!(
            cheap < s,
            "store_boundary_cross_penalty must reach the schedule: {cheap} vs {s}"
        );
        // A narrower store at the same offset fits inside the block.
        let narrow = prog("f", vec![stack_store_of_width(12, 4)]);
        assert_eq!(
            score_program(&narrow, &table, &place)
                .expect("narrow")
                .total_proxy_cycles,
            a,
            "a 4-byte store at offset 12 ends exactly on the boundary"
        );
    }

    /// SOG §4.5, load side: a load straddling a 64 B cache line costs
    /// strictly more than one inside it.
    ///
    /// `sp` is unknown modulo 64 (the ABI pins it only modulo 16), so the
    /// verdict quantifies over every `sp` the ABI permits and charges if
    /// *any* of them crosses — the over-cost reading of decision 1609.
    /// Offset 60 crosses the line whenever `sp ≡ 0 (mod 64)`, so it is
    /// charged; offset 56 crosses for no permitted `sp`.
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

    /// The crossing predicate itself, including the part that is *not*
    /// exact: `sp` is known only modulo `sp_align`, so the term charges if
    /// any permitted `sp` crosses.
    #[test]
    fn crosses_boundary_quantifies_over_every_permitted_sp() {
        let sp = crate::codegen::FRAME_SP_ALIGN_BYTES;
        assert_eq!(sp, 16);
        // Natural alignment ⇒ never crosses, at either boundary, for every
        // width and every 8-byte-multiple offset in one line.
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
        // 16 B is exact: `sp ≡ 0 (mod 16)`, so the offset decides alone.
        assert!(crosses_boundary(12, 8, 16, sp));
        assert!(!crosses_boundary(12, 4, 16, sp));
        assert!(crosses_boundary(15, 2, 16, sp));
        // 64 B is the over-approximation. Offset 12 with width 8 crosses
        // the line only when `sp ≡ 48 (mod 64)`, so a model that knew `sp`
        // exactly would not charge it — this one does, because it does not.
        assert!(crosses_boundary(12, 8, 64, sp), "some permitted sp crosses");
        assert!(
            !crosses_boundary(12, 8, 64, 64),
            "with sp pinned mod 64 the verdict is exact and there is no crossing"
        );
        // Offset 60 crosses for `sp ≡ 0 (mod 64)` and is charged either way.
        assert!(crosses_boundary(60, 8, 64, sp));
        assert!(crosses_boundary(60, 8, 64, 64));
        // Degenerate inputs never invent a charge.
        assert!(!crosses_boundary(60, 0, 64, sp));
        assert!(!crosses_boundary(60, 8, 0, sp));
    }

    /// A `Cold` base is **not** decided: its alignment is not a fact this
    /// model has (plans/M20.md decision 1611). The same offset/width pair
    /// that is charged against `sp` is charged nothing against a computed
    /// base — and that is the recorded residual, not an oversight.
    #[test]
    fn a_cold_base_is_not_decided_and_charges_nothing() {
        let table = table();
        let point = SweepPoint::pinned(&table);
        let stack = stack_store_of_width(12, 8);
        assert_eq!(
            alignment_penalty(&stack, &table, &point).expect("stack"),
            point.get("store_boundary_cross_penalty")
        );
        let cold = EmittedWord::new(
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
        let unique = EmittedWord::new(
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

    /// **The finding, measured rather than reasoned** (plans/M20.md item
    /// I): over the whole `cost-*` corpus, every emitted load/store carries
    /// a width, every `Stack` access is naturally aligned, and therefore
    /// neither §4.5 penalty is reachable. wrela's fixed frame makes both
    /// terms live-but-unexercised by construction.
    ///
    /// This fails if a future emit site produces an access this projection
    /// cannot size (an `LDP`/`STP`/vector form) or a frame offset that is
    /// not a multiple of its own transfer width.
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
                    // plans/M20.md item M: `cost-crosscore` is the first
                    // case to reach the **ordered** accesses, and the
                    // `LDAR`/`STLR` emit sites carry no `MemRef` at all
                    // (`ldar w11, [x9]` — a bare base register, no
                    // immediate). That is recorded here rather than fixed:
                    // it means row 39's "takes its plain twin's memory
                    // path" is, on today's stream, the `Unresolved` path
                    // (`lat_l3`) and not a `MemRef`-resolved one, and it
                    // means §4.5 declines to decide them. Tagging those
                    // sites is item G's surface, not this oracle's.
                    // Everything else must still be tagged: an untagged
                    // *plain* load/store is a defect and fails here.
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

    // --- pre-M20 behaviour that must survive -------------------------------

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

    /// `words` is the emitted word count and equals Σ Terms.
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
        let term_sum: u64 = r.fns[0].terms.values().sum();
        assert_eq!(term_sum, r.fns[0].words, "Σ Terms must equal words");
    }

    /// Footprint growth is priced through the schedule, and it scales.
    ///
    /// This replaces the pre-M20 `working_set_surcharge_scales_with_overflow`,
    /// whose fixture was polluted: offsets 0/8/16/24/32/40 are all inside
    /// **one** 64 B line, so its "distinct keys" were one line and its deltas
    /// came from the added instructions rather than from footprint. At 64 B
    /// line grain the same claim is made honestly — each extra *line* is a
    /// compulsory reference at `lat_l2`, not a hit at `lat_l1d_hit` — and the
    /// deleted surcharge (2 per key over a cap of 4) adds nothing on top of
    /// that 7-cycle differential. `cost::mem`'s
    /// `reuse_distance_subsumes_the_working_set_surcharge` carries the
    /// deletion argument; this is its schedule-level half.
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
        // One line, revisited: every reload after the first is an L1D hit.
        let one_line = total(&prog("f", chain(&[0, 8, 16, 24, 32, 40])));
        // Six distinct lines: six compulsory references.
        let six_lines = total(&prog("f", chain(&[0, 64, 128, 192, 256, 320])));
        assert!(
            six_lines > one_line,
            "6 lines {six_lines} must exceed 6 offsets in 1 line {one_line}"
        );
        // And it scales per line: the 6th line costs the same 7-cycle
        // differential the 5th did, so growth is priced rather than capped.
        let five = total(&prog("f", chain(&[0, 64, 128, 192, 256])));
        let four = total(&prog("f", chain(&[0, 64, 128, 192])));
        assert_eq!(
            six_lines - five,
            five - four,
            "each extra line costs the same compulsory differential"
        );
        assert_eq!(five - four, 11 + 1, "lat_l2 for the line plus its alu");
    }

    /// Soundness oracle (monotonicity): appending a dead, independent word
    /// must never *lower* a schedule. A model that rewards adding work is
    /// one an opt can win by growing the stream.
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

        // Dead: writes an otherwise-unused reg, reads an unwritten one.
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

    /// The report carries the **profile's** identity — pipeline set,
    /// dispatch constraints, bounded reorder window, and both digests —
    /// and the scoreboard is built from those same rows.
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
        // Independent alu + cold load: both cycle 0; retire = max(1, 35).
        let mixed = prog(
            "f",
            vec![
                word(CostRule::Alu, Some(1), &[0, 0]),
                load_cold_unique(2, 0),
            ],
        );
        assert_eq!(total(&mixed), 35);
    }

    /// The seam, from the scheduler's side (item H): `[branch]
    /// mispredict_penalty` is a real 14 cycles in the profile, and the
    /// charge is **bias-scaled** — zero with no data, the full penalty at a
    /// measured 50/50. A lone branch word with no resolvable target has no
    /// bias, so it still costs its T1 latency alone.
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
        // The seam reads its magnitude through the point, not the table.
        let even = BranchBias::from_distinct_counts(5, 5).expect("bias");
        assert_eq!(branch_mispredict_charge(&table, &point, Some(even)), 14);
        assert_eq!(
            branch_mispredict_charge(&table, &point.with("mispredict_penalty", 11), Some(even)),
            11
        );
        // Well predicted ⇒ ~0 whatever the penalty is.
        let skewed = BranchBias::from_distinct_counts(1, 999).expect("bias");
        assert!(branch_mispredict_charge(&table, &point, Some(skewed)) <= 1);
    }

    /// `abort_val` is a `BL` (T1: lat 1, ports I + B) plus the swept
    /// `call_overhead` residual, pinned at 0.
    #[test]
    fn abort_val_uses_table_latency() {
        assert_eq!(
            total(&prog("f", vec![word(CostRule::AbortVal, None, &[0])])),
            1
        );
    }

    /// A stack reuse beats a reference to a **different line**. Offset 16
    /// used to be the "miss" side of this unit and is now a hit: at 64 B
    /// line grain offsets 8 and 16 are the same line, which is the whole
    /// point of the grain change, so the miss side moves to offset 128.
    #[test]
    fn stack_hit_cheaper_than_a_different_line() {
        // Serial: each reload waits on the previous result, so the second
        // load's own hit/miss verdict is on the critical path.
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
        assert!(
            total(&hit) < total(&other_line),
            "stack hit schedule {} should beat another line {}",
            total(&hit),
            total(&other_line)
        );
    }

    /// **The pre-M20 behaviour this replaces.** A store used to *invalidate*
    /// its key so the reload missed; under a store buffer the reload
    /// **forwards** (SOG §3.10, inventory row 13). That is a cheapening, and
    /// it is correct — a spill/fill pair really is served from the buffer on
    /// this machine. At the pinned point the forward costs exactly
    /// `lat_l1d_hit`, so the store adds only its own cycle; at the swept
    /// bracket's low end the forward is cheaper than the L1 path, which is
    /// why `[sweep.store_to_load_forwarding]`'s high end is pinned at 4.
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
        // The forwarded reload is genuinely on the forwarding path, and the
        // swept dimension reaches it through the whole scoreboard.
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
            total(&after_call) > total(&hit),
            "call-cleared reload {} should exceed hit {}",
            total(&after_call),
            total(&hit)
        );
    }

    #[test]
    fn cold_unique_always_misses() {
        // Serial chain: distinct cold uniques never hit each other, so
        // each pays lat_l3 in turn.
        let two = prog(
            "f",
            vec![
                load_cold_unique(1, 0),
                word(CostRule::Alu, Some(1), &[1, 1]),
                load_cold_unique_after(2, 1, 1),
                word(CostRule::Alu, Some(3), &[2, 2]),
            ],
        );
        // load@0→35, alu@35→36, load@36→71, alu@71→72.
        assert_eq!(total(&two), 72);
        let stack_hit = prog(
            "f",
            vec![
                load_stack(1, 8),
                word(CostRule::Alu, Some(1), &[1, 1]),
                load_stack_after(2, 8, 1),
                word(CostRule::Alu, Some(3), &[2, 2]),
            ],
        );
        assert!(
            total(&two) > total(&stack_hit),
            "cold unique miss {} should exceed stack hit {}",
            total(&two),
            total(&stack_hit)
        );
    }

    #[test]
    fn missing_memref_is_cold_miss() {
        let untagged = prog("f", vec![word(CostRule::Load, Some(1), &[0])]);
        assert_eq!(total(&untagged), 35); // lat_l3
    }

    /// **A 5-way conflict inside capacity still misses on 4-way**, seen
    /// through the whole scoreboard rather than only through `cost::mem`.
    /// L1D has 256 sets, so offsets 0 / 16 KiB / 32 KiB / 48 KiB / 64 KiB all
    /// index set 0; the reload of offset 0 is 5 deep in a 4-way set and takes
    /// the L2 path. Five 64 B lines is 320 B against a 64 KiB cache, so no
    /// capacity term can explain it — this is the associativity term being
    /// live rather than decorative.
    #[test]
    fn five_way_conflict_inside_capacity_costs_more_than_a_reuse() {
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
        // Five lines that collide in one set, then reload the first.
        let conflict: Vec<u64> = (0..5).map(|k| k * SET_STRIDE).collect();
        // The control: five lines that do not collide, same count, same
        // dependence chain, same instruction multiset.
        let spread: Vec<u64> = (0..5).map(|k| k * 64).collect();
        let a = total(&serial(&conflict));
        let b = total(&serial(&spread));
        assert!(
            a > b,
            "a 5-way conflict {a} must cost more than a spread working set {b}"
        );
        assert_eq!(
            a - b,
            11 - 4,
            "the reload takes lat_l2 instead of an L1 hit"
        );
    }

    /// Call clobber: consumer of x0 waits on call retire (dst=Some(0)).
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
        assert_eq!(total(&no_dep), 1);
    }

    /// Mid-stream branch: followers wait on control-ready.
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

    /// Adrp is port I: it dual-issues with an independent load, and two
    /// Adrps share the three I pipes.
    #[test]
    fn adrp_dual_issues_with_load() {
        let mixed = prog(
            "f",
            vec![word(CostRule::Adrp, Some(1), &[]), load_cold_unique(2, 0)],
        );
        assert_eq!(total(&mixed), 35);
        let two_adrp = prog(
            "f",
            vec![
                word(CostRule::Adrp, Some(1), &[]),
                word(CostRule::Adrp, Some(2), &[]),
            ],
        );
        assert_eq!(total(&two_adrp), 1);
    }

    /// SP write clears the mem reuse window (epoch), like Call.
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
        assert!(
            total(&after_sp) > total(&hit),
            "sp-cleared reload {} should exceed hit {}",
            total(&after_sp),
            total(&hit)
        );
    }

    /// Store keeps other keys hittable; SP adjust then reload misses.
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
        assert!(
            total(&after_sp) > total(&after_store),
            "store→sp→reload miss {} should exceed store-only hit {}",
            total(&after_sp),
            total(&after_store)
        );
    }

    /// Non-unique MemRef without base∈srcs fails closed.
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
        EmittedWord::new(enc, String::new(), rule, dst, srcs)
    }

    /// Leaders at 0, fallthrough after branch, and PC-relative targets.
    #[test]
    fn basic_blocks_split_on_branch_targets_and_fallthrough() {
        use crate::encode::{Cond, enc_b, enc_b_cond};
        // Layout: 0:alu  1:b.eq ->4  2:alu  3:b ->5  4:alu  5:alu
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

    /// Under f≡1, Σ s(b) equals the per-fn schedule `score_fn` / dump flat uses.
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

    /// Every `CostRule` scores: no variant may reach the scheduler without
    /// a port string, a throughput, and a latency (freeze 1632's shape at
    /// scoreboard grain — an unpriced rule would panic or spin).
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

    /// The pinned point is what a golden pins (freeze 1625), and
    /// `score_program` must agree with `score_program_at` there.
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
        // The divide's latency comes from the point, not the row: moving
        // the swept dimension to its low end must move the schedule.
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
