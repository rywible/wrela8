# Item N findings — `BoundsElide`, restored and parked

**Status: DONE (2026-07-31).** Item N of
[codegen-pareto-2.md](codegen-pareto-2.md), the first of the three
restorations decision 1910 ordered. Decision block **1911–1919**.

Headline, in two lines. **The opt is restored, parked, and provably
alive**: it is in `OptId`, out of `RELEASE_OPTS`, its knob still answers
`apply_opts`, and its transform still fires. **And the obvious re-ask
condition was measured and came back negative in the most interesting
way**: on item M's tile compositor the opt *does* move — 10 975 → 10 848
proxy cycles, 11 658 → 11 523 words, falling at all 512 points of that
case's box — but the only two functions it changes are the case's own
`@test(runtime)` assertions. The compute kernel it was supposed to
finally have customers in is untouched, because every index in that
kernel is computed rather than constant. A compute workload was not what
this opt was missing; §4 says what is.

---

## 1. What was restored, and where it lives now

Recovered per file from `019c5132^`. A reverse-apply does not land: items
I, J and K and two later fixes moved every one of the surrounding
regions. The **transform** is byte-for-byte the one item H measured; only
its wiring changed.

| what | where it is now |
| --- | --- |
| `BOUNDS_ELIDE` TLS knob, `set_bounds_elide`, `bounds_elide` | `crates/wrela-compiler/src/lower.rs` (~line 193) |
| `literal_array_index_elide` (sync) + 2 call sites | `lower.rs` — `lower_place_write`, `lower_expr` |
| `literal_array_index_elide` (async) + 2 call sites | `crates/wrela-compiler/src/flowwir_lower.rs` |
| `OptId::BoundsElide`, `PARKED_OPTS`, `apply_opts` wiring | `crates/wrela-compiler/src/opts/mod.rs` |
| the measurement oracle | `opts/win.rs`, `unit:parked_bounds_elide_still_transforms_and_is_still_flat_on_the_appliance` |
| `--with-opt` | `crates/xtask/src/main.rs`, `diff_eval` / `diff_eval_over_cases` |

**Not restored, deliberately:** the three `cost/ab.rs` A/B oracles and
the `BoundsElide` entries in other items' baselines
(`codegen.rs`'s `WITHOUT`/`WITH`/`E`, `win.rs`'s `WITHOUT_REGALLOC` and
the item-C attribution lists). Two of the ab.rs oracles asserted
`Release < Dev` on a fixture, which is simply false for an opt the
product does not ship; the third re-scored the whole cost corpus twice to
assert a monotonicity the transform's shape guarantees. And a parked opt
has no business in the baseline another item is measured against — that
was how it came to be credited with 43.2 % of release's win in the first
place. `cost/ab.rs`'s module doc now says this rather than saying the
oracles "went with the opt".

---

## 2. Decisions

**Decision 1911 — parked means out of `RELEASE_OPTS` *and off by
default*.** The TLS knob's default flips from `true` (M19, when the opt
shipped and `apply_mode` was a formality) to `false`. This matters more
than it looks: with the old default, any path that lowers without going
through `apply_opts` — a unit test, a fuzz lane, a future tool — would
silently get the parked transform. Off-by-default makes "parked" a
property of the code rather than of every caller's discipline. Net effect
on the product: none. No emitted word changes on any path.

**Decision 1912 — `PARKED_OPTS` is a list in `opts/mod.rs`, not a
comment.** The doctrine names three things a parked opt must carry, so
they are attached to the list that holds it, next to `RELEASE_OPTS`,
where the next person ranking an opt will read them. Its entry for
`BoundsElide` is §3 below. Every `OptId` is in exactly one of the two
lists, asserted (`unit:every_opt_id_is_either_shipped_or_parked`), so an
id can be neither shipped nor parked only by failing a test.

**Decision 1913 — `cargo xtask diff-eval --with-opt <OptId>`.** A parked
opt is reachable from no `CompileMode`, and `diff_eval_over_cases`
hardcoded `apply_mode(Release)`. Without a way to name the opt, the
doctrine's "must still pass `diff-eval`" is unenforceable — and that
requirement is the whole difference between a park and a graveyard. The
flag adds ids **on top of** `RELEASE_OPTS`, so the parked path is proved
correct in the configuration the product otherwise is, and it **fails
closed on a name no id has** (a typo must not quietly run the plain
release lane and report a pass about the wrong thing). `opts::opt_by_name`
derives its table from `RELEASE_OPTS` + `PARKED_OPTS`, not a third
hand-written list.

**Decision 1914 — the parked opt gets a cheap point-estimate oracle, not
a gate.** `unit:parked_bounds_elide_still_transforms_and_is_still_flat_on_the_appliance`
asserts three things and takes ~3 s: the transform still fires
(1839 → 314 on its own fixture), the refusal still holds where it was
made (exactly flat on the appliance's four), and the compositor is the
one product case it is not flat on (strictly below in both cycles and
words). The third is pinned as an inequality, not as the numbers, so
ordinary drift does not re-pin it — what it defends is the sentence in
`PARKED_OPTS` about where that delta comes from (§4). The ∀ box sweep stays where it
was, in the deep lane; un-parking needs that sweep and a human, not this
test.

**Decision 1915 — `tests/golden/cost-bounds-elide` keeps its name and its
job, and gains one.** Item L kept the case (decision 1972) with a header
saying the name was historical. It is no longer historical: the case is
where the parked opt is still exercised by name. The pinned `cost.txt` is
still the *shipped* program, so what the golden witnesses is unchanged —
the bounds *check*, which was never the thing refused.

**Decision 1916 — the re-ask condition is a capability, not a
workload.** Stated in §3, measured in §4. The first draft of this park
named item M's compute title as the thing that would justify re-asking;
the measurement showed that title gives the opt nothing in its kernel, so
the condition names the missing *capability* — a constant-index supplier
early enough for lowering to see — instead.

**Decision 1917 — a green `diff-eval` is reported with its reach.**
Measured, not assumed: of the 48 cases the lane compares, exactly one is
a case this opt changes, and it changes it only in `@test(runtime)` fns
the lane does not compile. §5 has the number, the three reasons that is
not a hole being papered over, and the specific fix.

---

## 3. The three things the doctrine requires

Reproduced from `opts::PARKED_OPTS`, which is where they live in the
tree.

### The measurement that refused it

Item H, re-verified by item L (decision 1970): **byte-identical to `dev`
on all four programs the appliance ships** — same proxy cycles, same
emitted words, same hot text, on `--stage=asm`, `--stage=cost` and
`--stage=image` for `cost-product-{actors,appliance,blk,receipt}`. Its
entire measured effect was six microbenchmarks, the largest of which
(`cost-bounds-elide`, 1839 → 314 proxy cycles) was written for it. M20's
evidence block had credited it with 43.2 % of release's cycle win; that
credit came from fixtures. The ∀ gate scored it `veto` on the product
tier — the only `veto` row `PINNED_PRODUCT_TIER_VERDICTS` has ever
carried.

Re-verified again here, at HEAD, after four items of churn moved the
surrounding code:

| case | dev | `[BoundsElide]` | Δ cycles | Δ words |
| --- | ---: | ---: | ---: | ---: |
| `cost-product-actors` | 33 732 | 33 732 | 0 | 0 |
| `cost-product-appliance` | 31 576 | 31 576 | 0 | 0 |
| `cost-product-blk` | 34 904 | 34 904 | 0 | 0 |
| `cost-product-receipt` | 37 288 | 37 288 | 0 | 0 |

### The mechanism

The transform fires only where the index is an **integer literal,
syntactically, at the point of the index**. The appliance indexes with
loop variables, actor ids and field reads; where a literal index does
occur it is usually a struct-shaped access that never became an array.
So the opt's precondition is a property of *fixture* code — code written
by someone who knew what the opt looks for. Widening the corpus to
programs nobody wrote for the gate took its measured effect to exactly
zero. That is decision 1716's self-selection failure, caught by exactly
the widening item H exists to do.

### The named condition for re-asking it

**A capability, not a workload (decision 1916).** The obvious candidate
was a workload — "a shipped program with tight indexed loops over a
fixed-size array", i.e. item M's compositor. It was measured (§4) and the
answer was: the compositor's *kernel* gives this opt nothing, because
every index in it is computed rather than constant. Naming a workload
was naming the wrong thing.

What the opt is actually missing is a **supplier of proved constant
indices**. Its precondition is syntactic — a `TypedExprKind::Int` at the
index expression, evaluated during lowering — so its only supplier today
is what the programmer typed. An index the compiler already knows is
constant (unrolled, folded, or range-proved) still misses, because by the
time anything knows it, lowering is over: item J's `ConstProp` runs on
MWIR, one stage downstream. So re-ask when either

- **(a)** `ConstProp` (or an index-range analysis) can hand this
  transform a proved-in-range constant — the capability that would give
  it customers that are not the programmer's typing; or
- **(b)** a title indexes a fixed-size array at literals in its **hot**
  code rather than in its assertions. That is a stricter bar than "a
  compute title exists", and §4 is why it has to be.

---

## 4. The compositor measurement — and where the delta actually is

`tests/golden/boot-tile-compositor`, scored as `cost-product-compositor`
(a tile compositor: background fill, nearest-neighbour scaled blit,
digest fold, over `[u32; 128]` tile buffers and a `[u32; 64]` sprite), is
the repo's first compute title and the obvious candidate for "the
workload that would make a bounds-check elider worth re-asking".

**Point estimate**, committed profile (`bench/a76-pi5.toml`), scoring the
program that would ship:

| | dev | `[BoundsElide]` | Δ |
| --- | ---: | ---: | ---: |
| proxy cycles | 10 975 | 10 848 | **−127 (−1.16 %)** |
| emitted words | 11 658 | 11 523 | **−135 (−1.16 %)** |

**∀ box sweep**, `dev → [BoundsElide]`, that case only:

```
tier product cases=1 points_per_side=512 outcome=wins_at_every_point
```

It falls at every one of the 512 points of the residual box, by −151 to
−185 cycles depending on the point, with no budget growth and no veto
reason. Taken alone, that reads as a vindication.

**It is not one, and the per-function breakdown is why.** Scoring the
same case per fn, the opt changes exactly two functions:

| fn | dev | `[BoundsElide]` |
| --- | ---: | ---: |
| `background_pass_is_exact` | 416 | 331 |
| `sprite_is_exact` | 302 | 260 |

Both are `@test(runtime)` fns. They are the case's own assertions —
`assert pixels[0] == 0xFF1B2838`, `assert pixels[127] == …`, `assert
sprite[0] == …`. **Every other function is byte-identical**, including
all four kernels: `fill_background`, `blit_scaled`, `make_sprite`,
`render_strip`. The file says why in its own comment about the blit:
*"two array reads, one array write and one call per destination pixel,
with every index computed rather than constant."* A compositor indexes
by loop variable. The transform needs a `TypedExprKind::Int` at the
index. They never meet.

So item M's workload did arrive, and it gave this opt **nothing in a
single line of hot code**. The 43.2 % credit item H demolished came from
fixtures; this −1.16 % comes from test scaffolding. It is the same
self-selection failure one level down, and it is worth saying plainly
because the point estimate and the ∀ sweep, read without the breakdown,
both say `wins`.

**What this does and does not change.** It does not change the verdict:
the opt stays parked, and nothing here argues for shipping it. It does
change the *reason* recorded on `PARKED_OPTS` — decision 1916 replaces
"a compute workload" with a **capability**: `ConstProp`, or an
index-range analysis, able to hand this transform a proved constant.
Its precondition is syntactic and evaluated at lowering time, so an
index the compiler already knows still misses; item J's `ConstProp` runs
on MWIR, one stage too late to feed it. That is the gap, and it is a
better-specified thing to re-ask than "wait for a compute title".

**And it does vindicate the parking rule, on a narrower claim than I
expected to make.** Not "the deleted opt turned out to win", but: the
measurement that decides this opt's future took forty minutes because
the opt was still in the tree to measure. Under the rule item L worked
to, answering "did item M's workload change the picture?" would have
meant rebuilding it from a diff first, and — as decision 1910 records of
the inliner — that is exactly the reconstruction that does not happen.

## 5. `diff-eval` under the parked opt — and how far it actually reaches

Run, exit 0:

```
$ cargo xtask diff-eval --with-opt BoundsElide
diff-eval: opt list = RELEASE_OPTS + [BoundsElide]
...
diff-eval: 130 test(s) agree across 48 case(s), 8 lowering-skips,
           6 exhaustive-skips, 1 quota-skips, 0 import-skips
```

(The flag fails closed on a bad name: `--with-opt NoSuchOpt` →
`no such opt. Known ids: ConstProp Gvn Dce … BoundsElide`, exit 1.)

The named boot oracle, read-only, on the case that checks its own
computed output:

```
$ cargo xtask golden --only-boot --filter boot-tile-compositor
golden: 1 expectation(s) ok (1 case(s), filter `boot-tile-compositor`, boots only)
```

**The reach, measured (decision 1917) — and it is currently nil over
*this* transform, which is the point of saying so.** I measured it
rather than assuming it. Over all 242 golden case programs, the opt
changes exactly ten:

```
boot-tile-compositor  cost-align       cost-assoc-conflict  cost-bounds-elide
cost-forwarding       cost-mem-locality cost-ports          check-flow
check-generic-use     check-map-take-backend
```

Of the 48 cases `diff-eval` actually compares, exactly one —
`boot-tile-compositor` — is in that list, and §4 shows the only two fns
it changes there are `@test(runtime)` fns, which `diff-eval` does not
compile (it compiles `TestKind::Comptime` only). Every other touched case
has no comptime `@test` at all. So the run above proves the opt breaks
nothing in 130 comparisons, and proves ~nothing *about the elision
itself*. That is the "clean about nothing" failure CLAUDE.md makes every
fuzz lane print its reach to avoid, and it applies here.

Three things keep this from being a hole I am papering over:

1. This transform was `release`'s default from M18 through item L, so
   every green `diff-eval` run in that window *was* a run over it. It is
   old, exercised code, not new code being parked sight-unseen.
2. The compile-side oracles do exercise it by name and by shape —
   `unit:parked_bounds_elide_still_elides_when_named` checks that a
   proved-in-range literal becomes `Project`/`SetField`, that an
   out-of-range literal does **not**, and both lowerers carry the same
   copy.
3. What would fix it is small and specific, and I did not do it because
   it re-pins goldens this item may not touch: a **comptime** `@test`
   that indexes a `[T; N]` at literal indices. `boot-tile-compositor`
   already has exactly those asserts — `sprite[0]`, `pixels[0]`,
   `pixels[127]` — but only as `@test(runtime)`. A comptime twin of one
   of them would give this lane real reach over the transform, at the
   cost of re-pinning that case. Recommended as a follow-up.

## 6. What I could not do

- **The whole-product-tier ∀ sweep** (`dev → [BoundsElide]` over all five
  product cases at once) was started and **killed at ~17 minutes**. Three
  sibling worktrees were saturating the machine, and the deep-lane sweep
  was both starving and being starved by the two oracles this item
  actually owes (`diff-eval`, the boot golden). The single-case sweep on
  the compositor — the one the item asked for — did complete, and is in
  §4. The five-case verdict is a deep-lane number that
  `each_release_opt_is_re_asked_alone_on_the_product_tier` will produce
  the day anyone proposes un-parking; it is not needed to keep the opt
  parked, which is the state it is in.
- **Un-parking is not attempted**, and §4 argues it should not be on this
  evidence.
- **`diff-eval`'s reach over the transform is nil** — decision 1917
  above, with the measurement behind it and the specific fix.
- No `cargo xtask check`, no `bench`/`profile`/`repro`, no unfiltered or
  `--update` golden run, per the item's own instructions. **No
  `tests/golden/*/expected/*` file is touched**; the orchestrator re-pins.
  The only golden file this item changes is
  `tests/golden/cost-bounds-elide/input.wr`, and only its header comment.
- `codegen.rs` is untouched. Item L removed `OptId::BoundsElide` from
  three *other* items' baseline opt lists there, and putting a parked opt
  back into a baseline another item is measured against is precisely the
  error that produced the 43.2 % credit.
