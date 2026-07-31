# Item H — product-scale cost corpus (findings)

Working record for item H of [codegen-pareto.md](codegen-pareto.md), per
decision 1709. Item G folds what it needs into the evidence block; this
file stays as the record. Decision block **1780–1789**.

**Status: done, one finding.** The gate now ranks over nineteen cases in
two tiers instead of fifteen microbenchmarks. `RELEASE_OPTS` as a list
still wins ∀ in both tiers. **`BoundsElide` standing alone does not: it is
byte-identical to `dev` on every product-scale program.** That is stated
in full in §4.

---

## 1. What was added, and how each case is constructed

Four cases, in `tests/golden/cost-product-*`. **Every one of them contains
no `.wr` source.** The whole case is a one-line `root` file naming a
program that already exists in the tree for its own reasons, plus a pinned
`expected/cost.txt`:

| case | `root` | what it is | fns | dev cycles |
| --- | --- | --- | --- | --- |
| `cost-product-appliance` | `../appliance/src/image.wr` | the flagship image program: four modules, one driver, two actors, an `@image` | 21 | 2460 |
| `cost-product-actors` | `../boot-actors/input.wr` | the one program with a measured `lane2-freq.txt` and a weight in `bench/workloads.toml` | 21 | 4517 |
| `cost-product-blk` | `../boot-blk-two-devices/src/examples/boot_blk_two_devices.wr` | driver-heavy: stdlib `drivers/blk.wr`, two virtio-blk devices, `owner=driver` nonzero | 23 | 6070 |
| `cost-product-receipt` | `../boot-receipt-handoff/src/examples/boot_receipt_handoff.wr` | the largest real closure in the tree: receipts, handoff, driver bottom half, `core.io_error` | 33 | 7494 |

**Why borrow rather than copy or author (decision 1780).** Decision 1716's
first consequence is *self-selection* — every item on this plan is told to
add a `cost-*` case if none exercises its opt, so each opt is graded on a
program written to show it off. Copying a program into the case would
preserve the letter and lose the guarantee: a copy can be edited. A case
that does not *contain* its program cannot have had that program tuned for
the gate, and the four programs above are pinned by boot transcripts and
image goldens that would break if anyone touched them for this reason.

So the tier is not a label on a list somebody maintains. It is read off the
case's shape by `classify_cost_case`, which is a total function from a
directory to a tier **or an error**:

- **micro** — the case owns `.wr` source (flat `input.wr`, or a `root`
  naming a package inside the case);
- **product** — the case owns no source; `root` points outside the case.

Every other shape is an error that refuses the *whole* corpus (decision
1781): neither file, both files, an empty `root`, a `root` naming a file
that is not there, or a `root` pointing outside the case while `.wr` files
sit inside it. There is deliberately no "assume micro" branch — a case that
fell through would be scored by the gate while belonging to no tier's
verdict, which is M20's lane-nobody-ran failure in a new place.

The rule is also **collision-safe for the items running beside this one**:
B, C and E are each adding a `cost-*` case with its own `input.wr`, which
classifies as micro with no edit and no renumbering.

## 2. Lane tiering, and what it costs (H2)

The split is by cost, per CLAUDE.md, and it is stated in
`xtask::deep_lane`'s own doc comment (decision 1787):

| lane | who runs it | what it sweeps |
| --- | --- | --- |
| smoke | default `cargo test` | one **micro** case, ∀ over its box — unchanged |
| deep | `xtask::deep_lane` (full `cargo xtask check` only) | **both tiers**, plus each `RELEASE_OPTS` member alone over the **product** tier |

`cargo xtask check --fast` returns before `deep_lane()`, so none of this
lands on the per-merge lane.

**Measured, before and after.** All runs on this tree with four sibling
worktrees compiling concurrently, so every number is an upper bound; the
before-run re-measures M20's own 243 s idle figure at 411 s under the same
load, which is the scale factor to read the rest by.

| | before (15 micro) | after (15 micro + 4 product) |
| --- | --- | --- |
| `release_wins_at_every_point_of_the_residual_box` + `narrow_imm_alone_wins_at_every_box_point` | **411.05 s** | **597.07 s** |
| `each_release_opt_is_re_asked_alone_on_the_product_tier` (new) | — | **230.12 s** |
| deep lane total | **411 s** | **827 s** (~14 min) |
| points/side, release sweep | 26 112 | **36 352** |
| default `cargo test -p wrela-compiler --lib` | 7.57 s | 10.26–20.21 s |

The default-lane spread is load, not variance in the work: three census
tests outside `opts/win.rs` (`cost/branch.rs`, `cost/ab.rs`,
`cost/score.rs`) walk `discover_cost_corpus()` and now compile four more
programs. Against the locked `[tests] workspace_suite_max_us = 240 s` that
is not close to a placement failure.

**Nothing was sampled, truncated or capped.** The one bound in this path,
`MAX_SWEPT_DIMS = 14`, is untouched and was checked against the new cases
before anything else: the widest product case probes at **k=12**
(`cost-product-blk`, `cost-product-receipt`), under `cost-crosscore`'s 14.
A borrowed program reaching k=15 would have made the widened corpus refuse
to rank at all, which is that constant's own worked failure — it did not
happen, and the numbers are recorded in `MAX_SWEPT_DIMS`' doc so the next
person raising it has the table.

**Is ~14 minutes affordable?** Yes, and the answer does not need a
proposal: it is a close-item cost, run once per full gate and never by
`--fast`. If it later stops being affordable, the honest lever is the
per-opt lane (230 s), which is product-tier-only *by design* and would drop
to zero opts before any case is dropped from the ∀ sweep.

## 3. Reporting both numbers (H3)

Every table carries the tier on the row, plus per-tier subtotals and a
per-tier outcome line (decision 1783): `format_delta_table`,
`format_sweep_table` and `format_attribution_table` all changed. The sweep
prints, e.g.:

```
case cost-product-blk tier=product box_dims=17 box_cardinality=131072 swept_k=12 corners=4096
tier micro   cases=15 points_per_side=26112 outcome=wins_at_every_point
tier product cases=4  points_per_side=10240 outcome=wins_at_every_point
outcome=wins_at_every_point points_per_side=36352 tiers[micro=wins (15 case(s), 26112 points/side) product=wins (4 case(s), 10240 points/side)]
```

And the ∀ rule itself is now asked **once per tier** (decision 1782). This
is the load-bearing change, not the printing: pooled over both tiers,
"some case falls at every point" is satisfied by whichever tier is easiest,
which for every item on this plan is the microbenchmark it wrote itself.
Decision 1717 says an opt may not gate on a case it authored alone;
`SweepVeto::NoCaseFallsEverywhere { tier }` is what makes that sentence
enforceable rather than advisory.

## 4. The finding: `BoundsElide` is invisible to every program the appliance ships

**`release` vs `dev`, both tiers, ∀ over the residual box: still wins.**
15 micro cases and 4 product cases, 36 352 points per side, no case rose,
no budget overflow grew, no ordering word vanished, and some case fell at
every point *in each tier separately*. Nothing on this plan is un-landed by
item H, and item H adds and removes no opt.

**Each member alone, re-asked on the product tier** (decision 1784, pinned
by decision 1785 as `PINNED_PRODUCT_TIER_VERDICTS`):

```
BoundsElide: product=veto (10240 points/side over 4 case(s))
             reasons=[no_case_falls_everywhere:tier=product]
NarrowImm:   product=wins (10240 points/side over 4 case(s)) reasons=[]
```

`BoundsElide` does not merely fail to win on the borrowed programs. It is
**byte-identical to `dev` on all four** — same proxy cycles, same emitted
words, same hot text, at the pinned point and therefore at every point of
the box:

| case | dev | BoundsElide | NarrowImm | release |
| --- | --- | --- | --- | --- |
| `cost-product-actors` | 4517 | **4517** | 4287 | 4287 |
| `cost-product-appliance` | 2460 | **2460** | 2305 | 2305 |
| `cost-product-blk` | 6070 | **6070** | 5808 | 5808 |
| `cost-product-receipt` | 7494 | **7494** | 7234 | 7234 |
| SUB[product] | 20541 | **20541** | 19634 | 19634 |
| SUB[micro] | 124637 | 120045 | 118165 | 113997 |

Its entire measured effect is on six microbenchmarks:

| micro case | dev → BoundsElide | Δ |
| --- | --- | --- |
| `cost-bounds-elide` | 1839 → 314 | −1525 |
| `cost-ports` | 1286 → 381 | −905 |
| `cost-align` | 1090 → 331 | −759 |
| `cost-assoc-conflict` | 1605 → 959 | −646 |
| `cost-forwarding` | 724 → 341 | −383 |
| `cost-mem-locality` | 543 → 169 | −374 |

The largest of those, `cost-bounds-elide`, is the case that was written for
it. This is decision 1716's self-selection failure, measured: an opt in
`RELEASE_OPTS` whose whole justification is a corpus of programs written to
justify it, and which changes nothing in the image the appliance ships.

**Stated plainly, without softening it:** on the evidence now in the gate,
`BoundsElide` earns its place from microbenchmarks alone. Whether that is a
reason to delete it, to keep it (bounds checks in shipping code are a
correctness surface and the opt costs nothing where it does not fire), or
to find a product-scale program where array indexing is hot, is **not item
H's call** — H adds no opt and removes none, and freeze 1710 parks new
candidates. What H can say is that the question could not previously be
asked, and now it is pinned so it cannot be quietly un-asked.

`NarrowImm`, by contrast, is justified by the appliance and not only by the
corpus: it falls at every point of every borrowed program, and takes
19 067 → 13 685 emitted words out of the product tier (−28%).

## 5. What the product tier revealed that the micro tier hid

1. **The BoundsElide split above.** The micro tier says `release` beats
   `dev` by 8.5% of proxy cycles and both members contribute. The product
   tier says one member contributes 100% of the win and the other
   contributes nothing.
2. **The budget rule is inert on real programs, not just on small ones.**
   `charge = 0` on both sides of all four product cases. The two cases that
   exercise the per-core budget at all (`cost-itlb-span`, 24 428 → 18 463;
   `cost-icache-cliff`, 5229 → 2982) are both synthetic. The constraint
   that replaced the words veto (decision 1619) currently fires only on
   fixtures built to make it fire.
3. **The gate still does not score the image the appliance ships, and
   adding the appliance to the corpus did not change that** (decision
   1788, below). The same program prints two `hot_text_bytes` lines with
   the same name and an 11× gap between them:

   | | `dump --stage=cost` (what the gate scores) | `dump --stage=report` (what is built) |
   | --- | --- | --- |
   | `hot_text_bytes` | 8 256 | **91 456** |
   | `over_l1i_lines` | 0 | **405** |
   | `charge` | 0 | **2 835** |
   | `Owner name=runtime` | 1 193 | **28 069** |

   The cost stage lowers `lower::guest_reachable_keys_closure` with
   `emit_comptime_tests: false` and never builds an image; `wrela build`
   emits the whole runtime, entry and vector text (`code` section 84 392 B).
   `opts/win.rs`' module doc already warned that these two lines share a
   name; the product tier makes the size of the gap concrete on the
   flagship program. **Item D's premise — 93–98 KB of text against a
   64 KiB L1I — is a fact about the right-hand column, and the ∀ gate reads
   the left-hand one.** That is a deeper reason D scores zero than decision
   1750's order-invariance, and it is not fixed by widening the corpus.

## 6. What I could not do, and why

- **I did not make the gate score image text.** It is the single change
  that would close §5.3, and it is explicitly out of item H's scope: "if H
  wants to change what the model *reads*, that is a ruler change and is out
  of scope." Recorded as decision 1788 rather than done.
- **I did not act on the BoundsElide finding.** Deleting or defending an
  opt is a ruler/scope call for item G or the human, and freeze 1710 parks
  new candidates. H pinned the measurement and stopped.
- **`boot-device-bringup` and `boot-driver-message` cannot be scored at
  all** — `error[unimplemented]: lowering 'read_capacity_sectors': this
  image declares no 'capacity_sectors=' on its 'img.device'`. They were
  candidates for the driver-heavy slot and were passed over for
  `boot-blk-two-devices`, which scores clean. Reported, not worked around:
  the cost stage refusing a shipped program is a real gap, and it is a
  lowering gap, not a corpus one.
- **Every timing is from a loaded machine** (four sibling worktrees
  building). The before/after pair was taken under the same conditions and
  the M20 baseline was re-measured rather than quoted, so the ratio is
  sound even though the absolute numbers are high.
- **I did not run `cargo xtask check`, any boot lane, `diff-eval`,
  `repro`, `bench` or `profile`** (decision 1708 — five worktrees share one
  hypervisor). The `diff-eval` change in §7 (decision 1786) is therefore
  **unverified by a run**; it is a `continue` guarded by a pure path
  comparison with its own unit coverage in `classify_cost_case`, and the
  orchestrator's central `check` is where it first executes.

## 7. Decisions

**1780. The tier is read off the case's shape, and the rule *is* the
honesty rule.** A product-scale case owns no `.wr` source: a one-line
`root` names a program that already exists in the tree. A micro case owns
its program. Nobody can have tuned a program a case does not contain, so
"borrowed" and "product-scale" are the same predicate and the classifier
can be a total function of the directory rather than a declaration a future
case could get wrong — or quietly omit.

**1781. An unclassifiable case refuses the whole corpus.** Neither file,
both files, an empty `root`, a dangling `root`, or borrowed-but-also-
self-authored are each an error naming the case. There is no "assume
micro" fallback: a case that fell through would be scored by the gate while
belonging to no tier's verdict, and a sampled corpus ranked as if whole is
the exact failure item H exists to correct.

**1782. "Some case falls at every point" is asked once per tier.** Pooled,
the quantifier is satisfied by whichever tier is easiest, which for every
item on this plan is the microbenchmark it shipped with. This is what gives
decision 1717 force. `SweepVeto::NoCaseFallsEverywhere` now carries the
tier, and only tiers actually swept are asked — the smoke lane sweeps one
case and keeps meaning what it meant.

**1783. Every reported row carries its tier.** Case rows, per-tier
subtotals, per-tier outcome lines, and the overall line's `tiers[...]`
summary, across all three tables. A verdict a reader has to attribute to a
corpus by recognising case names is a verdict that gets attributed wrong,
and decision 1717 makes the attribution decide which number governs.

**1784. Each `RELEASE_OPTS` member is re-asked alone on the product tier,
in the deep lane.** Product tier only: the whole-corpus sweep already
covers both tiers for the list, and repeating it per member would triple a
lane already measured at 827 s. What is covered nowhere else is a member's
verdict on the programs it did not choose — 4 cases, not 19.

**1785. That verdict set is pinned, not asserted as a target.**
`PINNED_PRODUCT_TIER_VERDICTS = [("BoundsElide", "veto"), ("NarrowImm",
"wins")]` fails if either row moves **in either direction**. Pinning rather
than asserting `wins` for both is deliberate and is not a weakened gate:
freeze 1714 gates a *landing* (candidate list vs baseline list), and
`release` still wins ∀ in both tiers. What a member does standing alone on
programs it did not choose is a measurement, and the honest home for a
measurement that would otherwise be argued about is a pin whose failure
message states what it means.

**1786. `diff-eval` skips a borrowed case.** The case that owns the program
is in the same walk, so running it twice proves nothing twice and doubles
the compile. `golden_case_is_borrowed` restates decision 1780's predicate
on the harness side rather than adding a dependency from xtask to the
compiler's opt module.

**1787. `deep_lane`'s doc states which tier runs in which lane, with the
measured cost.** M20's lesson was a lane that said it ran and did not; the
counterpart is a lane whose cost is only discoverable by running it. The
before/after table lives in `MAX_SWEPT_DIMS`' doc, next to the bound whose
own history is the worked example.

**1788. Item H does not change what the model reads, and says so with the
number it declined to close.** The gate scores the cost stage's
guest-reachable closure; the appliance's shipped image is 11× larger by hot
text (8 256 B vs 91 456 B) and is the only one of the two that is over its
L1I. Making the gate score image text is a ruler change and is item H's own
named Out. §5.3 records the gap as item D's prerequisite rather than
closing it here.

## 8. Files and oracles

| file | what moved |
| --- | --- |
| `tests/golden/cost-product-{appliance,actors,blk,receipt}/` | new: `root` + `expected/cost.txt`, no source |
| `crates/wrela-compiler/src/opts/win.rs` | `CostTier`/`CostCase`/`classify_cost_case`, per-tier ∀, tier on every table, the per-opt deep lane and its pin |
| `crates/xtask/src/golden.rs` | `golden_case_is_borrowed` |
| `crates/xtask/src/main.rs` | `diff-eval` skips borrowed cases; `deep_lane` doc states the tier split and its cost |

Oracles, all in `opts::win::tests`:

- `every_cost_case_belongs_to_exactly_one_tier_and_both_tiers_are_populated`
  — the split is total *and* neither tier is empty. The second half is the
  M20 failure: an unpopulated product tier would leave
  `compare_opt_lists_over_box` reporting `wins` over microbenchmarks alone
  with nothing saying so.
- `an_unclassifiable_cost_case_fails_closed_rather_than_being_dropped` —
  all five error shapes plus both success shapes, on real directories.
- `a_win_confined_to_one_tier_is_vetoed_and_the_tier_is_named` — decision
  1782's rule on a constructed sweep, so it holds before an opt that trips
  it exists.
- `every_reported_table_names_the_tier_of_every_row` — decision 1783.
- `sweeping_an_unpopulated_tier_is_an_error` — a tier with no cases refuses
  rather than reporting a vacuous ∀.
- `each_release_opt_is_re_asked_alone_on_the_product_tier` (deep) —
  decisions 1784/1785.
- `release_wins_at_every_point_of_the_residual_box` (deep) — unchanged in
  meaning, now over 19 cases and asked per tier.
