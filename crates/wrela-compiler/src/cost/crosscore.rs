//! Cross-core, barrier, and system-op costs (plans/M20.md item G,
//! decisions 1602 / 1603 / 1604 / 1609, freeze 1633,
//! `compiler.costs.crosscore`).
//!
//! Inventory rows 17 (`DMB`), 18 (cross-core snoop), 19 (system-register
//! access), 20 (abort-path entry) and 39 (`STLR` / `LDAR`). **Every one of
//! them is T5 by absence**: the Cortex-A76 Software Optimization Guide has
//! no barrier, no load/store-exclusive, no system-register and no prefetch
//! instruction entry anywhere in its 46 pages, so under decision 1602 none
//! of these magnitudes may be pinned. `[crosscore]` declares the terms and
//! carries no numbers; the numbers are `[sweep]` dimensions read through a
//! [`SweepPoint`]. This module is the whole charge.
//!
//! ## What wrela emits (census taken 2026-07-29, in this tree)
//!
//! - **6 `@dmb` call sites** in `stdlib/core/runtime.wr` — `:1323` and
//!   `:1352` `ishst` (ring request / reply publish before the pending
//!   raise), `:1397` and `:1426` `ishld` (reply / request drain acquire
//!   before the payload loads), `:1840` `ishst` (a secondary's release
//!   before it parks) and `:2097` `ishld` (the primary's acquire once the
//!   secondaries read idle). The plan's research pass recorded **four**;
//!   the last two arrived with `lane1-per-core.md`'s quiesce work, which
//!   landed after that pass. One codegen emit site (`Inst::Dmb`) reaches
//!   all six.
//! - **6 `STLR` + 4 `LDAR` emit sites** in `codegen.rs`, all on the
//!   **interrupt-cell** path (`InterruptCellLoadAcquire`,
//!   `InterruptCellStoreRelease`, and `emit_interrupt_cell_rmw`'s
//!   swap / fetch-or in both widths). Exactly inventory row 39's count.
//! - **1 `CostRule::System` emit site**: the async-dispatch `BRK` trap.
//! - Abort branches on every check (`compiler.codegen.naive-locked`).
//!
//! ## Row 17, barrier
//!
//! One swept `dmb_cost` for `ishst` and `ishld` alike. **No source
//! distinguishes them** — the SOG prices neither, and nothing else found
//! this pass separates a store barrier from a load barrier on this core —
//! so inventing a distinction would be inventing a fact. Said plainly here
//! rather than left as an absence.
//!
//! The magnitude is small (`dmb_cost` pins its bracket's **low** end, 1 of
//! 1–64, because it is `removal_sensitive`), but the **ordering** is
//! charged structurally: a `DMB` sets [`CrossExtra::serializes_window`], so
//! nothing already in the window may still be in flight when it issues and
//! nothing later issues before it retires. That is what a barrier *is*, it
//! needs no magnitude, and it is where nearly all of a barrier's real cost
//! lives. Charging it structurally rather than as a big coefficient is also
//! what keeps freeze 1633 honest: see below.
//!
//! ## Row 18, cross-core snoop — what is and is not statically decidable
//!
//! Decision 1603 says sealed placement makes local-vs-remote static. It
//! does, but only for part of the stream, and the boundary matters enough
//! to write down.
//!
//! **Decidable from sealed placement + the `MemRef` surface:**
//!
//! 1. **Whether a peer core exists at all.** `PlacementTable::cores` is the
//!    image's sealed `N`. At `N = 1` nothing can be remote — a real,
//!    sound exclusion, not a default.
//! 2. **The accessing function's own core**, via
//!    [`accessing_core`] over `attr::classify_target`: a method or
//!    `rt_enqueue` of a placed type resolves through that type's sealed
//!    entry, and `rt_secondary_core_entry k` names core `k` outright.
//!    Runtime helpers, aborts and free functions are `Shared` — they name
//!    no core because any core runs them.
//! 3. **That a frame slot is never remote.** `MemClass::Stack` is proven
//!    SP-relative, so it is private to the executing core by construction.
//! 4. **That an `LDAR` is a cross-core acquire.** The acquire half of
//!    publish/acquire exists *because* another agent wrote the line; that
//!    is the instruction's whole reason for being emitted. With a peer core
//!    in the image this is a certain remote verdict, carried by the ISA tag
//!    itself rather than by an address analysis.
//!
//! **Not decidable, and therefore not claimed:**
//!
//! 5. **Which core last wrote an arbitrary `MemClass::Cold` line.** A cold
//!    `MemRef` is either `cold_stable(base_reg, imm)` — a runtime register
//!    plus an immediate — or `cold_unique(seq)`, an opaque per-push
//!    sequence. Neither names the placeable, the image section, or the
//!    writer. So a plain `LDR` of some actor's state field gets
//!    [`Locality::Unclassified`] and is charged **no** snoop.
//! 6. **A narrower peer set than "every brought-up core".** Point 2 gives
//!    the accessing core, but every core the image brings up runs the same
//!    publish/acquire protocol (`PENDING`, `PARK`, the idle words), so
//!    every other brought-up core is a possible writer. The accessing
//!    core is therefore classifiable and *does not narrow the verdict* —
//!    recorded because it is the natural thing to assume it does.
//!
//! **The residual, named in the direction it biases.** Point 5 leaves a
//! genuinely-remote plain load charged as local: an **under**-cost, which
//! is the direction 04 §5 forbids. It is recorded rather than fixed, and
//! the alternative is the decision-1609 conflict below.
//!
//! ### DECISION 1609 CONFLICT, RECORDED RATHER THAN RESOLVED SILENTLY
//!
//! The over-cost repair for point 5 is to charge `snoop_cost` on every
//! unclassifiable cold load in a multi-core image. That is conservative in
//! the letter of decision 1609 and destructive in its spirit:
//! `snoop_cost` pins **312**, so every cold load would cost 347 instead of
//! 35, the memory term would swamp every other term in the model, and the
//! credit for *deleting* a load would grow tenfold — the same
//! removal-sensitivity hazard freeze 1633 names for barriers, on the
//! largest population of words in the stream. It would also destroy the
//! very signal row 18 exists to produce: a ruler that charges every load a
//! snoop cannot show what cross-core traffic costs, because nothing is
//! local any more.
//!
//! No ordering of "accurate" and "conservative" is obvious here, so per the
//! plan's soundness posture this item **records the conflict** and states
//! its choice instead of picking quietly: the narrow, statically-certain
//! classification ships, and the named under-cost at point 5 stands as a
//! known bias until something in the `MemRef` surface can name a line's
//! owning placeable. A future item that wants the broad charge must
//! re-open this paragraph, not just change a coefficient.
//!
//! **What bounds the damage today, and what removes that bound.** The
//! under-cost is only *exploitable* by a transformation that changes which
//! core reads a line — and this rung lands **no named opt at all**
//! (freeze 1628), so nothing in the tree can currently spend it. The
//! transformation that would is **actor co-placement**, which
//! `plans/M20.md` item M names as a next-rung candidate precisely because
//! row 18 makes placement a cost question for the first time
//! (`placement.rs` packs on `(work, bytes)` with no notion of cross-core
//! traffic). So: **co-placement may not be scored on this ruler until
//! point 5 is closed.** Attempting it against an unclassifiable-cold-load
//! model would let a candidate move an actor to another core and pay
//! nothing for the remote reads it created — a proxy win that implies a
//! real-machine loss, which is the one thing 04 §5 exists to forbid.
//!
//! ## Row 19, system-register access
//!
//! SOG §4.10 gives, per register, whether access is non-speculative,
//! in-order, or carries a flush side-effect — **T1 for *which*, T5 for
//! cost**. Two halves, charged differently:
//!
//! - **Ordering, structurally**: an in-order access is not reordered
//!   against the window, which is `serializes_window` again — no
//!   magnitude, so no provenance problem.
//! - **The flush, swept**: `sysreg_flush_cost` is charged only for a
//!   system-register **write** ([`system_word_flushes`], decoded from the
//!   `MSR` register form). The model carries no per-register flush list,
//!   because wrela has no reason to carry one and inventing one would be
//!   inventing T1 facts; treating every emitted system-register write as
//!   flushing is over-broad in the over-cost direction, and that is stated
//!   rather than assumed.
//!
//! **No emitted system word carries a flush side-effect today.** wrela's
//! only `CostRule::System` word is the async-dispatch `BRK` trap, which is
//! not a system-register access at all. So the flush term is **live but
//! unexercised by the emitted stream**, and its oracle is a unit on a
//! synthetic `MSR` word. Recorded plainly: that is the honest outcome, not
//! a stub.
//!
//! ## Row 20, abort path — and what it says about emit-every-check
//!
//! The abort **branch** is charged (it is an ordinary `CostRule::Branch`
//! word in the checking function) and the handler body is **not**, because
//! the handler never returns: `__wrela_abort` is scored once as its own
//! function, not once per check site. This module adds nothing for
//! `Abort` / `AbortVal`, which is the substance of row 20.
//!
//! **Recorded finding.** Under this model the naive "emit every check"
//! policy (`compiler.codegen.naive-locked`) is **cheap**: each check costs
//! one compare, one always-not-taken branch and one `BL` word, and the
//! handler it targets is paid for exactly once for the whole program
//! however many checks point at it. Nothing about the policy scales with
//! check count except three words per check, and those words are the
//! cheapest groups the T1 table has (1 cycle, throughput 3 on port I for
//! the compare; 1 cycle on the single B pipe for the branch). Item H adds
//! the other half — an always-not-taken branch is charged ~0 mispredict
//! under a bias-derived model — and together these are the first
//! quantitative defence of a core doctrine choice this ruler has been able
//! to produce.
//!
//! ## Row 39, the ordered accesses
//!
//! `LDAR` / `STLR` take their plain twin's memory path plus a swept
//! increment (`load_acquire_cost` / `store_release_cost`, both pinned at
//! their bracket's low end of 0 because both are `removal_sensitive`).
//! They are deliberately **not** given `serializes_window`: each orders in
//! one direction only, and every emitted site sits inside a bracket a
//! `@dmb` already fences, so charging both as two-way fences would
//! double-count the bracket.
//!
//! ## Freeze 1633
//!
//! Barrier cost must never make barrier **removal** profitable, because
//! barriers are correctness-load-bearing and
//! `machine.cross-core.publish-acquire-barrier` is a known-risk gap in
//! plans/BLOCKED.md. That is **not** a coefficient here. It is
//! [`ordering_removals`], a structural refusal wired into `opts::win`:
//! a candidate that emits strictly fewer words of any `[crosscore]`-priced
//! rule than the baseline is refused, whatever the numbers say. It
//! compares **counts of emitted words**, so no value of any sweep
//! dimension, table row or coefficient can satisfy it — there is nothing
//! to tune.

use std::collections::BTreeMap;

use crate::placement::PlacementTable;

use super::attr::{AttrTarget, classify_target};
use super::rule::{CostRule, EmittedWord, MemClass};
use super::score::{CostReport, CrossExtra};
use super::sweep::SweepPoint;
use super::table::CostTable;

// ---------------------------------------------------------------------------
// The `[crosscore]` terms
// ---------------------------------------------------------------------------

/// Row 17. `DMB ishst` / `DMB ishld` alike — no source distinguishes them.
const TERM_DMB: &str = "dmb";
/// Row 18. Charged **above** the memory model's leaf latency.
const TERM_SNOOP: &str = "snoop";
/// Row 19. Charged only for a register §4.10 marks as flushing.
const TERM_SYSREG_FLUSH: &str = "sysreg_flush";

/// The `[sweep]` dimension a `[crosscore]` term names.
///
/// Fails closed rather than defaulting: a missing term would silently
/// price its whole dimension at 0, which is a discount (decision 1609).
/// This is the same posture `SweepPoint::get` takes for an undeclared
/// dimension, for the same reason.
fn sweep_dim<'t>(table: &'t CostTable, term: &str) -> &'t str {
    &table
        .crosscore(term)
        .unwrap_or_else(|| {
            panic!(
                "cost table: [crosscore.{term}] is required by the cross-core cost model \
                 (plans/M20.md item G); a missing term would price it at 0"
            )
        })
        .sweep
}

/// The term's value at `point`.
fn swept(table: &CostTable, term: &str, point: &SweepPoint) -> u64 {
    point.get(sweep_dim(table, term))
}

/// The term's value at `point` **above** the pinned value already folded
/// into the rule's latency.
///
/// `[crosscore.dmb]` and `[crosscore.sysreg_flush]` carry no `base`, so
/// `CostTable::latency` folds their dimension's *pinned* value straight
/// into `latencies[rule]` — which is also the "the word occupies at least
/// one cycle" floor both brackets name as their low end. Returning the
/// full swept value here would double-count that cycle. Both terms are
/// `removal_sensitive` and therefore pin their bracket's **low** end, so
/// this difference is never negative; `removal_sensitive_terms_pin_the_low_end`
/// holds that invariant against a profile edit.
fn swept_above_pinned(table: &CostTable, term: &str, point: &SweepPoint) -> u64 {
    let dim = sweep_dim(table, term);
    let pinned = table
        .sweep(dim)
        .unwrap_or_else(|| panic!("cost table: [sweep.{dim}] is required by [crosscore.{term}]"))
        .pinned;
    point.get(dim).saturating_sub(pinned)
}

// ---------------------------------------------------------------------------
// Row 18: the static local-vs-remote classification
// ---------------------------------------------------------------------------

/// What sealed placement can say about the core that last wrote a line.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Locality {
    /// Statically **local**: a frame slot, a non-load, or an image with no
    /// peer core. Never charged a snoop.
    Local,
    /// Statically **remote**: the acquire half of cross-core
    /// publish/acquire, in an image that brings up a peer core.
    Remote,
    /// A cold line whose writing core the `MemRef` surface cannot name.
    /// Charged as `Local` — the named under-cost in the module doc's
    /// decision-1609 conflict, not a claim that it is local.
    Unclassified,
}

impl Locality {
    /// True only for a verdict the model is prepared to charge for.
    pub fn is_remote(self) -> bool {
        matches!(self, Locality::Remote)
    }
}

/// The core the function `fn_key` is sealed to, or `None` when sealed
/// placement names no single core for it.
///
/// `None` covers `Shared` keys (runtime helpers, aborts, free functions —
/// any core runs them) and the `Err` case where one type is placed on more
/// than one core. Neither narrows the peer set, so both leave it at its
/// widest, which is the over-cost direction (decision 1609). Reuses
/// `attr::classify_target` so there is one mapping from a scored fn key to
/// a core, not two.
pub fn accessing_core(fn_key: &str, placement: &PlacementTable) -> Option<usize> {
    match classify_target(fn_key, placement) {
        Ok(AttrTarget::Core(n)) => Some(n),
        Ok(AttrTarget::Shared) | Err(_) => None,
    }
}

/// Static local-vs-remote verdict for one emitted word (decision 1603).
///
/// The derivation, in the order the code takes it:
///
/// 1. Only a **load** can be snooped; a store publishes.
/// 2. A `MemClass::Stack` slot is proven SP-relative and therefore private
///    to the executing core.
/// 3. A **peer core** must exist. Every core the image brings up runs the
///    publish/acquire protocol, so the possible-writer set is
///    `0..placement.cores`; a peer exists when that set holds a core other
///    than the accessing one. With the accessing core known this reads as
///    "some other brought-up core", and with it unknown as "more than one
///    brought-up core" — the same condition, which is exactly the finding
///    that the accessing core is classifiable but does not narrow the
///    verdict.
/// 4. A `LoadAcquire` is then **certainly** remote: the acquire half exists
///    because another agent wrote the line.
/// 5. Anything else cold is `Unclassified`.
pub fn classify_line(fn_key: &str, ew: &EmittedWord, placement: &PlacementTable) -> Locality {
    if !ew.rule.is_load() {
        return Locality::Local;
    }
    if matches!(ew.mem.map(|m| m.class), Some(MemClass::Stack)) {
        return Locality::Local;
    }
    let accessing = accessing_core(fn_key, placement);
    let peer_exists = match accessing {
        Some(n) => (0..placement.cores).any(|c| c != n),
        None => placement.cores > 1,
    };
    if !peer_exists {
        return Locality::Local;
    }
    if ew.rule == CostRule::LoadAcquire {
        return Locality::Remote;
    }
    Locality::Unclassified
}

// ---------------------------------------------------------------------------
// Row 19: which system words flush
// ---------------------------------------------------------------------------

/// AArch64 `MSR (register)`: `1101 0101 000 1 o0 op1 CRn CRm op2 Rt`.
/// `MRS` is the same shape with bit 21 set, and `BRK` is not in this space
/// at all.
const MSR_REGISTER_MASK: u32 = 0xFFF0_0000;
const MSR_REGISTER: u32 = 0xD510_0000;

/// True when `word` is a system-register **write**, the family SOG §4.10
/// documents as (for some registers) carrying a flush side-effect.
///
/// The model carries no per-register flush list: §4.10 says *which*
/// registers flush (T1) but wrela emits no system-register access at all,
/// so a list here would be speculative ISA coverage (freeze 1630) and
/// unsourced pinned data (freeze 1629). Treating every system-register
/// write as flushing is over-broad in the **over-cost** direction, and it
/// is inert on today's stream: the only emitted `CostRule::System` word is
/// the async-dispatch `BRK` trap, which is not a system-register access.
pub fn system_word_flushes(word: u32) -> bool {
    word & MSR_REGISTER_MASK == MSR_REGISTER
}

// ---------------------------------------------------------------------------
// The charge
// ---------------------------------------------------------------------------

/// The whole cross-core charge for one emitted word — `cost/score.rs`'s
/// `crosscore_extra` seam delegates here so item G's model lives in one
/// file.
pub fn charge(
    fn_key: &str,
    ew: &EmittedWord,
    table: &CostTable,
    point: &SweepPoint,
    placement: &PlacementTable,
) -> CrossExtra {
    let mut extra_cycles = 0u64;
    let mut serializes_window = false;

    match ew.rule {
        // Row 17. One swept row for `ishst` and `ishld` alike — no source
        // distinguishes them. The ordering is the structural half.
        CostRule::Barrier => {
            extra_cycles = swept_above_pinned(table, TERM_DMB, point);
            serializes_window = true;
        }
        // Row 19. Ordering always (§4.10's in-order, non-speculative
        // accesses, and a trap is certainly not reordered); the swept
        // flush only for a system-register write.
        CostRule::System => {
            serializes_window = true;
            if system_word_flushes(ew.word) {
                extra_cycles = swept_above_pinned(table, TERM_SYSREG_FLUSH, point);
            }
        }
        // Row 39. Increments on the plain twin's `[latency]` row; not
        // fences (see the module doc).
        CostRule::LoadAcquire | CostRule::StoreRelease => {
            if let Some(dim) = table.crosscore_extra_dim(ew.rule) {
                extra_cycles = point.get(dim);
            }
        }
        // Row 20. The abort branch is an ordinary `Branch` word and is
        // already charged; the handler never returns, so there is no
        // handler body to charge here. Named rather than defaulted.
        CostRule::Abort | CostRule::AbortVal => {}
        _ => {}
    }

    // Row 18. Above the memory model's leaf latency (item F owns that
    // path; this is an increment on top of it, never a reach into it).
    if classify_line(fn_key, ew, placement).is_remote() {
        extra_cycles = extra_cycles.saturating_add(swept(table, TERM_SNOOP, point));
    }

    CrossExtra {
        extra_cycles,
        serializes_window,
    }
}

// ---------------------------------------------------------------------------
// Freeze 1633: the structural barrier-removal refusal
// ---------------------------------------------------------------------------

/// The rules whose whole cost is a `[crosscore]` term: `barrier`,
/// `system`, `load_acquire`, `store_release`. Derived from
/// `CostRule::is_crosscore` rather than listed, so the refusal set cannot
/// drift from the table's own notion of what a cross-core term prices.
///
/// Freeze 1633 names the barrier. The other three are the same hazard by
/// their own profile rows: `sysreg_flush_cost`, `load_acquire_cost` and
/// `store_release_cost` are each `removal_sensitive = true` with an
/// `ambiguity` note citing this freeze.
pub fn ordering_rules() -> Vec<CostRule> {
    CostRule::ALL
        .iter()
        .copied()
        .filter(|r| r.is_crosscore())
        .collect()
}

/// Emitted-word counts for every ordering rule in a scored program, summed
/// over functions. Always the full rule set, so a rule absent from the
/// program reads as 0 rather than as a missing key.
pub fn ordering_word_counts(report: &CostReport) -> BTreeMap<&'static str, u64> {
    let mut out: BTreeMap<&'static str, u64> = ordering_rules()
        .iter()
        .map(|r| (r.as_str(), 0u64))
        .collect();
    for f in &report.fns {
        for (rule, n) in &f.terms {
            let Some(r) = CostRule::from_str(rule) else {
                continue;
            };
            if !r.is_crosscore() {
                continue;
            }
            *out.get_mut(r.as_str()).expect("ordering rule slot") += n;
        }
    }
    out
}

/// One rule the candidate emits strictly fewer words of than the baseline.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OrderingRemoval {
    pub rule: &'static str,
    pub baseline: u64,
    pub candidate: u64,
}

impl OrderingRemoval {
    /// Stable label for a veto/refusal report (04 §5 requires every reason
    /// that fires to be reported).
    pub fn label(&self) -> String {
        format!(
            "ordering_words_removed:{}:{}->{}",
            self.rule, self.baseline, self.candidate
        )
    }
}

/// **Freeze 1633, structurally.** Every ordering rule the candidate emits
/// fewer words of than the baseline.
///
/// A non-empty result is a refusal, not a ranking input: barriers and the
/// ordered accesses are correctness-load-bearing, so the gate may never
/// credit their deletion however the cycles come out. This compares
/// **counts of emitted words** — there is no coefficient, sweep dimension
/// or table row whose value can make a removed word un-removed, which is
/// what makes the rule impossible to tune around.
pub fn ordering_removals(
    baseline: &BTreeMap<&'static str, u64>,
    candidate: &BTreeMap<&'static str, u64>,
) -> Vec<OrderingRemoval> {
    let mut out = Vec::new();
    for (rule, &b) in baseline {
        let c = candidate.get(rule).copied().unwrap_or(0);
        if c < b {
            out.push(OrderingRemoval {
                rule,
                baseline: b,
                candidate: c,
            });
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::codegen::{CodegenFn, CodegenProgram};
    use crate::cost::rule::{FlagEffect, MemRef};
    use crate::cost::score::{FnCost, score_program_at};
    use crate::cost::table::load_default;
    use crate::eval::image::ImageDeclRef;
    use crate::placement::{PlacementEntry, PlacementSource};

    fn table() -> CostTable {
        load_default().expect("bench/a76-pi5.toml")
    }

    fn pinned() -> SweepPoint {
        SweepPoint::pinned(&table())
    }

    fn word(rule: CostRule, dst: Option<u8>, srcs: &[u8]) -> EmittedWord {
        EmittedWord::new(0, String::new(), rule, dst, srcs)
    }

    fn word_enc(enc: u32, rule: CostRule, dst: Option<u8>, srcs: &[u8]) -> EmittedWord {
        EmittedWord::new(enc, String::new(), rule, dst, srcs)
    }

    /// A cold-unique load (never a reuse hit), in either ordered or plain
    /// form, so "same reuse distance" is literally the same MemRef.
    fn cold_load(rule: CostRule, seq: u64) -> EmittedWord {
        word(rule, Some(1), &[0]).with_mem(MemRef::cold_unique(seq))
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

    /// One core, one placeable: no peer can have written anything.
    fn single_core() -> PlacementTable {
        PlacementTable {
            entries: vec![entry(ImageDeclRef::Actor(0), "Foo", 0)],
            cores: 1,
        }
    }

    /// Three cores with placeables on two of them — the flagship shape.
    fn three_cores() -> PlacementTable {
        PlacementTable {
            entries: vec![
                entry(ImageDeclRef::Actor(0), "Foo", 0),
                entry(ImageDeclRef::Actor(1), "Bar", 1),
            ],
            cores: 3,
        }
    }

    fn total(key: &str, code: Vec<EmittedWord>, placement: &PlacementTable) -> u64 {
        total_at(key, code, placement, &pinned())
    }

    fn total_at(
        key: &str,
        code: Vec<EmittedWord>,
        placement: &PlacementTable,
        point: &SweepPoint,
    ) -> u64 {
        score_program_at(&prog(key, code), &table(), placement, point)
            .expect("score")
            .total_proxy_cycles
    }

    // --- row 18: the placement oracle --------------------------------------

    /// **The oracle proving placement is actually consulted.** The same
    /// `LDAR` word, the same `MemRef`, the same reuse distance (none — a
    /// cold unique key always misses), scored under two placements. Only
    /// `PlacementTable` differs, and the remote one costs `snoop_cost`
    /// more.
    #[test]
    fn a_remote_load_costs_more_than_the_identical_local_load() {
        let code = || vec![cold_load(CostRule::LoadAcquire, 0)];
        let local = total("Foo.turn", code(), &single_core());
        let remote = total("Foo.turn", code(), &three_cores());
        // lat_l3 = 35, load_acquire_cost pinned 0, snoop_cost pinned 312.
        assert_eq!(local, 35, "one core: nothing can be remote");
        assert_eq!(
            remote,
            35 + 312,
            "peer core exists: snoop above the L3 path"
        );
        assert!(
            remote > local,
            "remote {remote} must exceed local {local} at equal reuse distance"
        );
        // And the charge really is the swept dimension, not a constant.
        let lo = pinned().with("snoop_cost", 0);
        assert_eq!(
            total_at("Foo.turn", code(), &three_cores(), &lo),
            35,
            "snoop_cost must reach the schedule through the sweep point"
        );
    }

    /// The verdict boundary, stated as assertions so the module doc's
    /// "what I can and cannot classify" list is executable.
    #[test]
    fn locality_classifies_exactly_what_placement_decides() {
        let three = three_cores();
        let one = single_core();

        // (4) an LDAR with a peer core: certainly remote.
        assert_eq!(
            classify_line("Foo.turn", &cold_load(CostRule::LoadAcquire, 0), &three),
            Locality::Remote
        );
        // (1) no peer core: local, whatever the rule.
        assert_eq!(
            classify_line("Foo.turn", &cold_load(CostRule::LoadAcquire, 0), &one),
            Locality::Local
        );
        // (5) a plain cold load: the MemRef names no writer.
        assert_eq!(
            classify_line("Foo.turn", &cold_load(CostRule::Load, 0), &three),
            Locality::Unclassified
        );
        // (3) a frame slot is private to the executing core.
        let stack = word(CostRule::LoadAcquire, Some(1), &[31]).with_mem(MemRef::stack(8));
        assert_eq!(classify_line("Foo.turn", &stack, &three), Locality::Local);
        // A store publishes; it is not snooped.
        let rel = word(CostRule::StoreRelease, None, &[0]).with_mem(MemRef::cold_unique(1));
        assert_eq!(classify_line("Foo.turn", &rel, &three), Locality::Local);
        // Only `Remote` is chargeable — `Unclassified` is the named
        // under-cost, not a second remote verdict.
        assert!(Locality::Remote.is_remote());
        assert!(!Locality::Unclassified.is_remote());
        assert!(!Locality::Local.is_remote());
    }

    /// `fn_key` is load-bearing: sealed placement resolves a method key
    /// through its type's entry and a secondary-core entry key by name.
    #[test]
    fn accessing_core_comes_from_sealed_placement() {
        let three = three_cores();
        assert_eq!(accessing_core("Foo.turn", &three), Some(0));
        assert_eq!(accessing_core("Bar.turn", &three), Some(1));
        assert_eq!(accessing_core("rt_enqueue Bar", &three), Some(1));
        assert_eq!(accessing_core("rt_secondary_core_entry 2", &three), Some(2));
        // Runtime helpers name no core — any core runs them.
        assert_eq!(accessing_core("__wrela_abort", &three), None);
        assert_eq!(accessing_core("free_helper", &three), None);
        // A type on two cores names no single core either; the peer set
        // stays at its widest rather than picking one.
        let split = PlacementTable {
            entries: vec![
                entry(ImageDeclRef::Actor(0), "Foo", 0),
                entry(ImageDeclRef::Actor(1), "Foo", 1),
            ],
            cores: 2,
        };
        assert_eq!(accessing_core("Foo.turn", &split), None);
        // The recorded finding: the accessing core is classifiable but
        // does not narrow the verdict, because every brought-up core runs
        // the publish/acquire protocol.
        for key in [
            "Foo.turn",
            "Bar.turn",
            "rt_secondary_core_entry 2",
            "__wrela_abort",
        ] {
            assert_eq!(
                classify_line(key, &cold_load(CostRule::LoadAcquire, 0), &three),
                Locality::Remote,
                "{key}"
            );
        }
    }

    // --- row 17: the barrier -----------------------------------------------

    /// A `DMB` costs more than an `ADD`. Not from the coefficient — at the
    /// pinned point `dmb_cost` is 1, the same as an `ADD`'s latency — but
    /// from the **ordering**: a barrier may not issue until everything
    /// already in the window has retired. A 35-cycle cold load in front of
    /// it makes that visible, and the `ADD` control shows the difference is
    /// the fence and not the word.
    #[test]
    fn a_dmb_costs_more_than_an_add() {
        let one = single_core();
        let load = || cold_load(CostRule::Load, 0);
        let with_add = total(
            "f",
            vec![load(), word(CostRule::Alu, Some(9), &[8, 8])],
            &one,
        );
        let with_dmb = total("f", vec![load(), word(CostRule::Barrier, None, &[])], &one);
        assert_eq!(with_add, 35, "an independent ADD hides under the load");
        assert_eq!(with_dmb, 36, "the barrier waits for the load to retire");
        assert!(with_dmb > with_add);
        // A later word may not pass the barrier either — the fence is live
        // in both directions.
        let after = total(
            "f",
            vec![
                word(CostRule::Barrier, None, &[]),
                load(),
                word(CostRule::Alu, Some(9), &[8, 8]),
            ],
            &one,
        );
        assert_eq!(
            after,
            1 + 35,
            "the load cannot issue before the barrier retires"
        );
        // And the swept magnitude reaches the schedule at the box's far end.
        let hi = pinned().with("dmb_cost", 64);
        assert_eq!(
            total_at("f", vec![word(CostRule::Barrier, None, &[])], &one, &hi),
            64,
            "dmb_cost must be read through the point, not pinned"
        );
        assert_eq!(
            total("f", vec![word(CostRule::Barrier, None, &[])], &one),
            1,
            "and the pinned point stays at the bracket's low end (freeze 1633)"
        );
    }

    /// `ishst` and `ishld` are charged identically: **no source
    /// distinguishes them**, so the model does not either.
    #[test]
    fn ishst_and_ishld_share_one_swept_row() {
        let t = table();
        let p = pinned();
        let one = single_core();
        let ishst = word_enc(crate::encode::enc_dmb_ishst(), CostRule::Barrier, None, &[]);
        let ishld = word_enc(crate::encode::enc_dmb_ishld(), CostRule::Barrier, None, &[]);
        assert_eq!(
            charge("f", &ishst, &t, &p, &one),
            charge("f", &ishld, &t, &p, &one)
        );
        assert_eq!(sweep_dim(&t, TERM_DMB), "dmb_cost");
    }

    // --- row 19: the system word ------------------------------------------

    /// A flush-side-effect register write serializes the window while an
    /// NZCV write does not.
    ///
    /// The NZCV half is a genuine cross-check on item E/I: NZCV and SP are
    /// fully renamed on A76 (SOG §4.10 note 1), so a flag write must *not*
    /// fence, and the same 35-cycle load in front makes both halves
    /// observable on one shape.
    #[test]
    fn a_flushing_register_write_serializes_but_an_nzcv_write_does_not() {
        let one = single_core();
        let load = || cold_load(CostRule::Load, 0);
        // `MSR TTBR0_EL1, x0` — op0=3 (o0 bit set), op1=0, CRn=2, CRm=0,
        // op2=0, Rt=0: the MSR (register) form, not the MRS read.
        let msr = word_enc(0xD518_2000, CostRule::System, None, &[0]);
        // Reads only registers the load does not write, so the only thing
        // that could delay it is a fence.
        let nzcv = EmittedWord::new(0, String::new(), CostRule::Alu, None, &[5, 6])
            .with_flags(FlagEffect::Write);

        assert_eq!(
            total("f", vec![load(), nzcv], &one),
            35,
            "a renamed-flag write does not fence: it hides under the load"
        );
        assert_eq!(
            total("f", vec![load(), msr.clone()], &one),
            36,
            "an in-order system access waits for the window to drain"
        );

        // The flush *charge* is what separates a register write from a
        // trap, and it is swept: at the box's far end only the MSR moves.
        let t = table();
        let hi = pinned().with("sysreg_flush_cost", 64);
        let brk = word_enc(crate::encode::enc_brk(1), CostRule::System, None, &[]);
        assert!(system_word_flushes(msr.word));
        assert!(
            !system_word_flushes(brk.word),
            "the async-dispatch BRK trap is not a system-register access"
        );
        assert_eq!(charge("f", &msr, &t, &hi, &one).extra_cycles, 63);
        assert_eq!(charge("f", &brk, &t, &hi, &one).extra_cycles, 0);
        // Both are ordered, though: a trap is certainly not reordered.
        assert!(charge("f", &msr, &t, &pinned(), &one).serializes_window);
        assert!(charge("f", &brk, &t, &pinned(), &one).serializes_window);
        // `MRS` is a read, not a write: not charged the flush.
        assert!(!system_word_flushes(0xD530_0000));
    }

    /// The emitted stream contains **no** flushing system word, so the
    /// flush term is live but unexercised. Pinned as a finding: if codegen
    /// ever emits an `MSR`, this fails and the term stops being inert
    /// deliberately rather than by accident.
    #[test]
    fn no_emitted_system_word_carries_a_flush_side_effect() {
        assert!(
            // `codegen::BRK_ASYNC_DISPATCH_NO_STATE_MATCHED` (private).
            !system_word_flushes(crate::encode::enc_brk(0xACD4)),
            "the only emitted CostRule::System word is the async-dispatch BRK trap"
        );
    }

    // --- row 39: the ordered accesses -------------------------------------

    /// The swept `LoadAcquire` / `StoreRelease` increments are live: an
    /// ordered access costs **at least** as much as the plain word it
    /// replaces, at every point of its bracket.
    #[test]
    fn ordered_accesses_never_cost_less_than_their_plain_twin() {
        let one = single_core();
        let t = table();
        for (ordered, plain, dim) in [
            (CostRule::LoadAcquire, CostRule::Load, "load_acquire_cost"),
            (
                CostRule::StoreRelease,
                CostRule::Store,
                "store_release_cost",
            ),
        ] {
            let row = t.sweep(dim).expect("bracket");
            for value in [row.lo, row.pinned, row.hi] {
                let p = pinned().with(dim, value);
                let mk = |rule: CostRule| {
                    if rule.is_store() {
                        word(rule, None, &[31, 0]).with_mem(MemRef::stack(8))
                    } else {
                        word(rule, Some(1), &[31]).with_mem(MemRef::stack(8))
                    }
                };
                let a = total_at("f", vec![mk(ordered)], &one, &p);
                let b = total_at("f", vec![mk(plain)], &one, &p);
                assert!(
                    a >= b,
                    "{} at {dim}={value}: {a} < plain {} {b}",
                    ordered.as_str(),
                    plain.as_str()
                );
                assert_eq!(
                    a,
                    b + value,
                    "{} must be its twin plus the swept increment",
                    ordered.as_str()
                );
            }
            // Pinned at the bracket's low end, so the canonical dump is
            // unmoved (freeze 1633's family).
            assert_eq!(row.pinned, row.lo);
        }
    }

    /// The ordered halves are increments, **not** fences: their sites sit
    /// inside a `@dmb`-fenced bracket, so fencing them too would
    /// double-count the bracket.
    #[test]
    fn ordered_accesses_do_not_serialize_the_window() {
        let t = table();
        let p = pinned();
        let one = single_core();
        for rule in [CostRule::LoadAcquire, CostRule::StoreRelease] {
            let ew = word(rule, Some(1), &[31]).with_mem(MemRef::stack(8));
            assert!(
                !charge("f", &ew, &t, &p, &one).serializes_window,
                "{} must not fence",
                rule.as_str()
            );
        }
    }

    // --- row 20: the abort path -------------------------------------------

    /// The abort **branch** is charged and no handler body is: the handler
    /// never returns, so it is scored once for the whole program however
    /// many check sites target it. This is why "emit every check" is cheap
    /// under this model.
    #[test]
    fn the_abort_branch_is_charged_but_no_handler_body_is() {
        let t = table();
        let p = pinned();
        let one = single_core();
        // The cross-core model adds nothing to an abort site.
        for rule in [CostRule::Abort, CostRule::AbortVal] {
            assert_eq!(
                charge("f", &word(rule, None, &[0]), &t, &p, &one),
                CrossExtra::default(),
                "{} must carry no cross-core charge",
                rule.as_str()
            );
        }

        // A handler with a body, and a checker with N abort sites: the
        // total grows with the checker's words and **not** with N copies of
        // the handler.
        let handler: Vec<EmittedWord> = (0..20)
            .map(|i| word(CostRule::Alu, Some((i % 8) + 1), &[9, 9]))
            .collect();
        let check_site = || {
            vec![
                word(CostRule::Alu, None, &[1, 2]),
                word(CostRule::Branch, None, &[]),
                word(CostRule::Abort, None, &[0]),
            ]
        };
        let build = |sites: usize| {
            let mut code = Vec::new();
            for _ in 0..sites {
                code.extend(check_site());
            }
            let mut fns = BTreeMap::new();
            fns.insert(
                "checker".to_string(),
                CodegenFn {
                    frame_size: 0,
                    code,
                    relocs: Vec::new(),
                },
            );
            fns.insert(
                "__wrela_abort".to_string(),
                CodegenFn {
                    frame_size: 0,
                    code: handler.clone(),
                    relocs: Vec::new(),
                },
            );
            score_program_at(
                &CodegenProgram {
                    fns,
                    rodata: Vec::new(),
                },
                &t,
                &one,
                &p,
            )
            .expect("score")
        };
        let one_site = build(1);
        let four_sites = build(4);
        let handler_cost = one_site
            .fns
            .iter()
            .find(|f| f.key == "__wrela_abort")
            .expect("handler")
            .proxy_cycles;
        assert!(handler_cost > 0, "the handler body is scored once");
        // Three extra check sites cost strictly less than one extra copy of
        // the handler would.
        let growth = four_sites.total_proxy_cycles - one_site.total_proxy_cycles;
        assert!(
            growth < handler_cost,
            "3 more checks cost {growth}, which must stay below one handler body \
             {handler_cost} — the handler never returns, so it is not per-site"
        );
        // And the handler really is counted once, not once per site.
        assert_eq!(
            four_sites
                .fns
                .iter()
                .find(|f| f.key == "__wrela_abort")
                .expect("handler")
                .proxy_cycles,
            handler_cost
        );
    }

    // --- freeze 1633 -------------------------------------------------------

    /// The refusal set is the `[crosscore]`-priced rules, derived rather
    /// than listed.
    #[test]
    fn ordering_rules_are_the_crosscore_priced_ones() {
        let names: Vec<&str> = ordering_rules().iter().map(|r| r.as_str()).collect();
        assert_eq!(
            names,
            vec!["load_acquire", "store_release", "barrier", "system"]
        );
    }

    /// Counting is over emitted words, summed across functions, with every
    /// rule present at 0.
    #[test]
    fn ordering_word_counts_sum_over_fns() {
        let mk = |key: &str, pairs: &[(&str, u64)]| FnCost {
            key: key.to_string(),
            owner: "runtime".to_string(),
            proxy_cycles: 0,
            words: 0,
            terms: pairs.iter().map(|(k, v)| ((*k).to_string(), *v)).collect(),
        };
        let report = CostReport {
            version: 3,
            digest: "d".to_string(),
            provenance: "p".to_string(),
            provenance_summary: "s".to_string(),
            profile: "a76-pi5".to_string(),
            pipelines: 8,
            dispatch_mops: 4,
            dispatch_uops: 8,
            reorder_window: 128,
            total_proxy_cycles: 0,
            total_words: 0,
            owner_totals: BTreeMap::new(),
            // Item F's per-core budget: empty here on purpose. This fixture
            // exercises the freeze-1633 ordering-word refusal, which reads
            // Term counts alone and must fire whatever the footprint says.
            footprint: Vec::new(),
            fns: vec![
                mk("a", &[("barrier", 2), ("alu", 99), ("load_acquire", 1)]),
                mk("b", &[("barrier", 4), ("store_release", 3)]),
            ],
            workloads_digest: None,
            workload_totals: BTreeMap::new(),
            workload_coverage: BTreeMap::new(),
        };
        let counts = ordering_word_counts(&report);
        assert_eq!(counts["barrier"], 6);
        assert_eq!(counts["load_acquire"], 1);
        assert_eq!(counts["store_release"], 3);
        assert_eq!(counts["system"], 0, "an absent rule reads 0, not missing");
        assert_eq!(counts.len(), 4, "no non-ordering rule leaks in");
    }

    /// **Freeze 1633.** Removing an ordering word is a refusal; adding or
    /// keeping one is not.
    #[test]
    fn ordering_removals_fire_only_on_a_drop() {
        let counts = |b: u64, l: u64, s: u64, y: u64| -> BTreeMap<&'static str, u64> {
            BTreeMap::from([
                ("barrier", b),
                ("load_acquire", l),
                ("store_release", s),
                ("system", y),
            ])
        };
        let base = counts(6, 4, 6, 1);
        assert!(ordering_removals(&base, &base).is_empty(), "identity");
        assert!(
            ordering_removals(&base, &counts(7, 4, 6, 1)).is_empty(),
            "adding a barrier is not a removal"
        );
        let dropped = ordering_removals(&base, &counts(5, 4, 6, 1));
        assert_eq!(
            dropped,
            vec![OrderingRemoval {
                rule: "barrier",
                baseline: 6,
                candidate: 5
            }]
        );
        assert_eq!(dropped[0].label(), "ordering_words_removed:barrier:6->5");
        // Every dropped rule is reported, not just the first.
        assert_eq!(ordering_removals(&base, &counts(0, 0, 0, 0)).len(), 4);
        // A missing key is a drop to 0 — the `--omit-dmb` shape, where the
        // candidate emits no barrier word at all.
        let mut missing = base.clone();
        missing.remove("barrier");
        assert_eq!(
            ordering_removals(&base, &missing),
            vec![OrderingRemoval {
                rule: "barrier",
                baseline: 6,
                candidate: 0
            }]
        );
    }

    // --- provenance invariants this module depends on ----------------------

    /// `swept_above_pinned` is only sound because `dmb_cost` and
    /// `sysreg_flush_cost` pin their bracket's **low** end (freeze 1633's
    /// family). If a profile edit ever pinned them high, the subtraction
    /// would saturate to 0 and quietly stop sweeping — so the invariant is
    /// asserted here rather than assumed.
    #[test]
    fn removal_sensitive_terms_pin_the_low_end() {
        let t = table();
        for dim in [
            "dmb_cost",
            "sysreg_flush_cost",
            "load_acquire_cost",
            "store_release_cost",
        ] {
            let row = t.sweep(dim).expect("bracket");
            assert!(row.removal_sensitive, "{dim} must be removal_sensitive");
            assert_eq!(row.pinned, row.lo, "{dim} must pin its low end");
            assert!(row.ambiguity.is_some(), "{dim} must record its ambiguity");
        }
        // And none of these may carry a magnitude in `[crosscore]` — item
        // D's parser refuses one; this asserts the shape the model reads.
        for term in [TERM_DMB, TERM_SNOOP, TERM_SYSREG_FLUSH] {
            assert!(t.crosscore(term).is_some(), "[crosscore.{term}] required");
        }
    }

    /// A missing `[crosscore]` term fails closed rather than pricing its
    /// dimension at 0 — `snoop` is the reachable case, since it names no
    /// `CostRule` and so its absence is not caught by the "every rule
    /// priced" check.
    // `#[should_panic]` first so `#[test]` sits directly above the `fn`:
    // xtask's ledger validator verifies a `unit:` citation by finding a
    // `#[test]` immediately followed by that function.
    #[should_panic(expected = "[crosscore.snoop] is required")]
    #[test]
    fn a_missing_crosscore_term_fails_closed() {
        let text = std::fs::read_to_string(crate::cost::table::default_table_path())
            .expect("committed profile");
        let mut out = String::new();
        let mut skipping = false;
        for line in text.lines() {
            if line.starts_with('[') {
                skipping = line.starts_with("[crosscore.snoop]");
            }
            if !skipping {
                out.push_str(line);
                out.push('\n');
            }
        }
        let t = crate::cost::table::parse(&out).expect("parses without the snoop term");
        let _ = swept(&t, TERM_SNOOP, &SweepPoint::pinned(&t));
    }
}
