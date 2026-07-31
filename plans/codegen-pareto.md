# Plan: codegen Pareto — registers, the ABI, and layout

**Status: ACTIVE (2026-07-31).** Activated at `eb42af7c`, with M20 closed
(its evidence block committed at `82dad845`, its post-close corrections at
`9ef284f5`/`eb42af7c`). Decision block 1700–1799 is this plan's. Prior rung:
[M20.md](M20.md) — every item is scored by M20's ruler and item A consumes
M20's Lane 2 block-grain sidecar.

## Evidence block (close, 2026-07-31)

**Gate.** `cargo xtask check` **ok** — every lane, including the deep ∀
lanes, all boot transcripts, `repro`, `diff-eval`, eight fuzz lanes at 1000
iterations each, the census ratchets and every bench lock. 1001 golden
expectations, 855 units. `release` beats `dev` at **every point of both
tiers**.

**Nine named opts** in `RELEASE_OPTS`, in order: `BoundsElide`,
`NarrowImm`, `AdrAddressing` (B1), `BfxNarrow` (C3), `MaskCheck` (C2),
`WideImmForms` (C5), `RegAlloc` (E), `InterprocRegs` (F1/F2),
`Frameless` (F3). Seven of the nine are this plan's.

**The product tier, `dev` → `release`** — the number that did not exist
before this plan, because the gate could not see a shipped program:

| program | dev | release | Δ |
| --- | --- | --- | --- |
| `cost-product-actors` | 4517 | 3978 | −11.9 % |
| `cost-product-appliance` | 2460 | 1986 | −19.3 % |
| `cost-product-blk` | 6070 | 4976 | −18.0 % |
| `cost-product-receipt` | 7494 | 6427 | −14.2 % |
| **total** | **20 541** | **17 367** | **−15.5 %** |

**Per-opt verdicts, each asked over its own baseline** (decision 1717).
`PINNED_PRODUCT_TIER_VERDICTS`: eight `wins`, one `veto` — `BoundsElide`,
which is byte-identical to `dev` on all four shipped programs.

**Correctness.** Not one guest transcript moved across the whole plan —
addressing, three arithmetic forms, block layout, the frame model and the
calling convention of every function all changed, and the machine's
observable behaviour did not. `diff-eval` 129 agree / 47 cases.

### What this plan learned, as distinct from what it built

Six of the plan's own premises were **falsified by measurement**. That is
the more valuable half of the output, and it is recorded here so the next
rung does not re-derive it:

1. **Item D's premise does not survive.** "93–98 KB against a 64 KiB L1I"
   is the *all-hot* row, order-invariant by construction and unmovable by
   block layout. The measured hot subset is ~23 KB and already **84.9 %
   dense**. D also proved to be a **substitute** for the word-shrinking
   items, not a complement: B and C deleted 78 words from the closure and
   *none* of it reached the packed layout. The framing that ranked D into
   the Pareto three — "it makes everything else affordable" — is wrong.
2. **"~28 usable GPRs" was nine** for a per-function allocator (item E),
   with every missing register accounted for. F's interprocedural pass
   reached **27**. The plan's number was right about the machine and wrong
   about which item could have it.
3. **C1 was never gated on the ruler.** Activation decision 1740 blamed a
   missing W-form cost row; item C added the row and W still scored
   identically to X. The real cause was the spill-everything frame donating
   6 cycles of M-pipe slack per multiply. C1 was gated on **item E** all
   along, and the same mechanism later explained C5.
4. **Two opts do not survive contact with the shipped image.**
   `BoundsElide` — credited by M20 with 43.2 % of release's win — is
   byte-identical to `dev` on all four product programs; its whole measured
   effect is six microbenchmarks, the largest of which was written for it.
   `WideImmForms` was flat too until `RegAlloc` existed.
5. **"Universal" tail calls (F5) are measurably worse** (+29 cycles on
   every case borrowing the runtime closure). The restricted form scores
   exactly zero on all twenty cases of both tiers, so F5 lands
   unconditionally with no `OptId`.
6. **Unrolling is not worth pulling in**, on measurement rather than
   argument: the peak measured trip count in the entire boot workload is
   **13.0**, and that loop runs once. The busiest loop by mass is 3.7 trips.

### The defect, and what it says about the oracles

Item F's first landing was **green on 851 units, both ∀ tiers and
`diff-eval` — and silently miscompiled the guest**, flipping fourteen boot
transcripts from passing to failing. `layout.rs` runs after codegen and
replaces compiled bodies under keys codegen has already published a
convention for, so a caller kept live values across a call whose body was
then substituted.

The lesson is item F's own, and it generalizes past this plan: **units and
`diff-eval` verify the semantics of a computation; they do not verify a
claim one stage makes about another stage's output** — and F is the first
item whose central mechanism *is* such a claim. The oracle
(`codegen::verify_conventions`, run once at the end of codegen and again in
`layout_program` after every substitution) lives in the build, so every
golden that lays out an image exercises it rather than only a boot
transcript.

**This was also a process failure of decision 1708.** Barring parallel
agents from the hypervisor prevented HVF deadlock, and it meant the
keystone item developed for hours against oracles that could not see a
correctness break. Any future rung with a runtime-touching item must give
that item a boot lane, serialized if necessary.

### What the measured results justify pulling in next

- **The inliner + GVN/SCCP/DCE** (ladder pull-in #1/#2) — unchanged in
  priority; nothing here contradicts it.
- **Making the footprint term order-sensitive** — the named prerequisite
  for item D ever scoring. Without it D cannot be ranked, and with the
  density number above it may not be worth ranking.
- **`BoundsElide`'s disposition** — delete, keep on correctness grounds, or
  find a product-scale program where array indexing is hot. Now a question
  that can be *asked*, pinned so it cannot be quietly un-asked.
- **Deferred from this plan, with owners:** B2 (`ADR` for placed statics,
  needs a boot transcript), B4 (branch-chain cleanup — *wins on all 15
  cases* but is blocked by M20 decision 1608's bridge contract), C4
  (constant-divisor strength reduction, blocked on the divide-lo corner
  defect below), F4's >8-argument ceiling (no producer exists), and
  call-site `mov` coalescing (needs a per-point interference model).
- **Two ruler defects found in passing**, neither fixed here: the
  divide-lo corner models correlated brackets as independent, so a 32-bit
  divide can score slower than the 64-bit one it replaced; and the
  `--stage=cost` and `--stage=report` hot-text columns disagree for the
  same program (8 256 B vs 91 456 B), which is why item D's premise lives
  in a column the ∀ gate does not read.

**1795. The plan's three Pareto items did not rank as predicted.** E was
the win it was billed as. F delivered, but only after the defect above. D
did not, and its own measurement says why. The Pareto selection was made
from unmeasured priors, and two of the three priors were wrong — which is
an argument for measuring the ruler's blind spots *before* selecting a
subset, not after.

## Activation record (item 0, 2026-07-31)

**Baseline.** `cargo xtask check --fast` green at `eb42af7c`. The full gate
(HVF + measurement lanes) is the close's job, per CLAUDE.md.

**Kept the name, not a milestone number.** The header's doctrine note says an
activated plan takes a number. `M21.md` is already taken by a *different*
proposed rung (wire types, decision block 1800–1899), so numbering this one
M21 would make every "M21" citation ambiguous. It stays
`codegen-pareto.md` with its own 1700-block; the number is the human's to
assign if they want the file renamed.

**Re-check of the item ordering against what M20 actually measured** (the
header demanded exactly this). Three items are *not* as scoreable as the
plan assumed. None is a reason to reorder A–F; each is a reason the item
owns a gate-visibility sub-task before it claims a win:

| Item | What M20 actually leaves | Disposition |
| --- | --- | --- |
| **D** hot/cold layout | The score is an order-invariant total over a flat all-hot footprint at synthetic addresses. Block *order* is invisible to it, so D can pass the ∀ gate while changing nothing the model can see — and equally can be right while scoring zero. | D lands as a **reported artifact plus a boot transcript**, not as a `RELEASE_OPTS` entry, unless it first makes the footprint term order-sensitive. Decision 1750. |
| **C1** type-driven W/X width | The table has no W-form `MADD`/`MSUB`/divide rows; W and X score identically today. | C1 must add the two rows (SOG §3.6 notes 2/4, the same provenance discipline M20 item A used) *before* it can be ranked. Decision 1740. |
| **F6** argument-specialized cloning | Wedged: block frequency is flat `f≡1` for anything Lane 2 did not measure, so a clone's win is invisible, while its extra words read as `CoverageFell`. | F6 is **cut from this plan** unless items A+D make per-call-site frequency real. Decision 1770. |

**The ledger is gone.** `aa05bf75` deleted `ledger/ledger.toml` and
`ROADMAP.md`. Items A and G still say "ledger clauses"; there is nothing to
open. The evidence block in item G is the whole record instead. Decision
1706.

**1706. No ledger clauses this plan.** The ledger was deleted before
activation. Every claim this plan makes lands in item G's evidence block and
in a pinned oracle — nowhere else. If the ledger returns, this plan's claims
are re-derivable from G.

**1707. Decision sub-blocks, one per item** (the parallel-agent collision
this repo hits every round): A `1720–1729` · B `1730–1739` · C `1740–1749` ·
D `1750–1759` · E `1760–1769` · F `1770–1779` (+1792) · H `1780–1789` ·
G `1795–1799` (moved by decision 1718; C took 1790–1791). An item may not number outside its block. (F's block was
halved when item H was added; F6's cut freed the numbers.)

**1718. Ten numbers per item was not enough, and two items overflowed.**
Decision 1707 gave each item a block of ten. Items C and F both exhausted
theirs and continued outside — C into `1790–1791` and F onto `1780`, which
was already item H's. Recorded rather than silently renumbered, because a
decision number is cited from code comments and findings files and a quiet
re-map is exactly the kind of edit that leaves a tree self-contradicting:

- **F's `1780` is a genuine duplicate** of H's, so it is renumbered to
  **1792**. It appeared only in F's findings file; every `1780` in the code
  is H's tiering decision and is untouched.
- **C's `1790–1791` stand.** They collide only with numbers reserved for a
  close that had not yet been written, so the cheaper fix is to move the
  close: **item G takes `1795–1799`**, and `1793–1794` are spare.

The scheme itself held up — it caught both collisions at merge, which is
what it is for. The lesson is the block width, not the block.

**1716. The gate corpus and the product image are disjoint — item H fixes
that, and it is a prerequisite for believing any of A–G's claims.** The ∀
gate ranks over `tests/golden/cost-*`: fifteen synthetic microbenchmarks,
median ~95 lines, smallest nine lines. Nothing the appliance actually ships
is in it. Three consequences, all of which land on *this* plan rather than
on M20:

- **Self-selection.** Every item here is told to add a `cost-*` case if none
  exercises its opt — so each opt is graded on a program written to show it
  off. That is the same failure M20 item M found when four intended witness
  cases turned out not to witness anything.
- **E and F are whole-program changes.** Register pressure in a 13-line
  `cost-calls` is not register pressure in the driver set, and F changes the
  calling convention of every function in the image. An allocator that never
  spills anywhere in the corpus is untested where it matters.
- **D's premise is unmeasurable on the corpus that gates it.** 93–98 KB of
  text against a 64 KiB L1I is a fact about the real image; the largest
  corpus case is 684 lines. This is a deeper reason D scores zero than
  decision 1750's order-invariance.

Item H is therefore **not** a new optimization candidate and freeze 1710 does
not park it: it adds no opt and changes no emission. It makes the existing
gate measure the thing the plan claims to improve.

**1717. An item may not gate on a case it authored alone.** An opt's ∀
verdict must hold over the product-scale tier item H adds, not only over the
microbenchmark it shipped with. Where the two disagree, both numbers are
reported and the product-scale one governs.

**1709. Each item's findings land in `plans/codegen-pareto-<ITEM>.md`.** Every
parallel item otherwise edits the same plan file and the round ends in a
merge conflict over prose. An item writes its numbers, its decisions, and its
oracles to its own sibling file; **item G folds them into the evidence block
here** and the siblings stay as the working record. Item A's three derived
tables live in `plans/codegen-pareto-A.md` for the same reason.

**1708. One golden owner per round — the orchestrator.** Items report
before/after numbers and update only the expectation files their own item
moves; `cargo xtask golden`, `check`, and every HVF lane are run centrally
after each merge, so one re-pin tests the round's changes *for interaction*
rather than in isolation. (Concurrent golden runs deadlock on HVF; this is
not a style preference.)

**Doctrine note on numbering.** ROADMAP holds that milestone plans are
written when a milestone activates, never earlier, "because each milestone
manufactures the facts the next plan needs." So this is deliberately a
*named* plan rather than `M21.md` — the same shape as
[lane1-per-core.md](lane1-per-core.md). When the human activates it, it
takes a milestone number then, and **the item ordering must be re-checked
against what M20 actually measured** rather than against this plan's
assumptions. Two facts in particular could reorder it: the hot-text
footprint number (item D's premise) and the cross-core bracket (which
decides whether codegen or messaging is the better next spend at all).

## Why this plan, and why only these items

[opts-ladder.md](opts-ladder.md) catalogues ~40 candidate opts across ten
tiers. This plan takes the **Pareto subset**: the three items carrying most
of the expected gain, plus the easy wins whose ratio of value to effort is
high enough that skipping them would be silly. Everything else is parked in
the ladder as a pull-in backlog, with the two highest-priority pull-ins
named there explicitly.

The three Pareto items, and why each:

1. **Register allocation** (item E). Every temp currently round-trips
   through the frame — ~5 cycles (store 1 + load 4) plus AGU pressure and a
   V-pipe store-data uop, per temp per use, everywhere. Largest single
   constant-factor win available in the compiler.
2. **The no-ABI items** (item F). No function in a sealed image is callable
   from outside it, so AAPCS64 is pure tax. This is where the crossover with
   LLVM comes from, and it is the most clearly LLVM-impossible work in the
   project ([beating-llvm.md](beating-llvm.md) §A).
3. **Hot/cold block layout** (item D). Text is **93–98 KB against a 64 KiB
   L1I**. Until the hot subset is dense, every code-growing opt on the
   ladder is spending an overdrawn budget. Cheap to build, and it makes
   everything else affordable.

Explicitly **not** Pareto and therefore parked: the inliner and
GVN/SCCP/DCE (large builds, and the ladder names them as pull-in #1 and #2),
the whole SIMD/vectorizer stack (gated on the pixels rung), and every
moonshot.

## Standing rules (inherited, restated once)

- Each opt is a **named function** in the in-code `RELEASE_OPTS` order
  ([opts/mod.rs:33](../crates/wrela-compiler/src/opts/mod.rs:33)). No pass
  manager, no plugin registry (M19 freeze 1402).
- Land gate is M20's: **∀ over the uncertainty box**, no measured coverage
  loss, per-core text/TLB budget respected, every veto reason reported.
- `dev` stays the correctness reference; both modes stay green under
  `diff-eval` and the goldens (M19 freeze 1407).
- Losers are **deleted**, not kept disabled.

## Decisions

**1700. Skip the interim spill peepholes.** The ladder's 1a (spill/reload
forwarding) and 1b (dead frame-slot elimination) are deliberately **not in
this plan**: they are the cheap 20% of what item E delivers, and building
them first means writing code whose only purpose is to be deleted by item E.
If E slips or is descoped, pull them in from the backlog then — not before.

**1701. The allocator is linear-scan, per function, first.** No SSA
construction, no graph colouring. wrela's exclusivity rules mean no alias
analysis is needed to establish a temp's live range, which is what usually
makes a first allocator hard. Interprocedural comes in item F, on top —
never as part of the first landing.

**1702. Frame layout stays a reviewed artifact.** The report's
`Field`/frame surfaces and the `asm-*` goldens are the review surface for
items E and F. Both items update expected files in their own commits and
cheap-verify on a named sample, per CLAUDE.md rule 3 for large golden moves.

**1703. `ADR` range is proved at layout time, not assumed.** Item B
replaces `ADRP`+`ADD` with `ADR` only under a **fail-closed link-time
check** that every site's distance is within ±1 MiB. Verified headroom at
plan write: worst pinned image spans ~99 KB code-to-rodata-end
(`boot-receipt-handoff`), and rtdata/pooldata sit ~256 KB out — 4–10×
inside range. The `pages` section (0x40000000, ~5 MB out) keeps the
absolute form.

**1704. Width selection is driven by the type, not by inference.** Item C
uses the declared integer width, which the type system already proves. It
does **not** build a range-propagation pass — that is the ladder's Tier 7
and stays parked. Type-known width is free information already in hand.

**1705. Text base alignment lands with the layout item, not separately.**
Item D moves code layout anyway, so folding the 2 MiB-aligned text base
(SOG §4.8 same-region property) into the same golden churn costs nothing
extra and avoids a second whole-tree address move.

## Freezes (item 0 / human — do not relitigate once activated)

| # | Freeze |
| --- | --- |
| 1710 | Pareto scope only. New candidates go to the backlog, not this plan. |
| 1711 | No SIMD, no vectorizer, no inliner, no GVN in this plan. |
| 1712 | First allocator is linear-scan per function (decision 1701). |
| 1713 | `ADR` substitution is fail-closed on range (decision 1703). |
| 1714 | No named opt lands without passing M20's ∀ gate; a green unit test that never exercises the new path is not an oracle. |
| 1715 | Items E and F each own their golden re-pin commit. Do not fold them together — the diff is the review surface. |

## Goal

> Does emitted code stop round-tripping every value through the frame, stop
> paying a calling convention that has no callers, and pack its hot path
> into L1I — with `dev` and `release` both still correct?

## The honest scope line

**IN:** Lane 2 derived tables (trip counts, hot blocks, hot fields);
`ADR`-only addressing for rodata/rtdata/pooldata with a link-time range
proof; `add sp` folding and branch-chain cleanup; type-driven W/X width
selection; bitmask-immediate `TST` overflow checks; `UBFX` narrowing;
constant-divisor strength reduction; `MOVN`/bitmask immediate
materialization; basic-block hot/cold layout with a 2 MiB text base;
per-function linear-scan register allocation; interprocedural register
allocation with per-function conventions, frameless functions, no
callee-saved discipline, universal tail calls.

**OUT** (each with its owner):

| Out | Owner |
| --- | --- |
| Interim spill peepholes (ladder 1a/1b) | decision 1700 — pull in only if E slips |
| Argument-specialized cloning (was F6) | decision 1770 — cut at activation; back to the ladder |
| Making the footprint term order-sensitive (would let D score) | decision 1750 — a ruler change, named as D's prerequisite |
| Inliner, GVN/SCCP/DCE | [opts-ladder.md](opts-ladder.md) pull-in #1 and #2 |
| Range propagation pass (Tier 7 beyond 1704's type-known widths) | backlog |
| SIMD infrastructure + vectorizer (Tiers 9–10) | backlog; gated on pixels rung |
| Traffic-aware placement | backlog; **blocked** on the work-vs-time `max_core` veto |
| Block page tables, `DC ZVA`, `LDAR`/`STLR`, TBI, compressed handles, SWAR, crypto tricks | backlog (ladder Tier 6) |
| LICM, measured-trip unrolling, the copy-loop unroll | backlog |
| Field splitting / per-array layout | backlog |
| All three moonshots (send fusion, comptime boot, superoptimization) | backlog |

## Parallelism map

```text
              [0 Activation + freezes (human)]
                            │
                            ▼
                   [A Lane 2 derived tables]
                            │
        ┌───────────────┬───┴────────────┬──────────────┐
        ▼               ▼                ▼              ▼
 [B Addressing]  [C Arithmetic]   [D Hot/cold      [E Register
                                    layout]         allocator]
        └───────────────┴────────────────┴──────────────┤
                                                        ▼
                                            [F No-ABI: interprocedural
                                             RA, frameless, tail calls]
                                                        │
                                                        ▼
                                                  [G Close]
```

B, C, D, E are independent after A. **D needs A** (measured block
frequencies); B, C, E do not, so they can start immediately. **F gates on
E** — interprocedural allocation extends a working allocator; it does not
replace one.

**Milestone cut points, if the human wants smaller units:** A+B+C is a
clean static-shape milestone (everything shrinks words, everything lands on
the flat gate). D+E is a second. F is a third.

## Items

### A. Lane 2 derived tables

Consume M20 item C's `lane2-freq.txt` and commit three derived tables to
this file, because four later items key off them:

- **per-loop measured trip counts** (decides whether unrolling is ever
  worth pulling in from the backlog);
- **per-block hot/cold classification** (item D's whole input);
- **per-fn call frequency** (item F's cloning decisions).

**Cheap:** the tables themselves, plus a unit asserting the derivation is
deterministic and fails closed on a missing or stale sidecar.
**Focused boot:** `boot-actors` under `--block-count`.

### B. Addressing wins (static-shape)

**B1. `ADR`-only rodata addressing.** Replace every `ADRP`+`ADD` pair with
one `ADR`, under decision 1703's fail-closed range check. `enc_adr` exists
([encode.rs:851](../crates/wrela-compiler/src/encode.rs:851)). Halves every
rodata access and deletes a reloc class. **LLVM cannot do this** — it must
stay relocation-safe at isel.

**B2. `ADR` for placed statics.** Same for rtdata/pooldata references
(~256 KB out, in range); keep the absolute form for `pages`.

**B3. Fold `add xN, sp, #off` into addressing modes**; **B4.** branch-to-
branch and branch-to-fallthrough cleanup.

**Files:** `codegen.rs` (emission + reloc classes), `layout.rs` (the range
proof), `encode.rs` (no new encodings expected).
**Cheap:** unit per substitution; a unit that a deliberately out-of-range
site **fails the build** rather than emitting a wrong `ADR`; `--stage=asm`
diff on `asm-*` goldens.

### C. Arithmetic wins (static-shape)

**C1. Type-driven W/X width selection.** The verified A76 payoff:
`MADD`/`MSUB` **W-form is 2-cycle at 1/cycle**, **X-form is 4-cycle at 1/3
throughput and stalls the only M pipe 2 extra cycles** (SOG §3.6 notes
2/4). A multiply whose declared type is ≤32 bits must never emit X-form.
Same for divides, which block that pipe for 5–20 cycles.

**C2. Bitmask-immediate `TST` overflow checks.** Codegen's own invariant:
"Unsigned: overflow iff the high word is nonzero." High-masks
(`0xFFFFFFFF00000000`, …) are all encodable AArch64 bitmask immediates, so
the check is one `TST` + `B.NE` — no canonicalize-and-compare, no constant
materialization. **Highest site count in the plan**, because every checked
narrow op in every program is a customer.

**C3. `UBFX`/`SBFX` for `narrow_to_width`.** The LSL/LSR pair becomes one
instruction, same I-port class. Also covers the syntactic
already-canonical case; the *range-proved* case stays parked.

**C4. Constant-divisor strength reduction.** Magic-number multiply-high +
shift instead of `SDIV`/`UDIV`. High value because divide blocks the single
M pipe for 5–20 cycles; well-understood technique.

**C5. `MOVN` and bitmask-immediate constant materialization** — the
`NarrowImm` sequel; small negatives go 4 words → 1.

**1740 (activation).** C1 cannot be ranked on the ruler as committed: the
cost table has no W-form `MADD`/`MSUB`/`SDIV`/`UDIV` rows, so W and X score
identically. C1 owns adding them, with M20 item A's provenance discipline
(`source`/`mechanism`/`note`/`ambiguity`, tier stated) — the table and
provenance digests both move and that is the review surface. If the rows
cannot be justified from published material, C1 lands as a *reported* form
change with no win claimed, and says so.

**Files:** `codegen.rs`, `mwir.rs` (width-carrying forms if needed),
`encode.rs` (`UBFX`/`SBFX`/`MOVN` if absent), `cost/table.rs` (C1's rows).
**Cheap:** one unit per item asserting the *emitted form* changed and the
semantics did not; `cost-arith` diff; `diff-eval` on the checked-arithmetic
corpus specifically for C2 (the highest-risk item — an overflow check that
is subtly wrong is a correctness bug, not a perf regression).

### D. Hot/cold basic-block layout

Pack measured-hot blocks contiguously; move cold blocks (abort paths, error
handling, rare branches) out of the hot region. Fold in the **2 MiB-aligned
text base** (decision 1705) so every branch and its target share one region
per SOG §4.8.

**Why this is the cheap large win:** static text stops mattering once the
hot subset is dense. It is the item that makes the ladder's code-growing
opts affordable later, and it is the direct answer to 93–98 KB vs 64 KiB.

**1750 (activation).** The ruler cannot see block order: its footprint term
totals a flat, all-hot text at synthetic addresses, and the total is
order-invariant. So D **does not become a `RELEASE_OPTS` entry** on this
plan. It lands as (a) the layout itself, (b) the per-core hot-text
footprint number in the report before/after, (c) the named boot transcripts,
and (d) an honest statement that the ∀ gate scored it at zero. Making the
footprint term order-sensitive is a ruler change and is out of scope here;
if D wants a gate win later, that change is its prerequisite, named.

**Files:** `layout.rs`, `codegen.rs` (block ordering), `report.rs` (the
per-core hot-text line M20 item F adds — cite it, do not duplicate it).
**Cheap:** unit that a synthetic hot/cold program orders as expected; the
per-core hot-footprint number from `--stage=report` before/after, recorded
here; representative `--stage=asm` diff.
**Focused boot:** `boot-actors`, `boot-cores-3`.
**Note:** every address in every golden moves. Own commit (freeze 1715's
spirit), review the diff, cite the clause.

### E. Register allocator (per function)

Linear-scan over the existing per-fn MWIR (decision 1701). ~28 usable
GPRs. Every resident temp deletes a store, a load, a V-pipe store-data uop,
and two AGU uops per use.

**Why it lands cleanly under the gate:** it *shrinks* words (spill code
deleted), so it passes even the old footprint veto. And the swept
store-to-load-forwarding latency must not be what the win depends on — it
will not be; the win is structural.

**Doctrine fit:** this is the canonical "dumb version replaced by a provably
equivalent one." `diff-eval` is the equivalence oracle and `dev` mode keeps
the spill-everything reference alive permanently.

**Files:** new `regalloc.rs` (long obvious file, no seam), `codegen.rs`
(frame construction consults it), `mwir.rs` (unchanged if possible).
**Cheap:** units — a value used twice in a row loads once; a program with
more live values than registers still spills correctly; frame shrinks on a
named case. Plus `diff-eval` and the `asm-*` golden family.
**Focused boot:** `boot-actors` plus one driver case.

### F. No-ABI: interprocedural allocation and its consequences

The keystone. AAPCS64 exists so strangers can call your code; a sealed image
has no strangers, no indirect calls, and no `dyn`.

**F1. Interprocedural register allocation** with a **custom convention per
function**, computed globally. Cross-call register residency without fusing
code — the live-range firewalls that make fusion counterproductive survive.

**F2. No callee-saved discipline.** The caller knows exactly what the callee
clobbers, so conservative save/restore disappears rather than shrinking.

**F3. Frameless functions.** A function whose values all fit in registers
gets no frame at all: prologue, epilogue, and `sub sp` vanish.

**F4. Arbitrary-arity register passing and multi-value returns.** No x0–x7
limit, no x8 indirect-result convention.

**F5. Universal tail calls.** Every tail-position call is a jump,
unconditionally.

**F6. Argument-specialized cloning. CUT at activation (decision 1770).** The
ruler scores unmeasured blocks at flat `f≡1`, so a clone's win is invisible
while its extra words read as a coverage fall — the item is wedged between
two terms, and no amount of implementation effort moves it. It returns to
[opts-ladder.md](opts-ladder.md) as a pull-in whose named prerequisite is
per-call-site measured frequency (item A's table extended to call sites, or a
frequency-aware footprint term). F1–F5 are unaffected: none of them depends
on it.

**Files:** `regalloc.rs` (whole-program mode), `codegen.rs`, `layout.rs`
(call sites become convention-aware), `report.rs` (the report should show
each function's chosen convention — otherwise the most consequential
decision in the compiler is invisible).
**Cheap:** units — a leaf whose values fit gets no frame; a caller does not
save a register the callee provably never touches; a tail call emits `B` not
`BL`+`RET`; a cloned function's specialized copy differs from the general
one. `diff-eval` is doing heavy lifting here — budget fuzz effort on the
`lower`/`sema` lanes accordingly.
**Focused boot:** `boot-actors`, `boot-cores-3`, one driver case.

### H. Product-scale cost corpus (added 2026-07-31, decision 1716)

Widen what the ∀ gate ranks over, from fifteen microbenchmarks to something
that includes the code the appliance ships.

**H1. A product-scale tier.** Add cost cases built from real programs — the
appliance image, `boot-actors`, a driver-heavy closure, and the stdlib
closure — so the sweep scores programs whose register pressure, text
footprint, and call graph are the product's rather than a fixture's.

**H2. Tier the lanes by cost, per CLAUDE.md** ("a test's home is chosen by
its cost, not its subject"). The micro corpus stays the smoke lane; the
product-scale tier runs in the deep lane. Measure and report what the
addition costs the deep lane before choosing — the whole-corpus sweep is
~4 minutes today, and a bound that refuses the corpus is not a bound
(M20's own lesson at `MAX_SWEPT_DIMS`).

**H3. Report both numbers.** Where the micro tier and the product tier
disagree about an opt, that disagreement is the finding — decision 1717 says
the product tier governs and both get printed.

**Files:** `tests/golden/cost-*` (new cases), `crates/wrela-compiler/src/opts/win.rs`
(corpus discovery + tiering), `crates/xtask` (which lane runs which tier).
**Cheap:** a unit that the tier split is explicit and that a case cannot
silently belong to neither tier; the measured deep-lane cost, before/after.
**Out:** no new opt, no emission change, no cost-table row. If H wants to
change what the model *reads*, that is a ruler change and is out of scope.

### G. Close

Full `cargo xtask check`; deep fuzz per the standing lanes; `bench guest`
recorded as **observation only** (M20 freeze 1631). Ledger clauses opened
per item and flipped here. Evidence block at the top of this file: the
per-point corpus table `dev` → `release` under M20's gate, the per-core
hot-text footprint before/after item D, frame-size and word-count deltas
from E and F, and item A's tables as committed.

Also record: **which backlog items the measured results now justify pulling
in**, with the numbers that justify them. That list is this plan's main
output for whatever comes next.

## Why this is a plan and not a cleanup

1. Item F changes the calling convention of every function in the image —
   the most invasive change the backend can undergo short of a rewrite.
2. Items D, E, F each move the entire `asm-*` golden family and every
   address in every report.
3. Item E replaces the frame model that `compiler.codegen.naive-locked`
   pins, so the clause's meaning changes and the note must say so.
4. It is the first body of work whose thesis is *beating LLVM* rather than
   *being adequate*, which means its claims need the ruler and the ∀ gate
   to be believable at all.
