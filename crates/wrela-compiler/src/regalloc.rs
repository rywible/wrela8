//! Per-function linear-scan register allocation
//! (plans/codegen-pareto.md item E, decision 1701 / freeze 1712).
//!
//! Before this module, every MWIR temp round-tripped through the frame:
//! a `str` after every definition and an `ldr` before every use, on every
//! path, forever. This module decides, per function, which temps may
//! instead live in a physical register for their whole live range. A
//! **resident** temp gets no frame slot at all; `codegen.rs` turns its
//! `load_slot`/`store_slot` into a register move, so each use loses a
//! store, a load, the store's V-pipe data uop and both accesses' AGU
//! uops. That win is structural — it is the *absence* of two memory
//! accesses, not a bet on store-to-load-forwarding latency, which is a
//! swept dimension of the residual box.
//!
//! ## Everything here is derived from the emitter, nothing is asserted
//!
//! The one way a register allocator goes silently wrong is by believing
//! a claim about the program that the code generator does not honour.
//! So this module is handed **facts measured from a real emission pass**
//! ([`FnFacts`], built by `codegen::probe_fn_facts`) rather than a
//! hand-written classification of MWIR instruction shapes that could
//! drift from `emit_one`:
//!
//! - **Which temps are reachable only as whole 8-byte scalars.** The
//!   probe records every `load_slot`/`store_slot`/`addr_of_slot` the
//!   emitter performs against the baseline frame. A temp whose bytes are
//!   ever reached by an address-taking `add` ([`Touch::Escape`]), at an
//!   interior offset, or with a slot size other than 8 is refused. This
//!   is where 04-compiler.md §5's "every backend fact (aliasing, ranges,
//!   alignment) must trace to a semantic proof — never be invented from
//!   naming or optimism" is honoured: the fact is *observed*, per
//!   function, not inferred.
//! - **Where a call can clobber a register.** The probe marks a program
//!   point that emits a `CostRule::Call` word. `CostRule::Abort` /
//!   `AbortVal` are noreturn, so they are deliberately not barriers.
//! - **Which registers this function's own emission already uses.** The
//!   probe unions every emitted word's `dst` and `srcs`. The pool is
//!   intersected with the complement, so an emitter scratch register or
//!   an incoming argument register can never be handed to a temp even if
//!   [`POOL`] were widened by mistake.
//!
//! ## Live ranges without alias analysis (decision 1701)
//!
//! A temp's live range is the closed interval between the first and last
//! program point that touches it, widened to a fixpoint over back edges
//! (below). Nothing else is needed, and in particular **no alias
//! analysis is needed**, because wrela's access modes make a scalar
//! temp's storage unreachable except through the temp itself:
//! 02-language.md §3 gives `read` (a loan that "may coexist with other
//! reads"), `mut` ("exclusive for its duration"; "while `mut x` is
//! active, no other path may touch the same storage; exclusivity is
//! checked on storage paths (fields, indexes, and potential overlaps),
//! not variable names") and `take` (the source becomes uninitialized).
//! A `mut` binding is the only way to obtain a writable second path to a
//! value, and it is exclusive by construction. The emitter's consequence
//! is exactly what the probe checks: a scalar temp that is never
//! address-taken has no second path to its bytes, so its interval is
//! exactly its touches. The probe is the proof; the language rule is why
//! the proof succeeds on real programs instead of refusing everything.
//!
//! ## Back edges, and why an interval is enough
//!
//! Intervals are linear but control flow is not, so the scan would be
//! unsound if two temps could be simultaneously live while their
//! intervals were disjoint. That can only happen when a use precedes its
//! own def in emission order, which requires a back edge. So every back
//! edge `(b -> h)`, `h <= b`, widens **every** interval it intersects to
//! cover all of `[h, b]`, iterated to a fixpoint (a widened interval can
//! reach a further back edge). After the fixpoint, two temps that are
//! ever simultaneously live have intersecting intervals:
//!
//! > Suppose `t` is live at point `p` with `p` outside `[a_t, b_t]`, say
//! > `p > b_t`. Liveness at `p` means some path runs def(`t`) -> `p` ->
//! > use(`t`) with both def and use inside `[a_t, b_t]`, so that path
//! > leaves `p` and returns to an index `<= b_t < p`: it contains a back
//! > edge `(b, h)` with `h <= b_t` and `b >= p`. That edge intersects
//! > `[a_t, b_t]` at `b_t`, so the fixpoint has already widened `t` to
//! > cover `[h, b]`, which contains `p`. Contradiction. The mirror
//! > argument covers `p < a_t`.
//!
//! ## The scan
//!
//! Intervals are sorted by start (ties by temp number — determinism
//! through dumbness, CLAUDE.md). The active set is kept sorted by end.
//! When the pool is empty the **furthest end** loses: if the incoming
//! interval ends later than every active one it simply stays in memory,
//! otherwise the latest-ending active temp is evicted back to memory and
//! yields its register. Eviction is free and always correct because
//! nothing has been emitted yet — "spilled" here just means "keeps the
//! frame slot it would have had anyway", which is exactly the
//! spill-everything behaviour `dev` keeps permanently as the correctness
//! reference (M19 freeze 1407).
//!
//! ## The pool, and why it is nine registers and not twenty-eight
//!
//! [`POOL`] is `x19..=x27`. The machine has ~28 usable GPRs, but
//! `codegen.rs` hardcodes most of them: `x0..x8` are the
//! argument/result registers, `x9..x14` are its scratch set
//! (`codegen.rs`'s `X_A..X_F`), `x15`/`x16` are `emit_format_scalar`'s
//! digit-loop pair (`X_I_REG`/`X_N_REG`), `x15`/`x16`/`x17` are
//! `emit_group_create`'s (`X_ARENA`/`X_CAND`/`X_TAG`), `x18` is the
//! platform register, `x28` is `X_FRAME` (the async turn base, reloaded
//! after every `BL` precisely because callees clobber it), and
//! `x29`/`x30`/`sp` are fixed. The stub emitters in `layout/harness.rs`
//! stay inside `x9..x11`, and the glue emitters in `codegen.rs` inside
//! `x0..x9` + `x30`. That leaves `x19..=x27` as the set no part of a
//! compiled image names — chosen the same way `X_FRAME`'s own doc
//! comment chose x28, and cross-checked per function by the probe's
//! `regs` union.
//!
//! Reaching the full ~28 means making each emission site
//! allocation-aware so the emitter's scratch registers stop being
//! reserved. That is a strictly larger change and it belongs with item
//! F, which rewrites the convention anyway.
//!
//! A temp is never left resident across a call, so a callee may clobber
//! every one of these nine freely: no callee-saved discipline is created
//! here and none is required (`codegen.rs`'s module doc — "the only
//! register that must survive a `BL` ... is the return address itself").
//! Item F removes the barrier by computing what each callee actually
//! clobbers.
//!
//! Nothing preempts a running function behind its back: 06-machine.md §4
//! gives this machine no emulated GIC and no exception vector table, and
//! "the guest observes vectors **only at checkpoints and parks**". A
//! checkpoint is an inline poll plus a `BL`, i.e. an ordinary
//! `CostRule::Call` word, so it is already a barrier here.

use std::cell::Cell;
use std::collections::{BTreeMap, BTreeSet};

/// The physical registers a resident temp may occupy. See the module
/// doc for why this set and not a larger one.
pub const POOL: &[u8] = &[19, 20, 21, 22, 23, 24, 25, 26, 27];

// Decision 1760: the allocator is a named opt with a TLS knob, exactly
// like `codegen::set_narrow_imm` (M19 freeze 1402 — `RELEASE_OPTS` is
// edited, nothing else is). Default **off**, so `dev` keeps
// spill-everything as the permanent correctness reference (M19 freeze
// 1407) and every caller that has not opted in is unaffected.
thread_local! {
    static REGALLOC: Cell<bool> = const { Cell::new(false) };
}

/// Enable/disable per-function register allocation for the current
/// thread (`opts::apply_opts`).
pub fn set_regalloc(enabled: bool) {
    REGALLOC.with(|c| c.set(enabled));
}

/// Whether register allocation is enabled (default false).
pub fn regalloc() -> bool {
    REGALLOC.with(|c| c.get())
}

/// How the emitter reached a temp's frame bytes at one program point.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub enum Touch {
    /// A whole-slot `load_slot` at the temp's own base offset.
    Read,
    /// A whole-slot `store_slot` at the temp's own base offset.
    Write,
    /// Anything else that reaches those bytes: an `addr_of_slot` (the
    /// address escaped into a register), an interior word of a
    /// multi-word aggregate, or a base access on a temp whose slot size
    /// is not exactly 8. Disqualifies the temp outright.
    Escape,
}

/// What one program point does. Point `0` is the prologue, points
/// `1..=body.len()` are `body[i - 1]`, and point `body.len() + 1` is the
/// epilogue — so the parameter stores the prologue emits and the
/// write-backs the epilogue emits are ordinary touches, not special
/// cases.
#[derive(Clone, Debug, Default)]
pub struct PointFacts {
    /// `(temp index, touch, word index within this point)`, in emission
    /// order. The word index is what makes the call barrier precise
    /// enough to be worth having — see [`allocate`]'s step 4.
    pub touches: Vec<(usize, Touch, usize)>,
    /// Word indices, within this point, of every `CostRule::Call` word:
    /// a call that returns, and therefore a point at which every pool
    /// register may have been clobbered by the callee.
    /// `CostRule::Abort`/`AbortVal` are noreturn and are deliberately
    /// not barriers.
    pub call_words: Vec<usize>,
    /// Every register this point's baseline emission names as a `dst` or
    /// a `src`. The pool is intersected with the complement of the union
    /// over all points.
    pub regs: BTreeSet<u8>,
}

/// Everything the allocator is told about one function. Every field is
/// measured by a real emission pass; none of it is asserted.
#[derive(Clone, Debug, Default)]
pub struct FnFacts {
    pub temp_count: usize,
    pub points: Vec<PointFacts>,
    /// `(source point, target point)` for every branch whose target is
    /// at or before its own index — see the module doc's proof.
    pub back_edges: Vec<(usize, usize)>,
}

/// The decision: one optional physical register per temp.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Assignment {
    reg: Vec<Option<u8>>,
}

impl Assignment {
    /// Spill everything — the `dev` reference model, and what every
    /// caller that has not opted in gets.
    pub fn none(temp_count: usize) -> Assignment {
        Assignment {
            reg: vec![None; temp_count],
        }
    }

    /// The physical register temp `t` lives in, if any.
    pub fn of(&self, t: usize) -> Option<u8> {
        self.reg.get(t).copied().flatten()
    }

    /// No temp is resident: the frame is byte-for-byte the naive one.
    pub fn is_empty(&self) -> bool {
        self.reg.iter().all(|r| r.is_none())
    }

    /// How many temps are resident (reported by the unit oracles).
    pub fn resident_count(&self) -> usize {
        self.reg.iter().filter(|r| r.is_some()).count()
    }

    /// Resident temps and their registers, ascending by temp — a stable
    /// surface for tests.
    pub fn residents(&self) -> Vec<(usize, u8)> {
        self.reg
            .iter()
            .enumerate()
            .filter_map(|(t, r)| r.map(|p| (t, p)))
            .collect()
    }
}

/// A temp's closed live interval over program points.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Interval {
    temp: usize,
    start: usize,
    end: usize,
}

/// Decide residency for one function.
///
/// `scalar_slot[t]` says the temp occupies exactly one 8-byte frame
/// slot; anything wider is refused before the scan even looks at it.
pub fn allocate(facts: &FnFacts, scalar_slot: &[bool]) -> Assignment {
    let n = facts.temp_count;
    let mut out = Assignment::none(n);
    if n == 0 || facts.points.is_empty() {
        return out;
    }

    // --- 1. eligibility: whole-slot scalars, never address-taken ------
    let mut eligible: Vec<bool> = (0..n)
        .map(|t| scalar_slot.get(t).copied().unwrap_or(false))
        .collect();
    for p in &facts.points {
        for &(t, touch, _) in &p.touches {
            if t < n && touch == Touch::Escape {
                eligible[t] = false;
            }
        }
    }

    // --- 1b. a register has to pay for itself -------------------------
    //
    // Residency rewrites each access one-for-one: a `store_slot` becomes
    // `mov home, reg` and a `load_slot` becomes `mov reg, home`. So a
    // temp written once and read once is not a residency at all, it is a
    // **copy** — the value is moved into its home register and moved
    // straight back out, and what used to be an independent `str`/`ldr`
    // pair (linked only through memory, and free to slack around the
    // code between them) becomes a strictly serial two-`mov` chain.
    //
    // Measured, not assumed (plans/codegen-pareto-E.md, decision 1765).
    // `cost-calls` is a program made entirely of such copies, and it is
    // the one case in the corpus the ∀ gate caught rising: **+2 at every
    // `store_to_load_forwarding=1` corner**, against −13 at every
    // `=4` corner. That is exactly the shape the plan warned this item
    // must not have — a win that depends on the swept forwarding
    // latency. Refusing single-read temps removes the dependence
    // instead of arguing with the ruler: what is left is only the temps
    // whose reloads are genuinely eliminated, and those fall at every
    // point of the box.
    //
    // The rule is about **reads**, not touches, because reads are what a
    // register saves. One write and two reads deletes a reload; one
    // write and one read deletes nothing.
    let mut reads: Vec<usize> = vec![0; n];
    for p in &facts.points {
        for &(t, touch, _) in &p.touches {
            if t < n && touch == Touch::Read {
                reads[t] += 1;
            }
        }
    }
    for t in 0..n {
        if reads[t] < 2 {
            eligible[t] = false;
        }
    }

    // --- 2. intervals from first touch to last touch ------------------
    let mut first: Vec<Option<usize>> = vec![None; n];
    let mut last: Vec<Option<usize>> = vec![None; n];
    for (i, p) in facts.points.iter().enumerate() {
        for &(t, _, _) in &p.touches {
            if t >= n {
                continue;
            }
            if first[t].is_none() {
                first[t] = Some(i);
            }
            last[t] = Some(i);
        }
    }
    let mut intervals: Vec<Interval> = (0..n)
        .filter(|&t| eligible[t])
        .filter_map(|t| match (first[t], last[t]) {
            (Some(a), Some(b)) => Some(Interval {
                temp: t,
                start: a,
                end: b,
            }),
            _ => None,
        })
        .collect();
    if intervals.is_empty() {
        return out;
    }

    // --- 3. widen over back edges, to a fixpoint ----------------------
    // Each round either changes nothing (done) or strictly grows at
    // least one interval, and an interval can only grow to the whole
    // point range, so the loop is bounded by `intervals * points`. The
    // explicit cap is a fail-closed guard, never a silent truncation:
    // reaching it would mean the monotonicity argument is wrong, and the
    // allocator then keeps the widening it has already applied, which is
    // strictly more conservative than the truth, not less.
    let cap = intervals.len().saturating_mul(facts.points.len()) + 2;
    let mut rounds = 0usize;
    loop {
        let mut changed = false;
        for &(b, h) in &facts.back_edges {
            let (lo, hi) = (h.min(b), h.max(b));
            for iv in intervals.iter_mut() {
                if iv.start <= hi && iv.end >= lo {
                    if iv.start > lo {
                        iv.start = lo;
                        changed = true;
                    }
                    if iv.end < hi {
                        iv.end = hi;
                        changed = true;
                    }
                }
            }
        }
        rounds += 1;
        if !changed || rounds > cap {
            break;
        }
    }

    // --- 4. a temp live across a returning call is refused ------------
    //
    // No callee-saved discipline exists in this ABI: only x30 survives a
    // `BL` (`codegen.rs`'s module doc), so a resident value cannot. Item
    // F is exactly the item that computes what a callee really clobbers
    // and lifts this.
    //
    // The test is at **word** grain, not point grain, and that is what
    // makes it worth having. A point-grain rule would refuse every call
    // argument and every call result, because `Inst::Call` emits its
    // argument loads, its `bl`, and the store of its result all inside
    // one point — so the two most common temps in call-heavy code would
    // never be resident. At word grain the question is the right one:
    // over which words of this point does the value have to be in a
    // register at all? A temp that is only read *before* the `bl` and a
    // temp that is only written *after* it are both fine; only a value
    // that must span the `bl` is refused.
    intervals.retain(|iv| {
        for i in iv.start..=iv.end.min(facts.points.len().saturating_sub(1)) {
            let p = &facts.points[i];
            if p.call_words.is_empty() {
                continue;
            }
            let mut lo_w: Option<usize> = None;
            let mut hi_w: Option<usize> = None;
            for &(t, _, w) in &p.touches {
                if t != iv.temp {
                    continue;
                }
                lo_w = Some(lo_w.map_or(w, |c: usize| c.min(w)));
                hi_w = Some(hi_w.map_or(w, |c: usize| c.max(w)));
            }
            // Live from before this point => the value already occupies
            // its register at word 0; live past it => it must still
            // occupy it at the last word.
            let lo = if iv.start < i {
                0
            } else {
                lo_w.unwrap_or(usize::MAX)
            };
            let hi = if iv.end > i {
                usize::MAX
            } else {
                hi_w.unwrap_or(0)
            };
            if lo > hi {
                // Not live anywhere inside this point.
                continue;
            }
            if p.call_words.iter().any(|&c| c >= lo && c <= hi) {
                return false;
            }
        }
        true
    });
    if intervals.is_empty() {
        return out;
    }

    // --- 5. the pool this function may actually use -------------------
    let mut used_regs: BTreeSet<u8> = BTreeSet::new();
    for p in &facts.points {
        used_regs.extend(p.regs.iter().copied());
    }
    let mut free: BTreeSet<u8> = POOL
        .iter()
        .copied()
        .filter(|r| !used_regs.contains(r))
        .collect();
    if free.is_empty() {
        return out;
    }

    // --- 6. the scan --------------------------------------------------
    intervals.sort_by_key(|iv| (iv.start, iv.temp));
    // `active` stays sorted by (end, temp): the eviction victim is its
    // last element, and expiry drops everything that ended earlier.
    let mut active: Vec<(usize, usize, u8)> = Vec::new(); // (end, temp, reg)
    let mut assigned: BTreeMap<usize, u8> = BTreeMap::new();

    for iv in &intervals {
        // Expire everything that ended strictly before this start.
        let mut still: Vec<(usize, usize, u8)> = Vec::with_capacity(active.len());
        for &(end, temp, reg) in &active {
            if end < iv.start {
                free.insert(reg);
            } else {
                still.push((end, temp, reg));
            }
        }
        active = still;

        match free.iter().next().copied() {
            Some(reg) => {
                free.remove(&reg);
                assigned.insert(iv.temp, reg);
                active.push((iv.end, iv.temp, reg));
            }
            None => {
                // Furthest-end heuristic: keep the shorter-lived value.
                let victim = active
                    .iter()
                    .copied()
                    .max_by_key(|&(end, temp, _)| (end, temp));
                match victim {
                    Some((vend, vtemp, vreg)) if vend > iv.end => {
                        assigned.remove(&vtemp);
                        active.retain(|&(_, t, _)| t != vtemp);
                        assigned.insert(iv.temp, vreg);
                        active.push((iv.end, iv.temp, vreg));
                    }
                    // This interval outlives every active one: it stays
                    // in the frame, which is simply today's behaviour.
                    _ => {}
                }
            }
        }
        active.sort_by_key(|&(end, temp, _)| (end, temp));
    }

    for (t, r) in assigned {
        out.reg[t] = Some(r);
    }
    out
}

#[cfg(test)]
pub(crate) mod tests_support {
    use super::*;

    /// One point whose touches all sit at word 0 and which contains no
    /// call — the shape most of these oracles need.
    pub(crate) fn point(touches: &[(usize, Touch)]) -> PointFacts {
        PointFacts {
            touches: touches.iter().map(|&(t, h)| (t, h, 0usize)).collect(),
            call_words: Vec::new(),
            regs: BTreeSet::new(),
        }
    }

    /// A point that is nothing but a returning call.
    pub(crate) fn call_point() -> PointFacts {
        PointFacts {
            touches: Vec::new(),
            call_words: vec![0],
            regs: BTreeSet::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::tests_support::*;
    use super::*;

    /// Straight-line: two overlapping scalars, both resident, distinct.
    #[test]
    fn straight_line_scalars_become_resident() {
        let facts = FnFacts {
            temp_count: 2,
            points: vec![
                point(&[(0, Touch::Write), (1, Touch::Write)]),
                point(&[(0, Touch::Read), (1, Touch::Read)]),
                point(&[(0, Touch::Read), (1, Touch::Read)]),
            ],
            back_edges: Vec::new(),
        };
        let a = allocate(&facts, &[true, true]);
        assert_eq!(a.resident_count(), 2);
        assert_ne!(
            a.of(0),
            a.of(1),
            "overlapping temps need distinct registers"
        );
    }

    /// An address-taken temp is refused outright.
    #[test]
    fn an_escaped_temp_is_never_resident() {
        let facts = FnFacts {
            temp_count: 1,
            points: vec![
                point(&[(0, Touch::Write)]),
                point(&[(0, Touch::Escape)]),
                point(&[(0, Touch::Read)]),
                point(&[(0, Touch::Read)]),
            ],
            back_edges: Vec::new(),
        };
        assert_eq!(allocate(&facts, &[true]).of(0), None);
    }

    /// A temp whose slot is not exactly 8 bytes is refused before the
    /// scan runs.
    #[test]
    fn a_non_scalar_slot_is_never_resident() {
        let facts = FnFacts {
            temp_count: 1,
            points: vec![
                point(&[(0, Touch::Write)]),
                point(&[(0, Touch::Read)]),
                point(&[(0, Touch::Read)]),
            ],
            back_edges: Vec::new(),
        };
        assert_eq!(allocate(&facts, &[false]).of(0), None);
    }

    /// A live range spanning a returning call is refused: this ABI
    /// preserves nothing but x30 across a `BL`.
    #[test]
    fn a_range_spanning_a_call_is_refused() {
        let facts = FnFacts {
            temp_count: 1,
            points: vec![
                point(&[(0, Touch::Write)]),
                point(&[(0, Touch::Read)]),
                call_point(),
                point(&[(0, Touch::Read)]),
            ],
            back_edges: Vec::new(),
        };
        assert_eq!(allocate(&facts, &[true]).of(0), None);

        // ...but a range that merely *ends* before the call is fine.
        let facts = FnFacts {
            temp_count: 1,
            points: vec![
                point(&[(0, Touch::Write)]),
                point(&[(0, Touch::Read)]),
                point(&[(0, Touch::Read)]),
                call_point(),
            ],
            back_edges: Vec::new(),
        };
        assert!(allocate(&facts, &[true]).of(0).is_some());
    }

    /// More simultaneously live values than registers: the surplus stays
    /// in the frame and nothing is assigned twice.
    #[test]
    fn more_live_values_than_registers_still_spills_correctly() {
        let n = POOL.len() * 3;
        let mut defs = Vec::new();
        let mut uses = Vec::new();
        for t in 0..n {
            defs.push((t, Touch::Write));
            uses.push((t, Touch::Read));
        }
        let facts = FnFacts {
            temp_count: n,
            points: vec![point(&defs), point(&uses), point(&uses)],
            back_edges: Vec::new(),
        };
        let a = allocate(&facts, &vec![true; n]);
        assert_eq!(
            a.resident_count(),
            POOL.len(),
            "exactly the pool is used, no more"
        );
        let regs: BTreeSet<u8> = a.residents().iter().map(|&(_, r)| r).collect();
        assert_eq!(regs.len(), POOL.len(), "no register is handed out twice");
    }

    /// A register already named by this function's own emission is never
    /// handed to a temp, even though it is in `POOL`.
    #[test]
    fn a_register_the_emitter_already_uses_is_withheld() {
        let mut p0 = point(&[(0, Touch::Write)]);
        p0.regs.extend(POOL.iter().copied().take(POOL.len() - 1));
        let facts = FnFacts {
            temp_count: 2,
            points: vec![
                p0,
                point(&[(0, Touch::Read), (1, Touch::Write), (1, Touch::Read)]),
                point(&[(0, Touch::Read), (1, Touch::Read)]),
            ],
            back_edges: Vec::new(),
        };
        let a = allocate(&facts, &[true, true]);
        assert_eq!(a.resident_count(), 1, "only the untouched register is free");
        assert_eq!(a.of(0), Some(*POOL.last().unwrap()));
    }

    /// The back-edge widening: a use that precedes its def in emission
    /// order forces the two temps apart even though their raw intervals
    /// are disjoint.
    #[test]
    fn a_back_edge_widens_intervals_so_loop_carried_temps_do_not_share() {
        // point 1: def t0        (raw interval [1,1])
        // point 2: def t1        (raw interval [2,3])
        // point 3: use t1, back edge 3 -> 1
        // point 4: use t0        (raw interval [1,4])
        let facts = FnFacts {
            temp_count: 2,
            points: vec![
                point(&[]),
                point(&[(0, Touch::Write)]),
                point(&[(1, Touch::Write), (1, Touch::Read)]),
                point(&[(1, Touch::Read), (0, Touch::Read)]),
                point(&[(0, Touch::Read)]),
            ],
            back_edges: vec![(3, 1)],
        };
        let a = allocate(&facts, &[true, true]);
        assert_eq!(a.resident_count(), 2);
        assert_ne!(
            a.of(0),
            a.of(1),
            "a loop-carried temp must not share a register with a body temp"
        );
    }

    /// Two temps whose intervals really are disjoint, with no back edge
    /// between them, do share a register — the point of the scan.
    #[test]
    fn disjoint_intervals_share_one_register() {
        let facts = FnFacts {
            temp_count: 2,
            points: vec![
                point(&[(0, Touch::Write)]),
                point(&[(0, Touch::Read)]),
                point(&[(0, Touch::Read)]),
                point(&[(1, Touch::Write)]),
                point(&[(1, Touch::Read)]),
                point(&[(1, Touch::Read)]),
            ],
            back_edges: Vec::new(),
        };
        let a = allocate(&facts, &[true, true]);
        assert_eq!(a.of(0), a.of(1));
        assert!(a.of(0).is_some());
    }

    /// Determinism through dumbness (CLAUDE.md): identical facts always
    /// produce an identical assignment.
    #[test]
    fn allocation_is_deterministic() {
        let facts = FnFacts {
            temp_count: 4,
            points: vec![
                point(&[(0, Touch::Write), (1, Touch::Write)]),
                point(&[(2, Touch::Write), (3, Touch::Write)]),
                point(&[
                    (0, Touch::Read),
                    (1, Touch::Read),
                    (2, Touch::Read),
                    (3, Touch::Read),
                ]),
                point(&[
                    (0, Touch::Read),
                    (1, Touch::Read),
                    (2, Touch::Read),
                    (3, Touch::Read),
                ]),
            ],
            back_edges: Vec::new(),
        };
        let first = allocate(&facts, &[true; 4]);
        for _ in 0..8 {
            assert_eq!(allocate(&facts, &[true; 4]), first);
        }
    }

    /// The pool avoids every register `codegen.rs` hardcodes: x0..x8
    /// arguments/results, x9..x14 scratch, x15/x16 `emit_format_scalar`,
    /// x15/x16/x17 `emit_group_create`, x18 platform, x28 `X_FRAME`,
    /// x29/x30/sp fixed.
    #[test]
    fn the_pool_avoids_every_register_codegen_hardcodes() {
        for r in POOL {
            assert!((19..=27).contains(r), "pool register x{r} is out of range");
        }
        assert_eq!(POOL.len(), 9);
    }
}

#[cfg(test)]
mod pays_for_itself_tests {
    use super::tests_support::*;
    use super::*;

    /// **Decision 1765.** A temp written once and read once is a *copy*,
    /// not a residency: residency would turn an independent `str`/`ldr`
    /// pair into a serial two-`mov` chain and buy nothing. It is refused.
    #[test]
    fn a_single_read_temp_is_refused_because_a_register_buys_nothing() {
        let facts = FnFacts {
            temp_count: 1,
            points: vec![point(&[(0, Touch::Write)]), point(&[(0, Touch::Read)])],
            back_edges: Vec::new(),
        };
        assert_eq!(allocate(&facts, &[true]).of(0), None);
    }

    /// ...and the second read is exactly what tips it.
    #[test]
    fn a_second_read_makes_the_register_pay() {
        let facts = FnFacts {
            temp_count: 1,
            points: vec![
                point(&[(0, Touch::Write)]),
                point(&[(0, Touch::Read)]),
                point(&[(0, Touch::Read)]),
            ],
            back_edges: Vec::new(),
        };
        assert!(allocate(&facts, &[true]).of(0).is_some());
    }

    /// A write-only temp is never resident either: nothing reads it, so
    /// there is no reload to delete.
    #[test]
    fn a_write_only_temp_is_refused() {
        let facts = FnFacts {
            temp_count: 1,
            points: vec![point(&[(0, Touch::Write)]), point(&[(0, Touch::Write)])],
            back_edges: Vec::new(),
        };
        assert_eq!(allocate(&facts, &[true]).of(0), None);
    }
}
