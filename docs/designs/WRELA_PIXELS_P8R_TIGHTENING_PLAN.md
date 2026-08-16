# Wrela Pixels P8R — reconciliation and tightening plan

This document is an interstitial milestone plan executed between the P8 close
(commit `44bcfcdc`) and Task P9.1. It follows the execution rules of the
[implementation plan](WRELA_PIXELS_COMPILER_IMPLEMENTATION_PLAN.md) §10.0 and
does not override the [normative contract](../language/07-pixels.md). A
semantic change lands in the contract chapter first and reconciles this plan in
the same change. The milestone is named P8R (reconciliation) because the
implementation plan already owns the name Task P8.5. Tasks are numbered in
execution order per §10.0 rule 1; the ordering is deliberate — contracts, then
measurement instruments, then a frozen baseline, then structure, then codegen,
so every optimization claim is a diff against a stable reference.

P8R adds no renderer feature and changes no displayed byte. Its result is that
P9 begins on a substrate whose contracts are internally consistent, whose hot
paths are measured against a bank-aware cost model rather than estimated,
whose scalar float lowering has a defined storage contract and no longer
transfers through general registers per operation, whose packet substrate is
sealed with complete numerical semantics, whose largest runtime source files
are decomposed along audited seams, and whose development loop re-verifies
only what changed.

## Evidence classes and citation rules

Every factual claim or result in P8R work products carries one of these
labels. Normative requirements (task instructions, contracts, gates) are not
labeled; labels apply to claims of fact and to results.

```text
[S] observed source fact       cited as commit:path:symbol (line numbers are
                               unstable and never the primary key)
[M] measured result            produced by a census, telemetry, or timed lane,
                               persisted as a checked-in report artifact —
                               never only as commit-message text
[I] model inference            derived from [S]/[M] through a stated model
[D] product decision           sealed with a stable decision ID (D-P8R-nn) in
                               the normative document that owns it
[H] research hypothesis        carries an explicit falsifier and kill condition
```

A factual claim without a label is not admissible as a P8R exit criterion.

## Budget framing (all [I] until the P13 proxy or hardware evidence exists)

```text
guest workers (pending D-P8R-02)         3 × Cortex-A76 @ 2.4 GHz
raw capacity at 3 workers                7.2 G cycles/s
1080p60 pixel rate                       124.4 M px/s
raw ceiling                              ~57.9 cycles/px
P12.8 admission margin (sealed)          at most 80% of modeled per-core budget
usable ideal ceiling at 3 workers        ~46.3 cycles/px
usable ideal ceiling at 4 workers        ~61.7 cycles/px
```

Both ceilings assume perfect balancing and exclude orchestration, display,
and memory-traffic effects; they bound ambition, they do not admit anything.
P8R calibrates a cost model; it does not convert modeled timing into physical
Raspberry Pi timing, and no P8R artifact may claim otherwise. Peak-FLOP
comparisons against unrelated hardware and workloads are not used in this
plan. Three pinned guest vCPUs are a deployment target, not current
functionality: the VMM's physical-Pi pinning is roadmap work, and the sealed
profile of D-P8R-02 must state the current proxy honestly.

## Closed decisions for P8R

These are fixed inputs to the tasks below, sealed as D-P8R-01 through
D-P8R-08 by Task P8R.0. Reopening one is a stop condition.

1. **D-P8R-01** The milestone is named P8R. It contains no shading, no
   temporal reuse, and no new field operations.
2. **D-P8R-02** The flagship worker topology is sealed as a named product
   profile in P8R.0 (target: three pinned guest rendering workers, one host
   core; four-worker fixtures remain functional fixtures). The cycle-proxy
   core count follows the profile.
3. **D-P8R-03** The camera-cut budget is not relaxed. The full-sweep contract
   and the P13.4 60 Hz cut/whip requirement stand unchanged.
4. **D-P8R-04** No implicit FMA contraction, ever. Fused arithmetic enters
   only as an explicit sealed operation with its own scalar reference.
5. **D-P8R-05** fp16 lanes are not added to the machine ISA surface in P8R.
   Narrow precision may later enter only as as-if candidate arithmetic whose
   conservative residual proves the stored output byte unchanged, under
   contract §0.1(8); that work is P9-era at the earliest and telemetry-gated.
6. **D-P8R-06** No external Cargo dependencies, per the standing dependency
   policy. Kernel search tooling (appendix) must be in-house.
7. **D-P8R-07** The P8R packet substrate is compiler-internal and
   renderer-internal: sealed MWIR operations surfaced only through the
   existing generated/sealed renderer intrinsic pattern
   (`pixels_i32x4_backend_add`-style backend helpers). It does not implement,
   extend, or expose the public `05-library.md` §8.1 SIMD types; those remain
   P12.3's deliverable, and P8R.0 records the reconciliation note in the
   canonical plan so P12.3 re-plates these operations onto the public types
   deliberately rather than discovering them.
8. **D-P8R-08** Formal claims use the sealed phrasing: Lean proves generic
   kernel mathematics; build-time Rust constructs concrete facts; generated
   guest verifiers check encoded records. No document claims an end-to-end
   verified compiler or renderer. Candidate/authority separation is
   untouched: every technique this plan or its appendix introduces proposes;
   conservative dyadic verification accepts.

## Task P8R.0 — contract and decision ledger

**Requires:** the P8 close gate.

**Produces:** One authoritative answer with a stable decision ID for every
known spec/code divergence, sealed before any code change in this milestone.

**Files:**

```text
docs/language/06-machine.md
docs/language/07-pixels.md
docs/designs/WRELA_PIXELS_COMPILER_IMPLEMENTATION_PLAN.md
docs/designs/WRELA_PIXELS_P8R_TIGHTENING_PLAN.md
crates/xtask/src/pixels_plan_lint.rs
```

**Contract/dump delta:** Documentation plus the plan-lint lane. No compiler,
stdlib, or fixture bytes change in this task.

**Work:**

Seal D-P8R-01 through D-P8R-08 above, each recorded once in the normative
document that owns it, with a dated decision line. Additionally:

- Canonical chain link [D]: P8R is currently disconnected from the canonical
  task chain — P9.1's prerequisite is only the preceding milestone close.
  Amend the canonical plan so Task P9.1 explicitly requires Task P8R.7, and
  add a pointer at the P8 close / P9 entry seam to this document.
- Plan-lint extension: the lint currently assumes exactly 154 canonical task
  headings and recognizes only `new at P-1 basis` [S]. Extend it to: know
  this document, lint the P8R task schema (the §10.0 required sections),
  verify the P9.1→P8R.7 link, accept `new at P8 basis` in file inventories,
  and implement the decision-ID registry check (every D-P8R-nn referenced
  anywhere resolves to exactly one definition). The P8R.7 doc-consistency
  lane builds on this check; until P8R.7, artifact-existence checking for
  the invariant matrix is explicitly manual.

- `RTDATA_BASE` [S→D]: `06-machine.md` states `IMAGE_BASE + 2 MiB`; the
  machine layout constant and the implementation plan's packing-window text
  both state 4 MiB. Determine the intended value, correct the losing
  document, and record why as D-P8R-09.
- rtdata padding claim [verify-or-discharge]: a prior working-session record
  asserts a deferred image-size padding fix; no repository artifact currently
  evidences it. Verify against the corrected D-P8R-09 contract. If real,
  record it as a named follow-up task with its own file inventory — it is
  not part of any P8R task. If not reproducible, discharge it explicitly so
  it stops circulating.
- Topology status [S/D]: record, as part of D-P8R-02, what the current tree
  actually provides (generated four-worker renderer; HVF-hosted development
  VMM; no physical-Pi pinning) versus the deployment target, and which
  milestone owns each gap. If reconciling the generated worker count exceeds
  documentation scope, record the generation change as a named follow-up
  with an owner.
- Debt ledger corrections [S]: record that `MaterialIntrinsic` classifies all
  four constructors and that `.to[f64]` produces `SymValue::F64`, striking
  both from open-debt lists; record the two product-tier optimization
  exceptions and the residual `ends_with(".subtract")` classification site
  as the remaining audited debt, each with an owner.
- Formal phrasing [D-P8R-08]: sweep the named documents for over-claims of
  formal scope and align them.
- Record the P12.3 reconciliation note required by D-P8R-07 in the canonical
  plan's P12.3 task text.

**Tests:**

- A repository-wide grep for the losing `RTDATA_BASE` value finds only
  historical/changelog text.
- The extended plan lint passes: P8R schema, the P9.1→P8R.7 requirement,
  `new at P8 basis` acceptance, and the decision-ID registry (every
  D-P8R-nn resolves exactly once), and fails correctly under an injected
  duplicate or dangling ID.

**Focused checks:** Run the focused tests, fixtures, dumps, and commands named
by this task before the repository gate.

**Repository gate:**

```text
cargo xtask verify
```

**Stop conditions:** Stop if resolving a divergence requires changing runtime
behavior or image bytes; that resolution is a named follow-up task with its
own gate.

**Commit**

```text
pixels P8R.0: seal the P8R decision ledger and reconcile contracts
```

## P8R.0 record — sealed decisions, follow-ups, and discharged claims

This section is the work product of Task P8R.0. The two decisions this plan
itself owns are defined here; the rest are defined in the normative document
that owns them, and the registry check in `pixels_plan_lint.rs` proves every
`D-P8R-nn` referenced anywhere resolves to exactly one definition.

> **D-P8R-01** (sealed 2026-08-15) — This interstitial milestone is named
> P8R (reconciliation), because the canonical implementation plan already
> owns the name Task P8.5. P8R contains no shading, no temporal reuse, and
> no new field operations. It adds no renderer feature and changes no
> displayed byte.

> **D-P8R-06** (sealed 2026-08-15) — P8R adds no external Cargo
> dependency, per the standing dependency policy in `AGENTS.md`. Every tool
> this milestone or its research appendix needs — kernel search, census
> emission, cache keying, doc linting — is in-house. The pinned Lean project
> under `formal/pixels/` remains the one approved non-Cargo proof tool, and
> it is outside the shipped image and the Cargo dependency graph.

### Decision index

```text
D-P8R-01  this document (above)          milestone name and scope
D-P8R-02  docs/language/06-machine.md §1 flagship worker topology profile
D-P8R-03  docs/language/07-pixels.md §1.3 camera-cut budget not relaxed
D-P8R-04  docs/language/06-machine.md §1  no implicit FMA contraction
D-P8R-05  docs/language/06-machine.md §1  no fp16 lanes on the ISA surface
D-P8R-06  this document (above)          no external Cargo dependencies
D-P8R-07  docs/language/07-pixels.md §1.5 packet substrate is internal
D-P8R-08  docs/language/07-pixels.md §1.5 formal-claim phrasing
D-P8R-09  docs/language/06-machine.md §2  RTDATA_BASE is IMAGE_BASE + 4 MiB
```

### Named follow-ups (owned, not P8R tasks)

**F-P8R-01 — reconcile the generated worker count with `pi5-3worker`.**
The generated renderer emits four workers while D-P8R-02 seals a
three-worker flagship profile. Reconciling them changes generated code and
therefore image bytes, which P8R's stop conditions forbid. File inventory:
`crates/wrela-compiler/src/pixels/glue.rs`, `stdlib/core/render.wr`,
`tests/census/p8-baseline/`. Owner: the P9 task that next regenerates worker
placement. Gate: `cargo xtask verify`.

**F-P8R-02 — stop materializing the `RTDATA_BASE` packing hole in the image
blob.** Verified real at P8R.0, against the corrected D-P8R-09 contract:
`layout.rs`'s `pad_to` resizes the image blob from the packed end of
`entry`/`code`/`rodata`/`abort`/`checkpoint` all the way to `RTDATA_BASE`,
so every sealed image carries the whole `IMAGE_BASE + 4 MiB` window as
literal zero bytes before `rtdata` begins — a floor of about 4 MiB per
image, paid on every write, digest, and guest load in every golden boot.
This is a real, reproducible cost, not a circulating rumor, and it is not
part of any P8R task: removing the padding changes the sealed image blob's
bytes and its section-table arithmetic. File inventory:
`crates/wrela-compiler/src/layout.rs`, `crates/wrela-machine/src/lib.rs`,
`crates/wrela-vmm/src/lib.rs`. Owner: the milestone that next opens image
packing. Gate: `cargo xtask verify`.

### Debt ledger corrections [S]

Struck from open-debt lists — both are implemented, and the source says so:

- `MaterialIntrinsic` classifies all four material constructors
  (`crates/wrela-compiler/src/pixels/material_intrinsics.rs::classify`:
  `standard`, `clay`, `porcelain`, `textured`; anything else fails closed).
- `.to[f64]` produces `SymValue::F64`
  (`crates/wrela-compiler/src/pixels/symbolic.rs`, the `key.ends_with(".to")`
  conversion arm).

Remaining audited debt, each with an owner:

- The residual string-suffix classification site
  `name.ends_with(".subtract")` in
  `crates/wrela-compiler/src/pixels/symbolic.rs` decides Vec3 add versus
  subtract by member-name suffix rather than a classified intrinsic. Owner:
  the P9 task that next opens Vec3 symbolic lowering.
- Product-tier optimization exception 1: `NarrowImm` falls at every point of
  every borrowed program in the corpus and is justified by the appliance
  rather than by the corpus. Owner: `opts::win`'s pinned product-tier
  verdict set (decision 1785).
- Product-tier optimization exception 2: `BoundsElide` is parked — in the
  tree, out of the shipped list — because it measured byte-identical to
  `dev` on all four product cases. Owner: `opts::PARKED_OPTS`
  (decisions 1970/1911).

## Task P8R.1 — bank-aware cost-model prerequisites

**Requires:** P8R.0.

**Produces:** A cost model that can truthfully describe mixed GPR/FP/ASIMD
code. This is a prerequisite, not an optimization: until operands are
bank-aware, any FP/ASIMD census is not a trustworthy baseline.

**Files:**

```text
crates/wrela-compiler/src/cost/rule.rs
crates/wrela-compiler/src/cost/score.rs
crates/wrela-compiler/src/cost/table.rs
crates/wrela-compiler/src/cost/mem.rs
crates/wrela-compiler/src/cost/oracles.rs
crates/wrela-compiler/src/cost/mod.rs
crates/wrela-compiler/src/codegen.rs
crates/wrela-compiler/src/mwir.rs
crates/wrela-compiler/src/lower.rs
crates/wrela-compiler/src/pixels/hot_census.rs   # new at P8 basis
bench/a76-pi5.toml
tests/census.toml
```

**Contract/dump delta:** The emitted-word operand representation, cost
classes, and cost-stage lock format change; every regenerated lock diff is
explained. No emitted instruction bytes change.

**Work:**

- Bank-aware operands [S]: `EmittedWord` currently records `dst`/`srcs` as
  bare `u8` register numbers and the scheduler keeps a single 32-entry ready
  table, so x0 and v0 alias as register 0 — the existing Q-register lowering
  already emits operands this way. Replace the operand representation with a
  banked form (general vs FP/SIMD, e.g. `Gpr(n)` / `FpSimd(n)`), split the
  scheduler's readiness state per bank, and update every `EmittedWord`
  construction site in code generation and every consumer in scoring,
  memory-rule, and oracle code.
- Opcode taxonomy: replace the generic ALU classification for the affected
  families with classes the A76 actually distinguishes, priced from the Arm
  Cortex-A76 Software Optimisation Guide: scalar FP add/sub, FP multiply,
  fused multiply-add, FP divide/sqrt, FP compare/convert, GPR↔FP/SIMD
  moves, FP loads and stores by width, ASIMD integer arithmetic, ASIMD FP
  add/multiply/FMA/compare/convert (required by P8R.5), and ASIMD loads/
  stores — including the fact that store-data micro-ops share the FP/ASIMD
  pipelines. Do not collapse observably different behaviors into one class.
  The cost-dimension inventory in `tests/census.toml` is updated in the
  same commit as the surface it locks.
- Census infrastructure: `emitted_a64_census.rs` is a static emitter-site
  inventory [S] and is not overloaded. Add a dedicated hot-path census
  module (`pixels/hot_census.rs`) that reports per-function, per-region
  breakdowns (setup, loop body, validation, stores, calls, spill/frame
  traffic) with a stable artifact format.
- Region mechanism, chosen here: non-emitting MWIR region markers — a
  marker instruction carried from source annotation through lowering,
  visible to the census, and dropped at encoding with a test proving zero
  emitted-byte effect. Stable block labels and helper-function boundaries
  were considered and rejected (label stability and call-overhead
  distortion respectively).
- Update `bench/a76-pi5.toml` entries for the new classes with provenance
  comments citing the optimisation-guide table rows.

**Tests:**

- A mixed-bank fixture proves x-register and v-register uses of the same
  number no longer create or hide dependencies.
- Every FP/ASIMD opcode emitted by the P8 renderer and reference fixtures
  maps to an operation-specific class; every other opcode maps to an
  intentional priced class (integer ALU, branch, and ordinary memory
  classes remain legitimate); only unpriced/unknown fails the lane.
- The region-marker zero-emission test: byte-identical emitted code with
  markers present and stripped.
- Cost-stage determinism across two runs and two build directories.

**Focused checks:** Run the focused tests, fixtures, dumps, and commands named
by this task before the repository gate.

**Repository gate:**

```text
cargo xtask verify
```

**Stop conditions:** Stop if bank-aware operands cannot be introduced without
changing emitted instruction bytes, or if a class's A76 pricing cannot be
grounded in a citable table row.

**Commit**

```text
pixels P8R.1: bank-aware cost operands and A76 opcode taxonomy
```

## Task P8R.2 — freeze the P8 reference census

**Requires:** P8R.1.

**Produces:** The checked-in measurement baseline every later P8R diff cites.

**Files:**

```text
crates/xtask/src/main.rs
stdlib/core/render.wr
stdlib/core/render_raster.wr
tests/census/p8-baseline/          # new at P8 basis
docs/designs/WRELA_PIXELS_P8R_TIGHTENING_PLAN.md
```

**Contract/dump delta:** One new census report lane, region markers placed in
the named stdlib sources (zero emitted-byte effect per the P8R.1 test), and
the checked-in artifacts.

**Work:**

- The exact census target list, by literal symbol:
  `__wrela_pixels_p8_raster_regular` (regions: scalar prefix, packet loop,
  scalar suffix, charge), `__wrela_pixels_p8_geometry_lane_valid`,
  `__wrela_pixels_p8_geometry_packet_valid`, the generated
  `__wrela_pixels_p8_write*` helper family as emitted,
  `__wrela_pixels_p7_union_silhouette_coverage_at_slack` (regions: entry,
  cell walk), `__wrela_pixels_p7_isolate_smooth_object`,
  `__wrela_pixels_p7_collect_roots_box`, and the generated sealed numeric
  helper sequences (`sqrt`/`rsqrt`). Region boundaries are the P8R.1
  non-emitting markers placed in the listed sources.
- Emit and check in the census artifact, internally split into two
  sections: measured facts [M] — per-function, per-region opcode counts by
  cost class, frame sizes, call counts, spill traffic, plus the image and
  dump digests identifying the exact basis — and modeled scores [I] — cycle
  totals derived from the [M] counts through the A76 cost table. The two
  sections are never merged; anything citing a cycle number cites [I].
- Record the census artifact schema version in the report header.

**Tests:**

- Census totals byte-identical across two runs and two build directories.
- The lane fails if any named function or region marker is missing.

**Focused checks:** Run the focused tests, fixtures, dumps, and commands named
by this task before the repository gate.

**Repository gate:**

```text
cargo xtask verify
```

**Stop conditions:** Stop if a named region cannot be delimited without
changing emitted code; record the boundary compromise explicitly instead.

**Commit**

```text
pixels P8R.2: freeze the priced P8 hot-path census baseline
```

## Task P8R.3 — renderer decomposition and recensus

**Requires:** P8R.2. Runs before all codegen changes so later measurements
target the final structure; it must not run in parallel with P8R.4/P8R.5.

**Produces:** `render.wr` decomposed along audited seams, the arrangement/
coverage tier restructured into a shared subdivision engine with statically
dispatched per-event-class handlers, and a fresh census of the result.

**Files:**

```text
stdlib/core/render.wr
stdlib/core/render_isolation.wr     # new at P8 basis
stdlib/core/render_certify.wr       # new at P8 basis
stdlib/core/render_arrangement.wr   # new at P8 basis
stdlib/core/render_orchestrate.wr   # new at P8 basis
stdlib/core/render_probe.wr         # new at P8 basis
crates/wrela-compiler/src/pixels/glue.rs
tests/census/p8-baseline/
```

**Contract/dump delta:** Structural dumps may change (module paths, spans,
ordering); every structural dump diff is reviewed and explained. Truth files,
displayed bytes, and presentation goldens are byte-identical. Generated-
intrinsic name changes are in scope only where the four synchronized sites
(compiler glue, stdlib, reference, tests) move in one reviewed change.

**Work:**

Stage 1 — behavior-preserving module moves: split `render.wr` into
isolation, certification, arrangement/coverage, orchestration, and
debug-probe modules (raster already separate). Moves only; function bodies
byte-identical.

Stage 2 — arrangement restructure: extract the fixed-capacity subdivision
stack shared by the union/arrangement walkers into one engine owning cell
state, depth budgets, and charge accounting; express polynomial-silhouette,
clip, smooth-band, deformation, and predicate handling as per-class
functions behind one static interface (match on sealed class, direct calls).
The interface owns, explicitly: subdivision capacity, proof-state handoff,
failure codes, event ownership, and deterministic visit order. The
canonical-torus fast path becomes one handler. Each extraction is its own
commit.

Stage 3 — recensus: rerun the P8R.2 lane against the decomposed tree, check
in the post-refactor census beside the baseline, and explain every delta
(new call boundaries, frame-size changes). This census supersedes P8R.2 as
the optimization baseline for P8R.4/P8R.5.

**Tests:**

- Conformance truth, displayed frame digests, and presentation goldens
  byte-identical after every commit.
- The debug-probe lane reproduces its recorded outputs unchanged.
- Emitted-A64 diff of the arrangement and raster hot loops against the
  P8R.2 baseline, with every delta explained; a structural assertion that
  the hot paths contain no indirect-call instruction remains as a secondary
  check.
- Charge/telemetry accounting equality on the conformance corpus.

**Focused checks:** Run the focused tests, fixtures, dumps, and commands named
by this task before the repository gate.

**Repository gate:**

```text
cargo xtask verify
```

**Stop conditions:** Stop if any truth or display byte changes, or if an
extraction cannot preserve charge/telemetry accounting exactly.

**Commit**

```text
pixels P8R.3: decompose the renderer, unify the arrangement walk, recensus
```

## Task P8R.4 — scalar FP storage contract and staged lowering

**Requires:** P8R.3.

**Produces:** A defined storage and location model for scalar floats, then
direct FP memory operations, then — only if the recensus still justifies it —
FP register residency for call-free intervals.

**Scope:** Synchronous, non-FlowWir renderer functions only. FlowWir state
frames store physical registers as bare `u8` and plan their own slots
(`frame_plan.rs`, `frame_color.rs`) [S]; extending the typed location model
compiler-wide is a recorded follow-up, not part of P8R, and the milestone
close claims are narrowed accordingly.

**Files:**

```text
crates/wrela-compiler/src/codegen.rs
crates/wrela-compiler/src/encode.rs
crates/wrela-compiler/src/regalloc.rs
crates/wrela-compiler/src/mwir.rs
crates/wrela-compiler/src/cost/table.rs
tests/census/p8-baseline/
```

**Contract/dump delta:** Emitted code, census artifacts, and cost locks
change with explained diffs. Semantic evaluation results, frame bytes, and
every golden digest are byte-identical.

**Work:**

Commit 1 — storage and location contract, no emitted-code change:

- Define the canonical representation of an `f32` in its eight-byte slot
  [S: `Type::F32` occupies the full slot today], including the upper-half
  convention. Audit every full-slot observer (slot copies, aggregate
  construction, formatting, comparison paths, any digesting of frame
  memory) and record which observers, if any, read beyond the low four
  bytes of an `f32` slot. The chosen convention and audit result are the
  written precondition for four-byte FP stores; if any observer reads the
  upper half, stores must canonicalize it explicitly.
- Define a typed value-location model — slot, GPR-resident, FP/SIMD-
  resident, immediate — integrated with the existing resident-register
  tracking, so an FP load can never blindly reload a slot whose live value
  is register-resident.
- Enumerate the scalar operations in scope: float binary arithmetic, float
  negation, float comparisons, int↔float conversions, and float call
  arguments/returns under the current ABI (each crosses register banks
  today and each needs an explicit rule).
- Fix the register discipline: the allocatable V-register pool
  (caller-saved registers only in this task), reserved scratch registers,
  call behavior, and the S/D/Q aliasing rule (one live value per register,
  width tracked).

Commit 2 — direct FP memory operations: add `ldr`/`str` s/d-form encodings
and use them per the sealed contract, eliminating GPR-mediated `fmov` round
trips while keeping the fixed-register operation structure. Rerun the
census; check in the diff [M].

Commit 3 (conditional) — FP residency for call-free intervals: proceed only
if the commit-2 census shows the GPR↔FP transfer class still contributing at
least 10% of the modeled cycle total [I], where the denominator is the
summed [I] cycle total of exactly these census functions:
`__wrela_pixels_p7_union_silhouette_coverage_at_slack`,
`__wrela_pixels_p7_isolate_smooth_object`,
`__wrela_pixels_p7_collect_roots_box`, and the generated sealed numeric
helper sequences. Otherwise record the measurement and stop this task at
commit 2. If proceeding: extend
the allocator with an FP register class over the caller-saved pool,
restricted to intervals crossing no call site, so AAPCS-style callee-save
obligations stay out of scope. Call-crossing residency and FP spill policy
are a recorded follow-up, taken only on further census evidence.

No fused operations are introduced in this task (D-P8R-04).

**Tests:**

- Full differential/eval fixture equality; every golden frame digest
  byte-identical across all commits.
- Slot-observer audit encoded as tests: an `f32` slot written by the new
  path satisfies the sealed upper-half convention under every observer
  found by the audit.
- ABI tests around calls with live float values in every lowering mode.
- Census diffs checked in for commits 2 and 3 ([M] counts, [I] scores),
  with the commit-3 threshold decision recorded either way.

**Focused checks:** Run the focused tests, fixtures, dumps, and commands
named by this task, including `cargo xtask diff-eval`, before the repository
gate.

**Repository gate:**

```text
cargo xtask verify
```

**Stop conditions:** Stop if bit-identity of any scalar result cannot be
shown, if the slot audit finds an observer the convention cannot satisfy, or
if commit 3 would require callee-save vector state to deliver its threshold.

**Commit**

```text
pixels P8R.4a: seal the scalar FP slot and location contract
pixels P8R.4b: load and store scalar floats directly through FP registers
pixels P8R.4c: allocate FP registers across call-free float intervals
```

## Task P8R.5 — renderer-internal packet substrate

**Requires:** P8R.4.

**Produces:** The exact closed vector operation set P9 geometry/shading
kernels will be authored against, as compiler-internal sealed operations per
D-P8R-07, with complete numerical semantics, scalar oracles, cost classes,
and decoded-instruction obligations — and nothing more.

**Files:**

```text
crates/wrela-compiler/src/mwir.rs
crates/wrela-compiler/src/mwir_opt.rs
crates/wrela-compiler/src/mwir_facts.rs
crates/wrela-compiler/src/lower.rs
crates/wrela-compiler/src/codegen.rs
crates/wrela-compiler/src/encode.rs
crates/wrela-compiler/src/cost/rule.rs
crates/wrela-compiler/src/cost/table.rs
crates/wrela-compiler/src/cost/oracles.rs
crates/wrela-compiler/src/pixels/glue.rs
bench/a76-pi5.toml
tests/census.toml
stdlib/core/render_raster.wr
stdlib/tests/pixels_packet_substrate.wr   # new at P8 basis
tests/census/p8-baseline/
```

**Scope:** Packet operations are available in synchronous renderer functions
only, matching the P8R.4 scope; FlowWir frames never hold packet values in
this milestone.

**Contract/dump delta:** New sealed MWIR operations and their stable dump
forms; new renderer-internal backend helpers following the existing sealed
intrinsic pattern; cost and census locks regenerate. No public library type
or syntax is added (D-P8R-07).

**Work:**

Before any operation lands, produce the consumer-to-operation matrix: each
P9.4–P9.11 kernel need mapped to the operation that serves it. An operation
without a row does not land, and the matrix must close over P9: every P9
packet need is either served by an operation in this task, or the matrix
records that the P9 path in question remains scalar and the canonical P9
task text is amended in the same commit to say so. Deferring a P9-required
operation to P12.3 is not a valid resolution — P9 executes first.

The target set, subject to that matrix:

```text
f32x4: load.aligned16 store.aligned16 splat add sub mul min max
       select_ge(a, b, t, f)   # lanewise: a >= b ? t : f
       select_gt(a, b, t, f)   # lanewise: a >  b ? t : f
       fma            # explicit sealed operation per D-P8R-04, distinct
                      # from mul+add, with its own bit-exact oracle
i32x4: load.aligned16 store.aligned16 splat add(existing) sub
       shr_arith_imm and or
       select_gt(a, b, t, f)   # lanewise signed compare-select
conv:  f32x4->i32x4 and i32x4->f32x4 with the sealed semantics below
```

There are no first-class mask values in this substrate: MWIR temporaries
receive frame storage, and this task introduces no packet/mask register
allocation, so a mask value class would either spill to memory or require a
lifetime design this milestone does not contain. The fused compare-select
operations lower to compare plus bitwise-select in fixed registers, making
mask escape structurally impossible. First-class masks, if ever needed,
arrive with P12.3's public types and a real mask-lifetime design.

Sealed representation and semantics, all mandatory and fixture-pinned:

- packed memory carrier: sealed nominal 16-byte packet structs (`I32x4`
  existing; `F32x4` new in the same style), lane 0 at the lowest address,
  16-byte frame alignment guaranteed by layout; backend helpers follow the
  existing sealed pattern with value-in/value-out signatures
  (`pixels_f32x4_backend_add(a: F32x4, b: F32x4) -> F32x4`); no unaligned
  packet memory operations exist in this substrate;
- `min`/`max` are sealed to AArch64 `FMIN`/`FMAX` semantics: a quiet-NaN
  operand propagates NaN, and −0.0 orders below +0.0; the scalar oracle
  implements these cases explicitly (a bare comparison-select is not a
  correct oracle) and fixtures pin NaN, −0.0, and mixed lanes;
- conversions are sealed to `FCVTZS` semantics for f32x4→i32x4 (round
  toward zero, NaN converts to 0, out-of-range saturates to the i32
  extremes) and `SCVTF` semantics for i32x4→f32x4 (round to nearest even
  where inexact); fixtures pin NaN, ±extreme, and inexact lanes;
- the `fma` scalar oracle is a bit-exact software reference in the existing
  sealed-numeric-sequence style (integer mantissa arithmetic in the Rust
  reference and generated fallback), differentially pinned against hardware
  `FMLA` lanes; an `a * b + c` source expression is not a fused reference,
  and evaluation through f64 is rejected for its double-rounding hazard;
- every operation has a scalar oracle body, a P8R.1 cost class (the ASIMD
  FP families P8R.1 added exist for exactly this task), one decoded AArch64
  obligation with emitted-word tests, and differential fixtures driving all
  lanes including the special-value lanes;
- P12 reconciliation: reciprocal/rsqrt estimates, lane extraction, shuffles,
  reductions, and first-class masks remain P12.3 deliverables; the matrix
  records only non-P9 needs against them.

**Tests:**

- Scalar/packet differential equality over the full fixture matrix,
  including NaN, signed-zero, and out-of-range conversion lanes.
- Stable MWIR dump forms; decode tests reject any emitted word outside the
  declared obligation set.
- `mwir_opt` temp-visitor coverage proving new instructions participate in
  the existing passes or are explicitly opted out with a recorded reason.
- The consumer matrix's P9 closure is checked: every P9.4–P9.11 packet need
  row resolves to a landed operation or an amended canonical task text.
- Census lane extended to the new operations.

**Focused checks:** Run the focused tests, fixtures, dumps, and commands
named by this task, including `cargo xtask diff-eval`, before the repository
gate.

**Repository gate:**

```text
cargo xtask verify
```

**Stop conditions:** Stop if an operation lacks a consumer-matrix row, if a
lane or special-value semantic cannot be given a scalar oracle, or if the
work drifts toward implementing the public §8.1 SIMD types (that is P12.3).

**Commit**

```text
pixels P8R.5: seal the renderer-internal f32x4/i32x4 packet substrate
```

## Task P8R.6 — safe development-loop acceleration

**Requires:** P8R.0. May run any time after P8R.0; it changes no compiler or
renderer bytes.

**Produces:** A conformance loop where the named instrumented-image and
scoring stages are skipped on verified key hits, with byte-identical cold,
warm, and cache-disabled results and a complete cache contract. Semantic
compilation still runs where other conformance checks consume it; this task
does not claim "unchanged work is never repeated" in general.

**Files:**

```text
crates/xtask/src/pixels_conformance.rs
crates/xtask/src/main.rs
crates/wrela-compiler/src/bin/wrela.rs
```

**Contract/dump delta:** None. Caches affect wall time only; all reports and
truth files byte-identical with caches disabled.

**Work:**

- Compile-closure cache. The cached value is defined exactly: the mapping
  from source-closure key to instrumented image digest. The key is the case
  sources, the stdlib closure, the compiler binary fingerprint, and the
  active compiler options. A verified hit skips the
  `wrela test --image-digest-only` compilation that currently precedes the
  boot-cache decision; when the VMM changed and a fresh boot is required,
  compilation still runs — this cache stores no image artifact and makes no
  claim beyond the digest step.
- Case-level score cache. The key covers every input `score_frame` actually
  consumes, without relying on the image digest to commit to compiler
  internals: source-closure digest, compiler fingerprint and active
  options, case identity and fixture version, the complete recorded
  evidence blob digests (stdout transcript, frame dump, state dump), scorer
  source fingerprint and options, and the truth-schema/numeric-contract
  version. The VMM binary digest keys only the boot cache and is explicitly
  not part of the score key.
- Cache contract, both caches: versioned artifact format with a schema
  header; atomic tmp+rename writes; concurrent writers race safely
  (content is digest-verified on read; a loser's identical artifact is
  harmless, a mismatched artifact is a miss); corruption or schema mismatch
  is a cache miss, never an error; failed scores/compiles are never cached;
  `WRELA_P8_*_CACHE=0` disables each cache; a named xtask flag clears them.
- Boot concurrency stays at two by default; a third worker is an opt-in
  environment flag recorded as an experiment, promoted only with a
  stability record.

**Tests:**

- Cold run, warm run, and cache-disabled run produce byte-identical reports
  and truth files.
- A table-driven key-perturbation matrix: each key component, perturbed,
  invalidates exactly its dependents.
- Corruption injection (truncated and bit-flipped artifacts) degrades to a
  miss with a warning, never a wrong result.
- Measured wall-time improvement recorded as a checked-in report [M].

**Focused checks:** Run the focused tests, fixtures, dumps, and commands named
by this task before the repository gate.

**Repository gate:**

```text
cargo xtask verify
```

**Stop conditions:** Stop if any cached path can produce output not
byte-identical to the uncached path, or if key derivation would need to
consult mutable global state.

**Commit**

```text
pixels P8R.6: versioned compile-closure and score caches, byte-identical
```

## Task P8R.7 — invariant ownership matrix and close

**Requires:** P8R.0–P8R.6.

**Produces:** The milestone's central artifact — one matrix tracing every
renderer invariant from normative authority to cost census — a final
recensus, and the P9 entry report with an unambiguous gap policy.

**Files:**

```text
docs/designs/WRELA_PIXELS_INVARIANT_MATRIX.md   # new at P8 basis
docs/designs/WRELA_PIXELS_P8R_TIGHTENING_PLAN.md
crates/xtask/src/pixels_plan_lint.rs
crates/xtask/src/main.rs
tests/census/p8-baseline/
```

**Contract/dump delta:** Documentation plus one machine-enforced
doc-consistency lane and the final census artifacts.

**Work:**

Build the matrix with one row per invariant, each row carrying a stable ID
(`INV-PIX-<AREA>-nnn`, e.g. `INV-PIX-COVERAGE-001`) so ownership and diff
review stay durable as rows are added — artifact existence alone does not
establish completeness. Columns:

```text
invariant -> normative authority -> compiler producer -> guest verifier
          -> Lean theorem -> differential fixture -> failure mapping
          -> cost census entry
```

Each cell holds one of: `covered(artifact)` [S], `not-applicable(reason)`,
`planned(Pn)`, or `blocking-gap(owner)`. Not every invariant requires every
column — `not-applicable` is a legitimate, reasoned state, and the matrix is
honest rather than aspirational.

Row coverage spans the full pipeline, not only numeric kernels: symbolic
lowering legality, event completeness, exclusion validity, projective
bounds, capacity derivation, placement, snapshot validation, run
certification, coverage/quantization, display submission and evidence,
replay, and failure propagation.

Gap policy, sealed here: a `blocking-gap` row blocks P9 entry until resolved
or explicitly converted, by decision ID, into an accepted-deferred gap with
an owner and a milestone. The P9 entry report lists both classes separately;
"stop" versus "record and proceed" is never left to executor judgment.

Extend the P8R.0 plan lint with the matrix check: every artifact a
`covered(artifact)` cell cites exists at the close commit, every row ID is
unique, and every `blocking-gap`/accepted-deferred row cites its decision
ID. (Decision-ID resolution itself has been machine-checked since P8R.0.)

Close with the final recensus [M] (superseding baseline: P8R.3 stage 3) and
the P9 entry report: sealed decisions, census baseline and per-task deltas,
cache behavior, topology status, and the two gap lists.

**Tests:**

- The doc-consistency lane passes and fails correctly under an injected
  missing artifact.

**Focused checks:** Run the focused tests, fixtures, dumps, and commands named
by this task before the repository gate.

**Repository gate:**

```text
cargo xtask verify
```

**Stop conditions:** Stop only for `blocking-gap` rows per the sealed gap
policy; every other finding is recorded, owned, and carried into the entry
report.

**Commit**

```text
pixels P8R.7: publish the invariant ownership matrix and close P8R
```

### Milestone P8R close

Run `cargo xtask verify`. The contracts named in P8R.0 are internally
consistent under stable decision IDs and the canonical chain requires P8R.7
before P9.1; the cost model distinguishes register banks and A76-distinct
operation families; a checked-in census baseline exists for the decomposed
renderer; scalar floats in synchronous renderer functions have a sealed
storage and location contract and no longer transfer through general
registers per operation (FlowWir state frames are a recorded follow-up); the
renderer-internal packet substrate is sealed with complete special-value
semantics and a P9-closed consumer matrix; the conformance loop skips its
named instrumented-image and scoring stages on verified key hits under a
complete cache contract; and the invariant matrix, with its stable row IDs,
machine-checked citations, and sealed gap policy, is the standing map of
proof ownership. No displayed byte or truth file
differs from the P8 close; structural dumps, cost locks, and census
artifacts differ only where a task explicitly regenerated them with an
explained diff. The cycle budget remains [I] until the P13 proxy or physical
hardware evidence exists.

---

## Appendix A — research backlog (not P8R tasks)

Non-executable. Every entry is [H] with a falsifier and an activation gate.
None may proceed under this plan; each requires its own future task entry.

**Advance when telemetry justifies:**

- Region-specialized field tapes (compile-time interval specialization of the
  scalar tape per screen/world cell, every dropped subtree carrying an
  exclusion record). Gate: tape-length/cell-size study on the adversarial
  corpus. Falsifier: frame-program or index capacity blowup at useful
  granularity; fallback is per-tile active-object lists.
- Scan-plane coefficient coherence (certified vertical forward differencing
  of scanline Bernstein coefficients, as a proposal mechanism only; complete
  root/event authority per row is unchanged). Gate: census/telemetry showing
  row-start isolation is a material share of frame cost. Falsifier: small
  share, or recurrence-error growth forcing resets every few rows.
- Compensated/EFT candidate arithmetic (TwoSum, TwoProd-via-explicit-fma,
  compensated Horner/de Casteljau) for proposal precision, with explicit
  overflow/underflow/subnormal handling. Gate: census showing tube widths
  dominated by proposal rounding rather than Taylor remainders.

**Adopt as methodology at the named milestone:**

- Verified-libm coefficient discipline for P9 summaries: offline Remez/
  Chebyshev generation, checked-in tables, Rust/Lean verification of the
  stated remainder. No second proof ecosystem is imported.
- Kinetic-data-structure vocabulary and certified-homotopy step theory
  (Krawczyk-method tracking bounds) at P11 design time, as derivation
  discipline for `@rate` slack budgets. Homotopy/α-theory estimates remain
  proposals; dyadic revalidation keeps authority, as the temporal contract
  already requires.

**Reframed by review, adopt only in reframed form:**

- Narrow-precision arithmetic: as-if candidates under contract §0.1(8) whose
  conservative residual proves the stored byte singleton; fixed-point summary
  recurrences first; no fp16 ISA surface until the f32 baseline and output-
  proof telemetry exist.
- Object-space summary anchoring: cache camera-invariant material basis terms
  where validity is certified; view direction, lights, shadows, footprint,
  exposure, and visibility remain per-frame obligations.
- Kernel-sequence search: in-house exhaustive enumeration over the sealed
  P12 kernel grammar with equivalence checking against the scalar oracle and
  the calibrated cost model; optimality claims are scoped to that grammar and
  model, nothing broader.

**Deferred with named blockers:**

- Taylor-model/reduced-affine enclosure unification: blocked on subdivision-
  depth telemetry demonstrating measured cost.
- Any "first"/novelty claim: blocked on a genuine literature and patent
  survey.
