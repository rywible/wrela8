# P8R final emitted-code and census delta ledger

Basis: `check-pixels-normal-moments`. The before side is now the schema-4
P8R.2 census produced by the current bank-aware compiler and cost table from
the sealed pre-decomposition P8 sources at commit `44bcfcdc`, with only the
non-emitting region markers injected. The after side is the release-mode dump
and schema-4 final census produced by this tree. Word digests hash
the ordered lowercase instruction words only, so source paths and dump prose
cannot perturb them.

## [M] emitted hot-function comparison

| function | P8 frame | P8 words | P8 calls | P8 word SHA-256 | P8R frame | P8R words | P8R calls | P8R word SHA-256 |
|---|---:|---:|---:|---|---:|---:|---:|---|
| `__wrela_pixels_p7_collect_roots_box` | 29472 | 7239 | 131 | `018d73af051e0efaf5422926a7ad7f32d5bfe4eac49d27612e648cd4c814f4b8` | 29568 | 6810 | 55 | `8b34a11f2fab6e1ea7fb49ce405e58fecef4bc7f4177acb08a0a711139dd9335` |
| `__wrela_pixels_p7_isolate_smooth_object` | 10160 | 3649 | 42 | `e082f5cc79c3bb2c83544a1b1257c526163a2580799cb8182087af7ddb037a74` | 10160 | 3141 | 19 | `fea90de8ef5c5b5e0f5660b8c04e050ed34cf1d5d02c6481e370e1b999ed8616` |
| `__wrela_pixels_p7_union_silhouette_coverage_at_slack` | 46272 | 47041 | 170 | `d0de7745adff797b13254382e11a4c49eac4c5f8b21ac6dde18fd29c0f60b369` | 57744 | 51899 | 62 | `06e56cb37d047d8cc0989471fe831c8596e632587be9f22a530b586cbb9a7f7c` |
| `__wrela_pixels_p8_raster_regular` | 1584 | 1049 | 35 | `ea3980b121ded09713263d9dec39419967585ce26f26b75dad37eec585fb275e` | 1584 | 1049 | 10 | `7f2e0f5f96d2133743ae6be5e6970eb97061106a399228363491a7d90bd4f5d8` |

Every before and after row contains zero `blr` or register-target `br`
instructions [M]. The live census independently fails closed if a named hot
function gains either form.

`p8r2-pre-decomposition.txt` and the post-P8R artifact
`check-pixels-normal-moments.txt` both report the required scalar-prefix,
packet-loop, scalar-suffix, charge, coverage-entry, and coverage-cell-walk
regions separately, with the geometry and write families and sealed numeric
helpers in the same schema. Their headers pin exact image, kernel, and codegen
dump identities, and the lane reproduces each in two distinct build
directories.

## [S/M] explained deltas by task

- P8R.1 changes bank and opcode metadata only; the region marker has a unit
  test proving zero emitted words.
- P8R.2 adds only those non-emitting markers. Its dedicated schema-4 artifact
  is the bank-aware pre-decomposition authority used by the P8R.3 delta.
- P8R.3 moves source bodies into five modules and centralizes fixed-capacity
  subdivision cell/depth/proof/failure/owner/charge state and routes the six
  sealed event classes through one direct-call dispatcher. The cumulative
  final union walker grows its frame by 11,456 bytes and 14,763 words; the
  larger fixed-capacity cell carrier is shared by every path instead of being
  reimplemented by the general walker. Root collection grows its frame by 96
  bytes while deleting 249 words, smooth-object isolation deletes 521 words,
  and the raster word count is unchanged. Per-task phase artifacts retain the
  intermediate measurements; this table intentionally reports only the
  P8-close-to-final delta.
- P8R.4 replaces synchronous scalar-float GPR bridges with direct S/D loads
  and stores while retaining FlowWir's separate cache contract. The immutable
  commit-2 checkpoint was a regression, not a reduction: against P8R.3, union
  `coverage.entry` grew from 9,492 to 13,940 words, entry ALU words from 4,128
  to 8,106, total union words from 56,034 to 61,892, and union proxy cycles
  from 24,988 to 26,345 (+5.4%); `sqrt_scalar` grew from 166 to 176 cycles and
  `rsqrt_scalar` from 34 to 44. Four-byte S-form accesses reached only 16,380
  bytes of the roughly 57 KiB frame and every f32 store paid a second zeroing
  store, so address materialization dominated the apparent direct-FP win.
  The final integration closes that owned follow-up: each zero-extending
  S-form producer is stored once through its D alias (32,760-byte reach), and
  direct-FP frames cluster floating slots before large aggregates. The final
  union drops to 51,899 words and 23,270 proxy cycles; the exact transfer count
  and historical threshold decision remain recorded in `p8r4-direct-fp.md`.
- P8R.5 adds a closed packet operation substrate. The P8 renderer consumes
  only its pre-existing integer packet recurrence, so the new operation set is
  measured independently in `p8r5-packet.txt`; decoded-word tests pin every
  new instruction obligation.
- P8R.6 changes the host conformance loop only. P8R.7 changes documentation
  and validation only. Neither can alter guest A64.

Boot golden verification after the final structural/static regeneration
reported all 154 boot expectations byte-identical [M]. The conformance lane
remains the authority for truth, displayed-frame digests, and telemetry.

## [I] final modeled basis

```text
commit_2_threshold_functions = __wrela_pixels_p7_union_silhouette_coverage_at_slack,__wrela_pixels_p7_isolate_smooth_object,__wrela_pixels_p7_collect_roots_box,sqrt_scalar,rsqrt_scalar,raster_rsqrt
commit_2_proxy_cycle_denominator = 34504
final_recensus_proxy_cycle_denominator = 31417
```

The modelled totals are not hardware timing. The schema-4 final census derives
them from the measured opcode counts through the checked-in A76 table and
reports both whole-function scheduler scores and per-region
latency-weighted totals. The machine-checked six-function commit-2 denominator
is 34,504 proxy cycles: union 26,345, isolate 2,402, roots 5,469, sealed
`sqrt_scalar` 176, sealed `rsqrt_scalar` 44, and sealed `raster_rsqrt` 68.
The immutable P8R.4 artifact is the threshold authority. The independently
regenerated final artifact totals 31,417 cycles: union 23,270, isolate 2,407,
roots 5,461, sealed `sqrt_scalar` 175, sealed `rsqrt_scalar` 44, and sealed
`raster_rsqrt` 60. This is 3,087 cycles (8.9%) below the commit-2 checkpoint
and 1,909 cycles (5.7%) below the P8R.3 six-function total. Keeping both
structured values above makes a later final-tree change visible without
rewriting or obscuring the commit-2 decision basis.

## Integration-history deviation and adjacent changes

The implementation was assembled in one previously uncommitted working tree,
so the task-by-task and per-extraction commit boundaries requested by the plan
cannot be reconstructed or honestly claimed after the fact. The immutable
phase artifacts, exact content identities, and final byte-identity/gate
evidence remain auditable, but they are not represented as proof that every
intermediate commit independently passed. This recorded deviation is the
authority for the final integration history.

That working tree also contained two explicitly adjacent initiatives. Runtime
`Layout` rows were added to `ImageReport` and the static report expectations
were mechanically regenerated; those rows describe the linked runtime layout
and do not change truth or displayed image bytes. The separately implemented
VMM host-backend design program includes compiler-owned stage-1 setup; its
additional fail-closed address checks explain the final-tree ALU and digest
delta above, but are not P8R acceptance evidence. The accelerated build profiles, embedded golden
compiler, isolated-worker arena reuse, and content-addressed development
caches are development-loop infrastructure; they change neither guest code
semantics nor the required `cargo xtask verify` gate.
