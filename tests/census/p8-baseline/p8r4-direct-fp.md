# P8R.4 direct scalar-FP decision record

The canonical `f32` slot is eight bytes: IEEE-754 binary32 bits occupy the low
four bytes and the upper four bytes are zero. The immutable commit-2 artifact
used `str sN` followed by `str wzr`; its four-byte scaled immediate reach and
paired stores caused the regression recorded below. The final integration
closes that owned follow-up with one `str dN`: every admitted scalar-f32
producer defines an S register and therefore clears V[127:32], while no lane
insert or DUP producer can reach `store_fp_slot`. Full-slot
copies, aggregate copies, call arguments/returns, frame digesting, and debug
formatting can observe all eight bytes and are safe under this convention.

The synchronous MWIR location vocabulary is sealed as slot, GPR, FP/SIMD, or
immediate. Scalar FP operations use fixed caller-saved v0-v2 scratch
registers; S/D/Q aliases name one physical register and cannot hold distinct
live values. Calls retain the existing GPR-bit ABI and end FP residency.
FlowWir frames remain outside this P8R contract.

## [S/M] full-slot observer audit

The audit is over synchronous MachineWir frame slots; FlowWir uses its
separate cache contract and is excluded by the task scope. Source symbols and
observed widths are pinned here so a new full-slot path cannot be inferred
from prose:

| observer/path | source symbol | bytes read from an `f32` slot | result |
|---|---|---:|---|
| direct FP producer | `FnCtx::store_fp_slot` | writes one canonical 8-byte D alias | low IEEE bits, high word zero by the S-producer invariant |
| scalar copy/conversion | `Inst::Copy`, `emit_convert`, `FnCtx::copy_slot_to_slot` | 8 | canonical slot preserved |
| aggregate/field/enum construction | `Inst::MakeAggregate`, `Inst::Project`, `Inst::SetField`, `Inst::MakeEnum`, `Inst::EnumPayload` | 8 per `f32` field | canonical slot preserved |
| scalar call argument/return | `Inst::Call`, `Inst::Return`, `emit_prologue` | 8 through the current GPR-bit ABI | canonical slot crosses the boundary |
| arithmetic/comparison/conversion reload | `FnCtx::load_fp_slot`, `load_float_temp` | 4 | upper word is not observed |
| runtime scalar formatting | `emit_format_scalar` | 0 | runtime `f32` formatting is rejected, so it is not a frame observer |
| report/image digesting | `report::render`, image hashing | 0 | hashes linked artifacts, not synchronous frame memory |

`direct_fp_canonical_slot_reaches_compiled_observer_paths` compiles source that
feeds a direct-FP result through scalar assignment, aggregate construction,
field assignment/projection, enum construction/payload extraction, calls,
returns, and comparison. It requires each corresponding MachineWir operation,
checks the emitted observer widths, and executes the actual emitted D store
and adjacent LDR-X/STR-X pairs against dirty slot bytes. The existing call-ABI
test separately pins generated caller/callee transfers. Together they fail if
a four-byte producer can leave stale upper bytes visible to a compiled
full-slot observer.

```text
census_artifact = tests/census/p8-baseline/p8r4-direct-fp.txt
commit_2_fp_move_count = 10
commit_2_fp_move_cycles = 30
commit_2_proxy_cycle_denominator = 34504
threshold_functions = __wrela_pixels_p7_union_silhouette_coverage_at_slack,__wrela_pixels_p7_isolate_smooth_object,__wrela_pixels_p7_collect_roots_box,sqrt_scalar,rsqrt_scalar,raster_rsqrt
```

## [M] commit-2 census

The post-direct-memory census contains 10 remaining `fp_move` operations in
the P8R.4 threshold functions: eight in
`__wrela_pixels_p7_union_silhouette_coverage_at_slack` and two in
`__wrela_pixels_p7_collect_roots_box`. They materialize fixed conversion
bounds, not live-value GPR round trips.

Direct FP was not a whole-function win at this checkpoint. Relative to the
immutable P8R.3 artifact, union coverage increased from 9,492 to 13,940
`coverage.entry` words, from 4,128 to 8,106 ALU words, and from 24,988 to
26,345 modeled proxy cycles (+5.4%). Its total function words grew from about
56,034 to 61,892. `sqrt_scalar` grew from 166 to 176 modeled cycles and
`rsqrt_scalar` from 34 to 44. The cause was mechanical: four-byte S-form frame
accesses reached only 16,380 bytes, so much of the roughly 57 KiB union frame
materialized addresses, and every f32 result paid a second zeroing store. The
final-tree follow-up uses a single eight-byte D store (32,760-byte reach) and
clusters floating slots before large aggregates. The final mutable recensus,
not this immutable decision checkpoint, measures that correction: union words
fall to 51,868, union proxy cycles to 23,260, and the canonical six-function
denominator to 31,386.

## [I] commit-3 threshold

The commit-2 release-mode census gives a modeled-cycle denominator of 34,504
cycles:

- union silhouette coverage: 26,345
- isolate smooth object: 2,402
- collect roots box: 5,469
- sealed `sqrt_scalar`: 176
- sealed `rsqrt_scalar`: 44
- sealed `raster_rsqrt`: 68

The machine-checked lines above are cross-validated against the full schema-4
commit-2 artifact, including its source-tree and emitted-kernel identities.
At the checked-in A76 table's three modeled cycles per `fp_move`, the ten
remaining fixed-bound moves contribute a conservative 30-cycle numerator.
The census lane fails unless `30 / 34,504` remains below the sealed 10%
threshold, so a later table recalibration cannot silently preserve this
decision. The measured ratio is therefore far below 10%.
P8R.4c is therefore not entered. Call-crossing FP residency and spill policy
remain future work contingent on new census evidence.
