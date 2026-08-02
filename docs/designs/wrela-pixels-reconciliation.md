# Wrela Pixels P-1 repository reconciliation

**Basis:** branch `pixels-mp-1`, commit
`e9f6bcb106fca3af8b8bb2fb57dd11fcdc4c4031`, 2026-08-02.

This inventory is the mechanical result of reconciling
`WRELA_PIXELS_COMPILER_IMPLEMENTATION_PLAN.md` with the repository before
Pixels implementation began. Paths that did not exist at the basis are
marked `new at P-1 basis` in the plan. Section 12.1 of the plan remains the
final ownership authority.

## Extension-point inventory

| concern | current extension point | P-1 decision |
|---|---|---|
| generic actor handles | `sema/types.rs::validate_actor_type`, `sema/bodies.rs::check_image_decl_method_intrinsic` | permit only `Actor[Renderer[P]]` minted by `Image.renderer[P](...).handle()`; retain the rejection for every other generic actor handle |
| image declarations | `eval/image.rs::{ImageDeclRef,ImageGraph}`, `eval/interp.rs::eval_intrinsic` | add an ordered renderer declaration kind; do not turn it into a source-authored ordinary actor |
| sealed image checks | `eval/image_checks.rs::check_sealed` | validate renderer labels, function roots, type agreement, display ownership, and the P-1 plane-only restriction before placement |
| image layout | `layout.rs`, `layout/rtdata.rs`, `layout/place.rs`, `placement.rs` | P-1 records the renderer envelope and logical generated actor in Pixels dumps; `frameprog` and `pixelsdata` physical sections remain owned by P5 |
| guest reachability | `lower.rs::guest_reachable_keys_closure`, generated rtconfig | renderer annotations force the generated P-1 renderer hook live; the boot fixture does not call a parallel display helper |
| stable dumps | CLI dispatch in `src/bin/wrela.rs`; image dump in `eval/image.rs` | add exactly `field-graph`, `frame-program`, and `render-layout` |
| AArch64 emission | `mwir.rs` → `codegen.rs` → `encode.rs` | P-1 uses scalar baseline instructions only |
| ISA feature ledger | `docs/language/06-machine.md` §1 and `wrela-machine` boot contract | current machine-v1 remains ARMv8.2-A + NEON/ASIMD; `FEAT_DotProd` is a planned P12 amendment, never an ambient probe in generated code |
| emitted-word audit | `emitted_a64_census.rs`, `cost/audit.rs`, `census.rs` | extend these only when P12 adds renderer instruction families |
| cycle proxy | `cost/*`, `bench/a76-pi5.toml`, `bench/thresholds.toml`, `bench/workloads.toml` | the P-1 skeleton reports deterministic numeric code-byte, memory-byte, and instruction baselines from the actual emitter; renderer admission remains P12/P13 |
| report | `report.rs`, `wrela-machine/src/report.rs` | P-1 keeps production report parsing stable; renderer-specific stable evidence is in the three Pixels dumps |
| VMM output/replay | `wrela-vmm/src/boot.rs`, `record.rs`, portable device models in `devices.rs` | add a portable display queue, headless digest sink, and frame-output replay choice |
| xtask | command dispatch and verify lanes in `crates/xtask/src/main.rs` | add plan lint, formal scan/build, and their focused checks without a Cargo dependency |

## Machine-v1 and `FEAT_DotProd`

The flagship Raspberry Pi 5 Cortex-A76 implements the Arm dot-product
extension; Arm's Cortex-A comparison table lists INT8 Dot Product for
Cortex-A76:

<https://developer.arm.com/-/media/Arm%20Developer%20Community/PDF/Cortex-A%20R%20M%20datasheets/Arm%20Cortex-A%20Comparison%20Table_v4.pdf>

The P-1 development host reports both NEON and `FEAT_DotProd`
(`hw.optional.neon=1`, `hw.optional.arm.FEAT_DotProd=1`). Hypervisor.framework
provides Arm feature-register access on Apple silicon:

<https://developer.apple.com/documentation/hypervisor/apple-silicon>

The phrase “any Linux/KVM aarch64 box” in the current machine chapter is not
by itself a dot-product guarantee. P12 therefore must make DotProd part of
the versioned machine baseline and have both VMM backends reject a host that
cannot expose it. There is still one image and no runtime dispatch or scalar
fallback. This closes the host-class question without claiming that all
pre-P12 generic AArch64 hosts already conform to the amended baseline.

## Formal toolchain

`formal/pixels/lean-toolchain` pins Lean 4.30.0 and `lakefile.toml` pins the
matching Mathlib tag. The project is outside Cargo and the shipped image.
`cargo xtask pixels-formal-scan` is the portable escape-token check.
`cargo xtask pixels-formal` builds the pinned project and checks its axiom
output, and the required `cargo xtask verify` gate runs that full command.
The P-1 environment has the Lean 4.30.0 toolchain installed and the Mathlib
cache is populated by the focused formal command.

## Task and path audit

The plan contains 154 unique task headings. P-1.1 converted all P0–P13 tasks
to the required nine-field executor schema:

`Requires`, `Produces`, `Files`, `Contract/dump delta`, `Work`, `Tests`,
`Focused checks`, `Repository gate`, and `Stop conditions`.

`cargo xtask pixels-plan-lint` enforces:

- unique task IDs and all nine fields in each task;
- exact task-heading order against the §14 commit-order list;
- future paths marked at the P-1 basis;
- the canonical three dump names;
- the canonical `KERNELS.txt` and `EXPECTED_AXIOMS.txt` manifest names;
- the canonical renderer declaration labels;
- the Wrela display placements, offsets, and frame size against the Rust
  machine constants;
- rejection of the stale `crates/wrela-machine/src/display.rs` owner;
- one definition each for `RenderError`, `FieldKind`, `Iv32`, and the
  80-byte `FrameProgramHeaderV1`.

The task schema is intentionally mechanical. Task-local prose still carries
the algorithm and acceptance detail; the added fields make prerequisites,
dump deltas, focused checks, and stop boundaries machine-auditable.
