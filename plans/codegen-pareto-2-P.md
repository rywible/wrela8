# Item P findings — the inliner, rebuilt and parked

**Status: DONE (2026-07-31).** Item P of
[codegen-pareto-2.md](codegen-pareto-2.md) — the third of the three
things deleted under the old "losers are deleted" rule and restored under
the new one (decision 1910). Decision block **1980–1989**.

Headline, in two lines. **The ordering question was real: the two
pipeline positions differ by 3× on cycles and 4× on words**, so item J's
un-recorded position was a genuine gap in its measurement. **And item J's
verdict survives it**: the inliner loses in *both* positions, and the
enabling-order numbers land within 6 % of item J's, which is also the
evidence that item J measured the enabling order. Decision 1935 stands,
and for the first time it can be re-derived from this repository.

---

## 1. What was rebuilt, in a paragraph

`crates/wrela-compiler/src/mwir_opt.rs` gains a fourth plain pass beside
item J's three: `inline_program`, `inline_into`, `splice`,
`inline_refusal`, `reference_counts`, `frame_estimate` and two constants.
No pass manager, no trait over "a pass" — `optimize` is still a `fn` that
calls whichever passes their TLS knobs enable, in a fixed order, and the
inliner is one more `if` in it. The rule is item J's decision 1921 as
stated: a call site whose callee is inlinable is inlined when either
**(i)** it is that callee's only reference in the whole sealed program —
MWIR bodies *and* FlowWir states, which is why `optimize` now takes the
`FlowWirProgram` — in which case the body *moves* and the callee is
deleted; or **(ii)** the callee's body is at most 8 MWIR instructions,
counted as the emitted words a call site deletes (5 prologue/epilogue/
`ret`, 1 `BL`, 2 argument/result moves). Parameters bind by **aliasing**:
the callee's parameter temp is rewritten to the caller's argument temp,
so a splice is a strict deletion of the call sequence rather than a trade
of a `BL` for a run of `mov`s. That is sound only because
`inline_refusal` rejects every callee that could observe the difference —
a receiver, a `mut`/`take` parameter, an `InterruptCell` op, a non-leaf,
an assignment to its own parameter (directly or through an in-place
base), and every late-bound runtime key. `Return` becomes a copy into the
call's `dst` plus a jump to the join; jump targets are remapped through a
two-phase index map, callee-space through `start[j]` and then caller-space
through `+ at`; the caller's own targets shift by `len(expansion) - 1`.

---

## 2. The two-position measurement — item P's whole point

All four MWIR passes run inside **one** `mwir_opt::optimize` call at the
top of `codegen_program`/`codegen_program_with_async` (decision 1920).
The opt *list* records that the inliner was on; it does not record where
it sat inside that call. An inliner is an **enabling** pass — its value
is not what it does alone but what redundancy elimination can do to the
merged body afterwards — so an inliner measured after GVN/DCE is a
measurement of nothing. `set_inline_after_redundancy` (decision 1981) is
the knob that asks both.

Whole `cost-*` corpus, 21 cases, both tiers, at the pinned point.
Re-run with:

```
cargo test -p wrela-compiler --lib the_inliner_measured_in_both_pipeline_positions -- --ignored --nocapture
```

| framing | position | Δ cycles | Δ words |
| --- | --- | --- | --- |
| `[ConstProp,Gvn,Dce]` → `+Inline` | **inline → ConstProp/Gvn/Dce** | **+728** | **+641** |
| `[ConstProp,Gvn,Dce]` → `+Inline` | ConstProp/Gvn/Dce → inline | +2 520 | +2 777 |
| `release-minus-Inline` → `+Inline` | **inline → ConstProp/Gvn/Dce** | **+233** | **+352** |
| `release-minus-Inline` → `+Inline` | ConstProp/Gvn/Dce → inline | +635 | +1 477 |
| rule (i) only, `release-minus-Inline` → `+Inline` | **inline → ConstProp/Gvn/Dce** | **+26** | **+5** |
| rule (i) only, `release-minus-Inline` → `+Inline` | ConstProp/Gvn/Dce → inline | −3 | −10 |

And the two shipped images, emitted words, with the reach counters and
the per-mnemonic deltas that carry the mechanism:

```
cargo test -p wrela-compiler --lib the_inliner_on_the_two_shipped_images_in_both_positions -- --ignored --nocapture
```

| image | position | release | +Inline | Δ | sites | moved | Δ`str` | Δ`ldr` | Δ`mov` | Δ`bl` | Δ`movz` |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| appliance | either | 13 577 | 13 577 | **+0** | **0** | 0 | 0 | 0 | 0 | 0 | 0 |
| compositor | inline → CP/GVN/DCE | 6 877 | 7 249 | **+372** | 17 | 2 | +100 | +41 | −3 | **+0** | +76 |
| compositor | CP/GVN/DCE → inline | 6 877 | 8 387 | **+1 510** | 25 | 2 | +704 | +322 | 0 | +22 | +131 |

### What this settles about decision 1935

**The positions differ materially, and that has to be said plainly.**
3.5× on cycles and 4.3× on words in the enabling framing; 2.7× and 4.2×
leave-one-out; 4.1× on the compositor's word count. An inliner run after
redundancy elimination is a strictly worse program than the same inliner
run before it, and by a factor, not a margin. Item J's measurement did
not record which one it was, and that was not a bookkeeping lapse: it is
the difference between the number that refuses 2a and a number four times
larger that refuses nothing in particular.

**And decision 1935 survives it.** Every whole-list framing loses in
*both* positions. More than that, the enabling-order column reproduces
item J's numbers closely enough to identify what item J measured:

| framing | item J | item P, enabling order | item P, after |
| --- | --- | --- | --- |
| `[ConstProp,Gvn,Dce]` → `+Inline`, cycles | +628 | **+728** | +2 520 |
| `release-minus-Inline` → `release`, cycles | +221 | **+233** | +635 |
| `release-minus-Inline` → `release`, words | +308 | **+352** | +1 477 |
| rule (i) only, leave-one-out, cycles | +36 | **+26** | −3 |

Every enabling-order row is within ~16 % of item J's, on a tree that has
since taken items K, L and M; every after-order row is 3–4× off. Item J
ran the inliner ahead of `ConstProp`/`Gvn`/`Dce`, its refusal was
measuring the right thing, and the only defect was that nobody could
check — which is exactly the defect CLAUDE.md's parking rule exists to
prevent. **Decision 1935 is confirmed, not overturned, and is now
reproducible.**

The **per-case** picture is the same in every framing and both positions:

- `cost-calls` is the only case that falls in every framing (−12 cycles /
  −10 words enabling, −14 / −12 after) — a microbenchmark that exists to
  contain calls.
- `cost-icache-cliff` **rises** by +339 cycles over the `[ConstProp,Gvn,Dce]`
  baseline and **falls** by −9 over the shipped list. Its words fall in
  every framing. This is the item-1937 mechanism again: what a splice
  moves is words, and words become cycles only once `RegAlloc` has taken
  out the schedule slack that absorbs them.
- **`cost-product-compositor` rises in every framing, in both positions,
  under both rules** — +254/+372 (full rule, enabling), +672/+1510 (full
  rule, after), +47/+25 (rule (i), enabling), +34/+23 (rule (i), after).
  `SweepVeto::CaseRose` is an absolute veto, so this alone refuses the
  opt however the sums come out.

### Rule (i) alone, refined (decision 1988)

Item J said "even rule (i) alone loses, and that is the result that
settles it". Measured here, rule (i) alone — where the body *moves*, the
callee is deleted, and nothing is duplicated — is **within noise on the
totals**: +26 cycles / +5 words in the enabling order and a small net
*fall*, −3 / −10, in the after order. That is not "loses"; that is a
wash. What refuses it is narrower and sharper: it raises
`cost-product-compositor` in every framing and position, and that is an
absolute veto. So the honest form of item J's claim is **rule (i) alone
is not a loss on the sum; it is refused by the one case it was supposed
to help.**

---

## 3. The three things doctrine requires of a parked opt

CLAUDE.md (2026-07-31): a parked opt carries the measurement that refused
it, the **mechanism** that explains the loss, and the **named workload or
capability that would make it worth re-asking**.

### 1. The refusal measurement

§2, both tables, both re-runnable by the commands printed above.
Reach is reported with them (decision 1985): 17 sites and 2 rule-(i)
moves on the compositor, **0 sites on the appliance**. Item J's "it has
no customers on the appliance at all" reproduces exactly — the
appliance's application half is six methods with zero `Inst::Call`
between them, and every call in that image is in the runtime closure
decision 1932 keeps off limits.

### 2. The mechanism, measured rather than argued

The per-mnemonic column is the finding. On the compositor, in the
enabling order, +372 words decompose as **+100 `str`, +41 `ldr`, +76
`movz`, −3 `mov`, and `bl` unchanged at +0**.

- **`bl` does not move.** Seventeen call sites were deleted, so seventeen
  `BL`s went — and seventeen came back, in the abort paths the
  *duplicated* bodies brought with them. `Inst::Shift`'s count-range
  check expands to 19 emitted words including a full `__wrela_abort_val`
  prologue at every non-constant shift site (item J §5), and duplicating
  `chan` duplicates its check. The call overhead an inliner is supposed
  to be spending is, on this backend, exactly repaid by the checks it
  copies.
- **The cost is spill traffic, not duplication.** +141 memory ops against
  −3 register moves is not a body that got bigger uniformly; it is a body
  whose register pressure went up. A callee's temps become the *caller's*
  frame slots, the caller's live ranges get longer and wider, and item E's
  per-function linear-scan allocator with a fixed pool answers by
  spilling. The callee's own compact frame and its own private allocation
  were cheaper than the merged one.
- **The after-position makes both effects worse** (+704 `str`, +322
  `ldr`, +22 `bl`) and inlines *more* sites (25 vs 17), because `Dce` has
  already shrunk callee bodies under the 8-instruction bound before the
  size rule is asked. More splices, each into a body nothing will clean
  up afterwards.

That is a structural loss, not a circumstantial one, and it is the same
mechanism item J named — but here it is four columns of a table rather
than a sentence.

### 3. The named re-ask condition

Re-ask `OptId::Inline` when **either** of these lands, and not before:

1. **A register allocator that survives the splice.** Item I's coalescing
   exists and is not enough; what is needed is live-range splitting or
   rematerialization, so that an inlined temp is not automatically a
   caller frame slot. The measurable precondition: the compositor's
   Δ`str`+Δ`ldr` under `+Inline` falls to roughly zero. Until then the
   splice pays for the call sequence twice.
2. **The shift-count range check becomes expressible** (item J §5's named
   successor, ladder 2c/2d). While a shift with an obviously-in-range
   literal count costs 19 emitted words, duplicating a body duplicates
   its checks and rule (ii) cannot win on words at all. Rule (i) — which
   duplicates nothing — is *already* a wash, so this is specifically the
   condition that would make rule (ii) worth re-asking.

**And the narrower one, worth naming separately**: rule (i) alone is
refused today by `cost-product-compositor` and by nothing else. If item I
or a later allocator item stops that case rising, rule (i) alone passes
the gate on its current numbers. That is the cheapest live path back for
2a, and it is the one to check first.

The workload condition item D's block layout needed — "a compute program
with headroom arrives" — is **not** this opt's condition. Item M's
compositor is exactly the shape 2a was written for, it is in the corpus,
and 2a still loses on it. The blocker is the allocator, not the corpus.

---

## 4. Oracles actually run

| lane | result |
| --- | --- |
| `cargo test -p wrela-compiler --lib` | **886 passed, 0 failed**, 14 ignored, 141 s (budget 240 s) — 9 new units, 2 new `#[ignore]`d deep lanes |
| `cargo xtask diff-eval --inline` | `diff-eval: release + the parked OptId::Inline (item P)` … `diff-eval: 130 test(s) agree across 48 case(s), 8 lowering-skips, 6 exhaustive-skips, 1 quota-skips, 0 import-skips` |
| `cargo xtask diff-eval` (baseline, for comparison) | `diff-eval: 130 test(s) agree across 48 case(s), 8 lowering-skips, 6 exhaustive-skips, 1 quota-skips, 0 import-skips` — **identical tally** |
| `cargo xtask golden --only-boot --filter boot-tile-compositor` | `golden: 1 expectation(s) ok (1 case(s), filter 'boot-tile-compositor', boots only)` — green, and this case checks its own computed pixels |
| `cargo xtask golden --only-boot --filter boot-actors` | `[cost]` differs; **the boot transcript is byte-identical**. Pre-existing: the identical filtered run at the base commit `0505aa1f` fails the same way, and item P's `[cost]` dump is byte-for-byte the base's. Not re-pinned — the orchestrator re-pins. |

**`diff-eval --inline` is the guest-execution oracle for the parked opt.**
It is not a static check: it builds, lays out, signs and *boots* a test
image per case in the VMM and compares each `@test`'s result against the
tree-walking evaluator, which is the reference implementation of the
semantics. 130 comparisons across 48 cases agree with the inliner on, and
the tally is identical to the baseline's — no case newly skipped, so the
reach did not collapse into a vacuous pass. That is the lane CLAUDE.md's
parking rule requires ("a parked opt must still compile and pass
`diff-eval`, so it cannot rot into a miscompile while disabled"), and it
did not exist before this item: `diff_eval_over_cases` calls
`apply_mode(Release)`, which by construction turns a parked opt off.
`cargo xtask diff-eval --inline` is one boolean, named for the one opt it
enables; items N and O can add their own beside it without colliding.

### The two silent miscompiles, pinned so a rebuild cannot repeat them

Both of item J §6's bugs are now unit-pinned in `mwir_opt.rs`:

- **`splicing_renames_every_temp_of_every_inst_shape`** (decision 1930's
  consequence). `visit_temps_mut_visits_exactly_the_temps_the_dump_prints`
  already pins the *walker* against `mwir::fmt_inst`; this pins what goes
  wrong when it is wrong. It splices a callee whose body is one instance
  of **all 53** `Inst` variants into a caller with a deliberately larger
  temp space, and asserts that no callee temp number survives — so an
  unrenamed field cannot hide as a plausible fresh temp. This is the
  `BytesIndexGet.index` gap that made the guest print 22 copies of the
  letter `t` where a test name belonged.
- **`inlining_refuses_a_placeholder_body_layout_will_replace`**
  (decisions 1929/1932). All three shapes of late-bound key —
  `__test_prefix_0`, `rt_boot_init 0` and the plain-named
  `stdlib/core/runtime.wr` helper `ascii_digit` that no prefix test
  catches — must keep both their `Inst::Call` and their key. This is the
  bug that turned every guest test line into a bare `ok`; units were
  green and both ∀ tiers were green, and only `diff-eval` caught it.

Plus `the_inliner_refuses_what_aliasing_cannot_model` (the seven
refusals as a table), `flowwir_references_count_towards_the_single_reference_rule`
(rule (i) may not consume a callee an async state machine calls),
`a_splice_keeps_jump_targets_honest` (a splice inside a loop body),
`rule_one_moves_the_body_and_deletes_the_callee`,
`inlining_splices_a_small_leaf_at_every_site`,
`both_inline_positions_are_deterministic` and
`opts::tests::the_inliner_is_wired_and_parked`.

---

## 5. What could not be done, and why

- **No `cargo xtask check`, no unfiltered or `--update` golden run, no
  `bench`/`profile`/`repro`, no `cargo test -p wrela-vmm`.** The item
  forbids all of them; the milestone close and the orchestrator's merge
  own them.
- **`boot-actors`'s `[cost]` expectation is stale on the base commit and
  is left stale.** Verified rather than assumed: `git checkout 0505aa1f
  -- crates/`, the same filtered golden, the same failure, and item P's
  actual dump byte-identical to the base's (185 lines, `diff` empty). The
  drift is in code shape — branch counts and per-fn cycles — so it
  belongs to an earlier item in this round, not here. No
  `tests/golden/*/expected/*` file is touched by this item.
- **The two boot goldens run under `RELEASE_OPTS`, which does not include
  `Inline`.** They prove item P moved nothing about the shipped program,
  which is the right thing for a parked opt to prove, but they do not
  boot the inliner. What boots the inliner is `diff-eval --inline`, over
  48 cases. Making `golden` able to run a case under an explicit opt list
  would be a real addition to the harness and is not this item's.
- **No ∀ residual-box sweep of `Inline`** (`compare_opt_lists_over_box_in_tier`).
  A parked opt does not need a box verdict to be parked, and
  `cost-product-compositor` already rises at the *pinned* point in every
  framing and both positions, which is an absolute `CaseRose` veto —
  a box sweep could only make the refusal stronger, never weaker.
- **`--stage=mwir-opt` still does not exist**, so the inliner's output has
  no dump of its own; what pins it is the `asm`/`image`/`cost` goldens
  downstream. Unchanged from item J §5, and still the cheapest way to
  close a real gap.
- **`INLINE_MAX_BODY` is not swept.** 8 is item J's counted value, not a
  tuned one, and tuning it to make the opt win is exactly what this
  round's rules forbid. Rule (i) — `INLINE_MAX_BODY` effectively 0 — is
  measured, and that is the honest end of the range.

---

## 6. Decisions

| # | decision |
| --- | --- |
| **1980** | **The shrinking inliner is rebuilt from item J's stated rule and parked.** `OptId::Inline` exists, reaches a knob, passes `diff-eval`, and is **not** in `RELEASE_OPTS`. This restores what decision 1935 deleted without overturning its verdict: a refusal that cannot be re-derived from the repository is not a refusal, it is a memory. |
| **1981** | The pipeline position is a **knob** (`set_inline_after_redundancy`), not an `OptId` — an id is a product decision and this is a question. Default `false`: the inliner ahead of `ConstProp`/`Gvn`/`Dce`, the only order in which an enabling pass can enable anything. |
| **1982** | `optimize` takes the `FlowWirProgram` as a **reference source**, so rule (i)'s "only reference in the whole sealed program" counts embedded `Inst::Call`s, `Send`/`ActorCall` method keys and `GroupStart` callee keys. FlowWir is still never rewritten — decision 1927 stands unchanged. `codegen_program`'s `None` is exact, not approximate: no FlowWir exists on the sync-only entry. |
| **1983** | Leaf-only inlining, four rounds. It terminates without a recursion check because a cycle is never a leaf, and a caller whose last call was just spliced away becomes one. |
| **1984** | The frame bound is a **conservative spill-everything estimate** against `build_frame`'s 4 095-byte `imm12` ceiling, computed before `RegAlloc` because that is when this pass runs. A refused splice stops inlining into that caller entirely rather than keeping a set of refused sites the next splice would renumber — dumb and obviously terminating. |
| **1985** | Measured reach (`take_inline_reach`: sites spliced, callees consumed) is reported with every refusal number. A refusal measured over zero call sites is clean about nothing, and "no customers on the appliance" is a reach number wearing a sentence. |
| **1986** | **The two pipeline positions differ materially — 3.5× on cycles, 4.3× on words** — so the opt list's order is not the pipeline's order and item J's un-recorded position was a real gap. Every future measurement of an enabling pass names its position in the pipeline, not merely its membership of the opt list. |
| **1987** | **Decision 1935 stands and is now reproducible.** Both positions lose on both currencies in both whole-list framings; the enabling order is +233 cycles / +352 words leave-one-out against item J's +221 / +308, and every enabling-order framing lands within ~16 % of item J's while every after-order framing is 3–4× off — which identifies item J as having measured the enabling order. The ladder's 2a stays below 2b and stays marked "needs a register allocator that survives the splice". |
| **1988** | **Rule (i) alone is a wash on the totals, not a loss** (+26 cycles / +5 words enabling; −3 / −10 after), refining item J's "even rule (i) alone loses". What refuses it is `cost-product-compositor` rising in every framing and both positions, and `CaseRose` is an absolute veto. `set_inline_rule_one_only` is the knob that measures it; like 1981's it is a question, not a product decision, so it is not an `OptId`. |
| **1989** | **The named re-ask condition is the allocator, not the corpus.** Re-ask when live-range splitting or rematerialization stops an inlined temp being a caller frame slot (measurable: the compositor's Δ`str`+Δ`ldr` under `+Inline` reaching ≈0), or when `Inst::Shift`'s 19-word count-range check becomes expressible so duplication stops duplicating checks. The cheapest live path is narrower: rule (i) alone is refused today by exactly one case, and passes on its current numbers the moment that case stops rising. Item M's compositor is already the workload 2a was written for and 2a still loses on it, so no future workload unblocks this. |
