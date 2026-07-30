# The optimization ladder: candidate opts for the A76 ruler

**Status: PROPOSED (2026-07-29).** Not activated, and **cannot activate
before M20 closes** — every candidate below is priced by the ruler M20
builds (block-grain `f`, 8-pipe port model, real memory hierarchy,
bias-derived branches, cross-core terms, ∀ sweep). This document is the
expanded form of what [M20.md](M20.md) item M records as "the candidate
list the new ruler makes scoreable," written down now so the arguments from
the 2026-07-29 design discussions are not re-derived later. Decision block
1700–1799 reserved on activation. Milestone numbering is the human's call;
the tiers below are dependency-ordered, not milestone-ordered — several
tiers could be one milestone or several.

## Standing rules (M19/M20 doctrine, restated once)

- Every opt is a **named function** in the in-code `RELEASE_OPTS` order
  ([opts/mod.rs:33](../crates/wrela-compiler/src/opts/mod.rs:33)). No pass
  manager, no plugin registry (M19 freeze 1402).
- The land gate is M20's: **∀ over the geometry/uncertainty box**, no
  measured coverage loss, per-core text/TLB budget respected, veto reasons
  all reported. Frequency-dependent opts additionally need veto-then-rank
  overall across the pinned workload set. Physical wall time never gates
  (M19 freeze 1404).
- Losers are **deleted**, not kept disabled (M19 doctrine).
- `dev` mode stays the correctness reference; `diff-eval` and the goldens
  hold for both modes (M19 freeze 1407).
- Barrier removal is never profitable by construction (M20 freeze 1633).

## What the ruler says about this machine (the priors behind the ordering)

Facts established in M20's research pass; each shaped the ordering below:

1. **Every temp round-trips through the frame.** Spill-everything means
   ~5 cycles (store 1 + load 4) and three pipe classes (AGU + V store-data
   + AGU) per temp per use. This is the largest single overhead in emitted
   code, and it is everywhere.
2. **wrela's redundancy is spatial, not temporal.** The waste repeats
   across the program (4-word constant materialization, `narrow_to_width`
   LSL/LSR pairs, per-index bounds checks, `ADRP`+`ADD` per rodata ref,
   spill/reload), not across loop iterations. Inlining + redundancy
   elimination attacks it; unrolling does not.
3. **A76 is a hardware unroller.** 128-entry OoO window, 8 issue queues,
   ~0-cost predicted back-edges. Loop overhead is 2 instructions on ports
   the body rarely saturates. Static unrolling is a narrow, loop-specific
   tool here (SOG §4.4 memory copy), not a strategy.
4. **The abort branches are nearly free.** Always-not-taken check branches
   cost ~0 mispredict under the bias model — the emit-every-check doctrine
   is cheap on this core. Do not spend effort deleting checks for speed;
   spend it only where a check is *provably redundant* (dominating-check
   elision), where the win is words, not cycles.
5. **Cross-core cost is the machine's bill and its least-known number**
   (M20 item G: `DMB` + snoop are swept, unpriced by any source). Opts
   that reduce cross-core traffic have the highest ceiling and the widest
   error bars — they land under the ∀ sweep or not at all.
6. **Code already exceeds L1I** (93–98 KB text vs 64 KiB), so every
   code-growing opt is spending a budget that is already overdrawn until
   the hot-footprint term says otherwise.

## Tier 0 — measure first (no opt lands; days, not weeks)

**0a. Measured trip-count table.** As soon as M20 item C's `lane2-freq.txt`
lands, derive per-loop measured trip counts on the pinned workloads and
commit the table to this file. Several later decisions (4c, 4d, both
moonshots' sizing) key off whether hot loops run ~3 times or ~300.

**0b. Hot-block / hot-field profile.** Same sidecar, aggregated per fn and
per placed static. This is the input for 4a (field splitting) and the
per-core hot-footprint arguments throughout.

**0c. The partition adversary's numbers.** M20 item L already runs
fused-vs-split and rolled-vs-unrolled on synthetic multisets. Copy the
results here verbatim when they exist; they are the empirical check on
priors 2 and 3.

## Tier 1 — the register story (the largest expected win in the compiler)

**1a. Spill/reload forwarding (interim, static-shape).** Delete
`str x, [sp,#N]` / `ldr x, [sp,#N]` back-to-back pairs and forward through
the register. Purely local, lands on the flat gate, removes a 4-cycle load
and a word each site. This is the cheap 20% of 1c that requires no
allocator; it is also disposable once 1c lands — delete it then (losers
and superseded opts do not accumulate).

**1b. Dead frame-slot elimination (interim, static-shape).** Temps never
reloaded get no slot; shrink the frame and the `sub sp` immediate. Cuts
distinct `MemRef` keys, which the reuse-distance model now prices. Same
disposability note as 1a.

**1c. Register allocation, per function.** The real item. Linear-scan over
the existing per-fn MWIR — no SSA construction required for a first
version; wrela's exclusivity rules mean no aliasing analysis is needed to
prove a temp's live range. ~28 usable GPRs. Every temp that stays resident
deletes a store, a load, a V-pipe data uop, and two AGU uops per use.
Expected to dominate every other row in this file on the corpus.
  - **Gate note:** this *shrinks* words (spill code deleted), so it lands
    even under the old veto; under the new gate it is also the first big
    test of the forwarding-term sweep (a swept forwarding latency must not
    be what the win depends on — it will not be; the win is structural).
  - **Doctrine note:** this is the canonical "dumb version replaced by a
    provably equivalent one" — `diff-eval` is the equivalence oracle, and
    `dev` mode keeps the spill-everything reference alive forever.
  - **Blast radius:** every `asm-*` and `cost-*` golden, most boot
    transcripts' timing-independent surfaces unaffected. Own milestone.

**1d. Spill-to-VPR for the residue (SOG §4.3).** What still spills under
1c goes to the 32 NEON registers (~64 extra 64-bit slots) before touching
the stack. Vendor-recommended, directly measurable by the V-pipe
contention and forwarding terms. Depends on 1c existing (spilling *less*
first is strictly better than spilling *elsewhere* first).

**1e. Per-turn register residency (capstone).** Turns are non-reentrant,
single-input, dynamically-allocation-free, and short: allocate registers
across the whole turn body (entry method + its inlined callees, after
Tier 2) so a turn's working state never touches the frame at all. This is
the sized-down, achievable form of "everything lives in registers" — the
live-range firewall is the turn boundary, which the language guarantees.

## Tier 2 — inline and clean (where the spatial redundancy dies)

**2a. Shrinking inliner.** Inline where the callee is (i) single-call-site
or (ii) smaller than the call overhead it deletes (BL + frame setup + arg
spill/reload). Both cases *reduce* words — gate-legal even before the
words veto retires. No inlining heuristics beyond these two rules; the
point is the enabling effect, and freeze-style discipline keeps it from
becoming a tuning pit.

**2b. GVN/CSE + SCCP + DCE over the inlined bodies.** The passes the
inliner feeds. Exclusivity means loads are trivially CSE-able within a
turn (no aliasing writes to prove absent). Targets, concretely: repeated
constant materializations, repeated `ADRP`+`ADD` rodata addressing,
repeated bounds-check subexpressions, `narrow_to_width` canonicalizations
of already-canonical values.

**2c. Redundant `narrow_to_width` elision.** The LSL/LSR pair is dead when
the input is provably canonical (prior narrow of same width, in-range
literal). Static-shape, cheap, large site count.

**2d. Dominating-check elision.** Generalize BoundsElide from
literal-in-range to "a dominating identical check already proved this."
Wins **words** (the cycles were ~free per prior 4); still worth it because
words are the overdrawn budget. Requires care at abort-edge semantics so
`diff-eval` stays honest — the eliminated check must be *provably*
subsumed, never probabilistically.

**2e. Constant/addressing peepholes.** `MOVN`/bitmask-immediate forms
(NarrowImm's sequel), fold `add xN, sp, #off` into addressing modes,
branch-to-branch and branch-to-fallthrough cleanup, `STP`/`LDP` merging of
adjacent frame traffic (throughput note: X-form STP is 1/cycle vs 2/cycle
single stores — the port model prices whether merging wins per site, which
is exactly the non-obvious call the ruler exists to make).

## Tier 3 — branch shaping (cheap, now scoreable)

**3a. If-conversion to `CSEL`** — only where the bias model shows a
near-50/50 branch; a predicted branch converted to CSEL is a *loss*
(serializes on the condition, both arms execute), which is precisely the
mistake M19's `branch_penalty = 0` reasoning existed to prevent. The bias
term makes the profitable subset identifiable for the first time.

**3b. Code placement for the §4.8 rules.** Align entry points and hot
branch targets to 32 B; keep hot loops inside one aligned 32 B region;
cap branch density per region. Pure layout, no semantic change, priced by
the front-end terms item H added.

## Tier 4 — memory and layout

**4a. Hot/cold field splitting (AoS-preserving).** Group per-turn-touched
fields of actor state into the same 64 B line; push cold fields (error
counters, provisioning metadata) out. Uses 0b's profile; scored by the
reuse-distance + 4-way-conflict model. Excludes `@layout`/`@offset`
structs and device-visible layouts (the language already marks both).
Moves every `Field offset=` line and the layout goldens — own commit, own
clause, census re-check. **Not SoA** — settled 2026-07-29: SoA optimizes
field-across-elements access; wrela's hot pattern is element-at-a-time.
Do not relitigate without a workload that actually scans one field across
many elements.

**4b. Frame-slot alignment for the 16 B store rule.** Order frame slots so
hot stores do not straddle 16 B boundaries (SOG §4.5). Statically
decidable, near-zero cost, small steady win.

**4c. The one vendor-blessed unroll: the linear copy.** SOG §4.4's exact
recipe applied to the copy loop at
[runtime.wr:904](../stdlib/core/runtime.wr:904): unrolled wide `LDP`/`STP`
of Q registers, discrete non-writeback forms, 16 B-aligned stores. One
loop, bounded growth, the single place unrolling has a vendor argument on
this core.

**4d. LICM + measured-trip unrolling (conditional).** Only if 0a shows hot
loops with small measured trips (≲4) or hoistable invariants the ruler
prices as real. Footprint-capped, by measured trip count, never by
`@budget` bound — the bound is a termination ceiling, often 10× over
(e.g. `CORE_SLOTS = 32` on a 3-core machine). Full unrolling and
whole-program fusion are **settled-rejected** (2026-07-29): ~13k
iteration-bodies ≈ 520 KB against a 64 KiB L1I, break-even only above
99.9% redundancy at the large bounds, observation-discharged loops have no
N at all, and the compile-time lock breaks first. Do not relitigate
without new facts.

## Tier 5 — machine-level (highest ceiling, widest error bars)

**5a. Traffic-aware placement.** Feed item G's cross-core term back into
placement inference: co-locate actor pairs whose measured message traffic
is highest, subject to the per-core L1I/TLB budgets that same ruler
enforces. Placement currently packs on `(work, bytes)` with no traffic
term. This is the first opt where the *placement* is the optimization —
and because the snoop cost is swept, it lands only if it wins across the
whole uncertainty box, which is the gate working as designed.

## Moonshots (wrela-only physics; either could dwarf the ladder)

### M1. Co-located send → call fusion ("devirtualize the mailbox")

**The idea.** When placement proves sender and receiver share a core, and
the send's delivery can be scheduled immediately (mailbox empty at the
proof point, no intervening admission), compile `send` as a direct call
into the receiver's turn body — skipping enqueue, ring bookkeeping,
scheduler dispatch, and dequeue entirely. The turn abstraction stays (the
callee's turn still runs to completion, non-reentrancy is trivially
preserved by the call structure); only the *transport* is deleted.

**Why only wrela can do this.** Placement is sealed at image build, so
co-location is a static fact, not a dynamic guess; exclusivity means the
handoff needs no synchronization; and the recorder's schedule is
checkpoint-injected, so the fusion is legal exactly where no admission
decision could have interleaved — a condition the compiler can *prove*
from the image graph rather than approximate.

**Why it could be enormous.** It attacks prior 5 — the machine's dominant
and least-priced cost — from the direction with *no* error bars: a fused
send costs zero cross-core budget not because snoop latency is small but
because the traffic no longer exists. It is the opt that wins at every
point of any sweep box by construction.

**The hard parts (also the kill conditions).** Record/replay: a fused send
must either still emit its admission-order choice point or be proven
absent from the schedule space — if fusion changes the enumerable choice
sequence, it breaks the M6 design constraint and dies. Mailbox-depth
semantics and deadline/cancellation observation points must be preserved
bit-for-bit in transcripts. First falsifiable step: fuse exactly one
statically-provable pair in a synthetic two-actor golden and demand
byte-identical transcripts under both modes.

### M2. Boot as a comptime value ("the image ships pre-booted")

**The idea.** wrela boot is deterministic by construction — no discovery,
fixed layout, sealed graph. Run the boot sequence *in the comptime
evaluator* at image build, snapshot the post-boot machine state (rtdata,
mailboxes opened, pools initialized, init spans applied), and emit that
snapshot as the image's initial data. Boot on hardware becomes: load, set
PC to the scheduler loop. The entire init code path moves from every boot
to one compile.

**Why only wrela can do this.** The evaluator is already the normative
reference for the semantics (doctrine: evaluator before backend); boot
already produces a deterministic, transcript-pinned state; and the image
report already describes the memory layout the snapshot would fill. In a
language with runtime discovery, clocks, or allocation, this is
impossible; here it is *almost* just running existing machinery at a
different time.

**Why it could be big.** Deletes the init code from the hot image (text is
the overdrawn budget), collapses boot latency to load time (a product
number for a sealed-image OS where every update reboots), and shrinks the
schedule space the recorder must cover (boot's choice points vanish).

**The hard parts (kill conditions).** Device state cannot be snapshotted —
the boundary must fall exactly at "everything before first device touch,"
and drivers' reset/handshake sequences stay runtime. The evaluator must
provably agree with the backend on every snapshotted byte (`diff-eval` is
the oracle, extended to boot). If the agreeing-byte-for-byte obligation
turns out to require evaluator work proportional to re-implementing the
backend, it dies. First falsifiable step: snapshot only the zeroing +
init-span phase (already pure data) and diff the resulting boot transcript
against the status quo — byte-identical minus the elided init lines.

## Suggested first milestone cut

Tier 0 + 1a/1b/2a/2c/2e as one static-shape milestone (all shrink words,
all land on the flat gate, all cheap oracles); 1c as its own milestone
(the allocator); then re-plan from the measured tables. M1/M2 activate
only by explicit human decision — each is a plan of its own with its
first falsifiable step as item 0.

## Explicitly not on the ladder (settled 2026-07-29; new facts required)

| Rejected | Why (short form) |
| --- | --- |
| Full unrolling / whole-program fusion / mega-function per vCPU | size arithmetic vs 64 KiB L1I; ceilings ≠ trip counts; A76 unrolls in hardware; compile-time lock |
| AoS→SoA rewriting | wrong access pattern (element-at-a-time); no vectorizer to feed |
| Auto-vectorization | no NEON emission to speak of; cost model deliberately has no NEON rows (M20 freeze 1630) |
| Check deletion for cycles | checks are ~free under the bias model; only *dominance-proven* elision (2d), for words |
| Barrier removal/motion | correctness-load-bearing; freeze 1633 |
