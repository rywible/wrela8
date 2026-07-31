# Item F — no-ABI: interprocedural allocation and its consequences

Sibling working record for [codegen-pareto.md](codegen-pareto.md) item F
(decision 1709). Decision block **1770–1779**, plus 1780. Branch `cp-F`,
based on `bac328db` — master with items A, B, C, D, E and H merged. F6
was **cut at activation** by decision 1770 and nothing here builds it.

The thesis under test: AAPCS64 exists so strangers can call your code,
and a sealed image has no strangers. Two of its four consequences
survived the ∀ gate as named opts, one landed unconditionally with the
gate scoring it at zero, and one is reported as unreachable. All four
numbers are below.

## What landed, per sub-item

| | Claim | Disposition |
| --- | --- | --- |
| **F1** | Interprocedural allocation, custom convention per function, computed globally | Landed as `OptId::InterprocRegs`. **Wins ∀ on both tiers.** |
| **F2** | No callee-saved discipline | Landed, in both directions: the caller keeps values in registers the callee was *measured* not to destroy (part of `InterprocRegs`), and the callee stops saving `x30` where nothing can clobber it (part of `Frameless`). |
| **F3** | Frameless functions | Landed as `OptId::Frameless`, in two steps. **Wins ∀ on both tiers.** |
| **F4** | Arbitrary-arity register passing, multi-value returns | **Partial.** The pool reached the rest of the register file (9 → up to 27). The arity ceiling and multi-value returns did **not** move; see "what I could not do". |
| **F5** | Universal tail calls | Landed **restricted and unconditional**. The gate scores it at exactly zero on all twenty cases of both tiers, so freeze 1714 keeps it out of `RELEASE_OPTS` (decision 1779). "Universal" did not survive contact with the measurement. |

## The convention-computation algorithm, in a paragraph

`regalloc::allocate_program` is handed one `FnInput` per sync function —
the probe facts item E already built, unchanged in shape. It builds the
call graph from the probe's own `Reloc::Call` targets rather than from a
second walk of MWIR, so a call the emitter makes and the analysis does
not see cannot exist. It then processes functions **callee before
caller**: repeated passes take every function all of whose named callees
are already done (Kahn's algorithm over a `BTreeMap`, so the visit order
is a function of the keys alone). Each function so reached gets
`free_pool` = `WIDE_POOL` minus every register its own probe emission was
measured naming, and its intervals are allocated with the *real* clobber
set of each callee they span — an interval is no longer refused for
crossing a call, it is refused only from the registers that callee
destroys, and dropped only when that leaves none. Its own clobber set is
then `measured ∪ homes ∪ ⋃ callees`. When a pass makes no progress,
everything left is on or above a call-graph cycle: every one of those is
stamped `ALL_REGS` **before** any of them is allocated, so a recursive
callee can never be believed. There is deliberately **no iteration to a
fixpoint**: `homes` is not monotone in the clobber sets it is computed
from (shrinking a callee's clobbers lets the caller allocate *more*
registers, growing its own), and a fixpoint over a non-monotone function
is not a proof. A callee whose body this compiler does not hold — a
runtime helper, an async turn body, a `rt_enqueue` key layout may
redirect, or a `CostRule::Call` word with no `Reloc::Call` beside it
(the checkpoint service) — clobbers `ALL_REGS`, which is exactly the
refusal item E applied to every call.

## Decisions

| # | Decision |
| --- | --- |
| **1771** | The whole-program convention is one named opt, `OptId::InterprocRegs`: a per-function pool measured from that function's own emission, and a per-callee clobber set in place of a blanket call barrier. It is inert without `RegAlloc` — there is no allocation for it to extend — which is why it is asked over a chained baseline and never over `dev`. |
| **1772** | `OptId::Frameless` is F3, in **two** steps, because they are two different savings with very different reach. (1) A function no `BL` ever returns into does not save `x30`: the slot, the `str` and the `ldr` all go. This is independent of residency, so every leaf gets it. (2) If dropping that slot leaves no bytes at all, `sub sp`/`add sp` go too. |
| **1773** | F5 substitutes a tail-position `Call` with a `B`, keeping the *same* `Reloc::Call` so every downstream consumer (reachability, the cross-core resolver, `validate`) still sees the call edge it is. `layout::patch_bl` preserves the `B`/`BL` bit the emitter encoded instead of overwriting it, and fails closed on a word that is neither form. |
| **1774** | Decision 1763 ("`RegAlloc` last") stands as written; item F's ids follow it. 1763 requires every opt whose transform the **probe** must see to run before the allocator. Neither of item F's is such an opt: `InterprocRegs` changes which register the allocator may choose, `Frameless` is read off the allocation's own result, and F5 is applied at emission only where `Frameless` already fired — the probe never substitutes it (1776). `RELEASE_OPTS` is asserted to admit only allocation-reading opts after `RegAlloc`. |
| **1775** | **Proposed and refused by the gate.** F3's first form relaxed decision 1765's two-read rule when the only thing between a function and no frame was single-read temps. It is not in the tree; the measurement is below. |
| **1776** | The probe **never** substitutes a tail call. It emits the ordinary `BL`, records the barrier and records the call edge, so the allocation is sound whether or not the jump is finally emitted. That is what breaks the circle (F5 depends on F3, F3 on the allocation, the allocation would otherwise depend on F5). F5 then rides on `Frame::lr_saved`: where `x30` was never saved the teardown is at most one `add sp`, so the jump replaces three or four words with one or two *and* deletes a returning call. |
| **1777** | The attribution oracle's `rel_win ≤ Σ singles` bound is restated. Item F breaks it and the break is the finding: both of its ids are unrankable against `dev`, so `Σ singles` is a sum missing two terms. The bound is now asked over the list **as it stood before item F**, whose members are all rankable alone, and `release` is required to beat that. |
| **1778** | `blocklayout`'s "every extra word is an accounted repair jump" equality is kept, with the frames-regained term made explicit and asserted to be zero. Under decision 1775's refused form it was **not** zero — reordering blocks could cost a function its residency and hand back a frame — and the term is what makes the equality a measurement rather than an assumption about a pass that is not on the compile path. |
| **1779** | **F5 gets no `OptId`.** The ∀ gate scores the substitution at exactly zero on all twenty cases of both tiers, and freeze 1714 keeps an unrankable transform out of `RELEASE_OPTS` — item C1's disposition under decision 1746, reached for the same reason. It lands unconditionally instead: it can only fire where `x30` was never saved, where it strictly deletes words, and it is unreachable under `dev` without `Frameless`, so it needs no knob. Two units pin this: `f5_has_no_opt_id` (cheap) and a deep lane that pins the zero, so a change that makes it fire re-opens the question loudly. |
| **1780** | Item F's two ids join `PINNED_PRODUCT_TIER_VERDICTS` rather than getting a second pinned table, and each is asked there over its own link in the chain for decision 1747's reason. Both rows read `wins`. |

| **1781** | **A key some later stage may own the body of is opaque to the allocator, and every published convention is verified against the emitted code — twice.** This is the defect the boot lanes caught; it has its own section below. |

Nothing outside 1770–1781 was numbered; 1770 is the plan's own F6 cut and
was not re-used.

## The pool: nine registers, then twenty-seven

Item E's pool was `x19..=x27` — nine — because a per-function allocator
cannot know which of `x0..x17` the emitter will want. It never had to
guess. The probe already reports every register the function's own
emission names, and item E already intersected `POOL` with that
complement *as a safety net*. Item F deletes the hand-picked part and
keeps the net, because the net was always the load-bearing half.

`WIDE_POOL` is `x0..=x17` + `x19..=x27` — **27 registers** — minus, per
function, `free_pool`'s measured subtraction. What is still reserved, and
why each is not negotiable:

| Register | Why |
| --- | --- |
| `x18` | The AArch64 platform register. Taking it would be a free extra register and item E's one line of restraint is kept. |
| `x28` | `X_FRAME`, the async turn base. Its whole job is to survive a `BL`. |
| `x29` / `x30` / `sp` | Frame pointer (never touched by this ABI), link register, stack pointer. |

Measured pool sizes on `boot-blk-two-devices`, from `--stage=report`:
**19–22 registers** for real driver code, **27** for a body that names
nothing. The subtraction is what stops `x0..x8` from being handed out in
a function that has arguments or makes calls — those registers appear in
the probe's `regs` union at the prologue's parameter stores, at every
call site's argument loads, and at `x8` for an aggregate result, so they
are removed without anyone having to enumerate the reasons correctly.

## Both tiers' ∀ verdicts, with real numbers

`table_digest=b2484a1b9c00d7fa`. Corpus: 16 micro + 4 product = 20 cases.

### Each id alone, over its own link in the chain

`unit:each_item_f_opt_wins_over_the_whole_box_alone`, deep lane, exit 0.

| id | baseline | micro | product | total points/side | verdict |
| --- | --- | --- | --- | --- | --- |
| `InterprocRegs` | `RELEASE_OPTS` minus item F (7 opts) | 16 cases, 25 088 points, **wins at every point** | 4 cases, 10 240 points, **wins at every point** | 35 328 | **wins** |
| `Frameless` | the above + `InterprocRegs` | 16 cases, 25 088 points, **wins at every point** | 4 cases, 10 240 points, **wins at every point** | 35 328 | **wins** |

No case rises at any point of either tier, and at least one case falls
everywhere in each tier (decision 1782's per-tier quantifier).

### The product tier alone, per `RELEASE_OPTS` member (decision 1717)

`unit:each_release_opt_is_re_asked_alone_on_the_product_tier`, deep lane,
exit 0. 10 240 points/side over 4 cases for every row.

| id | baseline | verdict |
| --- | --- | --- |
| `BoundsElide` | `dev` | veto (item H's finding, unchanged) |
| `NarrowImm` | `dev` | wins |
| `AdrAddressing` | `dev` | wins |
| `BfxNarrow` | `dev` | wins |
| `MaskCheck` | `dev` | wins |
| `WideImmForms` | `[NarrowImm]` | veto (item H's finding, unchanged) |
| `RegAlloc` | `dev` | wins |
| **`InterprocRegs`** | `RELEASE_OPTS` minus item F | **wins** |
| **`Frameless`** | the above + `InterprocRegs` | **wins** |

**Neither of item F's ids is a microbenchmark-only win.** Both fall on
every one of the four programs the appliance ships, at every point of
each one's residual box. That was the outcome decision 1717 was written
to make checkable and the one it did not get for `BoundsElide` and
`WideImmForms`.

### The list as a whole

`unit:release_wins_at_every_point_of_the_residual_box`, `dev → release`:
**wins at every point**, 36 864 points/side (micro 26 624 over 16 cases,
product 10 240 over 4). Freeze 1714's actual gate, still green with the
two new ids in the list.

### F5's zero

`unit:tail_calls_are_not_rankable_because_the_gate_corpus_never_fires_them`,
deep lane: **no cost-corpus case fires a single tail call**, in either
tier. The cost-stage closures the gate ranks are 2–33 functions and
contain no frameless tail-caller at all; the transform fires 4 times in
the real `boot-blk-two-devices` image, which the gate does not score.
This is not a claim that F5 is worthless — it is the statement that this
ruler cannot see it, which is the same shape as item D's finding under
decision 1750 and item C1's under 1746.

## Per-id attribution over the corpus (pinned point, `flat` workload)

`InterprocRegs`, over `RELEASE_OPTS` minus item F:

| case | base | +InterprocRegs | Δ cycles | Δ words |
| --- | --- | --- | --- | --- |
| cost-crosscore | 4 142 | 4 095 | −47 | 0 |
| cost-icache-cliff | 21 735 | 21 679 | −56 | 0 |
| cost-itlb-span | 53 828 | 53 781 | −47 | 0 |
| cost-runtime | 1 801 | 1 754 | −47 | 0 |
| cost-product-actors | 4 121 | 4 074 | −47 | 0 |
| cost-product-appliance | 2 152 | 2 105 | −47 | 0 |
| cost-product-blk | 5 148 | 5 101 | −47 | 0 |
| cost-product-receipt | 6 673 | 6 626 | −47 | 0 |
| the other twelve | — | — | 0 | 0 |
| **SUM** | **103 479** | **103 094** | **−385** | **0** |

`Δ words = 0` everywhere is the point: this id emits nothing new and
deletes nothing. Every one of the 385 cycles is a `str`/`ldr` pair
against a temp that used to be evicted at a call and now is not, replaced
one word for one word by a `mov`. The identical −47 on five different
programs is the shared runtime closure all of them borrow.

`Frameless`, over the above + `InterprocRegs`:

| case | base | +Frameless | Δ cycles | Δ words |
| --- | --- | --- | --- | --- |
| cost-product-receipt | 6 626 | 6 427 | **−199** | −46 |
| cost-icache-cliff | 21 679 | 21 541 | −138 | −28 |
| cost-product-blk | 5 101 | 4 976 | −125 | −32 |
| cost-crosscore | 4 095 | 3 976 | −119 | −28 |
| cost-product-appliance | 2 105 | 1 986 | −119 | −30 |
| cost-itlb-span | 53 781 | 53 671 | −110 | −46 |
| cost-product-actors | 4 074 | 3 978 | −96 | −22 |
| cost-runtime | 1 754 | 1 659 | −95 | −22 |
| cost-arith-w | 149 | 93 | −56 (**−38 %**) | −10 |
| cost-mpipe-block | 561 | 506 | −55 | −10 |
| cost-arith | 136 | 88 | −48 (**−35 %**) | −8 |
| cost-branch-bias | 393 | 347 | −46 | −8 |
| cost-align | 325 | 291 | −34 | −6 |
| cost-branchy / cost-calls / cost-mem-locality / cost-ports | 134/88/151/363 | 110/64/127/339 | −24 each | −4 each |
| cost-assoc-conflict | 946 | 922 | −24 | −4 |
| cost-forwarding | 319 | 306 | −13 | −6 |
| cost-bounds-elide | 314 | 304 | −10 | −2 |
| **SUM** | **103 094** | **101 711** | **−1 383** | **−326** |

**All twenty cases fall.** Item F's block total, `RELEASE_OPTS` minus F →
`RELEASE_OPTS`: **−1 768 proxy cycles, −326 words, −105 hot-text bytes**,
printed by `unit:narrow_imm_wins_on_cycles_...` as "item F block win".

## Frame, word and cycle deltas on named cases

Word counts, `--stage=asm` under `release`, against the committed
item-E-era goldens. **Every `asm-*` case shrinks; none grows.**

| case | words before | after | Δ |
| --- | --- | --- | --- |
| asm-arith | 107 | 99 | **−8** |
| asm-runtime-probe | 1 671 | 1 649 | **−22** |
| asm-struct | 137 | 131 | −6 |
| asm-calls | 63 | 59 | −4 |
| asm-loop | 182 | 178 | −4 |
| asm-placed-index | 75 | 71 | −4 |
| asm-take | 178 | 174 | −4 |
| asm-bytes-param | 37 | 35 | −2 |
| asm-enum-match | 109 | 107 | −2 |
| asm-generic | 36 | 34 | −2 |

Two words per leaf, which is exactly F3 step 1: a `str x30` deleted from
the prologue and an `ldr x30` from the epilogue. `asm-arith` is four
leaves, `asm-runtime-probe` eleven.

Frame sizes move less than words, because a frame is rounded to 16 and
the `x30` slot is 8 — the save usually vanishes into the rounding. Where
it does not:

| fn | case | frame before | after |
| --- | --- | --- | --- |
| `double_identity` | `asm-generic` | 48 | **32** |
| `Point.sum` | `asm-struct` | 64 | **48** |
| `consume_box` | `asm-take` | 32 | **16** |
| `use_takes` | `asm-take` | 256 | **240** |
| `BlkDriver.on_queue_irq` | `boot-blk-two-devices` | 208 | **192** |
| `sum_array` | `asm-loop` | 128 | **128 (unchanged)** |

`sum_array` is unchanged on purpose: it calls nothing, so item E already
gave it every register it could use, and its `x30` slot was already
inside the rounding.

Whole-image, `boot-blk-two-devices` under `--stage=report`:

```
  Convention fns=88 frameless=50 tail_calls=4
    Fn key=BlkDriver.drain frame=2432 residents=36 regs=x1-x8 clobbers=x0-x12,x30-x31 pool=22
    Fn key=BlkDriver.init frame=880 residents=17 regs=x4-x7,x12 clobbers=x0-x12,x30-x31 pool=19
    Fn key=Ledger.mark frame=64 residents=1 regs=x2 clobbers=x0-x2,x9-x12,x30-x31 pool=21
```

88 functions with a convention of their own, 50 of them frameless, 4 tail
calls (`__wrela_boot_call` twice, `__wrela_irq_invoke`,
`__wrela_wake_invoke`). `BlkDriver.drain` keeps 36 temps in `x1..x8` — a
set item E could not reach at all — and publishes a clobber set of
`x0-x12,x30-x31`, i.e. a caller may keep anything in `x13..x27` alive
across a call to it.

## The two measurements that changed the item's shape

### Decision 1775: the gate refused the relaxation, at both tiers

F3's first form had a feedback edge into the allocator. Decision 1765
refuses a temp read once, because residency turns an independent
`str`/`ldr` pair into a serial two-`mov` chain and buys nothing. That
trade is measured against a function that *still has a frame*. When the
only thing between a function and zero frame bytes was a handful of such
temps, the question was re-asked without the rule and the relaxed answer
taken if it landed at zero. The trade looked obviously right: four more
deleted words, plus a real `str` and a real `ldr` for `x30`, against a
copy that costs nothing at low forwarding latency.

It is wrong, and the gate said so on **both** tiers:

- micro: `cost-branch-bias` **267 → 310** (+16 %) at every corner with
  `store_to_load_forwarding=1`;
- product: `cost-product-receipt` **7 551 → 7 554** at the same corners.

That is precisely the shape item E measured when it promoted every scalar
(`plans/codegen-pareto-E.md`, "what the ∀ gate caught"): a result that
turns on the swept forwarding latency. Four deleted words do not pay for
a serialized copy chain in the body.

Item E's hand-off point 4 said decision 1765 "is a policy, not an
invariant; the ∀ gate is what decides it", and asked for it to be
re-measured rather than inherited. It has now been decided a second time,
on a second baseline, over a corpus that includes four programs the
appliance ships: **the rule stands.**

Removing the relaxation left F3 reaching almost nothing — the value a
function returns is read exactly once, so under the two-read rule almost
every function keeps a frame; **zero** functions in any `asm-*` golden
were frameless. That is what pointed at the better formulation, which
does not need the allocator's permission at all: *a leaf need not save
`x30` in the first place*. That reaches every leaf, framed or not, and
it is the version that landed.

### Decision 1776: "universal" tail calls are measurably worse

The plan asks for F5 unconditionally — "every tail-position call is a
jump". Built that way, it is a **loss** on this ruler. Measured on
`RELEASE_OPTS`-minus-F plus unconditional tail calls, against the same
list without them:

| case | Δ cycles | Δ words |
| --- | --- | --- |
| cost-runtime, cost-crosscore, cost-icache-cliff, cost-itlb-span, and all four product cases | **+29 each** | 0 |
| cost-calls | 0 | −1 |

The mechanism, read off the asm diff rather than guessed: the four sites
are `finish_abort` calls in the runtime's line-commit path, in functions
that have *other* returning calls and therefore still save `x30`. The
substitution replaces `bl <finish_abort>` (Call) + `str x0, [sp,#d]`
(Store) + `b <epilogue>` (Branch) with `ldr lr, [sp,#N]` (**Load**) +
`add sp` (Alu) + `b <finish_abort>` (Branch). It puts a load where a call
and a store were, at the same word count. The dynamic win — not executing
the callee's `ret` back into us, nor our own epilogue — is invisible to a
static `f ≡ 1` model, exactly as block order is invisible to the
footprint term (decision 1750).

Where `x30` was never saved, the teardown is at most one `add sp` and the
jump replaces three or four words with one or two while deleting a
returning call. So F5 fires there and nowhere else. On the gate corpus
that is *nowhere*, hence decision 1779.

## The defect the boot lanes caught (decision 1781)

**The first landing of this item broke 14 guest transcripts**, and every
cheap oracle was green while it did: 851 units, both ∀ tiers at 35 328
points a side, and `diff-eval` agreeing on 129 tests across 47 cases. The
orchestrator found it by merging and re-pinning the boot lanes. This
section is the post-mortem, because the lesson is worth more than the
fix.

### Root cause

`allocate_program` gave a callee a **measured** clobber set whenever its
key was present in `MwirProgram::fns`. That is the wrong test. `layout.rs`
runs after codegen and **replaces compiled bodies under keys codegen has
already published a convention for**:

| layout site | what it replaces |
| --- | --- |
| `harness::install_abort_tail_floor` | "*Replace the compiled `__wrela_abort_tail` stub* with the floor long-jump." |
| `harness::inject_test_runner_fns` | "*overwrite `__test_call_*` / `__test_prefix_*`* with specialized bodies." |
| `harness::inject_boot_init_fn` | fills in `rt_boot_init`, aliasing `rt_boot_init 0` |
| `harness::inject_rt_enqueue_and_dispatch_fns` | republishes `__enqueue_i` under `rt_enqueue <Actor>`, and inserts `__method_i` stubs |
| `harness::inject_rt_cross_core_fns` | prepends an SP install and republishes as `rt_secondary_core_entry <n>` |
| `apply_resume_remaps`, `resolve_cross_core_edge` | re-point calls at `__rt_xsend_*` trampolines |

So a caller was told "`__boot_call_0` clobbers `x30` and `sp`", kept a
live value in `x0`/`x9`/`x10` across it, and layout then filled in a body
that destroys exactly those. The corruption lands in the boot-init and
test-runner path, which is why the failure signature was the scheduler:
`boot-actor-chain` went `turns=2 run_one=3` → `turns=1 run_one=1`, and
`boot-actor-reply-result` blew its loop budget with `turns=0`.

Bisected in three boot runs: `RELEASE_OPTS` minus `Frameless` still fails
(so not F3/F5); minus `InterprocRegs` passes (so F1/F2); and narrowing
the base pool back to item E's nine **while keeping the per-callee
barrier** still fails — so it is the barrier, not the widened pool.

### Why nothing cheap saw it

`diff-eval` compares the tree-walking evaluator against the backend on
`@test` bodies. It never runs the scheduler, never crosses a turn
boundary, and — decisively — it never runs `layout`'s substitutions
against a program whose callers were compiled against the *pre*-
substitution bodies. The ∀ gate scores `CodegenProgram`s and does not
execute anything at all. The units test the analysis against itself. Every
one of those oracles was asking "is this consistent?" and none was asking
"is this **true of the program that finally ships?**"

### The fix, and why it is a rule and not a list

`FnInput::opaque_body`: a key some later stage may own publishes
`ALL_REGS`. Codegen decides it from the spelling —
`is_compiler_glue_symbol(key) || key.starts_with("__")` — and
deliberately does **not** enumerate the table above. A second source of
truth about which keys layout owns is the defect class itself, not a fix
for it: the table would be correct today and wrong the next time layout
grows a substitution, and nothing would say so.

Cost of the rule: **43 proxy cycles** of item F's 1 768-cycle block win
(`InterprocRegs` −428 → −385), and 96 → 88 functions with a convention of
their own on `boot-blk-two-devices`. The frameless count and the tail-call
count are unchanged, because F3 and F5 are properties of a function's own
body rather than claims about somebody else's.

### The oracle — the part that matters

`codegen::verify_conventions` refuses a program in which any published
clobber set is not a superset of

> every register that function's own **emitted** words name, unioned with
> every callee's published clobber set over the `Reloc::Call`s the
> emission actually pushed

with an unconventioned callee — an async turn body, hand-assembled glue,
a key layout re-points — contributing `ALL_REGS`. It is O(words) and it
runs **twice**:

1. at the end of `codegen_program` / `codegen_program_with_async`, where
   it catches an analysis that disagrees with its own emitter; and
2. in `layout_program`, **after** every `inject_*` and floor
   substitution, where it catches a body some later stage replaced. This
   is the one that would have caught the real defect; codegen's own check
   cannot see it by construction, because the substitution has not
   happened yet.

Reverting the fix and building `boot-actor-chain` now fails the build
with the function, the registers and the call named:

```
error[build]: layout: internal error: fn `__boot_call_0` was published as
clobbering x30-x31, but its emitted code reaches x0,x9-x10,x30-x31 (via
its call to `Inner.init`). Every caller that kept a value in x0,x9-x10
across a call to it has been miscompiled.

This check runs *after* layout's `inject_*` and floor substitutions, so
the usual cause is a body this stage replaced or aliased under a key
codegen had already published a convention for. A key a later stage may
own must be opaque to the whole-program allocator
(`regalloc::FnInput::opaque_body`), never given a measured clobber set.
```

Four cheap units back it, in `codegen::item_f_tests`:

- `verify_conventions_refuses_a_clobber_set_the_code_exceeds` — the
  negative case. A check that cannot fail is not a check.
- `verify_conventions_refuses_a_clobber_set_a_callee_exceeds` — the
  transitive case, which is the real defect's exact shape.
- `an_unconventioned_callee_forces_its_caller_to_be_opaque`.
- `every_key_a_later_stage_may_own_is_opaque_to_the_allocator` — asserted
  against the symbol **constructors** (`rt_boot_init_symbol()`,
  `rt_enqueue_symbol`, `rt_secondary_core_entry_symbol`) rather than
  hand-written spellings, and paired with the converse, so the rule
  cannot degenerate into "everything is opaque".

Because the check runs inside `layout_program`, it is exercised by every
golden case that lays out an image, by the `async` fuzz lane (which lays
out test images), and by any future item that builds one — not only by a
boot transcript.

### The general lesson, stated for the plan

Six items have now shipped on units plus `diff-eval`. That pair verifies
**semantics of a computation**. It does not verify a **claim one compiler
stage makes about another stage's output**, and item F is the first item
whose central mechanism is exactly such a claim. The generalisable rule
this suggests: *when a stage publishes a fact that a later stage can
falsify, the fact must be re-checked against the later stage's output,
and that check belongs in the build rather than in a test.* Items E and F
both had a version of this available and only item E took it — item E's
probe measures a real emission rather than modelling one, and the
equivalent for item F was to measure the real *final program* rather than
the one codegen handed over.

## Boot lanes (run after the orchestrator lifted the restriction)

`cargo xtask golden --only-boot`, whole suite: **zero `[test]`
expectations differ**. Every guest transcript matches its pinned
expectation byte for byte, including all fourteen the orchestrator
listed. The 60 remaining differences are pinned artifacts for the
orchestrator to re-pin: 24 `asm.txt`, 33 `report.txt`, 1 `cost.txt`.

Individually confirmed green on the guest transcript:
`boot-actor-chain`, `boot-actor-reply-result`, `boot-actor-reply-struct`,
`boot-actors`, `boot-blk-roundtrip`, `boot-blk-two-devices`,
`boot-cores-1`, `boot-dma-pool`, `boot-driver-message`, `boot-init-args`,
`boot-receipt-handoff`, `check-await-question-mark`,
`err-boot-driver-message`, `err-boot-receipt-handoff`.

## `diff-eval`, verbatim

`cargo xtask diff-eval`, exit code 0:

```
diff-eval: 129 test(s) agree across 47 case(s), 8 lowering-skips, 6 exhaustive-skips, 1 quota-skips, 0 import-skips
```

Nothing disagreed. The four `lowering failed closed` lines in the
transcript are the pre-existing skips (`check-const-generic-struct`,
`check-generic-order-f64`, `check-import-lower`,
`check-try-none-propagates`), unchanged from item E's run. This is the
oracle that matters most here: item F changes how every function
receives, keeps and returns values, and the tree-walking evaluator agreed
with the backend on all 129.

Other lanes run: `cargo xtask corpus` (lexed 33, parsed 26, 7 fragments
skipped), `cargo xtask stdlib-test` (28 comptime tests across 2 files,
ok), and all eight `fuzz` lanes at 200 iterations — every one clean with
its reach printed (`lower`: 18 check_typed / 18 codegen Ok; `async`: 25
flowwir_lower, 7 test images laid out).

## Golden diff shape (read-only; not re-pinned here, per decision 1708)

`cargo xtask golden --no-boot`: **79 expectations move.**

| stage | count |
| --- | --- |
| `report.txt` | 29 |
| `asm.txt` | 20 |
| `cost.txt` | 20 |
| `build` (whole-image report) | 6 |
| `img.hex` | 4 |

**Zero `err-*` cases appear**, so **no `error[...]` diagnostic moved
anywhere in the corpus** — the same property item E reported.

The `asm`/`cost`/`img` moves are the expected ones: every leaf loses two
words, every address after it shifts. The 29 `report.txt` moves are
section sizes plus the new `Convention` section, which is the largest
single addition this item makes to a pinned artifact: **~8 KB on a
23 KB report** for `boot-blk-two-devices`, 89 lines. It is a real cost to
the review surface and it is deliberate — the plan's own reason is that
the convention is otherwise invisible — but a future item that moves
residency will move 88 lines of golden with it. The section is
absent under `dev` and under item E's per-function allocator, so no `dev`
report moves.

## Ratchets and pinned measurements moved deliberately

| what | before | after | why |
| --- | --- | --- | --- |
| `tests/census.toml` `[emitted_a64] backend_emitters` | — | `+emit_frame_teardown`, `+emit_tail_call` | two new emitters |
| `[emitted_a64.encode_enc_sites_by_file]` | codegen 317, layout 9, total 356 | 318 / 10 / 358 | `emit_tail_call`'s `B` and `patch_bl`'s form-preserving `enc_b` |
| `[internal_error]` `layout.rs` / total | 59 / 210 | 60 / 211 | `patch_bl` refuses a word that is neither `B` nor `BL` |
| `[internal_error]` `codegen.rs` / total | 4 / 211 | 6 / 213 | `verify_conventions`' two producer-bug guards (decision 1781) |
| `cost-branchy` flat total (`cost::compose`) | 134 | **110** | −18 % on one case |
| item C1's crossover (`opts::win`) | (149, 152) | **(93, 96)** | item F removes more frame slack, so C1's W-form win grows from 3 cycles to 3 on a much smaller base — the crossover item C predicted is now wider, not narrower |
| `blocklayout` `BEFORE_HOT_TEXT_BYTES` | 7 616 | **7 616 (unchanged)** | F3's deletions land in leaves that are cold on `boot-actors`' measured vector; the constant was re-derived, not assumed |

Default `cargo test -p wrela-compiler --lib`: **15.8 s** (was ~20 s
before this item — the emitted code is smaller). The item F smoke lane
was 53 s when pointed at `cost-crosscore` (16 384 corners) and is 3.1 s
pointed at `cost-runtime` (1 024 corners), which is the same transform;
`bench/thresholds.toml`'s `[tests]` note is the reason that mattered.

## What I could not do, and why

- **The boot lanes were the only oracle that saw the real defect, and my
  own prediction about them was half right in an instructive way.** Before
  running them I wrote that a `boot-actors` failure would mean "a callee
  clobbers a register its measured `regs` union did not name", and named
  the first suspect as "a `Reloc::Call` whose target layout *redirects* —
  glue keys and `rt_enqueue` targets are excluded from the graph for
  exactly this reason, so a **new** redirect class would be the bug". The
  mechanism was right and the scope was wrong: the exclusions I had built
  covered *redirects* and not *replacements*, and `layout.rs` does both.
  Writing down what a failure would mean was worth doing — it named the
  right mechanism in one line — but it is not a substitute for running the
  lane, because the thing it got wrong is precisely the thing I could not
  have reasoned my way to. Decision 1708 was the right call for the round
  and it cost this item a merge; the durable answer is not "run more boot
  lanes" but the build-time check in decision 1781, which makes the class
  fail without one.

- **F4's arity ceiling and multi-value returns did not move.** The
  `more than 8 call arguments` refusal is still there, and MWIR has no
  multi-value return to lower. Both are buildable; neither is
  *exercisable*: nothing in the corpus or the image passes nine
  arguments, and lifting the ceiling means the prologue's aggregate copy
  (`load_ptr` through `X_A = x9`) starts colliding with argument
  registers above `x8`. Building an unexercised widening and pinning a
  unit against it is the green-unit-that-is-not-an-oracle freeze 1714
  forbids, so the ceiling stays and says so. The half of F4 that *was*
  reachable — getting the rest of the register file — landed.
- **The `mov` pairs around call sites survive**, which item E's hand-off
  point 3 named as "where F's biggest win is hiding". They do not move,
  for a reason worth writing down: they exist because a temp cannot be
  homed in the argument register it is about to occupy, and `free_pool`
  removes exactly those registers from the pool the moment the function
  makes any call at all — the prologue's parameter stores and the call
  site's argument loads both name them, so the measurement that makes the
  wide pool safe is the same measurement that withholds `x0..x7` from
  every caller. `asm-calls`'s `combo` is **byte-identical** before and
  after this item. Homing a parameter in its own incoming register needs
  a per-point interference model rather than a per-function union of
  registers, and that is a different item.
- **No coalescing, no spill/reload forwarding** — decision 1700 puts the
  latter out of this plan, and item E already recorded it.
- **The async path is untouched** (decision 1762). `build_frame_flow`
  passes `Assignment::none` and `save_lr = true`; a turn body's `x30`
  belongs to the scheduler and every suspension is a `ret` back to it.
  This was a contract to respect, not a limitation to lift, and nothing
  here lifts it.
- **`cargo xtask check`, `bench`, `profile`, `repro` were not run**
  (decision 1708). So there is **no measured wall-clock number** for the
  whole-program pass. Its cost is one probe emission per sync function —
  the same count item E already paid — plus one Kahn sweep over the call
  graph, which is `O(fns²)` in the worst case with a `BTreeSet`
  membership test per edge. On a 268-function image that is nothing, but
  it is unmeasured and `bench compiler` at the close is where it should
  be looked at.
- **`use_it`-shaped functions emit three dead words.** A function all of
  whose `Return`s are swallowed by tail calls still emits its epilogue,
  because suppressing it would need a reachability pass over the emitted
  word stream. Three words per fully-tail-called function, and F5 fires
  four times in the image, so the whole cost is bounded and known.
