# Item E — per-function register allocator: findings

Sibling working record for [codegen-pareto.md](codegen-pareto.md) item E
(decision 1709). Decision block **1760–1769**. Branch `cp-E`, based on
`01a7ca24`. Items A, B and C ran in parallel; none of their work is
assumed here.

## The allocator, in a paragraph

`crates/wrela-compiler/src/regalloc.rs` is a per-function linear scan
over the existing per-fn MWIR (decision 1701 / freeze 1712) — no SSA, no
graph colouring, nothing interprocedural. **Live ranges** are the closed
interval between the first and last program point that touches a temp,
over a point space where `0` is the prologue, `1..=body.len()` is the
body and `body.len()+1` is the epilogue (so parameter stores and `mut`
write-backs are ordinary touches, not special cases). Intervals are then
widened to a fixpoint over back edges: every branch whose target index is
at or before its own widens **every** interval it intersects to cover the
whole loop span. That widening is what makes a linear interval sound
against non-linear control flow, and the module doc carries the argument:
if `t` is live at a point outside its interval, the path proving it must
contain a back edge that intersects the interval, so the fixpoint has
already widened `t` to cover that point. **Scan order** is by interval
start, ties by temp number (determinism through dumbness); the active set
is kept sorted by end and expired from the front. **Spill heuristic** is
furthest-end: when the pool is empty, the latest-ending active temp is
evicted back to memory unless the incoming interval outlives it, in which
case the incoming one stays in memory. Eviction is free and always
correct because nothing has been emitted yet — "spilled" means "keeps the
frame slot it would have had anyway", i.e. exactly the spill-everything
behaviour `dev` keeps permanently (M19 freeze 1407). **The register set
is `x19..=x27`, nine registers**, and the reason it is nine and not the
~28 the plan estimated is in its own section below.

The thing that makes this trustworthy rather than merely plausible: the
allocator is handed **facts measured by a real emission pass**
(`codegen::probe_fn_facts`), not a hand-written classification of MWIR
instruction shapes. The probe runs the actual emitter once against the
naive frame and reports which temps were touched only as whole 8-byte
scalars, which words are returning calls, and which registers the
function's own emission already names. A second model of `emit_one` would
drift; a measurement cannot.

## Decisions

| # | Decision |
| --- | --- |
| **1760** | The allocator is a named opt (`OptId::RegAlloc`) with a TLS knob, added to the `RELEASE_OPTS` `const` slice and nowhere else (M19 freeze 1402). `dev` leaves it off, so spill-everything survives permanently as the correctness reference (M19 freeze 1407). |
| **1761** | Residency is decided from a **probe emission**, not from a static classification of MWIR shapes. `probe_fn_facts` runs the real emitter against the naive frame and reports what it did. A hand-written table of "which operand positions are plain scalar uses" would be a second source of truth that can disagree with the emitter, which is the defect the whole probe exists to prevent. Cost: one extra emission pass per function under `release`, none under `dev`. |
| **1762** | **The async path never allocates.** `build_frame_flow` passes `Assignment::none`. An async fn's locals must survive its own `ret`-to-scheduler suspension and a physical register does not — that is the entire reason `X_FRAME` and the persistent turn area exist. This is not a limitation to lift later; it is the contract. |
| **1763** | `RegAlloc` is **last** in `RELEASE_OPTS`. The allocator decides against the emitter's output, so every opt that changes emission must already be on when its probe runs, or the probe measures a program that is never built. |
| **1764** | The ∀ gate for this item is measured against `RELEASE_OPTS` **minus the allocator**, not against `dev`, so the verdict is about this item rather than about the whole mode. |
| **1765** | **A register has to pay for itself: a temp needs two reads.** Forced by the gate; see "what the gate caught" below. |
| **1766** | A resident temp keeps a **virtual** frame offset rather than losing `Frame::off` entirely, so the ~220 existing emission sites are untouched and only the three slot helpers translate. Any virtual offset that reaches a helper without a register behind it, and any `addr_of_slot` on a resident temp, is a **build failure**, not a fallback. |
| **1767** | The call barrier is at **word** grain, not point grain. Point grain would refuse every call argument and every call result, because `Inst::Call` emits its argument loads, its `bl` and its result store inside one point. |

Nothing outside 1760–1769 was numbered.

## Exclusivity vs. alias analysis — what was confirmed, and where

The plan asserts (decision 1701) that "wrela's exclusivity rules mean no
alias analysis is needed to establish a temp's live range". Confirmed,
with a caveat about what the confirmation is worth.

**Where.** [02-language.md §3](../docs/language/02-language.md), the
access-mode table and the sentence under it: `read` is a loan that "may
coexist with other reads"; `mut` is "exclusive for its duration", and
"while `mut x` is active, no other path may touch the same storage;
exclusivity is checked on storage paths (fields, indexes, and potential
overlaps), not variable names"; `take` leaves the source uninitialized.
A `mut` binding is the only way to obtain a writable second path to a
value and it is exclusive by construction, so a scalar local has no
second path to its bytes.

**The caveat, and what was built because of it.**
[04-compiler.md §5](../docs/language/04-compiler.md) is blunt: "every
backend fact (aliasing, ranges, alignment) must trace to a semantic proof
— never be invented from naming or optimism." A language-level argument
about `mut` is a statement about *source* paths; what an allocator needs
is a statement about *emitted* paths, and those are two different things
— the emitter itself takes the address of a frame slot whenever it needs
an aggregate pointer or an array base, entirely below the level the
access modes describe. So the exclusivity rule is not used as the proof.
It is used as the *reason the proof succeeds*: because no source-level
alias exists, a scalar temp's only address-takers can be the emitter's
own, and the probe observes those directly (`Touch::Escape` on every
`addr_of_slot`, every interior-offset access, and every base access on a
slot that is not exactly 8 bytes). A temp with any escape is refused
outright. The result is stronger than an alias argument and is checked
per function rather than assumed once.

## The pool: nine registers, not twenty-eight

The plan budgeted "~28 usable GPRs". The honest number available to a
first allocator that does not rewrite the emitter is **nine**, `x19..=x27`:

- `x0..x8` — argument/result registers.
- `x9..x14` — `codegen.rs`'s fixed scratch set (`X_A..X_F`).
- `x15`/`x16` — `emit_format_scalar`'s digit loop (`X_I_REG`/`X_N_REG`).
- `x15`/`x16`/`x17` — `emit_group_create` (`X_ARENA`/`X_CAND`/`X_TAG`).
- `x18` — platform register.
- `x28` — `X_FRAME`, the async turn base.
- `x29`/`x30`/`sp` — fixed.

Two cross-checks were run rather than assumed. First, the claim in
`codegen.rs:585` that x28 was chosen clear of "every register the
hand-assembled runtime routines in `layout.rs` use (`x9..x17`)` is
**stale**: M11 items F/G/J deleted those emitters and the runtime is now
force-rooted wrela in `stdlib/core/runtime.wr`, compiled by the ordinary
per-function emitter. What survives outside it is `layout/harness.rs`'s
stub emitters (`x9..x11` plus `sp`) and the glue emitters listed in
`tests/census.toml`'s `non_inventory` (`x0..x9`, `x30`, `sp`). Neither
reaches x19. Second, nothing preempts a running function behind its back:
[06-machine.md §4](../docs/language/06-machine.md) gives the machine no
emulated GIC and no exception vector table — "the guest observes vectors
**only at checkpoints and parks**" — and a checkpoint is an inline poll
plus a `BL`, which is already a `CostRule::Call` barrier here.

Belt and braces on top of both: the probe unions every `dst` and `src` of
the function's own baseline emission, and the pool is intersected with the
complement. A register the emitter touches for any reason cannot be handed
to a temp even if `POOL` were widened by mistake.

Reaching the full ~28 requires making each emission site
allocation-aware, so the emitter's scratch set stops being reserved. That
is a strictly larger change and it belongs with item F.

## What the ∀ gate caught, and what it cost

**The first landing failed the gate.** With every scalar temp promoted,
`compare_opt_lists_over_box(release-minus-RegAlloc, release)` refused it
and named one case: **`cost-calls` rose**, by `+2` at every corner with
`store_to_load_forwarding=1` (69 → 71) while falling `−13` at every corner
with `=4` (84 → 71). That is exactly the shape item E's own section warns
against — a result that turns on the swept store-to-load-forwarding
latency.

The mechanism is real, not a modelling artifact. Residency rewrites each
access one for one: a `store_slot` becomes `mov home, reg`, a `load_slot`
becomes `mov reg, home`. For a temp written once and read once that is not
a residency at all, it is a **copy** — the value goes into its home
register and comes straight back out. An independent `str`/`ldr` pair
(dispatched to the L and V pipes, with no register RAW edge between them,
so the scheduler can slack around it) becomes a strictly serial two-`mov`
chain on pipe I. `cost-calls` is a program made entirely of such copies.

Decision 1765 is the answer: **a temp needs two reads to be promoted.**
Reads are the right unit because a register saves reloads, and one write
with one read deletes no reload. The ruler was not touched, no threshold
in `bench/a76-pi5.toml` was moved, and no test was disabled.

**Verdict after the fix — the real numbers.**

`cargo test -p wrela-compiler --lib regalloc_wins_at_every_point_of_the_residual_box -- --ignored`
→ **passed**, 334.95 s. 15 cases, nominal box cardinality 131 072, `k`
between 8 and 14 swept dimensions per case, **24 576 corners scored per
side** (256+1024+256+256+512+512+512+16384+256+1024+1024+256+1024+256+1024).
No case rises at any point; at least one case falls at every point. The
smoke lane (`cost-branch-bias`, in the default `cargo test`) falls at all
512 of its corners.

**Cost of decision 1765, stated rather than hidden.** The allocator's own
corpus win drops from −33 910 to −23 257 proxy cycles, and `asm-loop`'s
`sum_array` frame goes 160 → 128 rather than 160 → 80. That is the price
of a result that holds at every point of the box instead of at the pinned
one.

## Per-opt attribution (pinned point, `flat` workload)

| config | proxy cycles | words | charge | hot text |
| --- | --- | --- | --- | --- |
| dev | 124 637 | 112 186 | 29 657 | 476 416 |
| BoundsElide alone | 120 045 | 107 416 | 29 657 | 457 344 |
| NarrowImm alone | 118 165 | 90 721 | 21 445 | 384 000 |
| **RegAlloc alone** | **101 380** | 112 186 | 29 657 | 476 416 |
| release (all three) | 90 733 | 87 859 | 21 445 | 372 480 |

RegAlloc alone is **−23 257 cycles, 18.7 %** — the largest single opt in
the compiler by a factor of five (BoundsElide −4 592 / 3.7 %, NarrowImm
−6 472 / 5.2 %). Its `words`, `charge` and `hot_text` columns are
**identical to `dev`**, which is the point: the win is entirely
schedule-side. Every `str`/`ldr` pair against a resident temp becomes one
`mov`, one word for one word.

Per case, RegAlloc alone against `dev`:

| case | dev | RegAlloc | Δ |
| --- | --- | --- | --- |
| cost-align | 1 090 | 1 085 | −5 |
| cost-arith | 162 | 162 | 0 |
| cost-assoc-conflict | 1 605 | 1 605 | 0 |
| cost-bounds-elide | 1 839 | 1 839 | 0 |
| cost-branch-bias | 562 | 437 | **−125 (−22 %)** |
| cost-branchy | 187 | 151 | −36 |
| cost-calls | 92 | 92 | 0 |
| cost-crosscore | 4 518 | 4 387 | −131 |
| cost-forwarding | 724 | 702 | −22 |
| cost-icache-cliff | 31 208 | 24 865 | −6 343 |
| cost-itlb-span | 78 056 | 61 662 | −16 394 |
| cost-mem-locality | 543 | 525 | −18 |
| cost-mpipe-block | 656 | 609 | −47 |
| cost-ports | 1 286 | 1 268 | −18 |
| cost-runtime | 2 109 | 1 991 | −118 |

## Frame-size and word-count deltas on named cases

**Word count: exactly zero change.** The `asm-*` golden family diffs
1 633 insertions against 1 633 deletions — a word-for-word substitution.
Frames carry the size win instead.

| fn | case | frame before | after |
| --- | --- | --- | --- |
| `sum_array` | `asm-loop` | 160 | **128** |
| `sum_to` | `asm-loop` | 128 | **96** |
| `__wrela_fmt_dec` | runtime | 528 | **496** |
| `__wrela_line_commit` | runtime | 336 | **304** |
| `ascii_digit` | runtime | 272 | **256** |
| `use_takes` | `asm-take` | 272 | **256** |
| `area`, `good` | `asm-struct`, `image-*` | 192 | **176** |
| `__wrela_console_append_bytes` | runtime | 176 | **160** |
| `use_ok` | `asm-generic` | 176 | **160** |
| `__wrela_console_append_line_buf`, `copy_bytes_range`, `copy_line_buf_range` | runtime | 144 | **128** |
| `checked_add` | `asm-arith` | 32 | **32 (unchanged)** |

`asm-arith` is unchanged on purpose and is pinned as such: every temp in
it is read exactly once, so decision 1765 declines to promote any of them.
"The allocator declines where a register would not pay" is as much a
property to hold as the shrink itself.

Golden surface moved: **67 expectations** — 19 `asm.txt`, 15 `cost.txt`,
29 `report.txt`, 4 `img.hex`. **Zero `error[...]` lines change anywhere in
the corpus**; no diagnostic moved.

## `diff-eval`, verbatim

```
diff-eval: 121 test(s) agree across 46 case(s), 8 lowering-skips, 6 exhaustive-skips, 1 quota-skips, 0 import-skips
```
Exit code 0. Run twice: once against the first landing and once against
the final `pays-for-itself` policy. Both agree.

## The note that replaces the deleted `naive-locked` clause

`compiler.codegen.naive-locked` lived in `ledger/ledger.toml`, which
`aa05bf75` deleted before this plan activated (decision 1706). The clause
has no home to be updated in, so the record is here, per decision 1709.

> **`compiler.codegen.naive-locked`, as of item E.** The clause locked two
> separate things that used to be one: *emit every check* and *round-trip
> every value through the frame*. Item E changes the second and leaves the
> first untouched. Under `release`, a scalar temp that is read more than
> once, never has its address taken, and is not live across a returning
> call lives in one of `x19..=x27` for its whole live range and has no
> frame slot; everything else still spills exactly as before. Under `dev`
> the old model is reproduced byte for byte and is the permanent
> correctness reference (M19 freeze 1407) — that is why the allocator is a
> named opt rather than a rewrite. **Every check is still emitted**, on
> both paths; nothing about abort branches, bounds checks or overflow
> checks moved. The naive form is no longer the *only* form, but it is
> still a form the compiler produces on demand and the one every
> equivalence oracle is stated against.

Two in-tree comments asserting the old model were corrected in the same
commits that changed it: `codegen.rs`'s frame-layout block (decision 4,
"spill-everything, fixed frame") now describes the residency case, and
`mwir.rs:580`'s reference was left alone because it is about immediate
materialization, not the frame. The stale claim in `codegen.rs:585` about
`layout.rs`'s hand-assembled routines using `x9..x17` is recorded above as
stale but **not** edited — it is item F's to fix along with the ABI text
it belongs to, and rewriting it here would collide with a parallel item.

## Hand-off to item F

**What F extends.** `regalloc::allocate` is already the whole decision
procedure and it already returns a per-function `Assignment` that
`build_frame` consumes. F's interprocedural pass should produce the same
`Assignment` type from a whole-program analysis and hand it to the same
`build_frame` — the codegen side does not need to know which analysis
produced it.

**What F must not have to rewrite.**

- `probe_fn_facts` and `FnFacts`. The measured-facts contract is the part
  worth keeping: it is what stops the allocator from believing something
  the emitter does not do. F should add fields to `PointFacts` (a
  per-call-site clobber set is the obvious one), not replace the pass.
- The virtual-slot mechanism (decision 1766). It is what let this land
  without touching ~220 emission sites, and it will let F's frameless
  functions (F3) land the same way: a function with no frame slots at all
  simply has every temp virtual.
- `Frame::temp_reg` / `Frame::residents`, `Assignment::residents` — the
  surfaces `report.rs` will want when F makes each function's chosen
  convention visible.

**Assumptions baked in that whole-program allocation will break — read
these before extending.**

1. **The call barrier is total.** `allocate` refuses any temp whose value
   must span a `CostRule::Call` word, because this ABI preserves nothing
   but x30. F1/F2 exist precisely to replace that with "what does *this*
   callee actually clobber", and when they do, the retain step in step 4
   must become a per-callee set intersection rather than a boolean. Nothing
   else in the module depends on the barrier being all-or-nothing.
2. **The pool is a constant.** `POOL` is `x19..=x27` for every function.
   Per-function conventions (F1) make the pool a function of the callee
   set, and `POOL` becomes an input to `allocate` rather than a `const`.
   The per-function subtraction of `used_regs` should survive as a
   safety net.
3. **`x0..x8` are excluded, and that is where F's biggest win is hiding.**
   The residual cost of this item is the `mov` pairs around call sites:
   `mov x0, x19` before a `bl` and `mov x19, x0` after it exist only
   because a temp cannot be homed in the argument register it is about to
   occupy. F4's arbitrary-arity register passing makes the argument
   register *be* the home, and both movs vanish. Read `asm-calls`'s
   `combo` in the golden — it is four such movs in an eight-word function.
4. **Decision 1765 is calibrated against a barrier F removes.** The
   two-read rule exists because a single-read temp is a pure copy. Once F
   keeps values resident across calls, many temps that read once *per
   call region* read several times over their real range, and the rule
   should be re-measured rather than inherited. It is a policy, not an
   invariant; the ∀ gate is what decides it.
5. **The async path is excluded (decision 1762) and that does not change.**
   F must not "extend" allocation into async bodies. A register does not
   survive a turn suspension. If F wants residency there it needs the
   turn record to grow a save area, which is a different item.

## What I could not do, and why

- **No boot lanes.** Decision 1708 reserves them for the orchestrator;
  four worktrees share one hypervisor. The focused boots this item would
  otherwise run are **`boot-actors`** and one driver case (**`boot-irq-isr`**
  is the right one — it exercises an ISR-bound plain `fn`, the sync path
  where residency is live, through the checkpoint poll). A failure in
  either would mean a resident value did not survive something the probe
  believes is not a barrier: the first suspects are a `CostRule::Call`
  word the emitter forgot to tag (which would make the barrier miss a real
  clobber) and the x28 preservation chain, which rests on two sema checks
  (`sema/bodies.rs:7536` and `:1780`) forcing ISR and `@task` methods to be
  plain `fn`. Neither is exercised by the non-boot lanes.
- **No `cargo xtask check`, `bench`, `profile`, `repro`** — decision 1708.
  So there is **no measured wall-clock number for the extra probe pass**.
  It is one additional emission per function under `release` only; the
  cost is real and unmeasured, and `bench compiler` at the close is where
  it should be looked at. If it matters, the fix is cheap and obvious:
  reuse the probe pass's word counts as the sizing pass when the
  assignment comes back empty.
- **No coalescing.** The residual `mov` pairs described in hand-off point 3
  are the largest remaining win in this item's territory and they were left
  alone deliberately: removing them means either rewriting emission sites
  to be allocation-aware or a spill/reload-forwarding peephole, and
  decision 1700 puts the latter out of scope for this plan.
- **`x18` is left out of the pool.** Nothing in a wrela image uses the
  platform register and taking it would be a tenth register for free, but
  "the AArch64 platform register is reserved" is a convention worth one
  line of restraint until something needs the register.
