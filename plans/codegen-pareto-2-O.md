# Item O findings — blocklayout, restored and parked

**Status: DONE (2026-07-31).** Item O of
[codegen-pareto-2.md](codegen-pareto-2.md). Decision block **1940–1949**.

Headline, in one line: **the pass that was deleted for being unrankable
wins 2 KB of hot text and takes the density charge to zero on the
workload that arrived hours after the deletion — and this tree still
cannot score it, for two reasons that both belong to the ruler.** It is
restored, parked, and unwired. Wiring it is the human's call; the number
is below.

---

## 1. What was restored

| what | where | state |
| --- | --- | --- |
| the pass | `crates/wrela-compiler/src/blocklayout.rs` | 1 228 lines recovered verbatim from `7c97adf4^`, minus the same-region proof; 14 units, all green |
| its pipeline entry point | `cost::stage::codegen_cost_stage_with_block_layout` | restored, re-exported from `cost::mod` |
| the module registration | `lib.rs` | `pub mod blocklayout;` |

Recovered with `git show 7c97adf4^:crates/wrela-compiler/src/blocklayout.rs`
and the three wiring hunks from `git show 7c97adf4`. The IR had **not**
moved under it: `MwirProgram`/`MwirFn`/`Inst`, `codegen::mwir_block_leaders`,
`cost::layout_classes`, `cost::BlockBridge` and `cost::footprint::compute`
all still have the shapes the pass was written against, and it compiled
on the first build with no edit to its body. Every change below is to a
*number* or to a *test*, not to the transform.

### What was deliberately **not** restored (decision 1943)

Decision 1754's same-region proof — `REGION_BYTES`, `same_region_holds`
and their two units
(`same_region_is_the_span_property_not_the_base_property`,
`the_region_constant_agrees_with_the_cost_table`) — stays in `layout.rs`,
its only consumer, where item K put it. `verify_branch_region` still
fails any image build whose branchable text straddles a 2 MiB region.
Restoring the pass does not un-do that consolidation; `blocklayout.rs`
carries a comment at the old site saying where the property went and why,
so the next reader does not go looking for it.

One wart item K left behind at that site, **not fixed here** because it is
`layout.rs`'s and not this item's: the doc comment that used to describe
`verify_branch_region` now sits above `REGION_BYTES` (the move inserted
the constant between the comment and the fn), and the test doc comment for
`verify_branch_region_refuses_a_straddling_text_span` now sits above
`same_region_is_the_span_property_not_the_base_property`. Cosmetic, but it
misattributes two doc comments.

---

## 2. The three things the parking rule requires

CLAUDE.md's new clause asks a parked opt to carry the measurement that
refused it, the mechanism, and the named re-ask condition. All three are
in the module doc so they cannot drift from the code; here they are with
their evidence.

### 2.1 The measurement that refused it

Item K's, unchanged and still correct: the column the ∀ gate reads is
`HotBlocks::All`, where `slack_lines` is **identically zero by
construction** — every block hot means each fn's fetched line count *is*
its packing floor. The pass scores 0 in the gate under every wiring
available in this tree. Re-verified here on a program item K never had:
`the_compositor_is_the_workload_that_could_re_ask` pins
`slack_lines = 0` and `charge = 0` on the compositor in both `dev` and
`release`.

### 2.2 The mechanism

Sinking cold blocks can only ever shrink a *fetched line set*, and a line
set can only shrink where some blocks are cold. The gate's column has no
cold blocks. Separately, coldness is only *known* where a block-grain
sidecar exists, and exactly one is committed
(`tests/golden/boot-actors/lane2-freq.txt`).

### 2.3 The named re-ask condition

Two rungs, and item O climbed most of the first one:

1. **A block-grain sidecar for a program with L1I headroom.** Measured
   below (§4). The sidecar can be generated; it cannot yet be *read* by
   the model (decision 1947).
2. **Async block layout.** `plans/codegen-pareto-D.md` §8.4 measured 36 %
   of hot blocks in async fns; this pass reaches none of them (decision
   1756 confines it to sync MWIR). Untouched by this item.

---

## 3. `boot-actors`: three numbers moved and one invariant died

The pass's own measurement unit,
`blocklayout::tests::the_measured_hot_text_footprint_before_and_after`,
re-run under today's `RELEASE_OPTS`:

```
D-MEASURE blocklayout fns_moved=7/17 hot=52 cold=49 unmeasured=18 repairs=15
D-MEASURE words before=1982 after=2204
D-MEASURE frameless before=6 after=5 regained=1 word_delta=222 repairs=15
D-MEASURE flat_hot_text before=8576 after=9536
D-MEASURE hot_bytes=4876 per_fn_packing_floor=5312 headroom=768 captured=0
D-MEASURE core=0 measured_hot_text before=6080 after=6528 lines 95->102 charge 84->49
```

| quantity | item K measured | item O measures |
| --- | --- | --- |
| baseline measured hot text | 7 616 B | **6 080 B** |
| density charge | 91 → 49 | **84 → 49** |
| after-side hot text | ≤ before (asserted) | **6 528 B — larger** |
| words | `before + repairs` | **`before + repairs + 207`** |
| frames regained | 0 (asserted) | **1** (`__wrela_line_commit`, 0 → 160 B) |

The baseline moved because items I and J delete words; that is the
assertion doing its job and it was re-pinned with the prose the assertion
demands.

### 3.1 Decision 1941 — the invariant that is genuinely gone

`assert!(after.hot_text_bytes <= before.hot_text_bytes)` — *"packing the
hot blocks must never grow the hot line set"* — is **false**. 6 080 →
6 528 B; 95 → 102 fetched lines. It has been replaced by a pinned pair,
not relaxed: the test still fails if either side moves.

### 3.2 Decision 1942 — the mechanism, attributed by leave-one-out

The pass did not stop packing; **the allocator started reacting to the
packing**. `regalloc::allocate` builds each temp's live interval as
`[first point, last point]` in *emission* order, and item I's
`hint_admissible` (decision 1902) walks that whole interval to decide
whether an argument/return register is free across it. Sinking a cold
block that uses a value stretches that value's interval over the entire
function, the hint stops being admissible, the temp loses its home and
goes back to the frame — and every word that costs is a **hot** word,
because it is spill traffic in the hot blocks.

The per-fn shape is unmistakable (repairs in brackets):

```
__wrela_fmt_dec                 332 -> 450 words [4]   frame  48 -> 496
__wrela_line_commit             103 -> 149 words [1]   frame   0 -> 160
__wrela_console_append_bytes     41 ->  68 words [3]   frame  32 -> 112
__wrela_console_append_line_buf   30 ->  48 words [2]   frame  16 ->  80
ascii_digit                      74 ->  85 words [3]   frame   0 ->   0
```

A thirteen-way leave-one-out over `RELEASE_OPTS` puts **all** of it on
`OptId::RegAlloc`:

```
ATTR release:         words 1982->2204 d=222 repairs=15 excess=207 frameless 6->5
ATTR no-ConstProp:    ... excess=207 frameless 6->5
ATTR no-Gvn:          ... excess=207 frameless 6->5
ATTR no-Dce:          ... excess=207 frameless 6->5
ATTR no-NarrowImm:    ... excess=207 frameless 6->5
ATTR no-AdrAddressing:... excess=207 frameless 6->5
ATTR no-BfxNarrow:    ... excess=207 frameless 6->5
ATTR no-MaskCheck:    ... excess=207 frameless 6->5
ATTR no-WideImmForms: ... excess=207 frameless 6->5
ATTR no-RegAlloc:     words 2565->2580 d=15 repairs=15 excess=0   frameless 0->0
ATTR no-InterprocRegs:words 2050->2287 d=237 repairs=15 excess=222 frameless 5->3
ATTR no-Frameless:    words 2010->2228 d=218 repairs=15 excess=203 frameless 0->0
ATTR no-BranchCleanup:... excess=207 frameless 6->5
ATTR no-TailCalls:    ... excess=207 frameless 6->5
ATTR dev:             words 3784->3799 d=15  repairs=15 excess=0   frameless 0->0
```

Only the one leave-one-out that carries the whole effect is committed, as
`without_the_allocator_a_reordering_costs_exactly_its_repairs`: with the
allocator off the reordered program is `words_before + repairs` **exactly**
and no fn's residency changes. The other twelve cost seconds to re-prove a
null and the sweep is reproducible from this file.

Note `no-InterprocRegs` is *worse* (222), so this is not item I's hinting
alone — it is the allocator as item I left it. The old test comment
predicted this interaction almost exactly ("reordering blocks could then
cost a function its residency and hand it back a frame — four words that
were neither a repair nor accounted anywhere") and recorded it as retired
because item F3's relaxation had been refused. Item I turned
`pays_for_itself` back off in `allocate` and the interaction came back
with it. **That is precisely what a parked pass is for**: the interaction
would have been invisible in a tree where the pass had been deleted.

### 3.3 Decision 1945 — the charge falls while the footprint rises

Read the two pinned numbers together: charge 84 → 49 *and* hot text
6 080 → 6 528 B. Both are correct. The density term charges
`fetched_lines − per_fn_packing_floor`, and the floor is computed from
the program being scored — so a pass that makes each fn bigger raises its
own floor and books the difference as *less* slack. **The density term
ranks orderings of a fixed program, which is what item K built it for; it
is not a footprint metric and must not be read as one.** Anything that
changes word counts and is scored by it needs the hot-text column beside
it.

### 3.4 Decision 1948 — decision 1753's identity no longer holds

The pass's correctness argument rests on "the reorder unit is the MWIR
block, and that is the same partition Lane 2 keys its ids over" (decision
1753). It is now false for two fns of `boot-actors`:

```
D-MEASURE partition-mismatch fn `Ledger.mark`       mwir_blocks=2 emitted_blocks=1
D-MEASURE partition-mismatch fn `Ledger.read_marks` mwir_blocks=2 emitted_blocks=1
```

Item J's `mwir_opt` runs **inside** `codegen`, after this pass has already
planned, and its `Dce` deletes whole blocks from app methods. The list is
empty with `OptId::Dce` off and is exactly these two with it on. It is
pinned as an exact list, so a third fn joining it is a failure rather than
a drift.

Consequence: a class looked up at ordinal `k` no longer necessarily
describes the run the pass is about to move. It does not miscompile —
`verify_successors` still proves CFG equivalence for every fn moved, and
the mismatched ordinals simply classify `Unmeasured` and stay put — but it
is a **second, independent reason not to wire the pass**, alongside
decision 1755's positional bridge.

---

## 4. Decision 1946 — the compositor, which is the point of this item

Item M's `boot-tile-compositor` is the first program in this tree with
L1I headroom. Committed measurement
(`the_compositor_is_the_workload_that_could_re_ask`):

```
O-COMPOSITOR dev:     words=11658 hot_text=47744 hot_code=46632 slack_lines=0 charge=0 pages=12
O-COMPOSITOR release: words= 6877 hot_text=28480 hot_code=27508 slack_lines=0 charge=0 pages=7
                      l1i=65536
```

Item M's "~17 KB of headroom" is the **dev** column; `release` has 37 KB.
Either way the L1I overflow term is zero on both sides, so the density
term is the only footprint term that could ever rank this pass here — and
in the flat column it is zero by construction. The committed answer is
therefore a null, and the pass is byte-identical on the compositor
(`fns_moved=0/32`, `repairs=0`, all 225 blocks `Unmeasured`), because no
sidecar is committed beside it.

### 4.1 So the sidecar was generated, once, off-tree

```
$ cargo xtask gen-lane2-freq boot-tile-compositor
gen-lane2-freq: wrote tests/golden/boot-tile-compositor/lane2-freq.txt
  (189 block(s), 2455 id(s) assigned in the test image, 128 pair(s) in the
   bounded transcript line)
```

and the pass measured against it:

```
CM blocklayout fns_moved=16/32 hot=119 cold=88 unmeasured=18 repairs=31
CM words 7579->7823 d=244 repairs=31 excess=213 frameless 12->11
CM core=0 measured_hot_text 28736->26688 lines 449->417 slack 9->0 pages 8->8 charge 63->0
CM mismatch=[]
```

**On the workload that did not exist when it was refused, the pass does
exactly what it claims.** Measured hot text falls **28 736 → 26 688 B** —
2 048 B, 32 whole cache lines — the density charge goes **63 → 0**, and
the layout reaches the per-fn packing floor exactly (`slack 9 → 0`). It
sinks 88 cold blocks across 16 of 32 fns for 31 repair jumps.

The sign is *opposite* to `boot-actors`, where the same pass grows hot
text by 448 B. The difference is the ratio: the compositor has 88
known-cold blocks in a 7 579-word program, `boot-actors` has 49 in a
1 982-word one, so there the allocator's 207-word reaction swamps a
packing win the program is too small to have. The allocator excess is
present on the compositor too (213 words) and is simply outweighed.

**This is the loud part.** Item K deleted this pass on an argument whose
third step was "only one program in the tree has a block-grain sidecar, so
D could move at most one case". Item M's compositor is a second such
program the moment anyone runs one command, and on it the pass is not
marginal — it takes a real footprint term to zero. Whether that is worth
wiring is the human's decision, not mine, and the two blockers below are
real. But "unprofitable on today's corpus" is no longer the right
description of this pass.

### 4.2 Decision 1947 — and the tree cannot read that sidecar

The number above is taken with `ConstProp`/`Gvn`/`Dce` **off**, because
that is the only configuration in which the sidecar resolves at all:

```
CM-PROBE release:  axis_delta blocks=3  resolve=FAILS — "sidecar key `axis_delta#3`
                   names block 3 of fn `axis_delta`, which has 3 Lane 2 block(s) —
                   out of range (decision 1608: fail closed, never attribute by
                   nearest offset)"
CM-PROBE no-jopts: axis_delta blocks=5  resolve=ok
CM-PROBE dev:      axis_delta blocks=5  resolve=ok
```

The cause is a mode mismatch between the two closures:

- `xtask::lane2_freq::gen_lane2_freq` builds the `@test(runtime)` image
  **in-process** and never calls `opts::apply_mode`, so every TLS opt knob
  sits at its default (`false`). The image it measures is a `dev` image.
  The only xtask lane that calls `apply_mode` is `diff-eval`
  (`crates/xtask/src/main.rs:2804`); the `golden` lane escapes it by
  shelling out to the `wrela` binary, which defaults to `release`
  (`crates/wrela-compiler/src/bin/wrela.rs:919`).
- `wrela dump --stage=cost`, which the model scores, is `release`.

Before item J, no release opt changed a block partition, so a `dev`-measured
sidecar happened to key a `release` closure and nobody noticed. Item J's
`Dce` deletes whole blocks from app fns, the partitions now disagree, and
`cost::bridge` fails closed — **correctly**. The committed `boot-actors`
sidecar still resolves only because it happens to name no key that item J
deleted.

Consequence, stated plainly: **any Lane 2 block-grain sidecar generated
today for a program with app code is unusable by the cost model.** That
blocks the parked pass's own re-ask condition, and it is a defect in the
ruler, not in the pass. The fix is one line in `gen_lane2_freq` (apply the
same mode `--stage=cost` scores) plus a re-measure; it is not this item's
to make, because it changes a committed measurement artifact.

The generated sidecar is **not committed**: it does not resolve under
`RELEASE_OPTS`, and committing it would move
`tests/golden/cost-product-compositor/expected/cost.txt`, which this item
may not touch. The command that reproduces it is in the module doc and
above, which is what decision 1910 asks for — the refusal, and now the
*win*, are reproducible from this repository.

---

## 5. Decision 1940 — the park's own oracles

A parked opt that is never exercised rots. Two new units, both cheap:

- **`the_parked_pass_is_not_on_the_compile_path`.** The dynamic half
  counts: a thread-local `RELAYOUT_CALLS` (test-only) proves a normal
  release build of `boot-actors` never reaches `relayout_program`, then
  drives the parked entry point with the *measured* classification that
  moves seven fns, then repeats the normal build and asserts it is
  byte-identical word for word and frame for frame. A pass that leaked
  into the compile path through a global fails here. The static half walks
  `crates/wrela-compiler/src` and asserts the only files naming
  `relayout_program(` / `codegen_cost_stage_with_block_layout(` are
  `blocklayout.rs` and `cost/stage.rs` — so a future session that wires it
  has to delete an assertion whose message names the two decisions in the
  way.
- **`without_the_allocator_a_reordering_costs_exactly_its_repairs`.**
  Decision 1942's control half (§3.2).

**Decision 1944:** the measurement unit now prints its whole measurement
*before* it asserts anything. The interesting failures of a parked pass
are interactions with other opts, and a bare `left != right` hides them —
this item lost two rounds of turnaround to exactly that.

---

## 6. Verification actually run

| command | result |
| --- | --- |
| `cargo build -p wrela-compiler` | clean (one pre-existing `opts/win.rs` dead-field warning) |
| `cargo test -p wrela-compiler --lib` | **890 passed, 0 failed, 12 ignored** (137.8 s) |
| `cargo test -p wrela-compiler --lib blocklayout` | **14 passed, 0 failed** (0.77 s) |
| `cargo xtask golden --only-boot --filter boot-tile-compositor` | **1 expectation ok** |
| `cargo xtask gen-lane2-freq boot-tile-compositor` | wrote 189 blocks; measured; artifact removed, tree clean |
| `cargo fmt --all` | applied |

Not run, per this item's instructions: `cargo xtask check`, unfiltered or
`--update` golden, `bench`/`profile`/`repro`, `cargo test -p wrela-vmm`.

### 6.1 A pre-existing golden failure, not caused by this item

`cargo xtask golden --filter cost-product-compositor` **fails at
`0505aa1f`, before any commit of this item**:

```
- Budget n=0 hot_text_bytes=31168 hot_code_bytes=30300 ... text_pages=8 ...
+ Budget n=0 hot_text_bytes=28480 hot_code_bytes=27508 ... text_pages=7 ...
- Total proxy_cycles=9076
+ Total proxy_cycles=8597
```

`tests/golden/cost-product-compositor/expected/cost.txt` was last written
by item M's merge (`2360187f`); items I, K and J merged after it and moved
the numbers. Item K's own findings file leaves "Golden diff shape:
PLACEHOLDER". This is untouched-golden drift and is the close item's to
re-pin — this item may not commit a golden change, and did not. The
`+28480` figure is byte-identical to what item O measures in process, which
is the cross-check that it is drift and not a new fault.

---

## 7. What I could not do, and why

- **Rank the pass.** It cannot be ranked in this tree: the ∀ gate reads
  `HotBlocks::All`, where the term that sees it is zero by construction
  (item K's theorem, unchanged). Making the gate read the measured column
  is ruler plumbing and was out of scope; it is also blocked by decision
  1947 for every program but `boot-actors`.
- **Commit the compositor sidecar.** It does not resolve under
  `RELEASE_OPTS` (decision 1947) and it would move a golden this item may
  not touch. The command that regenerates it is recorded instead.
- **Fix `gen-lane2-freq`'s mode.** One line, but it changes what a
  committed measurement artifact means, and re-generating
  `boot-actors/lane2-freq.txt` would move the measured tier. It needs its
  own item and a human deciding whether the measured tier is re-measured
  or the generator is pinned to `dev` on purpose.
- **Measure the pass on the compositor under full `RELEASE_OPTS`.**
  Impossible today for the same reason — the bridge fails closed. The
  no-jopts number in §4.1 is the honest ceiling available.
- **Fix `layout.rs`'s two misattributed doc comments** (§1). Out of scope
  and touching the file item K consolidated into.

## 8. Decision index

| # | decision |
| --- | --- |
| 1940 | `blocklayout.rs` is restored **parked**: present, compiling, tested, out of `RELEASE_OPTS` and off the compile path. "Not wired" is *counted* (a call tripwire) and *grepped* (a caller allowlist), not argued. |
| 1941 | `after.hot_text_bytes <= before.hot_text_bytes` is dead (6 080 → 6 528 B). Recorded as a pinned pair with its mechanism, never relaxed. |
| 1942 | The 207-word excess and the one regained frame are `OptId::RegAlloc`'s, by a thirteen-way leave-one-out. The control half is a committed unit. |
| 1943 | Decision 1754's same-region proof stays in `layout.rs`, where item K put it. Restoring the pass does not un-do that consolidation. |
| 1944 | The measurement unit prints its whole measurement before it asserts anything. |
| 1945 | The density charge ranks orderings of a *fixed* program. It is not a footprint metric: here it falls 84 → 49 while the hot text it describes rises. |
| 1946 | On item M's compositor with a real sidecar, the pass moves hot text **28 736 → 26 688 B** and the density charge **63 → 0**. The premise item K's deletion rested on is inverted; the pass stays parked pending a human decision on wiring. |
| 1947 | `gen-lane2-freq` measures a `dev` image while `--stage=cost` scores `release`; since item J's `Dce`, the two partitions disagree and every newly generated block-grain sidecar for a program with app code fails the bridge closed. The ruler's defect, and the blocker on 1946's re-ask. |
| 1948 | Decision 1753's "the MWIR partition is the emitted partition" is false for `Ledger.mark` and `Ledger.read_marks` since item J's `Dce`. Pinned as an exact list; a second independent reason not to wire. |
| 1949 | The pass is **not** wired, despite 1946. Wiring re-keys the Lane 2 bridge (1755) and plans over a partition that is not the emitted one (1948); both are decisions, not edits. |
