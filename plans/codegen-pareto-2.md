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
| `BoundsElide` | item L, decision 1970 | item N |
| `blocklayout.rs` (item D's pass) | item K, decision 1956 | item O |
| the inliner | item J, decision 1935 | item P — **rebuilt**, since it was never committed |

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
