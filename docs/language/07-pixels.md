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
cross-multiplied display-error reduction over estimated cost and recomputes
after every accepted refinement. No independent approximation ratio is part
of the contract.

Lean proves generic mathematics. Build-time Rust constructs concrete facts,
stable dumps expose them, and generated guest verifiers consume the encoded
records. Lean is not invoked by an ordinary Wrela build.

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
enums need no numeric range.

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

Local events include projected-bound entry/exit, feature validity, tangency,
smooth-band, identity, depth-order, repeat, and material boundaries. An
omitted object, feature, or event has a domain-scoped exclusion record with
strictly positive slack. Domain splits regenerate invalidated exclusions.
Global parameter-box and spatial-box strict-sign exclusions remain valid
across all allowed parameter motion. Their proof payload records the
normalized box, polynomial program, Bernstein degree and coefficient order or
subdivision tree, outward conversion radius, sign, and minimum margin. A
failed supported-shape proof emits the ordinary runtime predicate instead.

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
