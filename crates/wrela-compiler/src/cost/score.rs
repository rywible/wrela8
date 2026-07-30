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
//! - **No NZCV serialization.** NZCV and SP are fully renamed on A76 (SOG
//!   §4.10 note 1), so the pre-M20 scoreboard's flag edge was wrong for
//!   this core and is gone. `FlagEffect` itself stays — item I and the
//!   report read it.
//!
//! Four terms are **provisional seams**, each owned by a following item
//! and each a separate function so those items merge cleanly:
//! [`mem_access_latency`] (item F), [`crosscore_extra`] (item G),
//! [`branch_mispredict_charge`] (item H), [`alignment_penalty`] (item I).
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

use std::collections::{BTreeMap, VecDeque};

use crate::codegen::{CodegenFn, CodegenProgram};
use crate::placement::PlacementTable;

use super::owner::classify_owner;
use super::rule::{CostRule, EmittedWord, MEM_SP_REG, MemClass, MemRef};
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

/// Carried state of the memory model across one block's word stream.
///
/// Today this is the pre-M20 reuse **window** (a count of recent
/// `MemRef`s, `[transitional] mem_reuse_window`), which is why the
/// section it reads is a quarantine item F deletes. Item F replaces it
/// with reuse distance at 64 B line grain over real capacities and
/// associativity, L2 inclusivity, and store-buffer forwarding.
#[derive(Debug, Clone)]
pub struct MemState {
    window: VecDeque<WinEntry>,
    cap: usize,
}

impl MemState {
    pub fn new(table: &CostTable) -> MemState {
        MemState {
            window: VecDeque::new(),
            cap: table.mem_reuse_window().max(1) as usize,
        }
    }

    /// Call clobber and SP writes (a new frame epoch) end reuse: stack
    /// offsets after an SP adjust name different bytes.
    pub fn clear(&mut self) {
        self.window.clear();
    }

    fn push(&mut self, entry: WinEntry) {
        self.window.push_back(entry);
        while self.window.len() > self.cap {
            self.window.pop_front();
        }
    }
}

/// One slot in the mem reuse window: stores push for working-set only
/// (`grants_hit = false`); loads push hit-eligible entries.
#[derive(Debug, Clone, Copy)]
struct WinEntry {
    mem: MemRef,
    grants_hit: bool,
}

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

/// Measured taken/total for one branch's block (item H).
///
/// Placeholder: nothing constructs one yet. Under `W_flat` there is no
/// bias information at all, which is exactly why item H charges zero
/// there — no data, no charge (decision 1609).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BranchBias {
    pub taken: u64,
    pub total: u64,
}

// ---------------------------------------------------------------------------
// The four seams
// ---------------------------------------------------------------------------

/// **Item F** owns this. Reuse distance at 64 B line grain over real
/// capacities and associativity (4-way L1D ⇒ a conflict verdict, not only
/// a capacity verdict), L2 strict inclusivity of L1D, and store-buffer
/// forwarding.
///
/// Today: the pre-M20 hit/miss window derived from `[geometry]` via
/// `CostTable::mem()`, plus the `working_set_surcharge` nudge. The L2 and
/// L3 leaves are read through `point` so item J's sweep already reaches
/// them; everything else is pinned. Returns the whole memory latency of
/// this word (including the surcharge) and advances `state` in **program**
/// order.
fn mem_access_latency(
    ew: &EmittedWord,
    table: &CostTable,
    point: &SweepPoint,
    state: &mut MemState,
) -> u64 {
    // Leaf latencies: `lat_l1d_hit` is T1 and unbracketed, so it is read
    // pinned; `lat_l2` / `lat_l3` are bracketed and swept.
    let l1d = table.mem().load_stack_hit;
    let l2 = point.get("l2_latency");
    let l3 = point.get("l3_latency");

    if ew.rule.is_load() {
        let lat = match ew.mem {
            None => l3, // untagged address: cold miss, no window push
            Some(m) => {
                let hit = state.window.iter().any(|e| e.grants_hit && e.mem == m);
                let lat = match (m.class, hit) {
                    (MemClass::Stack, true) | (MemClass::Cold, true) => l1d,
                    (MemClass::Stack, false) => l2,
                    (MemClass::Cold, false) => l3,
                };
                state.push(WinEntry {
                    mem: m,
                    grants_hit: true,
                });
                lat.saturating_add(working_set_surcharge(state, table))
            }
        };
        return lat;
    }

    // Store: latency 1 to the **store buffer** (SOG §3.10), not to cache.
    let store = table.mem().store_stack;
    let Some(m) = ew.mem else {
        return store;
    };
    // A store invalidates every prior hit-granting entry for this key,
    // then pushes a working-set-only entry.
    state.window.retain(|e| e.mem != m);
    state.push(WinEntry {
        mem: m,
        grants_hit: false,
    });
    store.saturating_add(working_set_surcharge(state, table))
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

/// **Item H** owns this. The mispredict charge, scaled by measured
/// per-block branch bias: a well-predicted branch costs ~0 whatever
/// `[branch] mispredict_penalty` is, and a near-50/50 one costs the full
/// swept penalty. Zero under `W_flat`, where there is no bias information
/// — no data, no charge (decision 1609).
///
/// Today: zero, for the reason `CostTable::branch_penalty` records — this
/// scoreboard cannot argue that a branch *is* mispredicted, so charging
/// the penalty unconditionally would over-credit branch *removal*.
fn branch_mispredict_charge(
    table: &CostTable,
    point: &SweepPoint,
    bias: Option<BranchBias>,
) -> u64 {
    let _ = (point, bias);
    table.branch_penalty()
}

/// **Item I** owns this. SOG §4.5's two boundaries: a load crossing a
/// 64 B cache line, a store crossing a 16 B boundary. Both are statically
/// decidable from wrela's fixed 8-byte-slot frame layout, so this term
/// needs no dynamic information at all — the magnitudes are the swept
/// `load_line_cross_penalty` / `store_boundary_cross_penalty`.
///
/// Today: zero.
fn alignment_penalty(ew: &EmittedWord, table: &CostTable, point: &SweepPoint) -> u64 {
    let _ = (ew, table, point);
    0
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
    let machine = Machine::from_table(table)?;
    let mut fns = Vec::with_capacity(program.fns.len());
    let mut total_proxy_cycles = 0u64;
    let mut owner_totals = BTreeMap::from([
        ("app".to_string(), 0u64),
        ("runtime".to_string(), 0u64),
        ("driver".to_string(), 0u64),
    ]);

    let mut total_words = 0u64;
    for (key, f) in &program.fns {
        let (proxy_cycles, terms) = score_fn(key, f, table, placement, point, &machine)?;
        let words = f.code.len() as u64;
        total_proxy_cycles = total_proxy_cycles.saturating_add(proxy_cycles);
        total_words = total_words.saturating_add(words);
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
        total_proxy_cycles,
        total_words,
        owner_totals,
        fns,
        workloads_digest: None,
        workload_totals: BTreeMap::new(),
        workload_coverage: BTreeMap::new(),
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
pub fn block_schedule_lengths(
    fn_key: &str,
    code: &[EmittedWord],
    table: &CostTable,
    placement: &PlacementTable,
) -> Result<Vec<u64>, String> {
    let machine = Machine::from_table(table)?;
    let point = SweepPoint::pinned(table);
    let mut out = Vec::new();
    for (start, end) in basic_block_ranges(code) {
        let (s, _) = score_words(
            fn_key,
            &code[start..end],
            table,
            placement,
            &point,
            &machine,
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
) -> Result<(u64, BTreeMap<String, u64>), String> {
    let mut terms: BTreeMap<String, u64> = BTreeMap::new();
    if f.code.is_empty() {
        return Ok((0, terms));
    }
    let mut proxy_cycles = 0u64;
    for (start, end) in basic_block_ranges(&f.code) {
        let (s, block_terms) =
            score_words(key, &f.code[start..end], table, placement, point, machine)?;
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
fn score_words(
    fn_key: &str,
    code: &[EmittedWord],
    table: &CostTable,
    placement: &PlacementTable,
    point: &SweepPoint,
    machine: &Machine,
) -> Result<(u64, BTreeMap<String, u64>), String> {
    let mut terms: BTreeMap<String, u64> = BTreeMap::new();
    if code.is_empty() {
        return Ok((0, terms));
    }

    let mut mem = MemState::new(table);
    // Per-pipe next-free cycle. Index 0 unused (pipes are 1-based).
    let mut pipe_free = vec![0u64; machine.pipes + 1];
    // Functional-unit hold per dispatch class — only ever non-zero for a
    // class whose group holds its unit for more than one cycle (pipe M).
    let mut unit_free = [0u64; PORT_CLASS_COUNT];
    // A `m_pipe_block` group (divide) blocks the next such group until it
    // **completes** (SOG §3.6 note 1).
    let mut block_free = 0u64;
    let mut ready = [0u64; 32];
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
        *terms.entry(ew.rule.as_str().to_string()).or_insert(0) += 1;
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
            let mut earliest = base_ready;
            if occupancy > 1 {
                earliest = earliest.max(unit_free[u.class.index()]);
            }
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
            CostRule::Branch => {
                exec_lat.saturating_add(branch_mispredict_charge(table, point, None))
            }
            r if r.is_load() || r.is_store() => mem_access_latency(ew, table, point, &mut mem),
            _ => exec_lat,
        };
        lat = lat.saturating_add(cross.extra_cycles);
        lat = lat.saturating_add(alignment_penalty(ew, table, point));

        let finish = issue.saturating_add(lat);
        retire[i] = finish;
        if let Some(d) = ew.dst {
            let d = d as usize;
            if d < 32 {
                ready[d] = finish;
            }
        }
        // NZCV is fully renamed on A76 (SOG §4.10 note 1) — no flag edge.
        if ew.rule == CostRule::Branch {
            control_ready = finish;
        }
        if cross.serializes_window {
            serial_until = serial_until.max(finish);
        }
        max_retire = max_retire.max(finish);

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
fn branch_target_index(word: u32, from: usize) -> Option<usize> {
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

/// Surcharge for a window holding more distinct keys than the cap.
///
/// Scales with the overflow: `(distinct − cap) × working_set_surcharge`.
/// A flat charge would price a fn touching 5 distinct addresses the same
/// as one touching 500 — i.e. **under-cost** footprint growth, the one
/// direction 04 §5's proxy soundness rule forbids. Item F decides whether
/// this still adds ranking information once reuse distance exists.
fn working_set_surcharge(state: &MemState, table: &CostTable) -> u64 {
    let mut keys: Vec<(u8, u64)> = state
        .window
        .iter()
        .map(|e| {
            let class = match e.mem.class {
                MemClass::Stack => 0u8,
                MemClass::Cold => 1u8,
            };
            (class, e.mem.key)
        })
        .collect();
    keys.sort_unstable();
    keys.dedup();
    let distinct = keys.len() as u64;
    let over = distinct.saturating_sub(table.mem_working_set_cap());
    over.saturating_mul(table.mem().working_set_surcharge)
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

    /// SOG §4.10 note 1: NZCV and SP are fully **renamed** on A76, so the
    /// pre-M20 scoreboard's flag edge was wrong for this core. A dependent
    /// flag chain no longer serializes; a true GPR dependence still does.
    #[test]
    fn nzcv_chain_does_not_serialize_but_a_gpr_chain_does() {
        let flags = prog(
            "f",
            vec![
                word_flags(CostRule::Alu, None, &[1, 2], FlagEffect::Write),
                word_flags(CostRule::Branch, None, &[], FlagEffect::Read),
            ],
        );
        assert_eq!(
            total(&flags),
            1,
            "cmp and b.cond take different pipes and no renamed-flag edge exists"
        );
        let gpr = prog(
            "f",
            vec![
                word(CostRule::Alu, Some(1), &[0, 0]),
                word(CostRule::Alu, Some(2), &[1, 1]),
            ],
        );
        assert_eq!(
            total(&gpr),
            2,
            "a real register dependence still serializes"
        );
        // A longer flag chain stays flat while the GPR chain grows.
        let flag_chain = prog(
            "f",
            vec![
                word_flags(CostRule::Alu, None, &[1, 2], FlagEffect::Write),
                word_flags(CostRule::Alu, None, &[3, 4], FlagEffect::Read),
                word_flags(CostRule::Alu, None, &[5, 6], FlagEffect::Write),
                word_flags(CostRule::Alu, None, &[7, 8], FlagEffect::Read),
            ],
        );
        assert_eq!(total(&flag_chain), 2, "four I-class Mops, three pipes");
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

    /// Surcharge scales with overflow: 6 distinct keys over cap=4 costs
    /// strictly more than 5 distinct, which costs more than at-cap. The
    /// chain is genuinely serial (each load reads the previous result), so
    /// out-of-order issue cannot hide the surcharge off the critical path.
    #[test]
    fn working_set_surcharge_scales_with_overflow() {
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
        let a = total(&prog("f", chain(&[0, 8, 16, 24])));
        let b = total(&prog("f", chain(&[0, 8, 16, 24, 32])));
        let c = total(&prog("f", chain(&[0, 8, 16, 24, 32, 40])));
        assert!(b > a, "5 distinct {b} must exceed 4 distinct {a}");
        // The scaling claim: the 6th distinct key costs strictly more than
        // the 5th did — a flat surcharge would make these deltas equal.
        let delta_5 = b - a;
        let delta_6 = c - b;
        assert!(
            delta_6 > delta_5,
            "surcharge must scale: Δ6 {delta_6} should exceed Δ5 {delta_5}"
        );
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

    /// `[branch] mispredict_penalty` is a real 14 cycles in the profile,
    /// but `branch_mispredict_charge` is **0 until item H**: this
    /// scoreboard has no branch bias, so charging the penalty
    /// unconditionally would over-credit branch *removal*.
    #[test]
    fn branch_charges_latency_only_until_item_h() {
        let table = table();
        assert_eq!(
            total(&prog("f", vec![word(CostRule::Branch, None, &[])])),
            1
        );
        let point = SweepPoint::pinned(&table);
        assert_eq!(branch_mispredict_charge(&table, &point, None), 0);
        assert_eq!(
            table.branch_row("mispredict_penalty").unwrap().value,
            14,
            "the profile pins the real penalty; item H charges it bias-scaled"
        );
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

    #[test]
    fn stack_hit_cheaper_than_miss() {
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
        let miss = prog(
            "f",
            vec![
                load_stack(1, 8),
                word(CostRule::Alu, Some(1), &[1, 1]),
                load_stack_after(2, 16, 1),
                word(CostRule::Alu, Some(3), &[2, 2]),
            ],
        );
        assert!(
            total(&hit) < total(&miss),
            "stack hit schedule {} should beat miss {}",
            total(&hit),
            total(&miss)
        );
    }

    #[test]
    fn store_kills_stack_hit() {
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
        assert!(
            total(&after_store) > total(&reuse),
            "store-invalidated reload {} should exceed hit reload {}",
            total(&after_store),
            total(&reuse)
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

    #[test]
    fn working_set_surcharge_when_distinct_over_cap() {
        // cap=4: five distinct stack keys → surcharge on the 5th.
        let over = prog(
            "f",
            vec![
                load_stack(1, 0),
                load_stack(2, 8),
                load_stack(3, 16),
                load_stack(4, 24),
                load_stack(5, 32),
            ],
        );
        let under = prog(
            "f",
            vec![
                load_stack(1, 0),
                load_stack(2, 8),
                load_stack(3, 16),
                load_stack(4, 24),
                load_stack(5, 0), // reuse key 0 — still 4 distinct
            ],
        );
        assert!(
            total(&over) > total(&under),
            "surcharge schedule {} should exceed under-cap {}",
            total(&over),
            total(&under)
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
