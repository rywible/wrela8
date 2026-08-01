# Item K findings — the ruler's three defects

Working record for [codegen-pareto-2.md](codegen-pareto-2.md) item K.
Decision block **1950–1969**. Branch `cp2-K`, base `80434dbf`.

Everything this project claims about optimization is measured by the cost
model, so a defect in it is a wrong answer propagated to every landing
decision. All three named defects were real. None of them was fixed by
moving a number: **no bracket was widened, no pinned value lowered, and no
latency changed.** What changed is *which points the box contains*, *which
program the gate scores*, and *what the footprint term charges for*.

| commit | |
| --- | --- |
| `79241e2c` | **K1** — the residual box gets joint constraints |
| `d6e291fa` | **K2** — the ∀ gate scores the image that ships |
| `4b296bf8` | **K3** — the footprint term charges for density |
| (this file) | the blocklayout disposition, the ∀ re-runs, findings |

---

## Digests before / after

| | before | after |
| --- | --- | --- |
| `table_digest` | `b2484a1b9c00d7fa` | `ed181b5abc4dd5d5` |
| `provenance_digest` | `e851020b74800045` | `673bbd943e50d8ea` |
| provenance summary | `T1=39 T2=19 T3=0 T4=10 T5=16 rows=84` | **unchanged** |

The summary does not move because **no row was added or re-tiered**. The
only edit to `bench/a76-pi5.toml` is two new fields on one existing row
(§K1). Every `cost.txt` golden therefore differs from its expectation in
**the header line and nothing else** — checked case by case, §"golden diff
shape".

---

## K1. The divide-lo corner: correlated quantities modelled as independent

### What was actually wrong

The residual box is a **product of independent brackets**. Nothing in it
could express that two brackets measure correlated physical quantities, so
the box contained points no silicon can be at — and because the ∀ gate
treats every corner as equally admissible, **one impossible corner is
enough to veto a correct optimization.**

Item C hit this from two sides and named it both times (decision 1749, and
again in its "what I could not do"): a `[sweep.divide_w_latency] = [5,12]`
beside the committed `[sweep.divide_x_latency] = [5,20]` puts
`(x = 5, w = 12)` in the box, where a 32-bit divide scores **slower** than
the 64-bit divide it replaced. That describes no A76: there is one divider
and one early-termination rule, and a 32-bit divide of a value has
strictly fewer significant bits to retire than the 64-bit divide of the
same value. Item C refused to add the W-form row rather than let the gate
rank against that corner, and named the fix as a ruler change out of its
scope.

### The fix

A `[sweep.*]` row may now carry a **joint constraint**:

```toml
le = "<dim>"            # this <= that dimension
le = "<dim> - <dim>"    # this <= the difference of two dimensions
le_physics = "..."      # why the inequality holds on the machine
```

`endpoint_corners` evaluates every constraint against the whole point —
swept dimensions *and* the ones held at their pinned value — and drops
violating corners. It **fails closed** on an empty result: a box with no
admissible corner would make the ∀ gate vacuously true.

`le_physics` is required, and that is the load-bearing part of the design.
**Removing corners can only ever make a candidate easier to land**, which
makes this the single most dangerous mechanism on this plan; so the parser
refuses a constraint that

- names a dimension that does not exist, or names itself;
- chains (its target is itself constrained) — refused rather than solved,
  so the box stays something a reader can enumerate by hand;
- is unsatisfiable (the box would be empty);
- is **violated by the pinned point** — the committed model may not be
  pinned at a point the machine cannot be at;
- is **vacuous** — no corner of the product box violates it, so it is a
  comment rather than a constraint;
- carries no `le_physics`, or carries `le_physics` without an `le`.

The expression is deliberately just those two shapes. It is what the two
instances in the record need, and a general expression language here would
be a place to hide an assumption.

### The one live instance: decision 1951

`[sweep.snoop_cost]`'s own `source` already derives its high end
arithmetically — "a remote load is at worst the DRAM path, so the extra
over L3 is bounded by 347 − 35" — and then the box swept `snoop_cost`,
`dram_latency` and `l3_latency` independently. **2 of the 4 (dram, l3)
corners admitted `snoop_cost = 312` while `dram − l3` was 254 or 263**: a
remote line costing more than going to memory, on a machine whose DSU can
always go to memory instead. The constraint restates that row's published
derivation as box shape rather than as prose, at every point instead of
only at the pinned one.

**Full provenance for the changed row** (the only changed row in the file):

```toml
[sweep.snoop_cost]
lo = 0
hi = 312
pinned = 312
pessimistic = "hi"
le = "dram_latency - l3_latency"
le_physics = "a snooped remote line is at worst the DRAM path: the DSU can
  always fetch from memory instead of from a peer, so the extra a remote
  load pays over an L3 hit can never exceed dram_latency - l3_latency.
  This is the same arithmetic this row's own `source` uses to derive its
  high end (347 - 35), asserted at every point of the box instead of only
  at the pinned one"
tier = "T5"
source = "unpriced by absence. Bracket is an engineering bound: 0 (a remote
  line already in L3 costs nothing extra) to 312 — a remote load is at
  worst the DRAM path, so the extra over L3 is bounded by 347 - 35"
```

| field | value | justification |
| --- | --- | --- |
| `tier` | **T5**, unchanged | Unpriced by absence. The constraint is not a citation and does not raise the tier; it is a *relation* between two brackets, both of which keep their own tiers. |
| `source` | unchanged | Not touched. Rewriting a `source` to justify a new field would be exactly the move the relock discipline forbids. |
| `le` | `dram_latency - l3_latency` | Transcribed from this row's own `source`, which was written months before this item and states the bound as the derivation of `hi`. Nothing new is asserted; what was prose is now enforced. |
| `le_physics` | (above) | The mechanism, stated separately from the arithmetic so a reviewer can refuse the physics without re-deriving the numbers. |
| `ambiguity` | **absent**, unchanged | Not `removal_sensitive`; the record does not conflict here. |
| `pinned` | **312, unchanged** | The pinned corner `(312, 347, 35)` satisfies the constraint exactly (`347 − 35 = 312`), which is why **no scored number moves**. Checked by `unit:the_committed_pinned_point_is_physically_realizable`. |

Both digests move; no `proxy_cycles`, `charge` or `hot_text_bytes` in any
golden moves. That is the intended shape of a box-only change.

### What was **not** done, and why

**`[sweep.divide_w_latency]` is not added to the committed profile, and no
emission changed (decision 1952).** Freeze 1630 admits a `[latency]` group
only where the emitted stream contains it, and
`unit:every_swept_dimension_is_live_and_moves_the_score_the_way_it_declares`
enforces the same rule for a sweep dimension: a W-form divide row needs a
W-form divide emit site, which is C1's unlanded divide half, which is
**codegen**. Item K is a ruler item; changing emission here would have
moved every `asm-*` and `img.hex` golden and made the mandatory
`boot-actors` oracle unreadable. So the row lives in
`cost::sweep::tests::DIVIDE_W_ROW`, written out in full with its
constraint and T1 provenance, as the artefact whoever lands C1's divide
half or C4 pastes in without re-deriving the argument.

### Oracles

| unit | what it pins |
| --- | --- |
| `sweep::no_physically_impossible_divide_corner_is_enumerated` | the raw product box enumerates exactly `divide_x=5 divide_w=12` and the constrained box does not. **Fails on the old behaviour**: the mechanism did not exist, so every corner was enumerated. |
| `sweep::the_c4_shaped_comparison_no_longer_vetoes_at_the_divide_lo_corner` | the substitution rises at one corner of the product box (→ `CaseRose` → refusal) and at none of the constrained box |
| `sweep::a_constraint_is_checked_against_held_dimensions_too` | 8 product corners over `(snoop, dram, l3)` → 6 feasible; a constraint is not silently satisfied by holding one of its dimensions pinned |
| `sweep::the_committed_pinned_point_is_physically_realizable` | the committed model is not pinned at an impossible point |
| `table::the_committed_snoop_constraint_is_parsed_and_names_its_physics` | the constraint is data, and it is the only one |
| `table::reject_a_constraint_{with_no_physics,naming_no_such_dimension}`, `reject_a_self_referential_constraint`, `reject_a_vacuous_constraint`, `reject_a_constraint_the_pinned_point_violates` | the six refusals above |
| `table::a_constraint_moves_the_value_digest_and_its_physics_the_provenance_one` | a constraint is box shape (value digest); its physics is prose (provenance digest); separably |

### Is C4 rankable now?

**No — and K1 was not what was blocking it.** This is the honest answer and
it contradicts the plan's expectation, so it is stated first and in full.

K1 removes the corner where *a 32-bit divide is slower than the 64-bit one
it replaced*. C4 does not make that substitution. C4 replaces a divide by a
constant with a magic-number multiply-high plus shifts, and item C's own
analysis names the corner that refuses it: at `divide_x_latency = 5` a
divide costs 5 cycles, and `UMULH`/`SMULH` (lat 5, thru 1/4, 3-cycle
M-pipe stall) plus a materialized magic constant plus shifts costs more
than that. That is `CaseRose` at an admissible point, and the point **is**
physically realizable: it is a program whose dividends are small enough to
early-terminate. There is no correlation to assert between the divide
bracket and the multiply rows; they are not two readings of one quantity.

So what C4 needs is not a constraint. It is one of:

1. **a corpus case where C4 wins even at `divide_x_latency = 5`** — i.e. a
   program that divides by a constant in a loop, where the magic sequence's
   throughput advantage (`1/4` on pipe M against a divide that *blocks* the
   pipe, `m_pipe_block = true`) shows up rather than its latency
   disadvantage. This is a real and reachable route: the divide's blocking
   behaviour is already in the table and is the thing that does not
   early-terminate away. Item M's compute-heavy workload is where such a
   case would live.
2. **or the divide bracket becoming a distribution rather than an
   interval**, so "every divide in this program terminates in 5 cycles" is
   not asked with the same weight as "every divide takes 20". That is a
   larger ruler change than item K was scoped for and it needs its own
   provenance argument — a distribution is a claim about *programs*, not
   about the machine, and this model's discipline is that every number
   cites the machine.

K1 does discharge decision 1749's *second* reason for not adding the
W-form divide rows: the bracket geometry is no longer an obstacle. Its
first reason (freeze 1630, no emit site) stands and is now the only one.
**C1's divide half is unblocked as far as the ruler is concerned; C4 is
not.** Item L should not pick C4 up expecting the veto to be gone.

### A second live instance, found and deliberately not fixed

`[sweep.tlb_walk_cost] = [0,58]` is derived the same way: "347 − 289 = 58
is the largest residual the record brackets". Its bound is
`dram_latency − 289`, where 289 is a *measurement* (the tinymembench tail),
not a sweep dimension. The `le` grammar takes dimensions only, so
expressing it would mean admitting integer literals into the constraint
language — a place to hide an assumption, for one row. Recorded here rather
than done. The corner it admits (`tlb_walk_cost = 58, dram_latency = 289`)
is a walk costing more than the whole DRAM access that contains it.

---

## K2. `--stage=cost` and `--stage=report` disagreed about hot text

### What was actually wrong

Neither was wrong. **They score two different programs, and nothing on
either line said so.**

- `--stage=cost` scores `lower::guest_reachable_keys_closure` against the
  **stub** `core.__image_runtime`, with `emit_comptime_tests: false` and no
  image build. On the flagship that is **21 fns**.
- `--stage=report` scores what `wrela build` emits: live rtconfig from the
  real dispatch tables, image force-roots, entry and vector text. On the
  same root that is **325 fns**.

Both print `hot_text_bytes` under a line called `Budget`:

| | `--stage=cost` (what the gate read) | `--stage=report` (what ships) |
| --- | --- | --- |
| `hot_text_bytes` | 7 936 | **89 024** |
| `over_l1i_lines` | 0 | **367** |
| `charge` | 0 | **2 569** |
| fns | 21 | **325** |

So every landing decision on this plan and the last was taken against a
program the appliance does not ship, on which the per-core budget
constraint — the hard rule that replaced the retired words veto (decision
1619) — was **structurally inert**. Item H's finding that "the budget rule
is inert on real programs" was true of the closure and false of the image.
And item D's premise (93–98 KB of text against a 64 KiB L1I) was a fact
about the right-hand column, which is why a whole item could not be scored.

### The fix

`cost::stage::codegen_shipped_program(path)` returns the program the root
would **ship** plus a `TextScope` naming which it is:

- root declares an `@image` → `layout::lower_and_codegen_image`, the same
  call `wrela build` and `--stage=report` make → `TextScope::Image`;
- root declares none → it ships nothing, the closure is the whole program
  → `TextScope::Closure`.

`compile_side` (the ∀ sweep) and `compare_opt_lists` (the corpus gate) both
go through it, so the two gates rank the same program as each other and as
the shipped image. The two sides of a comparison must agree on scope or the
comparison is an error, not a rank.

**Reconciliation is structural, not asserted.** The unit checks that the
gate's appliance numbers *are* `--stage=report`'s: 23 242 cycles, 89 024 B
hot text, 367 lines over L1I, charge 2 569.

### What the gate sees now

`dev → release`, at the pinned point, over the whole corpus. Eight cases
changed program:

| case | scope | dev cycles | release cycles | dev charge | release charge |
| --- | --- | --- | --- | --- | --- |
| `cost-product-appliance` | image | 31 576 | 23 242 | **6 132** | **2 569** |
| `cost-product-actors` | image | 33 732 | 25 311 | 6 643 | 2 912 |
| `cost-product-blk` | image | 34 904 | 26 061 | 7 539 | 3 528 |
| `cost-product-receipt` | image | 37 288 | 28 203 | 8 113 | 3 962 |
| `cost-crosscore` | image | 33 183 | 24 876 | 12 397 | 5 208 |
| `cost-icache-cliff` | image | 59 860 | 42 471 | 17 451 | 11 004 |
| `cost-itlb-span` | image | 108 058 | 75 571 | 44 791 | 29 499 |
| every other case (12) | closure | unchanged | unchanged | 0 | 0 |

**Every image this tree ships is over its 64 KiB L1I** — 89–391 KB of hot
text, 367–5 092 lines over, on both cores of the two-core cases. No closure
is. That is now the coverage claim
`unit:release_words_are_reported_and_the_budget_is_the_live_condition`
pins, replacing "the two fixtures item M built to breach a budget", and it
makes decision 1619's central point concrete: an *absolute* `within_budget`
veto would now refuse the identity of every program the appliance ships.

### Oracles

| unit | what it pins |
| --- | --- |
| `stage::the_cost_stage_closure_and_the_shipped_image_are_two_programs_and_say_so` | the gap, named and measured: the closure fits its L1I and the image does not. **Fails on the old behaviour**: there was no way to ask the cost side for the shipped program's budget at all. |
| `stage::a_root_with_no_image_is_scored_as_a_closure_and_labelled_one` | the fallback is labelled, not guessed |
| `win::the_gate_scores_the_image_every_root_would_ship` | the exact image/closure split, pinned as two lists; every product-tier case ships an image |
| `win::release_words_are_reported_and_the_budget_is_the_live_condition` | image ⇒ over budget, closure ⇒ within, on every case and core |

### What was **not** done

**`--stage=cost` still dumps the closure, and its `Budget` line is not yet
tagged with its scope.** The oracle the plan set allows either agreement or
"their difference is named and intentional", and the difference is now
named in `TextScope`, in the gate's own data (`CaseDelta::scope`), in two
module docs and in a unit that measures it. Putting `program=closure` on
the rendered line is a two-token change that moves the `Budget` line of
~60 committed expectations; it is the right next step and it is left for
the re-pin rather than mixed into a lane the orchestrator has to review for
correctness. The load-bearing half — *the gate reads the shipped image* —
is done.

### Cost

Scoring the image is 12× the scoring work and 2.3× the compile:

| | closure | image |
| --- | --- | --- |
| `cost-product-appliance` compile | 74 ms | 170 ms |
| `cost-product-appliance` score, one point | 1.23 ms | 14.4 ms |
| `cost-product-receipt` score, one point | 2.75 ms | 16.8 ms |

Default `cargo test -p wrela-compiler --lib`: 19.5 s → 47.9 s, against the
locked `[tests] workspace_suite_max_us = 240 s`. Deep-lane cost is in
§"verification". Nothing is sampled, truncated or capped to buy it back;
item H's rule stands.

---

## K3. The footprint term was order-invariant

### What was actually wrong

The term charged for **overflow only** — lines beyond the L1I's ways, pages
beyond the TLBs. On a program comfortably inside both, every layout of the
same words charges zero, so block order was invisible: item D moved
`boot-actors`' measured hot text 7 616 → 7 360 B (−4 cache lines) and the
model scored it **0 → 0**.

### The fix (decision 1955)

Each fn starts at a 64 B boundary in this model, so the fewest lines *any*
intra-fn ordering of its hot blocks can occupy is

```
packing_floor_lines = Σ_fn ⌈hot bytes of that fn / 64⌉
slack_lines         = fetched lines − packing_floor_lines
```

A layout occupying more than the floor performs `slack_lines` extra
instruction-fetch line fills for bytes that never execute. That is the same
event the L1I overflow term already prices — a line that must come from L2
— reached for a different reason, so it is charged the same
`lat_l2 − lat_l1d_hit`. **The floor itself is not charged**: every layout
pays it, and pricing it would be a second static footprint term overlapping
the one already there.

`hot_code_bytes` and `slack_lines` join the budget line, so the charge's
inputs are reviewable rather than inferred from a total.

### The theorem that decides everything downstream

Under `HotBlocks::All` — `f ≡ 1`, every block hot — a fn's hot bytes are
*all* its bytes, so the floor equals the fetched line count and
`slack_lines` is **identically zero**. The flat row is order-invariant as a
theorem, not as an omission, and no committed flat number moves.

This is not a limitation of the implementation. Density can only differ
from 100 % where some blocks are cold, so **the column the ∀ gate reads
cannot see block order and cannot be made to.** Pinned by
`unit:the_flat_row_has_no_density_slack_by_construction`.

### Oracles

| unit | what it pins |
| --- | --- |
| `footprint::two_orderings_of_the_same_blocks_score_differently` | 32 one-word blocks, 16 hot: packed → 1 line, `slack 0`; alternating → 2 lines for the same 16 words, `slack 1`, `charge +7`. Same words, same hotness, different order, different score — **exactly what was impossible before**, and both sides are far inside every budget so the old term scored them identically at zero. |
| `footprint::the_flat_row_has_no_density_slack_by_construction` | the theorem above |
| `blocklayout::the_measured_hot_text_footprint_before_and_after` (before its deletion) | item D's layout moves the measured charge **91 → 49** where it moved 0 → 0 |

### The blocklayout disposition: **deleted** (decision 1956)

The measurement, in order:

1. The term is order-sensitive now, and item D's pass is what it sees:
   `boot-actors`' measured charge **91 → 49** (13 slack lines → 7, at 7
   cycles each), for +15 repair words.
2. The column the ∀ gate reads is `HotBlocks::All`, where the density
   charge is **identically zero by the theorem above**. D scores 0 in the
   gate, for a reason that no wiring can change.
3. Suppose the gate were made to read the *measured* column anyway. Exactly
   **one program in the tree has a block-grain sidecar**
   (`tests/golden/boot-actors/lane2-freq.txt`; the flagship `appliance`
   does not). So D could move at most one case —
   `cost-product-actors`, which borrows that program — and would be flat on
   all 15 micro cases and 3 of the 4 product ones. Decision 1782 asks "some
   case falls at every point" **once per tier**; the micro tier can never
   fall. **D vetoes under every wiring available in this tree.**

So the pass is not rankable, and the doctrine for a loser is deletion, not
a disabled flag. `crates/wrela-compiler/src/blocklayout.rs` (1 211 lines,
13 units) and `cost::stage::codegen_cost_stage_with_block_layout` are gone.

**What was kept**, because it is independent of the pass and outlives it:
decision 1754's same-region proof. `REGION_BYTES` and `same_region_holds`
moved into `layout.rs`, their only consumer, with their two units
(`same_region_is_the_span_property_not_the_base_property`,
`the_region_constant_agrees_with_the_cost_table`). `verify_branch_region`
still fails any image build whose branchable text straddles a 2 MiB region.

**What this deletion costs**, stated rather than buried: the 91 → 49
measurement was the only demonstration of the density term on a *real*
program, and it goes with the module. The term's own oracle is now
synthetic (`two_orderings_of_the_same_blocks_score_differently`). If block
layout is ever picked up again — item D's §8.4 argues the interesting rung
is **async** block layout, 36 % of the measured hot blocks, which this pass
never reached — it is a rewrite from that findings file, which is what
CLAUDE.md says code is for.

---

## The ∀ verdicts after all three changes

PLACEHOLDER

---

## Golden diff shape

PLACEHOLDER

---

## Verification actually run

PLACEHOLDER

---

## What I could not do, and why

PLACEHOLDER
