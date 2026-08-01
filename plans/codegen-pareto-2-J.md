# Item J findings — the inliner (refused), then GVN/SCCP/DCE (landed)

**Status: DONE (2026-07-31).** Item J of
[codegen-pareto-2.md](codegen-pareto-2.md), the ladder's pull-in #1 and
#2. Decision block **1920–1949**.

Headline, in one line: **the ladder was half right.** 2b (GVN + constant
propagation + DCE) is the largest single win this backend has taken —
**−10.3 % proxy cycles and −10.3 % emitted words over the whole shipped
list**, larger than `NarrowImm`, larger than `RegAlloc`. 2a (the
shrinking inliner) was built, worked, and **lost words and cycles in
every framing it was asked in**; it is deleted.

---

## 1. Where the passes live, and why there

All of item J is one file, `crates/wrela-compiler/src/mwir_opt.rs`:
three ordinary functions over `mwir::MwirFn`, plus a temp walker and a
body-compaction helper they share. No pass manager, no trait over "a
pass", no generic-over-IR seam. `optimize` is a `fn` that calls whichever
of the three their TLS knobs enable, in a fixed order.

**Decision 1920 — the passes run at the top of
`codegen::codegen_program` / `codegen_program_with_async`.** That is the
one choke point every path shares: `wrela build`, `--stage=asm`,
`--stage=report`, the cost stage the ∀ gate scores, `diff-eval`, and both
fuzz lanes. Putting them there means the program the gate ranks is
byte-for-byte the program that ships — no second pipeline to keep in
sync. It is also the point at which the program is *whole*: `lower.rs`
emits an imported fn into every importing module's own MWIR and
`merge_mwir_programs` resolves the duplicates last-wins, so anything
earlier would be optimizing a different program from the one that ships.

**Decision 1927 — the async path is excluded**, the analogue of item E's
decision 1762. A FlowWir fn's temps live in the persistent turn area
precisely because they must survive a `ret`-to-scheduler suspension. A
state's `ops` are one straight-line list, so GVN would be *locally*
sound, but a temp defined in one state is read in another, and DCE would
delete a definition whose only reader is three states away. There is no
whole-body liveness to reason from without building the cross-state
graph, so FlowWir is not rewritten. It is also not read: with the inliner
gone, nothing here needs the call graph.

---

## 2. The three passes

### ConstProp — item J's SCCP slot, named for what it is (decision 1924)

MWIR is **not SSA**. `lower.rs` re-defines a loop's induction temp with a
`Copy` on every back edge (`render_strip`: `Copy dst=t8 src=t18` closing
the loop), so the sparse SSA-lattice algorithm has nothing to be sparse
over. Calling the id `Sccp` would have been a claim the code does not
support, so it is `ConstProp` and this paragraph is the reason.

What lands is dense constant propagation over an **extended basic
block** — a single-predecessor chain, computed by `ebb_leaders` as "index
0, every jump target, and every instruction whose predecessor does not
fall through". Every fact established earlier in an EBB holds at every
later point of it, so no dominator tree exists to be got wrong. The table
is dropped at every join. A `JumpIfFalse` whose condition is a known
constant is then resolved: to nothing when the condition is true, to a
`Jump` when it is false, and DCE collects the arm that goes dead.

**Every arithmetic answer comes from `eval::value`** — `eval_ordinary`,
`eval_wrapping`, `eval_div_rem`, `eval_shift`, `eval_bitwise`,
`eval_compare`, `eval_neg`, `eval_bitnot`, `eval_to_scalar`. That is the
reference implementation of these semantics (CLAUDE.md: "evaluator before
backend"), so the fold cannot disagree with the evaluator by
construction. **When the evaluator returns `Err`, the instruction is left
exactly as it was**: an overflow, a division by zero, an out-of-range
shift or conversion is an abandonment the program is entitled to, and the
backend must still perform it. `unit:const_prop_refuses_to_fold_what_would_abandon`
pins that on `200u8 + 200u8`.

### Gvn — value numbering over an extended basic block (decision 1925)

A redundant pure *scalar* computation is replaced by a reference to the
earlier result, and every later read of its destination inside the same
EBB is rewritten to read the earlier temp directly. Where the old
destination is read outside the EBB, a `Copy` has to stay; where it is
not, [`collect_gvn_copies`] deletes it outright and `RegAlloc`'s
coalescing (item I) makes whatever survives free.

Trapping arithmetic (`ArithChecked`, `DivRem`, `Shift`, `Neg`, `Convert`)
**is** in GVN's purity whitelist, and that is sound in this direction:
reaching the second of two identical trapping computations proves the
first did not trap, so the second cannot either, and both carry the
byte-identical abort wording. It is emphatically *not* sound in DCE's
direction — see below.

Aggregates and every memory read are outside the whitelist, so no alias
question is ever asked. `SetField` / `IndexSet` / `MemStore` write
*through* a base temp, and two `MakeAggregate`s with equal elements are
two distinct mutable objects, not one value. The dumb answer is to not
number them.

The scope is an EBB and not a dominator tree for the same reason:
redundancy across a join is left on the table, and this sentence is the
number-free part of saying so.

### Dce — and what it refuses to delete (decision 1926)

Two things go: an instruction no path from entry reaches, and a
*non-trapping* pure instruction whose destination is read nowhere in the
function. "Read nowhere in the function" is deliberately whole-body and
not a liveness analysis — MWIR is not SSA, a temp can be defined at
several points, and the conservative question is the one that cannot be
got wrong.

**`dce_removable` is strictly smaller than `gvn_pure`, and the asymmetry
is the point.** `ArithChecked`, `DivRem`, `Shift`, `Neg`, `Convert`,
`IndexGet` and every memory read are excluded. A dead `let _ = a + b`
still abandons in the evaluator when it overflows, so deleting it because
its result is unread would make the backend disagree with the reference
implementation — exactly the divergence `diff-eval` exists to catch.
`unit:dce_keeps_a_dead_computation_that_can_abandon` pins it.

---

## 3. The inliner: built, measured, refused (decision 1935)

It was built in full and it worked. Leaf callees only (which makes the
pass terminate without a recursion check); parameters bound by
**aliasing** rather than copying, so a splice is a strict deletion of the
call sequence rather than a trade of a `BL` for a run of `mov`s; `Return`
rewritten to a copy-into-`dst` plus a jump to the join; jump targets
remapped through a two-phase index map; every callee it emptied deleted
from the program. `diff-eval` agreed with it and all three named boot
transcripts were byte-identical under it.

### The rule it was built on (decision 1921), stated

> A call site whose callee is *inlinable* is inlined when either
> **(i)** it is that callee's only reference in the whole sealed program —
> mwir bodies and flowwir states both — in which case the body *moves*
> rather than duplicates and the callee is deleted; or
> **(ii)** the callee's body is at most **8** MWIR instructions.
>
> There are no other heuristics. Nothing consults a score, a frequency or
> a profile.

The 8 is the call sequence a site deletes, counted rather than tuned: a
frame-carrying callee's prologue/epilogue/`ret` is 5 emitted words
(`sub sp`, `str lr`, `ldr lr`, `add sp`, `ret`), the `BL` is 1, and a
two-argument site's argument and result moves are 2 — 8 words, and one
MWIR instruction of a scalar body is one emitted word under `NarrowImm`.

### What it measured

Every number below is the whole 20-case `cost-*` corpus at the pinned
point, proxy cycles unless marked.

| framing | Δ cycles | Δ words |
| --- | --- | --- |
| `dev` → `[Inline]` | **+664** | — |
| `[ConstProp,Gvn,Dce]` → `+Inline` | **+628** | — |
| `release-minus-Inline` → `release` | **+221** | **+308** |
| rule (i) only (`INLINE_MAX_BODY=0`), `dev` → `[Inline]` | **+307** | — |
| rule (i) only, `release-minus-Inline` → `release` | **+36** | — |

On the two shipped programs, `--stage=asm --mode=release`:

| | without the inliner | with it |
| --- | --- | --- |
| appliance, emitted words | 1 201 | **1 201** |
| compositor, emitted words | 6 871 | **7 197** (+326, +4.7 %) |
| compositor, hot text (`--stage=report`) | 28 416 B | **29 632 B** (+1 216 B) |

**It has no customers on the appliance at all.** The appliance's own six
methods (`CacheActor.init/touch`, `StatsActor.init/record`,
`StorageDriver.capacity/init`) contain zero `Inst::Call` between them;
every call in that image is in the shared runtime closure, which decision
1932 keeps off limits. On the compositor — a program built out of
`chan`, `lerp8`, `pack`, `axis_delta`, exactly the shape 2a was written
for — it inlined 19 sites and *cost* 326 words.

**Even rule (i) alone loses**, and that is the result that settles it.
Rule (i) is the case where the body moves rather than duplicates and the
callee is deleted outright, so words should fall by the whole call
sequence; measured, it is +36 cycles over the shipped list. Every temp
the splice moves becomes a caller frame slot, and the caller's own
spill/reload traffic grows faster than the call sequence shrinks.

Losers are deleted, not kept disabled. The inliner is gone —
`OptId::Inline`, the knob, `inline_program`, `inline_into`,
`expand_callee`, `inline_refusal`, the frame bound and the two
constants. What survives it is listed in §5.

**This reshapes the ladder.** 2a is not "the largest parked win"; on this
backend it is a loss, and the reason is structural rather than
circumstantial: `dev`'s spill-everything discipline means an inlined temp
is a frame slot, and the footprint term now charges for density (item K),
so a merged body pays twice. 2a should be re-ranked below 2b and marked
"needs a register allocator that survives the splice", not "needs
building".

---

## 4. What the three landed passes are worth

### The gate

`RELEASE_OPTS` gains `ConstProp`, `Gvn`, `Dce` — in that order, ahead of
`NarrowImm`, which is **pipeline order**: all three rewrite MWIR, the
stage before every other id's own, and decision 1763 puts everything the
allocator's probe must see ahead of `RegAlloc`.

**Decision 1937 — the chain's baseline is the rest of the shipped list,
not `dev`.** This is not baseline shopping; it is the same mechanism
decision 1791 measured for item C5 one round earlier. What these passes
delete is *words*, and on this backend words become cycles only after
`RegAlloc` has removed the schedule slack that absorbs them. Asked over
`dev` the block still falls by 31 534 cycles, so nothing rests on the
choice; what the choice buys is that the *links* are individually clean.

**Decision 1938 — `Gvn` and `Dce` are one link, and that is a finding.**
GVN replaces a redundant computation with a reference to the earlier
result; where the old destination is read outside the EBB it must leave a
`Copy`, and deleting *that* is DCE's job. Asked strictly alone, GVN falls
by 20 833 cycles across the corpus and **raises `cost-branch-bias` by 7
and `cost-mem-locality` by 1** — on `cost-branch-bias` it takes words
230 → 224 while taking cycles 190 → 197, which is a live range it
extended, not a mistake it made. `SweepVeto::CaseRose` is an absolute
veto, so a 20 833-cycle transform would have been refused for two
microbenchmark cycles it created itself. Decision 1936 makes GVN collect
the copies it introduces, which removes the reducible part of that; the
pair is asked together for the irreducible part. **`Dce`'s own link is
re-asked immediately afterwards**, so a `Dce` refusal still names `Dce`
alone; only `Gvn` loses its solo verdict, and the fact that it does is
recorded live by `unit:gvn_collects_its_own_copies`.

### ∀ verdicts, both tiers

Pinned point, whole corpus, per link (`unit:item_j_as_a_block_over_the_shipped_list`
and the chain lanes):

| link | baseline → candidate | Σ cycles | cases falling | cases rising |
| --- | --- | --- | --- | --- |
| `ConstProp` | 206 848 → 206 729 | 3 | **none** |
| `Gvn`+`Dce` | 206 729 → 185 513 | 20 | **none** |
| `Dce` (re-asked alone) | 185 849 → 185 513 | 21 | **none** |
| **all three as a block** | **206 848 → 185 513** (−10.3 %) | 21 | **none** |

Words over the same block comparison: **167 269 → 150 087 (−10.3 %)**.

∀ over the residual box, both tiers — `each_item_j_link_wins_over_the_whole_box`
(deep lane, `--ignored`) and `each_item_j_link_wins_at_every_box_point_on_its_smoke_case`
(cheap lane). Smoke corners and first-corner deltas:

| link | smoke case | corners | first corner |
| --- | --- | --- | --- |
| `ConstProp` | `cost-product-compositor` | 512 | 7 486 → 7 397 |
| `Gvn`+`Dce` | `cost-icache-cliff` | 1 024 | 29 228 → 23 473 |
| `Dce` | `cost-arith-w` | 256 | 28 → 26 |

`PINNED_PRODUCT_TIER_VERDICTS` gains `("ConstProp","wins")`,
`("Gvn","wins")`, `("Dce","wins")` — every row still `wins`, decision
1970's shape preserved. `Gvn`'s row is asked over
`item_j_baseline() + ConstProp` rather than `dev`, for the decision-1937
reason; asked over a bare `[ConstProp]` it raises all three product cases,
because without `RegAlloc` the words it deletes are absorbed by slack.

### The two shipped images

`--stage=asm --mode=release` / `--stage=report --mode=release`:

| | appliance dev | appliance release | compositor dev | compositor release |
| --- | --- | --- | --- | --- |
| emitted words | 2 660 | **1 201** | 11 658 | **6 871** |
| `movk` | 780 | **23** | 2 562 | **44** |
| `movz` | 260 | 257 | 854 | 550 |
| `mov x` | 55 | 103 | 112 | 222 |
| `bl` | 51 | 50 | 224 | 194 |
| fns | 21 | 21 | 32 | 32 |

Item J's own contribution to release (release-minus-J → release):

| | appliance | compositor |
| --- | --- | --- |
| emitted words | 1 204 → **1 201** (−3) | 7 575 → **6 871** (−704, **−9.3 %**) |
| hot text (report) | 68 928 B → 68 928 B (0) | 31 168 B → **28 416 B** (−2 752 B, −8.8 %) |
| hot *code* (report) | 54 272 → 54 260 B (−12) | 30 300 → **27 484 B** (−2 816 B) |
| text pages | 17 → 17 | 8 → **7** |
| L1I budget | 65 536 B, **53 lines over** either way | 65 536 B, **0 over**, 37 120 B spare |

**Text growth against the L1I line: none, in either direction.** All
three passes only ever delete; the appliance's 53-line L1I overflow is
unchanged because item J cannot reach the runtime that causes it, and the
compositor moves further *under* its budget and drops a whole text page.

The appliance's −3 words is the honest headline for the flagship, and §5
explains it: the appliance's application half is six methods totalling a
few dozen instructions, and everything else in that image is the shared
runtime closure item J is not allowed to touch. Item M's compositor is
where a codegen opt is visible on this tree, exactly as item M intended.

---

## 5. What could not be done, and why

### The runtime closure is off limits (decision 1932)

All three passes rewrite only the fns the *user's own source* declares.
`is_late_bound` names the rest: anything prefixed `__` or `rt_`, plus —
found the hard way — every fn `stdlib/core/runtime.wr` declares under a
plain name (`ascii_digit`, `copy_bytes_range`, `copy_line_buf_range`,
`turns`), read once from the toolchain and cached. Three independent
reasons, each sufficient:

1. **Their bodies are placeholders.** `layout.rs` *replaces*
   `__test_call_{i}`, `__test_prefix_{i}`, `__method_{n}`,
   `__enqueue_{i}`, `rt_enqueue <actor>` and `__wrela_abort_tail` with
   hand-assembled code after codegen. Optimizing a placeholder optimizes
   nothing; inlining one is a miscompile (§6).
2. **Their block partition is a committed measurement.**
   `tests/golden/boot-actors/lane2-freq.txt` is a Lane 2 block-grain
   frequency vector keyed `<fn_key>#<block_index>`, and it is
   overwhelmingly runtime keys. Decision 1608's bridge fails closed when
   a key names a block the scored program no longer has — which is
   exactly what happens when a pass merges or deletes a runtime block
   (`ascii_digit#21`, `__wrela_line_commit#3`,
   `__wrela_console_append_bytes#9` all fired). The honest options were
   "re-measure on HVF" or "do not repartition"; this item takes the
   second.
3. **It puts the win where it can be attributed.** Item F measured all
   four product cases moving by the identical −47 and −108, because what
   moved lived in the shared runtime every one of them borrows. Confining
   item J to app code makes its number a statement about the
   *application*.

**The cost of that confinement is measured and it is small**: with the
runtime included, the appliance closure went 1 204 → 1 182 words instead
of 1 204 → 1 201 — **19 words, 1.6 %**. That is the price of keeping the
measured tier valid, and it is worth paying.

**This is a standing ruler defect, and it is item K's fourth.** The
block-grain sidecar is keyed by a partition that *any* code-changing
transform invalidates. Items C2, L and E each got away with it because
their transforms were block-count-preserving or merged inside a span;
item J is the first that genuinely repartitions, and the only lever
available was to not repartition. Lifting decision 1932 needs the
sidecar's key space to survive a repartition — a stable block identity
carried through the transform, or a coarser join — plus a re-measurement
on HVF (`cargo xtask gen-lane2-freq boot-actors`). Neither is in item J's
scope.

### GVN's scope is an EBB, not a dominator tree

Redundancy across a join is not eliminated. The dumb version is the one
that is obviously right, and the win is already the largest in the tree;
the residual is unmeasured.

### The shift-count range check is untouched, and it is the next big word

The single largest block of dead words in both shipped images is a check
item J cannot express. `Inst::Shift` carries a runtime count-range test,
and codegen expands it to **19 emitted words** — `movz`/`cmp`/`b.cc` plus
a full `__wrela_abort_val` prologue — *at every shift site, including the
ones whose count is a literal that is obviously in range*. The compositor
release image emits **24** of these; the appliance closure emits 3. That
is ≈ 456 words on the compositor, ≈ 6 % of it.

ConstProp folds a shift whose *operands are both* constant, so it kills
the check there. It cannot kill the check where the count is constant and
the value is not, because MWIR has no unchecked shift and no field on
`Inst::Shift` that says "this count is in range" — that would be a real
IR change (one field, three sites: `lower.rs`, `codegen::emit_shift`,
`mwir::fmt_inst`), and it is ladder 2c/2d work rather than 2b's. It is
named here with the count so the next item can pick it up without
re-deriving it. It is the obvious successor to item J, and it is
*bigger* on the compositor than anything left in 2b.

### Not done

- No re-measurement of `lane2-freq.txt` (needs HVF and would need one per
  opt-list variant the gate compares — the design does not support that).
- `cargo xtask check` was not run; the plan reserves it for the
  milestone close and the orchestrator's merge.
- No `bench`/`profile`/`repro`.

---

## 6. The two bugs, kept because both were silent

Both were caught by an oracle rather than by reading, and both are pinned
so they cannot come back.

**Decision 1930 — a temp the walker forgot.** Every pass renames temps
through one `visit_temps_mut`. `BytesIndexGet`'s `index` was grouped with
`Project`'s literal slot number and went unvisited, so the (then-live)
inliner spliced `copy_bytes_range` into `__wrela_console_append_bytes`
with its loop counter still reading the *callee's* temp number — which in
the caller held `bump`. The guest printed **22 copies of the letter `t`**
where a test name should have been. Units were green. Both ∀ tiers were
green. `wrela test` on `boot-tile-compositor` is what showed it.

The fix is a test, not care: `visit_temps_mut_visits_exactly_the_temps_the_dump_prints`
builds one instance of all **53** `Inst` variants and asserts the
walker's visited set equals the set of `tN` tokens `mwir::fmt_inst`
prints for the same instruction. Two independently written enumerations
of the same 53 variants have to agree, and `the_variant_list_covers_every_inst_shape`
keeps the list complete.

**Decision 1929 — a placeholder body spliced as if it were the program.**
`rtconfig` generates `__test_prefix_{i}` as a `return` stub and `layout`
injects the real "test `<name>`: " body after codegen. The inliner saw a
one-instruction single-call-site leaf, spliced it into
`__wrela_test_append_prefix`, and deleted the key before layout could
inject anything. Every guest test line became a bare `ok` with the name
gone. Units green, both ∀ tiers green; **`diff-eval` is what caught it**,
which is the second time in two rounds that the cheap oracles all passed
a real miscompile.

---

## 7. Verification actually run

| lane | result |
| --- | --- |
| `cargo test -p wrela-compiler --lib` | **877 passed, 0 failed**, 12 ignored, 75 s (budget 240 s) |
| `cargo xtask diff-eval` | `diff-eval: 130 test(s) agree across 48 case(s), 8 lowering-skips, 6 exhaustive-skips, 1 quota-skips, 0 import-skips` — identical tally to the base |
| `cargo xtask golden --only-boot --filter boot-actors` | transcript **byte-identical**; only the `[cost]` dump moved |
| `cargo xtask golden --only-boot --filter boot-blk-roundtrip` | transcript **byte-identical**; only the `[report]` dump moved |
| `cargo xtask golden --only-boot --filter boot-tile-compositor` | **no artifact moved at all** — and this case checks its own computed pixels, so a miscompile fails it |
| `cargo xtask fuzz lower --iters 250` | `250 iteration(s) clean (seed=1); reached check_typed 17, lower Ok 17, codegen Ok 17` |
| `cargo xtask fuzz async --iters 250` | `250 iteration(s) clean (seed=1); reached check_typed 35, flowwir_lower 35 (15 with >=1 async fn, 32 async fns lowered), async codegen 35, test image laid out 10` |
| `each_item_j_link_wins_over_the_whole_box` (deep, `--ignored`) | see §4 |
| `each_release_opt_is_re_asked_alone_on_the_product_tier` (deep, `--ignored`) | see §4 |

Golden expectations were **not** re-pinned; the orchestrator does that
centrally. The moved dumps are `[cost]` on the boot cases and every
`asm`/`image`/`report` artifact item J changes.

---

## 8. Decisions

| # | decision |
| --- | --- |
| **1920** | Item J's passes run at the top of `codegen_program`/`codegen_program_with_async`, against the merged `MwirProgram` — the one choke point every path shares, so the gate ranks the program that ships. |
| **1921** | The inlining rule, stated: single call site (body moves, callee deleted) **or** body ≤ 8 MWIR instructions (the call sequence it deletes). No other heuristics. *Superseded by 1935.* |
| **1922** | The inliner binds parameters by aliasing, not copying, and refuses any callee with a receiver, a `mut`/`take` parameter, an `InterruptCell` op, or an assignment to its own parameter. *Superseded by 1935.* |
| **1923** | Leaf-only inlining, re-applied for at most 4 rounds — terminates without a recursion check because a cycle is never a leaf. *Superseded by 1935.* |
| **1924** | `ConstProp` is item J's SCCP slot named for what it is: MWIR is not SSA, so this is dense constant propagation over an extended basic block plus constant-condition branch resolution, folding through `eval::value` and leaving anything that would abandon exactly as it was. |
| **1925** | `Gvn` numbers pure scalar computations over an extended basic block — a single-predecessor chain, so no dominator tree exists to be wrong. Aggregates and memory are outside the whitelist, so no alias question is asked. |
| **1926** | `Dce` deletes unreachable instructions and non-trapping pure ones whose result is read nowhere. `dce_removable` is strictly smaller than `gvn_pure`: a dead overflowing add still abandons in the evaluator. |
| **1927** | The async path is excluded — the analogue of item E's 1762. A FlowWir temp is read across state boundaries, so a per-state pass has no whole-body liveness to reason from. |
| **1928** | Item J's ids lead `RELEASE_OPTS` in pipeline order: they rewrite MWIR, the stage before every other id's own, and 1763 puts everything the probe must see ahead of `RegAlloc`. |
| **1929** | Item J touches only keys the user's own source declares. Compiler-generated runtime/harness keys are late-bound — `layout.rs` replaces their bodies after codegen — so what this stage sees at them is a placeholder. |
| **1930** | `visit_temps_mut` is pinned against `mwir::fmt_inst` over one instance of all 53 `Inst` variants. A temp-shaped field the walker forgets is a silent miscompile. |
| **1931** | The inliner never builds a frame past the 4 095-byte `imm12` window `codegen::build_frame` fails closed on, computed conservatively (spill-everything) because it runs before `RegAlloc`. *Superseded by 1935.* |
| **1932** | All three passes leave the shared runtime closure alone — placeholder bodies, a committed Lane 2 block partition decision 1608 fails closed on, and attribution. Measured cost: 19 words on the appliance closure. |
| **1933** | Item F's three call-shaped units are asked over `RELEASE_OPTS` minus item J's three: item F's transform is unchanged, its *subject* is what item J folds away. |
| **1934** | The pinned block/branch/coverage counts item J moves — `cost-branchy` flat 59 → 47, `boot-actors` word blocks 312 → 310, Lane 2 blocks 184 → 182, branch words 243 → 241, `layout_classes` cold 85 → 83, C1's crossover (37, 70) → (26, 28) — are re-pinned with the reason, not rescaled. Every one is in app code; the measured join is untouched. |
| **1935** | **The shrinking inliner is refused and deleted.** +664 cycles alone over `dev`, +628 on top of the other three, +221 cycles and +308 words leave-one-out over the shipped list; rule (i) alone still +307. On the shipped images: +0 words on the appliance (no customers) and +326 on the compositor. Losers are deleted. Overturns 1921/1922/1923/1931 and the ladder's ranking of 2a. |
| **1936** | `Gvn` collects the copies it introduces rather than leaving them for `Dce` — ten lines that turn a large part of GVN's self-inflicted `CaseRose` into nothing. Relaxing the veto instead would have been tuning the ruler. |
| **1937** | Item J's chain is asked over the rest of the shipped list, not `dev` — the same mechanism 1791 measured for C5: what these passes delete is words, and words become cycles only once `RegAlloc` has removed the slack. `Gvn`'s pinned product-tier row uses the same baseline. |
| **1938** | `Gvn` and `Dce` are asked as one gate link, because GVN's residual `Copy` is DCE's to delete and GVN alone raises two microbenchmarks by a cycle each. `Dce`'s own link is re-asked immediately after, so a `Dce` refusal is still attributable; only `Gvn` loses its solo verdict. |
