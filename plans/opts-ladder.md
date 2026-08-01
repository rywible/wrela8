# Optimization research backlog: candidates to pull in as needed

**Status: BACKLOG (2026-07-29).** **Not a plan and not scheduled.** This is
the catalogue of optimization candidates for the A76 ruler, kept so the
arguments from the 2026-07-29 design discussions are not re-derived later.
Every candidate is priced by the ruler [M20.md](M20.md) builds (block-grain
`f`, 8-pipe port model, real memory hierarchy, bias-derived branches,
cross-core terms, ∀ sweep), so nothing here can be evaluated before M20
closes.

**The Pareto subset was extracted into a real plan:
[codegen-pareto.md](codegen-pareto.md)** — register allocation, the no-ABI
items, hot/cold block layout, and the easy static-shape wins. Items marked
**[IN PLAN]** below live there now and are kept here only as context.
Everything else is parked, to be pulled in when measurement justifies it.
Decision block 1700–1799 belongs to the plan; a pulled-in item takes its
numbers from whatever plan adopts it.

**Pull-in priority when the plan closes** (revise against its measured
results, not against this ordering):

1. ~~**The inliner (2a) + GVN/SCCP/DCE (2b).**~~ **Pulled in and settled
   by [codegen-pareto-2.md](codegen-pareto-2.md) item J
   ([findings](codegen-pareto-2-J.md)), and it came out split.** 2b is
   real and large: `ConstProp` + `Gvn` + `Dce` are worth **−10.3 %** proxy
   cycles and **−10.3 %** emitted words over the whole shipped list, more
   than `NarrowImm` or `RegAlloc`. **2a is a loss on this backend** — the
   inliner was built, worked, agreed with `diff-eval` and three boot
   transcripts, and cost `+221` cycles / `+308` words leave-one-out over
   the shipped list, `+326` words on the compositor and `+0` on the
   appliance (it has no customers there). Even single-call-site inlining,
   where the body moves rather than duplicates, was `+36`. It is deleted
   (decision 1935). See 2a below for the re-ranking.
2. **Range propagation (Tier 7 beyond type-known widths).** Potentiates 2b
   and unlocks provable check elision.
3. **Block page tables (6a).** Plausibly the single largest lever in Tier 6,
   and it deletes a whole class of cost-model uncertainty rather than
   optimizing against it.
4. **SIMD + vectorizer (Tiers 9–10).** Gated on the pixels rung; when that
   activates, this becomes urgent rather than optional.
5. Everything else by opportunity.

The tier numbers below are **identifiers grouped by mechanism**, not a
schedule. See "Recommended sequencing" near the end for dependency order.

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
7. **There is no SIMD at all.** No NEON encodings beyond incidental
   `CostRule::Neon`, no vector MWIR, no vectorizer — while 06 §7 already
   commits the display path to "pure CPU (**NEON**)" rendering. Tiers 9–10
   close that gap; until they do, the machine cannot render its own
   flagship mode at the quality the contract implies.
8. **The no-ABI fact is the least-exploited structural advantage in the
   project.** AAPCS64 exists so strangers can call your code; a sealed
   image has no strangers. Tier 1's interprocedural allocator is the
   keystone item on this whole ladder.

## Why the ceiling is above LLVM (the thesis, in one table)

Full argument in [beating-llvm.md](beating-llvm.md); the short form,
because it drives the tier ordering. Hand-written assembly beats compiler
output for about ten reasons, and wrela structurally removes seven:

| Why hand asm wins | wrela | Tier |
| --- | --- | --- |
| Human knows the aliasing | **removed** (type-system fact) | 10 |
| Human ignores the ABI | **removed** (no external linkage) | **1** |
| Human knows alignment | **removed** (compiler owns layout) | 4, 9 |
| Human knows trip counts/shapes | **removed** (bounds, comptime sizes, measured trips) | 0, 10 |
| Human allocates registers globally | **removed** (no ABI + whole program) | **1** |
| Human uses exotic instructions | reducible (single target, no feature guards) | 6 |
| Human schedules for the µarch | mostly moot (A76 is OoO, 128-entry window) | — |
| Human knows value invariants | **partly removed** (checked arithmetic is a range oracle) | 7 |
| Human accepts unmaintainable code | *not* removed (compile time is a product number) | — |
| Human chose a better algorithm | *not* removed | — |

**The honest claim this licenses:** hand-asm-quality codegen *for the
algorithm you wrote*. Not algorithm discovery. That is still the whole
prize, because it means writing ordinary wrela gets you what writing
assembly would have.

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

**1c. Register allocation, per function.** **[IN PLAN]** The real item. Linear-scan over
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

### The no-ABI items (LLVM structurally cannot do these)

**No function in a sealed image is callable from outside it.** AAPCS64 —
x0–x7 arguments, x8 indirect result, x19–x28 callee-saved, x29 frame
pointer — exists so strangers can call your code. There are no strangers.
LLVM approximates this with `internal` + `fastcc` + LTO, but visibility
rules, symbol interposition, and plugin boundaries keep it partial.

**1f. Interprocedural register allocation.** **[IN PLAN]** One allocation problem over
the whole program, with a **custom convention per function** computed
globally rather than AAPCS64's fixed one. This is the *legitimate* form of
the "one function per vCPU" instinct: cross-call register residency
**without fusing code**, so the live-range firewalls that made fusion
counterproductive survive intact. Keystone item — 1c is a prerequisite,
not a substitute.

**1g. No callee-saved discipline.** **[IN PLAN]** The caller knows exactly which
registers the callee clobbers (whole program, no indirect calls, no `dyn`),
so conservative save/restore **disappears** rather than being minimized.

**1h. Frameless functions.** **[IN PLAN]** A function whose values all fit in registers
needs no stack frame: prologue, epilogue, and the `sub sp` all vanish.
Against a spill-everything baseline this is the largest per-function
constant-factor win available.

**1i. Arbitrary-arity register passing and multi-value returns.** **[IN PLAN]** Twenty
live values across a call if the allocator wants it; no x0–x7 limit, no
x8 indirect-result convention, no struct-return dance.

**1j. Universal tail calls + argument-specialized cloning.** **[IN PLAN]** Every
tail-position call becomes a jump, unconditionally — not an optimization
that sometimes applies. And because all call sites are known, clone a
function per call site's known-constant arguments **without inlining it**:
the enabling effect of inlining at a fraction of the size cost. Pairs with
2a; prefer cloning where inlining would grow words.

## Tier 7 — value ranges (automating "the human knows the invariants")

The least-explored axis, and pure profit. wrela has **sized integer types
with checked arithmetic**: if a program is accepted, every arithmetic
result is provably in range. That is a free range oracle LLVM must
reconstruct with `computeKnownBits` and routinely loses across calls.

**7a. Type-driven width selection.** **[IN PLAN]** The A76 payoff is concrete, large, and
verified from the SOG: `MADD`/`MSUB` **W-form is 2-cycle at 1/cycle
throughput**, while **X-form is 4-cycle at 1/3 throughput *and* stalls the
only M pipe for 2 extra cycles** (§3.6 notes 2/4). A multiply whose type
proves ≤32 bits must never emit X-form. Same for divides, which block that
same single pipe for 5–20 cycles. Cheap, immediate, provable — a good
warm-up item.

**7b. Range-proved check elision.** Propagate declared ranges to prove
overflow and bounds checks dead. This is the *provable* subset of check
removal, which is the only permitted subset (see 2d and prior 4).

**7c. Range-proved canonicalization elision.** `narrow_to_width`'s LSL/LSR
pair is dead whenever the range proves the value already canonical —
supersedes 2c's syntactic version with a semantic one.

**7d. Range-driven representation choice.** A value proven < 2^31 lives in
a `W` register and stores as 4 bytes, feeding the compressed handle packing
in 6m. A value proven < 2^8 enables the SWAR paths in 6o.

## Tier 8 — frequency-driven code layout (PGO, without PGO)

LLVM's PGO needs an instrumented build, a training run, and a profile that
goes stale. wrela's Lane 2 sidecar is committed next to the source and
validated by Lane 3 host agreement — so every decision here is *ordinary*,
not opt-in.

**8a. Basic-block hot/cold layout.** **[IN PLAN]** Pack the measured hot path
contiguously; move cold blocks (abort paths, error handling, rare branches)
out of the hot region entirely. **This is the cheapest large win on the
ladder and the one that makes every other code-growing opt affordable**,
because it attacks the 93–98 KB-text-vs-64 KiB-L1I problem directly: static
size stops mattering once the *hot* subset is dense. Do this early.

**8b. Fallthrough = measured-likely path**, on every branch. Free, and
pairs with 3a's bias data.

**8c. Call-site-frequency-driven specialization** instead of heuristic
inlining thresholds — the frequency input 2a and 1j should consult.

## Tier 9 — SIMD infrastructure (prerequisite for Tier 10)

Nothing here is an opt; it is the missing backend capability. 06 §7 already
commits the display path to NEON rendering, so this is closing a gap
between the machine contract and the compiler, not adding a feature.

**9a. NEON encodings in `encode.rs`.** A bounded set, not the whole ISA
(freeze 1630 — model and emit only what is used): `LD1`/`ST1` vector
load/store, `LDP`/`STP` of Q registers (also wanted by 4c), integer
`ADD`/`SUB`/`MUL`/`MLA` on vectors, logical `AND`/`ORR`/`EOR`, shifts,
`CMEQ`/`CMGT` compares, `DUP`, `EXT`, and the reduction ops (`ADDV`,
`UMAXV`). `enc_ldaxr_w`'s presence-but-unused precedent shows the shape.

**9b. Vector types in MWIR + codegen.** New MWIR instruction forms over
fixed-width vector temps, mapped to `V0`–`V31`. Register allocation must
learn the V bank — which composes with 1d (spill-to-VPR) since both need V
registers to be first-class rather than scratch.

**9c. Cost-model NEON rows.** M20 inventory **row 35's trigger condition
has fired**: wrela now emits FP/ASIMD, so the freeze that declined those
rows is satisfied by adding them, not violated. The SOG's ASIMD tables are
already extracted — transcription, not research. Two inventory rows flip
from N/A to **live** the moment 9a lands, and both must be modelled here:
  - **row 32, region-based fast forwarding** (SOG §4.7): +1 cycle when
    producer and consumer are in different forwarding regions. Real once
    vector chains exist.
  - **row 33, the §4.2 dispatch stall**: a V-pipe uop with more than one
    quad-word source previously written as single words stalls dispatch
    3 cycles. Avoidable by construction if 9b never emits S-register
    writes feeding Q reads — make that an invariant, not a hope.
  - Also now live: **store-data uops share the V pipes**, so vector work
    contends with every scalar store in the program. The port model
    already knows this (M20 item E); vectorized code makes it bite.

**9d. Language-surface decision (settle in item 0, do not drift).**
Recommended: **no language surface at all.** Vectorization is an *as-if*
transformation — `dev` mode stays scalar and remains the correctness
reference, `diff-eval` proves equivalence, and the docs gain a sentence in
04 §5 rather than new syntax, types, or intrinsics. Rejected: explicit
vector types (large 02 change, new sema, new goldens) and NEON intrinsics
(a language surface for a codegen concern, and adjacent to the inline-asm
prohibition). **The open question this forces:** does the software
rasterizer rely entirely on auto-vectorizing its scalar loops, or does the
stdlib need a vector-shaped API? Answer it *before* the pixels rung
designs its renderer, because the answer shapes that renderer.

## Tier 10 — the vectorizer (where ffmpeg parity is defensible)

**Why LLVM's auto-vectorizer produces cautious, bloated code** — four
things it *must* emit:

1. runtime alias checks guarding the vector loop;
2. a scalar fallback loop for when those checks fail;
3. a remainder loop for `n % VF`;
4. an alignment peeling loop.

**wrela removes all four structurally**, which is the whole argument:

**10a. No alias checks, no scalar fallback.**
`values.exclusivity.no-overlap` makes non-aliasing *checked*, so
vectorization legality is a type-system consequence rather than an analysis
that usually fails. Neither guard nor fallback is ever emitted.

**10b. No remainder loop — and the trick only wrela can play: the compiler
picks the size.** Array sizes are comptime and **pool sizes are chosen at
image build**, so the compiler can round a pool *up* to a SIMD-friendly
multiple and delete the epilogue entirely. LLVM can never change your
array's size. Report the rounding in the image report so the footprint
cost is visible, and let the ruler price size-vs-epilogue per array.

**10c. No alignment peeling.** Alignment is a build-time fact the compiler
chose (4b, 9a's `LD1` alignment preferences), not a runtime unknown.

**10d. No trip-count dispatch.** `@budget` bounds and measured trips
(Tier 0a) replace LLVM's `if (n < VF) goto scalar`.

**10e. Reduction and idiom recognition**, informed by Tier 7 ranges: which
lanes can overflow decides whether a reduction needs widening, and
accumulator expansion (the one surviving classical unroll motivation) is a
vectorizer decision here rather than a separate unroll pass.

**The resulting output shape is *just the vector loop*** — which is exactly
what hand-written assembly looks like, and precisely why ffmpeg's asm is
smaller and faster than compiler output for the same algorithm: it is not
carrying four contingencies that cannot occur. The wrela vectorizer does
not need to be cleverer than LLVM's. **It needs less to be afraid of.**

**Gate note.** Vectorization is frequency-dependent and code-growing, so it
lands under veto-then-rank overall with the per-core text budget enforced —
never on the flat gate alone. And it is the first opt where `diff-eval` is
doing heavy lifting as the equivalence oracle rather than a formality;
budget fuzzing effort accordingly.

## Tier 2 — inline and clean (where the spatial redundancy dies)

**2a. Shrinking inliner. MEASURED AND REFUSED** — codegen-pareto-2.md
item J, decision 1935. The rule above was implemented exactly as written
(single call site, or a body no larger than the call sequence it
deletes, no other heuristics) and the premise "both cases *reduce* words"
is **false on this backend**. Every temp a splice moves becomes a caller
frame slot under spill-everything, and item K's footprint term now
charges for density, so a merged body pays twice: `+308` words and `+221`
cycles over the shipped list, `+326` words on the compositor,
`+36` cycles even for single-call-site-only. Re-rank 2a **below** 2b and
mark it blocked on a register allocator that survives the splice, not on
being built. The numbers are in [codegen-pareto-2-J.md](codegen-pareto-2-J.md).

**2b. GVN/CSE + SCCP + DCE. LANDED** — codegen-pareto-2.md item J,
decisions 1924–1926. It did **not** need the inliner to feed it: scoped
to an extended basic block, over application code only, it is worth
`-10.3 %` on both cycles and words over the shipped list, and `-9.3 %`
emitted words on item M's compositor. SCCP is `ConstProp` — MWIR is not
SSA, so the sparse algorithm has nothing to be sparse over. Loads and
aggregates are deliberately *outside* the whitelist rather than
CSE-able: `SetField`/`IndexSet`/`MemStore` write through a base temp, so
exclusivity is not the same as no-aliasing at MWIR grain. Residual, and
it is the biggest single block of dead words left in both shipped
images: the shift **count-range check**, 19 emitted words per site at
every shift whose count is not a folded constant — 11 sites in the
shipped compositor, ≈ 3 % of it. That needs an
unchecked-shift form in `mwir::Inst` and is 2c/2d work, not 2b.

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

## The work-vs-time hazard (read before Tier 5 or either moonshot)

**The gate measures work, not time, and a placement opt can game that.**
`cost(P, W) = Σ_b f_W(b) × s(b)` is a **sum over the program's work**. On a
one-core machine work and time coincide; on three cores they diverge
completely. Packing every actor onto core 0 strictly *minimizes* total
work — no snoop, no `DMB`, every send fusable under M1 — while tripling
makespan. And the gate cannot see it: `CaseDelta`
([win.rs:36](../crates/wrela-compiler/src/opts/win.rs:36)) carries totals
and words only, with **no per-core field**, even though the report already
prints `Core n=0 proxy_cycles=… max_turn_proxy=…`
([appliance report:191](../tests/golden/appliance/expected/report.txt:191)).
The data exists; the gate ignores it. A degenerate all-on-core-0 placement
would pass every check in M20 and be a large real-world regression.

**Containment: the hazard is entirely in 5a, not in M1.** M1 *consumes*
placement — it fuses sends that are already co-located and cannot move an
actor. If M1 lands and 5a never does, there is no gaming vector: placement
comes from source annotation or existing inference, and M1 harvests
whatever co-location happens to exist. **So 5a is gated on the fix below
and M1 is not.**

**Existing counter-pressure (friction and visibility, not a gate).**
Packing all actors on one core sums their hot text into a *single* 64 KiB
L1I and 48-entry I-TLB budget (M20 item F), which pushes back hard; the
virtio-blk driver is pinned to core 0 regardless; placement is a
report-visible, golden-pinned artifact (`Placement id=… core=…
source=inferred`), so a collapse shows up in the diff; and `cores=N` is a
sealed image fact, so declaring 3 and using 1 is legible. None of these
*refuse* the degenerate — they only make it visible and partly expensive.

**Fix, phase 1 (required before 5a): veto on `max_core` rising.** Add
per-core totals to `CaseDelta` and refuse any candidate that raises the
maximum per-core cost, even when it lowers the sum. Cheap — the dump
already computes it — and it kills the degenerate directly.

**Known limitation of phase 1, recorded so it is not mistaken for
correctness:** `max_core` assumes per-core work *overlaps*. For a strictly
serial chain (A sends to B, B replies to A, alternating), splitting across
cores overlaps nothing and adds cross-core latency, yet `max_core` scores
the split as cheaper — so phase 1 *over-credits splitting a serial chain*.
That errs toward refusing good co-locations rather than allowing bad ones,
which is decision 1609's direction, so it is the acceptable dumb version.

**Fix, phase 2 (only if 5a is actually pursued): critical path over the
sealed message graph.** The correct metric is the longest path through the
actor wiring, where same-core edges cost dispatch (or ~0 when M1 fuses
them), cross-core edges cost snoop + barrier, and independent subgraphs
overlap. The graph is sealed at image build, so this is computable rather
than estimated. It is a **cost-model extension, not a placement
heuristic** — it belongs behind the same provenance and ∀-sweep discipline
as everything else in M20, and it is a plan of its own.

## Tier 5 — machine-level (highest ceiling, widest error bars)

**5a. Traffic-aware placement. Blocked on the `max_core` veto above.**
Feed item G's cross-core term back into placement inference: co-locate
actor pairs whose measured message traffic is highest, subject to the
per-core L1I/TLB budgets that same ruler enforces. Placement currently
packs on `(work, bytes)` with no traffic term. This is the first opt where
the *placement* is the optimization — which is exactly why it is the first
opt that can game a work-only metric. Do not start it before phase 1
lands; prefer waiting for phase 2.

## Tier 6 — single-target ISA arcana (v8.2-A + A76 + sealed image)

The payoff of targeting exactly one ISA/extension set with a sealed image:
tricks a portable compiler must guard behind runtime checks, feature
detection, or conservative layout assumptions become *unconditional* here.
First, the honest envelope — what this core has and does not have, so
nobody chases ghosts: A76 is v8.2-A with crypto, CRC32, LSE atomics
(v8.1), FP16, and dotprod. **No SVE, no pointer auth (v8.3), no LDAPR/
RCpc, no BTI, no MTE.** LSE exists but atomics are settled-out (decision
1500) — noted for whenever a future rung reopens that door, because LSE
`LDADD` beats an exclusives loop soundly.

Each item below fights the ruler like everything else; most shrink words
and land on the flat gate. Ordered by expected value.

**6a. Block-mapped page tables: delete the TLB problem by construction.**
The guest owns its EL1 translation regime, the layout is fixed and
published (06 §2), and the L1 D-TLB holds 4 KiB–512 MiB pages. Map the
image and DRAM with 2 MiB (or larger) block entries and the entire
machine fits in a handful of L1 TLB entries — the I-TLB span term (M20
item F), the L2-TLB walk sweep dimension, and the T3 curve's page-walk
tail all become near-moot *by construction rather than by modelling*.
First step is descriptive, not code: record how boot currently maps
memory; if it already uses blocks, update item F's TLB terms to say so;
if it uses 4 KiB pages, this is likely the highest single-lever item in
this tier. Attribute boundaries (device vs normal) force a few splits;
the layout already segregates them.

**6b. [IN PLAN]** `ADR`-only rodata addressing: the sealed layout makes `ADRP+ADD`
pointless.** Every rodata reference today is a 2-instruction `ADRP`+`ADD`
pair plus a reloc. `ADR` reaches ±1 MiB; the *worst* pinned image spans
~99 KB from first code byte to last rodata byte
(`boot-receipt-handoff`: code 0x40500050 + 97688, rodata 0x40517de8 +
1574) — 10× inside range. `enc_adr` already exists
([encode.rs:851](../crates/wrela-compiler/src/encode.rs:851)). Replace
the pair with one `ADR`; **fail closed at layout time** if any site's
distance ever exceeds ±1 MiB (the sealed image makes this a link-time
proof, not a hope). Halves every rodata access, deletes a reloc class,
shrinks words — flat-gate legal.

**6c. `DC ZVA` + `STP XZR, XZR` zeroing.** `DC ZVA` zeroes a full 64 B
line without read-for-ownership — no fetch, no store-buffer occupancy,
one instruction per line. A portable compiler must read `DCZID_EL0` and
handle the trap-disabled case; wrela owns EL1 and the VMM, so ZVA=64 is a
machine constant and its enablement is part of the sealed configuration.
Use it for the boot zeroing loops (`__wrela_rt_primary_boot`'s stripe
zeroing, `LANE2` reset) and any pool/page zeroing; use `STP XZR, XZR`
(16 B/insn, no materialized zero) for sub-line tails. Also the degenerate
form everywhere: `STR XZR` for single zero stores instead of
materializing 0 into a register.

**6d. One-way barriers: `LDAR`/`STLR` instead of `DMB` + plain access.**
The four cross-core publish/acquire sites pair a full `DMB(ishst/ishld)`
with a plain store/load. Acquire/release accesses are the
architecturally-intended shape: `STLR` orders only what precedes it,
`LDAR` only what follows, instead of a full fence on everything in
flight. The encodings already exist, unused
([encode.rs:233–261](../crates/wrela-compiler/src/encode.rs:233)).
**This is not barrier removal** (freeze 1633 untouched) — it is
replacement with equivalent-or-stronger ordering for these exact sites,
strictly weaker global serialization. It is also a `docs/language/` +
ledger change (`@dmb` is a language surface; either the intrinsic set
grows acq/rel forms or lowering recognizes the `@dmb`+access idiom), and
its magnitude is T5 like every other cross-core cost — the *mechanism*
argument (one-way ≤ full fence) is what lands it, under the ∀ sweep.

**6e. [IN PLAN, partly]** `UBFX`/`SBFX`/`BFI` for `narrow_to_width` and field access. The
LSL/LSR canonicalization pair is `UBFX` in one instruction — same 1-cycle
throughput-3 I-port class, half the words, and it composes with Tier 2c
(elide when provably canonical, `UBFX` when not). `TBZ`/`TBNZ` similarly
collapse mask-test-branch sequences to one instruction for tag and flag
dispatch; `CBZ` exists in the encoder already.

**6f. `CCMP` chains and flag-folding.** Compound conditions
(`a && b && c`) become `CMP; CCMP; CCMP; B.cond` — one branch, no
short-circuit branch tree, 1-cycle throughput-3 each. Pair with using
`ADDS`/`SUBS` to fold a comparison into arithmetic that was happening
anyway. Fewer branches also relieves the §4.8 four-per-32 B density rule.

**6g. Strength reduction with the port quirk as the rule.** The arcane
fact: shifted-operand `ADD`/`SUB` with **LSL ≤ 4 stays 1-cycle
throughput-3 on I; LSL > 4 or any LSR/ASR/ROR drops to 2-cycle
throughput-1 on M** (SOG §3.4). So ×3/×5/×9/×17 are single cheap
instructions, ×24 is two (×3 then `LSL #3`), and the boundary between
"free" and "M-pipe" is knowable per constant. Likewise every
constant-divisor `SDIV`/`UDIV` (5–20 cycles, blocks the only M pipe)
becomes multiply-high + shifts, and every power-of-two ring/pool size
becomes `AND`. Where a size is *almost* a power of two, rounding the pool
up buys the cheap index math — a layout/speed trade the ruler can now
price.

**6h. `RBIT`+`CLZ` pending-vector scan.** Where the scheduler scans for
ready work bit-by-bit or slot-by-slot, `RBIT`+`CLZ` (both 1-cycle,
throughput 3) computes find-first-set in 2 instructions and jumps
straight to the winner. Applies wherever the pending vector is a bitmask;
if it is currently an array walk, making it a bitmask is the enabling
layout change and should be priced as a pair.

**6i. Count-down loops: fold the trip counter into the induction.** An
accepted `@budget` emits a hidden counter (increment, compare, abort
branch) beside the loop's own induction. Counting the induction *down*
from N with `SUBS` makes the flags free and one `B.NE`/abort-check serve
both purposes — the fail-closed bound survives, expressed through the
loop's own arithmetic. Semantics preserved (the abort still fires at
N+1); this is codegen shape, not a bound change — but it touches the
trip-counter contract (02 §8.1), so it cites the clause and pins a
golden.

**6j. [IN PLAN]** Text placement constants. Two free wins from owning the layout:
start the code section on a 2 MiB-aligned base so every branch and target
share one 2 MiB region (SOG §4.8 — current base 0x40500050 is not
2 MiB-aligned); and have Tier 3b's 32 B alignment done against that same
base. One constant in the layout, every address in every golden moves —
its own commit, like every layout move.

**6k. [IN PLAN]** `ADR` reach extends past rodata to the runtime data sections.
Follow-on to 6b, verified: rtdata (0x40540000) and pooldata (0x405401f8)
sit ~256 KB from the code base — inside `ADR`'s ±1 MiB. So placed-static
address materialization (today `MOVZ`+`MOVK`s or `ADRP`+`ADD`) can be one
`ADR` for every section except `pages` (0x40000000, ~5 MB away — keep the
absolute form there). Same fail-closed link-time range proof as 6b.

### Representation arcana (overloading bits, sealed-map edition)

The second family the question asked for: the 1 GiB fixed map and owned
EL1 make several bit-overloading tricks *architecturally free* here.

**6l. Top-Byte Ignore: 8 tag bits in every pointer, enforced by hardware
config wrela owns.** TBI (`TCR_EL1.TBI0`, base v8.0) makes the MMU ignore
bits 63:56 of data addresses. A portable compiler cannot assume it; wrela
*sets* TCR_EL1, so it is a machine-contract fact (06 §2 addition). Every
data pointer carries 8 metadata bits that cost **zero masking on
dereference** — tags are checked only where a check is wanted. Candidate
payloads: pool-slot generation counts (making stale-handle aborts a
1-instruction `CMP` on the tag — a *strengthening* of fail-closed, not a
speed trick), handle kind tags, owning-core ids. Docs + ledger change;
the language's handle representation is a normative surface.

**6m. The whole machine fits in 31 bits: compressed pointers for free.**
Guest-physical runs 0x40000000–0x7FFFFFFF — every address fits a `u32`
with **zero decompression**: `LDR W` zero-extends to the full pointer.
Pointer-bearing runtime structures (ring slots, queue entries, turn
records) can store 32-bit absolute addresses and double their density per
cache line — which the reuse-distance model now prices directly. Combined
with 8-byte alignment (3 low bits) and TBI (8 high bits), one 64-bit slot
honestly holds a pointer plus ~⁠35 bits of metadata: `{addr[31], gen[8],
tag[8], len[16]}` in one register, one load.

**6n. [IN PLAN]** Checked-narrow overflow via bitmask-immediate `TST`. Codegen's
own invariant states it: "Unsigned: overflow iff the high word is
nonzero." High-mask constants (`0xFFFFFFFF00000000`, `0xFFFFFFFFFFFFFF00`,
…) are all encodable AArch64 bitmask immediates — a rotated run of ones —
so the check is one `TST` + `B.NE`, no canonicalize-and-compare dance and
no constant materialization. **The customer is every checked narrow op in
every program** — the single widest check surface the emit-every-check
doctrine produces. Signed forms use `CMP x, w, SXTW`-shaped
sign-extension compares. This is arguably the highest-value item in the
whole tier because its site count is enormous and it shrinks words.

**6o. SWAR with free constants.** The classic byte-parallel-in-a-GPR
tricks (`haszero(v) = (v − 0x0101…) & ~v & 0x8080…`, byte broadcast,
2-digits-at-a-time decimal formatting) all depend on repeating-pattern
constants — which AArch64 bitmask immediates encode in **zero extra
instructions**, where x86 pays a 10-byte `mov` each. Customers: the
transcript/console formatting paths (decimal emission, line scanning) —
hot in every boot golden by construction. Related cheap win the dump will
show anyway: the runtime builds console lines byte-by-byte
(`TEST_LINE_BUF.bytes[i] = …` one store each); adjacent-constant-store
merging into word/`STP` stores is Tier 2e's peephole family and cuts
those sites ~8×.

**6p. Crypto units as non-crypto ALUs — recorded, awaiting a customer.**
The honest framing: these are real and fast, and wrela currently has no
consumer, so they are *recorded* (fail-closed doctrine: no speculative
machinery) rather than scheduled.
  - **AESE/AESMC fused pairs as a 128-bit mixer.** A76 fuses adjacent
    AESE+AESMC (SOG §4.6): ~1 fused uop per cycle on V0, 2-cycle latency
    — two rounds give avalanche-quality mixing. This is the aHash/GxHash
    trick. Customer appears the day the stdlib grows a hash-keyed
    structure; determinism is fine (fixed key, deterministic input).
  - **`PMULL` bit-spreading: GF(2) squaring is a 1-instruction Morton
    spread.** Carryless `PMULL(x,x)` places bit *i* at position *2i* —
    the exact interleave step of Z-order curves. Customer: framebuffer
    tiling when Pixels activates; Z-order tiles turn 2D locality into the
    1D locality the reuse-distance model rewards.
  - **`CRC32C` chains at 1/cycle with late-forwarding** (8 B/cycle
    checksums, M port): the receipt/transcript checksum primitive if one
    is ever wanted.

**Not applicable / handle with care, recorded so nobody hunts for them:**
NaN-boxing (no dynamic typing); non-temporal `LDNP`/`STNP` (hint largely
inert on A76 — measure before believing); software `PRFM` (competes with
the unmodelled hardware prefetcher; only worth trying in the 4c copy
loop); `WFE`/`SEV` parking (energy and snoop win, but wake timing
interacts with the recorder's checkpoint injection — needs its own design
note before anyone touches the park loop).

**Asked and answered (2026-07-29), so they are not re-litigated:**
  - **FEAT_LPA (52-bit PA): not on this core, and maximally irrelevant.**
    Cortex-A76 implements a **40-bit** physical address; FEAT_LPA arrives
    in later cores. And wrela's machine is 1 GiB at a fixed base — 31
    bits of address on a sealed map. Nothing here would change even on a
    core that had it. (The only adjacent nugget: PTE bits 58:55 are
    software-defined and hardware-ignored — free metadata bits — but
    under 6a's block mappings the whole machine is a handful of PTEs, so
    there is nothing worth tagging.)
  - **FEAT_LSE: available (v8.2 mandates it), and the door stays closed —
    but for a better reason than "decision 1500 said so."** LSE's
    headline win is replacing LL/SC lock loops, and wrela has **no locks
    and no LL/SC loops to replace** — exclusivity, SPSC rings, per-core
    striping, and checkpoint-injected admission exist precisely so that
    no two cores ever contend on a word. Striping (decision 1500) is
    strictly better than even LSE *far atomics* (the arcane DSU form
    that executes `LDADD` at L3 to avoid line ping-pong): a striped
    counter generates zero coherency traffic, a far atomic still
    generates some. If a future construct genuinely needs multi-producer
    admission, LSE is the primitive to reopen the door with — and its
    cost will be T5, since the SOG prices no atomic at all.
  - **RAS: the host's business, and replay is the better detector.** Pi 5
    ships non-ECC LPDDR4X, so DRAM RAS coverage is absent on the
    flagship; whatever cache ECC BCM2712 enabled is T5. Physical RAS
    errors surface to the *host* Linux/VMM, not the guest, and a RAS
    event is inherently nondeterministic — the physical world intruding
    on a deterministic machine. The doctrine answer: a hardware error is
    **fatal and fail-closed**, recorded by the recorder as a terminal
    event, never "handled" (recovery would be a nondeterminism source).
    And wrela already owns a stronger serviceability primitive than RAS:
    **byte-identical replay is a bit-flip detector** — if a divergence
    does not reproduce under replay, the hardware did it, with perfect
    attribution. RAS registers only tell you sooner and name the unit;
    the VMM may *read* them on a fatal event for the post-mortem line,
    and that is the entire integration.

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

**It does not create the placement hazard, but it does create a programmer
incentive.** M1 cannot move an actor, so it cannot game placement (see the
hazard section). It does mean co-locating a chatty pair gets measurably
faster, which is a legitimate design tradeoff made with `core=K` in hand —
not gaming, provided the programmer can *see* it. So M1 owes the report a
line per send site recording fused-vs-transported, or the incentive is
invisible and the tradeoff is being made blind. Treat that report line as
part of M1's deliverable, not a nice-to-have.

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

### M3. Verified superoptimization ("synthesize our own peephole tables")

**The idea.** For the measured-hot basic blocks only — Lane 2 says which,
and there will be few — search the space of equivalent instruction
sequences and keep the cheapest under the M20 cost model. The search is
**offline**; its output is a table of ordinary in-code rewrite patterns, so
build-time compile cost is zero and the compile-time lock is untouched.

**Why wrela can and LLVM effectively cannot.**
  - **One target.** A rewrite need not be valid or profitable anywhere
    else. LLVM's peephole patterns must generalize across every backend.
  - **A perfect equivalence oracle.** `diff-eval` plus byte-identical
    deterministic replay makes "are these sequences equivalent?" *decidable
    by execution* on this machine. This is the piece every superoptimizer
    struggles with, and wrela built it years ago for testing reasons.
  - **A real cost model to rank candidates** — that is what M20 is.
  - **A tiny measured hot set.** Superoptimization fails on whole programs
    and succeeds on kernels; Lane 2 names the kernels.
  - **Offline is explicitly allowed** by the cleverness budget, provided the
    output is a committed, locked artifact.

**Why it could be the "god tier" item.** LLVM's peephole tables were
hand-written over twenty years. wrela could **synthesize its own, verified,
for its one target** — which is a credible path to patterns no human would
have thought to write. That is the actual definition of beating hand asm.

**Kill conditions.** If the search space is intractable at realistic block
sizes even offline, it dies. If discovered rewrites are all special cases
rather than stable patterns, the table never converges and it dies. Do not
start before Tiers 1 and 8 exist — a superoptimizer optimizes whatever the
rest of the pipeline hands it, and handing it spill-everything code wastes
the search. **First falsifiable step:** take the single hottest block in
`boot-actors`, enumerate 3-instruction equivalents under the cost model,
and see whether anything beats what codegen emits today.

## Recommended sequencing

Tier numbers are **identifiers grouped by mechanism**, not a schedule. The
dependency order that matters:

1. **Tier 0** — measure. Nothing below is well-founded without the
   trip-count and hot-block tables.
2. **Tier 8a** — hot/cold block layout. Cheapest large win; makes every
   later code-growing opt affordable against the L1I budget. Do it early
   even though its tier number is high.
3. **Static-shape milestone** — 1a, 1b, 2a, 2c, 2e, 6b, 6k, 7a. All shrink
   words, all land on the flat gate, all have cheap oracles. 6b (`ADR`-only
   rodata) and 7a (W-form multiply) are the standout ratio items.
4. **The allocator milestone** — 1c, then 1d. Own milestone; large golden
   blast radius.
5. **The no-ABI milestone** — 1f–1j, then 1e. The keystone, and the most
   clearly LLVM-impossible work in the project.
6. **Tier 7 + Tier 2b** — ranges and redundancy elimination, which
   potentiate each other.
7. **Tier 9** — SIMD infrastructure. Gated on the pixels rung wanting it,
   or on 9d's open question being answered, whichever comes first.
8. **Tier 10** — the vectorizer. Only after 9, and only once something in
   the stdlib has a real data-parallel loop worth vectorizing.
9. **Tiers 4, 5, 6 remainder, M1, M2** — by opportunity and appetite.
10. **M3** — research. After 1 and 8, never before.

## Suggested first milestone cut

Tier 0 + 1a/1b/2a/2c/2e as one static-shape milestone (all shrink words,
all land on the flat gate, all cheap oracles); 1c as its own milestone
(the allocator); then re-plan from the measured tables. M1/M2 activate
only by explicit human decision — each is a plan of its own with its
first falsifiable step as item 0.

## Explicitly not on the ladder (settled 2026-07-29; new facts required)

| Rejected | Why (short form) |
| --- | --- |
| Full unrolling / whole-program fusion / mega-function per vCPU | size arithmetic vs 64 KiB L1I; ceilings ≠ trip counts; A76 unrolls in hardware; compile-time lock. **Note:** 1f delivers this instinct's actual payoff (cross-call register residency) without fusing anything |
| Blanket AoS→SoA rewriting | wrong access pattern for actor state (element-at-a-time). **Superseded in part:** decide layout **per array** from measured access (4a) once Tier 10 exists and a field-scanning workload appears — a rasterizer's pixel buffers may qualify where actor state never will |
| ~~Auto-vectorization~~ | **no longer rejected — now Tiers 9–10.** The prior reason (no NEON emission, no cost rows) was a statement about the backend, not about the idea; M20 row 35's trigger has fired |
| Explicit vector types / NEON intrinsics as a language surface | 9d — vectorization is an as-if transform; `dev` mode + `diff-eval` are already the right oracles, so the language does not need to change |
| Check deletion for cycles | checks are ~free under the bias model; only *dominance-proven* (2d) or *range-proven* (7b) elision, and the win is words |
| Barrier removal/motion | correctness-load-bearing; freeze 1633 |
| Algorithm discovery | no compiler does this. The claim is hand-asm quality *for the algorithm you wrote* |
