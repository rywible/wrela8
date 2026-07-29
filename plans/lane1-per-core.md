# Plan: per-core Lane 1 counters + quiesce-before-halt

**Status: PROPOSED (2026-07-29).** Not started. Prior rung: M19 COMPLETE
([M19.md](M19.md)); this is post-M19 integrity work, in the same family as
the "Integrity close Q" commits. Decision block 1500–1519 reserved.

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
