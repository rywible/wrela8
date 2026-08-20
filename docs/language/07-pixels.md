# Pixels language, compiler, and runtime contract

This chapter is the normative contract for the production Pixels subsystem.
The [implementation plan](../designs/WRELA_PIXELS_COMPILER_IMPLEMENTATION_PLAN.md)
controls task order, repository ownership, and gates; it does not override this
chapter's language or runtime semantics. A semantic change lands here first and
reconciles the plan in the same change.

Pixels compiles a field-authored scene into a sealed, immutable frame program.
A validated scanline sweep constructs visible structure from scratch. Later
frames may reuse that structure only while every relevant certificate remains
valid. Any proof or capacity failure returns `RenderError` and leaves the last
complete framebuffer on screen; it never guesses a hit, a color, or background.

## 0. Delivered contract

A source image binds a pure `@field` root, a pure `@material` root, bounded
frame inputs, and one display driver with `Image.renderer`. The returned
`ImageDecl[Renderer[P]]` can produce an `Actor[Renderer[P]]` handle. The public
method owns the parameter value during rendering:

```text
pub async fn render(
    take frame: RenderFrame[P],
) -> Result[RenderedFrame[P], RenderError]
```

`RenderFrame[P]` contains owned `params: P`, `camera`, a fixed-capacity
`LightFrame`, `exposure`, `environment`, and `frame_index`. Runtime camera,
light, exposure, and environment values are validated against the renderer
declaration before a worker writes output. `RenderedFrame[P]` returns `P` only
after successful presentation.

The flagship `RenderProfile.AaaByteExact` contract is deterministic and
allocation-free. It supports hard and smooth field geometry, exact object and
material identity, analytic silhouette coverage, deterministic lighting and
filtering, ordered transparency, and exact replay bytes. Stochastic sampling,
stochastic dither, denoising, a host renderer, and a GPU renderer are outside
version 1.

The P-1 64×32, one-plane walking skeleton remains an isolated end-to-end
boundary fixture. It is not production renderer semantics and must not be
generalized piecemeal. Its semantic seed is generated-actor compatibility
metadata, never a v1 table count or reserved header byte. Correcting that
envelope does not change the locked P-1 displayed-frame digest.

### 0.1 Definition of done

The production renderer is complete only when every item below is true:

1. A Wrela image declares a renderer through `Image.renderer` and boots it
   through the ordinary image and layout pipeline.
2. `@field` and `@material` functions have stable typed dumps, deterministic
   compiler artifacts, and focused diagnostics.
3. Every flagship field operation has exact scalar semantics and a
   conservative compiler proof rule.
4. The compiler emits a versioned `FrameProgram v1` data section and exact
   mutable renderer-state placement.
5. The guest constructs a frame from scratch with the validated sweep,
   without dense truth, a sample lattice, a dense edge mask, or previous-frame
   state.
6. Kinetic reuse is only an optimization; disabling it produces the same
   displayed bytes.
7. Every accepted visibility run proves root existence, root uniqueness,
   identity stability, and front order for its complete domain.
8. Every approximation in coverage, shading, transparency, post, and temporal
   transport is either proven unable to change the stored output code or is
   refined or falls back.
9. A proof or capacity failure prevents presentation and returns
   `RenderError`; it never becomes background, a stale hit, or a guessed
   color. Acceptance is fail-closed but not universally total over every
   declared numeric range: release conformance separately proves zero errors
   for the locked workload.
10. The compiler report publishes frame-program bytes, renderer-state bytes,
    per-core placement, exact capacity derivations, fallback classes, and
    generated hot functions.
11. The Lean project builds with no admissions and prints no unexpected axioms
    for the trust-boundary theorems.
12. The Rust compiler reference, generated Wrela scalar kernels, generated
    Wrela SIMD kernels, and host oracle agree on every permanent differential
    fixture; every hot workload satisfies its one-ISA instruction obligation
    with no missed or illegal idiom.
13. The machine-v1 display conformance lane presents the exact expected frame
    digests.
14. The flagship A76/Pi 5 target profile is admitted at 1080p60 by the
    exact sealed renderer cycle proxy, with every acceptance frame below budget, no
    unresolved frame, and no output divergence during the locked workload.
    Physical hardware execution is not a conformance input.

Items 13 and 14 are release conformance, not research lanes. They do not
choose algorithms or tune tolerances. The algorithms and tolerances in this
chapter are fixed before those gates run.

## 1. Closed architectural decisions

### 1.1 Compiler data, not another executable IR

`FieldGraph`, `MaterialGraph`, and `FrameProgram` are compiler-owned data.
Source functions still type-check into the typed program. Ordinary renderer
runtime code still lowers through FlowWir, MachineWir, and AArch64.
`FieldGraph` exists only during Pixels compilation; it is not serialized as a
compiler cache. `FrameProgram v1` is immutable image data consumed by the
standard-library renderer. Its scalar tape defines fallback source semantics;
it is not a fourth Wrela executable IR.

Pixels uses a dedicated symbolic interpreter over typed expressions and
statements. Renderer-only symbolic values never enter the generic comptime
`eval::Value` domain.

### 1.2 Opaque fields and structural semantics

`core.field.Field` has a scalar runtime representation but private
construction. Authors compose it only through the closed field API. This keeps
surface, feature, object, and material structure recoverable and prevents
ordinary scalar arithmetic from acquiring an ambiguous field meaning.

The compiler emits both structural object/feature records and a scalar
semantic tape. Structural specializations may propose candidates and discharge
proofs; the scalar tape remains the exact fallback and differential oracle.

### 1.3 Complete local proof, from scratch

Pixels does not precompute a global arrangement or enumerate all visibility
cells. For the current tile, row band, and parameter box it proves:

```text
all possible roots are covered
+ all possible combinatorial changes have active predicates
+ every omitted predicate has a valid exclusion certificate
+ active predicates exclude zero over the run
=> visible combinatorics are fixed over the run
```

An exclusion proved over the complete declared parameter box and spatial
domain is permanent even when an influencing parameter has nonzero rate.
Exclusions whose spatial and influencing-parameter dependence is polynomial
after bounded coefficient lowering first use outward Bernstein coefficient
signs and bounded subdivision. Unsupported or inconclusive forms remain
runtime predicates. Every globally excluded subject retains an auditable
compiler proof record; it is never silently dropped.

The validated scanline sweep is the primary renderer. It is correct without
prior-frame state and is used for the first frame, camera cuts, whips, disabled
kinetic reuse, and failed temporal certificates. A run ends at the earliest
geometric event, proof expiry, ordering expiry, shading/transfer expiry,
fixed-point range expiry, or tile boundary.

Kinetic maintenance is optional work reduction. It is never a correctness
input, and disabling it preserves displayed bytes and errors.

> **D-P8R-03** (sealed 2026-08-15) — The camera-cut budget is not relaxed.
> The full-sweep contract above stands unchanged: a cut, a whip, a first
> frame, disabled kinetic reuse, and a failed temporal certificate each
> rebuild from scratch inside the ordinary per-frame budget, and the 60 Hz
> cut/whip requirement of the temporal milestone is not weakened to buy
> headroom for any optimization. An optimization that only meets budget by
> exempting cuts has not met budget.

### 1.4 Separate candidates from authority

Candidate construction may use bit-defined floating-point arithmetic, jets,
or other approximations. Acceptance uses conservative dyadic intervals with
integer endpoints. No approximate value has authority until the interval
verifier accepts it. An overflow or invalid proof domain is unresolved and
fails closed; it is not widened to an apparently useful infinite interval.

`AaaByteExact` rejects unsupported field operations, unbounded transforms,
runtime topology branches, unbounded repetition, missing proof ranges, and
unbounded material discontinuities at build time. It never silently lowers
them to an uncertified marcher.

### 1.5 Quality and proof ownership

Point and directional shadows use certified secondary visibility. Area lights
use deterministic adaptive emitter integration with interval radiance bounds.
Polynomial or tensor shading summaries are accepted only by residual bounds;
low-rank compression is optional and never assumed.

The deterministic refinement scheduler orders candidates by exact
cross-multiplied guaranteed display-error reduction lower bound over estimated
cost and recomputes after every accepted refinement. A lower bound may be zero
when the certificate guarantees only discrete-depth progress; such ties use
the stable source/payload order and may never claim a positive byte reduction.
No independent approximation ratio is part of the contract.

Lean proves generic mathematics. Build-time Rust constructs concrete facts,
stable dumps expose them, and generated guest verifiers consume the encoded
records. Lean is not invoked by an ordinary Wrela build.

> **D-P8R-08** (sealed 2026-08-15) — Formal claims use exactly this
> phrasing: Lean proves generic kernel mathematics; build-time Rust
> constructs concrete facts; generated guest verifiers check encoded
> records. No document claims an end-to-end verified compiler or renderer,
> a proof of the shipped image, or a machine-checked pipeline. The
> candidate/authority separation of §1.4 is part of the same claim: every
> technique introduced anywhere in this stack *proposes*, and conservative
> dyadic verification *accepts*. The tracked-tree sweep
> (`cargo xtask agnostic-sweep`) enforces the superseded over-claims so a
> document cannot drift back into them.

> **D-P8R-07** (sealed 2026-08-15) — The renderer packet substrate is
> compiler-internal and renderer-internal. Sealed MachineWir packet
> operations are surfaced only through the existing generated/sealed
> renderer intrinsic pattern (`pixels_i32x4_backend_add`-style backend
> helpers with value-in/value-out signatures over sealed 16-byte packet
> structs). They do not implement, extend, or expose the public library
> SIMD types of [05 §8.1](05-library.md); those remain the deliverable of
> the canonical plan's Task P12.3, which re-plates these operations onto
> the public types deliberately rather than discovering them.

## 2. Source and semantic contract

### 2.1 Attributes

`@field` is allowed only on a top-level synchronous function with one of these
shapes:

```text
@field
fn world(p: Vec3) -> Field

@field
fn world(p: Vec3, read params: P) -> Field
```

The root has no receiver or generics, is not async or a task, and returns
exactly `core.field.Field`. `P` is finite data. The transitive call graph has
available bodies, is pure and terminating, has no recursion, and may loop only
when comptime unrolled over an exact array length. Runtime control flow
depending on coordinates or parameters is rejected. Hardware, actors, time,
entropy, panic, mutable statics, allocation, runtime object/material IDs, and
runtime repetition counts are forbidden.

`@material` is allowed only on a top-level synchronous function:

```text
@material
fn shade(surface: SurfaceContext[M], read params: P) -> MaterialSample
```

The parameter may be omitted. `M` is the single nominal material enum reached
by every `mark` in the bound field graph. Material selection on
`surface.material` is explicit dataflow. Other runtime control flow is accepted
only when the material graph represents both branches and proves their
boundary.

`@range(min=..., max=...)` applies to influencing `f32`, `Vec2`, `Vec3`, or
`Rgb` fields. Endpoints are finite `f32` literals with `min <= max`; vector
ranges apply component-wise. Structs and arrays have no recursive shorthand.
Every influencing numeric path resolves to exactly one range. Integers and
enums need no numeric range. A direct `[f32; N]` or tuple-of-`f32` parameter
cannot attach contracts to its elements and therefore cannot influence a
renderer. Put each influencing scalar/vector in a named struct field carrying
`@range`, then store that finite wrapper in the array or tuple.

`@rate(max_delta=..., max_second_delta=...)` is optional. Its finite,
nonnegative values are measured per rendered frame. Omission is legal and
disables kinetic reuse whenever that path changes. The runtime checks observed
deltas before using a temporal certificate.

The symbolic dependency classes are `Comptime`, `Coordinate`, `Parameter`,
and `CoordinateAndParameter`. A field control-flow condition must be
`Comptime`. Explicit field operations such as min/max, CSG, feature validity,
and material selection remain graph nodes with representable boundaries.

### 2.2 Closed field operations

The version-1 constructors are:

```text
plane sphere box round_box capsule finite_cylinder finite_cone torus
```

Transforms are:

```text
translate rotate rigid_transform uniform_scale
finite_repeat_x finite_repeat_y finite_repeat_z
```

Composition is:

```text
union intersection subtract
smooth_union smooth_intersection smooth_subtract
```

Metadata and deformation are:

```text
mark sinusoidal_displace
```

The closed scalar/vector helper surface used inside field and material
helpers is:

```text
sqrt_scalar rsqrt_scalar sin_scalar cos_scalar
dot3 cross3 length2 length3 normalize3
```

These helpers have ordinary Wrela bodies that define fallback semantics and
dedicated symbolic nodes with versioned semantic operation IDs.

Every operation has a real scalar Wrela implementation, a symbolic rule,
range and derivative rules, structural bounds, a cost rule, a Rust reference,
and a theorem or reduction to proved primitives. `sinusoidal_displace` is the
only public deformation constructor in v1. The compiler derives its amplitude,
gradient, Hessian, and third-derivative bounds; authors cannot assert arbitrary
deformation contracts.

The polynomial smooth minimum is:

```text
if a <= b - k: a
else if b <= a - k: b
else:
    h = 0.5 + 0.5 * (b - a) / k
    b + (a - b) * h - k * h * (1 - h)
```

`k` is finite and positive. Saturated branches return the selected operand
bit-for-bit. Its bound is
`min(a,b) - k/4 <= smooth_min(a,b,k) <= min(a,b)`. Smooth support budgets
accumulate conservatively; sampled branch gaps are never proof.

### 2.3 Renderer declaration

Version 1 recognizes:

```text
img.renderer[P](
    field=field_fn,
    material=material_fn,
    display=display_driver_decl,
    width=W,
    height=H,
    refresh_hz=R,
    shade_hz=S,
    profile=RenderProfile.AaaByteExact,
    tone_curve=T,
    near=NEAR,
    far=FAR,
    world_min=MIN,
    world_max=MAX,
    camera_bounds=CAMERA,
    light_config=LIGHTS,
    exposure_range=EXPOSURE,
    environment_range=ENVIRONMENT,
    ao=AO,
    probes=PROBES,
    initialization_deadline_ms=DEADLINE,
)
```

Every label is required. `P`, the two roots, and their material identity must
match. The display declaration is owned by one renderer. Dimensions and rates
are positive compile-time integers; `shade_hz` divides `refresh_hz`. Near/far,
world, camera, light, exposure, environment, AO, probe, and initialization
contracts are finite, ordered, and compile-time. The output extent matches the
display. The renderer and its generated workers receive deterministic
placement and participate in the image dependency DAG.

`CameraBounds.bounded(max_motion)` is a sealed version-1 world-relative camera
contract, not merely a performance hint. The canonical eye has x/y at the
renderer world-AABB center and z behind its minimum-z face by
`near + max_motion`; its canonical forward/right/up basis is `+Z/+X/+Y`, its
vertical field of view is 90 degrees, and its basis tolerance is the
versioned renderer numeric tolerance. The complete accepted eye box expands
each canonical eye component by `max_motion`, and the accepted basis box
expands each corresponding canonical basis component by `max_motion` plus the
numeric tolerance. Runtime camera inputs may be
authored through any closed camera constructor, but canonicalization must
produce a right-handed orthonormal basis and a pose inside this box; otherwise
the frame fails with `FrameContractMismatch`. `max_motion` also bounds
inter-frame eye displacement for temporal certificates. This finite absolute
pose contract is what makes compile-time projected spans authoritative.

The result is `ImageDecl[Renderer[P]]`. Its `handle()` method yields
`Actor[Renderer[P]]` under the narrow sealed generic-actor exception; arbitrary
generic actors remain rejected.

### 2.4 Runtime results and failure

The sole public failure contract is:

```text
enum RenderError:
    ParameterOutOfRange(RenderPath)
    NonFiniteInput(RenderPath)
    FrameContractMismatch(RenderPath)
    RootIsolationExhausted(TileId)
    EventIsolationExhausted(TileId)
    CertificateExhausted(TileId)
    CapacityExceeded(RenderCapacity)
    FixedPointRangeExceeded(TileId)
    Display(DisplayError)
    InternalInvariant(RenderInvariant)
```

`RenderedFrame[P]` contains returned `params`, `frame_index`,
`displayed_digest`, `rebuilt_tiles`, and `reused_tiles`. No failure flushes a
partial back buffer. Root, event, or certificate exhaustion is an expected
fail-closed result. A static table/workspace mismatch is an internal invariant.
Neither means background or success. The release workload has the stronger
requirement that no supported frame returns an error.

## 3. Compiler pipeline and ownership

After image evaluation and sealing, `pixels::compile_all` compiles every
renderer declaration before guest reachability is finalized:

```text
typed @field/@material roots
  -> FieldGraph and MaterialGraph
  -> structural proofs and capacities
  -> FrameProgram v1
  -> generated actor and glue facts
  -> ordinary FlowWir/MachineWir lowering and code generation
  -> code, rodata, rtdata, frameprog, pixelsdata, report, image digest
```

Pixels results are passed explicitly through build, layout, and report code;
they are never hidden in thread-local state.

The only stable Pixels dump stages are `field-graph`, `frame-program`, and
`render-layout`. They sort renderers by image declaration index, accept
`--renderer=<index>` to select one renderer, and print version headers plus
`Renderers count=0` for a valid image with no renderer. An index that does not
exist is a build error.

`field-graph` records canonical scalar/field/material nodes, parameter paths,
proof metadata, features, objects, CSG, interaction edges, and capacities.
`frame-program` records header/digest, directory, encoded records, offsets,
alignment, sizes, and revisions. `render-layout` records program/state
placement, generated actors, per-core workspaces, tile ownership, display
buffers, and report totals.

### 3.1 Compiler ownership

The `pixels` compiler subsystem owns attributes and diagnostics; the dedicated
symbolic evaluator; canonical scalar, vector, field, material, object, and
feature graphs; parameter paths; interval/derivative/world/support proofs;
smooth-object and hard-CSG partitioning; projective, polynomial, derivative,
and event programs; capacity derivation; frame-program records and
encoding/decoding; stable dumps and report blocks; generated actor/glue facts;
and allocation-free Rust references used only by compiler tests and host
conformance.

Syntax supplies ordinary attribute spans but no Pixels grammar. Sema owns
source typing and body-subset legality. Image evaluation owns sealed renderer
declarations but no symbolic field values. Layout owns explicit `frameprog`
and `pixelsdata` placement supplied by Pixels. Ordinary runtime code remains
owned by lowering and code generation. The VMM owns only display validation
and scanout, never renderer semantics.

### 3.2 P8 raster and presentation boundary

The certified sweep lowers regular half-open domains to compact `RasterRun`
records containing fixed-q `q`, `q_u`, and `q_v` recurrences, nominal summary
IDs, and an output proof code. Event corridors lower to one `EventPixel` per
owned pixel. Feature arrays and root-isolation stacks do not cross this
boundary. A run that cannot fit a certified fixed-q microtile is split and
re-anchored before lowering.

The permanent scalar raster is the differential oracle. Production regular
runs use four-lane `i32x4` recurrence packets; the AArch64 lowering emits
128-bit ASIMD add operations and records the real frame-slot loads/stores in
the cost model. Scalar prefix/suffix lanes and every microtile reset use the
same certified recurrence. Regular runs never evaluate a field gradient or
solve a root per pixel. Geometry dependency flags reconstruct the inverse-
depth normal `(q_u, q_v, q-u*q_u-v*q_v)` and normalize it with the sealed
numeric sequence only when direction is consumed. World position is absent
from the P8 debug-identity program because no summary requests it.

Event coverage is analytic interval coverage, not MSAA or TAA. Each event
pixel blends its certified front and back codes with exact interval
arithmetic. A non-singleton stored byte triggers the bounded curve, side, and
pixel-domain refinement sequence; remaining ambiguity is
`CertificateExhausted`. Regular runs skip event-owned pixels, so a visible
pixel is written exactly once.

Each renderer owns two complete row-major scanout generations. Image boot
zeros both 8192-byte-per-tile allocations once. Every later successful frame
overwrites every visible byte; padding is never touched and stays zero. The
display owns the front generation until completion. Cancellation or display
failure reclaims the back generation and preserves the prior front and frame
sequence. A successful coordinator submission publishes ascending
descriptors, records descriptor/visible/raw digests, and swaps ownership only
after the machine-v1 completion status is `presented`.

Before the coordinator hands the generation to the display device, generated
bounded routines digest the visible bytes, every byte of each full tile
allocation, and the exact ascending descriptor encoding independently. Those
three guest evidence values occupy distinct machine control fields; the VMM
reconstructs each input and rejects the submission if any class differs.

## 4. Internal data model and image format

All IDs are typed dense integer newtypes assigned after deterministic
canonicalization. No runtime record implicitly stores a source or Rust enum
discriminant. The canonical order, for every order that reaches a dump,
encoded program, report, or build digest, is:

1. renderer declaration order in the sealed image graph;
2. canonical callee key;
3. source span `(module path, byte start, byte end)`;
4. structural child IDs;
5. exact immediate bits.

Implementations use ordered maps, sorted vectors, and explicit stable sorts.
The source span is an explicit canonical key, not merely a diagnostic
tie-break.

Symbolic values distinguish scalar/vector nodes, opaque fields, object and
material IDs, finite arrays/structs, and comptime values. They never represent
actors, hardware resources, runtime allocation, or arbitrary function values.
Every field node carries conservative value, world, Lipschitz, derivative,
smooth-support, identity, and finiteness metadata where applicable.

Each maximal smooth object owns the complete composed scalar root program.
Hard union, intersection, subtraction, and negation are object-partition
boundaries. Closed coordinate transforms and positive uniform scaling commute
through hard min/max in source-f32 order. The compiler also distributes the
closed common additive deformation through hard operations, reversing its
offset under negation/subtraction. Smooth operations that enclose a hard
operation are rejected because rounded source-f32 smooth minimum is not
distributive over hard min/max; rewriting such a tree would change its authored
zero set. Authors keep hard operations outside maximal smooth subtrees. This
fail-closed frontier preserves the exact authored scalar bits and guarantees
that every accepted hard boundary is exposed to object partitioning.
Primitive candidate slabs are conservative sublevel domains where
`leaf <= accumulated_support_budget`, not neighborhoods inferred only from
leaf zeros. Polynomial sublevel boundaries may be isolated from
`leaf - accumulated_support_budget` with Bernstein sign variation and bounded
subdivision. Runtime isolation evaluates the full object scalar throughout
every retained slab. Consequently the permanent `a=b=k/4` smooth-min root is
covered even though neither leaf is zero.

Camera rays are unnormalized:

```text
r(u,v) = forward + u*right + v*up
P(u,v,q) = eye + r(u,v)/q
q = 1 / view_axis_depth, q > 0
```

For degree-`d` implicit `phi`, the compiler uses
`Phi(u,v,q) = q^d * phi(eye + r(u,v)/q)`. Positive-q roots are preserved.
Affine inverse-depth features also carry a canonical rational program
`numerator/denominator`, its complete domain, and a strict-sign denominator
proof. The verifier reconstructs both parts from the affine root equation.
For a local scanline, composition substitutes both `q=q_hat(X)` and
`u=u0+X`. The fixed-shape schedule records the source `u`, `q`, and explicit
`X` degrees and expands both affine powers; treating `u` as a frozen
coefficient is invalid.

The version-1 source `Camera` keeps raw fields private and exposes closed
constructors for canonical, eye/target/up, quaternion, and explicit-basis
authorship. The noncanonical constructors return `Result`, reject
zero/parallel/left-handed inputs, and normalize into the common
eye/right/up/forward representation. Every runtime/reference camera evaluation
validates orthonormality, handedness, and pose-box containment before using a
sample; the compiler contract encloses every unit-basis component and its
declared frame-rate motion.

Local events include projected-bound entry/exit, feature validity, tangency,
smooth-band, identity, depth-order, repeat, and material boundaries. An
object derivative cluster names the full composed scalar DAG root, its sorted
leaf predictor signature, derivative-bound sources, complete-domain Taylor
remainder, and total predictor-slab/root-corridor capacity. Smooth-object
capacity charges every bounded subdivision leaf of every predictor slab.
Primitive zero equations
are candidate slabs and never replace the composed smooth root. A smooth band
has two boundary branches, `a-b-k=0` and `a-b+k=0`; its root capacity is twice
the corresponding center-tie bound.

Non-affine depth swaps use a quadratic local implicit-sheet difference. Their
remainder program names both sheets' third-partial programs, the local
scanline and common-q domains, and requires a strict runtime `G_q` proof. If
that proof fails, the Taylor predictor is discarded and the complete
q-difference interval is returned as ambiguity; it is never accepted as a
signed ordering proof.
omitted object, feature, or event has a domain-scoped exclusion record with
strictly positive slack. Domain splits regenerate invalidated exclusions.
Global parameter-box and spatial-box strict-sign exclusions remain valid
across all allowed parameter motion. Their proof payload records the
normalized box, polynomial program, Bernstein degree and coefficient order or
subdivision tree, outward conversion radius, sign, and minimum margin. A
failed supported-shape proof emits the ordinary runtime predicate instead.
Undeformed torus silhouettes use the local simultaneous oracle `G=G_q=0`
with exact derivative program IDs, complete-domain Taylor bounds, and the
degree-four/degree-three Bézout bound of twelve isolated scanline roots.
Generic positive-q feature roots use outward interval Bernstein subdivision;
a cell is accepted only after a strict derivative hull and opposite endpoint
signs prove one simple root. An unresolved cell at the sealed ambiguity depth
is retained as a corridor, so tangent/even-multiplicity roots cannot disappear;
adjacent corridors are conservatively merged to the polynomial-degree
capacity. Smooth
band/tie ceilings multiply the owning sheet degree or deformation-oscillation
bound by the summed opposite-operand bound, rather than treating the number of
ray sheets as a scanline intersection count. Scalar numeric events serialize
a quadratic world-space Taylor program with the feature-AABB diagonal and the
complete `M3 * delta^3 / 3!` remainder; the verifier reconstructs those values
bit-for-bit.

### 4.1 FrameProgram v1

The frame program is little-endian, pointer-free, offset-based, and
directory-based. Its header is exactly 80 bytes:

```text
magic[8] = "WRELAPX\0"
version: u16 = 1
header_bytes: u16 = 80
flags: u32
total_bytes: u32
renderer_index: u16
reserved0: u16 = 0
numeric_revision: u32
formal_revision: u32
table_count: u16
reserved1[14] = 0
digest[32]
```

The directory begins at byte 80. Its 16-byte entries contain table kind,
record size, count, aligned offset, and byte length, sorted by the versioned
table-kind number. The namespace covers scalar, field, object, feature,
material, parameter, event, CSG, fixed-domain, immediate, camera/light/post,
texture, shading, transparency, probe, kinetic, and optional debug-name data.
Absent tables encode zero count, offset, and length.

The digest is SHA-256 over the complete encoding with the digest field zero.
Serialization writes fields explicitly and never uses Rust host layout. All
tables are 16-byte aligned and all reserved bytes are zero. The compiler/VMM
decoder rejects wrong identity or version, unsorted/duplicate/inconsistent
directory entries, unknown required kinds or opcodes, nonzero reserved bytes,
overflow, overlap, misalignment, noncanonical order, and digest mismatch.

### 4.2 Placement and capacity

`frameprog` is immutable and 64-byte aligned. `pixelsdata` is zero-initialized
mutable renderer state and 64-byte aligned. Both follow ordinary rtdata and do
not change the machine-v1 device ABI.

Mutable state is compiler-sized and allocation-free: coefficient and frame
input snapshots; double-buffered frame complexes; per-core candidate, root,
event, sheet, run, corridor, fixed-q, shading, and transparency workspaces;
probe state; tile ownership; fixed-schema certificate telemetry counters in
diagnostic/conformance builds; and one failure record. A failed rebuild cannot
corrupt the last valid complex; swap occurs only after every tile succeeds.

Telemetry storage is fixed integer counters rather than a log or allocation.
Its enabled or omitted bytes are part of the generated build-mode layout
report, and renderer decision code cannot read it.

The compiler derives and reports all feature, candidate, root, sheet, event,
run, corridor, transparency, stack, shading, and probe capacities. Authors do
not provide runtime completeness capacities. A successful build proves the
encoded widths and total image memory fit. Diagnostic telemetry bytes derive
from versioned schema enum counts, never observed scene data.

The machine-v1 structural ceilings are versioned constants:

| structural capacity | ceiling |
|---|---:|
| maximal smooth object instances | 1,024 |
| fused feature slots, including instance-transformed slots | 2,048 |
| repeated object instances | 1,024 |
| packed parameter slots | 4,096 |
| hard-CSG stack depth | 256 |
| immutable local-index bytes | 64 MiB |
| structural event records | 1,048,576 |
| run records per tile row | 1,048,576 |
| repeat-analysis candidate instances | 1,000,000 |
| scalar/field/material structural depth | 1,024 |
| per-root dyadic event isolation depth | 2 |
| total mutable renderer state | 536,870,912 bytes |

The compiler may derive a smaller exact count and may share immutable feature
templates between repeated instances. It may not silently clamp a derived
count to these ceilings; an excess is `P015` with its exact why-chain.
Per-generator event storage is the proved root-count bound multiplied by
`2^2` dyadic leaves. Root count and isolation depth are separate contributors:
close roots may consume the full depth, and failure to isolate within it is a
render error that prevents presentation rather than an unreserved allocation.

### 4.3 Canonical record details

The distinct dense ID domains are `ScalarId`, `Vec3Id`, `FieldId`,
`ObjectId`, `FeatureId`, `MaterialId`, `ParamSlotId`, `EventId`,
`ProgramId`, `TextureId`, and `ProbeSetId`. Encoded tables never substitute an
untyped integer for one of these identities.

The scalar graph is append-only. Its closed operations are constants,
parameter and coordinate loads, arithmetic, explicit comparison predicates,
min/max/abs/clamp, bit-defined square root and reciprocal, vector
construction/selection, dot products, lengths, and the closed trigonometric
form used by `sinusoidal_displace`. It has no general control flow,
value-called functions, memory mutation, or implicit host math.

`Length3` remains a fused scalar operation. It must not decompose into
`sqrt(x*x + y*y + z*z)` in derivative or range programs: its derivative bound
uses Cauchy–Schwarz and must remain defined at zero.

The closed structural field kinds are:

```text
Plane Sphere Box RoundBox Capsule FiniteCylinder FiniteCone Torus
Translate Rotate RigidTransform UniformScale
FiniteRepeatX FiniteRepeatY FiniteRepeatZ
Union Intersection Subtract
SmoothUnion SmoothIntersection SmoothSubtract
Mark BoundedDisplace
```

Primitive records carry canonical operands and a source span. Feature records
decompose primitives into a finite set of algebraic or Taylor-bounded sheets
plus explicit validity predicates. Feature kinds cover plane and sphere
sheets; box faces, edges, and corners; capsule bodies and caps;
cylinder/cone bodies and caps; torus sheets; repetition boundaries; and
bounded-deformation sheets. A feature identity is stable only on a domain
where both validity and owning object identity are certified.

Hard CSG is an occupancy program over maximal smooth objects. Smooth
operators belong inside those objects. Hard union, intersection, and
subtraction determine occupancy and winning identity between them. `mark`
contributes exact object and nominal material identity without changing the
scalar.

Every referenced runtime coefficient becomes a stable field/index access path
into `P`, with layout offset, scalar kind, outward range, and optional rate
contract. Only referenced paths are snapshotted. Each construction carries:

```text
ProofMeta:
    value: optional outward f64 interval
    world_bounds: optional Aabb3
    lipschitz: optional f64
    gradient_norm: optional outward interval
    hessian_norm: optional outward interval
    third_derivative_norm: optional outward interval
    smooth_support: f64
    identity_set: finite identities
    finite: bool
```

Host `f64` results are analysis aids rather than proof records. Authority
begins only after outward conversion into an encoded fixed domain.

For polynomial smooth minimum, the exact support rules are:

```text
min(a,b) - k/4 <= smooth_min(a,b,k) <= min(a,b)
support(leaf) = 0
support(smooth_min(a,b,k)) = k/4 + max(support(a), support(b))
bulge(g,k) = (k-g)^2 / (4k), when 0 <= g <= |a-b| <= k is certified
```

Without a certified gap lower bound the compiler uses `k/4`. A
`SmoothObjectRootProgram` names the maximal object, its complete scalar root,
primitive candidate slabs, support certificate, and derived isolation
capacity.

The direct projective forms include:

```text
plane:
Phi = q * (dot(n, eye) + c) + dot(n, r)

sphere, a = eye - center:
Phi = (dot(a,a) - radius^2) * q^2
    + 2 * dot(a,r) * q
    + dot(r,r)
```

Other algebraic features use the sparse-polynomial homogenizer and are checked
against scalar evaluation. Primary certificates never normalize the camera
ray.

Events are analytic conics, sparse polynomials, or Taylor-bounded predicates.
Static interaction edges exist only when conservative world/projected bounds
and q ranges may overlap. Runtime active covers name included objects and
features, active events, exclusions, and the complete q domain. Exclusion
reasons are projected-bounds disjointness, support-shell disjointness,
q-range disjointness, false CSG influence, false feature validity, or
parameter-box disjointness, plus global parameter-box or spatial-box strict
sign.

P4 q-order candidates contain geometry features only: material-boundary
families update material state on an existing sheet and never become geometry
competition subjects. Opaque and potentially transmitting geometry remain
mutually order-compatible because later transparency uses the same ordered
surface sequence; opacity alone therefore cannot justify dropping a geometry
pair. Same-feature diagonal pairs are suppressed by canonical unordered-pair
construction. For hard CSG, small Boolean supports are checked exhaustively
for a state in which both object cofactors differ; a proven absence is an
audited exclusion, while supports above the versioned exhaustive ceiling
retain the pair.

The frame-program directory entry is exactly 16 bytes and encodes
`kind: u16`, `record_bytes: u16`, `count: u32`, `offset: u32`, and
`byte_len: u32`. Every renderer has exact program/state base and byte size
plus exact per-core workspace placement.

Derived maxima cover features per object; candidate objects/features per
tile; row-start roots; simultaneous sheets; event intervals, runs, and
singular corridors per row; transparency layers; root/event stack nodes;
shading terms; and probe invalidations per frame. Global maxima are permitted
until tighter proofs exist. Undercounting is never permitted.

## 5. Runtime mathematics

`Iv32 { lo, hi }` interprets endpoints in a compiler-selected
`FixedDomain { frac_bits, min_raw, max_raw }`. Addition and subtraction are
checked. Multiplication uses four `i64` products and outward floor/ceil shift.
Division uses a separately certified reciprocal interval. Conversion from
`f32` uses the exact IEEE value and outward integer rounding. Invalid domains
and overflow are unresolved, never saturation.

At each row start, the renderer isolates every object root over
`[1/far, 1/near]`, front to back in decreasing q. Interval exclusion discards
root-free boxes. Monotone sign-bracket contraction or Krawczyk contraction
isolates roots. Tangent or near-multiple roots that cannot be separated become
event corridors. Exhaustion returns `RootIsolationExhausted`.

Before complete-object isolation, compiler-emitted leaf sublevel slabs
partition the q domain. For polynomial leaves, the fixed Bernstein
sign-variation/subdivision kernel isolates boundaries of
`leaf - accumulated_support_budget`; analytic primitive roots may refine those
proposals. Leaf zeros alone never establish completeness. For a supported
polynomial q interval, Bernstein range and proved sign-variation/root-count
predicates run before generic interval/Taylor evaluation. An inconclusive
count continues with contraction or subdivision and never labels a partial
root list complete.

Inside a candidate run, implicit derivatives propose a quadratic q sheet. The
certificate proves one root in its tube, feature validity, stable identity,
strict separation from competing sheets, and a complete active cover. It also
proves empty q slabs cannot contain an untracked root unless covered by an
active tangency corridor. Failed proofs shorten or split the run; tolerances
are never widened.

Larger q is nearer. Adjacent strict interval order implies total order.
Ordinary runs never cross an event corridor. Corridors use bounded local
isolation and analytic coverage; unresolved output prevents presentation.

Silhouette coverage uses certified line or quadratic segments and analytic box
filter integration. Positional and curvature uncertainty is converted to
color uncertainty through local contrast. MSAA and TAA do not establish
correctness.

Certified runs use checked fixed-q forward differences with resets no farther
than 64 pixels. Their certificates include coefficient quantization,
recurrence error, and overflow freedom. Ordinary normals come from q-sheet
derivatives; exact field gradients handle corridors or excessive normal cones.

Material and lighting summaries are constant, polynomial, tensor, optional
verified low-rank, or dense. Every summarized form is accepted against the
exact graph with an interval residual bound.

Transparency uses premultiplied radiance and residual transmittance:

```text
(C1,T1) compose (C2,T2) = (C1 + T1*C2, T1*T2)
```

A suffix is dropped only when bounded remaining radiance times current
transmittance fits its assigned encoded error. Point/directional visibility is
certified. Rectangular/disk lights use deterministic adaptive integration. AO
uses bounded field taps. GI uses a deterministic fixed-capacity world-space
probe clipmap with compiler-known update and invalidation rules.

The final interval pipeline is geometry and coverage, material, lighting,
transparency, exposure, color transform, monotone tone/transfer tables, then
u8 quantization. A channel is fixed only when both interval endpoints encode
to the same byte; otherwise deterministic refinement continues or the frame
fails.

Kinetic transport stores derivative and slack bounds for parameters, events,
roots, q order, identity, shading, transfer, and fixed-point state. Reuse is
legal only when the complete perturbation bound is strictly below stored
slack. Static framebuffer reuse additionally requires exact equality digests
covering all geometry, camera, light, material, probe, exposure, transform,
table, extent, dither-policy, and renderer-program inputs.

### 5.1 Fixed-domain rules

Square and absolute value use exact sign cases. `lo <= hi` is mandatory. Hot
SIMD groups share one domain rather than carrying per-lane exponents. World
coordinates, q, q derivatives, field residual, radiance, coverage, and proof
slack use separate compiler-selected domains.

The row-start isolation stack pushes the farther half before the nearer half,
so the larger-q domain is processed first. A domain is discharged only by
strict zero exclusion, a monotone opposite-sign bracket and contraction, or a
strictly smaller Krawczyk contraction. Reaching minimum width unresolved is an
exhaustion error. Root results are sorted and merged only after disjointness is
proved.

### 5.2 Run proposal and acceptance

For `G(x,q)=0`, candidates use:

```text
q_x  = -G_x / G_q
q_xx = -(G_xx + 2*G_xq*q_x + G_qq*q_x^2) / G_q
q_hat(dx) = q0 + q_x*dx + 0.5*q_xx*dx^2
```

Acceptance evaluates both correction-tube boundaries, proves `G_q` excludes
zero, requires opposite strict boundary signs, proves feature validity and
identity, separates every competitor, and retains the complete active cover.
Parametric Krawczyk is allowed only when its image lies strictly inside the
correction interval.

For an algebraic or Taylor polynomial whose composed tube faces remain inside
the sealed degree and term shapes, the preferred representation is Bernstein
coefficient form. A generated checked-dyadic schedule composes
`G(x,q_hat(x)-eps)` and `G(x,q_hat(x)+eps)`, widens every affected coefficient
by candidate-conversion and Taylor remainder radii, proves the two face signs
by complete coefficient scans, and uses de Casteljau subdivision when a hull
is inconclusive. These are integer verifier kernels. Floating FMA or dot
products may propose coefficients but cannot accept a certificate.
Subdivision tightens a hull; it does not reduce degree. Unsupported shapes or
checked arithmetic failure use the ordinary interval/Taylor tube rather than
rejecting an otherwise supported renderer.

Continuation alone is insufficient. Every run starts from complete q-domain
isolation and proves each tracked root remains regular; every intervening q
slab excludes zero or belongs to a tangency corridor; and all support,
projected-bound, validity, repetition, identity, and material events stay away
from zero. The earliest expiring condition ends the run. A winner requires
`winner.lo > competitor.hi` after all root and quantization errors.

Analytic event roots are enclosed outward. General events use interval
subdivision and derivative contraction. Overlapping roots merge into one
corridor widened by curve-position and fixed-point error.

### 5.3 Coverage, recurrence, and normals

Line coverage clips a polygon against the pixel square. Quadratic coverage
uses a Green's-theorem boundary integral. A segment splits whenever its
remainder can exceed the assigned output-code error.

The scalar fixed-q recurrence is:

```text
q  += d1
d1 += d2
```

For a four-pixel packet:

```text
q4_delta = 4*d1 + 6*d2
delta_step = 16*d2
```

No update may overflow before the next certified reset.

For camera-space `P(u,v)=(u/q,v/q,1/q)`, an unnormalized normal is
proportional to:

```text
N = (q_u, q_v, q - u*q_u - v*q_v)
```

The exact field gradient replaces this proposal in corridors or when the
normal-cone error exceeds the material allocation.

### 5.4 Shading, transparency, and output bytes

The v1 material model is energy-conserving Lambert diffuse, one GGX-style
glossy lobe, emissive radiance, scalar opacity, and filtered normal/slope
moments. Lights are directional, point, rectangular area, or disk area.

The exact transparency tail condition is:

```text
current_transmittance * max_remaining_radiance <= assigned_encoded_error
```

The radiance bound comes from material/light proofs, never a heuristic.
Area-light integration works over emitter coordinates with an interval
remainder. AO uses four or five field-distance taps valid for their own
spatial domains. Probe updates and invalidations are deterministic; culling a
probe contribution requires a transfer-sensitive encoded-error proof.

The compiler proves tone and transfer tables monotone. Refinement selects the
largest remaining contributor under the deterministic exact error/cost
ordering.

#### 5.4.1 Working color and output transfer

Scene radiance, material RGB, and light RGB use linear Rec.709/sRGB primaries
with a D65 white point. Authored material and light channels are finite and
nonnegative. Material base color is in `[0,1]`; a signed procedural
intermediate is legal only when its certified final range is nonnegative.
Radiance bounds retain negative roundoff conservatively and clamp to zero only
at the input of tone mapping.

`FilmicV1` is the sealed 4097-entry u16 table over log2 radiance `[-16,+16]`.
Its canonical source formula is

```text
A=.15 B=.50 C=.10 D=.20 E=.02 F=.30 W=11.2
h(x)=((x*(A*x+C*B)+D*E)/(x*(A*x+B)+D*F))-E/F
f(x)=clamp(h(x)/h(W),0,1)
```

The runtime does not evaluate that formula. Zero selects code zero directly;
positive values clamp to the log2 domain, then use fixed-point piecewise
linear interpolation. `srgb_v1_u16.bin` is likewise a sealed 4097-entry u16
table over `[0,1]`. Its output is quantized to u8 with the specified integer
ties rule. The canonical SHA-256 values are
`834b92da2dc0efaa7ffeee438f95a9de53988abcfa0d122f55329ec01e1ebf6f`
and `28c6391387185672fd824973e342a185f7cc90d487be3d966821412509213201`
respectively. Length, endpoints, monotonicity, domains, interpolation tag,
and digests are verified as one numeric-revision contract.

Frame exposure is expressed in stops and must lie exactly on the sealed
`1/256`-stop lattice. Validation rejects an in-range value between lattice
points; runtime multiplication rechecks the lattice before using the fixed
binary-root product.

#### 5.4.2 Standard material

`MaterialSample.standard` contains base color, metallic, roughness, dielectric
specular scale, emissive radiance, opacity, and closed normal detail. Metallic,
specular, and opacity are in `[0,1]`; roughness is in `[0.02,1]`; emissive is
finite, nonnegative, and profile-bounded. No unlisted lobe exists in v1.
For unit normal `n`, view `v`, light `l`, and half vector `h`:

```text
alpha = max(roughness^2, 0.0004)
D_GGX = alpha^2 / (pi * (NoH^2*(alpha^2-1)+1)^2)
lambda(x) = (sqrt(1 + alpha^2*(1-NoX^2)/max(NoX^2,1e-12))-1)/2
G2 = 1 / (1 + lambda(v) + lambda(l))
F0 = mix(0.08*specular, base_color, metallic)
F = F0 + (1-F0)*(1-VoH)^5
Fd90 = 0.5 + 2*roughness*VoH^2
Fd = (1+(Fd90-1)*(1-NoL)^5) * (1+(Fd90-1)*(1-NoV)^5)
BRDF = base_color*(1-metallic)*(1-F)*Fd/pi
     + D_GGX*G2*F/max(4*NoV*NoL,1e-12)
```

The `1e-12` floors are domain guards for grazing-direction division and the
GGX rational denominator. They do not hide a failed interval: a normal cone
crossing the domain boundary is split or conservatively bounded. Reflected
radiance multiplies `BRDF*NoL`; emissive is added afterward. Opacity belongs
to ordered transfer and never scales geometry coverage.

#### 5.4.3 Textures and normal moments

V1 compiler-owned textures are immutable `Rgb8Srgb`, `Rgb8Linear`,
`Rg8Snorm`, or `R8Linear` assets. Their canonical record contains every mip to
1x1, per-mip signed-aware channel extrema, a per-texel Q16.16 pyramid of slope
first/second moments when applicable, wrap tags, and a SHA-256 identity. Each
parent moment texel is the deterministic average of its actual children; a
whole-level average is not a legal substitute. Filtering is bilinear within a
mip, trilinear between adjacent mips, and four equal taps at
`[-3/8,-1/8,1/8,3/8]` along the major footprint when anisotropy exceeds two;
the major footprint is capped to anisotropy four. Mip choice derives from the
certified UV derivative footprint. V1 selects the octave by exact
powers-of-two comparisons and uses the sealed linear coordinate within that
octave; its interval enclosure includes adjacent levels for outward rounding.
Clamp and repeat are the only wraps. A repeat
integer crossing is an explicit seam event.

UV mappings are closed: analytic plane/sphere/cylinder/torus, feature-local
box/round-box, and object/world triplanar mappings. There is no runtime UV
callback. The closed `MaterialSample.textured` and `TextureSlope` constructors
use world triplanar projection unless a compiler-known primitive mapping is
sealed; the dominant geometric-normal axis uses x/y/z tie order, and the same
projection carries the certified world-space footprint. Screen coordinates
are never substituted for UV. Slope filtering carries `E[sx]`, `E[sy]`, `E[sx^2]`, `E[sx sy]`,
and `E[sy^2]`; the covariance must be positive semidefinite. It produces a
mean normal, variance roughness adjustment, and BRDF curvature residual. An
over-budget residual subdivides or uses a bounded deterministic tap set. The
v1 terminal set for the sealed 2x2 slope assets evaluates all four signed
base-level slopes in row-major order and averages their BRDF/source bounds
with exact quarter weights. This terminal never substitutes the mean normal
or drops the curvature remainder while continuing to use the moment proposal.

Feature-local mappings receive the exact originating feature ID from the
certified visibility result. The generated evaluator replays that feature's
authoritative source-order translation, rigid rotation, and fixed repeat
instance onto the point, geometric normal, and both footprint derivatives
before analytic, feature-local, or object-triplanar coordinates are evaluated.
Uniform distance scaling does not move coordinates. World triplanar
deliberately retains the world point and derivatives. A missing/malformed
feature or transform record is a render failure, never a fall back to screen
or world coordinates.

#### 5.4.4 Lighting, secondary visibility, AO, and summaries

Directional lights carry a normalized direction and radiance. Point lights
use `1/max(r^2,radius^2)` and require `radius >= 2^-12`, so the squared clamp
cannot underflow to a singular denominator. Rectangle lights carry a center
and two orthogonal half-axis vectors whose lengths are the half extents. Disk
lights carry a center, normal, and radius; the constructor deterministically
normalizes the normal and lowers it to two orthogonal, equal-radius half axes
in the sealed runtime record. Every sealed
light has a kind and an explicit per-slot `LightRange`: ordered position
minimum/maximum, maximum absolute source-axis component, per-channel maximum
radiance, and maximum frame delta. The frame range/rate contract, world and
influence bounds, and maximum incident-radiance bound are serialized in the
verified program. Range and slot-kind checks run before snapshot publication.
After the first frame, all
15 scalar lanes of each light are compared with the preceding renderer
snapshot using the submitted frame-index distance; a non-increasing index or
an absolute lane delta above `max_delta * frame_steps` is rejected. Values
outside that contract reject the frame; the rate is
only a reuse premise, so an otherwise valid out-of-rate frame is evaluated
from scratch.

Secondary queries traverse a compiler-emitted, median-split surface-object
BVH. Splits use greatest centroid extent (ties x, y, z) and stable object ID.
Leaves retain object and feature ranges. Complete feature root isolation,
validity, q ordering, and CSG occupancy determine clear or the first blocker;
any capacity, ordering, or numeric ambiguity is unresolved. Only the exact
originating feature inside the certified normal-offset corridor is excluded.
BVH endpoints are converted to f32 outward by the compiler, so guest slab
tests require no guessed padding. The origin epsilon is derived from eight
f32 ulps at the reconstructed point scale plus four sealed fixed-q quanta;
the runtime uses that same value for its offset and exclusion corridor.

Rectangle/disk integration uses dyadic Morton-order source cells. The disk
uses the concentric map with Jacobian `pi/4`. Whole-cell secondary visibility
first produces a complete 8x8 mask and a canonical 32x32 terminal mask. The
lighting certificate retains the lowest-index live area-light slot for the
global queue's sealed density terminal. Only that slot's unresolved 32x32
parents subdivide to an 8x8 grid each, giving exact clear and possible counts
on the canonical 256x256 source grid. At most 62 unresolved parents fit the
fixed 31-word payload; exceeding that count or any work bound reports
`CertificateExhausted`.

For a selected parent, shading evaluates four canonical 64x64 contribution
children and retains their minimum and maximum source density. Multiplying
those bounds by the recorded clear/possible fractions encloses every spatial
allocation of visibility within the parent. The midpoint visibility is only
the deterministic candidate; it has no proof authority. A cell is accepted
only when the resulting whole-cell contribution interval fits its dyadic
share of the encoded budget. Other area lights retain their complete coarse
bounds and are never silently promoted by the selected source's payload.

AO uses distances `[1/16,1/8,1/4,1/2,1]*ao_radius`, weights
`[.40,.25,.16,.11,.08]`, and
`occ_i=clamp((s_i-max(distance_lower,0))/s_i,0,1)`. Its complete result is
`clamp(1-ao_strength*sum(weight_i*occ_i),0,1)` with reversed interval
endpoints because occlusion decreases with distance. Each tap first queries
the surface-object BVH over its complete radius cube; an empty candidate set
proves zero occlusion, while a nonempty set runs the active semantic distance
program. The same evaluation carries its exact source-language f32 candidate.
If the real-interpretation enclosure still spans output codes, the `Ao`
terminal rung consumes that deterministic five-tap candidate and removes only
the enclosing evaluator-rounding residual.

Material and shading summaries form the fixed ladder constant, affine-x,
quadratic-x, separable rank at most four, and exact pixel. A summary is usable
only with an a-posteriori HDR residual. Cross pivots use the fixed 5x5 grid,
greatest residual upper bound, then y/x ties. Packet candidates use the same
coefficients as scalar evaluation and retain HDR intervals until encoded
endpoints agree.

#### 5.4.5 Display refinement and opaque storage

Every unresolved output unit reports an error source, a conservative integer
lower bound on code-span reduction, static operation count, remaining discrete
depth/current code span, and stable payload ID. Zero is the only permitted
lower bound when a rung proves depth progress but not an immediate encoded-span
decrease. The queue compares reduction/cost by integer cross multiplication;
ties use source enum then payload ID. Applying an option must strictly decrease
`(remaining_depth, interval_width)` lexicographically or terminate. If endpoint
codes differ after the queue is exhausted, rendering fails with
`CertificateExhausted`; the candidate is never rounded to a nearby byte. Opaque
storage writes exact B, G, R singleton codes and alpha 255.
Coverage events composite HDR front/back intervals before tone mapping. The
old visibility colors remain available only to compiler-internal conformance
mode and cannot be selected by source code.

The exact per-pixel rung may call the encoder directly only when every active
contributor has already been evaluated deterministically and the HDR lower,
candidate, and upper values are bit-identical. This is the zero-width terminal
case of the same verifier, not an unchecked nearest-byte fallback. Any nonzero
span must enter the refinement queue or fail `CertificateExhausted`.

### 5.5 Temporal reuse

Regular-sheet temporal proposals use `q_t = -G_t/G_q`. Reuse validates actual
parameter deltas, event signs, root tubes, adjacent q order, feature and
identity stability, shading/post bounds, and fixed-point margins. Diagnostics
retain the component owning minimum slack. A failed margin rebuilds the tile
or the full frame from scratch.

A condition or homotopy-style estimate may propose a temporal center,
refinement order, or carry length. It has no authority: acceptance still uses
the complete dyadic first/second-order remainder and every event, root, order,
identity, shading, transfer, and quantization margin. Diagnostics classify
scheduled expiry, isolated transverse events, simultaneous or degenerate
events, dependency or numeric invalidation, and event storms. A storm label
never makes a correctness decision; sealed current work bounds choose between
local repair and the equivalent full sweep.

### 5.6 Certificate telemetry

Beginning with complete run construction, diagnostic and conformance builds
record stable-ID counters for:

- run-length histograms and regular/event-corridor pixel fractions;
- root-certificate method and composed polynomial degree/term shape;
- run-ending cause and minimum-margin owner;
- active feature, sheet, event, and predicate counts;
- leaf-sublevel and active smooth-cluster sizes;
- root/event subdivision depth and bounded-rebuild terminal reason;
- numeric domain/overflow and refinement causes.

Workers own fixed local counter arrays and merge them in tile-ID order.
Telemetry is report and conformance evidence only: the renderer cannot read it
to choose a proof, vary quality, or admit a frame. P8 locks the schema and
adversarial visibility corpus. P12/P13 may add versioned non-regression
thresholds, but histograms never replace or relax exact structural workload
and per-frame cycle admission.
