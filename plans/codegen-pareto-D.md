# codegen-Pareto item D — hot/cold basic-block layout

Findings file for item D (decision 1709). Code:
`crates/wrela-compiler/src/blocklayout.rs`, plus
`cost::stage::codegen_cost_stage_with_block_layout` and
`layout::verify_branch_region`. Decisions **1750–1757**. Branch: `cp-D`.

**Every number in this file was measured on a tree containing items A, B
and C** (master at `bef42b3a`, merged into `cp-D`). The first pass of this
item measured against A alone and its numbers are superseded; §3.3 keeps
both, because the movement between them turned out to be a finding in its
own right (decision 1757).

**Headline.** The pass is built, proved and measured. On the one program in
the tree that has a block-grain profile it moves the measured hot-text
footprint **7616 → 7360 bytes (−256 B, −3.36%, 119 → 115 cache lines)** for
15 extra words and no change to the flat footprint row. The ∀ gate scores it
at **zero**, as decision 1750 said it would. The premise the item was ranked
on does not survive: the 93–98 KB figure is the **flat, all-hot** footprint,
which hot/cold layout cannot reduce by construction, and the *measured* hot
subset of `boot-actors` is **22976 B against a 64 KiB L1I** — already 84.9%
dense, already inside budget with 65% to spare. §7.

The pass is **not installed on the compile path**: `cost::bridge` resolves a
sidecar key to a block *by position*, so reordering blocks would make the
only measured case's `MeasuredBudget` line silently describe the wrong
blocks. Decision 1755, §5.

---

## 1. The layout algorithm

A **stable two-way partition** over the MWIR block partition. For each fn,
walk its blocks in original order and emit first every block classified
`Hot` **or** `Unmeasured`, then every block classified `Cold`, each run
keeping its original relative order; then rewrite every `Jump`/`JumpIfFalse`
target through the old→new index map and append one explicit `Inst::Jump` to
any block whose fallthrough successor is no longer the block physically
after it. Nothing else moves — no trace formation, no frequency sort, no
chain layout. Stability buys the two properties the item needs: a fn with no
cold block (or no measurement at all) permutes to the identity and emits
byte-identically, and every fallthrough edge whose ends land in the same run
survives, so repairs cost one word per hot/cold *boundary* rather than one
per block.

The reorder unit is deliberately `codegen::mwir_block_leaders`' partition —
the same function that assigns Lane 2 its block ids — so a class looked up
at index `k` names exactly the run being moved. Checked against a real
bridge-mode build, not argued.

## 2. Decisions

| # | Decision | Oracle |
| --- | --- | --- |
| **1750** (activation) | The ∀ gate cannot see block order; D is not a `RELEASE_OPTS` entry and scores zero. Confirmed, §4. | `unit:the_measured_hot_text_footprint_before_and_after` pins `charge 0 → 0` and `flat 11008 → 11008` |
| **1751** | Stable two-way partition (hot-or-unmeasured, then cold) with explicit fallthrough repair. Not a trace layout, not a frequency sort. | `unit:a_synthetic_hot_cold_program_packs_its_hot_blocks`, `unit:an_identity_plan_is_byte_identical` |
| **1752** | `Unmeasured` is laid out **hot** and never sunk; no sidecar → identity permutation → byte-identical body. `MeasuredBlocks::is_hot` is not reused (it answers `false` for unmeasured — right for the footprint term, wrong for layout). | `unit:unmeasured_blocks_are_not_sunk`, `unit:no_sidecar_degrades_to_a_byte_identical_layout` |
| **1753** | The pass reorders exactly the partition Lane 2 keyed its ids over; `blocklayout::block_ranges` calls `codegen::mwir_block_leaders` rather than defining a second notion of "block". | the per-fn `plan.order.len() == recorded spans` assertion in the measurement unit, over a real bridge-mode build |
| **1754** | The 2 MiB text base is **not** moved. SOG §4.8's same-region property already holds and is now *proved* on every image build by `layout::verify_branch_region`, which fails the build if text growth breaks it. §6. | `unit:verify_branch_region_refuses_a_straddling_text_span`, `unit:same_region_is_the_span_property_not_the_base_property`, `unit:the_region_constant_agrees_with_the_cost_table` |
| **1755** | The pass is **not** installed on the default compile path. `cost::bridge::BlockBridge` resolves a sidecar key by *position*, so a reordered program re-keys the correspondence and the `MeasuredBudget` line of the only measured case would be silently wrong. Fail closed: build it, measure it, do not install it. §5. | the `after` predicate in the measurement unit is built from `FnLayout::new_block_span`, not from `MeasuredBlocks::resolve` — that substitution *is* the demonstration |
| **1756** | **Sync MWIR fns only.** Async fns are emitted from FlowWir through the state-machine path, whose flattened stream is indexed by `state_flat_base` from the dispatch header; permuting it is a different job with a different correctness argument. Reported, not hidden: on the measured closure this leaves **29 of 81 hot blocks (36%) out of reach**. | `summary.fns_total` (17 sync) vs `program.fns.len()` (21), both printed |
| **1757** (added on re-measure) | **The baseline is pinned, not tracked.** `BEFORE_HOT_TEXT_BYTES` in the measurement unit is the *unmodified compiler's* number, so every word-shrinking opt breaks this test on purpose. When it breaks, the constant is re-pinned **and every number in this file is re-measured** — never rescaled arithmetically. §3.3 is why: the baseline moved twice inside one plan round and the item's win moved with it, non-proportionally. | the assertion's own failure message, which names the three observed baselines and refuses the arithmetic shortcut |

Nothing outside 1750–1757 was numbered. No `OptId` was added, no cost-table
row was touched, no term was made order-sensitive.

## 3. The numbers, as measured

### 3.1 The cost-stage closure

From `unit:the_measured_hot_text_footprint_before_and_after`, which emits the
real `boot-actors` cost-stage closure twice — once as the compiler emits it
today, once under item D's layout — and asks the **unmodified**
`cost::footprint::compute` for each core's budget. Reproduce with:

```
cargo test -p wrela-compiler --lib \
  blocklayout::tests::the_measured_hot_text_footprint_before_and_after -- --nocapture
```

```
D-MEASURE blocklayout fns_moved=7/17 hot=52 cold=49 unmeasured=18 repairs=15
D-MEASURE fns sync=17 total=21
D-MEASURE words before=2588 after=2603
D-MEASURE flat_hot_text before=11008 after=11008
D-MEASURE hot_bytes=6420 per_fn_packing_floor=6784 headroom=832 captured=256
D-MEASURE core=0 measured_hot_text before=7616 after=7360 lines 119->115 pages 3->3 charge 0->0
```

| | before | after | Δ |
| --- | --- | --- | --- |
| **measured** hot text, core 0 | **7616 B** | **7360 B** | **−256 B, −3.36%** |
| — in 64 B lines | 119 | 115 | −4 |
| text pages | 3 | 3 | 0 |
| L1I charge | 0 | 0 | 0 |
| **flat** (all-hot) hot text, core 0 | 11008 B | 11008 B | 0 |
| static words | 2588 | 2603 | +15 (+0.58%) |

The +15 words are the repair jumps, one per broken fallthrough, and every
extra word is accounted: the unit asserts
`words_after == words_before + repairs`. They cost **nothing** on the flat
footprint row — 11008 B both sides — because they land inside 64 B lines
that row already counted. That is the row 04 §5's veto is argued against, so
this item costs the budget nothing, and it is reported beside the win rather
than netted against it.

**How much of the available win was captured.** The hard floor for *any*
intra-fn block ordering is `Σ_fn ⌈hot bytes / 64⌉ × 64`, because
`footprint::compute` gives each fn its own 64 B-aligned base — which also
makes `hot_text_bytes` invariant under **fn** reordering, so intra-fn
packing is the only lever. On this closure the hot blocks are 6420 B, the
floor is 6784 B, total headroom against the 7616 baseline is **832 B**, and
the pass captured **256 — 31% of it**.

**What the pass did not reach.**

- **Async fns (decision 1756).** The closure has 21 fns, 17 of them sync;
  the pass classifies 119 of the closure's 184 blocks. Item A measured 81
  hot blocks over the whole closure and this pass sees **52**, so **29 hot
  blocks — 36% of the measured hot code — are in async fns it does not
  touch.** `__wrela_rt_run_one`, `__wrela_rt_select` and the actor methods
  are in that set, and they are among the hottest code in the image (item
  A's table 2).
- **Repairs.** 15 words of forwarding branches added back into hot text.

The hot/cold/unmeasured split over the blocks the pass classifies is
**52 / 49 / 18**, unchanged by B and C. The 18 unmeasured are the same 18
item A measured, and none was sunk (decision 1752).

### 3.2 Image scale

From `wrela dump --stage=report`, a committed golden surface:

| case | flat `Budget` hot text | over_l1i_lines | charge | measured `MeasuredBudget` hot text |
| --- | --- | --- | --- | --- |
| `boot-actors` | 92480 B (90.3 KiB) | 421 | 2947 | **22976 B (22.4 KiB)** |
| `boot-cores-3` | 89216 B (87.1 KiB) | 370 | 2590 | — (no sidecar) |
| `appliance` | 89216 B (87.1 KiB) | 370 | 2590 | — (no sidecar) |

**The after-number at image scale is not measurable**, because the image
build path (`layout::lower_and_codegen_image`) lowers internally and the
pass is not installed in it (decision 1755). What *is* computable is the
floor. Instrumenting `footprint::compute` with a throwaway probe that sums
`⌈hot bytes per fn / 64⌉ × 64` gives, for `boot-actors`:

```
measured: lines=359  hot_text=22976  hot_bytes=19500  per_fn_packing_floor=20928
flat:     lines=1445 hot_text=92480  hot_bytes=84284  per_fn_packing_floor=92480
```

So at image scale the **entire** headroom for hot/cold block layout is
22976 → 20928, i.e. **2048 bytes, 8.9%** — before repairs and before the
async 36% is subtracted. The flat row's floor equals the flat row, which is
the arithmetic statement of why block order is invisible to it.

Measured hot-text **density** is 19500 / 22976 = **84.9%**.

### 3.3 The baseline moved twice, and the win moved with it (decision 1757)

This item was first measured against item A alone, then re-measured after
items B and C merged. Same code, same oracle, three trees:

| tree | measured hot text, before | after | Δ | per-fn floor | headroom | captured |
| --- | --- | --- | --- | --- | --- | --- |
| A only | 7744 B | 7360 B | −384 B (−4.96%) | 6912 | 832 | 46% |
| A + B (orchestrator's run) | 7680 B | — | — | — | — | — |
| **A + B + C (this file)** | **7616 B** | **7360 B** | **−256 B (−3.36%)** | 6784 | 832 | **31%** |

Two things to read off it, neither of which was anticipated:

1. **The `after` number did not move at all.** 7360 B on both trees. Items B
   and C deleted 78 words from this closure (2666 → 2588) and *none* of that
   deletion showed up in the packed layout's line count — the packed layout
   had already absorbed those words into lines it occupies. All 128 B of the
   saving landed on the **unpacked** baseline instead.
2. **So D's win shrank by a third (384 → 256 B) because other items got
   better**, while the headroom against the packing floor held at 832 B.

On this term, item D and the word-shrinking items are **substitutes, not
complements** — the opposite of the framing that ranked D into the Pareto
three ("it makes everything else affordable"). Two data points is not a law,
and the mechanism is specific to a line-quantized footprint model, but the
direction is measured rather than argued, and §8 acts on it.

This is also why the measurement unit pins the baseline as a constant with a
loud failure message rather than tracking it: the constant breaking *is* the
signal, and it caught this interaction on the orchestrator's merge.

## 4. What the ∀ gate scored: zero, and why

Confirmed on the merged tree, exactly as decision 1750 predicted — and worth
stating precisely, because there are two independent reasons and only one of
them was known at activation.

1. **The gate's footprint term is order-invariant.** The `Budget` line the
   gate reads is `HotBlocks::All` — every block is hot — so the line set is
   the whole fn's line set and its size does not depend on the order of the
   blocks inside it. `flat_hot_text before=11008 after=11008` is that fact
   measured. This is decision 1750.
2. **Even the order-*sensitive* term charges zero here.** The measured
   footprint term (`HotBlocks::Measured`) genuinely does move — that is the
   −256 B above. But `charge` is driven by `over_l1i_lines`, and 7616 B (or
   22976 B at image scale) against a 64 KiB L1I overflows nothing. So the
   measured term charges **0 before and 0 after**, and D would score zero
   even if the gate read the measured row.

Both reasons **survive** B and C. B and C shrank the image, which moves the
measured hot subset *further* inside L1I (23744 → 22976 B, i.e. from 36% to
35% of the cache), so reason 2 is if anything stronger than it was.

Neither number was tuned to produce this and no term was made
order-sensitive. **The named prerequisite for D ever scoring stands, and it
is bigger than decision 1750 said:** making the footprint term
order-sensitive is necessary but *not sufficient*, because the hot subset
already fits L1I on every program in the corpus. A gate win would need the
footprint term to charge for *density* rather than only for overflow — a
strictly larger ruler change than the one decision 1750 named, and out of
scope here in exactly the same way.

## 5. Why the pass is not on the compile path (decision 1755)

`cost::bridge::BlockBridge::build_with_counts` requires, per fn, that the
recorded `BlockSpan`s satisfy **`block_index == emission position`**: it
errors on `block ordinals out of order` and on any `word_start` that does
not continue the previous span (they must *tile* the fn's word range in
order). So a sidecar key `fn#k` resolves to **the k-th emitted block**, by
position.

Item D permutes emission order. After the pass, the k-th emitted block is no
longer original block `k`, so `MeasuredBlocks::resolve` would attribute the
sidecar's counts to the wrong blocks and `--stage=report`'s `MeasuredBudget`
line for `boot-actors` — the only case that has one — would be silently
wrong. A second, smaller instance of the same coupling: a repair jump
appended after a conditional branch is a block leader of its own, so the
post-pass partition also has *more* blocks than the sidecar describes.

There is no way to wire the pass and keep that line honest without either

- **re-measuring** — a fresh `--block-count` boot producing a sidecar keyed
  to the new layout, which is circular (the new layout is derived from the
  old sidecar and is not a fixpoint under the new one), or
- **changing the bridge** to carry a block's identity instead of inferring
  it from position. Concretely: record spans labelled with the *original*
  block index (`FnLayout::new_block_span` is exactly that datum, and it is
  already built and tested here) and relax `build_with_counts`' two ordering
  checks to "the `block_index` values are a permutation of `0..n` and the
  spans tile the word range in push order".

That second change is small and is the right one. It is **not** made here:
it is in `cost/`, it changes how the model's measured input is resolved, and
this item was told not to touch the ruler. It is named as item D's
prerequisite for installation, alongside §4's prerequisite for scoring.

Landing the pass unwired follows item A's precedent one rung earlier
("Nothing in this item is on the compile path"). Unlike item A, this module
*is* exercised on real programs — `cost::stage::codegen_cost_stage_with_block_layout`
is a real pipeline entry point, and the measurement unit drives the whole
lower → relayout → codegen → score path on `boot-actors` in 0.34 s.

## 6. What the 2 MiB text base changed: nothing, deliberately (decision 1754)

Decision 1705 asked to fold in a 2 MiB-aligned text base "so every branch
and its target share one region (SOG §4.8)". Measured first, on the merged
tree — item B's `ADR` substitution moved every one of these addresses, which
is exactly why the property is checked rather than assumed:

```
boot-actors  entry 0x40500000+80  code 0x40500050+85136  abort 0x40514ff4+120  checkpoint 0x4051506c+28
appliance    entry 0x40500000+80  code 0x40500050+82076  abort 0x4051433c+120  checkpoint 0x405143b4+28
```

Branchable text spans `0x40500000..0x40515088` at worst — **~85 KB inside
the aligned 2 MiB region `[0x40400000, 0x40600000)`**. The property SOG §4.8
actually states already holds, by a factor of ~24. The *base* is not
2 MiB-aligned (ladder 6j is right about that), but base alignment is neither
necessary nor sufficient for the property: a 2 MiB-aligned base followed by
more than 2 MiB of text still straddles.

What aligning the base would cost: `IMAGE_BASE` is `0x4050_0000` and the
next 2 MiB boundary is `0x4060_0000`, so either **~1 MiB of zero padding in
every shipped image** — on an appliance where every update is a full image
recompile shipped as an A/B triple — or a move of `IMAGE_BASE` itself, which
is a `wrela-machine` contract constant the VMM shares. And the model prices
none of it: `cost::branch` lists SOG §4.8 row 24 ("branch and target in
different 2 MiB regions") as **undecidable for a word-index model, charges
0**.

So the base does not move. Instead `layout::verify_branch_region` proves the
property from the section table on every image build and **fails the build**
if text growth ever breaks it. It is a build error rather than an internal
error, because unlike its neighbours in `verify_section_sizes` this one is
reachable from a source program: an image whose text outgrows 2 MiB breaks
it with no editing mistake at all. Its message names the fix.

This is a deviation from decision 1705 as literally worded and it is the
whole of the deviation: the property it asks for is delivered, as a proof
rather than as a purchase.

## 7. The premise, re-checked — a negative result that survives B and C

Item D was ranked third in the plan's Pareto three on this argument:

> Text is **93–98 KB against a 64 KiB L1I**. Until the hot subset is dense,
> every code-growing opt on the ladder is spending an overdrawn budget.

Measured on the merged tree, both halves are wrong in the same way, and both
verdicts **survive** B and C — the numbers moved, the conclusions did not.

- **93–98 KB is the flat, all-hot number**, and it is now 89–92 KB
  (`boot-actors` 92480, `boot-cores-3` 89216, `appliance` 89216 — B and C
  shaved 2.3–2.4%). It is all-hot *by construction* and order-invariant, so
  hot/cold block layout cannot reduce it by a single byte. Reducing it is
  item E's and item F's job (they delete words, and B and C just
  demonstrated the mechanism); it was never D's.
- **The hot subset is already dense, and already fits.** `boot-actors`'
  measured hot text is 22976 B — 19500 B of actual hot code in 22976 B of
  cache lines, **84.9% density** — against a 64 KiB L1I. `over_l1i_lines=0`,
  `charge=0`, 65% headroom. The best any intra-fn ordering can do is
  20928 B: an 8.9% improvement on a quantity that is not binding.
  (Density was 84.8% before B and C, so the "~85% dense" verdict is not an
  artefact of one tree.)

So "until the hot subset is dense, every code-growing opt is spending an
overdrawn budget" does not describe this image. The budget is not overdrawn
on the hot path; it is overdrawn on the **flat** path, which is the row the
veto reads and the row this item cannot move.

One more scope fact that bounds the item independently of any of the above:
**exactly one program in the tree has a block-grain sidecar**
(`tests/golden/boot-actors/lane2-freq.txt`; no other `tests/golden/*/`
carries one, and the flagship `appliance` does not). Every other program —
including every `asm-*` case, every other boot case, and the appliance image
— takes the `LayoutClasses::Unmeasured` path and is laid out exactly as
today. Item D's blast radius, wired or not, is one image.

## 8. What this should change downstream

Recorded for item G, not acted on here.

1. **Ladder 8a's "cheapest large win on the ladder" claim does not survive
   this measurement** and should be re-graded. Its stated justification is
   the 93–98 KB-vs-64 KiB figure, which is the flat row.
2. **D is a substitute for E and F on this term, not a complement**
   (§3.3). Its win fell 33% when B and C landed, and every byte B and C
   saved on the measured row came out of D's margin. The plan's "it makes
   everything else affordable" framing predicts the opposite. Item G should
   re-measure D's delta *after* E and F land before crediting it with
   anything; the expected direction is smaller again.
3. **The real L1I question is the flat footprint**, and the items that move
   it are E (spill code deleted) and F (prologue/epilogue and calling
   convention deleted). B and C already moved it 2.3% between two merges.
   Those items carry the argument; D does not add to it.
4. **Item D's own next rung, if it is ever pulled back in, is async block
   layout** (decision 1756) — 36% of the measured hot blocks are behind it,
   and it is where `__wrela_rt_run_one`/`__wrela_rt_select` live. That is a
   larger and more interesting piece of work than the sync pass and should
   be argued on its own numbers.
5. **F6 stays cut.** Nothing here makes per-call-site frequency real
   (decision 1770's named prerequisite), and this item's experience with the
   position-keyed bridge is a second reason to expect trouble there.
6. **Two prerequisites are now named for D**, in order: the bridge must
   carry block identity (§5) before the pass can be installed at all, and
   the footprint term must charge for density rather than only for overflow
   (§4) before it can score. Both are ruler-side.

## 9. Golden diff shape

**Empty.** Item D moves no expectation file, on the merged tree as on the
old one.

```
cargo xtask golden --no-boot --filter asm-       → 12 ok (12 cases)
cargo xtask golden --no-boot --filter cost-      → 16 ok (16 cases)
cargo xtask golden --no-boot --filter image-     → 39 ok (28 cases)
cargo xtask golden --no-boot --filter appliance  → 4 ok (1 case)
```

Read-only, no `--update`, nothing committed under `tests/golden/*/expected/`
(decision 1708). This is not a filter artefact: the pass is not on the
compile path, and `verify_branch_region` is a pure verification that passes
on every image today, so **no address in any golden moves**. The families
above are the four item D could conceivably touch — emission, cost model,
image report, and the flagship.

For the orchestrator's central re-pin: item D contributes **nothing** to it.

## 10. Verification

All on `cp-D` after `git merge master` (`bef42b3a`, items A+B+C plus the
central re-pin); the merge was conflict-free.

```
cargo test -p wrela-compiler --lib blocklayout   → 13 passed; 0 failed; 0.34 s
cargo test -p wrela-compiler --lib               → 813 passed; 0 failed; 4 ignored; 13.2 s
cargo fmt -p wrela-compiler --check              → clean
cargo clippy -p wrela-compiler --lib --all-targets → no warning in the new code
cargo doc -p wrela-compiler --no-deps            → no warning in the new code
cargo xtask golden --no-boot --filter {asm-,cost-,image-,appliance}  → all ok (read-only)
cargo xtask diff-eval                            → 129 test(s) agree across 47 case(s),
                                                   8 lowering-skips, 6 exhaustive-skips,
                                                   1 quota-skip, 0 import-skips
```

**`diff-eval` is green and it does not exercise this pass.** It compares the
comptime evaluator against the **default** compile path, and item D is not
on it (decision 1755), so its 129 agreements say only that nothing
regressed. The pass's own semantic oracle is
`blocklayout::verify_successors`, which runs inside `apply_fn` for every fn
it moves — on real programs, not only on the units' synthetic bodies — and
proves that the reordered body has the same successor relation as the
original, resolving through the pure-forwarding repair jumps. It **fails
closed**: a permutation it cannot prove equivalent is never emitted. That
check ran over all 7 moved fns of the `boot-actors` closure in the
measurement unit.

### The units, and what each is an oracle for (freeze 1714)

| unit | what it would catch |
| --- | --- |
| `a_synthetic_hot_cold_program_packs_its_hot_blocks` | hot blocks not contiguous / cold not sunk; wrong repair target |
| `no_sidecar_degrades_to_a_byte_identical_layout` | the degrade path guessing instead of doing nothing |
| `unmeasured_blocks_are_not_sunk` | `Unmeasured` conflated with `Cold` (asserts the wrong order is *not* produced) |
| `a_stale_sidecar_fails_the_build_rather_than_laying_out` | a stale or empty partition laying out an image |
| `the_permuted_body_has_the_same_successor_relation` | a wrong permutation, over all 16 hot/cold assignments of a 4-block body |
| `a_repair_after_a_conditional_costs_one_block` | the block-count claim drifting from the truth |
| `new_block_span_locates_every_original_block` | the block-identity map that §5 turns on |
| `an_identity_plan_is_byte_identical`, `block_ranges_are_the_leader_partition`, `a_class_vector_of_the_wrong_length_fails_closed` | the basics |
| `the_measured_hot_text_footprint_before_and_after` | **the item's claim itself**, end to end on a real build, through the unmodified cost model — and, via `BEFORE_HOT_TEXT_BYTES`, any cross-item baseline move (decision 1757; it has fired twice) |
| `verify_branch_region_refuses_a_straddling_text_span`, `same_region_is_the_span_property_not_the_base_property`, `the_region_constant_agrees_with_the_cost_table` | decision 1754's property being assumed rather than proved |

### Focused boots: not run, per the round's instruction

`boot-actors` and `boot-cores-3` were **not** run (worktrees building
concurrently; boot lanes contend on the shared hypervisor). Expected result:
**byte-identical transcripts on both.** `boot-cores-3` has no sidecar, and
`boot-actors`' compile path does not call this pass — the only new code on
either build path is `verify_branch_region`, which emits nothing and passes.
A diff in either transcript would mean the region check or the codegen
visibility change reached emission, which would be a bug in this item.

## 11. What I could not do

- **The image-scale after-number.** §3.2 gives the before (22976, a
  committed golden) and the hard floor (20928, from a named throwaway
  probe), but not a measured after, because the image build path lowers
  internally and the pass is not installed in it. Blocked on §5.
- **Async block layout** (decision 1756) — 36% of the measured hot blocks.
- **Making D score.** Deliberately, and §4 names both prerequisites.
- **`cargo xtask check`, `bench`, `profile`, `repro`, any boot lane, any
  unfiltered or `--update` golden run** — the round forbids them; the
  orchestrator runs them centrally. (The first pass of this item ran one
  unfiltered read-only `golden --no-boot`; that was more than it was asked
  to run, and it was not repeated on the re-measure.)

### A discrepancy with the orchestrator's number, resolved

The merge report quoted a new baseline of **7680**; this file measures
**7616**. Both are right: 7680 is the A+B tree, 7616 is A+B+C. The baseline
moved once per merge — 7744 (A) → 7680 (A+B) → 7616 (A+B+C) — one 64 B cache
line each time. §3.3 records all three, and decision 1757 is the rule that
keeps the next mover from papering over the fourth.
