# wrela

One designed machine, front to back: a language and compiler (no LLVM, no
external linker), a stdlib with the drivers, and a VMM implementing the
wrela machine. Flagship: wrela OS on Raspberry Pi 5 / 1 GiB.

## Ground truth, in order

0. `ROADMAP.md` — the standing doctrine (dumb-and-correct) and milestone
   ladder — plus `plans/M<n>.md`, the active milestone's ordered plan
   (its first deliverable; sessions pick the next item from it). Neither
   is relitigated mid-milestone.
1. `docs/language/` — normative. If code disagrees with the docs, the code
   is wrong. Doc changes are deliberate and human-reviewed.
2. `ledger/ledger.toml` — maps every normative clause to tests (or an
   explicit `gap`). Progress = shrinking the gap list.
3. `tests/golden/` — pinned artifact dumps. The golden diff is the review
   surface.
4. The code — disposable. Any crate should be rewritable from docs +
   contracts + tests alone. If that stops being true, the structure is
   wrong, not the rewrite.

## Commands (no CI — these run locally, always)

- `cargo xtask check` — the gate: fmt, tests, golden, ledger. Run before
  calling a milestone (or any multi-item body of work) done — **not**
  after every plan item (see Verification below).
- `cargo xtask golden [--update]` — golden tests; `--update` rewrites
  expectations, then you review the diff and cite a ledger clause id in
  the commit.
- `cargo xtask corpus` — every ```wrela block in the docs must lex (and,
  from M1, parse). The docs are test inputs; drift is a failure.
- `cargo xtask stdlib-test` — comptime `@test`/`@test(exhaustive)` under
  `stdlib/tests/`; fail closed if empty. Wired into `check`.
- `cargo xtask fuzz [lexer|parser|sema|eval|lower|async|imports]
  [--iters N] [--seed S]` — deterministic in-tree fuzzer, no external
  engine. All seven lanes are live: seeded splitmix64, random bytes /
  corpus mutation / token soup (plus fixed shapes where a lane needs
  them), checked every iteration for panics, nondeterminism, rejections
  outside the fixed category set, and `internal error:` messages (each
  of which is a bug, not an outcome). Bare `fuzz` runs `lexer` at its
  deep default (200_000 iterations); every lane has its own deep default
  and its own smoke budget wired into `check`. Every lane's summary
  prints its **measured reach** into the pass it exists to test
  (plans/M9.md item PP) — modelled on the `async` lane — so a collapse
  to "clean about nothing" is visible in the output. `async`
  (plans/M7.md item Y) is the one lane that reaches `flowwir_lower` and
  the async codegen/image-layout path — it mutates the async/actor
  goldens, since no random byte stream ever spells a valid actor image.
- `cargo xtask ledger` — validate spec coverage, list gaps.
- `cargo xtask repro` / `cargo xtask diff-eval` — determinism and
  evaluator-vs-backend oracles; they fail closed until implemented and
  must never fake a pass.
- `cargo xtask profile` / `cargo xtask bench` — measurement, two lanes:
  compiler speed (**live**: `wrela --timings` prints per-phase wall time
  to stderr; `xtask bench compiler` times lex+parse over the full corpus
  in-process and locks the median against `bench/thresholds.toml`, wired
  into `check`) and guest speed (`bench guest`, replay-based, alive at
  M5 — still fails closed, like bare `bench` and `profile`). The only
  path to cleverness — in the compiler too — runs through them.

## Verification (cheap per item; expensive at close)

`cargo xtask check` is the gate — fmt, all tests, **all** goldens
(including HVF boots), corpus, fuzz smoke, ledger, repro lanes. It is
**too expensive to run after every plan item.** Active milestone plans
split verification; agents follow that split (or this default when the
plan is silent):

| Lane | When | What |
| --- | --- | --- |
| **Cheap (required per item)** | Before that item's commit | Unit tests + dumps for **new/changed** behavior only; no full golden suite; no deep fuzz; no `bench guest` |
| **Focused boot (only if the item claims HVF)** | Before that item's commit | Run **the one or two boot goldens the item names**, via `wrela` + `wrela-vmm` on those paths alone — not the whole `boot-*` corpus |
| **Expensive (close item only)** | Milestone close | Full `cargo xtask check`; deep fuzz when the plan requires it; `bench guest` deltas when the plan records them |

**Cheap recipes (use these, not `xtask check`, between items):**

```bash
# Unit filter (pick the crate the item touched)
cargo test -p wrela-machine --lib <filter>
cargo test -p wrela-compiler --lib <filter>
cargo test -p wrela-vmm --lib <filter>

# One non-boot golden: build wrela, dump/build the case, diff expected/
cargo build -q -p wrela-compiler --bin wrela
./target/debug/wrela dump --stage=<image|check|rtconfig|report|asm> \
  tests/golden/<case>/input.wr | diff -u tests/golden/<case>/expected/<stage>.txt -

# Docs-only items
cargo xtask corpus          # or corpus --sema when relevant
cargo xtask ledger

# Focused boot (only named cases the item claims)
cargo build -q -p wrela-compiler --bin wrela
# build image + boot with wrela-vmm for that case only (same shape the
# golden runner uses; do not loop every boot-*)
```

**Rules:**

1. Every item's **Cheap** oracle must fail if the new behavior is wrong
   and pass when it is right — a green unit filter that never touches
   the new code is not an oracle.
2. Do **not** run full `cargo xtask check` or deep fuzz on ordinary
   items. Drift in untouched goldens is the close item's job.
3. Items that **move** a large golden surface update the expected files
   in that commit and cheap-verify with dump/report on a representative
   sample plus unit tests; the full corpus is re-checked at close.
4. Claims of byte-identical transcripts are verified on the **named**
   control case in Cheap/Focused, not by replaying the entire suite.
5. The close item is not optional. A milestone is not COMPLETE until
   the expensive lane is green.

## Layout

- `crates/wrela-machine` — machine-contract types shared by compiler & VMM.
- `crates/wrela-compiler` — the whole pipeline, one crate (split only when
  build times demand, along artifact boundaries). Binary: `wrela`; every
  stage is reachable as `wrela dump --stage=<s> file.wr`.
- `crates/wrela-vmm` — KVM (Linux) + Hypervisor.framework (macOS) backends,
  device models, recorder. Consumes the image report as its whole config.
- `crates/xtask` — the harness above.
- `stdlib/` — wrela source: core + the machine's driver set.
- `docs/archive/` — the superseded draft spec; read-only history.

## Rules for working here

- Rigor lives in oracles (goldens, ledger, verifiers, differential runs),
  not in architecture. Do not add: traits with one implementation,
  generic-over-backend seams, plugin systems, or layers for their own
  sake. There is one machine and one backend — hardcode them.
- Dumbness is permanent, not v0-only. Cleverness is bought with a profile:
  an optimization needs a replayable workload's flame graph, a
  before/after on that same recording, and a regression lock — or it does
  not land, however obviously fast (ROADMAP.md, "cleverness budget").
- Every pipeline stage gets a stable text dump and golden coverage before
  it gets features.
- Fail closed: an unimplemented path errors loudly; it never approximates.
- New/changed doc rules get a ledger clause in the same commit.
- Prefer long obvious files over deep indirection; keep behavior local.
- Dependencies are liabilities; adding one needs a reason the ledger can't
  provide.
