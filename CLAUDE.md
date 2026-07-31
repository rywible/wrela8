# wrela

One designed machine, front to back: a language and compiler (no LLVM, no
external linker), a stdlib with the drivers, and a VMM implementing the
wrela machine. Flagship: wrela OS on Raspberry Pi 5 / 1 GiB — a
**fixed-function games console** appliance ([01 §1](docs/language/01-model.md)).
Titles ship inside the image; every update is a full image recompile
shipped as an A/B triple. The only thing that survives an update is the
device's storage, so any on-disk format is a product compatibility
surface, not an implementation detail.

## Ground truth, in order

1. `docs/language/` — normative. If code disagrees with the docs, the code
   is wrong. Doc changes are deliberate and human-reviewed.
2. `tests/golden/` — pinned artifact dumps. The golden diff is the review
   surface.
3. The code — disposable. Any crate should be rewritable from docs +
   tests alone. If that stops being true, the structure is wrong, not the
   rewrite.

`plans/M<n>.md` is the active milestone's ordered plan; sessions pick the
next item from it and do not relitigate it mid-milestone.

## Doctrine (decided once; sessions do not relitigate)

- **Compiler:** single-threaded batch pipeline. No query system, no
  incremental compilation, no interning, no arenas. Clone freely.
  Hand-written recursive-descent parser. Sema as whole-tree passes.
- **Determinism through dumbness:** `BTreeMap`/sorted `Vec` on every
  output-touching path, never `HashMap` iteration; no threads; no wall
  clock. Reproducibility passes by construction. (This governs the
  _compiler_. The xtask harness may run independent cases in parallel.)
- **Evaluator before backend:** the tree-walking comptime evaluator is the
  reference implementation of the semantics. No bytecode.
- **Backend: embarrassingly naive.** Fixed frames, spill everything, every
  check emitted, identity FlowWir "optimization". `diff-eval` makes it
  trustworthy.
- **VMM:** no QEMU. The machine has no GIC; a minimal wrela VMM is the
  runner we keep.
- **Diagnostics are the one place not to be dumb** — errors are pinned
  golden artifacts and a core feature.
- **No foreign code in an image, ever.** No dynamic loader, JIT, or fused
  foreign driver. Read a Linux driver as a _specification_ of the
  hardware's register contract, then write the driver in wrela under 03's
  rules — documentation that happens to be C, not fusion.

## Commands (no CI — these run locally, always)

- `cargo xtask check` — the gate: fmt, tests, golden, corpus, fuzz smoke,
  repro, bench locks. Run before calling a milestone (or any multi-item
  body of work) done — **not** after every plan item.
  `check --fast` drops the HVF and measurement lanes and names what it
  skipped; it is not the gate.
- `cargo xtask golden` — pinned dumps. Flags: `--update`, `--filter
  <substr>`, `--only-boot`, `--no-boot`, `--jobs N`, `--boot-jobs N`.
  `--update` rewrites expectations; review the diff before committing.
  `--filter` selects by substring and **fails closed when it matches
  nothing**.
  Non-booting cases run across every core; booting cases run at
  `--boot-jobs` (default 4) because guest transcripts are wall-clock
  sensitive and diverge above the performance-core count. `--jobs 1`
  forces the serial lane.
- `cargo xtask corpus [--sema]` — every fenced `wrela` block in the docs
  must lex and parse, then sema-classify against `tests/census.toml`. The
  docs are test inputs; drift is a failure.
- `cargo xtask stdlib-test` — comptime `@test` under `stdlib/tests/`.
- `cargo xtask fuzz <lane> [--iters N] [--seed S]` — deterministic in-tree
  fuzzer, no external engine. Lanes: `lexer parser sema eval lower async
  imports report`. Every iteration is checked for panics, nondeterminism,
  out-of-category rejections, and `internal error:` (each a bug, not an
  outcome). Every lane prints its **measured reach**, so a collapse to
  "clean about nothing" is visible. `async` is the one lane reaching
  `flowwir_lower` and image layout.
- `cargo xtask repro` / `diff-eval` — determinism and
  evaluator-vs-backend oracles. They fail closed; they never fake a pass.
- `cargo xtask cost-inventory` — every `CostRule` names an inventory row
  in `plans/M20.md`, and every row it names exists.
- `cargo xtask bench <compiler|build|guest>` / `profile` — measurement.
  The only path to cleverness runs through them.

## Verification (cheap per item; expensive at close)

`cargo xtask check` is the gate and is too expensive to run after every
plan item. Active milestone plans split verification; follow that split,
or this default when the plan is silent:

| Lane             | When                                           | What                                                                        |
| ---------------- | ---------------------------------------------- | --------------------------------------------------------------------------- |
| **Cheap**        | Before each item's commit                      | Unit tests + `golden --filter` for **new/changed** behavior only            |
| **Focused boot** | Before the commit, only if the item claims HVF | `golden --only-boot --filter <case>` on the one or two cases the item names |
| **Fast**         | Before merging a body of work                  | `cargo xtask check --fast`                                                  |
| **Expensive**    | Milestone close                                | Full `cargo xtask check`; deep fuzz and `bench guest` when the plan says so |

```bash
cargo test -p wrela-compiler --lib <filter>
cargo xtask golden --filter <case-substring>
cargo xtask golden --only-boot --filter <case-substring>
cargo xtask corpus            # docs-only items
cargo xtask check --fast      # a body of work, before its merge
```

**Choose the oracle before writing the code, not after.** Write the
expected output first, from the docs; the cheap lane is then whatever
pins that artifact. Every plan item names its oracle from this menu:

| What the item changes     | Its oracle                               |
| ------------------------- | ---------------------------------------- |
| A diagnostic              | a golden pinning the `error[…]` text     |
| A pipeline stage's output | that stage's dump golden                 |
| Codegen / layout          | `asm` / `image` / `img.hex` goldens      |
| Runtime / VMM behavior    | the **named** boot transcript            |
| Semantics                 | `diff-eval`                              |
| Lexer / parser surface    | `corpus` + that fuzz lane's reach number |
| A cost-model term         | a `cost.txt` golden                      |
| Compiler-visible perf     | a `bench` lane + a re-locked threshold   |

**A test's home is chosen by its cost, not its subject.** Sub-second →
`cargo test`. Seconds → an xtask lane in `check`. Minutes → a deep lane,
`#[ignore]`d, run by the close item and named in the plan. The default
`cargo test` lane has a locked wall-time budget in `bench/thresholds.toml`
(`[tests] workspace_suite_max_us`), enforced by `check`.

**Rules.** An item's cheap oracle must fail if the new behavior is wrong —
a green unit filter that never touches the new code is not an oracle.
Drift in untouched goldens is the close item's job. Items that move a
large golden surface update the expectations in that commit and
cheap-verify a representative sample. Claims of byte-identical transcripts
are verified on the **named** control case, not by replaying the suite.
The close item is not optional.

## Layout

- `crates/wrela-machine` — machine-contract types shared by compiler & VMM.
- `crates/wrela-compiler` — the whole pipeline, one crate. Binary:
  `wrela`; every stage reachable as `wrela dump --stage=<s> file.wr`.
- `crates/wrela-vmm` — KVM (Linux) + Hypervisor.framework (macOS)
  backends, device models, recorder. Consumes the image report as its
  whole config.
- `crates/xtask` — the harness above.
- `stdlib/` — wrela source: core + the machine's driver set.
- `tests/census.toml` — ratchet allowlists (locked surface counts, corpus
  sema pins). Update deliberately, in the commit that moves the surface.
- `docs/archive/` — the superseded draft spec; read-only history.

## Rules for working here

- Rigor lives in oracles (goldens, verifiers, differential runs), not in
  architecture. Do not add traits with one implementation,
  generic-over-backend seams, plugin systems, or layers for their own
  sake. There is one machine and one backend — hardcode them.
- Dumbness is permanent, not v0-only. **The cleverness budget:** an
  optimization needs a replayable workload's flame graph, a before/after
  on that same recording, and a regression lock — or it does not land,
  however obviously fast.
- Every pipeline stage gets a stable text dump and golden coverage before
  it gets features.
- Fail closed: an unimplemented path errors loudly; it never approximates.
- Prefer long obvious files over deep indirection; keep behavior local.
- Dependencies are liabilities; adding one needs a reason.
- A session that cannot reach green ends with `git restore`, not a
  "mostly done" tree.
