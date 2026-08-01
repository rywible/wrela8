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
//!
//! ## Item F: the convention is the function's own
//!
//! Everything above describes item E, which is still exactly what runs
//! when `OptId::InterprocRegs` is off. Item F
//! (plans/codegen-pareto-F.md) replaces the two facts that were global
//! constants with facts computed **per function, over the whole
//! program**:
//!
//! - **The pool.** [`POOL`] is nine registers because item E could not
//!   know which of `x0..x17` the emitter would want. It never had to
//!   guess: the probe already reports every register the function's own
//!   emission names, so the honest pool is [`WIDE_POOL`] minus that
//!   measured set. See [`free_pool`].
//! - **The call barrier.** Item E refuses any temp whose value spans a
//!   returning call, because "this ABI preserves nothing but x30". That
//!   is a statement about a convention written for strangers. A sealed
//!   image has no strangers: [`allocate_program`] computes, for every
//!   function, the set of registers a *caller* must assume it destroys,
//!   bottom-up over the call graph, and a temp is then refused only from
//!   the registers its own spanned callees actually clobber.
//!
//! Both are still measurements, never models. A callee whose body this
//! compiler did not emit — a runtime helper, an async turn body, a
//! `rt_enqueue` key layout may redirect, anything at all outside
//! `MwirProgram::fns` — clobbers **everything** ([`ALL_REGS`]), and so
//! does every function on a call-graph cycle. Failing closed there is
//! what makes the win on the rest believable.

use std::cell::Cell;
use std::collections::{BTreeMap, BTreeSet};

/// A set of physical registers, one bit per register number 0..=31.
/// A bitmask rather than a `BTreeSet` because it is unioned once per
/// call site per interval and because a mask has exactly one ordering
/// (CLAUDE.md: determinism through dumbness).
pub type RegSet = u32;

/// Every register, the answer for any callee this compiler cannot see
/// the body of and for every function on a call-graph cycle.
pub const ALL_REGS: RegSet = u32::MAX;

/// `1 << r`, guarded so a bogus register number cannot silently wrap
/// into some other register's bit.
pub fn reg_bit(r: u8) -> RegSet {
    debug_assert!(r < 32, "register number {r} out of range");
    1u32 << (r & 31)
}

/// The physical registers a resident temp may occupy. See the module
/// doc for why this set and not a larger one.
pub const POOL: &[u8] = &[19, 20, 21, 22, 23, 24, 25, 26, 27];

/// Item F's pool: every general-purpose register that is not fixed by
/// the machine or by this backend's own permanent choices, before the
/// per-function subtraction of what the emitter measurably uses.
///
/// What is *not* here, and why each is not negotiable:
///
/// - **`x18`** — the AArch64 platform register. Nothing in a wrela image
///   uses it and taking it would be a free extra register, but item E's
///   one line of restraint is kept: a register the architecture reserves
///   is not a place to find a win.
/// - **`x28`** — `X_FRAME`, the async turn base. Its whole job is to
///   survive a `BL`, and codegen reloads it after every one.
/// - **`x29`/`x30`/`sp`** — frame pointer (never touched by this ABI),
///   link register, stack pointer.
///
/// `x0..x17` are in the *base* pool and were not in item E's, which is
/// the single largest change to the pool this plan makes. They are not
/// free for the taking: they are the argument/result registers and the
/// emitter's scratch set, so [`free_pool`] subtracts every one of them
/// the function's own probe emission names. What survives is the part of
/// the register file this particular function genuinely does not use.
pub const WIDE_POOL: &[u8] = &[
    0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 19, 20, 21, 22, 23, 24, 25, 26,
    27,
];

// Decision 1760: the allocator is a named opt with a TLS knob, exactly
// like `codegen::set_narrow_imm` (M19 freeze 1402 — `RELEASE_OPTS` is
// edited, nothing else is). Default **off**, so `dev` keeps
// spill-everything as the permanent correctness reference (M19 freeze
// 1407) and every caller that has not opted in is unaffected.
thread_local! {
    static REGALLOC: Cell<bool> = const { Cell::new(false) };
    static INTERPROC: Cell<bool> = const { Cell::new(false) };
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

/// Item F / decision 1771: enable/disable the whole-program convention
/// (widened per-function pool + per-callee clobber sets). Inert unless
/// [`regalloc`] is also on — there is no allocation for it to extend.
pub fn set_interproc_regs(enabled: bool) {
    INTERPROC.with(|c| c.set(enabled));
}

/// Whether the whole-program convention is enabled (default false).
pub fn interproc_regs() -> bool {
    INTERPROC.with(|c| c.get())
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

/// One returning call, as the probe emission performed it: the word
/// index within its own point, and the callee's key when the emitter
/// named one (`Reloc::Call`). `None` is every other returning `BL` —
/// the checkpoint service, a runtime helper, anything whose target this
/// module cannot look up — and it resolves to [`ALL_REGS`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CallWord {
    pub word: usize,
    pub callee: Option<String>,
}

/// What one program point does. Point `0` is the prologue, points
/// `1..=body.len()` are `body[i - 1]`, and point `body.len() + 1` is the
/// epilogue — so the parameter stores the prologue emits and the
/// write-backs the epilogue emits are ordinary touches, not special
/// cases.
#[derive(Clone, Debug, Default)]
pub struct PointFacts {
    /// `(temp index, touch, word index within this point, register)`, in
    /// emission order. The word index is what makes the call barrier
    /// precise enough to be worth having — see [`allocate`]'s step 4.
    ///
    /// The **register** is the one the emitter moved that value through
    /// at that access, and it is item I's argument/return-position input
    /// (decision 1902): an access at a fixed-role register — `x0..x7` at
    /// a call site, `x0` at a `Return`, the incoming parameter register
    /// in the prologue — is a copy whose other end this module can
    /// coalesce by *choosing that register as the temp's home*.
    pub touches: Vec<(usize, Touch, usize, u8)>,
    /// Every `CostRule::Call` word this point emits: a call that
    /// returns, and therefore a point at which the callee's clobber set
    /// takes effect. `CostRule::Abort`/`AbortVal` are noreturn and are
    /// deliberately not barriers.
    pub call_words: Vec<CallWord>,
    /// Every register this point's baseline emission names as a `dst` or
    /// a `src`. The pool is intersected with the complement of the union
    /// over all points.
    pub regs: BTreeSet<u8>,
    /// The same information at **word** grain: `(word index, register)`
    /// for every register each word of this point names.
    ///
    /// [`regs`](Self::regs) answers "does this function use `x1`", which
    /// is the only question a per-function pool can ask and the reason
    /// item F could not reach argument position: a function with any call
    /// at all names `x1`, so `free_pool` withholds it from every temp.
    /// This field answers the question that was actually wanted — "is
    /// `x1` busy *here*" — and item I's [`hint_admissible`] is what turns
    /// it into a decision.
    pub word_regs: Vec<(usize, u8)>,
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
    /// Item F: every function this one transfers control to and may
    /// return from, from the `Reloc::Call` entries the emitter itself
    /// pushed. **Tail calls are in here and are not in `call_words`**,
    /// which is exactly right for both readers: a tail call clobbers
    /// registers on this function's behalf (so its callers must know)
    /// but nothing of this function's is live across it.
    pub calls: BTreeSet<String>,
    /// True when this function made a returning call the emitter did not
    /// name — the checkpoint service is the one that occurs in practice.
    /// It forces the whole function's clobber set to [`ALL_REGS`].
    pub opaque_calls: bool,
    /// True when any point emits a returning call at all. F3 reads it:
    /// a function that can return from a `BL` must save `x30`, and
    /// saving `x30` is the last reason a leaf needs a frame.
    pub has_returning_call: bool,
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

/// A closed live interval over program points, and the temps that share
/// it.
///
/// It is a *set* of temps rather than one because of item I's coalescing
/// (plans/codegen-pareto-2.md, decision 1901): when a copy's two ends do
/// not otherwise interfere they are one value in one register, and the
/// copy is then not an instruction at all.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Interval {
    /// Every temp in this class, ascending. One entry before coalescing.
    temps: Vec<usize>,
    /// The lowest temp number in `temps`: the scan's tie-break key, so
    /// the order is a function of the facts alone (CLAUDE.md,
    /// determinism through dumbness).
    key: usize,
    start: usize,
    end: usize,
    /// Registers this interval may **not** occupy, because a callee it
    /// spans destroys them. `0` for an interval that spans no returning
    /// call; [`ALL_REGS`] under item E's total barrier, which is why
    /// the same scan expresses both policies.
    forbidden: RegSet,
    /// Item I's coalescing hints, in preference order: fixed-role
    /// registers the emitter already moves this value through, each one
    /// verified free over the whole interval by [`hint_admissible`].
    /// Taking one deletes the copy at every access that uses it.
    hints: Vec<u8>,
}

/// Union-find over temps, used only by [`coalesce`]. A plain `Vec` with
/// path halving — no interning, no arena (CLAUDE.md).
fn uf_find(parent: &mut [usize], mut x: usize) -> usize {
    while parent[x] != x {
        parent[x] = parent[parent[x]];
        x = parent[x];
    }
    x
}

/// Every register this function's own baseline emission names, as a
/// mask. The one input the pool is subtracted from; see [`free_pool`].
fn measured_regs(facts: &FnFacts) -> RegSet {
    let mut m: RegSet = 0;
    for p in &facts.points {
        for &r in &p.regs {
            m |= reg_bit(r);
        }
    }
    m
}

/// The registers this function may hand to a temp: `base_pool` minus
/// every register its own probe emission was measured naming.
///
/// This is the whole of item F's "reaching the rest of the register
/// file". Nothing about it is a new proof: item E already intersected
/// [`POOL`] with this complement as a *safety net* behind a hand-picked
/// nine. Item F deletes the hand-picked part and keeps the net, because
/// the net was always the load-bearing half. A register the emitter
/// touches for any reason — an argument register at a call site, `x8`
/// for an aggregate result, `X_A..X_F` scratch, the digit-loop pair — is
/// named by some word of some point and is therefore out, per function,
/// without anyone having to enumerate the reasons correctly.
pub fn free_pool(facts: &FnFacts, base_pool: &[u8]) -> Vec<u8> {
    let used = measured_regs(facts);
    base_pool
        .iter()
        .copied()
        .filter(|&r| used & reg_bit(r) == 0)
        .collect()
}

/// Decide residency for one function under item E's rules: the nine-
/// register [`POOL`], and a **total** call barrier.
///
/// `scalar_slot[t]` says the temp occupies exactly one 8-byte frame
/// slot; anything wider is refused before the scan even looks at it.
pub fn allocate(facts: &FnFacts, scalar_slot: &[bool]) -> Assignment {
    allocate_with(facts, scalar_slot, POOL, None, false)
}

/// Decide residency for one function under item F's rules.
///
/// `base_pool` is the register set before the per-function subtraction
/// ([`free_pool`]). `callee_clobbers` is `None` for item E's total call
/// barrier and `Some(map)` for item F's per-callee one; a callee absent
/// from the map clobbers [`ALL_REGS`], which is the same refusal item E
/// applied to every call.
/// `pays_for_itself` is decision 1765's two-read rule. It is `true`
/// everywhere except in the one place item F3 turns it off: see
/// [`allocate_one`], where a function whose *only* remaining frame bytes
/// belong to single-read temps is re-asked without it, because deleting
/// the whole frame is worth more than the copy the rule was protecting
/// against.
pub fn allocate_with(
    facts: &FnFacts,
    scalar_slot: &[bool],
    base_pool: &[u8],
    callee_clobbers: Option<&BTreeMap<String, RegSet>>,
    pays_for_itself: bool,
) -> Assignment {
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
        for &(t, touch, _, _) in &p.touches {
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
        for &(t, touch, _, _) in &p.touches {
            if t < n && touch == Touch::Read {
                reads[t] += 1;
            }
        }
    }
    if pays_for_itself {
        for t in 0..n {
            if reads[t] < 2 {
                eligible[t] = false;
            }
        }
    }

    // --- 2. intervals from first touch to last touch ------------------
    let mut first: Vec<Option<usize>> = vec![None; n];
    let mut last: Vec<Option<usize>> = vec![None; n];
    for (i, p) in facts.points.iter().enumerate() {
        for &(t, _, _, _) in &p.touches {
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
                temps: vec![t],
                key: t,
                start: a,
                end: b,
                forbidden: 0,
                hints: Vec::new(),
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

    // --- 5. the pool this function may actually use -------------------
    //
    // Hoisted above the barrier because item F's barrier is a *register*
    // question, not a yes/no one: an interval survives a call iff some
    // register in this function's pool is one the callee does not
    // destroy, and that cannot be asked before the pool is known.
    let pool: Vec<u8> = free_pool(facts, base_pool);
    if pool.is_empty() {
        return out;
    }
    let pool_mask: RegSet = pool.iter().fold(0, |m, &r| m | reg_bit(r));

    // --- 4. what a temp live across a returning call may occupy -------
    //
    // **Item E:** nothing. Only x30 survives a `BL` under a convention
    // written so that strangers can call your code, so a resident value
    // cannot, and the interval is dropped.
    //
    // **Item F (`callee_clobbers = Some`):** the caller knows exactly
    // which registers this callee destroys, because the whole program is
    // in the image and [`allocate_program`] computed the answer bottom-up
    // before this function was reached. The interval is not dropped; it
    // is *constrained*, and it is dropped only when the constraint
    // leaves no register at all. That is F2 — the conservative
    // save/restore does not shrink, it never existed, and the callee-
    // saved discipline that would have replaced it is not created either:
    // the caller simply picks a register this callee was measured not to
    // touch.
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
    for iv in intervals.iter_mut() {
        for i in iv.start..=iv.end.min(facts.points.len().saturating_sub(1)) {
            let p = &facts.points[i];
            if p.call_words.is_empty() {
                continue;
            }
            let mut lo_w: Option<usize> = None;
            let mut hi_w: Option<usize> = None;
            for &(t, _, w, _) in &p.touches {
                if !iv.temps.contains(&t) {
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
            for cw in &p.call_words {
                if cw.word < lo || cw.word > hi {
                    continue;
                }
                iv.forbidden |= match callee_clobbers {
                    // Item E: nothing survives a `BL`.
                    None => ALL_REGS,
                    // Item F: exactly what this callee destroys, and
                    // everything when the callee is one whose body this
                    // compiler does not hold.
                    Some(map) => match &cw.callee {
                        Some(k) => map.get(k).copied().unwrap_or(ALL_REGS),
                        None => ALL_REGS,
                    },
                };
            }
        }
    }
    // --- 5b. coalescing (item I, decisions 1901 / 1902) ---------------
    coalesce(facts, &mut intervals, pool_mask);
    for iv in intervals.iter_mut() {
        iv.hints = hints_for(facts, iv);
    }

    // An interval survives if it can take *some* register: one of the
    // pool's, or one of the fixed-role registers item I proved free over
    // its whole range.
    intervals.retain(|iv| {
        let hint_mask: RegSet = iv.hints.iter().fold(0, |m, &r| m | reg_bit(r));
        (pool_mask | hint_mask) & !iv.forbidden != 0
    });
    if intervals.is_empty() {
        return out;
    }

    // --- 6. the scan --------------------------------------------------
    intervals.sort_by(|a, b| (a.start, a.key).cmp(&(b.start, b.key)));
    let mut free: BTreeSet<u8> = pool.iter().copied().collect();
    // `active` stays sorted by (end, temp): the eviction victim is its
    // last element, and expiry drops everything that ended earlier.
    let mut active: Vec<(usize, usize, u8)> = Vec::new(); // (end, temp, reg)
    let mut assigned: BTreeMap<usize, u8> = BTreeMap::new();
    // Item I: hinted registers live outside the pool, so they get their
    // own occupancy book — `reg -> the end point of the class holding
    // it`. A later interval may take it only once that end has passed.
    let mut hint_held: BTreeMap<u8, usize> = BTreeMap::new();

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

        // **Item I, decision 1902.** A hint is a fixed-role register the
        // emitter already moves this value through, and [`hints_for`] has
        // already proved it free over the whole interval. Taking it
        // deletes a `mov` at every access that uses it — the whole of
        // argument and return position. Hints are tried before the pool,
        // and a hint outside the pool is booked in `hint_held` so a
        // second, overlapping interval cannot take it too.
        let mut pick = None;
        for &r in &iv.hints {
            if iv.forbidden & reg_bit(r) != 0 {
                continue;
            }
            if pool_mask & reg_bit(r) != 0 {
                // Also a pool register: bias towards it, but it is the
                // ordinary `free` set that says whether it is available.
                if free.contains(&r) {
                    pick = Some(r);
                    break;
                }
            } else if hint_held.get(&r).is_none_or(|&e| e < iv.start) {
                hint_held.insert(r, iv.end);
                assigned.insert(iv.key, r);
                pick = Some(r);
                break;
            }
        }
        if let Some(r) = pick {
            if pool_mask & reg_bit(r) == 0 {
                // A hinted register outside the pool: already booked.
                active.sort_by_key(|&(end, temp, _)| (end, temp));
                continue;
            }
        }
        // The lowest free register this interval is *allowed* to take.
        // Under item E `forbidden` is only ever `0` or `ALL_REGS`, so
        // this reduces exactly to "the lowest free register".
        let pick = pick.or_else(|| {
            free.iter()
                .copied()
                .find(|&r| iv.forbidden & reg_bit(r) == 0)
        });
        match pick {
            Some(reg) => {
                free.remove(&reg);
                assigned.insert(iv.key, reg);
                active.push((iv.end, iv.key, reg));
            }
            None => {
                // Furthest-end heuristic: keep the shorter-lived value.
                // Only an occupant whose register this interval could
                // actually use is a candidate victim — evicting one whose
                // register is forbidden here would spill a live value and
                // buy nothing.
                let victim = active
                    .iter()
                    .copied()
                    .filter(|&(_, _, reg)| iv.forbidden & reg_bit(reg) == 0)
                    .max_by_key(|&(end, temp, _)| (end, temp));
                match victim {
                    Some((vend, vkey, vreg)) if vend > iv.end => {
                        assigned.remove(&vkey);
                        active.retain(|&(_, k, _)| k != vkey);
                        assigned.insert(iv.key, vreg);
                        active.push((iv.end, iv.key, vreg));
                    }
                    // This interval outlives every active one: it stays
                    // in the frame, which is simply today's behaviour.
                    _ => {}
                }
            }
        }
        active.sort_by_key(|&(end, temp, _)| (end, temp));
    }

    for iv in &intervals {
        if let Some(&r) = assigned.get(&iv.key) {
            for &t in &iv.temps {
                out.reg[t] = Some(r);
            }
        }
    }
    out
}

/// **Item I's coalescing** (plans/codegen-pareto-2.md, decision 1901).
///
/// A copy is two live ranges that meet at exactly one program point and
/// are otherwise disjoint. The scan cannot see that: both ranges contain
/// the copy's point, so they "interfere", get different registers, and
/// the copy survives as a `mov`. That is precisely the 161 moves item E
/// added to the appliance image. This pass merges such a pair into one
/// interval *before* the scan runs, so the two ends get one register and
/// `FnCtx::mov_reg` elides the copy outright.
///
/// **What counts as a copy is measured, not asserted.** A copy point is
/// a point whose entire slot traffic is one `Read` of `s` followed by one
/// `Write` of `d` — read off the probe's own emission, exactly like every
/// other fact this module consumes (decision 1761). That signature also
/// catches `Neg`, `Not`, `BitNot`, `Convert`, `EnumTag`, `PtrOffset`, a
/// scalar `Project` and an `MmioRead`: every MWIR instruction with one
/// source temp and one destination temp. Merging those is not an
/// approximation — a single-source instruction reads its operand and
/// writes its result in one instruction, so `neg x4, x4` and
/// `ldr x4, [x4, #8]` are exactly as correct as `mov x4, x4` is
/// unnecessary, and each is one word instead of two.
///
/// **The merge condition is the whole proof.** `s` and `d` are merged
/// only when the copy point is `s`'s class's *last* point and `d`'s
/// class's *first*, so the union of the two ranges is the single interval
/// `[s.start, d.end]` and no other value is live inside it that was not
/// live inside one of them. Classes are merged iteratively in point
/// order against their *current* extents, so a chain `y = x; z = y`
/// collapses onto one register and a merge that would create an overlap
/// is refused rather than approximated. A point that emits a returning
/// call is never a copy point: the barrier owns those.
fn coalesce(facts: &FnFacts, intervals: &mut Vec<Interval>, pool_mask: RegSet) {
    if intervals.len() < 2 {
        return;
    }
    let n = facts.temp_count;
    // temp -> index into `intervals`, and the union-find that merges them.
    let mut idx: Vec<Option<usize>> = vec![None; n];
    for (i, iv) in intervals.iter().enumerate() {
        idx[iv.temps[0]] = Some(i);
    }
    let mut parent: Vec<usize> = (0..intervals.len()).collect();
    let mut merged = false;

    for (i, p) in facts.points.iter().enumerate() {
        if !p.call_words.is_empty() || p.touches.len() != 2 {
            continue;
        }
        let (s, sh, sw, _) = p.touches[0];
        let (d, dh, dw, _) = p.touches[1];
        if sh != Touch::Read || dh != Touch::Write || s == d || sw > dw {
            continue;
        }
        let (Some(si), Some(di)) = (idx.get(s).copied().flatten(), idx.get(d).copied().flatten())
        else {
            continue;
        };
        let (cs, cd) = (uf_find(&mut parent, si), uf_find(&mut parent, di));
        if cs == cd {
            continue;
        }
        // The two ranges must meet at this point and nowhere else, and
        // the merged class must still have a register it may occupy.
        if intervals[cs].end != i || intervals[cd].start != i {
            continue;
        }
        let forbidden = intervals[cs].forbidden | intervals[cd].forbidden;
        if pool_mask & !forbidden == 0 {
            continue;
        }
        let hints = Vec::new();
        let (start, end) = (intervals[cs].start, intervals[cd].end);
        let mut temps = std::mem::take(&mut intervals[cd].temps);
        temps.append(&mut intervals[cs].temps);
        temps.sort_unstable();
        temps.dedup();
        let key = temps[0];
        intervals[cs] = Interval {
            temps,
            key,
            start,
            end,
            forbidden,
            hints,
        };
        // `cd` is now empty; point it at `cs` and leave the husk to be
        // dropped below.
        parent[cd] = cs;
        merged = true;
    }

    if merged {
        intervals.retain(|iv| !iv.temps.is_empty());
    }
}

/// The highest register item I will hint. `x0..=x8` are the fixed-role
/// registers — the argument/result set — and they are the *only* place
/// argument and return position can live. Nothing above them is hinted:
/// `x9..x17` are the emitter's scratch set, and a scratch register the
/// emission never names is already an ordinary pool register, so hinting
/// it would buy nothing that [`free_pool`] does not already give.
pub const MAX_HINT_REG: u8 = 8;

/// **Item I's argument/return position** (decision 1902).
///
/// The `mov` around every call site exists because a value cannot be
/// homed in the register it is about to occupy: the call site's argument
/// load names `x1`, so [`free_pool`]'s per-function subtraction withholds
/// `x1` from every temp in any function that makes a call. That is item
/// F's own recorded residual, and its note says the fix needs "a
/// per-point interference model rather than a per-function union of
/// registers".
///
/// This is that model, and it is small because the question is small.
/// The candidates are not guessed: they are the registers the emitter
/// **was measured moving this value through** — `x0..x7` at a call site,
/// `x0` at a `Return`, the incoming parameter register in the prologue.
/// Each is then admitted only if [`hint_admissible`] finds it free at
/// every *word* the value is live over, counting the value's own accesses
/// as free because those are exactly the copies being coalesced away.
///
/// Order is by how many accesses the register would delete, then by
/// register number — a function of the facts alone, so the answer does
/// not depend on iteration order (CLAUDE.md).
fn hints_for(facts: &FnFacts, iv: &Interval) -> Vec<u8> {
    let mut counts: BTreeMap<u8, usize> = BTreeMap::new();
    for i in iv.start..=iv.end.min(facts.points.len().saturating_sub(1)) {
        for &(t, how, _, reg) in &facts.points[i].touches {
            if reg <= MAX_HINT_REG && how != Touch::Escape && iv.temps.contains(&t) {
                *counts.entry(reg).or_insert(0) += 1;
            }
        }
    }
    let mut cands: Vec<(usize, u8)> = counts
        .into_iter()
        .filter(|&(r, _)| hint_admissible(facts, iv, r))
        .map(|(r, c)| (c, r))
        .collect();
    // Descending by count, ascending by register on a tie.
    cands.sort_by(|a, b| (b.0, a.1).cmp(&(a.0, b.1)));
    cands.into_iter().map(|(_, r)| r).collect()
}

/// Is register `r` free over every word this interval is live across?
///
/// The window inside one point is the same one the call barrier uses: a
/// value live *into* the point occupies its register from word 0, and a
/// value live *past* it occupies it to the last word — but at the point
/// where the range begins, the words before its first access are not its
/// own, and at the point where it ends, the words after its last access
/// are not either. That refinement is the whole reason argument position
/// is reachable: a temp whose last access is `ldr x1, [sp, #N]` at word 1
/// of a call point is dead by the `bl` at word 2, so the `bl` naming `x1`
/// is not a conflict.
///
/// A word that is one of the value's **own** accesses is never a
/// conflict. That is not a loophole, it is the coalescing: an access at
/// `x1` is a copy between `x1` and the home, and choosing `x1` as the
/// home is what deletes it.
///
/// **Why the baseline's word indices describe the final program.** The
/// facts come from the probe, which emits with nothing resident. The
/// final emission differs by deleting words (item I's coalescing) and by
/// substituting a home register for a scratch one at an access. Deleting
/// words only removes register uses; a substitution puts the home of the
/// temp *that access belongs to* where a scratch register was, and two
/// temps that are simultaneously live never share a register — so no word
/// inside this interval can come to name `r` unless it already did.
fn hint_admissible(facts: &FnFacts, iv: &Interval, r: u8) -> bool {
    let last = facts.points.len().saturating_sub(1);
    for i in iv.start..=iv.end.min(last) {
        let p = &facts.points[i];
        let mut own: Vec<usize> = Vec::new();
        for &(t, _, w, _) in &p.touches {
            if iv.temps.contains(&t) {
                own.push(w);
            }
        }
        let lo = if iv.start < i {
            0
        } else {
            match own.iter().min() {
                Some(&w) => w,
                // The range starts at this point but touches nothing in
                // it: only a widened range does that, and it is live
                // throughout.
                None => 0,
            }
        };
        let hi = if iv.end > i {
            usize::MAX
        } else {
            match own.iter().max() {
                Some(&w) => w,
                None => usize::MAX,
            }
        };
        for &(w, reg) in &p.word_regs {
            if reg != r || w < lo || w > hi {
                continue;
            }
            if !own.contains(&w) {
                return false;
            }
        }
    }
    true
}

// --- item F: the whole-program convention -----------------------------------

/// Everything the allocator is told about one function, plus the one
/// derived fact `build_frame` needs. Assembled by `codegen.rs` for every
/// sync fn before any of them is emitted.
#[derive(Clone, Debug, Default)]
pub struct FnInput {
    pub facts: FnFacts,
    /// `scalar_slot[t]`: the temp occupies exactly one 8-byte slot.
    pub scalar_slot: Vec<bool>,
    /// **This key's body is not this compiler's last word on it**
    /// (decision 1793): a compiler-synthesized symbol that `layout.rs`
    /// may replace outright, alias onto another body, or re-point at a
    /// cross-core trampoline. Codegen decides it from the key's spelling
    /// and this module never looks at spellings itself.
    ///
    /// Such a function still gets its own allocation — its emitted code
    /// is its own — but it publishes [`ALL_REGS`], because a caller's
    /// residency decision would otherwise be about a body that is not
    /// the one that runs. This is the failure the boot transcripts
    /// caught and neither the units nor `diff-eval` could: layout's
    /// `install_abort_tail_floor` and `inject_test_runner_fns` overwrite
    /// `__wrela_abort_tail` and `__test_call_*` with hand-assembled
    /// bodies *after* codegen has published a convention for the wrela
    /// stubs they replace.
    pub opaque_body: bool,
}

/// One function's **chosen convention** — the thing the report has to
/// show, because it is the most consequential decision in the compiler
/// and it is otherwise invisible.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Convention {
    /// Which temps live in which physical registers.
    pub assignment: Assignment,
    /// The registers this function was allowed to use, after subtracting
    /// what its own emission names.
    pub pool: Vec<u8>,
    /// Every register a **caller** must assume this function destroys:
    /// its own emission's registers, its residents' homes, and the union
    /// of the same over everything it calls. [`ALL_REGS`] for a function
    /// on a call-graph cycle or one that reaches a body this compiler
    /// does not hold.
    pub clobbers: RegSet,
    /// Whether the clobber set is the fail-closed [`ALL_REGS`] answer
    /// rather than a measured one. Reported, not inferred from the mask:
    /// a function that genuinely touches every register would otherwise
    /// be indistinguishable from one that was refused.
    pub opaque: bool,
}

/// Compute every sync function's convention, over the whole program.
///
/// **The algorithm, in one paragraph.** Build the call graph from the
/// probe's own `Reloc::Call` targets — never from a second walk of MWIR,
/// so a call the emitter makes and the analysis does not see cannot
/// exist. Then process functions in *callee-before-caller* order, by
/// repeated passes that take every function all of whose named callees
/// are already done (Kahn's algorithm, run over a `BTreeMap` so the
/// order is a function of the keys alone). Each function so reached is
/// allocated with the real clobber set of each callee it spans, and its
/// own clobber set is then `measured ∪ homes ∪ ⋃ callees`. When a pass
/// makes no progress, everything left is on or above a call-graph cycle:
/// every one of those is stamped [`ALL_REGS`] *before* any of them is
/// allocated, so a recursive callee can never be believed. That is the
/// whole of the fail-closed story — there is no iteration to a fixpoint,
/// because `homes` is not monotone in the clobber sets it is computed
/// from and a fixpoint over a non-monotone function is not a proof.
pub fn allocate_program(fns: &BTreeMap<String, FnInput>) -> BTreeMap<String, Convention> {
    // The call graph, and which functions name a callee we do not hold.
    let mut callees: BTreeMap<&str, BTreeSet<&str>> = BTreeMap::new();
    let mut opaque_callee: BTreeMap<&str, bool> = BTreeMap::new();
    for (key, input) in fns {
        let mut set: BTreeSet<&str> = BTreeSet::new();
        let mut unknown = input.facts.opaque_calls;
        for k in &input.facts.calls {
            match fns.get_key_value(k) {
                Some((held, held_input)) if !held_input.opaque_body => {
                    set.insert(held.as_str());
                }
                // A helper, an async turn body, a symbol only the glue
                // emitters define — or a key this compiler emitted a body
                // for that a later stage may replace (decision 1793).
                _ => unknown = true,
            }
        }
        callees.insert(key.as_str(), set);
        opaque_callee.insert(key.as_str(), unknown);
    }

    let mut clobbers: BTreeMap<String, RegSet> = BTreeMap::new();
    let mut out: BTreeMap<String, Convention> = BTreeMap::new();
    let mut done: BTreeSet<&str> = BTreeSet::new();

    loop {
        let mut progressed = false;
        for (key, input) in fns {
            let k = key.as_str();
            if done.contains(k) {
                continue;
            }
            if !callees[k].iter().all(|c| done.contains(c)) {
                continue;
            }
            let mut conv = allocate_one(input, &clobbers, opaque_callee[k], &callees[k]);
            if input.opaque_body {
                conv.clobbers = ALL_REGS;
                conv.opaque = true;
            }
            clobbers.insert(key.clone(), conv.clobbers);
            out.insert(key.clone(), conv);
            done.insert(k);
            progressed = true;
        }
        if !progressed {
            break;
        }
    }

    // Everything still undone is on, or reaches, a call-graph cycle.
    // Stamp them all opaque *first*: a member allocated while another
    // member's answer was still missing would be reading a hole.
    let cyclic: Vec<&str> = fns
        .keys()
        .map(|k| k.as_str())
        .filter(|k| !done.contains(k))
        .collect();
    for k in &cyclic {
        clobbers.insert((*k).to_string(), ALL_REGS);
    }
    for k in &cyclic {
        let input = &fns[*k];
        let mut conv = allocate_one(input, &clobbers, true, &callees[*k]);
        conv.clobbers = ALL_REGS;
        conv.opaque = true;
        out.insert((*k).to_string(), conv);
    }
    out
}

/// One function's convention, given final answers for its callees.
fn allocate_one(
    input: &FnInput,
    clobbers: &BTreeMap<String, RegSet>,
    opaque_callee: bool,
    callees: &BTreeSet<&str>,
) -> Convention {
    let pool = free_pool(&input.facts, WIDE_POOL);
    let assignment = allocate_with(
        &input.facts,
        &input.scalar_slot,
        WIDE_POOL,
        Some(clobbers),
        false,
    );
    // **Decision 1775, and what the ∀ gate did to it.**
    //
    // The first landing of F3 had a feedback edge here: when the *only*
    // thing standing between a function and no frame at all was a handful
    // of temps decision 1765 refuses (read once, so a register turns an
    // `str`/`ldr` pair into a serial two-`mov` chain), the question was
    // re-asked without the rule and the relaxed answer taken if it landed
    // the function at zero frame bytes. The trade looked obviously right:
    // four more words deleted (`sub sp`, `str x30`, `ldr x30`, `add sp`)
    // against a copy that costs nothing when the forwarding latency is
    // low.
    //
    // **It is not right, and the gate said so at both tiers.** With the
    // relaxation on, `Frameless` was refused for `cost-branch-bias`
    // rising **267 -> 310** at every corner with
    // `store_to_load_forwarding=1`, and for `cost-product-receipt` rising
    // **7551 -> 7554** at the same corners — the identical shape item E
    // measured when it promoted every scalar (plans/codegen-pareto-E.md,
    // "what the ∀ gate caught"). Four deleted words do not pay for a
    // serialized copy chain in the body, and the frame that F3 removes
    // was never on the critical path to begin with.
    //
    // So decision 1765 is **inherited, not re-derived**: item E's
    // hand-off called it "a policy, not an invariant; the ∀ gate is what
    // decides it", and the gate has now decided it a second time, on a
    // second baseline, over a corpus that includes four programs the
    // appliance ships. F3 fires only where the two-read rule already left
    // the frame empty.
    let mut mask = measured_regs(&input.facts);
    for (_, r) in assignment.residents() {
        mask |= reg_bit(r);
    }
    let mut opaque = opaque_callee;
    for c in callees {
        match clobbers.get(*c) {
            Some(&m) => mask |= m,
            // Unreachable by construction (every callee in `callees` is
            // in `fns`, and every member of `fns` has an entry by the
            // time it is read). Fail closed rather than assume.
            None => {
                mask = ALL_REGS;
                opaque = true;
            }
        }
    }
    if opaque_callee {
        mask = ALL_REGS;
    }
    Convention {
        assignment,
        pool,
        clobbers: mask,
        opaque,
    }
}

/// `x19-x21,x27` — the compact spelling the report uses for a register
/// set. `ALL_REGS` is rendered by its name, because "every register"
/// is a different fact from "these thirty-two registers".
pub fn render_reg_set(mask: RegSet) -> String {
    if mask == ALL_REGS {
        return "all".to_string();
    }
    if mask == 0 {
        return "none".to_string();
    }
    let mut out = String::new();
    let mut r = 0u8;
    while r < 32 {
        if mask & reg_bit(r) == 0 {
            r += 1;
            continue;
        }
        let start = r;
        while r < 31 && mask & reg_bit(r + 1) != 0 {
            r += 1;
        }
        if !out.is_empty() {
            out.push(',');
        }
        if start == r {
            out.push_str(&format!("x{start}"));
        } else {
            out.push_str(&format!("x{start}-x{r}"));
        }
        r += 1;
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
            // Register 9 (`codegen::X_A`) is the scratch register the
            // emitter really uses for most of these accesses, and it is
            // above `MAX_HINT_REG`, so these fixtures exercise the
            // ordinary pool path rather than item I's hinting.
            touches: touches.iter().map(|&(t, h)| (t, h, 0usize, 9u8)).collect(),
            call_words: Vec::new(),
            regs: BTreeSet::new(),
            word_regs: Vec::new(),
        }
    }

    /// A point that is nothing but a returning call to an unnamed
    /// target — the shape item E's barrier tests need.
    pub(crate) fn call_point() -> PointFacts {
        call_point_to(None)
    }

    /// A point that is nothing but a returning call to `callee`.
    pub(crate) fn call_point_to(callee: Option<&str>) -> PointFacts {
        PointFacts {
            touches: Vec::new(),
            call_words: vec![CallWord {
                word: 0,
                callee: callee.map(str::to_string),
            }],
            regs: BTreeSet::new(),
            word_regs: Vec::new(),
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
            ..Default::default()
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
            ..Default::default()
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
            ..Default::default()
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
            ..Default::default()
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
            ..Default::default()
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
            ..Default::default()
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
            ..Default::default()
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
            ..Default::default()
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
            ..Default::default()
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
            ..Default::default()
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

    /// **Decision 1765, overturned by item I (decision 1904).**
    ///
    /// 1765 refused a temp written once and read once, because residency
    /// then turned an independent `str`/`ldr` pair into a *serial
    /// two-`mov` chain* and bought nothing — the ∀ gate caught exactly
    /// that, rising at every `store_to_load_forwarding=1` corner.
    ///
    /// Coalescing removes the premise. A single-read temp's copy is now
    /// elided rather than emitted, so residency costs **no** instruction
    /// instead of two, and the `str`/`ldr` pair is deleted outright. The
    /// rule 1765 imposed was never about read counts; it was about the
    /// price of a copy, and that price is now zero. So a single-read temp
    /// **is** promoted, and this test pins the reversal rather than being
    /// deleted, so that losing coalescing would fail here loudly instead
    /// of silently reintroducing the regression 1765 was written to stop.
    #[test]
    fn a_single_read_temp_is_now_promoted_because_its_copy_is_free() {
        let facts = FnFacts {
            temp_count: 1,
            points: vec![point(&[(0, Touch::Write)]), point(&[(0, Touch::Read)])],
            back_edges: Vec::new(),
            ..Default::default()
        };
        assert!(
            allocate(&facts, &[true]).of(0).is_some(),
            "with coalescing a single-read temp's copy is free, so residency pays"
        );
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
            ..Default::default()
        };
        assert!(allocate(&facts, &[true]).of(0).is_some());
    }

    /// A write-only temp is never resident either: nothing reads it, so
    /// there is no reload to delete.
    #[test]
    /// **Also reversed by item I (decision 1904).** A temp written twice
    /// and never read used to be refused on the same "a register buys
    /// nothing" reasoning. With coalescing it buys two deleted `str`s and
    /// costs nothing, so it is promoted.
    ///
    /// Safe because a frame slot is private to its function: a temp with
    /// no read in `FnFacts` has no reader anywhere, and item E's rule that
    /// `addr_of_slot` on a resident temp is a **build failure** is what
    /// keeps an address-taken value from reaching this path at all. (The
    /// writes are also dead, and a DCE pass should delete them outright —
    /// that is item J's, not the allocator's.)
    #[test]
    fn a_write_only_temp_is_now_promoted_and_its_stores_deleted() {
        let facts = FnFacts {
            temp_count: 1,
            points: vec![point(&[(0, Touch::Write)]), point(&[(0, Touch::Write)])],
            back_edges: Vec::new(),
            ..Default::default()
        };
        assert!(allocate(&facts, &[true]).of(0).is_some());
    }
}
