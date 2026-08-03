# Pixels formal project

This project uses exactly Lean 4.30.0 and Mathlib 4.30.0. The toolchain,
dependency revision, and resolved dependency graph are pinned in
`lean-toolchain`, `lakefile.toml`, and `lake-manifest.json`. It is deliberately
outside the Cargo dependency graph and the shipped image.

Install `elan` as a user tool outside this checkout. The upstream installer,
run from any directory, writes under the user's `.elan` directory and does not
modify this repository:

```sh
curl --proto '=https' --tlsv1.2 -sSf \
    https://raw.githubusercontent.com/leanprover/elan/master/elan-init.sh | sh
elan toolchain install leanprover/lean4:v4.30.0
```

From `formal/pixels`, `lake exe cache get` is an optional local acceleration.
It downloads matching prebuilt Mathlib artifacts but does not change the
normative result. The normative project check is:

```sh
lake build
```

Repository commands wrap the project checks:

```sh
cargo xtask pixels-formal-scan
cargo xtask pixels-formal
```

`pixels-formal-scan` rejects project-defined admissions and forbidden proof
shortcuts after recognizing nested comments and strings. The required
`cargo xtask verify` gate runs this portable scan. `pixels-formal` runs the
normative `lake build` and compares normalized `#print axioms` output with
`EXPECTED_AXIOMS.txt`; `cargo xtask verify-deep` runs that complete command.

The formal project proves generic mathematics. It does not certify arbitrary
compiler output without the compiler-side proof-object checks: Rust must
construct and validate each concrete proof record, and generated guest
verifiers must check the encoded record. Lean is not invoked by an ordinary
Wrela build.
