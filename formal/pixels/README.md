# Pixels formal project

This pinned Lean project proves generic mathematical facts used by the Wrela
Pixels compiler and runtime. It is deliberately outside the Cargo dependency
graph and the shipped image.

Run:

```text
cargo xtask pixels-formal-scan
cargo xtask pixels-formal
```

The first command rejects project-defined admissions and forbidden proof
shortcuts after recognizing nested comments and strings. The required
`cargo xtask verify` gate runs this portable scan. The second command builds
with the pinned Lean 4.30.0/Mathlib environment and compares normalized
`#print axioms` output with `EXPECTED_AXIOMS.txt`; `cargo xtask verify-deep`
runs that complete formal command.

Rust constructs concrete proof records; Lean is not invoked by an ordinary
Wrela build.
