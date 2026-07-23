# wrela

One designed machine, front to back: a language and compiler (no LLVM, no
external linker), a stdlib with the drivers, and a VMM implementing the
wrela machine. Flagship: wrela OS on Raspberry Pi 5 / 1 GiB.

## Ground truth, in order

0. `ROADMAP.md` — the v0 doctrine (dumb-and-correct) and milestone ladder.
   Sessions pick work from it; they do not relitigate its decisions.
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
  calling anything done.
- `cargo xtask golden [--update]` — golden tests; `--update` rewrites
  expectations, then you review the diff and cite a ledger clause id in
  the commit.
- `cargo xtask corpus` — every ```wrela block in the docs must lex (and,
  from M1, parse). The docs are test inputs; drift is a failure.
- `cargo xtask ledger` — validate spec coverage, list gaps.
- `cargo xtask repro` / `cargo xtask diff-eval` — determinism and
  evaluator-vs-backend oracles; they fail closed until implemented and
  must never fake a pass.

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
- Every pipeline stage gets a stable text dump and golden coverage before
  it gets features.
- Fail closed: an unimplemented path errors loudly; it never approximates.
- New/changed doc rules get a ledger clause in the same commit.
- Prefer long obvious files over deep indirection; keep behavior local.
- Dependencies are liabilities; adding one needs a reason the ledger can't
  provide.
