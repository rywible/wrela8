# Wrela Pixels invariant ownership matrix

This P8R-close artifact traces each renderer invariant from authority through
measurement. Status cells use only `covered(artifact)`,
`not-applicable(reason)`, `planned(Pn)`, `blocking-gap(...)`, or
`accepted-deferred(...)`. The plan lint parses the table, validates unique row
IDs, and checks every covered artifact exists.

| ID | invariant | normative authority | compiler producer | guest verifier | Lean theorem | differential fixture | failure mapping | cost census entry |
|---|---|---|---|---|---|---|---|---|
| INV-PIX-LOWERING-001 | symbolic lowering accepts only the sealed field language | covered(docs/language/07-pixels.md) | covered(crates/wrela-compiler/src/pixels/symbolic.rs) | not-applicable(build-time-legality) | covered(formal/pixels/Pixels/TrustBoundary.lean) | covered(tests/golden/err-pixels-unsupported-op) | covered(docs/language/07-pixels.md) | not-applicable(no-guest-code-on-rejection) |
| INV-PIX-EVENTS-001 | emitted event families cover every supported structural boundary | covered(docs/language/07-pixels.md) | covered(crates/wrela-compiler/src/pixels/events.rs) | covered(stdlib/core/render_arrangement.wr) | covered(formal/pixels/Pixels/EventCover.lean) | covered(tests/golden/check-pixels-smooth-interior-root) | covered(docs/language/07-pixels.md) | covered(tests/census/p8-baseline/check-pixels-normal-moments.txt#fn.__wrela_pixels_p7_union_silhouette_coverage_at_slack) |
| INV-PIX-EXCLUSION-001 | feature exclusions never remove a potentially visible root | covered(docs/language/07-pixels.md) | covered(crates/wrela-compiler/src/pixels/exclusions.rs) | covered(stdlib/core/render_isolation.wr) | covered(formal/pixels/Pixels/RootIsolation.lean) | covered(tests/golden/check-pixels-thin-feature) | covered(docs/language/07-pixels.md) | covered(tests/census/p8-baseline/check-pixels-normal-moments.txt#fn.__wrela_pixels_p7_isolate_smooth_object) |
| INV-PIX-PROJECTIVE-001 | projective bounds enclose every admitted camera ray | covered(docs/language/07-pixels.md) | covered(crates/wrela-compiler/src/pixels/projective.rs) | covered(stdlib/core/render_certify.wr) | covered(formal/pixels/Pixels/Projective.lean) | covered(tests/golden/check-pixels-camera-inside) | covered(docs/language/07-pixels.md) | covered(tests/census/p8-baseline/check-pixels-normal-moments.txt#fn.__wrela_pixels_p7_collect_roots_box) |
| INV-PIX-CAPACITY-001 | every arena and fixed stack has a compiler-derived capacity | covered(docs/language/07-pixels.md) | covered(crates/wrela-compiler/src/pixels/capacities.rs) | covered(stdlib/core/render_arrangement.wr) | covered(formal/pixels/Pixels/Capacity.lean) | covered(tests/golden/err-pixels-capacity) | covered(docs/language/07-pixels.md) | covered(tests/census/p8-baseline/check-pixels-normal-moments.txt#fn.__wrela_pixels_p7_union_silhouette_coverage_at_slack) |
| INV-PIX-PLACEMENT-001 | placed renderer data is aligned and non-overlapping | covered(docs/language/06-machine.md) | covered(crates/wrela-compiler/src/layout.rs) | covered(stdlib/core/render_orchestrate.wr) | not-applicable(machine-layout-arithmetic) | covered(tests/golden/check-pixels-source-abi) | covered(docs/language/06-machine.md) | not-applicable(image-layout-outside-priced-hot-functions) |
| INV-PIX-SNAPSHOT-001 | frame snapshots validate count range rate and digest before use | covered(docs/language/07-pixels.md) | covered(crates/wrela-compiler/src/pixels/reference/snapshot.rs) | covered(stdlib/core/render_probe.wr) | not-applicable(byte-schema-validation) | covered(tests/golden/boot-pixels-frame-input) | covered(docs/language/07-pixels.md) | not-applicable(snapshot-validation-not-a-census-target) |
| INV-PIX-CERTIFY-001 | run certificates prove order and root obligations before raster | covered(docs/language/07-pixels.md) | covered(crates/wrela-compiler/src/pixels/reference/certificate.rs) | covered(stdlib/core/render_certify.wr) | covered(formal/pixels/Pixels/RunCertificate.lean) | covered(tests/golden/check-pixels-torus-roots) | covered(docs/language/07-pixels.md) | covered(tests/census/p8-baseline/check-pixels-normal-moments.txt#fn.__wrela_pixels_p7_collect_roots_box) |
| INV-PIX-COVERAGE-001 | arrangement coverage follows deterministic subdivision and owns each event once | covered(docs/language/07-pixels.md) | covered(crates/wrela-compiler/src/pixels/reference/coverage.rs) | covered(stdlib/core/render_arrangement.wr) | covered(formal/pixels/Pixels/Coverage.lean) | covered(tests/golden/check-pixels-simultaneous-event) | covered(docs/language/07-pixels.md) | covered(tests/census/p8-baseline/check-pixels-normal-moments.txt#fn.__wrela_pixels_p7_union_silhouette_coverage_at_slack) |
| INV-PIX-QUANTIZE-001 | output succeeds only when interval endpoints encode the same byte | covered(docs/language/07-pixels.md) | covered(crates/wrela-compiler/src/pixels/reference/raster.rs) | covered(stdlib/core/render_raster.wr) | covered(formal/pixels/Pixels/DisplayByte.lean) | covered(tests/golden/check-pixels-fixed-q-range) | covered(docs/language/07-pixels.md) | covered(tests/census/p8-baseline/check-pixels-normal-moments.txt#fn.__wrela_pixels_p8_raster_regular) |
| INV-PIX-DISPLAY-001 | only a certified complete frame may be submitted | covered(docs/language/07-pixels.md) | covered(crates/wrela-compiler/src/pixels/glue.rs) | covered(stdlib/core/render_orchestrate.wr) | not-applicable(device-protocol) | covered(tests/golden/boot-pixels-plane) | covered(docs/language/07-pixels.md) | not-applicable(device-protocol-outside-priced-hot-path) |
| INV-PIX-EVIDENCE-001 | presentation evidence commits to displayed bytes and guest state | covered(docs/language/07-pixels.md) | covered(crates/xtask/src/pixels_conformance.rs) | covered(stdlib/core/render_probe.wr) | not-applicable(host-evidence-schema) | covered(tests/golden/check-pixels-visibility-probe) | covered(docs/language/07-pixels.md) | not-applicable(host-conformance-not-guest-cost-model) |
| INV-PIX-REPLAY-001 | identical presentation evidence replays to an identical output decision | covered(docs/language/07-pixels.md) | covered(crates/wrela-vmm/src/record.rs) | not-applicable(host-replay) | not-applicable(host-replay) | covered(crates/wrela-vmm/src/replay.rs#replay_compares_all_exact_output_classes_and_names_first_divergence) | covered(docs/language/07-pixels.md) | not-applicable(host-conformance-not-guest-cost-model) |
| INV-PIX-FAILURE-001 | every unresolved proof capacity or device failure propagates without display | covered(docs/language/07-pixels.md) | covered(crates/wrela-compiler/src/pixels/worker_errors.rs) | covered(stdlib/core/render_orchestrate.wr) | not-applicable(error-routing) | covered(tests/golden/boot-pixels-renderer-unavailable) | covered(docs/language/07-pixels.md) | not-applicable(failure-routing-not-priced-hot-path) |

## P9 entry report

- Sealed decisions: D-P8R-01 through D-P8R-09 resolve once in the decision
  registry. P8R adds no public SIMD surface and no dependency.
- Final census baseline: `tests/census/p8-baseline/check-pixels-normal-moments.txt`.
  P8R.3 module seams and subdivision ownership precede the P8R.4 direct-FP
  and P8R.5 packet deltas; the final artifact supersedes intermediate counts.
  The P8-close-to-final emitted-code comparison and per-task explanations are
  in `tests/census/p8-baseline/p8r-final-deltas.md`.
- Scalar FP threshold: P8R.4c did not run; the recorded denominator and
  remaining transfer count are in `tests/census/p8-baseline/p8r4-direct-fp.md`.
- Packet closure: P9.4–P9.11 consumers are closed by
  `docs/designs/WRELA_PIXELS_PACKET_CONSUMER_MATRIX.md`.
- Cache behavior: cold, warm, and disabled results are identical; the measured
  timings are in `tests/census/p8-baseline/cache-parity.txt`.
- Topology: D-P8R-02 remains the normative three-worker flagship profile;
  F-P8R-01 owns reconciliation of the currently generated fourth worker.
- Blocking gaps: none.
- Accepted-deferred gaps: none. Planned P9/P12 work shown in prose is not a
  P8R invariant gap and does not weaken a matrix row.
