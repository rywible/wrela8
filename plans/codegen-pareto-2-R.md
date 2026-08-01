# codegen-pareto-2 item R — the ∀ sweep's inner loops

Harness performance only. **No scored number moved**, and that is the
claim this item is actually about; the timings are the incidental part.
Decisions **1960–1964** (1960–1969 verified free at `15716e0d`).

Commits, in order:

| commit     | what                                                  |
| ---------- | ----------------------------------------------------- |
| `84176100` | R1 — a lean scoring entry point (1960, 1961, 1962)     |
| `d419b1cc` | R2 — one front, two backs (1963)                       |
| `244bf57c` | R1b — the per-word table reads move into `ScoreCtx` (1964) |

---

## What each change does

### Decision 1960 — `ScoreCtx`: the table constants, hoisted

`score_program_at` was the sweep's per-corner entry — 21 cases × up to
12 288 corners × 2 sides — and every call rebuilt `Machine::from_table`:
five `[pipelines]` row lookups, five string pipe-range parses, a `Vec` of
port letters. Nothing in it reads a `SweepPoint`, which is exactly what
licenses hoisting it: a `ScoreCtx` built from a table scores every point
of that table's box. Built once per case (and once per probe), not once
per corner.

Same shape as the fix already in the tree at `cost/branch.rs` — a
per-call reconstruction of a constant, inside a loop whose only job is to
vary something else.

### Decision 1961 — `score_totals_at` / `ScoreTotals`: no report

Every corner used to build a full `CostReport`:

- `table.table_digest()`, `table.provenance_digest()` and
  `table.provenance_summary()` — the first two format **every row of the
  profile** into a `String` and hash the lines; the third formats the
  tier mix;
- one `FnCost` per function, each with a cloned `String` key, an owner
  `String` and a term `BTreeMap`.

The sweep reads cycles, words, budgets and the ordering counts, and
discards the rest. `score_totals_at` returns exactly that.

Both entry points go through **one** `score_program_core`, so the lean
path cannot drift from the reported one: its `want_fns` flag decides what
is *materialized*, never what is *computed*.

### Decision 1962 — the ordering census is computed where it is read

`SideScore.ordering` needs the per-rule term map, which is one `String`
allocation per emitted word (~22 000 words on the flagship image). But
`refuse_at_point` only *reads* it at the first corner, and the reason is
already written down in its doc comment: ordering-word counts are counts
of **emitted words**, so they are identical at every point of the box.
The other `2^k − 1` corners were paying for a value nothing read.

It is now computed at corner 0 only — the same corner `sweep_case` passes
`check_ordering = true` for, so what is built and what is read are the
same corner by construction. `SideScore.ordering` became `Option`, and
`refuse_at_point` **fails closed** on `None` rather than reading an
uncomputed census as "no barriers removed". The sensitivity probe still
asks for it on both sides at every one of its points, because its whole
verdict is `a != z` over the scored value and it must compare what it
compared before; the probe is `dims × 3 bases × 2 ends × 2 sides`
scorings, not `2^k`, so it is not in the loop this item cuts.

### Decision 1963 — one front, two backs (R2)

`compile_side` ran read → lex → parse → loader → sema → `@image` eval →
layout-merge for **each** side of a comparison, on the same source, for
an answer that cannot differ between the two.

`cost::stage::ShippedFront` is that opt-independent front, run once per
case; `codegen_shipped_from` is the back half, run once per side under
the opt TLS the caller has just applied. `codegen_shipped_program` is now
the two in sequence, so every existing caller is unchanged, and
`cost_stage_pieces` grew a `_from` twin so the no-`@image` closure path
stops re-loading a closure it was already handed.

### Decision 1964 — the scoreboard's per-word table reads (R1b)

With the report gone, the sweep's cost was the scoreboard's innermost
loop, and most of what it did per **emitted word** was a property of the
*rule*, not of the point:

- `machine.uops_for(ports_for(rule, table)?)?` — a `[latency]` row lookup
  by string key, then a split of that row's port string and a fresh
  `Vec<Uop>` **heap allocation, per word, per corner**;
- `timing_row(rule, table)` — a second lookup for the throughput hold,
  the M-pipe stall and the block flag;
- `rule_latency` — a third, for a row whose `lat`, `sweep` and
  `sweep_add` cannot move within a table.

All three are resolved once per rule in `ScoreCtx`, indexed by
`rule as usize`. Two properties kept deliberately intact:

- **The error, and when it is raised.** The uop expansion caches the
  `Result`, not just the value, so a rule the profile cannot price still
  fails with the identical message and still fails only when a word
  naming that rule is scored — not at `ScoreCtx::new`. A rule missing
  from `CostRule::ALL` errors rather than reading a neighbour's slot.
- **The reads through `SweepPoint::get`.** Only the *table* half of
  `rule_latency` is cached; the point half is still read word by word.
  The set of dimensions a scoring *reads* is what the sensitivity probe
  records and therefore what decides how many corners a case is swept
  over, so caching it would have changed the box — a silent model change
  wearing a speedup's clothes.

---

## Did the front-end-is-opt-independent claim hold?

**Yes**, and it was audited rather than assumed. `opts::apply_opts` drives
fifteen knobs. Every call site of every getter:

| getter | read in |
| --- | --- |
| `lower::bounds_elide` | `lower.rs`, `flowwir_lower.rs` |
| `mwir_opt::inlining` / `const_prop` / `gvn` / `dce` | `mwir_opt.rs` |
| `codegen::narrow_imm` / `adr_addressing` / `bfx_narrow` / `mask_check` / `wide_imm_forms` / `frameless_fns` / `tail_calls` / `branch_cleanup` | `codegen.rs` |
| `regalloc::regalloc` / `interproc_regs` | `codegen.rs` |

Plus `opts/mod.rs`'s own `live_knobs()` readback test. **No call site in
`syntax/`, `sema/`, `loader.rs`, `eval/`, or `layout.rs`'s
`merge_layout_ctx`.** The front is opt-blind, so it is shared.

### The opt TLS, which `compile_side` writes

`compile_side` was never pure: its first statement is `apply_opts(opts)`,
and the sweep driver depends on the TLS state it leaves behind. After the
split:

- `apply_opts` is still the **first statement** of the per-side function
  (`compile_side_from`), still called **once per side**, still in the same
  order (baseline, then candidate);
- the TLS left behind when `sweep_corpus`'s per-case loop ends is still
  the candidate's, and `sweep_corpus` still calls
  `apply_mode(CompileMode::Release)` after the loop exactly as before;
- what **did** move: the front end now runs *before* `apply_opts(baseline)`
  rather than after — i.e. under whatever the TLS held from the previous
  case's candidate list.

That last point is safe only because the front reads no knob. It is the
one place where the audit above is load-bearing, and it is also the one
place the empirical evidence is decisive: if any front-end path read a
knob, the first case's baseline side would have been compiled under the
*previous* case's candidate opts and the whole-corpus table would have
moved. It did not move by a byte.

---

## Byte-identical evidence

Captured on the **base tree at `15716e0d`** before any edit, and again
after each commit.

**1. `format_sweep_table` for the named cases.** Four whole comparisons —
`dev → release` on `cost-arith`, on `cost-product-compositor` and on
`cost-product-appliance`, plus `release−Frameless → release` on
`cost-product-compositor` (the framing `frameless_on_the_compute_workload`
uses) — printed with every point row, every held dimension, every per-tier
verdict, the reason list and `cmp.wins()`.

```
2616 lines   md5 7b669a046b95ce680989e0da565655b3
```

Identical at base, after `84176100`, after `d419b1cc` and after
`244bf57c`. `diff` empty in all three cases.

**2. The whole-corpus ∀ evidence table.** The `--nocapture` output of
`release_wins_at_every_point_of_the_residual_box`: all 21 cases, all
29 184 scored points per side, both sides, plus the swept/held/static
dimension lists, the per-tier verdicts and the overall outcome.

```
29318 lines  md5 a68499a6a48804dbce2fb436dc3b1f89
```

Identical at `15716e0d` and at HEAD. The only textual difference anywhere
in that run's output is the panic's thread id and the `win.rs` line number
the pre-existing assertion fires on (see below).

**3. The pinned tests, unchanged and un-re-pinned.**

- `item_j_as_a_block_over_the_shipped_list` — passes; `(207_196,
  185_636)` untouched.
- `frameless_on_the_compute_workload` — passes; 512 points, falls at
  every one, rises at none.
- `cargo xtask golden --no-boot --filter cost-` — `21 expectation(s) ok`.
  No `--update` was run at any point; no `tests/golden/*/expected/*` file
  is touched by any of the three commits.
- `cargo test -p wrela-compiler --lib` — **905 passed, 0 failed, 14
  ignored**, the same counts as at base.

Nothing was re-pinned. No expectation file was edited.

---

## Timings

Apple M4. **The machine was shared with item S's builds and test runs
throughout**, so these carry real noise — see the R1→R2 deep pair below,
which is inside it.

| lane | base `15716e0d` | after R1 | after R2 | after R1b (HEAD) |
| --- | --- | --- | --- | --- |
| `--lib release_wins_at_every_point_of_the_residual_box -- --ignored` | **202.56 s** | 167.75 s | 169.58 s | **134.53 s** |
| `--lib` (whole default unit lane) | **42.44 s** | 35.59 s | 33.53 s | **31.73 s** |
| `--lib frameless_on_the_compute_workload` | **1.45 s** | 1.37 s | 1.29 s | **0.95 s** |

(`finished in` figures; wall-clock `real` was within 0.05 s of each.)

Deep lane **202.56 s → 134.53 s, −33.6 %**. Default unit lane **42.44 s →
31.73 s, −25.2 %**. The named single-case sweep **1.45 s → 0.95 s,
−34.5 %**.

**R2's own effect is not visible in the deep-lane column** — 167.75 →
169.58 s is noise, not a regression — because the deep lane runs one
front per case (21 of them) against 29 184 scorings per side. It was
measured directly instead, under `RELEASE_OPTS`:

| case | front (shared) | back (per side) | comparison before | after |
| --- | --- | --- | --- | --- |
| `cost-arith` | 1.7 ms | 2.9 ms | 9.2 ms | 7.5 ms |
| `cost-product-compositor` | 50.2 ms | 19.2 ms | 138.8 ms | 88.6 ms (−36 %) |
| `cost-product-appliance` | 48.3 ms | 118.3 ms | 333.2 ms | 284.9 ms (−15 %) |

So R2 does **not** halve the compile work as hoped: it removes the front
end from the second side, and the front is 72 % of a side's compile on the
compositor but only 29 % on the appliance. It is worth most to the callers
that compile many cases and score few points — the `compare_opt_lists`
family — and least to the deep lane.

---

## A pre-existing failure, found and not fixed

`release_wins_at_every_point_of_the_residual_box` **fails at
`15716e0d`**, before any change here, and fails identically at HEAD:

```
assertion `left == right` failed: cost-align: corners must be 2^k
  left: 256
 right: 512
```

The sweep itself is green — `outcome=wins_at_every_point`, both tiers —
and the whole evidence table prints before the assertion fires. The
assertion is the problem. It dates from `bcfdf922` (M20 item J) and reads
`c.points.len() == 1 << c.swept.len()`; `79241e2c` (item K, joint
constraints on the residual box) then made `endpoint_corners` **filter
infeasible corners**, so a case whose swept set contains two constrained
dimensions no longer has `2^k` of them. 15 of the 21 cases are now short:
`swept_k=9 → 256`, `swept_k=13 → 4096`, `swept_k=14 → 12288`.

Not fixed here. Correcting it means changing a pinned expectation, which
is a gate question, not a harness-performance one, and this item's whole
contract is that nothing pinned moves. It is flagged rather than touched.

---

## Undone, and why

- **`footprint::compute` still runs per corner in full.** Its per-core
  line, page and data-page sets are built by walking every hot block of
  every fn and inserting into three `BTreeSet`s — and **none of that
  depends on the point**. Only the final `charge` reads
  `l2_latency`, `l3_latency` and `tlb_walk_cost`. Caching the sets per
  side and re-deriving only `charge` per corner is the obvious next win
  and is very likely the largest one left. It is not done: it needs a
  per-side cache keyed on the program, and it must keep the three
  `point.get` calls firing per scoring or it moves the probe's read set
  and therefore the box. Deferred deliberately, with the identity
  harness in this item's history as the way to check it.
- `cargo xtask check` was not run (out of scope for this item by
  instruction). Verification here is: the whole default unit lane, the
  21 `cost-*` goldens filtered, the two named pinned tests, and the two
  byte-identity diffs above.
- No `bench` / `profile` / `repro` lane was run, so no threshold was
  re-locked. Nothing here is a compiler-visible perf change — the
  compiler emits the same words in the same time; only the *scorer* got
  faster.
- Timings are single runs on a loaded machine. They are honest wall-clock
  numbers, not a benched distribution.
