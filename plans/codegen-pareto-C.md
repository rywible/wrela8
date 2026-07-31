# Item C findings — arithmetic wins

Working record for [codegen-pareto.md](codegen-pareto.md) item C, per
decision 1709. Item G folds the numbers into the evidence block; this file
is where they are derived. Decision block **1740–1749**.

Branch `cp-C`, based on `01a7ca24`. Commits:

| | |
| --- | --- |
| `7ed6da12` | `UBFX`/`SBFX` and a fail-closed bitmask-immediate encoder |
| `d7a8c451` | **C1: price the W-form multiply-accumulate group** — the digest move, alone |
| `05d5f61e` | C2/C3/C5 codegen + opts, and C1's emit site |
| (this file) | findings + the C2 differential corpus extension |

## What landed

| Sub-item | Landed as | ∀ gate verdict |
| --- | --- | --- |
| **C1** type-driven W/X width | Unconditional codegen form change + a T1 cost row. **Not** an `OptId`. | Scored at zero on item C's tree (1746); rankable after item E, still not an id — its only customer is the case it authored, and it is byte-identical on all four shipped programs (**1790**, follow-up below). |
| **C2** bitmask-immediate `TST` overflow checks | `OptId::MaskCheck` in `RELEASE_OPTS` | **Passes**, alone vs `dev`, at all 1024 corners of `cost-arith` and over the whole corpus |
| **C3** `UBFX`/`SBFX` narrowing | `OptId::BfxNarrow` in `RELEASE_OPTS` | **Passes**, alone vs `dev`, on `cost-runtime` |
| **C4** constant-divisor strength reduction | **Not landed** | Not attempted — see below |
| **C5** `MOVN` + bitmask-immediate materialization | `OptId::WideImmForms` in `RELEASE_OPTS` | Micro: **passes** over `[NarrowImm]` on `cost-mpipe-block`. Product: `veto` over `[NarrowImm]`, **passes** over `[NarrowImm, RegAlloc]` and by leave-one-out, on `cost-product-blk` and `cost-product-receipt` (**1791**, follow-up below). Marginal — tripwire recorded. |

`RELEASE_OPTS` was `[BoundsElide, NarrowImm, BfxNarrow, MaskCheck,
WideImmForms]` at item C's close; after items B and E merged it is
`[BoundsElide, NarrowImm, AdrAddressing, BfxNarrow, MaskCheck,
WideImmForms, RegAlloc]`. Item C's follow-up (below) adds no id and
removes none.

## Decisions

**1740 (the plan's own, discharged).** C1 owned adding the missing W-form
cost rows. The `MADD`/`MSUB` row is added, with T1 provenance (below), in
its own commit. The `SDIV`/`UDIV` W-form rows are **not** added, for two
independent reasons — see 1749.

**1741. Item C's transforms are four separable knobs, not one.** The ∀
gate ranks one `OptId` at a time; a knob switching two substitutions at
once cannot be attributed when it is refused. (C1's knob was later
deleted by 1746, leaving three.)

**1742. C2 covers both signedness cases, by two different identities.**
Unsigned: in range iff `v & !(2^bits − 1) == 0`, one `TST` against an
encodable bitmask immediate. This catches **both** failure directions at
once — a value above the range sets a bit inside the mask, and a negative
value (which checked `-` can produce from two canonical unsigned
operands) sets bit 63, also inside the mask because `bits < 64`. Signed:
in range iff `v` is its own sign-extension from `bits`, which is exactly
what `SBFX` computes, so the check is that `SBFX` compared against `v`.
Both forms reach **one** abort where the old form reached two, and that
is where most of the words go.

**1743. C3 is a rewrite of one instruction as itself.** `LSL #s` then
`LSR`/`ASR #s` is `UBFM`/`SBFM` twice; `UBFX`/`SBFX` at lsb 0 is the same
bitfield move in one word. Same SOG group (`bitfield move, basic`), one
fewer word and one fewer link of dependence chain.

**1744. C5 is gated on what `NarrowImm` cannot reach.** `NarrowImm` skips
*zero* halfwords, so a small negative still costs four words — `-1` is
`0xFFFF` in all four and none of them is zero. `MOVN` covers every value
whose inverse is a single non-zero halfword; the bitmask-immediate `MOV`
(`ORR Xd, XZR, #imm`) covers every rotated repeating run of ones. Both are
tried before the `MOVZ`/`MOVK` chain and only when strictly shorter, so
this can never lengthen a materialization.

**1745. One `OptId` per landed transform.** C2, C3 and C5 each get one.
C1 gets none (1746); C4 gets none because it did not land. An id whose
transform cannot be ranked would be a claimed win with no evidence.

**1746. C1 lands as a reported form change, with no win claimed.** Freeze
1714 forbids a named opt that does not pass the ∀ gate, and C1 does not:
it scores **exactly zero** on every case. Decision 1740 prescribed this
outcome in advance ("land C1 as a reported form change with no win
claimed, and say so"), and item D's decision 1750 is the same disposition
for the same kind of result. So the W-form selection is unconditional —
instruction *selection* from a width the type system already proves, the
same category as choosing `STRB` over `STR` for a `u8` field — and it is
emitted in `dev` and `release` alike. This also keeps freeze 1630
satisfied: the `mul_w` row has a live emit site.

**1747. C5 is gated against `[NarrowImm]`, not against `dev`.** Not a
softer gate — the only gate that means anything for this opt. With
`NarrowImm` off, `load_imm` returns through `load_imm_naive` before it
ever reaches C5's one-word forms, so `[] → [WideImmForms]` is the
identity comparison and would pass or fail for reasons having nothing to
do with C5. The plan names C5 "the `NarrowImm` sequel"; this is what that
composition means once the gate has to rank it. Measured: `WideImmForms`
alone against `dev` moves **0 cycles and 0 words on every case in the
corpus**; over `[NarrowImm]` it moves −4 cycles and −20 words.

**1748. C2 moves `boot-actors`' block census, and it is not a coverage
loss.** Measured word blocks 190 → 188, all word blocks 333 → 331, branch
words 265 → 264, `no_data` 104 → 103. Attributed by ablation: removing
`MaskCheck` alone from `RELEASE_OPTS` returns every one of those numbers
to its M20 value. The join still resolves the **same 81 keys** and leaves
the **same 291 unresolved**, and `biased` (14) and `mispredict` (145) are
unchanged — the measurement explains exactly what it explained before, in
two fewer blocks, because C2 deleted an abort site and therefore a block
boundary. Decision 1617 ("do not fix coverage") is not engaged: coverage
did not move.

**1749. The W-form divide rows are not added, and C1's divide half is not
implemented.** Two independent reasons, either sufficient:

1. **Freeze 1630.** A `[latency]` row may exist only for a group the
   emitted stream contains. C1 did not land W-form divides, so the row
   would be a priced group with no emit site.
2. **The bracket geometry makes it unrankable anyway** — and this is the
   reviewer's "divide-lo corner" showing up in C1 rather than in C4. SOG
   §3.6 gives X-form divide as 5–20 and W-form as 5–12. The committed
   table pins the pessimistic end and sweeps the bracket
   (`divide_x_latency ∈ [5,20]`). A W-form row would need its own
   `divide_w_latency ∈ [5,12]`, and two *independent* dimensions put the
   corner `(divide_x_latency = 5, divide_w_latency = 12)` inside the box.
   At that corner a 32-bit divide scores **slower** than the 64-bit
   divide it replaced, so `CaseRose` fires and the ∀ gate refuses — for a
   point that is physically impossible on one divider, since a 32-bit
   divide of the same numerator cannot take more iterations than the
   64-bit one. The two brackets are correlated and the box models them as
   independent. Fixing that is a **ruler change** (a joint constraint, or
   one dimension with a width-dependent ceiling) and is out of scope under
   freeze 1710. Named here as the prerequisite for anyone pulling C1's
   divide half or C4 in later.

## C1's cost-table row: exact provenance

Added in `d7a8c451`, its own commit, per the plan's instruction.

```toml
[latency.mul_w]
lat = 2
acc_lat = 1
thru_num = 1
thru_den = 1
ports = "M"
tier = "T1"
source = "SOG §3.6 multiply-accumulate, W-form (32-bit); note 4's M-pipe stall is on the X-form row only"
note = "no m_pipe_stall: SOG §3.6 note 4 attaches to the X-form row, not this one"
```

| Field | Value | Justification |
| --- | --- | --- |
| `tier` | **T1** | Cortex-A76 Software Optimization Guide, vendor, this core, normative — the same document and the same §3.6 table as `[latency.mul]`, which is the row directly above it. |
| `source` | SOG §3.6 multiply-accumulate, W-form | Transcribed into [M20.md](M20.md)'s own AArch64 integer table at plan time: "Multiply-accumulate, W-form \| `MADD`, `MSUB` \| 2 (acc 1) \| 1 \| M". That transcription predates this item by months, so the numbers were not chosen with C1's result in view. |
| `lat` / `acc_lat` | 2 / 1 | Read directly off that row. No bracket in the source, so nothing to sweep and nothing to pin pessimistically. |
| `thru_num`/`thru_den` | 1 / 1 | Read directly off that row (1 per cycle, against the X-form's 1/3). |
| `ports` | `"M"` | Same pipe as every other multiply — SOG §2.1 pipeline 4. |
| `m_pipe_stall` | **absent** | The claim, not an omission. SOG §3.6 **note 4** attaches to the X-form multiply-accumulate row only; the W-form row carries no note. Recorded in `note` so the absence is legible as a decision. |
| `ambiguity` | **absent** | Deliberately. The over-cost rule (decision 1609) requires an `ambiguity` field where the record conflicts or gives a bracket. Here it does neither: these are two rows of one published table that disagree about nothing. Compare `[latency.sdiv]`, whose 5–20 span is a genuine bracket and is therefore swept. |

**Digests before/after** (the review surface, per the relock discipline in
`bench/a76-pi5.toml`'s own header):

| | before | after |
| --- | --- | --- |
| `table_digest` | `043f8dc4b21c4b12` | `b2484a1b9c00d7fa` |
| `provenance_digest` | `3f7c1c0356b12d8d` | `e851020b74800045` |
| provenance summary | `T1=38 T2=19 T3=0 T4=10 T5=16 rows=83` | `T1=39 T2=19 T3=0 T4=10 T5=16 rows=84` |

The row's own commit moves **47 expectation files and nothing but those
two header lines in each** — no case's `proxy_cycles` changes, because the
row is inert until the following commit gives it an emit site. That is why
the commits are split.

## The ∀ gate, with real numbers

Baseline vs candidate, `total_proxy_cycles`, over the whole
`tests/golden/cost-*` corpus at the pinned point
(`unit:item_c_attribution_over_the_corpus`):

| config | cycles | words | Δ cycles vs dev |
| --- | --- | --- | --- |
| `dev` | 124813 | 112412 | — |
| `BfxNarrow` alone | 124809 | 112367 | **−4** |
| `MaskCheck` alone | 124757 | 112265 | **−56** |
| `WideImmForms` alone | 124813 | 112412 | 0 (inert without `NarrowImm` — decision 1747) |
| `[NarrowImm]` | 118338 | 90936 | — |
| `[NarrowImm, WideImmForms]` | 118334 | 90916 | **−4** over that baseline |
| `WFormMul` (pre-1746) | 124813 | 112412 | **0** |
| `release` (all five) | 114124 | 87925 | −10689 |

∀ verdicts, `compare_opt_lists_over_box`, each opt **alone** against its
baseline — no case rises at any point, one case falls at every point:

| opt | baseline | smoke case | corners | first corner |
| --- | --- | --- | --- | --- |
| `MaskCheck` | `dev` | `cost-arith` | 1024 | 121 → 105 |
| `BfxNarrow` | `dev` | `cost-runtime` | 1024 | 1692 → 1691 |
| `WideImmForms` | `[NarrowImm]` | `cost-mpipe-block` | 1024 | 448 → 445 |

Whole-corpus sweep: `unit:each_item_c_opt_wins_over_the_whole_box_alone`
(deep lane, `#[ignore]`d).

Per-case cost-golden totals, `dev` → `release`, for the cases that moved:

| case | before | after |
| --- | --- | --- |
| `cost-arith` | 153 | **140** |
| `cost-crosscore` | 4319 | 4313 |
| `cost-icache-cliff` | 29583 | 29577 |
| `cost-itlb-span` | 74079 | 74073 |
| `cost-mpipe-block` | 630 | 627 |
| `cost-runtime` | 1954 | 1948 |

`cost-arith`'s `checked_add` is the shape C2 was built for: **37 emitted
words → 24** (counted off `asm-arith`'s golden before and after), `abort`
2 → 1, `adrp` 2 → 1, `mov_wide` 5 → 1, `alu` 11 → 7, `store` 8 → 6, and
the fn's own `proxy_cycles` 36 → 30.

## Why C1 is not rankable — measured, not asserted

Decision 1740 predicted C1 would be unrankable *because the table had no
W-form row*. The row is now there, with T1 provenance; W-form multiplies
are emitted and are priced at their own latency — and the substitution
still moves the total by **exactly zero on every case**. So the missing
row was not the reason, and
`unit:item_c1_is_hidden_by_the_frame_until_the_multiply_outgrows_its_slack`
finds the reason that is.

It prices the *same emitted program* under the committed
`[latency.mul_w]` and under the X-form row those words replaced, then
walks the counterfactual latency upward:

```
the emitted W-form words score 167 at the committed `[latency.mul_w]`
and the identical 167 when priced at the X-form row they replaced —
the substitution is worth 0. It first becomes visible at lat = 10
(169 cycles), so the spill-everything frame is donating 6 cycles of
M-pipe slack per multiply, against the 2 cycles SOG §3.6 says the
substitution saves.
```

Under `compiler.codegen.naive-locked`'s spill-everything frame every
operand arrives from a 4-cycle frame load and every result leaves through
a store, so the two L pipes bound the block and the M pipe has slack to
spare. The X-form numbers (lat 4, hold `ceil(3/1) + 2 = 5`) fit inside
that slack; the W-form numbers (lat 2, hold 1) fit inside it too, and two
quantities that both fit under the same bound are the same number to a
block schedule. `cost-mpipe-block`'s golden recorded the same shape for
the X-form *stall* at M20 ("the frame's own load/store traffic already
spaces the multiplies more than five cycles apart"); this is that
observation carried to the latency.

The test also refuses the other failure: if **no** inflation changed the
total, the multiply term would be inert in the model and the zero would
mean nothing. It is not inert; it is hidden.

**So C1's payoff is gated on item E, not on the ruler.** When temps stay
in registers the multiply reaches a critical path and the row added here
starts to pay. The threshold — 6 cycles of donated slack — is the number
item E has to eat into. Re-run the ablation after E lands.

## C2's differential oracle

`cargo xtask diff-eval`, run once at the end, is C2's equivalence oracle.
The committed corpus **did not reach the new path**: `check-tests-arith`
exercises *wrapping* arithmetic (`+%`, `-%`), which emits no range check
at all, and nothing anywhere ran a *checked* narrow `+ - *` or a
narrowing `.to[T]()` through the differential. An oracle that never
reaches the code is not an oracle (freeze 1714), so the corpus is
extended: **`tests/golden/check-tests-checked-narrow`**, 8 comptime
`@test` fns (grouped one per declared type: `rtconfig`'s runtime test pool
holds 16 and one-assertion-per-test wanted 17 — the paths reached are the
same either way, and the pool refusal is a clean fail-closed panic).

Every assertion lands **exactly on a boundary of the declared type's
range**, which is where the substitution can be wrong and nowhere else:

- unsigned `u8`/`u16`/`u32`, each reaching its own maximum by checked
  `+` and by checked `*`, and reaching `0` by checked `-` — a mask one
  bit too wide rejects the maximum, one bit too narrow accepts a value
  the type cannot hold and the wrong value shows up in the comparison;
- signed `i8`/`i16`/`i32` at **both** ends, plus `-1`/`0` crossings — the
  two values that differ in every bit above the type's width, which is
  what separates a correct sign-extension test from one that happens to
  work on small positives;
- narrowing `.to[u8]()`/`.to[u32]()` at the boundary, the *other* caller
  of the same check.

Result, verbatim, with the extension in place:

```
diff-eval: case check-tests-checked-narrow: 8 comptime test(s) agree (0 exhaustive skipped)
diff-eval: 129 test(s) agree across 47 case(s), 8 lowering-skips, 6 exhaustive-skips, 1 quota-skips, 0 import-skips
```

**0 disagreements.** Before the extension the run was `121 test(s) agree
across 46 case(s)` with identical skip counts, so the delta is exactly the
one new case and its 8 tests — the evaluator and the real backend under
the VMM agree on every boundary value the masked check decides.

## The encoder, and the trap `encode.rs` named

`encode.rs`'s header called the bitmask immediate "a correctness trap this
milestone does not need to take on … an immediate that silently encodes a
*different* value than the one asked for". C2 and C5 both need it, so the
trap is closed **structurally**: `encode_bitmask_imm` builds the
`N:immr:imms` triple and then decodes it back through
`decode_bitmask_imm`, an independent transcription of the ARM ARM's
`DecodeBitMasks`, returning `None` unless the round trip reproduces the
requested value bit for bit. A wrong encoding cannot reach an emitted
word; it can only fail to be an encoding, and every caller falls back to
the form it already had.

The oracle is exhaustive, not sampled:
`unit:every_valid_bitmask_immediate_round_trips` enumerates all
`2 × 64 × 64` syntactically valid triples, decodes each, and requires the
encoder to recover a triple decoding to the same value — **5334 distinct
immediates**, the published count for the 64-bit form.

64-bit forms only. The 32-bit form needs `N = 0` and a pattern repeating
within 32 bits; codegen has no 32-bit caller, and an encoder with an
unexercised half is where a wrong word hides.

## What I could not do, and why

**C4 — constant-divisor strength reduction: not attempted.** Honest
reason: budget. The three ranked opts, C1's provenance work and C2's
differential corpus consumed the item, and C4 is the largest of the five
(magic-number selection for signed and unsigned, at every width, each
needing its own correctness argument and its own boundary corpus).
Nothing found here argues against it, and one thing found here argues
*for* looking at it carefully first:

The plan's reviewer flagged that signed C4 is "likely wrongly vetoed at
the divide-lo corner". I did not reach C4, so I cannot confirm that on
C4's own numbers — but I hit **the same corner from the other side**
(decision 1749), and it is real. The committed box sweeps
`divide_x_latency ∈ [5,20]` and pins the pessimistic end. At the
`divide_x_latency = 5` corner a divide costs 5 cycles, which is cheap
enough that a magic-number sequence (`SMULH` at lat 5 / thru 1/4 /
3-cycle M-pipe stall, plus shifts and an add) can easily score *worse*
than the divide it replaces — while at the pinned corner (20) it scores
much better. That is a `CaseRose` at one point of the box and therefore a
refusal under ∀, whatever the pinned point says. So the reviewer's
suspicion is structurally sound and the veto is a property of how the
divide bracket is modelled, not of C4's code.

Whoever picks C4 up should decide the ruler question **first**: the
divide bracket is data-dependent early termination, so `[5,20]` is a
distribution collapsed to a range, and the ∀ gate treats every point in it
as equally admissible. Either C4 needs a corpus case where it wins even at
`divide_x_latency = 5`, or the bracket needs to become something a gate
can reason about (a joint constraint with the multiply rows, or a measured
distribution rather than an interval). Both are ruler changes, out of
scope under freeze 1710.

**The W-form divide half of C1: not landed.** Decision 1749.

**Boot lanes: not run.** Decision 1708 — `cargo xtask check`, unfiltered
`golden`, and every booting lane are the orchestrator's, run centrally
after merge. Two things in this item land on that lane and want checking
there:

- `tests/golden/check-tests-checked-narrow/expected/test.txt` — a
  `test.txt` expectation makes the case a **boot** case, so
  `golden --no-boot` skips it by construction. Its content was generated
  from `wrela test` and is byte-identical in shape to
  `check-tests-arith`'s, and `diff-eval` ran all 17 of its tests on the
  real backend under the VMM — but the golden itself has not been run
  through the boot lane here.
- `boot-actors`' transcript, whose block structure moved (decision 1748).

## Verification actually run

| Command | Result |
| --- | --- |
| `cargo test -p wrela-compiler --lib` | **779 passed, 0 failed, 3 ignored** |
| `cargo xtask golden --no-boot` | **696 expectation(s) ok (560 cases, boots skipped)** |
| `cargo test --lib each_item_c_opt_wins_at_every_box_point_on_its_smoke_case` | ok — 3 opts × 1024 corners |
| `cargo test --lib each_item_c_opt_wins_over_the_whole_box_alone -- --ignored` | deep lane, whole corpus |
| `cargo xtask diff-eval` | **129 agree across 47 cases, 0 disagreements** |

Not run, deliberately (decision 1708): `cargo xtask check` in any form,
unfiltered `golden`, anything `--only-boot`, `bench`/`profile`/`repro`,
`cargo test -p wrela-vmm`.

---

# Follow-up after items A/B/D/E/H merged (master `bac328db`)

Item C's own findings predicted one of these and were wrong-footed by the
other. Decisions **1790** and **1791** — item C's 1740–1749 block was
exhausted, and 1789–1799 is the free tail of this plan's 1700 block
(item F holds 1770–1779).

## 1790 — C1 does **not** become a ranked `OptId`, for a new reason

**The prediction landed.** Item C's findings said "C1's payoff is gated on
item E, not on the ruler" and named the threshold: the spill-everything
frame was donating 6 cycles of M-pipe slack per multiply against the 2 the
substitution saves. Item E's allocator removed that slack and
`unit:item_c1_is_hidden_by_the_frame_until_the_multiply_outgrows_its_slack`
failed on schedule, exactly as its own message said it would. On the
merged tree the ablation reads **W-form 149 vs X-form 152** — a 3-cycle
win with 1 cycle of residual slack.

So decision 1746's premise ("scores exactly zero, therefore unrankable,
therefore freeze 1714 forbids a named opt") is gone. C1 is rankable now.
It still does not become an `OptId`, and this is the measurement that
decides it:

| baseline `[RegAlloc]` → `+WFormMul` | Δ cycles |
| --- | --- |
| `cost-arith-w` | **−1** |
| every other micro case (14) | 0 |
| `cost-product-actors` | **0** |
| `cost-product-appliance` | **0** |
| `cost-product-blk` | **0** |
| `cost-product-receipt` | **0** |

and in the full shipped list, `RELEASE_OPTS` vs `RELEASE_OPTS` minus C1:
`cost-arith-w` 152 → 149, **every product program unchanged**.

The product tier is flat for a reason that needs no sweep to establish.
Counting W-form multiplies in the four programs the appliance ships:

```
cost-product-actors:    mul_w=0  mul_x=25
cost-product-appliance: mul_w=0  mul_x=24
cost-product-blk:       mul_w=0  mul_x=28
cost-product-receipt:   mul_w=0  mul_x=28
```

**Zero.** All 105 multiplies in shipped code are X-form — checked
multiplies, whose overflow test needs the high half, or 64-bit wrapping
ones. C1 changes not one emitted word on the product tier, so it scores
identically at every point of the box by construction.

That leaves `cost-arith-w` as the only case in either tier that moves at
all — **and item C wrote `cost-arith-w`**, precisely because no case in
the corpus had a narrow wrapping multiply. Freeze 1717 says an opt may not
gate on a case it authored alone, and that is the entire gate C1 would
have. So C1 stays what decision 1740 prescribed in advance: a reported
form change, unconditional, with no win claimed.

`cost-arith-w` stays too — it is a legitimate cost golden pinning the
W-form emission and the `mul_w` row, and it is the ablation's substrate.
It just cannot be C1's gate.

**Revisit when** the appliance ships a narrow wrapping multiply. The
ablation test is the tripwire and already reads the right way.

## 1791 — C5 keeps its place, and the veto was the baseline

Item H's re-run found `WideImmForms` flat on the product tier — Δ = +0 at
all 10 240 points, asked over `[NarrowImm]`. That turned
`unit:each_item_c_opt_wins_over_the_whole_box_alone` red on master once
the product tier became part of the box it sweeps. The finding is real and
is reproduced here. Two separate questions came out of it.

### The `MaskCheck` hypothesis: **confirmed, and sharper than stated**

Item C's own `RELEASE_OPTS` ordering note guessed the mechanism —
`MaskCheck` deletes most of the constant materializations `WideImmForms`
would otherwise shorten. Counting C5's actual customers (emitted `MOVN`
plus bitmask-immediate `MOV` words) on the four shipped programs:

| program | customers, `MaskCheck` off | customers, `MaskCheck` on | words saved off → on |
| --- | --- | --- | --- |
| `cost-product-actors` | 1 (1 bitmask) | **0** | 1 → 0 |
| `cost-product-appliance` | 1 (1 bitmask) | **0** | 1 → 0 |
| `cost-product-blk` | 3 (2 `MOVN`, 1 bitmask) | 2 (2 `MOVN`) | 7 → 6 |
| `cost-product-receipt` | 2 (1 `MOVN`, 1 bitmask) | 1 (1 `MOVN`) | 4 → 3 |
| **total** | **7** | **3** | **13 → 9** |

`MaskCheck` deletes **4 of C5's 7 customers on shipped code, and every
single bitmask-immediate one** — those were exactly the narrow
range-check bounds C5 would have materialized in one word, and `MaskCheck`
removes the materialization entirely. Two of the four shipped programs are
left byte-identical with and without C5. The hypothesis was right.

### But that is not why the verdict was `veto`

Three `MOVN` customers survive on two programs, and they are worth real
cycles — just not against a baseline without the allocator:

| baseline | `cost-product-blk` | `cost-product-receipt` |
| --- | --- | --- |
| `[NarrowImm]` → `+C5` | 5808 → 5808 (**0**) | 0 |
| `[NarrowImm, RegAlloc]` → `+C5` | 5217 → **5216** | 6737 → **6736** |

**C5's saving is words; words become cycles only once the schedule has no
slack left to absorb them, and `RegAlloc` is what removes that slack.**
That is the same crossover as C1 — one mechanism, two opts, and the reason
item H's `[NarrowImm]` baseline could not see it. `[NarrowImm]` asks the
question in a configuration the product does not ship, which is the same
objection decision 1747 raised against `dev`.

### The membership claim, asked the hardest way

Changing a baseline after seeing a test go red is exactly the move that
needs to be distrusted, so C5's place is **not** rested on it. The
strictest question available has no baseline freedom at all: remove C5
from the shipped list and ask whether the shipped list gets worse. C5 then
has to beat every other opt, including the two that delete most of its
customers.

```
C5 leave-one-out, product tier: 10240 points/side
  falls at every point on: ["cost-product-blk", "cost-product-receipt"]
  rises anywhere:          []
  vetoes:                  []
```

Identical verdict, identical two cases, as the corrected alone-gate. Pinned
in `unit:item_c5_earns_its_place_by_leave_one_out_on_the_product_tier`, so
the claim rests on the question with no freedom in it and the smoke row's
baseline cannot be what carries it.

### Recommendation: **keep C5**, and here is the line

C5 is **not** the `BoundsElide` pathology. `BoundsElide` is byte-identical
to `dev` on all four shipped programs — it changes nothing the appliance
runs. C5 changes shipped code, on two of four programs, and lowers their
cycle count at every point of the product box under the strictest question
this repo has.

It is nonetheless **marginal** and should be read that way: 3 customers, 9
words, 1 cycle each on two programs. It survives on `MOVN` alone; every
one of its bitmask-immediate customers on shipped code is already gone,
eaten by `MaskCheck`. So the honest disposition is to keep it and write
down the tripwire:

> **If `MaskCheck`'s coverage grows to eat the remaining `MOVN` customers,
> C5 becomes dead weight and the doctrine is to delete it — not to look
> for another baseline where it still wins.**

The leave-one-out test is that tripwire, and its failure message says so.

## What changed in the tree

- `OptId::WFormMul` **not** added; C1's W-form selection stays
  unconditional (1790). `codegen.rs`'s emit-site comment carries the new
  reason.
- `ITEM_C_SMOKE`'s C5 row: baseline `[NarrowImm]` → `[NarrowImm, RegAlloc]`,
  with the full history at the row.
- `each_release_opt_is_re_asked_alone_on_the_product_tier`: same baseline
  correction; `PINNED_PRODUCT_TIER_VERDICTS` row for `WideImmForms`
  `veto` → `wins`, with the measurement.
- New deep-lane test
  `item_c5_earns_its_place_by_leave_one_out_on_the_product_tier`.
- **No emission change, and it is checked rather than asserted.** The
  whole `codegen.rs` diff for this follow-up is comment-only — `git diff`
  filtered to non-comment lines is empty — and `opts/mod.rs` is untouched,
  so `RELEASE_OPTS` is byte-for-byte what item E's merge left. Goldens are
  therefore untouched (spot-checked: `cost-product` 4/4 and `asm-arith`
  1/1 ok), and `diff-eval` was not re-run because nothing here can move an
  emitted word.
- Deliberately **not** touched, so item F's live worktree does not
  collide: `opts/mod.rs`, and any executable line of `codegen.rs`.
