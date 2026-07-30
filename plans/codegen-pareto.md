# Plan: codegen Pareto — registers, the ABI, and layout

**Status: PROPOSED (2026-07-29).** Not activated. Decision block 1700–1799
reserved. Prior rung: [M20.md](M20.md) (ACTIVE) — **this plan cannot
activate before M20 closes**, because every item is scored by M20's ruler
and item A consumes M20's Lane 2 block-grain sidecar.

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
callee-saved discipline, universal tail calls, argument-specialized
cloning.

**OUT** (each with its owner):

| Out | Owner |
| --- | --- |
| Interim spill peepholes (ladder 1a/1b) | decision 1700 — pull in only if E slips |
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

**Files:** `codegen.rs`, `mwir.rs` (width-carrying forms if needed),
`encode.rs` (`UBFX`/`SBFX`/`MOVN` if absent).
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

**F6. Argument-specialized cloning.** All call sites are known, so clone per
call site's known constants **without inlining** — the enabling effect at a
fraction of the size cost. Uses item A's call-frequency table.

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
