# Plan: per-core Lane 1 counters + quiesce-before-halt

**Status: COMPLETE (2026-07-29).** All four items landed; `unpinned-lane1` is
deleted and `boot-cross-core-ring-full` pins its trailer again, which was this
plan's exit criterion. Prior rung: M19 COMPLETE ([M19.md](M19.md)); this is
post-M19 integrity work, in the same family as the "Integrity close Q"
commits. Decision block 1500–1519 reserved; 1500–1504 used as written.

**What landed, per item** (records at the bottom):

| Item | Commit | Shape |
| --- | --- | --- |
| A | `1e66cf28` | `LANE1: Lane1Stripe`, `N_CORES` slots per counter, folded at dump; device-window verifier |
| B+C | `77926555` | bounded quiesce + `PENDING.idle` release/acquire; `run_one`'s definition written down |
| D | this commit | stopgap deleted, trailer re-pinned, clause opened, standing repro lane |

Three deviations from the plan as written, each forced and each recorded in
full below: the stripe is **struct-of-arrays** (a nested placed-static index
does not lower), `LANE1` **moved** to 0x4000b000 (two rows at the old address
reach into `LANE2`), and item C needed **no code change** (the field was
already batching-invariant after the audit note plus items A and B) — so it
delivered the definition and the measurement instead.

## The defect

`LANE1` is one shared `@placed(0x40008000)` static, and every update is a
**non-atomic read-modify-write executed by whichever core drains**:

| Site | `stdlib/core/runtime.wr` | Enclosing fn |
| --- | --- | --- |
| `LANE1.method_hits[flat] +% 1` | `:606` | `__wrela_lane1_record_method` |
| `LANE1.run_one_hits +% drained` | `:1095` | `__wrela_rt_run_one(core)` |
| `LANE1.run_one_hits +% 1` | `:1119`, `:1128` | `__wrela_rt_run_one(core)` |
| `LANE1.messages +% 1` | `:1427` | `__wrela_rt_enqueue(root, …)` |
| `LANE1.turns +% 1` | `:1597` | `__wrela_rt_select(root)` |

plans/M15.md item I deleted the Condvar baton, so released cores run
`hv_vcpu_run` concurrently. Two cores draining different mailboxes therefore
execute `ldr / add / str` against the same address with no mutual exclusion,
and **an increment can be lost**.

There is no barrier fix. `@dmb(ishst)` / `@dmb(ishld)` exist (item H, used at
`runtime.wr:1208/1237/1280/1309`) but a barrier orders *visibility*, not
read-modify-write atomicity. The language has **no atomic or load-exclusive
intrinsic** at all (`encode.rs` carries an unused `enc_ldaxr_w`, so the ISA
layer anticipates one; nothing exposes it).

### Why it matters beyond a flaky transcript

`ledger/ledger.toml:697` names Lane 1 as a **cost-model frequency source** —
"Frequency sources: Lane 1 method/turn transcript counters (committed
`lane1-freq.txt` fixture for offline dump)", with the fixture living at
`tests/golden/boot-actors/lane1-freq.txt`. A lost increment silently biases
the `f` vector in `Σ f(fn)×s(fn)`, i.e. it under-reports a hot method. That
is the same class of error clause `compiler.costs.predicted-vs-measured`'s own
note calls out: "a proxy win bought by measuring less".

### Current exposure — measured, 2026-07-29

10 boots per case on one machine, comparing the pinned `expected/test.txt`:

| Case | reproduces the pinned trailer |
| --- | --- |
| `boot-cross-core-ring-full` | **2 / 10** |
| `boot-cross-core-admission-order` | 10/10 here; 19/20 over a larger sample |
| `publish-acquire`, `mailbox-depth`, `call`, `driver-mailbox`, `two-senders` | 10/10 |
| `boot-cores-2`, `boot-cores-3`, `boot-cross-core` | 10/10 |

The RMW window is three instructions, so the **lost-update race is latent, not
currently the cause of any failure**. `ring-full`'s flake has a different and
simpler cause (below) and has already been unpinned; the race is what would
make a naive re-pin flake again later, and it is why item B alone is not
enough.

`ring-full` was already unpinned as a stopgap: marker
`tests/golden/boot-cross-core-ring-full/unpinned-lane1`, honoured by
`xtask::golden::{lane1_is_unpinned, strip_lane1_lines}`. **Deleting that marker
is this plan's exit criterion.**

## Decisions

**1500. Stripe; do not add an atomic.** `static LANE1: [Lane1Counters; N_CORES]`,
indexed by the core that is already threaded into every update site. Each core
owns its row, so there is no concurrent RMW to make atomic. Rejected:
exposing an atomic-add intrinsic — that is a `docs/language/` change plus a new
codegen intrinsic plus a ledger clause, to buy mutual exclusion this design
does not need. Rejected: a guest lock — same objection, worse.

**1501. Size by the generated `N_CORES`, not `CORE_SLOTS`.** `rtconfig`
already emits `N_CORES` and `SCHED` already stripes by it
(`RuntimeTables::stripe_for_cores`). `CORE_SLOTS = 32` would cost
32 × 1048 = 33,536 bytes of rtdata for a machine that may have three cores.
A one-core image keeps exactly one row and its present byte size.

**1502. Sum at dump, in core order.** `__wrela_lane1_dump` folds rows
`0..N_CORES` before emitting. The emitted text keeps its current shape, so
single-core goldens move only where the *type* spelling changed (item A), not
where the counts did.

**1503. `run_one` stays a poll count and stays unpinnable.** Striping makes it
*correct* (no lost updates) but not *deterministic*: it counts how many times
a core polled, which is an interleaving artifact. The 2026-07-29 audit note
above `__wrela_rt_drain` already redefined it toward "messages drained"
(batching-invariant); item C finishes that. Until then no golden pins
`run_one` for a multi-core case.

**1504. Quiesce is bounded and fails closed.** Item B waits a bounded number
of park polls for released cores to reach `Parked`, then halts regardless and
the transcript records that it timed out. An unbounded wait would turn a
scheduling bug into a hang.

## Items

### A. Stripe `LANE1` per core

Mechanical, and the whole blast radius lands here.

**Files:**
- `stdlib/core/runtime.wr` — `static LANE1: [Lane1Counters; N_CORES]`; the six
  update sites take the core they already have (`rt_run_one(core)` /
  `rt_drain(core)` directly; `rt_select(root)` / `rt_enqueue(root, …)` /
  `__wrela_lane1_record_method` via the root's owning core — thread it as a
  parameter rather than re-deriving it, and refuse rather than default it).
  `__wrela_rt_primary_boot`'s zeroing loop (`:1925-1931`) zeroes every row.
- `stdlib/core/runtime.wr` — `__wrela_lane1_dump` sums rows before emitting
  (decision 1502).
- `crates/wrela-compiler/src/rtconfig.rs` — no new const; `N_CORES` exists.
- `ledger/census.toml` — `[placed_static].fixed_core_names` keeps the name
  `LANE1`, so `FIXED_SET_LEN = 81` is unchanged; re-check `spans`
  (`N_INIT_SLOTS`) because the init-span coalescing sees a wider static.

**Blast radius, counted 2026-07-29:** 61 goldens pin
`PlacedStatic name=LANE1 type=Lane1Counters … size=1048`; 65 pin the
`Layout name=Lane1Counters` block; 80 pin a `lane1 …` trailer. Multi-core
cases additionally move `rtdata` sizes and every address above `LANE1`.
Update with `cargo xtask golden --update`, then **review the diff** and cite
the clause item D opens (`machine.lane1.per-core-counters`) in the commit, per
CLAUDE.md's golden-update rule. Do not fold this into any other commit: the
golden diff is the review surface, and a ~65-file layout move is only
reviewable on its own.

**Cheap:** `cargo test -p wrela-compiler --lib` layout/rtdata filters;
`wrela dump --stage=report tests/golden/boot-cores-1/input.wr` shows one row
and the same `size=1048` a one-core image has today;
`--stage=report tests/golden/boot-cores-3/input.wr` shows three.
**Focused boot:** `boot-cores-3` and `boot-cross-core-two-senders` only.
**Deferred to D:** full `check`.

### B. Bounded quiesce-before-halt in the entry driver

This is what makes `turns` / `messages` / `method_hits` deterministic
*totals*: the work a program does is fixed, only the sampling instant was
racy.

**Files:**
- `stdlib/core/runtime.wr` — before `__wrela_rt_summary_and_halt` dumps,
  core 0 polls the released cores' park state up to a fixed bound
  (`@budget`-shaped, like the existing drain loops), then proceeds. On
  timeout the trailer says so, in one fixed spelling, so a timeout is
  visible in the golden rather than silently indistinguishable from success
  (decision 1504).
- `crates/wrela-vmm/src/boot.rs` — no change expected; park/release state
  already lives in `Shared.sched.state`.

**Cheap:** unit test on the bound; `boot-cross-core-ring-full` reproduces its
Lane 1 trailer 20/20 (it is 2/10 today).
**Focused boot:** `boot-cross-core-ring-full`, `boot-cross-core-admission-order`.
**Deferred to D:** full `check`.

### C. Make `run_one` batching-invariant, or drop it

Finish what the 2026-07-29 audit note started. Either count messages drained
(invariant to how publishes batch — the note's own argument) or remove the
field from the trailer. Pick one; do not leave a field whose value depends on
poll batching in a pinned surface.

**Files:** `stdlib/core/runtime.wr` (`rt_run_one`, `lane1_dump`); the 80
trailer goldens if the field's meaning or presence changes.

**Cheap:** `boot-cross-core-admission-order` 20/20 (it is ~19/20 today, and
its historical flake was exactly this field, 12 vs 13).

### D. Close: re-pin `ring-full` and delete the stopgap

- Delete `tests/golden/boot-cross-core-ring-full/unpinned-lane1`.
- Delete `lane1_is_unpinned` / `strip_lane1_lines` from
  `crates/xtask/src/golden.rs` and the branch that calls them.
- Re-pin `ring-full`'s trailer, then run its case **20×** and require 20/20
  before committing the pin. A pin that reproduces 19/20 is not a pin.
- New ledger clause — proposed id `machine.lane1.per-core-counters`: "Lane 1
  counters are per-core and summed at dump; no counter is a shared
  read-modify-write across concurrently running cores." Tested by items A+B's
  goldens. This is the clause item A's golden update should cite.
- Re-run the cost-model surface: `lane1-freq.txt`'s fixture and the
  `Workload name=…` rows in the cost goldens, since item A changes how the
  frequency vector is gathered. Record before/after in this plan.

**Expensive (close only):** full `cargo xtask check`; `bench guest` deltas if
the quiesce loop moves them.

## Explicitly out of scope

- **An atomic-add intrinsic** (decision 1500). If a future rung wants one,
  `enc_ldaxr_w` is already encoded and unused, but it needs
  `docs/language/` + ledger + codegen and buys nothing here.
- **`machine.cross-core.publish-acquire-barrier`** stays `gap` per
  [BLOCKED.md](BLOCKED.md) known risk 4. This plan does not touch the
  publish/acquire barriers or their mutated arm; it is about counter
  ownership, not store visibility. Do not conflate them: item A would still
  be correct on a host that never reorders.
- **Exhaustive schedule enumeration** (M15 item J, cut). Item B makes the
  *totals* deterministic; it does not make the *interleaving* reproducible,
  and does not revive the enumerator.

## Why this is a plan and not a cleanup

Three properties, any one of which would be enough:

1. It changes the machine's placed-static layout, which ~65 goldens pin.
2. It changes a cost-model input, so it wants its own before/after and its
   own ledger clause.
3. `LANE1` is a member of the closed `fixed_core_names` census set, so the
   change is visible in `placed_static_census.rs`'s own oracle.

The stopgap (unpinning one case, with the reason recorded in the marker file)
is deliberately the smallest thing that stops an oracle asserting something
untrue, and it is reversible in one `rm` once item D lands.

## Records (2026-07-29, closing)

### Forced deviations

**A1. Struct-of-arrays, not `[Lane1Counters; N_CORES]`.** `lower.rs`'s
`placed_array_field_index` resolves exactly **one** array-field index into a
placed static; a nested `LANE1.cores[core].method_hits[flat]` refuses to lower
at all ("an array length expression that is not a literal is not implemented
yet") and the generic array path is not a placed-memory store. `Lane1Stripe`
is therefore four arrays (`turns`/`run_one_hits`/`messages` of `N_CORES`, plus
`method_hits` of `N_CORES × METHOD_CALL_POOL_COUNT` under a new module const),
which keeps every access single-level and keeps a one-core image's byte layout
identical to the old `Lane1Counters` (`turns@0x0`, `run_one_hits@0x8`,
`messages@0x10`, `method_hits@0x18`, `size=1048`). Extending the compiler to
lower two levels was the alternative; it buys nothing here.

**A2. `LANE1` moved from 0x40008000 to 0x4000b000.** The plan assumed the
stripe could grow in place. It cannot: `LANE2` sits at 0x40008800, so **two**
rows (2096 bytes) already cross into it, and nothing would have complained —
a placed static is an address plus a layout type. Moving the growing static
*above* the fixed page gives it the rest of the window and leaves the host's
pinned `LANE2_BASE` (`wrela-vmm/src/lane3.rs`) untouched, which is also the
smaller golden diff. `layout::verify_device_window_statics` now refuses any
image whose placed statics overlap or leave `0x4000_8000..0x4001_0000`,
scoped to that window because `INIT_SPAN{k}` overlays legitimately alias
rtdata state elsewhere (measured: 15 report goldens have an intentional
`RT`/`INIT_SPAN0` overlap, so a global rule would be false).
**Consequence, stated rather than hidden:** at `METHOD_CALL_POOL_COUNT = 128`
the window holds 19 rows, so an image declaring more than 19 cores is now
refused by name. `CORE_SLOTS` is 32 and no golden declares more than 3.

**C1. `run_one` needed no code change.** After the 2026-07-29 audit note
(`drained`, not one-per-call) plus items A and B, every increment is a unit of
work the program determines — messages drained, turn slices, child slices —
and unproductive polls, including every spin of item B's quiesce loop, count
nothing. The field does not depend on poll batching, so neither of the plan's
two options (redefine to messages-drained-only, or delete) was the right
move: messages-drained-only would read 0 on every single-core image, and
deletion would drop a live signal. Item C delivered the definition, written
next to the counters, and the measurement.

### Measured: trailer reproducibility

20 boots per case, this machine, after items A+B (before → after):

| Case | before | after |
| --- | --- | --- |
| `boot-cross-core-ring-full` | 2/10 (five distinct trailers over 20) | **20/20** `turns=1 run_one=2 messages=1`, `hits=0:1` |
| `boot-cross-core-admission-order` | 19/20 historically | **20/20** `turns=5 run_one=13 messages=5` |
| `boot-cross-core-two-senders` | 10/10 | **20/20** `turns=4 run_one=10 messages=4` |
| `boot-cores-2`, `boot-cores-3` | 10/10 | **20/20** `turns=1 run_one=3 messages=1` |

No boot printed `lane1 quiesce=timeout`. The 20× measurement is not the
standing oracle: `xtask repro`'s `repro_lane1_trailer_repeats` re-boots
`ring-full` and `admission-order` five times each on every `check` and fails
on any trailer difference or any timeout, because a golden that boots once
cannot tell a pin from a race it won.

### Measured: the cost-model surface (item D's before/after)

The **frequency vector is unchanged**, which is the number that matters: the
control case `boot-actors` still dumps `lane1 hits=0:3,1:1,2:3,3:2,4:2`, so
`tests/golden/boot-actors/lane1-freq.txt` needed no regeneration, and its
`--stage=cost` rows are identical before and after
(`Workload name=flat proxy_cycles=5055`,
`Workload name=boot-actors proxy_cycles=4351`, `coverage=11/11`).

What moved is the **runtime's own scored mass**, uniformly, because the
runtime got bigger (guards, the fold loops, the quiesce loop, the enqueue
hop). Every runtime-bearing report moved by the same delta:

| Case | `code` size | `Cost … total=` |
| --- | --- | --- |
| `boot-cores-1` | 85648 → 87488 (+1840) | 29227 → 29740 (+513; A +440, B +73) |
| `boot-cores-3` | 82916 → 84756 (+1840) | 27161 → 27674 (+513) |
| `boot-actor-smoke` | 80876 → 82716 (+1840) | 26534 → 27047 (+513) |
| `appliance` | 82552 → 84392 (+1840) | 27084 → 27597 (+513) |

The `cost-*` goldens (pure app scoring, `Owner name=runtime proxy_cycles=0`
except `cost-runtime`) did not move at all. `PENDING` grew 256 → 2304 bytes
inside its existing page, so no address moved for it.

### Close verification (the expensive lane)

`cargo xtask check`: **ok** — fmt, all crate tests, the full golden suite
(every boot golden, with nothing stripped from any transcript), corpus, fuzz
smoke, `stdlib-test`, `report-determinism` (92 cases), every `repro` lane
including the new `repro_lane1_trailer_repeats`, all four `bench` lanes within
their locked thresholds, and `ledger` at 210 clauses / 198 tested / 12 gaps
(`machine.cross-core.publish-acquire-barrier` still among them, as intended).

Two notes worth keeping:

* It ran while another session was deliberately saturating this host (ten spin
  loops plus a `repro` loop), so the quiesce bound was exercised under real
  contention rather than on an idle box — no boot printed
  `lane1 quiesce=timeout`.
* `bench guest` on `boot-actors` was unmoved by the quiesce (median 142773us
  against a locked 700000us; `transcript=101 byte(s), exit_code=0, exits=1,
  choices=0` on every timed boot). Single-core images take the
  `N_CORES <= 1` early return, so the loop never runs there — the whole point
  of that branch.

### Ledger

New clause `machine.lane1.per-core-counters` (`doc = 04-compiler.md#5`, where
Lane 1 is named as a frequency source), status `test`, citing the five boot
goldens, the three new unit tests, and `xtask:repro`. It records the whole
argument, including what was deliberately **not** touched:
`machine.cross-core.publish-acquire-barrier` stays `gap` per
[BLOCKED.md](BLOCKED.md) known risk 4 (that clause is store *visibility* under
a deleted barrier; this one is counter *ownership*), and M15 item J's
enumerator is not revived.
