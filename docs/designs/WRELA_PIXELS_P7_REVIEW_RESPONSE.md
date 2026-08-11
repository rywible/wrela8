# P7 review response — remediated findings and historical record

## 2026-08-09 close-review addendum

The material below records the earlier review wave and is retained as
diagnostic history. The later close review is fully remediated in the current
tree:

- all affected runtime goldens and `tests/pixels_truth/p7-visibility.txt` are
  regenerated only from green runs;
- analytic coverage declines a centre-miss pixel unless every covering
  boundary is classified and provably non-crossing, and it declines on a
  second crossing regardless of whether the two bytes happen to agree;
- projected-union refinement prefilters relevant events once, scopes torus
  magnitude caches by event, and reuses side occupancy only for a genuinely
  single-curve pixel;
- `run_job` preflight failures use invariant codes and the single-source lint
  scans both failure helpers and direct worker-error calls;
- the pixels green-transcript guard parses the passing count and requires at
  least one executed test;
- sema fences `__wrela_*` and `RendererWorker*` from ordinary user modules,
  closing spelling-dispatched lowering and injected-prelude shadowing;
- conformance uses the last frame-digest marker consistently and the synthetic
  structural control no longer asserts an uncomputed legacy-lattice premise;
- instrumented dump hooks in the one-core plane and tangent fixtures are
  reachable, with a source lint preventing a direct return before any dump;
- one/four-worker proof equality samples the same final-tile plane run in both
  layouts, and reports telemetry and evidence drift separately;
- predicate witnesses evaluate all four corners directly and torus side
  classification uses the evaluator residual.
- affine-in-q validity eliminants whose leading sign is not global are oriented
  only after a four-corner per-cell strict-sign proof; sibling events are
  suppressed only after the representative region is actually classified.
- predicate branches carry compact metadata into one shared evaluator, avoiding
  per-event cloning of the checked polynomial accessor and preserving the 2 MiB
  branch-region contract on the large field-ops control.
- instrumented run evidence always records row `y`, selects record zero only
  after a same-identity centre/q recheck, authorizes sampled normals only
  through an exact certified normal model, and is decoded fail-closed before
  the independent q/identity/normal checks (or a resolved centre miss for
  background evidence); the full frame is scored with inset probes and a whole-frustum
  interval proof before any five-miss pixel is called background.
- the host root oracle preserves proved endpoint orientation across adjacent
  conservative cells and partitions parameter-independent finite-repeat ties,
  eliminating dependency-only roots without weakening unresolved regions.

The authoritative as-built deviations and closure status live in
`WRELA_PIXELS_COMPILER_IMPLEMENTATION_PLAN.md` deviations 13 and 17–23.

The final torus closure adds a canonical standalone analytic tier with
compensated-f32 coefficient intervals, outward bivariate Horner evaluation,
P-Q point/cell classification, a three-rung precision ladder, and an
error-carrying affine sample cache. Its coefficient, complete-discriminant,
and cell-classification enclosures have independent f64 containment tests.
The real `check-pixels-torus-roots` guest run completes with `2 passed, 0
failed`; its transcript is accepted only through the green-golden policy.
The corresponding thin-feature closure keeps the original refinement ledger
and caps and completes `check-pixels-thin-feature` with `2 passed, 0 failed`.

## Historical review response (superseded)

Everything below this heading records the state and decisions of the earlier
review wave. Headings such as “Not attempted” and “Gate status” are preserved
for traceability; they do not describe open work in the current tree. Their
closures are the addendum above and the authoritative as-built deviations.

Response to the P7 close review. Ordered by the review's own numbering. Every
"fixed" item below was verified by execution against the guest fixtures under
the VMM, not by inspection alone.

The single most important verification result: after fix 1, the plane fixture's
output is **byte-identical to the recorded golden** (`tests/golden/boot-pixels-plane/expected/test.txt`),
which confirms the review's hypothesis that the goldens predate the regression
and are themselves sound. That identity was re-checked after every subsequent
change in this pass and still holds.

## Fixed

### 1. CRITICAL — swapped `P7RootList` count initializations
`certify_root_list_at_u` now initializes `count=0` (rebuilds the filtered list
in place from index 0; safe because every read of `roots[i]` precedes the write
at `destination <= i`). `isolate_smooth_object` now initializes
`count=roots.count` (continues appending after roots already collected for
earlier features in the shared workspace).

Additional fix beyond the review: `isolate_smooth_object`'s two run-merge
guards tested `result.count > 0` and merged into `result.count - 1`. With the
corrected initialization that could reach back into a *different* feature's
root. Both guards are now scoped to `result.count > appended_base`.

Verified: plane fixture goes from `FAILED assertion failed: valid plane frame
renders` to `2 passed, 0 failed`, byte-identical to the golden.

### 3. HIGH — row-level exclusion unsound for smooth-object members
`row_candidates` now looks up the feature's owning object (feature operand 0)
and **skips row-level exclusion entirely for members of composed objects**.
This is the review's second suggested correction, chosen over generating a
support-shifted exclusion polynomial because it needs no new codegen and is
unconditionally conservative. Support pruning still happens per-object later in
`feature_support_q_span`.

The rationale is recorded in the source: exclusion proves `{leaf = 0}` empty,
but completeness for a smooth member needs `{leaf <= support_budget}` empty,
and a smooth-union root lives where every leaf is strictly positive.

Verified: `check-pixels-smooth-interior-root` still `4 passed, 0 failed` — and
now for the right reason rather than because the interval Horner happened to be
too weak to prove positivity.

### 4. HIGH — `select_visibility` overlap groups
Both defects fixed:

* **Parity.** The group now tallies crossings per distinct object and XORs only
  objects with an odd crossing count, instead of XORing each distinct object
  once. An even number of crossings of one object (a near-tangent entry/exit
  pair whose tubes overlap after the ±8192-raw-unit widening) no longer leaves
  a phantom inside-bit.
* **Skip condition.** The `all_singletons_transition` heuristic is replaced by
  an exact arrangement check. Because toggles commute, the states reachable as
  a prefix of *some* interleaving are exactly the `2^object_count` parity
  combinations over the distinct objects in the group, so the group is skipped
  only when every one of those subsets evaluates the composite to `occupied`.
  Groups with more than 6 distinct objects return error 4 to the rebuild ladder
  rather than being guessed at.

The trailing `occupied = after.value` was dropped: the parity state is one of
the enumerated subsets, so it is `occupied` by construction.

### 5. MEDIUM — certificate-exhaustion misclassified as `InternalInvariant`
`__worker_error` now maps codes 4, 42, 43, 44, 45, 47, 48, 49, 50, 61–69 and
100 to `CertificateExhausted(tile)`, leaving 41, 46 and the 5-family on the
`InternalInvariant` path. The taxonomy is documented as a comment block at the
mapping site, which is the single place it can now be read off.

Verified end-to-end: `check-pixels-hard-csg` previously traced
`0xe4000001` (= `InternalInvariant`, kind `0x80000000 | 100<<24 | tile 1`) and
now traces `0x0000000300000001` (= `CertificateExhausted(tile 1)`). P7.10 is
satisfiable for these paths.

**`order_margin` scoping.** The global `order_margin()` method is deleted. The
margin is now accumulated inside the sweep, over only the gaps it actually
traverses: on entering each group, the gap to the previously-skipped group's
lower frontier is folded in. Tubes overlapping *behind* the visible surface can
no longer zero the front sample's margin and turn a certifiable pixel into
worker error 12. Overlaps *within* a group are likewise excluded, which is
sound precisely because of the fix in 4 — a group is only passed through once
every interleaving is proven to leave the composite constant, which makes the
post-group state independent of tube order.

### 10. LOW — `isolate_power_front` discarded an accepted front root
The walk's result is widened from `[i64; 4]` to `[i64; 6]`:
`[status, found_flag, found_lo, found_hi, unresolved_lo, unresolved_hi]`. On an
unresolved cell the walk now returns any root it had already certified
alongside the unresolved marker. The caller emits that root through the normal
validity filter before enclosing the remainder in a corridor.

This is sound because a cell is skipped by the bisection only when its Horner
range excludes zero, so every cell between the accepted root and the unresolved
cell is *proven* root-free; the guard `isolated[2] >= isolated[5]` keeps the
root only when it lies entirely above the corridor.

Implementation note: the array is built inline at each return site via an
`i64` mirror of the `found` flag. A private helper function was tried first and
rejected — bodies evaluated in the `core.__image_pixels` module context cannot
reach private helpers declared in `core.render`.

### 11. LOW — maintainability
* Deleted 372 lines of dead generated-source from `glue.rs`: the `filter_source`
  string, `let _ = filter_source;`, and the `if false { … }` block containing an
  obsolete f32 exclusion/tube pipeline with a different (flat 128-raw-unit)
  error allowance.
* Removed the `empty` vestige in `support_q_span` and replaced the empty
  `pass` arm with a comment explaining why the straddling-direction axis
  contributes no bound and admits no slab-miss detection.
* Deleted the now-unused `P7RootList::order_margin` method.
* Renamed the oracle's depth parameter and interval field from `q` to `t`
  (`near_q`/`far_q` → `near_t`/`far_t`, `OracleRoot::q`/`VisibilityScore::q` →
  `::t`) and updated `conformance.rs`, which inverts via `1/t`. 125 reference
  unit tests pass.
* The error taxonomy is documented at the `__worker_error` mapping (see 5).

### 6 (MEDIUM) — proposals change displayed bytes
Proposals no longer seed the *first* attempted span width. `proposed_end` is
still computed, so revalidation matching and its telemetry stay live, but it no
longer decides `width` — the first attempt is always the maximal fresh width,
identical to the proposals-disabled path.

A disabled/enabled digest-equality assertion is added to
`check-pixels-close-depth`, which the review correctly identifies as the missing
coverage: it is a genuine silhouette (two overlapping spheres) that actually
exercises the proposal path (180 revalidated proposals, 24 event corridors),
unlike `boot-pixels-plane-one-core` where every row partitions identically. The
digest is compared against the enabled render rather than a pinned constant, so
the assertion survives the scene moving.

**Measurement that refines the review's claim.** The review states that frame
bytes differ between proposals on/off. That is not borne out: with the old
seeding restored, `check-pixels-close-depth` produces *byte-identical* output in
both modes (the new assertion passes under the old code too). What did differ is
`certificate_runs` — 206 with proposal-seeded widths versus 136 with fresh
maximal widths — and the run-length telemetry derived from it. The debug alpha
turns out not to be partition-sensitive for this scene, because the per-pixel
q interval, not the span width, dominates the width class.

So P7.11's "displayed output identical" already held; what was violated was
telemetry invariance. The fix makes `certificate_runs` mode-invariant too. The
remaining divergence surface — retry-tier widths after an x-split failure — is
unchanged, and no current fixture exercises it.

### 8 (partial) — the `1/width` secant term
Not landed; see "Attempted and reverted" below.

## Attempted and reverted, with evidence

These are changes that implement the review's stated correction but were backed
out because they regress working scenes. Each is left in the source as a
documented `NOTE (deviation)` / `KNOWN GAP` comment at the exact site.

### 9 — analytic-coverage pixels with a missed centre sample
The review's correction is to resolve the covering identity or fall through to
the quadtree. Both stopgaps were implemented and measured:

* Falling through to the quadtree (`integrated_coverage = -1`) makes
  `check-pixels-smooth-csg` run past a 10-minute wall clock.
* Failing closed with a new exhaustion code (50) turns `smooth-csg`,
  `close-depth` and `tangent` from `2 passed, 0 failed` into `1 passed, 1 failed`.

The `integrated_coverage > 0 && !selected_hit` case is therefore **common** in
these scenes, not the corner case the review models it as. Trading three
working scenes for a hypothesised silent-wrong-pixel is a net regression, so
the behaviour is unchanged and the gap is documented in place. The real fix is
to have the coverage integrand carry side identities so the covering identity
can be resolved; code 50 is reserved and already classified.

### 5 (coverage 100 fall-through)
Letting code 100 fall through to the quadtree rather than failing closed is
what the plan ultimately wants, but the quadtree exhausts its 2M cell budget on
the hard-CSG, close-depth and tangent scenes. The *classification* fix (100 →
`CertificateExhausted`) is what finding 5 actually asks for and is landed; the
control flow stays fail-closed.

### 8 — `gradient = first_i * lower[0]`
The review reads this as a status-flag multiplication to be replaced with a
plain sum. It is not a bug: `lower[0]` is an `f32` array element and the
multiplication is an **undocumented f32 type-ascription idiom**. Bare float
literals default to `f64`, so `gradient = {} + {} + {}` fails typecheck
(`expected f64, found f32` at the live rtconfig re-check), as does an explicit
`gradient: f32 = …` annotation in that position. `lower[0]` is guaranteed `1.0`
by the preceding `if lower[0] != 1.0: return` guard, so the current code is
correct. Reverted and left as-is; the idiom deserves a comment or a language
affordance rather than a mechanical rewrite.

The `1/width`-scaled secant allowance was implemented alongside and reverted
with it; it should be re-attempted independently once the typing above is
settled, since the underlying observation (secant evaluation error is amplified
by `1/width` and the `arithmetic` term does not scale) is correct.

## Historical items not attempted in that wave (later closed)

Left untouched, with reasons. None of these are hidden — all are load-bearing
for the review's own conclusion that P7 is not closeable.

* **2 (HIGH) — box certification never excludes additional roots of represented
  features.** This is the architectural finding: point-collect-then-box-certify
  structurally cannot see roots that appear off-centre, because the exclusion
  decisions happen on the centre ray and leave no record. The correction (run
  the isolation walk in box-interval arithmetic, or record and re-verify the
  point-stage exclusion cells) is a rewrite of the isolation core. Landing it
  without a per-pixel conformance gate to validate against would be
  unverifiable, and the gate itself is finding 7.
* **7 (MEDIUM) — conformance gate far weaker than P7.14/P7.15.** Needs
  `conformance::run` to decode the guest debug framebuffer per fixture and
  score per pixel against `oracle::isolate_all_roots`/`first_boundary`. This is
  the highest-leverage remaining item: it is what would have caught 2–4, and
  nothing else can validate a fix to 2.
* **8 (MEDIUM) — compile-time forward error analysis.** No per-renderer error
  budget is computed from sealed `@range`/camera bounds.
* **12 — coverage gaps.** P7.4's static-vs-runtime exclusion counter
  distinction and P7.7's multiple-event-IDs-per-corridor in *guest* records
  (present only host-side) are still missing.

## Additional finding not in the review

Four fixtures have their **failure recorded as the expected golden**:

```
check-pixels-displace          FAILED  valid displaced surface renders from scratch
check-pixels-enclosed-feature  FAILED  enclosed feature is not suppressed by prior samples
check-pixels-hard-csg          FAILED  hard-CSG visibility renders from scratch
check-pixels-material-edge     FAILED  material edge is charged to event corridors
```

These are P7 acceptance criteria whose failing state was regenerated into
`expected/test.txt` rather than being fixed. They are not stale — they match the
current tree — so `cargo xtask golden` will pass over them silently. All four
are scenes that findings 2, 4 and 9 predict would break. They should be treated
as open P7 blockers, and the staleness/expectation guard the review proposes
(hashing stdlib and glue inputs into golden headers) would not catch them
either — a "record the failure" expectation needs a separate lint that rejects
`FAILED` lines in a golden that is not an `err-*` fixture.

## Historical gate status (superseded)

`cargo xtask verify` cannot pass yet: the `check.txt` goldens are stale with
respect to signature changes made in this branch before this pass (functions
such as `torus_event_magnitudes`, `quartic_discriminant`,
`point_union_occupancy` and the changed arities of
`event_polynomial_uv2_bounds`, `union_silhouette_coverage`,
`silhouette_coverage`), and this pass adds further signature churn
(`isolate_power_front` arity, removal of `order_margin`). Regenerating those is
mechanical but should happen only once the four fixtures above are decided,
since regenerating now would re-record their failures a second time.
