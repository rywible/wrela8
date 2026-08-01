# Plan: codegen Pareto round 2 — delete the movs, kill the redundancy, fix the ruler

**Status: ACTIVE (2026-07-31).** Follows [codegen-pareto.md](codegen-pareto.md),
whose close measured −15.5 % on the product tier and, more usefully, showed
*why* that number is small. Decision block **1900–1999**.

**Old decisions do not bind this round.** The human's instruction is explicit:
do not preserve backwards compatibility, do not defer to a prior freeze that
measurement has since contradicted. Where a decision from M19/M20 or round 1
blocks something that is now known to be worth doing, **overturn it and say so
in a numbered decision here.** Golden churn is expected and is the
orchestrator's to re-pin.

## What round 1 actually showed

On the real appliance image, `dev` → `release`:

| | dev | release |
| --- | --- | --- |
| `movk` | 780 | 0 |
| `str` | 509 | 461 |
| `ldr` | 430 | 344 |
| `mov` | 0 | **161** |
| total words | 2660 | 1807 |

Three facts follow, and they set this round's agenda:

1. **The allocator relocates data movement rather than deleting it.** −134
   memory ops, **+161 register moves**: net *positive* on word count. The
   promised "every resident temp deletes a store and a load" did not happen.
   Both items E and F named coalescing as the missing piece and left it.
2. **The word win belongs to an opt that predates the plan.** `movk` 780 → 0
   is constant-materialization elimination, and `NarrowImm` alone accounts
   for 19 067 → 13 685 words of the product tier.
3. **95 % of modelled cost is `runtime`, not `app`** (22 134 vs 1108 on the
   appliance). There is no compute-heavy program in this repo for a codegen
   opt to win on, so the corpus cannot show what these opts are for. That is
   the same self-selection failure round 1's item H found, one level up.

## Decision 1910 — the parking rule, and what this round deleted under the old one

**The rule changed mid-round (2026-07-31, human).** "Losers are deleted, not
kept disabled" is replaced in CLAUDE.md by: *a refused opt is parked, not
deleted*. It stays in the tree, out of `RELEASE_OPTS`, carrying the
measurement that refused it, the **mechanism**, and the **named workload or
capability that would make it worth re-asking**. It must still compile and
pass `diff-eval`, so it cannot rot into a miscompile while parked.

The old rule cost real work twice inside this one session, which is why it
changed:

- **Item D's block layout** was deleted by item K on the measurement that it
  vetoes under every wiring available in the tree — and then item M's compute
  workload landed *hours later* and inverted the premise the deletion rested
  on (that no image has L1I headroom; the compositor has ~17 KB).
- **The inliner** was refused on a corpus that, until item M, contained no
  compute-heavy program at all — and was deleted before it was ever
  committed, so the numbers that re-ranked ladder 2a **cannot be reproduced
  from this repository**. `git log -S"OptId::Inline"` returns nothing.

Three things were deleted under the old rule and are restored, parked:

| what | deleted by | restored as |
| --- | --- | --- |
| `BoundsElide` | item L, decision 1970 | item N — **DONE**, [findings](codegen-pareto-2-N.md). Item M's compositor was measured as the re-ask condition and **refused it again**: −127 cycles, but all of it in the case's own `@test(runtime)` asserts, none in the kernel, whose every index is computed. The condition is now a capability, not a workload. |
| `blocklayout.rs` (item D's pass) | item K, decision 1956 | item O |
| the inliner | item J, decision 1935 | item P — **rebuilt**, since it was never committed ([findings](codegen-pareto-2-P.md)) |

Item P also settles a question the original measurement cannot answer: the
inliner's numbers were taken with it *in* the opt list, but the pipeline
order — whether it ran before or after `ConstProp`/`Gvn`/`Dce`, which is what
decides whether an enabling pass can enable anything — is not recorded
anywhere committed. An inliner measured after redundancy elimination is a
measurement of nothing.

## Items

### I. Register coalescing — delete the 161 movs

Make the allocator's `mov` pairs disappear instead of merely replacing
spill traffic. A copy whose source and destination do not interfere is one
register, not two and a move. Includes argument/return position: the home
of a value passed to a call should *be* the argument register (item F's F4
note), so the `mov` around every call site vanishes.

Overturn round 1's **decision 1765** (a temp needs two reads before
promotion) if coalescing makes single-read promotion profitable again —
1765 existed only because a single-read temp became a serial two-`mov`
chain, and coalescing is exactly what removes that.

**Oracle:** `mov` count on the appliance asm dump, before/after, recorded.
Both ∀ tiers. `diff-eval`. **A named boot case, run by you** (see the boot
rule below).
**Decisions 1900–1919.**

### J. The inliner, then GVN/SCCP/DCE — **DONE**, and it split

Findings: [codegen-pareto-2-J.md](codegen-pareto-2-J.md).

**2b landed and is the largest single win this backend has taken.**
`ConstProp`, `Gvn` and `Dce` join `RELEASE_OPTS`: **206 848 → 185 513**
proxy cycles and **167 269 → 150 087** emitted words over the shipped
list (−10.3 % on both), no case rising at any point of either tier's
residual box. On item M's compositor, **7 575 → 6 871** emitted words
(−9.3 %) and 31 168 B → 28 416 B of hot text, one whole text page.

**2a — the inliner — was built, measured and refused** (decision 1935).
It worked and it lost: `+221` cycles and `+308` words leave-one-out over
the shipped list, `+326` words on the compositor, `+0` on the appliance
because it has no customers there at all, and `+36` cycles even
restricted to single-call-site inlining where the body moves rather than
duplicates. Losers are deleted. The ladder's ranking of 2a as "the
largest parked win" is overturned.

The appliance moved by **3 words**, and that is the item's other finding:
decision 1932 keeps the shared runtime closure off limits (its bodies are
placeholders `layout.rs` replaces, and its block partition is a committed
Lane 2 measurement decision 1608 fails closed on), and the appliance's
application half is six methods. Item M's compositor is where a codegen
opt is visible on this tree — exactly what item M was added for.

**Decisions 1920–1938.**

### K. Fix the ruler's three known defects

1. **The divide-lo corner.** Independent `[5,20]` and `[5,12]` brackets put
   `(x=5, w=12)` inside the box, where a 32-bit divide scores *slower* than
   the 64-bit one it replaced — physically impossible on one divider.
   Correlated quantities are modelled as independent. Fix the correlation;
   this unblocks C4.
2. **`--stage=cost` and `--stage=report` disagree about hot text** for the
   same program (appliance: 8 256 B vs 91 456 B). One of them is wrong, or
   they measure different things and both are mislabelled. Reconcile, and
   make the ∀ gate read the column that describes the shipped image.
3. **The footprint term is order-invariant**, which is why round 1's item D
   could not be scored. Make it order-sensitive — charge for *density*, not
   only for overflow — so block layout is rankable. Then either wire item D's
   `blocklayout.rs` (it exists and is tested but deliberately unwired) or
   **delete it**, on the measurement.

Overturn M20's freezes where they are what is wrong. Every changed or added
row keeps M20 item A's provenance discipline (`source`, `mechanism`, `note`,
`ambiguity`, tier). Digests will move; that is the review surface.
**Decisions 1950–1969.**

### L. Delete the losers, land the blocked

- **Delete `BoundsElide`.** Byte-identical to `dev` on all four shipped
  programs; its whole measured effect is six microbenchmarks, the largest
  written for it. Doctrine: losers are deleted, not kept disabled. Delete the
  opt, its transform, and `cost-bounds-elide` if that case exists only to
  flatter it. If deleting it turns out to change shipped code after all,
  **that** is the finding — report it and stop.
- **Re-examine `WideImmForms`** once I and J have landed: its three
  remaining customers may be gone. Same rule.
- **Unblock B4 (branch-to-branch and branch-to-fallthrough cleanup).** It
  wins on all 15 cost cases and was reverted only because eliding a branch
  merges two emitted-word blocks while the Lane 2 MWIR partition stays
  finer — M20 decision 1608's bridge contract. **Overturn 1608**: carry block
  identity through the elision, or relax the bridge to a coarser join.
- **Land B2** (`ADR` for placed statics, rtdata/pooldata), deferred in round 1
  only because its oracle is a boot transcript.

**Decisions 1970–1989.**

### M. A workload the compiler can be judged on

There is no compute-heavy program in the tree. Add one — real work with
loops, arrays, arithmetic and calls, of the shape the flagship will actually
run (pixel/blit-like inner loops, fixed-point maths) — as a product-tier cost
case *and* a boot case, so codegen quality is visible in both the model and a
guest transcript. It must be a genuine program, not a benchmark written to
flatter an opt: state plainly what it computes and why that shape.

**Decisions 1990–1999.**

### Q. The configuration search — evidence, not an autotuner

**Premise, established before speccing this (decision 1911).** `apply_opts`
reads `opts.contains(..)` — **membership only**. Permuting `RELEASE_OPTS`
cannot change an emitted word, and `swapped_order_scores_same_as_release_opts`
already asserts it. Every ordering rationale in the tree ("`RegAlloc` last
because the allocator decides against the emitter's output", "`WideImmForms`
after `MaskCheck`", "everything the probe must see precedes `RegAlloc`")
documents the *pipeline's* hardcoded order; it does not cause it. So there is
no list-order search space, and building one means making the list drive pass
invocation — a **pass manager**, which M19 freeze 1402 forbids. Not proposed.

What is real, and what this item does:

**Q1. Subset search over every `OptId`, including the parked ones.** The
shipped list is a hand-accumulated subset, one opt per item, each admitted
against the list as it stood that day. Nothing has ever asked whether the
*subset* is right — and the opts are now known to compose **superadditively
on both tiers**, which is exactly the regime where hand-accumulation is
wrong. Parked opts are in the space by construction: parking exists so the
question can be re-asked, and this is the thing that re-asks it.

Exhaustive is not affordable (2^n over a 20-case corpus), so the search is
staged and **deterministic** — sorted iteration, no RNG, no wall clock, no
threads (`CLAUDE.md`'s determinism clause applies to it exactly as to the
compiler):

1. every single opt against `dev`;
2. every **pair** — n(n−1)/2, ~120 configs — because the known interactions
   are pairwise (`NarrowImm`+`RegAlloc`, `MaskCheck`+`WideImmForms`,
   `Frameless`+`TailCalls`);
3. greedy forward selection seeded by (1) and (2), then backward
   elimination, so a member that stopped paying once its neighbours landed
   is dropped;
4. the winner validated against the shipped list on the **∀ box, both
   tiers** — the search scores at the pinned point for speed, and a pinned
   point is not a gate.

**Q2. The one real order probe.** `mwir_opt::optimize` runs ConstProp → Gvn →
Dce in code. That is 6 permutations, plus the question of whether a second
`Gvn` after `Dce` pays. Cheap, and it is the only place in the compiler where
pass order is both real and free.

**Q3. Anti-overfit, built in rather than bolted on.** A search maximizes
against the corpus — that is the definition of overfitting, and it is the
failure this whole plan has been fighting since decision 1716. So: every
result is reported **per tier**, and no configuration may be recommended
unless it wins on the **product** tier, not merely on the total. The output
is a *report with mechanisms*, not a rewritten `RELEASE_OPTS`; adopting it is
the human's call.

**Explicitly not an ML model.** Twenty cases is not a training set; more
decisively, a learned heuristic cannot state a mechanism, and every decision
in this tree carries one. A deterministic search gives the same answer with an
explanation attached.

**Decisions 1912–1919.**
### P. The inliner, rebuilt and parked — **DONE**

Findings: [codegen-pareto-2-P.md](codegen-pareto-2-P.md).

`OptId::Inline` is back in the tree, wired, proved by `cargo xtask
diff-eval --inline`, and deliberately out of `RELEASE_OPTS`. **The
ordering question was real**: the inliner run *before*
`ConstProp`/`Gvn`/`Dce` and the same inliner run *after* them differ by
3.5× on cycles and 4.3× on words, so item J's un-recorded pipeline
position was a genuine gap (decision 1986). **And decision 1935 survives
it**: both positions lose in both whole-list framings, and the
enabling-order numbers land within ~16 % of item J's — which also
identifies item J as having measured the enabling order (decision 1987).
The refusal is now re-derivable from this repository, which it was not
before.

Two refinements. Rule (i) alone — body moves, callee deleted, nothing
duplicated — is a **wash** on the totals rather than a loss (+26 cycles /
+5 words leave-one-out), and is refused by exactly one case rising:
`cost-product-compositor`, under `CaseRose` (decision 1988). And the
mechanism is measured rather than argued: on the compositor a splice adds
+100 `str` and +41 `ldr` while `bl` does not move at all, so the call
sequence it deletes is repaid twice over — once by the abort paths the
duplicated bodies copy, once by the spills a longer live range costs the
per-function allocator. The named re-ask condition is therefore the
**allocator, not the corpus** (decision 1989).

**Decisions 1980–1989.**

## Rules for this round

- **Every item runs its own named boot case.** Round 1 barred agents from the
  hypervisor and a keystone item silently miscompiled the guest for hours.
  That trade is reversed: a filtered, read-only `cargo xtask golden
  --only-boot --filter <case>` is required before an item reports done.
  Never `--update`; the orchestrator re-pins.
- **Both ∀ tiers bind**, each opt asked over its own baseline (round 1
  decision 1717).
- **Never tune the cost model to make an opt win.** Item K changes the model
  *only* where it is provably wrong, with provenance.
- **A refused opt is parked, not deleted** (CLAUDE.md, 2026-07-31). It stays
  in the tree, out of `RELEASE_OPTS`, with the measurement that refused it,
  the mechanism, and the named workload that would justify re-asking. It
  must still compile and pass `diff-eval`. This replaces the "losers are
  deleted" rule this round was written under, and items J, K and L each
  deleted something under the old rule — see decision 1910.
