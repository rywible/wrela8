# Repository Guidelines

Wrela is a new language for building appliance OS images.
The flagship product is a game console that runs on Raspberry Pi 5.

## Project Structure & Module Organization

This is a Rust 2024 workspace. `crates/wrela-compiler` contains the compiler and the `wrela` CLI; major passes live in focused modules such as `syntax/`, `sema/`, `eval/`, `layout/`, and `cost/`. `crates/wrela-machine` defines shared machine types, `crates/wrela-vmm` provides the virtual machine monitor, `crates/wrela-fieldprobe` holds graphics experiments, and `crates/xtask` implements repository checks. Wrela standard-library sources are under `stdlib/core/`, with language-level tests in `stdlib/tests/`. Documentation lives in `docs/` including the wrela language spec; benchmark models and locked thresholds live in `bench/`. Compiler fixtures are organized as `tests/golden/<case>/input.wr` (or a `root` project) plus files under `expected/`.

## Coding Style & Naming Conventions

Wrela files use four-space indentation, lowercase module paths, and `.wr` extensions. Name golden cases by behavior: `check-*` for accepted programs, `err-*` for diagnostics, `boot-*` for runtime images, and `cost-*` for cost-model fixtures.

## Testing Guidelines

Place Rust unit tests beside implementation code in `#[cfg(test)]` modules. Every compiler behavior or diagnostic change should add or adjust a focused golden case. There is no numeric coverage target; regression coverage and deterministic expected output are the gate.

## Rules for working here

- No CI
- No new dependencies
- Prefer "dumb", maintainable code. Simple and direct.
- Every pipeline stage gets a stable text dump and golden coverage before
  it gets features.
- Fail closed.

## Verification

Agents use exactly one required gate:

- `cargo xtask verify` after every individual task and when closing a whole
  milestone. It includes the complete static and boot golden corpora plus
  focused signed HVF coverage, and therefore requires macOS/aarch64.

`cargo xtask verify-deep` is an optional maintainer diagnostic for the
expensive whole-corpus optimizer proofs, complete signed HVF vectors,
determinism reproduction, hardware oracles, and benchmarks. It is not a task
or milestone gate. Maintainers run it before cutting a release.

Other maintainer xtasks are useful for focused diagnosis, but are not
substitutes for `cargo xtask verify`. Fuzzing is a separate nightly-style
discovery lane: `cargo xtask fuzz all`. Promote every fuzz finding to
permanent unit or golden coverage before fixing it.
