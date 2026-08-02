# Wrela Pixels compiler and runtime implementation plan

**Status:** IMPLEMENTATION — no experimental lanes, no fieldprobe dependency, no unresolved research tasks.

**Repository basis:** `rywible/wrela8`, branch `master`, commit `a784076ccc97586132f47152b9a010f5b9574a4e` (2026-08-01).

**Primary owner:** `crates/wrela-compiler`.

**Runtime owner:** Wrela standard library under `stdlib/core/` and the machine-v1 display driver under `stdlib/drivers/`.

**Formal owner:** `formal/pixels/`, a pinned Lean project outside the Cargo dependency graph.

**Historical evidence:** `crates/wrela-fieldprobe` remains checked in as the record that rejected the sample-first quadtree certificate. No task in this plan changes fieldprobe or uses dense truth to make an online decision.

---

## How to execute this plan

This document is intentionally written for an implementation agent that should not need to choose architecture, invent proof policy, or infer task ordering.

Execution rules:

1. Work milestones and tasks strictly in numerical order. A later task may rely on every earlier task's stable dump and invariant.
2. Treat each `Task Px.y` as one commit unless that task explicitly permits otherwise.
3. Start each task by reading its **Purpose**, **Files**, **Work**, and **Acceptance criteria** in full. Do not implement from the task title alone.
4. Run the task's stated gate before committing. Run `cargo xtask verify-milestone` at every milestone close.
5. Never skip a stable-dump task. The dumps are the compatibility boundary between compiler stages and the forensic record when a later invariant fails.
6. Do not begin runtime optimization before the from-scratch validated sweep is correct. Kinetic reuse, SIMD, and hardware admission are deliberately downstream.
7. Section 13 is the invariant ladder. Section 14 is the exact commit order. Section 15 is a hard prohibition list.

Milestone map:

| milestone | primary deliverable | tasks |
|---|---|---:|
| P0 | normative contract, compiler/formal scaffolding, permanent fixtures | 4 |
| P1 | typed `@field`/`@material` source surface and sealed `Image.renderer` declaration | 8 |
| P2 | deterministic dedicated symbolic field/material compiler | 7 |
| P3 | structural bounds, smooth support, objects, fused features, capacities | 12 |
| P4 | projective inverse-depth programs and complete local event/exclusion system | 12 |
| P5 | binary `FrameProgram v1`, image placement, generated renderer actors | 11 |
| P6 | verified dyadic/numeric kernels and Lean–Rust–Wrela correspondence | 12 |
| P7 | correct from-scratch validated scanline sweep | 15 |
| P8 | fixed-q raster, analytic coverage, display-byte output, replay | 11 |
| P9 | deterministic AAA material, texture, lighting, shadow, AO, filtering | 13 |
| P10 | ordered transparency and deterministic probe GI | 10 |
| P11 | optional kinetic proof maintenance with full-sweep byte equivalence | 13 |
| P12 | generated kernel palette, NEON lowering, A76 cost admission | 10 |
| P13 | Pi 5 release conformance, normative activation, ownership closure | 10 |

The first production-capable correctness boundary is the end of P8: a field scene can be compiled, swept from scratch, rasterized, and presented without temporal state. P9–P10 establish the full-quality image contract. P11 reduces recurring work without changing bytes. P12–P13 establish that the generated implementation fits and sustains the target machine profile.

## 0. What this plan delivers

This plan delivers one production renderer, not a portfolio of alternatives:

> Wrela compiles a field-authored scene into a sealed frame program. A validated scanline sweep constructs the visible structure from scratch. The renderer preserves that proof between frames while its certificates remain valid. Lighting, filtering, transparency, and final quantization operate on the certified structure. Any proof failure rebuilds a bounded region or refuses to present the frame; it never guesses.

The completed source experience is:

```wrela
module game.scene

from core.field import (
    Field,
    Vec3,
    plane,
    round_box,
    capsule,
    torus,
    mark,
    smooth_union,
)
from core.render import (
    Camera,
    MaterialSample,
    RenderFrame,
    RenderProfile,
    ToneCurve,
)

struct SceneParams:
    @range(min=-1.4, max=1.4)
    @rate(max_delta=0.08, max_second_delta=0.02)
    sword_angle: f32

    @range(min=-4.0, max=4.0)
    fighter_offset: Vec3

@field
fn world(p: Vec3, read s: SceneParams) -> Field:
    ground = mark(
        plane(p, Vec3(x=0.0, y=1.0, z=0.0), offset=0.0),
        object=ObjectId.Ground,
        material=MaterialId.Clay,
    )

    torso = round_box(
        p - s.fighter_offset,
        half=Vec3(x=0.28, y=0.52, z=0.16),
        radius=0.10,
    )
    arm = capsule(p, a=arm_a(read s), b=arm_b(read s), radius=0.09)
    body = smooth_union(torso, arm, k=0.08)
    body = mark(body, object=ObjectId.Fighter, material=MaterialId.Porcelain)

    return ground.union(body)

@material
fn shade(surface: SurfaceContext, read s: SceneParams) -> MaterialSample:
    match surface.material:
        MaterialId.Clay:
            return MaterialSample.clay(
                color=Rgb(r=0.40, g=0.20, b=0.12),
                roughness=0.82,
            )
        MaterialId.Porcelain:
            return MaterialSample.porcelain(
                color=Rgb(r=0.92, g=0.88, b=0.78),
                roughness=0.16,
            )
```

The image binds that scene explicitly:

```wrela
@image
fn image() -> Image:
    img = Image(name="arena", target=Target.wrela_machine_v1, cores=4)

    display_device = img.device[DisplayDevice](transport=Transport.Blob)
    display = img.driver(
        drivers.Display,
        device=display_device,
        vector=5,
        mailbox=2,
    )

    renderer = img.renderer[SceneParams](
        field=world,
        material=shade,
        display=display,
        width=1920,
        height=1080,
        refresh_hz=60,
        shade_hz=30,
        profile=RenderProfile.AaaByteExact,
        tone_curve=ToneCurve.FilmicV1,
        near=0.05,
        far=200.0,
        world_min=Vec3(x=-64.0, y=-16.0, z=-64.0),
        world_max=Vec3(x=64.0, y=64.0, z=64.0),
    )

    game = img.actor(
        Game,
        mailbox=8,
        renderer=renderer.handle(),
    )
    img.on_failure(policy=Failure.Halt)
    return img.seal()
```

`img.renderer[P](...)` declares a standard-library `Renderer[P]` actor instance. Its public method is:

```wrela
pub async fn render(
    take frame: RenderFrame[P],
) -> Result[RenderedFrame[P], RenderError]
```

`RenderFrame[P]` owns `P` for the duration of the render, so the renderer can snapshot only the coefficient paths actually referenced by `@field` and `@material` without copying the whole value. `RenderedFrame[P]` returns ownership of `P` after presentation. The method has no hidden allocation and no implicit shared mutation.

### 0.1 User-visible result

The flagship result is a deterministic 1080p60 software renderer with:

- field-authored hard and smooth geometry;
- exact surface and material identity;
- vector-crisp analytic silhouette coverage;
- stable normals and filtered microdetail;
- deterministic direct lighting, glossy response, AO, soft shadows, and probe GI;
- ordered transparency with a certified radiance-tail cutoff;
- no stochastic sampling or denoising;
- no temporal ghosting as a correctness mechanism;
- no runtime allocation;
- no GPU or host renderer;
- exact replay frame digests.

The VMM still only scans out guest-owned tile pages. The guest constructs, shades, rasterizes, and presents every output pixel.

### 0.2 Definition of done

The renderer is complete only when all of these are true:

1. A Wrela image can declare a renderer through `Image.renderer` and boot it through the ordinary image/layout pipeline.
2. `@field` and `@material` functions have stable typed dumps, deterministic compiler artifacts, and focused diagnostics.
3. Every flagship field operation has an exact scalar meaning and a conservative compiler proof rule.
4. The compiler emits a versioned `FrameProgram v1` data section and exact mutable renderer-state placement.
5. The guest can construct a frame from scratch with the validated sweep without dense truth, a sample lattice, a dense edge mask, or previous-frame state.
6. Kinetic reuse is only an optimization; disabling it produces the same displayed bytes.
7. Every accepted visibility run proves root existence, root uniqueness, identity stability, and front order for its complete domain.
8. Every approximation in coverage, shading, transparency, post, and temporal transport is either proven unable to change the stored output code or is refined/falls back.
9. A proof or capacity failure prevents presentation and returns `RenderError`; it never becomes background, a stale hit, or a guessed color.
10. The compiler report publishes frame-program bytes, renderer-state bytes, per-core placement, exact capacity derivations, fallback classes, and generated hot functions.
11. The Lean project builds with no admissions and prints no unexpected axioms for the trust-boundary theorems.
12. The Rust compiler reference, generated Wrela scalar kernels, generated Wrela SIMD kernels, and host oracle agree on all permanent differential fixtures.
13. The machine-v1 display conformance lane presents the exact expected frame digests.
14. The flagship Pi 5 acceptance image presents 1080p60 with no missed vsync, no unresolved frame, no thermal throttling, and no output divergence during the locked acceptance workload.

Items 13–14 are release conformance, not research lanes. They do not choose algorithms or tune tolerances. The algorithms and tolerances in this document are fixed before those gates run.

---

## 1. Architectural decisions that are closed

These are implementation decisions. An executor must not reopen them inside a task.

### 1.1 No second executable IR

`FieldGraph` and `FrameProgram` are compiler-owned data structures, not a fourth Wrela executable IR.

- Source functions still type-check into the existing typed program.
- Ordinary renderer runtime code is Wrela and lowers through the existing FlowWir → MachineWir → AArch64 path.
- `FieldGraph` exists only while compiling `@field` and `@material` roots.
- `FrameProgram v1` is immutable image data read by the stdlib renderer.
- The fallback field evaluator is a compact data program, not executable compiler IR.

Do not add field nodes to FlowWir or MachineWir. Do not serialize `FieldGraph` as a compiler cache format.

### 1.2 Dedicated symbolic evaluator, not `eval::Value::Symbolic`

The earlier design suggested adding `Symbolic(node)` to the generic comptime evaluator. Do not do that in this repository.

Create `crates/wrela-compiler/src/pixels/symbolic.rs`, a dedicated interpreter over `TypedExpr` and `TypedStmt`. It may reuse small scalar helpers from `eval::value`, but it has its own value domain, call stack, quota, parameter taint, and intrinsic table.

Reason: image evaluation, const evaluation, exhaustive tests, and field compilation have different legality rules. Keeping field symbolism out of `eval::Value` avoids making every comptime match exhaustive over renderer-only states.

### 1.3 The field source type is opaque `Field`, not naked `f32`

`@field` returns `core.field.Field`. `Field` has scalar runtime representation but private construction. Authors compose it through the closed `core.field` API.

This prevents unsupported arithmetic from silently destroying surface structure. The symbolic evaluator never has to guess whether `a + b` means field displacement, density addition, or an ordinary scalar expression.

### 1.4 The frame program is structural and semantic

The compiler emits both:

- a structural surface/object/feature representation used by the renderer; and
- a scalar semantic tape used for exact fallback and differential validation.

The structural representation may propose candidates and prove specialized cases. The scalar tape defines the source semantics when a specialized proof is unavailable. A fused primitive must remain differential-equivalent to its source runtime implementation within the language’s bit-exact math contract.

### 1.5 Local event completeness, not a global aspect graph

Do not enumerate all visibility cells or build global pairwise resultants for the complete scene.

For each current tile, row band, and parameter box, the runtime builds a finite active event set and carries exclusion certificates for omitted objects/features/pairs. The proof seam is:

```text
all possible surface roots are covered
+ all possible combinatorial changes have active predicates
+ every omitted predicate has a valid exclusion certificate
+ active predicates exclude zero over the run
=> visible combinatorics are fixed over the run
```

Global closed-form event polynomials are allowed only as compact specializations for planes, quadrics, and other low-degree cases. They are not required for correctness.

### 1.6 The validated scanline sweep is the primary renderer

The first frame, a camera cut, a whip, disabled kinetic reuse, or any failed temporal certificate uses the same from-scratch sweep.

A scanline run ends at the earliest of:

- a geometric/event boundary;
- root-certificate expiry;
- q-order expiry;
- material/shading certificate expiry;
- transparency-order expiry;
- fixed-point range expiry;
- tile boundary.

Inside a run, no field root is solved. The renderer advances fixed-q and shading state by forward differences and stores pixels.

### 1.7 Kinetic maintenance is never required for correctness

The persistent `FrameComplex` is reused only while complete event, parameter, q-order, identity, shading, and quantization margins remain positive. A failed certificate rebuilds a bounded region. A frame-wide invalidation runs the full sweep.

There is no “validate a few rays and hope.”

### 1.8 Candidate arithmetic and proof arithmetic are separate

- Candidate construction uses bit-defined `f32`/`f64` calculations and jets.
- Acceptance uses conservative dyadic intervals with integer endpoints.
- An approximate value may be arbitrarily clever. It has no authority until the integer verifier accepts it.

The flagship certificate domain is `Iv32`: two `i32` endpoints interpreted through a compiler-selected fixed binary exponent. Products use `i64` intermediates. If the compiler cannot prove all intermediate products fit, it subdivides the static domain or rejects the renderer declaration.

### 1.9 Unsupported flagship operations are build errors

`RenderProfile.AaaByteExact` does not silently lower unsupported geometry to a pixel-density marcher.

An unsupported field operation, unbounded transform, runtime topology branch, unbounded repetition, missing parameter range, or unbounded material discontinuity is a build error naming the operation and source span.

A later compatibility profile may permit local exact fallback. It is outside this plan.

### 1.10 No stochastic dither, stochastic sampling, or denoising in v1

The flagship output transfer is deterministic and monotone. Ordered deterministic dither may be added only when its exact per-pixel offset is included in the byte certificate. The first implementation disables dither.

### 1.11 Soft shadows use certified source integration

Do not implement the earlier GTD/PTD analogy as a universal shadow formula.

- Point and directional lights use certified visibility queries represented and reused over runs.
- Rectangular and disk lights use deterministic adaptive integration over emitter coordinates with interval radiance bounds.
- A compact transition function is accepted only after the emitter-domain integral verifier bounds its error.

### 1.12 Low-rank shading is a proposal, never a premise

The production approximation is a verified polynomial/tensor summary. Optional low-rank compression may reduce storage/work after an a posteriori residual bound passes. Rank 2–4 is not assumed.

### 1.13 The scheduler has no unproved approximation guarantee

Use a deterministic max-heap ordered by exact cross-multiplied `display_error_reduction / estimated_cost`. Do not claim a `1 - 1/e` theorem. Refinements interact, so every pop recomputes the candidate against current neighbors.

### 1.14 Lean proves mathematics; build-time Rust checks compiler facts

Lean does not run inside a Wrela build. It proves generic theorems used by the compiler/runtime design. Rust code constructs concrete proof records and the guest verifier checks them.

The compiler is responsible for the correspondence between a source `@field` program and the emitted records. That correspondence is guarded by stable dumps, differential execution, frame goldens, and explicit theorem-to-kernel maps.

---

## 2. Source and semantic contract

### 2.1 New attributes

Add these attributes using the existing generic attribute syntax. No lexer or parser grammar change is required.

#### `@field`

Allowed only on a top-level synchronous function.

Required signature:

```wrela
@field
fn name(p: Vec3, read params: P) -> Field
```

The `params` argument may be omitted for a static scene:

```wrela
@field
fn name(p: Vec3) -> Field
```

Rules:

- no receiver;
- no generics on the root;
- no `async` or `@task`;
- return type exactly `core.field.Field`;
- first parameter exactly `core.field.Vec3`, ordinary read-by-value;
- optional second parameter exactly `read P`, where `P` is a finite data type;
- no other parameters;
- no effects, hardware access, actors, time, entropy, panic, mutable statics, or allocation;
- no recursion;
- loops only when comptime unrolled over an exact array length;
- every runtime branch depending on `p` or `params` is rejected;
- every function called transitively must have a body available and satisfy the same purity/termination subset;
- field composition occurs only through the `core.field` API;
- object/material identifiers are comptime constants;
- finite repetition counts are comptime constants;
- every runtime coefficient path used by geometry has a finite declared range.

#### `@material`

Allowed only on a top-level synchronous function.

Required signature:

```wrela
@material
fn name(surface: SurfaceContext, read params: P) -> MaterialSample
```

The `params` argument may be omitted. Material code may branch on the compile-time-dense `surface.material` identifier. Other runtime control flow is accepted only if the material compiler can represent both branches and prove their boundary; otherwise `AaaByteExact` rejects it.

#### `@range(min=..., max=...)`

Allowed on numeric fields reachable from a renderer parameter type.

- On `f32`, both endpoints are finite `f32` literals and `min <= max`.
- On `Vec2`/`Vec3`/`Rgb`, the same range applies component-wise.
- On arrays and structs, a field-level range applies recursively only to direct scalar/vector leaves that lack their own range.
- A more specific nested field range wins.
- Integer and enum values do not need a numeric range.
- Every geometry coefficient must resolve to exactly one range.

#### `@rate(max_delta=..., max_second_delta=...)`

Optional. It enables kinetic transport for that path.

- Values are finite, nonnegative `f32` literals in units per rendered frame.
- Missing `@rate` does not reject the renderer; it marks the coefficient `rebuild_on_change`.
- The runtime checks actual deltas before using a kinetic certificate.

### 2.2 Closed field operation set

Create `stdlib/core/field.wr` with ordinary scalar implementations for these operations. The symbolic compiler recognizes their canonical callee keys.

Primitive constructors:

```text
plane
sphere
box
round_box
capsule
finite_cylinder
finite_cone
torus
```

Transforms:

```text
translate
rotate
rigid_transform
uniform_scale
finite_repeat_x
finite_repeat_y
finite_repeat_z
```

Composition:

```text
union
intersection
subtract
smooth_union
smooth_intersection
smooth_subtract
```

Metadata and bounded deformation:

```text
mark
bounded_displace
sinusoidal_displace
```

Every operation has:

1. a scalar Wrela implementation defining source semantics;
2. a symbolic compiler rule;
3. a value/range/derivative rule;
4. a structural bounds rule;
5. a cost rule;
6. a Rust reference implementation;
7. a Lean theorem or a theorem that reduces it to already proved primitives.

`bounded_displace` requires explicit conservative amplitude and gradient bounds. `sinusoidal_displace` derives them from compile-time frequency/amplitude data. Arbitrary `Field + f32` is impossible because `Field` is opaque.

### 2.3 Exact smooth CSG semantics

The standard polynomial smooth minimum is normative:

```text
if a <= b - k: return a verbatim
if b <= a - k: return b verbatim
h = 0.5 + 0.5 * (b - a) / k
return b + (a - b) * h - k * h * (1 - h)
```

Requirements:

- `k` is finite and strictly positive;
- saturated branches return the selected operand bit-for-bit;
- the range rule uses `min(a,b) - k/4 <= smin <= min(a,b)`;
- the gradient rule uses the convex combination inside the active band;
- support budgets accumulate by `k/4`, tightened by a certified branch-gap bound where available.

The standard-library implementation, comptime evaluator, scalar tape, Rust reference, and generated guest code must agree bit-for-bit in saturated branches.

### 2.4 Parameter topology rule

The symbolic evaluator classifies every value as:

```rust
enum Dependency {
    Comptime,
    Coordinate,
    Parameter,
    CoordinateAndParameter,
}
```

A control-flow branch is legal in `@field` only when its condition is `Comptime`. A runtime `if`, `match`, early return, loop bound, array length, or function-selection decision depending on coordinate/parameter data is rejected.

This does **not** reject `min`, `max`, smooth CSG, feature validity, or material selection. Those are explicit dataflow nodes whose boundaries the renderer can represent.

### 2.5 Renderer declaration

Add a compiler-recognized image builder intrinsic:

```wrela
img.renderer[P](
    field=field_fn,
    material=material_fn,
    display=display_driver_decl,
    width=W,
    height=H,
    refresh_hz=R,
    shade_hz=S,
    profile=RenderProfile.AaaByteExact,
    tone_curve=ToneCurve.*,
    near=NEAR,
    far=FAR,
    world_min=Vec3(...),
    world_max=Vec3(...),
)
```

All labels are required in v1. Validation:

- `P` is a finite data type;
- `field_fn` is `@field` and has the same `P`;
- `material_fn` is `@material` and has the same `P`;
- `display` references the one display driver used by this renderer;
- width/height/refresh/shade rates are positive comptime integers;
- `shade_hz` divides `refresh_hz`;
- near/far/world bounds are finite, ordered, and compile-time;
- output extent matches the display declaration;
- only one renderer may own a given display declaration;
- a renderer declaration participates in the image-construction DAG;
- the renderer actor and its internal worker actors receive deterministic core placement.

The call returns `ImageDecl[Renderer[P]]`. `handle()` returns `Actor[Renderer[P]]` and follows the existing image-declaration handle rules.

### 2.6 Runtime result and failure semantics

Add to `stdlib/core/render.wr`:

```wrela
enum RenderError:
    ParameterOutOfRange(RenderPath)
    NonFiniteInput(RenderPath)
    RootIsolationExhausted(TileId)
    EventIsolationExhausted(TileId)
    CertificateExhausted(TileId)
    CapacityMismatch(RenderCapacity)
    Display(DisplayError)
    InternalInvariant(RenderInvariant)

struct RenderedFrame[P]:
    params: P
    frame_index: u64
    displayed_digest: [u8; 32]
    rebuilt_tiles: u32
    reused_tiles: u32
```

No `RenderError` path flushes the partially built back buffer. The previous complete framebuffer remains on screen. The error returns to the caller; the image’s ordinary failure policy decides what the application does next.

Capacity exhaustion should be impossible after a successful build. If observed, it is reported as an internal invariant failure rather than interpreted as a scene miss.

---

## 3. Compiler pipeline and ownership

### 3.1 Pipeline insertion

The whole-image pipeline becomes:

```text
parse all modules
  -> sema and specialization
  -> evaluate and seal @image
  -> validate image declarations
  -> compile every Image.renderer declaration
       @field/@material roots
       -> FieldGraph/MaterialGraph
       -> structural proofs and capacities
       -> FrameProgram v1
       -> generated renderer actor/glue facts
  -> merge generated renderer roots into reachability
  -> FlowWir / MachineWir lowering of ordinary Wrela runtime
  -> codegen
  -> layout code, rodata, rtdata, frame-program data, renderer state
  -> image report and sealed digest
```

`pixels::compile_all` runs after `eval::image_checks::check_sealed` has validated the graph and before guest reachability is finalized. It returns a `PixelsProgramSet` carried explicitly through build/layout/report functions.

Do not hide it in thread-local state.

### 3.2 New compiler module layout

Add:

```text
crates/wrela-compiler/src/pixels/
    mod.rs
    attrs.rs
    diagnostics.rs
    symbolic.rs
    graph.rs
    canonicalize.rs
    params.rs
    bounds.rs
    support.rs
    features.rs
    csg.rs
    projective.rs
    polynomial.rs
    derivatives.rs
    events.rs
    capacities.rs
    material.rs
    program.rs
    encode.rs
    decode.rs
    dump.rs
    report.rs
    glue.rs
    reference.rs
```

Responsibilities:

- `attrs.rs`: parse and validate `@field`, `@material`, `@range`, `@rate` metadata from typed declarations.
- `symbolic.rs`: dedicated interpreter over typed bodies.
- `graph.rs`: canonical scalar, field, material, object, and feature node types.
- `canonicalize.rs`: deterministic CSE, constant folding, identity-preserving rewrites.
- `params.rs`: extract parameter access paths, layout offsets, ranges, and rates.
- `bounds.rs`: interval value bounds, world bounds, Lipschitz/derivative bounds.
- `support.rs`: smooth-CSG tropical support budgets and pruning margins.
- `features.rs`: primitive feature decomposition and validity predicates.
- `csg.rs`: partition maximal smooth objects and compile hard-CSG occupancy programs.
- `projective.rs`: inverse-view-depth equations and camera substitution.
- `polynomial.rs`: sparse degree-limited polynomial arithmetic and Bernstein conversion.
- `derivatives.rs`: scalar/jet derivative programs and remainder metadata.
- `events.rs`: local event templates and static interaction/exclusion graph.
- `capacities.rs`: derive every runtime array bound and state byte count.
- `material.rs`: material graph, filtering metadata, opacity/radiance bounds.
- `program.rs`: in-memory `FrameProgram` records.
- `encode.rs`/`decode.rs`: versioned little-endian data format.
- `dump.rs`: stable stage output.
- `report.rs`: image-report renderer block.
- `glue.rs`: generated actor, boot-init, snapshot, and program-address facts.
- `reference.rs`: allocation-free Rust mirrors used only by compiler tests and host conformance.

Add `pub mod pixels;` to `crates/wrela-compiler/src/lib.rs` only after the first empty dump task passes.

### 3.3 Existing modules modified

The implementation will touch these existing files deliberately:

```text
crates/wrela-compiler/src/bin/wrela.rs
crates/wrela-compiler/src/syntax/ast.rs              # no grammar change; span plumbing only if needed
crates/wrela-compiler/src/sema/typed.rs
crates/wrela-compiler/src/sema/types.rs
crates/wrela-compiler/src/sema/bodies.rs
crates/wrela-compiler/src/sema/intrinsics.rs
crates/wrela-compiler/src/eval/image.rs
crates/wrela-compiler/src/eval/image_checks.rs
crates/wrela-compiler/src/eval/interp.rs
crates/wrela-compiler/src/eval/value.rs
crates/wrela-compiler/src/lower.rs
crates/wrela-compiler/src/flowwir_lower.rs
crates/wrela-compiler/src/mwir.rs
crates/wrela-compiler/src/codegen.rs
crates/wrela-compiler/src/layout.rs
crates/wrela-compiler/src/layout/boot_init.rs
crates/wrela-compiler/src/layout/rtdata.rs
crates/wrela-compiler/src/placement.rs
crates/wrela-compiler/src/report.rs
crates/wrela-compiler/src/rtconfig.rs
crates/wrela-compiler/src/cost/*
crates/xtask/src/main.rs
```

The executor must not opportunistically refactor unrelated compiler code while making these changes.

### 3.4 New dump stages

Add three stable CLI stages before adding renderer behavior:

```text
wrela dump --stage=field-graph <input>
wrela dump --stage=frame-program <input>
wrela dump --stage=render-layout <input>
```

- `field-graph` prints symbolic nodes, parameter paths, ranges, object/material identities, bounds, support budgets, and feature decomposition.
- `frame-program` prints decoded records in stable ID order. It never dumps host pointers or raw nondeterministic bytes.
- `render-layout` prints immutable program placement, mutable state placement, generated actors/workers, per-core workspace, tile ownership, and exact capacity derivations.

Every later task updates one or more of these dumps before consuming its new facts.

---

## 4. Internal data model

### 4.1 IDs and deterministic ordering

Use dense newtypes over `u32`:

```rust
pub struct ScalarId(pub u32);
pub struct FieldId(pub u32);
pub struct ObjectId(pub u32);
pub struct FeatureId(pub u32);
pub struct MaterialId(pub u32);
pub struct ParamId(pub u32);
pub struct EventTemplateId(pub u32);
pub struct ProgramRendererId(pub u16);
```

No runtime record stores a Rust enum discriminant implicitly. The encoder maps every kind to an explicit versioned `u8`/`u16` opcode.

Canonical order:

1. renderer declaration order in the sealed image graph;
2. canonical callee key;
3. source span `(module path, byte start, byte end)`;
4. structural child IDs;
5. exact immediate bits.

Use `BTreeMap`, sorted vectors, and explicit stable sorts everywhere an order reaches a dump, encoded program, report, or build digest.

### 4.2 Symbolic value domain

```rust
#[derive(Clone, Debug)]
enum SymValue {
    Unit,
    Bool(SymBool),
    U16(SymU16),
    F32(ScalarId),
    Vec2([ScalarId; 2]),
    Vec3([ScalarId; 3]),
    Rgb([ScalarId; 3]),
    Field(FieldId),
    Material(MaterialValueId),
    Struct(Vec<SymValue>),
    Array(Vec<SymValue>),
    Enum { variant: usize, payload: Vec<SymValue> },
}
```

`SymBool` and `SymU16` must distinguish a comptime constant from runtime symbolic data. Runtime symbolic booleans may exist as material predicates but cannot drive `@field` control flow.

The interpreter carries:

```rust
struct SymbolicCtx<'a> {
    programs: &'a BTreeMap<String, TypedProgram>,
    root: CalleeKey,
    quota: SymbolicQuota,
    call_stack: Vec<CalleeKey>,
    scopes: Vec<BTreeMap<String, SymValue>>,
    scalar: ScalarArena,
    fields: FieldArena,
    materials: MaterialArena,
    params: ParamCollector,
}
```

Use an explicit quota for steps, nodes, call depth, unrolled statements, and aggregate elements. Quota exhaustion is a build error naming the root and current call stack.

### 4.3 Scalar graph

```rust
enum ScalarOp {
    ConstF32(u32),
    CoordX,
    CoordY,
    CoordZ,
    Param(ParamId),
    Add(ScalarId, ScalarId),
    Sub(ScalarId, ScalarId),
    Mul(ScalarId, ScalarId),
    Div(ScalarId, ScalarId),
    Neg(ScalarId),
    Abs(ScalarId),
    Min(ScalarId, ScalarId),
    Max(ScalarId, ScalarId),
    Clamp { value: ScalarId, lo: ScalarId, hi: ScalarId },
    Sqrt(ScalarId),
    Rsqrt(ScalarId),
    SinRestricted(ScalarId),
    CosRestricted(ScalarId),
    Dot3([ScalarId; 3], [ScalarId; 3]),
    Length3([ScalarId; 3]),
}

struct ScalarNode {
    op: ScalarOp,
    dependency: Dependency,
    span: Span,
}
```

`Length3` remains fused. Do not lower it to `sqrt(x*x + y*y + z*z)` inside derivative/range programs because its derivative bound uses Cauchy–Schwarz and must remain defined at zero.

### 4.4 Field graph

```rust
enum FieldKind {
    Primitive(Primitive),
    HardUnion { a: FieldId, b: FieldId },
    HardIntersection { a: FieldId, b: FieldId },
    HardSubtract { a: FieldId, b: FieldId },
    SmoothUnion { a: FieldId, b: FieldId, k: ScalarId },
    SmoothIntersection { a: FieldId, b: FieldId, k: ScalarId },
    SmoothSubtract { a: FieldId, b: FieldId, k: ScalarId },
    Transform { child: FieldId, transform: TransformProgram },
    FiniteRepeat { child: FieldId, axis: Axis, first: i32, count: u32, period: ScalarId },
    BoundedDisplace {
        base: FieldId,
        displacement: ScalarId,
        amplitude_bound: ScalarId,
        gradient_bound: ScalarId,
    },
    Mark {
        child: FieldId,
        object_source: CanonicalIdentity,
        material_source: CanonicalIdentity,
    },
}

struct FieldNode {
    kind: FieldKind,
    scalar_value: ScalarId,
    span: Span,
}
```

Every `FieldNode` retains `scalar_value`, the exact scalar expression used by the fallback evaluator. Structural lowering may not substitute a mathematically equivalent but bit-different scalar value unless the language specification explicitly authorizes it.

### 4.5 Primitive records

```rust
enum Primitive {
    Plane { normal: [ScalarId; 3], offset: ScalarId },
    Sphere { center: [ScalarId; 3], radius: ScalarId },
    Box { center: [ScalarId; 3], half: [ScalarId; 3] },
    RoundBox { center: [ScalarId; 3], half: [ScalarId; 3], radius: ScalarId },
    Capsule { a: [ScalarId; 3], b: [ScalarId; 3], radius: ScalarId },
    FiniteCylinder { a: [ScalarId; 3], b: [ScalarId; 3], radius: ScalarId },
    FiniteCone { a: [ScalarId; 3], b: [ScalarId; 3], radius_a: ScalarId, radius_b: ScalarId },
    Torus { center: [ScalarId; 3], axis: [ScalarId; 3], major: ScalarId, minor: ScalarId },
}
```

Each primitive supplies:

```rust
trait PrimitiveRule {
    fn scalar_semantics(&self) -> ScalarId;
    fn world_bounds(&self, ctx: &BoundCtx) -> Result<Aabb3, PixelsError>;
    fn lipschitz(&self, ctx: &BoundCtx) -> Result<UpperBound, PixelsError>;
    fn features(&self, ctx: &FeatureCtx) -> Result<Vec<Feature>, PixelsError>;
    fn implicit_polynomial(&self, feature: &Feature) -> Option<SparsePoly3>;
    fn max_ray_roots(&self, feature: &Feature) -> u8;
}
```

This is documentation-level pseudocode. Do not introduce a dynamic trait object in the compiler hot path; implement it with direct `match` functions.

### 4.6 Features

A feature is a smooth algebraic surface plus validity predicates:

```rust
enum FeatureKind {
    Plane,
    Quadric,
    Quartic,
    GenericImplicit,
}

struct Feature {
    primitive: FieldId,
    kind: FeatureKind,
    implicit: Option<SparsePoly3>,
    validity: Vec<PredicateProgram>,
    orientation: Orientation,
    local_bounds: Aabb3,
    max_ray_roots: u8,
}
```

Required decompositions:

- box: 6 planar faces;
- round box: 6 planes, 12 finite cylinders, 8 spheres;
- capsule: one finite cylinder and two spheres;
- finite cylinder: one cylinder side and two cap planes;
- finite cone: one conic side and two cap planes;
- torus: one quartic feature with a bounded validity domain;
- plane/sphere: one feature each.

The feature validity predicate is part of the root certificate. A root of the infinite supporting plane/cylinder is not accepted unless the finite feature inequalities hold over the root interval.

### 4.7 Maximal smooth objects and hard CSG

Partition the field tree at hard CSG operators.

A `SurfaceObject` is a maximal subtree containing primitives, transforms, repetition, bounded displacement, and smooth CSG but no hard union/intersection/subtraction above its root.

Compile the remaining hard CSG tree to a Boolean occupancy program:

```rust
enum CsgBoolOp {
    Leaf(ObjectId),
    Not(CsgBoolId),
    And(CsgBoolId, CsgBoolId),
    Or(CsgBoolId, CsgBoolId),
}
```

At runtime, oriented surface crossings toggle object occupancy. The first crossing that changes the root Boolean value is the visible hard-CSG boundary. This removes whole-scene `min/max` evaluation from ordinary hard-CSG visibility.

Also compute the Boolean influence program for each leaf:

```text
influence_i(state) = root(state with i=0) XOR root(state with i=1)
```

A crossing with false influence cannot change root occupancy and is skipped.

### 4.8 Parameter slots

```rust
struct ParamPath {
    root_type: Type,
    projections: Vec<Projection>,
    leaf_type: ParamLeafType,
    source_span: Span,
}

enum Projection {
    Field { name: String, index: u16 },
    ConstIndex { index: u32 },
    Tuple { index: u16 },
}

struct ParamSlot {
    path: ParamPath,
    byte_offset: u32,
    component: u8,
    range: F32Range,
    rate: Option<RateBound>,
    use_mask: ParamUseMask,
}
```

Deduplicate exact paths. Sort by layout byte offset, then component, then canonical path. The renderer snapshot code loads only these slots.

`ParamUseMask` records geometry, material, light, exposure, and topology-sensitive use. Any path that reaches topology-sensitive control flow is rejected before this stage.

### 4.9 Bounds and proof metadata

Every scalar/field/object/feature record carries only facts that have a conservative construction rule:

```rust
struct ProofMeta {
    value: Option<IvF64>,
    world_bounds: Option<Aabb3>,
    lipschitz: Option<f64>,
    gradient_norm: Option<IvF64>,
    hessian_norm: Option<IvF64>,
    third_derivative_norm: Option<IvF64>,
    smooth_support: f64,
    identity_set: IdentitySet,
    finite: bool,
}
```

Host analysis uses `f64` plus outward endpoint construction. Encoded runtime certificates use `Iv32` fixed domains. Host `f64` facts are not themselves proof objects unless converted outward into an encoded domain.

### 4.10 Smooth support theorem used by the compiler

For polynomial smooth minimum:

```text
min(a,b) - k/4 <= smin(a,b,k) <= min(a,b)
```

For a smooth tree:

```text
support(leaf) = 0
support(smin(a,b,k)) = k/4 + max(support(a), support(b))
```

Therefore every zero of the smooth tree lies within the accumulated support shell of at least one primitive leaf.

Use the gap-sensitive form when a certified lower bound `g <= |a-b| <= k` is available:

```text
bulge(g,k) = (k - g)^2 / (4k)
```

The compiler emits the conservative maximum when no gap bound is available. It may never use sampled branch gaps as proof.

### 4.11 Projective coordinate contract

Use unnormalized camera rays:

```text
r(u,v) = forward + u*right + v*up
P(u,v,q) = eye + r(u,v)/q
```

where `q = 1 / view_axis_depth` and `q > 0`.

Do not normalize `r` in any primary visibility certificate. The exact cancellation between normalized ray distance and view-axis depth is part of the formal core.

For a degree-`d` implicit polynomial `phi(P)`, compile:

```text
Phi(u,v,q) = q^d * phi(eye + r(u,v)/q)
```

`Phi` is polynomial and has the same positive-q roots.

Required direct formulas in tests:

Plane:

```text
Phi = q * (dot(n, eye) + c) + dot(n, r)
```

Sphere, with `a = eye - center`:

```text
Phi = (dot(a,a) - radius^2) * q^2
    + 2 * dot(a,r) * q
    + dot(r,r)
```

All other algebraic features use the generic sparse-polynomial homogenizer and are checked against direct scalar evaluation.

### 4.12 Local event templates

```rust
enum EventKind {
    ProjectedBoundEnter,
    ProjectedBoundExit,
    FeatureValidity,
    Tangency,
    SmoothBandBoundary,
    IdentityBoundary,
    DepthOrderSwap,
    RepeatBoundary,
    MaterialBoundary,
}

enum EventRep {
    AnalyticConic(ConicProgram),
    SparsePolynomial(PolynomialProgram),
    TaylorPredicate(PredicateProgram),
}

struct EventTemplate {
    kind: EventKind,
    participants: SmallParticipantSet,
    representation: EventRep,
    static_exclusion: Vec<ExclusionRule>,
}
```

Do not generate all pairwise depth-order events. Generate a static interaction edge only when conservative world/projected bounds and possible q ranges overlap. At runtime, an event is active only when the current tile/row/parameter box still overlaps.

### 4.13 Completeness record

Each tile/row sweep builds:

```rust
struct ActiveCover {
    included_objects: IdRange,
    included_features: IdRange,
    active_events: IdRange,
    exclusions: IdRange,
    q_domain: Iv32,
}

enum ExclusionReason {
    ProjectedBoundsDisjoint,
    SupportShellDisjoint,
    QRangeDisjoint,
    CsgInfluenceFalse,
    FeatureValidityFalse,
    ParameterBoxDisjoint,
}
```

An omitted participant is legal only when one `ExclusionRecord` encloses the complete current domain and has strictly positive slack. Records are regenerated after any domain split that invalidates their scope.

### 4.14 Frame program wire format

`FrameProgram v1` is little-endian, offset-based, and pointer-free.

Header, exactly 160 bytes:

```rust
#[repr(C)]
struct FrameProgramHeaderV1 {
    magic: [u8; 4],          // b"WPIX"
    version: u16,            // 1
    header_bytes: u16,       // 160
    total_bytes: u32,
    flags: u32,
    renderer_id: u16,
    reserved0: u16,

    scalar_count: u32,
    field_count: u32,
    object_count: u32,
    feature_count: u32,
    material_count: u32,
    param_count: u32,
    event_template_count: u32,
    csg_op_count: u32,
    fixed_domain_count: u32,

    scalar_off: u32,
    field_off: u32,
    object_off: u32,
    feature_off: u32,
    material_off: u32,
    param_off: u32,
    event_template_off: u32,
    csg_off: u32,
    fixed_domain_off: u32,
    immediate_off: u32,
    string_off: u32,

    width: u32,
    height: u32,
    refresh_hz: u16,
    shade_hz: u16,
    tile_width: u16,
    tile_height: u16,
    near_bits: u32,
    far_bits: u32,

    program_digest: [u8; 32],
    reserved: [u8; 20],
}
```

The encoder must assert the Rust struct is not used for serialization. Fields are written explicitly in order so host padding cannot affect bytes.

All table offsets are 16-byte aligned. Records have explicit encoded sizes and reserved bytes set to zero. The decoder rejects:

- wrong magic/version/header size;
- nonzero reserved bytes;
- integer overflow in offset+length;
- overlap between tables;
- misalignment;
- unknown opcodes;
- noncanonical ordering;
- digest mismatch.

The guest does not run the full defensive decoder on its own sealed image. The compiler decoder and VMM image verifier do. Guest table access uses compiler-proven offsets and a small debug assertion path.

### 4.15 Image placement

Add two image sections:

```text
frameprog   immutable, 64-byte aligned
pixelsdata  zero-initialized mutable renderer state, 64-byte aligned
```

Placement order:

```text
entry/code/ordinary rodata
  -> steer to RTDATA_BASE
  -> rtdata
  -> frameprog
  -> pixelsdata
  -> existing pools/reservations as layout requires
```

Do not place frame programs below `RTDATA_BASE`; code and existing rodata already consume that constrained space. `frameprog` and `pixelsdata` are ordinary image-internal sections after rtdata and do not change the machine-v1 device contract.

Every renderer receives:

```rust
struct RendererPlacement {
    program_base: u64,
    program_bytes: u64,
    state_base: u64,
    state_bytes: u64,
    per_core: Vec<RendererCorePlacement>,
}
```

Add section rows and renderer placement rows to `ImageLayout` and the image report.

### 4.16 Mutable renderer state

Compiler-derived layout, no guest allocation:

```text
RendererStateHeader
current coefficient snapshot
previous coefficient snapshot
current camera/light/post snapshot
previous camera/light/post snapshot
FrameComplex A
FrameComplex B
per-core CandidateSet
per-core root-isolation stack
per-core event-isolation stack
per-core active sheet list
per-core run list
per-core event corridor list
per-core fixed-q raster state
per-core shading summary cache
per-core transparent transfer tree
probe clipmap and invalidation queue
tile descriptors and tile ownership table
failure record
```

Double-buffer `FrameComplex` so a failed rebuild cannot corrupt the last valid structure. Swap only after every tile reports success.

### 4.17 Capacity derivation

No author-written runtime capacity is allowed to stand in for completeness.

Derive and report:

```text
max features per object
max candidate objects per tile
max candidate features per tile
max roots at a row start
max simultaneously active sheets
max event intervals per row
max runs per row
max singular corridors per row
max transparency layers
max root stack nodes
max event stack nodes
max shading summary terms
max probe invalidations per frame
```

Conservative formulas may overallocate. They may not undercount.

The first implementation may use global maxima when a tighter per-tile bound is unavailable. Later tightening is an optimization that must preserve the same derivation proof.

A successful `AaaByteExact` build proves every count fits its encoded integer width and the total memory fits the image’s declared machine profile.

---

## 5. Runtime mathematics fixed by this plan

### 5.1 Fixed domains and intervals

```wrela
struct FixedDomain:
    frac_bits: u8
    min_raw: i32
    max_raw: i32

struct Iv32:
    lo: i32
    hi: i32
```

Interpretation:

```text
real(raw) = raw / 2^frac_bits
```

Rules:

- `lo <= hi` is mandatory;
- add/sub use checked `i32` and outward saturation is **not** allowed;
- multiplication uses four `i64` products and floor/ceil shift;
- division multiplies by a separately certified reciprocal interval;
- square and absolute value use exact sign cases;
- an invalid domain or overflow returns `UnresolvedNumeric`, never an infinite interval that appears useful;
- conversion from `f32` uses the exact IEEE binary value and outward integer rounding;
- every hot path groups values with one shared domain so SIMD lanes do not carry exponents.

The compiler chooses separate domains for world coordinates, q, q derivatives, field residual, radiance, coverage, and proof slack.

### 5.2 Root isolation at a row start

For fixed `(x,y)`, process the complete positive-q interval corresponding to `[near, far]`:

```text
q_near = 1 / near
q_far  = 1 / far
root domain = [q_far, q_near]
```

Use a front-to-back stack ordered by decreasing q:

```rust
fn isolate_all_roots(
    object: ObjectId,
    ray: RayKey,
    q_domain: Iv32,
    out: &mut FixedVec<RootInterval>,
    stack: &mut FixedStack<Iv32>,
) -> Result<(), RenderError> {
    stack.push(q_domain)?;
    while let Some(q) = stack.pop() {
        let f = eval_object_range(object, ray, q)?;
        if f.excludes_zero() {
            continue;
        }

        let dq = eval_object_dq_range(object, ray, q)?;
        let ends = eval_object_endpoints(object, ray, q)?;

        if dq.excludes_zero() && ends.have_opposite_uniform_signs() {
            out.push(contract_monotone_root(object, ray, q, dq)?)?;
            continue;
        }

        if let Some(contracted) = krawczyk_contract(object, ray, q)? {
            if contracted.strictly_smaller_than(q) {
                stack.push(contracted)?;
                continue;
            }
        }

        if q.width_raw() <= MIN_ROOT_WIDTH_RAW {
            return Err(RenderError::RootIsolationExhausted(ray.tile));
        }

        let (far_half, near_half) = q.split_midpoint();
        stack.push(far_half)?;
        stack.push(near_half)?; // popped first: larger q, nearer
    }
    sort_and_merge_disjoint(out)?;
    Ok(())
}
```

This routine finds every object root, not only the first. Tangent/near-multiple roots that cannot be isolated become event corridors; they are not discarded.

### 5.3 Run candidate from implicit derivatives

For `G(x,q)=0` on one row:

```text
q_x  = -G_x / G_q
q_xx = -(G_xx + 2 G_xq q_x + G_qq q_x^2) / G_q
```

Construct:

```text
q_hat(dx) = q0 + q_x*dx + 0.5*q_xx*dx^2
```

The candidate stores f32 coefficients. The verifier converts them outward into the run’s q domain.

### 5.4 Primary run certificate

Given screen interval `X`, candidate `q_hat(X)`, and correction interval `E=[-eps,+eps]`:

1. evaluate `G(X, q_hat(X)-eps)`;
2. evaluate `G(X, q_hat(X)+eps)`;
3. evaluate `G_q(X, q_hat(X)+E)`;
4. require `G_q` excludes zero;
5. require the two endpoint ranges have opposite strict signs;
6. require feature-validity predicates hold throughout the tube;
7. require identity classification is fixed or one explicit blend class;
8. require q interval separation from every competing active sheet;
9. require the complete active cover/exclusions remain valid over `X`.

This proves exactly one root per x in the tube by continuity and monotonicity.

If endpoint ranges are too wide, attempt scalar parametric Krawczyk:

```text
A = reciprocal enclosure around G_q(center)
K = -A * G(X, q_hat(X))
    + (1 - A * G_q(X, q_hat(X)+E)) * E
accept only if K is strictly inside E
```

If both fail, halve `X`. Do not widen tolerance.

### 5.5 Complete root cover across a run

Continuation of known roots is insufficient by itself. A new pair of roots can appear only where `G=0` and `G_q=0`, or when a previously excluded support/feature becomes active.

Therefore every run also proves:

- all support candidates at its left endpoint were isolated over the complete q domain;
- every tracked root has `G_q != 0` over the run;
- every unoccupied q slab between tracked tubes excludes zero, or is covered by an active tangency event corridor;
- support, projected-bound, feature-validity, and repetition-boundary events exclude zero before the run end.

The run end is shortened to the earliest certificate expiry.

### 5.6 q-order and winner

Larger q is nearer. A sheet wins over a run when:

```text
winner.lo > competitor.hi
```

for every competitor after applying all root and quantization errors.

Maintain adjacent order certificates for a sorted active list. Adjacent strict order implies total order. Recompute the winner only when an adjacent certificate expires or an event inserts/removes a sheet.

### 5.7 Event isolation

For each active event predicate over row interval `X`:

- if its interval excludes zero, retain the sign and slack;
- if it is an analytic conic/quadratic, solve row intersections and enclose each root outward;
- otherwise use interval subdivision plus derivative contraction;
- overlapping root intervals are merged into one `EventCorridor`;
- the corridor is widened by curve-position and fixed-point error;
- no ordinary run crosses a corridor.

Inside a corridor, use bounded local re-isolation at pixel/subpixel domains and analytic coverage. If the output code cannot be fixed, the corridor subdivides. Exhaustion prevents presentation.

### 5.8 Analytic coverage

Represent a local event edge as a line or quadratic Bézier segment with a certified positional strip and curvature remainder.

For the v1 box filter:

- exact line coverage uses polygon clipping against the pixel square;
- quadratic coverage uses a Green’s-theorem boundary integral;
- if the curve remainder can alter coverage beyond the allocated output budget, split the segment;
- foreground/background radiance intervals convert coverage uncertainty into color uncertainty by local contrast.

No MSAA or TAA is used to establish silhouette correctness.

### 5.9 Fixed-q raster recurrence

Within a certified run, quantize q to `i32` with a micro-run reset no longer than 64 pixels.

For one row:

```text
q0 = q(x)
d1 = q(x+1) - q(x)
d2 = q(x+2) - 2q(x+1) + q(x)
```

Advance:

```text
q  += d1
d1 += d2
```

Four-pixel packet setup stores q at `x..x+3`; packet advance by four uses:

```text
q4_delta  = 4*d1 + 6*d2
delta_step = 16*d2
```

The run certificate includes coefficient quantization and recurrence error. The compiler/runtime proves no `i32` overflow before the next reset.

### 5.10 Normal reconstruction

For camera-space position:

```text
P(u,v) = (u/q, v/q, 1/q)
```

an unnormalized normal is proportional to:

```text
N = (q_u, q_v, q - u*q_u - v*q_v)
```

Use q-sheet derivatives for ordinary interiors. Use exact field gradients at event/singular corridors or whenever the normal-cone certificate exceeds the material’s allocated shading error.

### 5.11 Material and shading summary

Compile `@material` to a material scalar graph. At a run/tile, propose one of:

1. constant material/radiance;
2. cubic one-dimensional polynomial along the run;
3. tensor polynomial over a row band;
4. low-rank factorization of that tensor polynomial;
5. dense per-pixel shader evaluation.

Acceptance always uses an interval residual bound against the exact material/light graph. Low-rank form is merely a storage/execution optimization after the tensor summary is accepted.

### 5.12 Transparency algebra

Use premultiplied radiance and residual transmittance:

```text
(C1,T1) compose (C2,T2) = (C1 + T1*C2, T1*T2)
```

The operation is associative. Store deep transparent stacks in a fixed-capacity segment tree keyed by the certified q order.

Terminate a remaining suffix only when:

```text
current_transmittance * max_remaining_radiance <= assigned_encoded_error
```

The max remaining radiance comes from material/light bounds, not a constant heuristic.

### 5.13 Direct lighting and shadows

Flagship material model:

- energy-conserving Lambert diffuse;
- one GGX-style glossy lobe;
- emissive term;
- scalar opacity;
- filtered normal/slope moments.

Lights:

- directional;
- point;
- rectangular area;
- disk area.

Point/directional shadow visibility uses certified secondary field queries at shading summary sites and fits a verified transition over the run. Area-light visibility integrates that query over emitter coordinates using deterministic adaptive quadrature with interval remainder bounds.

### 5.14 AO and GI

AO remains 4–5 field-distance taps along the certified normal. The taps execute against the object/tile active program valid for their own spatial domains.

GI remains a deterministic world-space probe clipmap:

- exact fixed capacity;
- compiler-known level sizes;
- deterministic update order;
- invalidation from object swept bounds;
- no volumetric visibility/light bake;
- contribution culling only when throughput × incident-radiance × transfer sensitivity fits the assigned encoded error.

### 5.15 Display-byte verifier

The complete output path is:

```text
geometry/coverage interval
  -> material interval
  -> direct/AO/GI/shadow interval
  -> transparency composition
  -> exposure
  -> 3x3 color transform
  -> monotone tone-curve LUT
  -> transfer LUT
  -> u8 quantization
```

Tone and transfer tables are immutable build inputs. The compiler verifies each channel LUT is monotone.

For an exact channel value enclosed by `[lo,hi]`, if both encoded endpoints map to the same u8 code, the byte is fixed. Otherwise refine the largest remaining contributor according to the deterministic error/cost queue.

### 5.16 Kinetic transport

For regular sheets:

```text
q_t = -G_t / G_q
```

Store first- and second-order parameter/time derivative bounds. A temporal certificate includes:

- parameter ranges and observed deltas;
- event predicate sign slack;
- root/tube slack;
- adjacent q-order slack;
- feature/identity slack;
- shading/post slack;
- fixed-point quantization slack.

Compress them to the minimum margin only after recording the component that owns that minimum for diagnostics.

A tile is reused only when the sum of all certified perturbation bounds is strictly below its stored slack. Otherwise rebuild that tile from scratch.

### 5.17 Static frame reuse

Reuse the previous framebuffer without visibility or shading work only when a digest covers equality of:

- geometry coefficients;
- camera;
- lights;
- material coefficients;
- probe state/version;
- exposure and color transform;
- tone/transfer table IDs;
- output size;
- dither phase/policy;
- renderer program digest.

Scanout still presents the existing buffer normally.

---

## 6. Compiler and runtime interfaces

This section fixes the Rust and Wrela interfaces. Later tasks implement these shapes; they do not invent replacements.

### 6.1 Typed renderer metadata

Extend `crates/wrela-compiler/src/sema/typed.rs` with renderer-specific metadata rather than rescanning AST attributes after typing:

```rust
#[derive(Debug, Clone, PartialEq)]
pub struct FieldFnMeta {
    pub point_param: usize,
    pub params_param: Option<usize>,
    pub return_ty: types::Type,
}

#[derive(Debug, Clone, PartialEq)]
pub struct MaterialFnMeta {
    pub surface_param: usize,
    pub params_param: Option<usize>,
    pub return_ty: types::Type,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ScalarRange {
    pub lo: f64,
    pub hi: f64,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ScalarRate {
    pub max_delta: f64,
    pub max_second_delta: f64,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct RenderAttrs {
    pub field: Option<FieldFnMeta>,
    pub material: Option<MaterialFnMeta>,
    pub field_ranges: BTreeMap<Vec<usize>, ScalarRange>,
    pub field_rates: BTreeMap<Vec<usize>, ScalarRate>,
}
```

Add `render_attrs: RenderAttrs` to `TypedProgram`, not to every `TypedFn`. Function metadata is keyed by canonical function key inside `RenderAttrs` in the implementation if multiple field/material roots are allowed across imported modules. The public shape above is illustrative; the stable dump must print canonical keys.

Rules:

- source spans remain in a parallel diagnostic map, not in the serialized frame program;
- imported `@field`/`@material` metadata is merged using canonical module-qualified names;
- generic instances are stored by their existing canonical instantiation keys;
- a function cannot be both `@field` and `@material`;
- `@field` and `@material` functions are ordinary guest-reachable functions only when called by ordinary source; their annotations alone do not force executable code emission;
- the renderer compiler consumes their typed bodies host-side.

### 6.2 Compiler entry point

Create `crates/wrela-compiler/src/pixels/mod.rs` with this public compiler-facing API:

```rust
pub struct CompileInput<'a> {
    pub programs: &'a BTreeMap<String, TypedProgram>,
    pub owner_module: &'a str,
    pub field_key: &'a str,
    pub material_key: &'a str,
    pub renderer: &'a eval::image::RendererDecl,
}

pub struct CompiledRenderer {
    pub program: FrameProgram,
    pub encoded: Vec<u8>,
    pub mutable_layout: RendererStateLayout,
    pub generated: GeneratedRenderer,
    pub report: RendererReport,
}

pub fn compile(input: CompileInput<'_>) -> Result<CompiledRenderer, PixelsError>;
```

`pixels::compile` is pure with respect to the repository and host:

- no filesystem reads;
- no environment reads;
- no host CPU feature reads;
- no wall clock;
- no RNG;
- no hash-map iteration in emitted order;
- no dense frame rendering;
- no invocation of fieldprobe;
- no dependency on output from a previous build.

All nondeterministic containers must be absent from the renderer compiler. Use indexed vectors, `BTreeMap`, `BTreeSet`, and explicit stable sorts.

### 6.3 Image graph declaration

Extend `crates/wrela-compiler/src/eval/image.rs`:

```rust
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum ImageDeclRef {
    Device(usize),
    Driver(usize),
    Actor(usize),
    Renderer(usize),
    Pool(String),
    DmaPool(String),
}

#[derive(Debug, Clone, PartialEq)]
pub struct RendererDecl {
    pub params_type: Type,
    pub args: Vec<DeclArg>,
}

pub struct ImageGraph {
    // existing fields...
    pub renderers: Vec<RendererDecl>,
}
```

`Image.renderer[P]` returns `ImageDeclRef::Renderer(index)`. `RendererDecl` is part of the construction DAG. A renderer may depend on:

- one display driver declaration;
- function references for `field=` and `material=`;
- immutable scalar/configuration arguments;
- no actor handle except the display driver;
- no pool declaration authored by the application.

The compiler creates all renderer pools and actor instances from the declaration. This keeps capacity derivation authoritative and prevents source from lying about workspace sizes.

### 6.4 Renderer declaration accessors

Add typed helpers in `pixels/config.rs`:

```rust
pub struct RendererConfig {
    pub params_type: Type,
    pub field_key: String,
    pub material_key: String,
    pub display: ImageDeclRef,
    pub width: u32,
    pub height: u32,
    pub refresh_hz: u32,
    pub shade_hz: u32,
    pub profile: RenderProfile,
    pub tone_curve: ToneCurve,
    pub near: f64,
    pub far: f64,
    pub world: Aabb64,
    pub camera_bounds: CameraBounds,
    pub lights: Vec<LightConfig>,
    pub probe: ProbeConfig,
}

pub fn parse_renderer_decl(
    decl: &RendererDecl,
    programs: &BTreeMap<String, TypedProgram>,
) -> Result<RendererConfig, PixelsError>;
```

`parse_renderer_decl` validates labels exactly. It must reject unknown, duplicate, missing, non-comptime, non-finite, and out-of-range values. Do not scatter argument lookup through later passes.

### 6.5 Image-check integration

Add `check_renderers` to `eval/image_checks.rs` and call it from `check_sealed` before placement.

The checker proves:

- at least one display declaration exists for each renderer;
- `display=` points to a `drivers.Display` declaration and not merely any driver;
- the display is bound to a machine-v1 display device;
- field/material function references resolve across the checked closure;
- the parameter type is identical at the renderer, field, material, and `RenderFrame[P]` boundary;
- width and height equal the declared display mode;
- `refresh_hz > 0`, `shade_hz > 0`, and `refresh_hz % shade_hz == 0`;
- near/far/world bounds are finite and ordered;
- renderer count and generated actor count fit existing actor and core-placement limits;
- all fields reachable from `P` that influence geometry, materials, lights, exposure, or post have compile-time range metadata;
- every temporally changing influencing path has `@rate` metadata;
- no renderer declaration participates in a construction cycle.

### 6.6 Frame-program compiler context

Use one owner object passed explicitly through renderer compilation:

```rust
pub struct PixelsCx<'a> {
    pub input: CompileInput<'a>,
    pub config: RendererConfig,
    pub spans: SpanTable,
    pub scalar: ScalarArena,
    pub fields: FieldArena,
    pub materials: MaterialArena,
    pub params: ParamTable,
    pub objects: Vec<SmoothObject>,
    pub csg: CsgProgram,
    pub diagnostics: Vec<PixelsDiagnostic>,
}
```

Do not use thread-local compiler switches. Do not mutate `TypedProgram`. Every pass takes and returns ordinary structs.

### 6.7 Pass driver

The pass order in `pixels::compile` is fixed:

```rust
pub fn compile(input: CompileInput<'_>) -> Result<CompiledRenderer, PixelsError> {
    let config = config::parse_renderer_decl(input.renderer, input.programs)?;
    let mut cx = PixelsCx::new(input, config);

    symbolic::compile_field_root(&mut cx)?;
    symbolic::compile_material_root(&mut cx)?;
    canonicalize::run(&mut cx)?;
    params::collect_and_validate(&mut cx)?;
    bounds::propagate(&mut cx)?;
    support::propagate(&mut cx)?;
    objects::partition(&mut cx)?;
    features::decompose(&mut cx)?;
    csg::compile_boolean_program(&mut cx)?;
    projective::compile_features(&mut cx)?;
    derivatives::compile(&mut cx)?;
    events::compile_generators(&mut cx)?;
    capacities::derive(&mut cx)?;
    material::compile_summaries(&mut cx)?;
    let program = program::finish(&mut cx)?;
    verify::check_program(&program)?;
    let encoded = encode::encode(&program)?;
    let decoded = decode::decode(&encoded)?;
    if decoded != program {
        return Err(PixelsError::internal("frame-program encode/decode mismatch"));
    }
    let mutable_layout = state::layout(&program)?;
    let generated = glue::generate(&program, &mutable_layout)?;
    let report = report::build(&program, &mutable_layout, &generated);

    Ok(CompiledRenderer { program, encoded, mutable_layout, generated, report })
}
```

No task may reorder this pipeline without updating the stable dump and the documented invariant consumed by every later pass.

### 6.8 Frame-program verifier

`pixels::verify::check_program` is a hostile-input verifier even though the compiler produced the value. It must validate the same constraints a future external decoder would need:

- all IDs in range;
- every table sorted as specified;
- no duplicate stable IDs;
- every offset and count inside its section;
- all coefficient references valid;
- all CSG stack programs balanced and bounded;
- every feature belongs to exactly one object;
- every object owns at least one feature or is explicitly empty and pruned;
- all support budgets nonnegative and finite;
- all interval endpoints ordered;
- denominators proven positive where required;
- all fixed-point exponents in supported range;
- every event generator names valid dependencies;
- every exclusion record covers a specific omitted candidate or pair;
- all derived capacities are at least the exact table requirements;
- all runtime loops have finite declared maxima;
- all binary-size additions and multiplications checked for overflow.

The encoder accepts only a `VerifiedFrameProgram` newtype returned by this verifier. Do not make `encode(FrameProgram)` public.

### 6.9 Binary decoder contract

The runtime does not decode a self-describing graph with allocations. The compiler lays out packed tables and emits a generated `FrameProgramView` containing constant base addresses and counts.

The Rust decoder exists only for compiler tests, report generation, fuzzing, and binary round-trip checks. It must:

- reject unknown version;
- reject nonzero reserved bytes;
- reject misaligned offsets;
- reject overlapping tables unless the format explicitly aliases them;
- reject trailing bytes not named by the header;
- avoid recursive descent;
- impose a hard byte cap before allocation.

### 6.10 Image layout integration

Extend `layout::ImageLayout`:

```rust
pub struct RendererPlacement {
    pub index: usize,
    pub frameprog_base: u64,
    pub frameprog_size: u64,
    pub state_base: u64,
    pub state_size: u64,
    pub worker_core_count: usize,
}

pub struct ImageLayout {
    // existing fields...
    pub renderers: Vec<RendererPlacement>,
}
```

Packing order is fixed:

```text
IMAGE_BASE
  entry
  code
  rodata
  abort/checkpoint
RTDATA_BASE
  existing runtime tables
PIXELS_DATA_BASE = align_up(RTDATA_BASE + rtdata_size, 64 KiB)
  renderer 0 FrameProgram, 64-byte aligned
  renderer 1 FrameProgram, 64-byte aligned
  ...
PIXELS_STATE_BASE = align_up(end(FramePrograms), 64 KiB)
  renderer mutable states, page aligned
  framebuffer tile backing, page aligned
  probe storage, page aligned
```

Add machine-layout constants for `PIXELS_DATA_BASE_MIN`, maximum renderer-program bytes, maximum renderer-state bytes, and maximum framebuffer reservation. These are packing ceilings, not runtime discovery.

The layout must fail before blob construction if any region overlaps, exceeds the image reservation, crosses an address-space checked-add, or violates display-device page alignment.

### 6.11 Generated renderer actor

For each renderer declaration, generate one coordinator actor and `N` worker actors, where `N` is the number of cores assigned to the renderer by placement. Generated symbols use reserved names:

```text
__wrela_renderer_<r>_coordinator
__wrela_renderer_<r>_worker_<core>
__wrela_renderer_<r>_render
__wrela_renderer_<r>_render_tile
__wrela_renderer_<r>_sweep_tile
__wrela_renderer_<r>_shade_run
__wrela_renderer_<r>_raster_run4
__wrela_renderer_<r>_present
```

Generated actors are ordinary typed/FlowWir/MachineWir participants for capacity, placement, cost, and progress. Do not inject raw machine-code blobs for the whole renderer.

The standard renderer implementation is authored once in Wrela using generated constants and frame-program table views. Only the constants, static table addresses, and bounded generic extents differ per image.

### 6.12 Runtime ownership protocol

The coordinator owns:

- the current and next frame metadata;
- the active framebuffer tile list;
- global camera/light/probe version state;
- the final display-driver handle;
- the exact presentation sequence number.

Each worker owns:

- one disjoint tile range for the current frame;
- one fixed workspace slice;
- one output tile buffer at a time;
- no mutable reference to another worker's state.

At frame start the coordinator partitions tiles deterministically by ascending tile ID and sends one bounded job to each worker. A job contains a copied coefficient snapshot and immutable frame-program handle. Workers return owned completed tile lists. The coordinator concatenates in tile-ID order and submits exactly once.

There is no shared q-buffer, event list, run list, or probe update buffer between workers. Cross-tile edge continuity is guaranteed by globally defined pixel coordinates and half-open ownership, not shared mutation.

### 6.13 Runtime failure protocol

A renderer never flushes a partially built back buffer.

```wrela
enum RenderError:
    UnsupportedFrameState
    NonFiniteInput
    CertificateExhausted
    EventCapacityExceeded
    SheetCapacityExceeded
    RunCapacityExceeded
    TransparentLayerCapacityExceeded
    ProbeCapacityExceeded
    FixedPointRangeExceeded
    InternalProgramViolation
    DisplayFailure
```

Worker errors return to the coordinator. The coordinator:

1. cancels or drains all outstanding jobs for that frame;
2. leaves the last fully presented front buffer untouched;
3. returns the exact `RenderError` to the caller;
4. records the failure in replay output;
5. does not silently lower quality or loosen a tolerance.

`CertificateExhausted` means every fixed refinement tier and bounded local rebuild was attempted. It does not mean “draw background.”

### 6.14 Runtime numeric modules

Add these standard-library modules:

```text
stdlib/core/field.wr
stdlib/core/render.wr
stdlib/core/render_interval.wr
stdlib/core/render_program.wr
stdlib/core/render_events.wr
stdlib/core/render_sweep.wr
stdlib/core/render_coverage.wr
stdlib/core/render_material.wr
stdlib/core/render_light.wr
stdlib/core/render_transfer.wr
stdlib/core/render_probe.wr
stdlib/core/render_raster.wr
stdlib/core/render_actor.wr
stdlib/drivers/display.wr
```

Module responsibilities are strict. `render_interval.wr` cannot import display or actor code. `render_raster.wr` cannot evaluate a field. `render_actor.wr` orchestrates but contains no numeric formulas.

### 6.15 Scalar and packet kernel pairing

Every hot packet kernel has a scalar function with the same semantic name plus `_scalar`:

```wrela
fn q_order4_scalar(...)
fn q_order4(...)

fn raster_q4_scalar(...)
fn raster_q4(...)

fn compose_transfer4_scalar(...)
fn compose_transfer4(...)
```

The scalar function is the differential oracle for the packet function. The packet function uses only closed SIMD operations from `05-library.md §8.1`; no inline assembly and no target feature checks.

---

## 7. Diagnostics contract

All renderer diagnostics use category `pixels`. Error messages name the source construct, the failed proof obligation, and the remediation. Do not expose internal node numbers without also printing the source span and stable object/feature name.

### 7.1 Diagnostic data

```rust
pub struct PixelsDiagnostic {
    pub code: &'static str,
    pub message: String,
    pub primary: Span,
    pub notes: Vec<String>,
    pub help: Vec<String>,
}
```

`PixelsError` contains exactly one primary diagnostic for ordinary user errors. Internal consistency failures use `error[internal]` and include the stage name.

### 7.2 Required diagnostics

The following codes and leading messages are stable:

| code | leading message |
|---|---|
| `P001` | `` `@field` function `<name>` must return `Field` `` |
| `P002` | `` `@field` function `<name>` has unsupported parameter shape `` |
| `P003` | `` `@field` function `<name>` uses runtime control flow that changes field topology `` |
| `P004` | `` field operation `<op>` is not available in `AaaByteExact` `` |
| `P005` | `` parameter path `<path>` influences rendering but has no `@range` `` |
| `P006` | `` parameter path `<path>` changes between frames but has no `@rate` `` |
| `P007` | `` range for `<path>` is empty, non-finite, or not representable `` |
| `P008` | `` renderer declaration has unknown or duplicate argument `<label>` `` |
| `P009` | `` renderer field/material parameter types disagree `` |
| `P010` | `` renderer display mode disagrees with the bound display driver `` |
| `P011` | `` smooth CSG radius `k` is not strictly positive over its declared range `` |
| `P012` | `` repetition has no finite relevant-instance bound in the renderer world box `` |
| `P013` | `` deformation `<name>` lacks a conservative amplitude/derivative contract `` |
| `P014` | `` material `<name>` contains an output-affecting discontinuity with no explicit event `` |
| `P015` | `` renderer capacity `<kind>` exceeds the machine-v1 sealed ceiling `` |
| `P016` | `` projective denominator may reach zero inside the declared camera/world domain `` |
| `P017` | `` fixed-q state cannot be represented with the supported i32 microtile format `` |
| `P018` | `` tone or transfer table is not monotone `` |
| `P019` | `` transparent material has no finite radiance-tail bound `` |
| `P020` | `` renderer proof table failed internal verification `` |
| `P021` | `` more than one renderer claims display declaration `<display>` `` |
| `P022` | `` render profile `<profile>` is not implemented by machine v1 `` |
| `P023` | `` field recursion is forbidden `` |
| `P024` | `` field loop bound is not comptime-known `` |
| `P025` | `` renderer-generated image memory exceeds the 1 GiB flagship profile `` |

### 7.3 Why-chain diagnostics

For capacity and unsupported-operation failures, emit a bounded why-chain. Example:

```text
error[pixels P015]: renderer capacity `event_records` needs 13,442 slots,
which exceeds the machine-v1 ceiling of 8,192
  --> game/scene.wr:42:12
  object `fighter_cluster`
  4 smooth objects × 17 feature families
  1,078 projected-overlap pairs survive support pruning
  each pair reserves at most 12 event records
  plus 506 primitive/feature boundary records
help: split the scene into independently bounded render layers, reduce the
parameter/world range, or replace the unsupported topology-changing motion
```

The chain is computed from exact pass records. Never print an estimated count as an exact requirement.

### 7.4 Internal diagnostics

Every pass boundary validates its input. Internal messages follow:

```text
error[internal]: pixels::<stage>: <invariant>
```

Examples:

- `pixels::support: child budget missing after topological traversal`;
- `pixels::events: omitted pair has no exclusion record`;
- `pixels::encode: table offset is not 16-byte aligned`;
- `pixels::glue: generated worker count differs from placement`.

Internal errors are test failures. Do not convert them to `P020` except when decoding an already serialized frame program.

---

## 8. Stable dumps, reports, and reproducibility

### 8.1 CLI stages

Extend the CLI usage and dump dispatcher with:

```text
--stage=field-graph
--stage=frame-program
--stage=render-layout
```

All three require a complete build closure and exactly one renderer unless `--renderer=<index>` is supplied. Add `--renderer=<index>` only to these three stages. Unknown indices fail.

### 8.2 `field-graph` dump

The dump is semantic and pre-encoding:

```text
FieldGraph v1
  Renderer index=0 field=game.scene::world material=game.scene::shade
  Param id=p0 path=SceneParams.sword_angle ty=f32 range=[-1.4,1.4]
    Rate max_delta=0.08 max_second_delta=0.02
  Scalar id=s0 kind=Param param=p0
  Field id=f0 kind=Plane ...
  Field id=f1 kind=RoundBox ...
  Field id=f2 kind=SmoothMin a=f1 b=f3 k=s17
  Object id=o0 root=f0 identity=Ground material=Clay
  Feature id=g0 object=o0 kind=PlaneFace bounds=...
  Csg root=c2
    0000 PushObject o0
    0001 PushObject o1
    0002 Union
```

Ordering:

1. renderer;
2. parameter slots;
3. scalar nodes by ID;
4. field nodes by ID;
5. objects by ID;
6. features by ID;
7. CSG instructions.

No host paths, pointer values, hash values, wall time, or floating default formatting. Print finite f64 values with a dedicated round-trip formatter and normalize negative zero to `-0.0` only when the source semantics preserve it; otherwise canonicalize zero.

### 8.3 `frame-program` dump

This dump mirrors binary table order and includes every proof/capacity fact:

```text
FrameProgram v1 renderer=0 digest=<sha256>
  Header bytes=... flags=[opaque,transparent,kinetic]
  Table name=params count=...
  Table name=objects count=...
  Table name=features count=...
  Table name=events count=...
  Table name=exclusions count=...
  Table name=scalar_ops count=...
  Capacity kind=sheet_slots value=...
  Capacity kind=event_slots value=...
  Capacity kind=run_slots_per_tile value=...
  Event id=e0 kind=Silhouette feature=g3 repr=QuadricDiscriminant
  Exclusion pair=[g3,g8] reason=ProjectedBoundsDisjoint margin=...
```

The digest is SHA-256 over encoded `FrameProgram` bytes, not over dump text.

### 8.4 `render-layout` dump

```text
RenderLayout v1
  Renderer index=0
    FrameProgram base=0x... size=...
    State base=0x... size=...
    Coordinator core=0 actor=...
    Worker index=0 core=0 tiles=[0,135)
    Worker index=1 core=1 tiles=[135,270)
    Buffer front=0x... back=0x... bytes=...
    Workspace core=0 base=0x... bytes=...
    Probe base=0x... bytes=...
```

Every range is half-open. Hex is lowercase with `0x` and fixed natural width, not padded to host pointer width.

### 8.5 Image report section

Append one stable section per renderer after ordinary placement and before cost:

```text
  Renderer index=0 profile=AaaByteExact
    Field key=game.scene::world
    Material key=game.scene::shade
    Display ref=driver#0
    Mode width=1920 height=1080 refresh_hz=60 shade_hz=30
    FrameProgram version=1 digest=... bytes=...
    State bytes=...
    Framebuffer bytes=...
    Probe bytes=...
    Capacity objects=... features=... events=... sheets_per_row=...
    Capacity runs_per_tile=... transparent_layers=...
    Formal contract=pixels-v1
    Fallback bounded_local_rebuild=true dense_frame=false
```

The report must name any generated hot function and its owning core in the existing cost/convention sections. Missing cost ownership is a build error.

### 8.6 Build identity

The build input digest already covers source files. Add these renderer facts directly to report/build identity:

- frame-program digest;
- tone/transfer table digest;
- renderer profile revision;
- numeric-contract revision;
- formal theorem-set revision string;
- generated renderer-layout digest.

A frame program built under a different numeric-contract revision is a different image even when bytes accidentally match.

### 8.7 Reproducibility gates

Add an `xtask pixels-repro` command that builds each renderer golden twice in fresh temp directories and compares:

- field-graph dump bytes;
- frame-program dump bytes;
- encoded frame-program bytes;
- image report bytes;
- final image bytes.

It prints the first differing byte/line and fails. It never updates goldens.

---

## 9. Formal verification and theorem-to-kernel contract

The formal project is a production verification dependency for the renderer design, but it is not a Cargo dependency and it is not invoked by ordinary `wrela build`. It is invoked by repository milestone verification and whenever the numeric contract changes.

### 9.1 Toolchain pin

Create:

```text
formal/pixels/
  lean-toolchain
  lakefile.toml
  Pixels.lean
  Pixels/
```

Pin both Lean and Mathlib to tag `v4.30.0`. Do not use moving branches. Commit `lake-manifest.json` after the first successful `lake update`.

`formal/pixels/README.md` records:

- exact Lean/Mathlib versions;
- how to install `elan` without modifying the repository;
- `lake exe cache get` as an optional local acceleration;
- `lake build` as the normative check;
- how to run the axiom/admission scan;
- that the formal project proves generic mathematics and does not certify arbitrary compiler output without the compiler-side proof-object checks.

### 9.2 Formal module layout

Use these modules:

```text
Pixels/Dyadic.lean
Pixels/Interval.lean
Pixels/Projective.lean
Pixels/Primitive.lean
Pixels/SmoothMin.lean
Pixels/SupportTree.lean
Pixels/Csg.lean
Pixels/Bernstein.lean
Pixels/RootIsolation.lean
Pixels/Krawczyk.lean
Pixels/RunCertificate.lean
Pixels/EventCover.lean
Pixels/QOrder.lean
Pixels/FixedQ.lean
Pixels/Coverage.lean
Pixels/Normal.lean
Pixels/MaterialBound.lean
Pixels/Compositing.lean
Pixels/TransparencyTail.lean
Pixels/DisplayByte.lean
Pixels/Kinetic.lean
Pixels/Capacity.lean
Pixels/TrustBoundary.lean
```

Avoid one giant file. Every module imports only earlier layers. `Pixels.lean` imports `TrustBoundary` and contains the final `#print axioms` commands.

### 9.3 No-admission policy

The formal tree may contain none of:

```text
sorry
admit
axiom
unsafe
```

An `axiom` imported from Mathlib may appear in `#print axioms` output only if documented in `formal/pixels/EXPECTED_AXIOMS.md`. The expected initial list is ordinary classical/propositional extensionality machinery used by Mathlib, not project-defined assumptions.

Add `crates/xtask/src/pixels_formal.rs` to scan project source tokens before invoking Lean. Strip comments and strings before scanning so examples in documentation do not trip it.

### 9.4 Dyadic integer model

Define a normalized dyadic:

```lean
structure Dyadic where
  mantissa : Int
  exponent : Int
```

Its denotation is `mantissa * 2^exponent`. Runtime `Iv32` intervals use a shared exponent and signed i32 endpoints; formalize their denotation separately:

```lean
structure Iv32 where
  lo : Int
  hi : Int
  exponent : Int
  ordered : lo ≤ hi
```

Prove:

- endpoint containment;
- addition/subtraction widening;
- multiplication with i64 intermediate bounds;
- reciprocal preconditions for positive/negative intervals;
- exact min/max;
- monotone affine maps;
- conversion-radius containment from finite f32/f64 source values;
- checked-shift scaling;
- intersection containment;
- union containment.

The Lean model is mathematical integers. Rust/Wrela prove separately that checked machine operations return failure before exceeding their representable ranges.

### 9.5 Projective geometry theorems

Prove the exact camera cancellation:

```text
normalize(r) * (z / dot(normalize(r), f)) = z * r
```

under orthonormal camera basis and `dot(r,f)=1`.

Then prove projective equations for:

- plane;
- sphere;
- infinite cylinder;
- finite-cylinder side plus cap validity;
- cone;
- capsule side and cap features;
- box face;
- rounded-box face, edge cylinder, and corner sphere;
- torus quartic equivalence for positive q.

Each theorem has this shape:

```lean
q > 0 → FeatureValidity feature p →
  OriginalFieldZero feature (eye + rawRay u v / q) ↔
  ProjectivePolynomialZero feature u v q
```

Do not formalize the expanded SDF `abs/max/min/sqrt` as the primary feature proof. Prove feature decomposition plus feature-validity coverage.

### 9.6 Smooth-min and support theorems

For the exact polynomial smooth minimum, prove:

```text
min(a,b) - k/4 ≤ smin(a,b,k) ≤ min(a,b)
```

for `k > 0`, plus the exact active-band gap:

```text
min(a,b) - smin(a,b,k) = (k - |a-b|)^2 / (4*k)
```

when `|a-b| ≤ k`.

Define arbitrary smooth-min trees and prove:

```text
F_tropical(x) - budget(tree) ≤ F_smooth(x) ≤ F_tropical(x)
```

and:

```text
F_smooth(x) = 0 → ∃ leaf, leaf(x) ≤ budget(tree)
```

The compiler’s support-shell completeness checker directly instantiates this theorem.

Also prove the gradient convexity rule and Lipschitz preservation for the fused smooth-min definition.

### 9.7 CSG event algebra

Model occupancy as a Boolean expression over object-inside bits. Prove:

- an oriented boundary crossing toggles one bit;
- evaluating the CSG Boolean program before and after the toggle identifies whether the composite occupancy changes;
- equal cofactors imply the event is non-influential;
- skipping a non-influential event preserves root occupancy;
- a front-to-back ordered event sweep returns the first composite boundary transition;
- union, intersection, subtraction, and negation instruction encodings match their Boolean meanings.

The runtime CSG program is stack-based. Prove its evaluation equivalent to the compiler’s expression tree for well-formed programs.

### 9.8 Polynomial range and Bernstein theorems

Formalize only the bounded degree/variable forms used by `FrameProgram v1`:

- univariate degree ≤ 4;
- bivariate degree ≤ 3 in each variable;
- trivariate sparse Taylor model with explicit interval remainder.

Prove:

- Bernstein partition of unity;
- coefficient convex-hull enclosure;
- positivity/negativity from coefficient signs;
- derivative coefficient construction;
- de Casteljau subdivision preserving the represented polynomial;
- exact quadratic rectangle range candidate completeness: corners, interior stationary point, and edge stationary points;
- Taylor polynomial plus remainder containment.

Do not build a generic computer algebra library.

### 9.9 Root-isolation theorems

Prove the scalar bisection invariant:

- initial bracket has opposite signs or endpoint zero;
- each step preserves a bracket;
- width halves;
- returned interval contains at least one root under continuity.

Prove interval-Newton/Krawczyk uniqueness under the exact conditions used by runtime:

```text
K(E) ⊂ interior(E)
```

implies one unique correction root in `E` for every fixed parameter in the run domain.

Prove monotone-tube existence/uniqueness:

- derivative interval has one sign and excludes zero;
- endpoint field intervals have opposite uniform signs;
- therefore each ray has exactly one root in the tube.

### 9.10 Run-cover theorem

Define a `RunCertificate` abstractly with:

- half-open x domain;
- complete candidate feature set;
- one root certificate per active feature;
- exclusion proof for every omitted feature;
- stable feature validity/identity;
- strict adjacent q order;
- fixed CSG event sequence.

Prove:

1. every geometric root in the run belongs to an active feature;
2. every active feature contributes the roots claimed by its certificate and no others in its q interval;
3. the event sequence and occupancy automaton identify the composite visible crossing;
4. strict q order chooses the same crossing over the whole run;
5. therefore the run winner is the exact first visible boundary for every covered pixel center.

This is the central trust-boundary theorem. Its hypotheses must correspond one-for-one to fields in the runtime `CertifiedRun` record.

### 9.11 Event-cover theorem

The formal project does not prove that arbitrary generated event lists are complete. It proves a reusable conditional theorem:

```text
active generators cover every predicate family named by the structural scene
+ every omitted family has an exclusion proof over domain D
+ no active predicate reaches zero inside open run R
→ structural identities and order cannot change inside R
```

Compiler tests establish that every event family in `FrameProgram v1` is either emitted or excluded. Runtime intervals establish sign persistence.

### 9.12 q-order and braid theorems

Prove:

- interval containment plus strict interval separation implies exact order;
- adjacent strict order implies the complete order;
- the frontmost sheet is the first element;
- a bounded perturbation smaller than adjacent slack preserves order;
- failure-count zero for a fixed packet implies all packet relations hold;
- a local adjacent swap plus revalidation repairs the order if no simultaneous event is present.

Kinetic local-swap surgery remains an optimization. The theorem does not remove the need for bounded rebuild on simultaneous/degenerate events.

### 9.13 Fixed-q theorems

For integer quadratic forward differencing prove:

```text
q(n+1) = q(n) + dq(n)
dq(n+1) = dq(n) + ddq
```

and the four-lane stride recurrence. Prove:

- conversion radius plus recurrence error encloses real q;
- microtile reset bounds total error;
- strict q-order survives quantization when slack exceeds both candidates’ radii;
- all intermediate integer expressions stay in i32 when compiler-produced setup bounds hold;
- packet and scalar recurrence produce identical integer values.

### 9.14 Coverage and normal theorems

For line and quadratic edge models prove conservative pixel coverage intervals under the fixed box filter.

For inverse-depth sheets prove the camera-space normal formula:

```text
N = (q_u, q_v, q - u*q_u - v*q_v)
```

up to positive scale, and propagate q/q-derivative intervals into a normal cone.

Prove color error from coverage error:

```text
|ΔC| ≤ |Δα| * |C_front - C_back|
```

per channel and for the maximum channel norm used by the scheduler.

### 9.15 Material/filter theorems

Prove generic bounds used by runtime:

- Lipschitz post-map error propagation;
- affine and quadratic centered filter moments;
- bounded BRDF second derivative gives a moment-filter error bound;
- product/separable approximation error composition;
- low-rank sum residual plus arithmetic radius bounds exact shading;
- deterministic Simpson three-point integration is exact for cubic shutter models;
- source-domain interval quadrature enclosure composition.

Do not prove that every material is low rank. Prove that a representation with a supplied residual bound is safe.

### 9.16 Transparency and display theorems

For premultiplied transfer records `(C,T)`, prove associativity, identity, ordered summary concatenation, and local subtree replacement.

Prove tail cutoff:

```text
T_prefix * max_suffix_radiance_deviation ≤ ε
```

implies replacing the suffix by its proxy changes final radiance by at most `ε`.

Prove monotone endpoint byte singleton:

```text
x ∈ [lo,hi]
monotone encode
quantize(encode(lo)) = quantize(encode(hi))
→ quantize(encode(x)) has that code
```

The runtime separately validates LUT monotonicity and interval enclosure of every pre-LUT stage.

### 9.17 Kinetic theorems

Prove:

- implicit sheet flow `q_t = -G_t/G_q` from the differentiated zero equation;
- first/second-order transport plus remainder encloses future q;
- compressed minimum slack preserves all margins when total perturbation is smaller;
- adjacent order survives bounded drift;
- event sign survives bounded drift;
- unchanged frame-input digest permits exact framebuffer reuse.

The last theorem is an equality theorem over a declared dependency tuple; the compiler’s dependency collector must supply that tuple completely.

### 9.18 Capacity theorems

Capacity calculations are finite arithmetic, not asymptotic claims. Prove generic lemmas for:

- sum/product checked bounds;
- maximum overlapping interval count bounded by endpoint sweep;
- per-row sheet slots from projected feature spans;
- event slots from emitted generators plus maximum subdivisions;
- run slots from event endpoints plus one;
- transfer-tree nodes from layer capacity;
- worker workspace from per-tile maxima;
- framebuffer bytes from tile geometry and buffering count.

The compiler instantiates these lemmas in Rust and rejects overflow/ceiling violations.

### 9.19 Theorem-to-kernel manifest

Create `formal/pixels/KERNELS.toml`, parsed by a tiny local parser in xtask or represented as a simple line format to avoid a new dependency. Each row names:

```text
theorem = Pixels.FixedQ.packet_eq_scalar
rust = crates/wrela-compiler/src/pixels/reference/fixed_q.rs::raster_q4
wrela_scalar = core.render_raster::raster_q4_scalar
wrela_packet = core.render_raster::raster_q4
goldens = fixed-q-basic,fixed-q-near-overflow
```

Required mappings include:

- interval add/sub/mul/intersection;
- quadratic exact range;
- root bracket step;
- monotone run certificate predicate;
- q-order packet;
- fixed-q packet recurrence;
- line/quadratic coverage;
- transfer composition;
- transparency tail predicate;
- byte singleton predicate;
- kinetic slack predicate.

`cargo xtask pixels-formal-map` fails if a manifest symbol is absent from source or a required kernel lacks a mapping.

### 9.20 Formal gate

Add to `verify-milestone`:

```text
cargo xtask pixels-formal
```

That command:

1. checks tool presence and exact versions;
2. scans for forbidden admissions;
3. runs `lake build` in `formal/pixels`;
4. runs the theorem-to-kernel manifest check;
5. captures `#print axioms` output;
6. compares it to a checked-in normalized expected file;
7. fails on any difference.

Ordinary `verify` runs only the source token scan, manifest shape check, and focused Rust/Wrela differential tests. It must stay within the repository’s default unit-test budget.

---

## 10. Implementation program

### 10.0 Executor rules for every task

Each task below is one commit unless the task explicitly says otherwise.

For every task, the executor must:

1. read the listed prerequisite task outputs and current stable dumps;
2. make only the files listed under **Files**, except compilation-only fallout;
3. add focused Rust unit tests beside implementation code;
4. add or update named golden fixtures;
5. run `cargo fmt --all`;
6. run `cargo xtask verify`;
7. commit only after the gate is green;
8. run `cargo xtask verify-milestone` at milestone close;
9. never update a golden before reading and explaining the diff;
10. never loosen a numeric tolerance, capacity, or error budget to make a test pass;
11. never use fieldprobe output, dense truth, or previous-frame data to make a renderer decision;
12. never reinterpret `Unresolved` or `RenderError` as background or success.

The task descriptions below already choose the algorithm. When implementation reveals an internal contradiction, stop at that task, preserve the failing fixture, and fix the contradiction in the plan/documentation before proceeding. Do not substitute a different renderer architecture inside a code commit.

---

# Milestone P0 — contract, scaffolding, and permanent fixtures

Milestone result: the repository knows that Pixels is a production compiler subsystem, has stable empty-stage dumps, a pinned formal project, and a fixed permanent fixture corpus. No renderer semantics exist yet.

## Task P0.1 — add the normative implementation chapter

**Purpose**

Place the closed source/compiler/runtime contract in the repository before code depends on it.

**Files**

```text
docs/language/07-pixels.md
docs/language/04-compiler.md
docs/language/05-library.md
docs/language/06-machine.md
docs/designs/pixels.md
```

**Work**

- Add `07-pixels.md` containing sections 0–5 of this plan in normative form.
- Amend compiler chapter §5 to name `FieldGraph` and `FrameProgram` as compiler-owned data, not executable IR.
- Add `@field`, `@material`, `@range`, `@rate`, `Image.renderer`, renderer public types, and SIMD kernel obligation to library chapter.
- Add `frameprog`/`pixelsdata` image regions and generated renderer actors to machine chapter while preserving machine-v1 display semantics.
- Mark the existing `docs/designs/pixels.md` historical measurements as evidence only and link to the normative chapter.
- Preserve the unfavorable online fieldprobe result. Do not rewrite history to imply it validated this renderer.

**Acceptance criteria**

- Every new source spelling appears in exactly one normative chapter.
- The chapters state that the validated sweep is correct without kinetic state.
- The chapters state that `AaaByteExact` rejects unsupported source at build time.
- No normative statement cites modeled/Pi-unmeasured performance as fact.
- Documentation links resolve relative to their files.

**Gate**

```text
cargo xtask verify
```

**Commit**

```text
pixels P0.1: write the production Pixels contract
```

## Task P0.2 — add compiler module scaffolding and empty dumps

**Purpose**

Establish module ownership and stable stage names before features.

**Files**

```text
crates/wrela-compiler/src/lib.rs
crates/wrela-compiler/src/pixels/mod.rs
crates/wrela-compiler/src/pixels/diagnostics.rs
crates/wrela-compiler/src/pixels/dump.rs
crates/wrela-compiler/src/bin/wrela.rs
tests/golden/check-pixels-empty/input.wr
tests/golden/check-pixels-empty/expected/field-graph.txt
tests/golden/check-pixels-empty/expected/frame-program.txt
tests/golden/check-pixels-empty/expected/render-layout.txt
```

**Work**

- Add `pub mod pixels;`.
- Define `PixelsError` and `PixelsDiagnostic` without renderer behavior.
- Add CLI stage parsing for `field-graph`, `frame-program`, `render-layout`, and `--renderer=<index>`.
- Until a renderer exists, dumps print version headers plus `Renderers count=0`.
- A renderer index on an image with no renderer is a clear build error.
- Add the three stages to CLI help and stage-validation tests.

**Code shape**

```rust
pub enum PixelsDumpStage {
    FieldGraph,
    FrameProgram,
    RenderLayout,
}

pub fn dump_empty(stage: PixelsDumpStage) -> String;
```

**Acceptance criteria**

- All three dump stages produce byte-stable empty outputs.
- Existing stage behavior and usage text remain unchanged except for additions.
- Unknown `--renderer` use is rejected, not ignored.
- No renderer code is imported by sema, eval, lower, or layout yet.

**Gate**

```text
cargo xtask verify
```

**Commit**

```text
pixels P0.2: add stage and module scaffolding
```

## Task P0.3 — create the formal project skeleton

**Purpose**

Pin the formal environment and make admission checks permanent before theorem work.

**Files**

```text
formal/pixels/lean-toolchain
formal/pixels/lakefile.toml
formal/pixels/Pixels.lean
formal/pixels/Pixels/TrustBoundary.lean
formal/pixels/README.md
formal/pixels/EXPECTED_AXIOMS.txt
crates/xtask/src/pixels_formal.rs
crates/xtask/src/main.rs
```

**Work**

- Pin Lean/Mathlib `v4.30.0`.
- Add a trivial theorem with `#print axioms`.
- Implement comment/string-aware forbidden-token scanning.
- Add `cargo xtask pixels-formal` and `pixels-formal-scan`.
- `verify` runs the scan only.
- `verify-milestone` runs the complete formal command.
- Missing Lean in the milestone environment fails closed with installation instructions; it is not silently skipped.

**Acceptance criteria**

- No project source contains an admission.
- `pixels-formal-scan` is platform portable.
- Formal build output is normalized before comparison.
- The ordinary Cargo dependency graph is unchanged.

**Gate**

```text
cargo xtask verify
cargo xtask verify-milestone
```

**Commit**

```text
pixels P0.3: pin the Lean verification project
```

## Task P0.4 — install the permanent renderer fixture corpus

**Purpose**

Name every correctness class before implementation so later code cannot select only favorable examples.

**Files**

```text
tests/golden/check-pixels-plane/
tests/golden/check-pixels-hard-csg/
tests/golden/check-pixels-smooth-csg/
tests/golden/check-pixels-repeat/
tests/golden/check-pixels-displace/
tests/golden/check-pixels-close-depth/
tests/golden/check-pixels-thin-feature/
tests/golden/check-pixels-enclosed-feature/
tests/golden/check-pixels-material-edge/
tests/golden/check-pixels-transparent-tail/
tests/golden/check-pixels-area-light/
tests/golden/check-pixels-kinetic/
tests/golden/err-pixels-unsupported-op/
tests/golden/err-pixels-missing-range/
tests/golden/err-pixels-topology-branch/
stdlib/tests/pixels_contract.wr
tests/census.toml
```

**Work**

- Add source fixtures with expected placeholder errors saying the production Pixels stage is not implemented.
- Record all fixture names in the existing test census.
- Geometry is deterministic and documented in a `README.md` inside each complex fixture.
- Thin/enclosed/close-depth cases use exact integer or dyadic source constants, not random placement.
- Add expected final-frame digest placeholders only where the golden harness already supports boot output; do not invent a second fixture system.

**Acceptance criteria**

- Every fixture is discovered by ordinary golden enumeration.
- Each adversarial scene states the failure class it protects.
- No fixture uses a dense edge mask or precomputed renderer hints as source input.
- The test census refuses accidental deletion.

**Gate**

```text
cargo xtask verify
```

**Commit**

```text
pixels P0.4: pin the permanent renderer corpus
```

### Milestone P0 close

Run:

```text
cargo xtask verify-milestone
```

The milestone is closed when documentation, empty dumps, formal skeleton, and fixture census are all pinned. No placeholder may say “choose later”; it may only say “implemented in task Px.y.”

---

# Milestone P1 — source surface, semantic checks, and image declaration

Milestone result: Wrela can type-check `@field`, `@material`, ranges/rates, and `Image.renderer`; the sealed image graph contains renderer declarations. No symbolic graph or frame program is emitted yet.

## Task P1.1 — add standard-library field and renderer types

**Purpose**

Make the source API parse and type-check using ordinary Wrela declarations while keeping constructors sealed.

**Files**

```text
stdlib/core/field.wr
stdlib/core/render.wr
stdlib/core/prelude.wr
stdlib/tests/pixels_contract.wr
```

**Work**

Define:

- `Vec2`, `Vec3`, `Vec4`, `Rgb`, `Aabb`, `Camera`;
- opaque `Field`;
- `ObjectId`, `MaterialId` as user enums accepted by `mark`;
- primitive/composition function signatures from §2.2;
- `SurfaceContext`;
- `MaterialSample` closed constructors;
- `RenderProfile`, `ToneCurve`, `RenderFrame[P]`, `RenderedFrame[P]`, `RenderError`;
- opaque `Renderer[P]` actor handle surface;
- `Image.renderer[P]` declaration signature in the image-builder surface.

The Wrela bodies of compiler-recognized field constructors may be `panic("compiler intrinsic")` if they can never execute. Ordinary scalar/material helper bodies must be real Wrela.

**Acceptance criteria**

- Source examples in §0 parse.
- User code cannot construct arbitrary `Field` storage or access its representation.
- `MaterialSample` constructors validate finite/clamped source arguments at runtime where appropriate.
- Existing prelude users see no ambiguous names.
- No compiler intrinsic exists solely because a normal Wrela body would suffice.

**Gate**

```text
cargo xtask verify
```

**Commit**

```text
pixels P1.1: add field and render library surfaces
```

## Task P1.2 — classify `@field` and `@material` attributes

**Purpose**

Carry annotations through declaration and typed-body checking.

**Files**

```text
crates/wrela-compiler/src/sema/types.rs
crates/wrela-compiler/src/sema/bodies.rs
crates/wrela-compiler/src/sema/typed.rs
crates/wrela-compiler/src/sema/mod.rs
tests/golden/check-pixels-plane/
tests/golden/err-pixels-field-signature/
tests/golden/err-pixels-material-signature/
```

**Work**

- Add `is_field` and `is_material` to declaration/typed function metadata, or canonical keyed metadata in `TypedProgram` as specified in §6.1.
- Reject attribute arguments.
- Reject duplicate attributes and functions carrying both.
- Validate signature forms exactly.
- Reject `async`, `@task`, `@image`, `@layout_assert`, receiver methods in v1, variadic/closure returns, and resource parameters.
- Permit imported and specialized generic helper calls, but field/material root functions themselves are nongeneric in v1.
- Print metadata in `--stage=typed`.

**Exact signature checks**

```text
@field fn f(p: Vec3) -> Field
@field fn f(p: Vec3, read params: P) -> Field
@material fn m(surface: SurfaceContext) -> MaterialSample
@material fn m(surface: SurfaceContext, read params: P) -> MaterialSample
```

Parameter names are not semantic. Order and types are.

**Acceptance criteria**

- Every invalid shape has a focused golden with `P001` or `P002`.
- Typed dumps contain canonical root keys and parameter indexes.
- Generic helper instantiations retain root call resolution.
- Attribute handling is deterministic across module import order.

**Gate**

```text
cargo xtask verify
```

**Commit**

```text
pixels P1.2: type field and material roots
```

## Task P1.3 — implement `@range` and `@rate` attributes

**Purpose**

Capture the finite parameter domain needed by all later proofs.

**Files**

```text
crates/wrela-compiler/src/sema/types.rs
crates/wrela-compiler/src/sema/bodies.rs
crates/wrela-compiler/src/sema/typed.rs
crates/wrela-compiler/src/sema/attrs.rs
tests/golden/check-pixels-ranges/
tests/golden/err-pixels-range/
tests/golden/err-pixels-rate/
```

**Work**

Create one reusable attribute parser for numeric named arguments. Accept attributes on scalar fields of data structs reachable from renderer parameter type `P`.

Rules:

- `@range` requires exactly `min` and `max`;
- values are comptime finite scalar constants convertible to the field scalar type;
- `min <= max`;
- integer ranges are exact;
- vector fields place attributes on each scalar component in v1; struct-level/vector shorthand is not supported;
- `@rate` requires `max_delta >= 0` and `max_second_delta >= 0`;
- rate units are per presented frame at declared `refresh_hz`;
- a zero rate means statically unchanged after initialization;
- attributes on non-render-parameter fields are legal but ignored by Pixels and still print in typed metadata only if semantically retained.

**Acceptance criteria**

- Range/rate metadata is keyed by stable field-index paths, not source field names alone.
- Rename with identical layout changes the source digest but not field-path ordering.
- NaN, infinity, reversed range, unknown label, duplicate label, and nonconstant values are rejected.
- Exact diagnostic spans point at the bad attribute argument.

**Gate**

```text
cargo xtask verify
```

**Commit**

```text
pixels P1.3: record parameter range and rate contracts
```

## Task P1.4 — add closed field intrinsic typing

**Purpose**

Recognize the field operation surface without allowing arbitrary `Field` manipulation.

**Files**

```text
crates/wrela-compiler/src/sema/intrinsics.rs
crates/wrela-compiler/src/sema/bodies.rs
crates/wrela-compiler/src/sema/types.rs
stdlib/core/field.wr
tests/golden/check-pixels-field-ops/
tests/golden/err-pixels-field-private/
```

**Work**

- Add every field constructor/combinator to the written-down intrinsic census.
- Type labels and generic arguments exactly.
- `mark` accepts only comptime enum variants for object/material identity in v1.
- `repeat` requires a positive finite period and explicit finite axis mask.
- transforms accept only rigid/uniform-scale operations in v1; nonuniform scale is a separate `ellipsoid` primitive or a build error.
- `bounded_displace` requires declared amplitude, gradient, and Hessian bounds.
- ordinary arithmetic on `Field` is unavailable.
- field values cannot be stored in user structs, arrays, statics, actors, messages, or returned from non-`@field` public APIs.

**Acceptance criteria**

- Intrinsic census equals all producer sites.
- Every field op is typed in one central match.
- `Field` cannot escape its root expression graph.
- Existing intrinsic diagnostics/census remain green.

**Gate**

```text
cargo xtask verify
```

**Commit**

```text
pixels P1.4: seal and type the field operation set
```

## Task P1.5 — enforce field/material body legality

**Purpose**

Reject source forms that prevent finite structural compilation.

**Files**

```text
crates/wrela-compiler/src/pixels/legality.rs
crates/wrela-compiler/src/sema/mod.rs
tests/golden/err-pixels-topology-branch/
tests/golden/err-pixels-field-recursion/
tests/golden/err-pixels-field-loop/
tests/golden/err-pixels-field-effects/
```

**Work**

Walk transitive callees from each root and classify operations.

For field roots reject:

- recursion of any kind;
- `while`;
- `for` without a comptime exact extent;
- `await`, `send`, actor calls, groups;
- placed/static mutation, MMIO, entropy, time, panic on a reachable path;
- runtime branch selecting different field topology;
- function values or indirect calls;
- dynamic indexing whose finite alternatives cannot be unrolled.

Permit a runtime branch only when both arms compile to identical field topology and differ solely in scalar coefficients. Establish identity by canonical field-shape hash after symbolic compilation; before that stage, mark the branch as `NeedsTopologyEqualityCheck` rather than accepting it.

Material roots may branch on material identity, explicit event-classified scalar predicates, and ordinary bounded values. A material discontinuity affecting output must be surfaced later as an event predicate.

**Acceptance criteria**

- Every rejected effect names the transitive call chain.
- Recursive SCC diagnostics list all cycle members.
- Fixed loops are unrolled deterministically in source order.
- No body is accepted because its unsupported branch happened to be unreachable under one sample.

**Gate**

```text
cargo xtask verify
```

**Commit**

```text
pixels P1.5: enforce finite renderer body legality
```

## Task P1.6 — implement `Image.renderer` intrinsic construction

**Purpose**

Record renderer declarations through ordinary comptime image evaluation.

**Files**

```text
crates/wrela-compiler/src/sema/intrinsics.rs
crates/wrela-compiler/src/sema/bodies.rs
crates/wrela-compiler/src/eval/interp.rs
crates/wrela-compiler/src/eval/image.rs
crates/wrela-compiler/src/eval/value.rs
crates/wrela-compiler/src/report.rs
tests/golden/check-pixels-plane/
tests/golden/err-pixels-renderer-decl/
```

**Work**

- Add `Image.renderer` to the image-builder intrinsic surface and intrinsic census.
- Resolve `P` as the type argument.
- Preserve function references as `Value::Fn` in declaration args.
- Add `ImageDeclRef::Renderer` rendering and recursive declaration-reference scanning.
- Add renderer blocks to the `--stage=image` dump and ordinary report.
- `renderer.handle()` produces `Actor[Renderer[P]]` only after generated actor synthesis exists; until P5 it returns a typed opaque declaration ref accepted only by image construction. Do not fake a numeric actor ID.

**Acceptance criteria**

- Multiple renderers preserve source construction order.
- Renderer declaration references participate in DAG cycle checks.
- Two renderers may share field/material functions but not claim the same display driver.
- Unknown/duplicate labels are rejected during sema/eval, not ignored.
- Image dump round-trips deterministic enum/function renderings.

**Gate**

```text
cargo xtask verify
```

**Commit**

```text
pixels P1.6: record renderer image declarations
```

## Task P1.7 — validate sealed renderer declarations

**Purpose**

Make an image with a renderer a closed, self-consistent build fact.

**Files**

```text
crates/wrela-compiler/src/eval/image_checks.rs
crates/wrela-compiler/src/pixels/config.rs
crates/wrela-compiler/src/pixels/diagnostics.rs
tests/golden/err-pixels-renderer-display/
tests/golden/err-pixels-renderer-params/
tests/golden/err-pixels-renderer-mode/
```

**Work**

Implement §6.4–6.5. Centralize enum variant decoding, integer/floating extraction, function-ref extraction, display reference validation, mode checks, and profile validation.

No renderer compilation occurs yet. `check_sealed` only returns a validated `RendererConfig` side table keyed by declaration index. Store it in a new build-closure structure rather than recomputing it from `ImageGraph` in every stage.

**Acceptance criteria**

- Every required argument has one diagnostic.
- Every function reference resolves to a matching annotated root.
- Cross-module roots work through the ordinary checked closure.
- Parameter type equality is structural/canonical, not string comparison.
- `refresh_hz % shade_hz != 0` is rejected.
- Non-finite camera/world/light values are rejected.

**Gate**

```text
cargo xtask verify
```

**Commit**

```text
pixels P1.7: seal renderer configuration contracts
```

## Task P1.8 — complete P1 dumps and fixtures

**Purpose**

Pin source/typed/image behavior before symbolic compilation.

**Files**

```text
crates/wrela-compiler/src/sema/typed.rs
crates/wrela-compiler/src/eval/image.rs
crates/wrela-compiler/src/report.rs
tests/golden/check-pixels-*/expected/check.txt
tests/golden/check-pixels-*/expected/typed.txt
tests/golden/check-pixels-*/expected/image.txt
```

**Work**

- Update only P1-relevant fixtures.
- Add unit tests for renderer ordering, function references, path metadata, and construction DAG behavior.
- Add a report-determinism case with two modules and two renderer declarations.
- Ensure no field/frame-program dump contains implementation data yet; it prints `Compilation status=not-run` with renderer config, not placeholder failure.

**Acceptance criteria**

- P1 accepted fixtures reach sealed image configuration.
- All P1 rejected fixtures fail before lower/codegen.
- Report determinism passes under reversed filesystem/module discovery order.
- Existing non-Pixels image goldens change only where enum rendering gained `Renderer` support, with reviewed diffs.

**Gate**

```text
cargo xtask verify
cargo xtask report-determinism
```

**Commit**

```text
pixels P1.8: pin renderer source and image dumps
```

### Milestone P1 close

Run `cargo xtask verify-milestone`. The milestone is closed only when the complete source API type-checks and every bad declaration fails before symbolic compilation.

---

# Milestone P2 — dedicated symbolic field and material compiler

Milestone result: accepted roots compile into deterministic scalar, field, and material graphs with exact source identities. No geometric bounds, feature decomposition, or binary frame program exists yet.

## Task P2.1 — implement stable arenas and IDs

**Purpose**

Create deterministic storage for all symbolic nodes.

**Files**

```text
crates/wrela-compiler/src/pixels/ids.rs
crates/wrela-compiler/src/pixels/arena.rs
crates/wrela-compiler/src/pixels/scalar.rs
crates/wrela-compiler/src/pixels/graph.rs
crates/wrela-compiler/src/pixels/material_graph.rs
crates/wrela-compiler/src/pixels/mod.rs
```

**Work**

Implement newtype IDs and append-only arenas. IDs are assigned only after child nodes exist. Provide checked getters returning internal errors, not panics on user-triggerable paths.

Node equality for canonicalization is structural and excludes source span. Keep `NodeOrigin { primary, expansion_chain }` in a side table keyed by ID.

**Acceptance criteria**

- IDs format exactly as §4.1.
- Arena iteration is insertion order.
- Origin side tables cover every node.
- No node owns a `HashMap`, `Rc`, `Arc`, trait object, or closure.
- Unit tests cover stale/wrong-arena ID detection in debug helpers.

**Gate**

```text
cargo xtask verify
```

**Commit**

```text
pixels P2.1: add deterministic symbolic arenas
```

## Task P2.2 — implement scalar symbolic values

**Purpose**

Compile ordinary scalar/vector expressions referenced by field/material operations.

**Files**

```text
crates/wrela-compiler/src/pixels/symbolic.rs
crates/wrela-compiler/src/pixels/scalar.rs
crates/wrela-compiler/src/pixels/params.rs
crates/wrela-compiler/src/pixels/diagnostics.rs
```

**Work**

Implement `SymValue` from §4.2 and scalar node kinds for:

- all scalar constants preserving exact source f32/f64 bits;
- parameter field paths;
- vector construction/projection;
- checked and wrapping integer arithmetic where used for compile-time indexing;
- float add/sub/mul/div/neg;
- min/max/abs/clamp;
- sqrt/rsqrt/sin/cos with fixed semantic op IDs;
- dot, cross, length, normalize as fused scalar/vector nodes;
- comparisons used only in topology-equality/material event branches;
- tuple/struct temporary values needed by helper functions.

Parameter paths are resolved through typed field indices. Store human spelling only in diagnostics/dumps.

**Acceptance criteria**

- Constant bit patterns, including negative zero, survive the graph dump.
- Parameter path collection is exact and deterministic.
- Unsupported scalar operation reports `P004` with call chain.
- Division/reciprocal records a denominator proof obligation; it is not assumed nonzero.
- Fused operations retain source-level semantics through explicit op definitions.

**Gate**

```text
cargo xtask verify
```

**Commit**

```text
pixels P2.2: compile scalar renderer expressions
```

## Task P2.3 — implement symbolic call/control evaluation

**Purpose**

Evaluate renderer roots and helpers without reusing generic comptime values.

**Files**

```text
crates/wrela-compiler/src/pixels/symbolic.rs
crates/wrela-compiler/src/pixels/legality.rs
crates/wrela-compiler/src/pixels/quota.rs
```

**Work**

Implement:

- lexical environment stack;
- typed local assignment and immutable value semantics;
- direct calls by `CalleeKey`;
- canonical generic instance lookup;
- `let`, expression statement, return;
- `if` with compile-time condition;
- coefficient-only runtime branch represented as `SymSelect` pending topology equality;
- exact bounded `for` unrolling;
- `match` over compile-time enum or explicit material/object identity;
- call-depth, node-count, loop-expansion, and symbolic-memory quotas;
- error stack preserving root/helper call chain.

No `while`, `await`, send, closure invocation, mutation through aliases, or exception-like recovery.

**Acceptance criteria**

- The evaluator is total over the accepted legality subset.
- Quota exhaustion is a `pixels` build error, not panic or partial graph.
- Identical helper calls can later CSE but preserve all source origins.
- Runtime branch arms are both compiled; neither is selected by a sample value.
- Fixed loop expansion order is ascending source iteration order.

**Gate**

```text
cargo xtask verify
```

**Commit**

```text
pixels P2.3: evaluate finite renderer bodies symbolically
```

## Task P2.4 — lower field operations into `FieldGraph`

**Purpose**

Build the exact structural field expression from the closed intrinsic surface.

**Files**

```text
crates/wrela-compiler/src/pixels/symbolic.rs
crates/wrela-compiler/src/pixels/graph.rs
crates/wrela-compiler/src/pixels/field_intrinsics.rs
```

**Work**

For every field intrinsic, parse labels once and emit a typed `FieldNode`:

```rust
enum FieldKind {
    Plane { point: Vec3Expr, normal: Vec3Expr, offset: ScalarId },
    Sphere { point: Vec3Expr, center: Vec3Expr, radius: ScalarId },
    Box { point: Vec3Expr, center: Vec3Expr, half: Vec3Expr },
    RoundBox { point: Vec3Expr, center: Vec3Expr, half: Vec3Expr, radius: ScalarId },
    Capsule { point: Vec3Expr, a: Vec3Expr, b: Vec3Expr, radius: ScalarId },
    Cylinder { ... },
    Cone { ... },
    Torus { ... },
    HardMin { a: FieldId, b: FieldId },
    HardMax { a: FieldId, b: FieldId },
    Neg { child: FieldId },
    SmoothMin { a: FieldId, b: FieldId, k: ScalarId },
    SmoothMax { a: FieldId, b: FieldId, k: ScalarId },
    Transform { child: FieldId, transform: TransformExpr },
    Repeat { child: FieldId, spec: RepeatSpecExpr },
    BoundedDisplace { child: FieldId, displacement: ScalarId, contract: DeformContract },
    Mark { child: FieldId, object: ObjectKey, material: MaterialKey },
    SelectSameTopology { cond: ScalarId, then_field: FieldId, else_field: FieldId },
}
```

Do not immediately flatten transforms or marks. Preserve source structure.

**Acceptance criteria**

- Every closed field op has one lowering path and one unit test.
- Missing/duplicate labels cannot reach this pass.
- Object/material keys remain nominal enum identity, not bit masks.
- Transform composition preserves source order.
- Field graph can represent the complete permanent fixture source set.

**Gate**

```text
cargo xtask verify
```

**Commit**

```text
pixels P2.4: lower the closed field graph
```

## Task P2.5 — lower material operations into `MaterialGraph`

**Purpose**

Capture material semantics and output dependencies structurally.

**Files**

```text
crates/wrela-compiler/src/pixels/material_graph.rs
crates/wrela-compiler/src/pixels/material_intrinsics.rs
crates/wrela-compiler/src/pixels/symbolic.rs
```

**Work**

Material graph values include:

- base color;
- opacity;
- emissive RGB;
- roughness;
- metallic;
- specular level/IOR;
- normal/slope perturbation model;
- supported analytic procedural patterns;
- explicit material identity selection;
- light-independent scalar/vector intermediates.

Represent output-affecting runtime branches as explicit `MaterialSelect { predicate, a, b }`. Record a pending event obligation for discontinuous predicates.

Do not compile lights, shadows, AO, or probes into the material graph. Those belong to renderer configuration and runtime shading.

**Acceptance criteria**

- Every `MaterialSample` field has a graph source or constructor default.
- Alpha is clamped only according to source constructor semantics, not compiler convenience.
- Material identity match compiles into a finite table keyed by nominal variant.
- Unsupported procedural texture or indirect call is `P004`/`P014`.
- Graph dump identifies all parameter and surface-context dependencies.

**Gate**

```text
cargo xtask verify
```

**Commit**

```text
pixels P2.5: compile material semantics
```

## Task P2.6 — canonicalize and CSE symbolic graphs

**Purpose**

Produce one deterministic graph independent of helper inlining accidents while preserving exact semantics.

**Files**

```text
crates/wrela-compiler/src/pixels/canonicalize.rs
crates/wrela-compiler/src/pixels/scalar.rs
crates/wrela-compiler/src/pixels/graph.rs
crates/wrela-compiler/src/pixels/material_graph.rs
```

**Work**

Canonicalization performs only semantics-preserving rewrites fixed here:

- constant fold with the same bit-exact scalar helpers as comptime eval;
- eliminate identity transform;
- compose adjacent rigid transforms in source order;
- eliminate `min(x,x)`, `max(x,x)`, `smin(x,x,k)` only where exact source semantics agree;
- canonicalize commutative hard min/max child order by stable structural key;
- do **not** reorder smooth min/max unless the source operation is proven bit-commutative by the math contract;
- preserve saturated smooth branch identity;
- hash-cons exact equal scalar/field/material nodes;
- merge origins into stable sorted span lists;
- resolve `SelectSameTopology` only after shape equality; otherwise emit `P003`.

Use a deterministic structural key enum, not serialized debug text.

**Acceptance criteria**

- Canonicalization is idempotent.
- Running it twice produces byte-identical dumps and IDs.
- Differential unit tests compare pre/post scalar evaluation over deterministic input grids.
- Smooth-min one-ulp/saturation fixtures remain exact.
- A coefficient-only branch with equal topology succeeds; unequal topology fails.

**Gate**

```text
cargo xtask verify
```

**Commit**

```text
pixels P2.6: canonicalize renderer graphs exactly
```

## Task P2.7 — emit complete `field-graph` dumps

**Purpose**

Pin the symbolic representation before geometric proof work.

**Files**

```text
crates/wrela-compiler/src/pixels/dump.rs
crates/wrela-compiler/src/bin/wrela.rs
tests/golden/check-pixels-*/expected/field-graph.txt
```

**Work**

Implement §8.2. Include:

- root keys and config;
- parameter paths and source contracts;
- scalar nodes;
- field nodes;
- material nodes;
- source identity tables;
- pending proof/event obligations;
- symbolic quota counts.

Do not include future bounds/features as zero-valued fake facts. Print `Analysis status=pending` once.

**Acceptance criteria**

- All accepted permanent fixtures produce stable graph dumps.
- Unsupported fixtures fail before a partial dump is emitted.
- Reordering independent source helper declarations does not change canonical node order when call graph/semantics are unchanged.
- Round-trip float formatting reproduces bits in a parser unit test.

**Gate**

```text
cargo xtask verify
cargo xtask report-determinism
```

**Commit**

```text
pixels P2.7: pin symbolic field and material dumps
```

### Milestone P2 close

Run `cargo xtask verify-milestone`. The graph dump becomes a compatibility artifact; later changes must be deliberate and reviewed.

---

# Milestone P3 — structural proofs, bounds, objects, features, and capacities

Milestone result: every field/material graph is converted into a finite structural scene with conservative parameter/value/derivative bounds, complete smooth-CSG support shells, maximal smooth objects, explicit hard-CSG logic, fused primitive features, and exact compile-time capacity derivations. Projective equations and runtime events are not yet emitted.

## Task P3.1 — collect exact renderer parameter dependencies

**Purpose**

Determine the smallest coefficient snapshot and the complete frame dependency tuple.

**Files**

```text
crates/wrela-compiler/src/pixels/params.rs
crates/wrela-compiler/src/pixels/report.rs
crates/wrela-compiler/src/pixels/dump.rs
```

**Work**

Traverse scalar, field, material, camera, light, exposure, tone, and probe configuration graphs. For each referenced path record:

```rust
pub enum ParamUse {
    Geometry,
    Material,
    Camera,
    Light,
    Exposure,
    Post,
    Probe,
}

pub struct ParamSlot {
    pub id: ParamId,
    pub path: Vec<FieldIndex>,
    pub scalar_ty: ScalarType,
    pub range: ScalarRange,
    pub rate: ScalarRate,
    pub uses: BTreeSet<ParamUse>,
    pub packed_offset: u32,
}
```

Pack in lexicographic field-index-path order with natural scalar alignment. Do not pack unused fields of `P`.

Create a complete dependency digest schema over:

- packed parameter bytes;
- camera coefficients;
- light coefficients;
- exposure/post IDs;
- probe version;
- output mode;
- deterministic frame phase.

**Acceptance criteria**

- A parameter used in both geometry and material has one slot and two uses.
- Unused fields contribute zero renderer-state bytes.
- Every referenced path has `@range`; every nonzero/changeable use has `@rate`.
- Static zero-rate parameters are marked immutable and may be folded later.
- Dependency dump is independent of source field spelling.

**Gate**

```text
cargo xtask verify
```

**Commit**

```text
pixels P3.1: derive renderer coefficient dependencies
```

## Task P3.2 — propagate scalar value intervals

**Purpose**

Give every scalar/vector node a conservative finite range over the complete declared parameter/world domain.

**Files**

```text
crates/wrela-compiler/src/pixels/bounds.rs
crates/wrela-compiler/src/pixels/scalar.rs
crates/wrela-compiler/src/pixels/reference/interval.rs
formal/pixels/Pixels/Interval.lean
```

**Work**

Implement host compile-time `F64Interval` with checked finite endpoints. This is not the runtime dyadic interval; it is a compiler analysis domain.

Rules:

- constants exact;
- parameters from `@range`;
- world point from renderer world AABB;
- arithmetic via outward endpoint widening using f64 candidates and `next_down`/`next_up`;
- min/max/clamp exact interval rules;
- `abs` split around zero;
- square exact around zero;
- sqrt rejects negative lower domain after proving source operation validity;
- reciprocal rejects zero-containing interval;
- sin/cos use critical-point-aware range over reduced finite arguments, not always `[-1,1]` unless the span covers a full period;
- dot/cross/length/normalize use fused conservative rules;
- selected branch unions both values until branch stability is proven.

Every non-finite result is a build diagnostic naming the node and input ranges.

**Acceptance criteria**

- 100,000 deterministic random point checks per operation are permanent bug-finders.
- Analytic edge cases cover signed zero, subnormal, extrema, critical trigonometric points, reciprocal near zero, and normalization near zero.
- Range propagation is one topological pass.
- No interval intersection feeds back into predecessor values.
- Lean interval module proves the abstract operations used by runtime; compiler f64 implementation has differential containment tests.

**Gate**

```text
cargo xtask verify
cargo xtask pixels-formal
```

**Commit**

```text
pixels P3.2: propagate conservative scalar ranges
```

## Task P3.3 — propagate derivative and Lipschitz bounds

**Purpose**

Compute local/global bounds required by root isolation, continuation, displacement, filtering, and kinetic transport.

**Files**

```text
crates/wrela-compiler/src/pixels/derivative_bounds.rs
crates/wrela-compiler/src/pixels/bounds.rs
formal/pixels/Pixels/SmoothMin.lean
```

**Work**

For each scalar node compute conservative bounds with respect to:

- world x, y, z;
- screen u, v after projective compilation later;
- each used runtime parameter;
- presentation time/frame delta;
- second derivatives needed by Taylor remainder and material filtering.

At P3, world/parameter derivatives are primary. Use explicit chain rules. Fused rules include:

- `length(v)`: `|d length(v)| <= |dv|`, avoiding division at zero;
- rigid transform preserves gradient norm;
- uniform scale adjusts distance and derivative consistently;
- hard min/max bound is max child bound;
- smooth min/max bound is max child bound through convex gradient weights;
- bounded displacement adds its declared derivative/Hessian contract;
- repetition is handled only inside a fixed instance cell; a cross-wrap range creates a split obligation.

Store componentwise and Euclidean-norm bounds where each later consumer needs them; do not repeatedly derive one from the other.

**Acceptance criteria**

- Melee-like nested one-Lipschitz primitives derive `L <= 1` absent displacement/scale.
- The displacement fixture derives its explicit bound from declared frequencies/amplitudes, not global `4`.
- Every derivative bound has a source rule ID in the dump.
- Randomized gradient/Hessian samples never exceed the bound.
- Kink-containing domains are marked nonsmooth rather than assigned arbitrary derivatives.

**Gate**

```text
cargo xtask verify
cargo xtask pixels-formal
```

**Commit**

```text
pixels P3.3: derive renderer derivative contracts
```

## Task P3.4 — compute structural world bounds

**Purpose**

Bound every field subtree and primitive independently of a screen sample.

**Files**

```text
crates/wrela-compiler/src/pixels/world_bounds.rs
crates/wrela-compiler/src/pixels/graph.rs
crates/wrela-compiler/src/pixels/diagnostics.rs
```

**Work**

Compute `AabbExpr`/conservative AABB over the full parameter range for each geometric subtree.

Primitive rules:

- plane is unbounded and clipped to renderer world AABB;
- sphere/box/round-box/capsule/cylinder/cone/torus have analytic bounds;
- rigid transform transforms all AABB corners and unions;
- parameterized transforms use interval matrix/vector bounds;
- repeat enumerates the finite set of cells intersecting the world AABB;
- bounded displacement expands by amplitude;
- hard CSG union unions bounds;
- intersection intersects bounds and rejects empty subtrees;
- subtraction keeps left bound;
- smooth union/intersection expand according to support budget;
- negation alone does not define a finite object and must occur inside a supported hard-CSG shape.

Every bound stores the rule and exact expansion contributors.

**Acceptance criteria**

- `control-enclosed-feature` is discoverable solely from its primitive bound.
- Thin features retain nonempty conservative bounds even below one output pixel.
- Repeat instance enumeration is exact and finite.
- Empty intersections are pruned with a stable reason.
- Unbounded geometry outside explicit world clipping is rejected with `P012`/`P016` as appropriate.

**Gate**

```text
cargo xtask verify
```

**Commit**

```text
pixels P3.4: derive complete structural world bounds
```

## Task P3.5 — propagate smooth-CSG support budgets

**Purpose**

Prove that expanded leaf shells form a complete candidate source for smooth composites.

**Files**

```text
crates/wrela-compiler/src/pixels/support.rs
crates/wrela-compiler/src/pixels/graph.rs
formal/pixels/Pixels/SupportTree.lean
```

**Work**

For each field subtree compute:

```rust
pub struct SupportInfo {
    pub max_budget: F64Interval,
    pub leaf_budgets: Vec<(PrimitiveLeafId, F64Interval)>,
    pub gap_sensitive: Option<GapBudgetProgram>,
}
```

Rules:

- primitive leaf budget `0`;
- hard min/max/negation preserve/merge child budgets according to semantics;
- smooth min/max add `k/4` to the maximum descendant budget;
- when a child-gap lower bound is known inside a region, emit the exact active-band bulge program `(k-gap)^2/(4k)`;
- displacement amplitude is a separate shell expansion and is not double-counted as smooth support;
- coefficient-only selects union both arms and require equal leaf topology.

Balance only compiler-generated associative smooth trees if source bit semantics explicitly define reassociation. For v1, preserve authored tree and report its maximum support depth; do not silently reassociate.

**Acceptance criteria**

- Every smooth composite zero has at least one leaf shell in the formal model.
- Per-leaf shell expansion is finite.
- Gap-sensitive programs never exceed the coarse max budget.
- Unit tests cover nested, saturated, equal-child, and varying-k trees.
- Dump prints the support path producing each maximum.

**Gate**

```text
cargo xtask verify
cargo xtask pixels-formal
```

**Commit**

```text
pixels P3.5: prove smooth surface support shells
```

## Task P3.6 — partition maximal smooth objects and compile hard CSG

**Purpose**

Separate local smooth root problems from global Boolean occupancy.

**Files**

```text
crates/wrela-compiler/src/pixels/objects.rs
crates/wrela-compiler/src/pixels/csg.rs
crates/wrela-compiler/src/pixels/graph.rs
formal/pixels/Pixels/Csg.lean
```

**Work**

Partition at hard operations:

- a maximal subtree containing primitives, transforms, repeat-fixed instances, bounded displacement, and smooth min/max is one `SmoothObject`;
- hard union/intersection/subtraction/negation compile to an occupancy expression over object IDs;
- marks establish object/material identity within a smooth object; conflicting marks inside one smooth blend produce a finite blend identity set;
- repeated instances become distinct object instances sharing one feature template and coefficient program;
- empty/pruned objects are removed and CSG expression simplified exactly.

Compile the hard CSG expression to a bounded stack program:

```rust
enum CsgInst {
    Push(ObjectId),
    Not,
    And,
    Or,
}
```

Subtraction is `a AND NOT b` in occupancy semantics. Record per-object Boolean influence/cofactor programs for event pruning.

**Acceptance criteria**

- CSG stack depth is computed exactly and finite.
- Compiler tree evaluation and stack program agree exhaustively for up to 12 objects and deterministically sampled assignments beyond that.
- Hard union of marked objects preserves independent identity until the visible crossing is selected.
- Smooth blends remain within one object and are not represented as Boolean toggles.
- Object ordering is stable by canonical root structural key then source origin.

**Gate**

```text
cargo xtask verify
cargo xtask pixels-formal
```

**Commit**

```text
pixels P3.6: partition smooth objects and compile CSG occupancy
```

## Task P3.7 — decompose fused primitive features

**Purpose**

Replace expanded SDF kinks with exact geometric features and validity predicates.

**Files**

```text
crates/wrela-compiler/src/pixels/features.rs
crates/wrela-compiler/src/pixels/primitive.rs
crates/wrela-compiler/src/pixels/graph.rs
```

**Work**

Emit features:

- plane: one plane feature;
- sphere: one quadric feature;
- box: six planar faces;
- round box: six plane faces, twelve edge cylinders, eight corner spheres;
- capsule: cylinder side plus two spherical caps;
- finite cylinder: side plus two caps;
- cone: side plus cap where authored;
- torus: one quartic feature with angular/radial validity;
- repeated primitive: instance-transformed copies;
- bounded displacement: base feature plus deformation program and contract.

Each `FeatureRecord` contains:

```rust
pub struct FeatureRecord {
    pub id: FeatureId,
    pub object: ObjectId,
    pub kind: FeatureKind,
    pub world_bounds: Aabb64,
    pub support_expand: f64,
    pub validity: PredicateProgram,
    pub orientation: OrientationProgram,
    pub identity_set: IdentitySetId,
    pub scalar_semantic_root: FieldId,
}
```

Feature-validity predicates are explicit polynomial/analytic inequalities. Shared boundaries belong to both adjacent features; half-open runtime ownership resolves duplicate emission, not compiler exclusion.

**Acceptance criteria**

- Union of feature-validity domains covers each primitive boundary.
- No feature domain includes a geometrically different primitive branch.
- Rounded-box face interiors no longer carry artificial `abs/max/sqrt` derivative ambiguity.
- Feature count and bound expansion are exact in dumps.
- Primitive scalar reference remains available for semantic validation.

**Gate**

```text
cargo xtask verify
```

**Commit**

```text
pixels P3.7: compile fused surface features
```

## Task P3.8 — compile repetition and bounded deformation templates

**Purpose**

Make discontinuous coordinate wrapping and analytic deformation explicit rather than opaque range operations.

**Files**

```text
crates/wrela-compiler/src/pixels/repeat.rs
crates/wrela-compiler/src/pixels/deform.rs
crates/wrela-compiler/src/pixels/features.rs
```

**Work**

For repetition:

- enumerate all integer instance indices whose expanded bounds intersect the world AABB over all parameter ranges;
- instantiate affine translation coefficient programs;
- emit wrap-boundary event families only where a moving camera/parameter domain can cross an instance boundary;
- reject a count exceeding the sealed instance ceiling;
- never evaluate a certificate over a domain spanning two instance indices.

For bounded displacement:

- store amplitude, gradient, Hessian, and optional third-derivative bounds;
- retain base feature projective equation as predictor;
- compile displacement value/derivative programs;
- expand bounds exactly once by amplitude;
- classify supported sinusoidal/octave forms for tighter range programs;
- treat arbitrary bounded scalar helper as a Taylor/interval deformation, not an analytic primitive.

**Acceptance criteria**

- Repeat fixture contains no runtime modulo/floor inside a fixed instance certificate.
- Instance ordering and IDs are deterministic for negative indices.
- Displacement contracts are checked against source-declared frequencies/amplitudes where structurally known.
- An understated user contract is caught by deterministic differential compile tests and documented as a source contract violation; production trust relies on compiler derivation for built-in forms and explicit checked wrapper types for custom forms.
- Cross-wrap domains always create event/split obligations.

**Gate**

```text
cargo xtask verify
```

**Commit**

```text
pixels P3.8: compile finite repeats and bounded deformations
```

## Task P3.9 — compile material discontinuity obligations

**Purpose**

Ensure depth-smooth surfaces cannot hide output-discontinuous material changes.

**Files**

```text
crates/wrela-compiler/src/pixels/material.rs
crates/wrela-compiler/src/pixels/material_graph.rs
crates/wrela-compiler/src/pixels/objects.rs
```

**Work**

Classify material graph predicates:

- nominal material identity selection: already tied to geometric identity event;
- threshold on scalar surface/world/parameter expression: emit explicit material event predicate;
- smooth blend/select: compile value/derivative bound and no topological event;
- procedural wrap/step: emit finite period/threshold events when bounded; otherwise reject;
- texture lookup: v1 supports only immutable compiler-known textures with explicit filter and finite dimensions; discontinuities at texel/filter boundaries are represented in the shading error bound, not geometry identity.

Attach event obligations to the owning smooth object/feature set. Material events split shading runs but do not insert geometry roots.

**Acceptance criteria**

- `control-material-edge` is visible in the structural event set before rendering.
- A material threshold with unknown finite crossing count is `P014`.
- Smooth material expressions do not create unnecessary hard event records.
- Material event identity is stable and source-spanned.

**Gate**

```text
cargo xtask verify
```

**Commit**

```text
pixels P3.9: expose material discontinuities structurally
```

## Task P3.10 — derive exact structural capacities

**Purpose**

Reserve all later frame-program and runtime storage without runtime allocation.

**Files**

```text
crates/wrela-compiler/src/pixels/capacities.rs
crates/wrela-compiler/src/pixels/report.rs
formal/pixels/Pixels/Capacity.lean
bench/thresholds.toml
```

**Work**

Derive and checked-add/multiply:

- object count;
- feature count;
- repeated instance count;
- scalar/derivative program slots;
- maximum projected features overlapping a row and tile, conservatively from world/camera bounds;
- maximum object roots per row start;
- maximum active sheet records per row;
- maximum primitive/feature/material event generators;
- maximum event subdivisions from fixed degree and dyadic isolation depth;
- maximum run records per tile row;
- maximum CSG events per row;
- maximum transparent layers per pixel/run;
- maximum local rebuild queue;
- per-worker scratch bytes;
- output tile and double-buffer bytes;
- probe bytes;
- kinetic certificate bytes.

Where an exact geometric overlap count is expensive, use a proven conservative endpoint sweep over projected bounding intervals. Do not substitute a hand-authored cap without reporting how it was derived.

Define machine-v1 ceilings in one `PixelsCeilings` struct and mirror them in the spec. A build above a ceiling fails with `P015` and why-chain.

**Acceptance criteria**

- Every runtime vector/array bound in later Wrela modules traces to one capacity field.
- Arithmetic overflow is a build error before comparison to ceilings.
- Capacity values appear in field-graph dump and report.
- A deliberately oversized fixture fails at compile time with exact contributors.
- Formal capacity lemmas build.

**Gate**

```text
cargo xtask verify
cargo xtask pixels-formal
```

**Commit**

```text
pixels P3.10: seal structural renderer capacities
```

## Task P3.11 — add the structural program verifier

**Purpose**

Refuse incomplete or internally inconsistent scene structures before projective/runtime lowering.

**Files**

```text
crates/wrela-compiler/src/pixels/verify.rs
crates/wrela-compiler/src/pixels/mod.rs
```

**Work**

Validate:

- every field root reachable and acyclic;
- every smooth object has complete leaf support information;
- every primitive leaf belongs to exactly one feature template family;
- every feature has world bounds, validity, orientation, semantic root, and identity set;
- every hard CSG object bit refers to a live object;
- every material discontinuity has an event obligation;
- every repeat/deformation contract is finite;
- every parameter use has range/rate;
- every topology-select arm has identical canonical structure;
- all capacities dominate exact table sizes;
- no unsupported source node remains.

Return `VerifiedStructuralProgram` required by P4.

**Acceptance criteria**

- Corruption unit tests remove each required record and receive a specific internal invariant error.
- Verification order is deterministic and reports the lowest stable offending ID.
- No downstream P4 function accepts an unverified mutable compiler context.
- Valid permanent fixtures pass.

**Gate**

```text
cargo xtask verify
```

**Commit**

```text
pixels P3.11: verify complete structural renderer programs
```

## Task P3.12 — extend graph dumps with structural proofs

**Purpose**

Pin all P3 facts before projective lowering.

**Files**

```text
crates/wrela-compiler/src/pixels/dump.rs
tests/golden/check-pixels-*/expected/field-graph.txt
```

**Work**

Add:

- parameter dependency/use table;
- scalar value/derivative bounds;
- subtree world bounds;
- support budgets and paths;
- object partition;
- CSG stack/influence;
- features and validity summary;
- repeat instances;
- deformation contracts;
- material event obligations;
- capacities.

Use compact stable expressions. Large coefficient arrays print count, degree, and a deterministic digest plus a separately numbered coefficient block, not elided `...` text.

**Acceptance criteria**

- Every structural proof object has a dump representation.
- Dump has no `pending` line after P3.
- Permanent fixtures pin the intended object/feature/event-obligation counts.
- Report determinism remains green.

**Gate**

```text
cargo xtask verify
cargo xtask report-determinism
```

**Commit**

```text
pixels P3.12: pin structural proof dumps
```

### Milestone P3 close

Run `cargo xtask verify-milestone`. Do not start projective lowering until all accepted fixtures have a verified structural program and all runtime storage classes have finite capacities.

---

# Milestone P4 — projective surface programs and complete local event generators

Milestone result: the verified structural scene compiles into inverse-view-depth feature equations, derivative/Taylor programs, conservative projected spans, and a complete finite set of local runtime event generators plus explicit exclusion records. No global aspect graph or whole-scene resultant is emitted.

## Task P4.1 — define the camera/projective coefficient model

**Purpose**

Use one camera algebra throughout compiler, runtime, reference, and Lean.

**Files**

```text
crates/wrela-compiler/src/pixels/projective.rs
crates/wrela-compiler/src/pixels/camera.rs
stdlib/core/render.wr
formal/pixels/Pixels/Projective.lean
```

**Work**

Define screen coordinates at pixel centers:

```text
u = ((x + 0.5) / width  * 2 - 1) * aspect * tan(fov_y/2)
v = (1 - (y + 0.5) / height * 2) * tan(fov_y/2)
raw_ray(u,v) = forward + u*right + v*up
P(u,v,q) = eye + raw_ray(u,v) / q
q = 1 / view_z
```

Camera basis requirements:

- `right`, `up`, `forward` are an orthonormal, right-handed basis within a compiler/runtime error contract;
- camera input may be authored as eye/target/up, quaternion, or explicit basis, but generated frame coefficients are always the canonical basis above;
- compiler derives basis value/rate bounds from parameter contracts;
- runtime candidate basis is f32/f64; verifier basis is dyadic intervals enclosing it;
- near/far convert to positive q range `[1/far, 1/near]`.

Do not normalize rays in per-feature or per-pixel projective evaluation.

**Acceptance criteria**

- Plane inverse depth is affine in `(u,v)` in compiler and formal model.
- Projective cancellation theorem builds.
- Screen-coordinate convention has permanent corner/center tests.
- Camera handedness and y direction match framebuffer goldens.
- Zero/degenerate up vectors are rejected at source/build boundary.

**Gate**

```text
cargo xtask verify
cargo xtask pixels-formal
```

**Commit**

```text
pixels P4.1: fix the inverse-view-depth camera model
```

## Task P4.2 — implement bounded polynomial and rational programs

**Purpose**

Represent only the low-degree projective equations and predicates the runtime needs.

**Files**

```text
crates/wrela-compiler/src/pixels/polynomial.rs
crates/wrela-compiler/src/pixels/program.rs
crates/wrela-compiler/src/pixels/reference/polynomial.rs
formal/pixels/Pixels/Bernstein.lean
```

**Work**

Define sparse monomials with fixed variable IDs:

```rust
pub enum Var {
    U,
    V,
    Q,
    X,      // local scanline coordinate where emitted
    T,      // normalized frame delta where emitted
    Param(ParamId),
}

pub struct Exponents {
    pub u: u8,
    pub v: u8,
    pub q: u8,
    pub t: u8,
    pub param_terms: SmallFixedParamExponents,
}

pub struct PolyProgram {
    pub terms: Vec<PolyTerm>,
    pub degree_u: u8,
    pub degree_v: u8,
    pub degree_q: u8,
    pub degree_t: u8,
    pub coefficient_program: CoeffProgramId,
}
```

Do not add a third-party small-vector crate. Use fixed arrays plus explicit count.

Canonical term order is lexicographic `(q degree desc, u degree desc, v degree desc, t degree desc, parameter path)`. Combine exact equal monomials. Remove exact zero coefficients only after bit-exact folding.

A rational program is `numerator/denominator` plus a verifier proof obligation that denominator has one strict sign over its use domain.

Hard limits for v1:

- feature q degree ≤ 4;
- event bivariate degree per variable ≤ 6 after local construction;
- runtime univariate event/root degree ≤ 8;
- terms per program ≤ a sealed ceiling derived and reported;
- parameter coefficient programs may be arbitrary finite scalar DAGs already accepted by P3, but local Taylor expansion order is bounded.

**Acceptance criteria**

- Polynomial arithmetic is deterministic and checked for degree/term overflow.
- Different construction orders canonicalize identically.
- Horner and direct-sum reference evaluation agree over permanent fixtures.
- Bernstein conversion supports the fixed degree set and passes formal coefficient enclosure tests.
- Exceeding a degree/term ceiling is `P004`/`P015`, not truncation.

**Gate**

```text
cargo xtask verify
cargo xtask pixels-formal
```

**Commit**

```text
pixels P4.2: add bounded projective polynomial programs
```

## Task P4.3 — compile projective equations for planar and quadric features

**Purpose**

Eliminate routine field marching for the most common feature classes.

**Files**

```text
crates/wrela-compiler/src/pixels/projective.rs
crates/wrela-compiler/src/pixels/primitive.rs
formal/pixels/Pixels/Primitive.lean
```

**Work**

Compile exact homogeneous q equations for:

- planes and box/cylinder caps;
- spheres and rounded-box corners;
- infinite cylinder sides and capsule sides;
- cones;
- general affine quadrics if source primitive maps exactly.

For each feature emit:

```rust
pub struct ProjectiveFeature {
    pub feature: FeatureId,
    pub root_equation: PolyProgramId,
    pub q_degree: u8,
    pub validity_predicates: Vec<PredicateProgramId>,
    pub orientation_program: ScalarProgramId,
    pub q_seed_kind: SeedKind,
}
```

`SeedKind` includes affine, quadratic formula with stable branch selection, and generic isolated root. Analytic solution is a candidate only; all accepted roots still pass the runtime feature/root verifier.

Stable quadratic formula:

```text
disc = b*b - 4*a*c
q0 = (-b - sign(b)*sqrt(disc)) / (2*a)
q1 = c / (a*q0)
```

with explicit linear fallback when `a` interval contains zero. Candidate f32/f64 evaluation must never decide validity from an unchecked negative discriminant caused by rounding; the verifier owns the sign.

**Acceptance criteria**

- Plane, sphere, box-face, round-box corner, capsule-side, cylinder, cone fixtures compile.
- Original primitive scalar zero and projective zero agree in deterministic f64 differential tests.
- Feature validity rejects cap/side/corner roots outside their domains.
- Orientation matches field outside→inside sign convention.
- Formal equivalence theorems build for each feature class.

**Gate**

```text
cargo xtask verify
cargo xtask pixels-formal
```

**Commit**

```text
pixels P4.3: compile planar and quadric q equations
```

## Task P4.4 — compile torus and bounded-deformation equations

**Purpose**

Cover the nonquadric flagship features without opaque scene-wide ranges.

**Files**

```text
crates/wrela-compiler/src/pixels/projective.rs
crates/wrela-compiler/src/pixels/deform.rs
formal/pixels/Pixels/Primitive.lean
```

**Work**

Torus:

- emit the exact quartic homogeneous q polynomial;
- compile validity/orientation;
- use deterministic Sturm-sequence or Bernstein subdivision for root count/isolation, not the closed quartic formula;
- preserve all positive q roots inside near/far.

Bounded deformation:

- keep the base feature polynomial as predictor;
- compile residual `D(u,v,q)` plus value/first/second/third derivative bound programs;
- build a local sparse Taylor model around a base root;
- certify roots through monotone tube/Krawczyk;
- do not pretend the deformed surface is still algebraic if its source is transcendental.

Sinusoidal/octave built-ins compile exact phase recurrence coefficient programs plus explicit minimax polynomial/remainder for local sine/cosine verifier evaluation. The approximation table and remainder constants are versioned numeric-contract data.

**Acceptance criteria**

- Torus retains multiple ordered roots where present; it never returns only the nearest candidate before CSG/occupancy processing.
- Deformed-plane fixture constructs a root tube around the exact plane predictor.
- Approximation remainder is included in every residual/derivative interval.
- Unsupported custom deformation without sufficient derivative contract fails at build time.
- Formal torus equivalence and Taylor-with-remainder generic theorems build.

**Gate**

```text
cargo xtask verify
cargo xtask pixels-formal
```

**Commit**

```text
pixels P4.4: compile quartic and deformed surface programs
```

## Task P4.5 — compile derivative and Taylor coefficient programs

**Purpose**

Generate all runtime continuation data once from structural/projective expressions.

**Files**

```text
crates/wrela-compiler/src/pixels/derivatives.rs
crates/wrela-compiler/src/pixels/polynomial.rs
crates/wrela-compiler/src/pixels/program.rs
```

**Work**

For each projective feature/root equation `G(u,v,q,params)` compile programs for:

```text
G
G_u, G_v, G_q
G_uu, G_uv, G_uq, G_vv, G_vq, G_qq
selected third derivatives required by the run remainder
G_param_i for influencing parameters
G_t and optional G_tt assembled from parameter rates
```

Use symbolic differentiation of bounded polynomial/scalar programs. Apply CSE across derivative outputs. Store one derivative bundle ID per feature template.

For smooth object equations, derivative evaluation follows the structurally active primitive/blend cluster. Compile cluster programs keyed by a sorted leaf-signature template; do not emit every possible subset. Capacity derives the maximum allowed template count. Unsupported dynamic cluster explosion is a build error.

**Acceptance criteria**

- Analytic derivatives agree with finite differences only as a bug-finder; exact symbolic differentiation is the implementation.
- Mixed partials canonicalize consistently.
- Nonsmooth feature/branch boundaries are excluded by event predicates, not assigned derivatives.
- Derivative bundles share coefficient subprograms.
- Dumps name derivative degree/term counts and influencing parameter set.

**Gate**

```text
cargo xtask verify
```

**Commit**

```text
pixels P4.5: emit projective derivative bundles
```

## Task P4.6 — derive conservative projected feature spans

**Purpose**

Drive row/tile candidate discovery from structural bounds with no sampling.

**Files**

```text
crates/wrela-compiler/src/pixels/projection_bounds.rs
crates/wrela-compiler/src/pixels/camera.rs
crates/wrela-compiler/src/pixels/capacities.rs
```

**Work**

Project each expanded world AABB through the complete camera parameter bounds. Emit conservative normalized-screen and integer pixel/tile bounds.

Rules:

- clip against near plane conservatively;
- if a box can cross/contain the eye or near plane, expand to the full affected screen domain rather than divide by a zero-containing z interval;
- use interval projective division with denominator sign proof;
- clamp only after outward expansion;
- convert normalized bounds to half-open pixel ranges using outward floor/ceil;
- add one pixel coverage halo for event curves and filter footprint halo from profile;
- preserve row interval endpoints for overlap-capacity sweep.

For large unbounded plane features, derive screen span from intersection with renderer world AABB and frustum; full-screen is valid when tighter clipping is unavailable.

**Acceptance criteria**

- Every oracle-visible permanent-fixture feature lies inside its projected bound at all deterministic sampled parameter corners.
- Enclosed/thin features produce nonempty spans.
- Pixel-range conversion has exact boundary tests at integer/subpixel edges.
- No finite feature is dropped because its projected area rounds below one pixel.
- Overlap capacity recomputes from these spans and remains within P3 ceiling or fails build.

**Gate**

```text
cargo xtask verify
```

**Commit**

```text
pixels P4.6: derive complete projected feature spans
```

## Task P4.7 — compile primitive and feature event generators

**Purpose**

Represent every local change in root existence or feature validity.

**Files**

```text
crates/wrela-compiler/src/pixels/events.rs
crates/wrela-compiler/src/pixels/event_kinds.rs
formal/pixels/Pixels/EventCover.lean
```

**Work**

Emit event families per projected feature:

```rust
enum EventKind {
    Silhouette,          // G = 0 and G_q = 0 / discriminant zero
    FeatureBoundary,     // validity predicate zero
    RepeatBoundary,
    SmoothBandEnter,     // |a-b| - k = 0
    SmoothCenterTie,     // a-b = 0 where identity/derivative ownership changes
    MaterialBoundary,
    NearClip,
    FarClip,
    FixedPointResetOnly, // not topology, but run endpoint class
}
```

Representation choice is fixed by feature class:

- plane: none for silhouette unless parallel/grazing domain requires one;
- quadric: discriminant/conic event;
- feature validity: direct low-degree predicate;
- torus/deformation: local numeric event oracle with value/derivative/Taylor bounds;
- repeat: affine wrap lines/curves;
- smooth band/tie: structural child field predicate evaluated only within support overlap;
- material: predicate compiled in P3.9.

Every generator includes projected domain, coefficient dependencies, maximum root count or subdivision depth, and event-side meaning.

**Acceptance criteria**

- Each feature kind has a complete event-family constructor.
- Generators outside projected spans are not emitted.
- Event-side labels are deterministic and sufficient to update active feature/identity state.
- Numeric generators carry derivative/remainder programs; no black-box boolean sample.
- Formal conditional event-cover theorem builds.

**Gate**

```text
cargo xtask verify
cargo xtask pixels-formal
```

**Commit**

```text
pixels P4.7: compile local feature event generators
```

## Task P4.8 — compile q-order competition pairs and swap events

**Purpose**

Monitor only feature sheets that can actually compete for visibility.

**Files**

```text
crates/wrela-compiler/src/pixels/competition.rs
crates/wrela-compiler/src/pixels/events.rs
crates/wrela-compiler/src/pixels/csg.rs
```

**Work**

Build candidate competition pairs after pruning by:

1. projected tile/row span overlap;
2. conservative q-range overlap;
3. object CSG influence under reachable occupancy states;
4. opaque/transparency class compatibility;
5. material-only events not competing for geometry;
6. same-feature duplicate suppression.

For each surviving pair emit a local `DepthSwap` generator for `q_a - q_b = 0` represented as:

- direct polynomial/rational cross-product when both sheets have explicit q forms and denominators have fixed sign;
- local Taylor difference plus remainder otherwise.

Do not compute a whole-scene resultant eliminating q from arbitrary feature equations. At runtime, both sheets are already isolated; the event predicate compares their certified q functions.

**Acceptance criteria**

- Every omitted pair has one stable exclusion record and positive margin/domain proof.
- `control-close-depth` pair survives and receives a swap/ambiguity event.
- Nonoverlapping colonnade objects are excluded before event emission.
- CSG-noninfluential interior boundaries are excluded with Boolean cofactor proof.
- Pair count and each pruning reason are dumped/reported.

**Gate**

```text
cargo xtask verify
cargo xtask pixels-formal
```

**Commit**

```text
pixels P4.8: compile sparse q-order competitions
```

## Task P4.9 — compile omission/exclusion proof records

**Purpose**

Make local event and candidate completeness auditable rather than implicit in compiler control flow.

**Files**

```text
crates/wrela-compiler/src/pixels/exclusions.rs
crates/wrela-compiler/src/pixels/events.rs
crates/wrela-compiler/src/pixels/verify.rs
```

**Work**

Every potential candidate/event/pair considered by the structural enumerator ends in exactly one of:

- emitted runtime record ID;
- compile-time exclusion record.

```rust
enum ExclusionReason {
    WorldBoundsDisjoint,
    ProjectedBoundsDisjoint,
    QRangesDisjoint,
    OutsideNearFar,
    CsgNonInfluential,
    FeatureValidityImpossible,
    SupportShellDisjoint,
    MaterialClassIrrelevant,
    StaticStrictOrder,
    DuplicateCanonicalFeature,
}

pub struct ExclusionRecord {
    pub subject: ExclusionSubject,
    pub domain: DomainId,
    pub reason: ExclusionReason,
    pub margin: F64Interval,
    pub dependencies: Vec<ProofRecordId>,
}
```

A zero or sign-indefinite margin cannot justify exclusion. Static strict-order exclusions become runtime invariants only if all coefficient dependencies are zero-rate; otherwise emit a kinetic/event predicate.

**Acceptance criteria**

- Verifier accounts for every enumerated subject exactly once.
- Removing an exclusion/emitted record triggers internal verification failure.
- Exclusion dependencies are acyclic and point to earlier pass facts.
- Report can explain any omitted competition from source feature names to final margin.
- No “default pruned” reason exists.

**Gate**

```text
cargo xtask verify
```

**Commit**

```text
pixels P4.9: record complete event exclusions
```

## Task P4.10 — compile row/tile event indexes

**Purpose**

Let runtime retrieve relevant features/events in O(records for tile/row), not O(scene).

**Files**

```text
crates/wrela-compiler/src/pixels/index.rs
crates/wrela-compiler/src/pixels/program.rs
crates/wrela-compiler/src/pixels/capacities.rs
```

**Work**

Build immutable compressed indexes:

- tile → feature IDs;
- tile → event-generator IDs;
- tile → competition-pair IDs;
- row-block → repeat instances;
- object → feature range;
- feature → derivative bundle;
- material identity → material program;
- light/probe influence indexes.

Use offset/count arrays into sorted ID tables. Do not use pointer-rich trees or runtime hash maps.

Index construction may duplicate small IDs between adjacent tiles; report bytes. Every table is sorted ascending by ID inside each cell. Duplicate IDs within one cell are forbidden.

**Acceptance criteria**

- Runtime lookup is two bounds-checked loads plus a contiguous slice.
- Every feature/event appears in every tile its conservative span touches.
- No record appears outside its span unless required halo is documented.
- Index size fits capacity and image ceiling.
- Unit tests compare indexed retrieval to a slow full-table overlap filter.

**Gate**

```text
cargo xtask verify
```

**Commit**

```text
pixels P4.10: build immutable local renderer indexes
```

## Task P4.11 — verify projective/event completeness

**Purpose**

Close the compiler proof boundary before binary emission.

**Files**

```text
crates/wrela-compiler/src/pixels/verify.rs
crates/wrela-compiler/src/pixels/projective.rs
crates/wrela-compiler/src/pixels/events.rs
```

**Work**

Extend verifier to prove/check:

- every feature has projective or deformed root representation;
- every projective denominator has a strict sign obligation;
- every validity predicate boundary has an event family or static exclusion;
- every silhouette/tangency family exists;
- every repeat boundary exists;
- every smooth band/tie affecting active structure exists;
- every material discontinuity exists;
- every surviving competition pair has depth-swap/ambiguity tracking;
- every omitted candidate/pair has an exclusion record;
- event degree/root/subdivision bounds fit capacities;
- local indexes cover every emitted record;
- no global resultant/table is required or present.

Return `VerifiedProjectiveProgram` required by P5.

**Acceptance criteria**

- Corruption tests for every missing event family fail.
- `control-enclosed-feature` completeness is independent of event samples.
- `control-close-depth` cannot be statically ordered.
- Static plane-only scene has no unnecessary silhouette generator.
- All permanent fixtures verify.

**Gate**

```text
cargo xtask verify
cargo xtask pixels-formal
```

**Commit**

```text
pixels P4.11: verify complete projective event programs
```

## Task P4.12 — pin projective/event dumps

**Purpose**

Make the final compiler math before serialization visible and stable.

**Files**

```text
crates/wrela-compiler/src/pixels/dump.rs
tests/golden/check-pixels-*/expected/field-graph.txt
```

**Work**

Add projective feature equations, derivative bundle summaries, projected spans, event generators, competition pairs, exclusions, and local indexes to the graph dump.

Print polynomial terms in canonical order with exact coefficient source IDs. Print interval margins as explicit endpoints. Print every exclusion subject once.

**Acceptance criteria**

- There is enough dump information to reconstruct why a feature/event is present or absent.
- Counts in dump equal capacity/report counts.
- No pointer/address or host formatting appears.
- Permanent fixture event counts are pinned.
- Report determinism passes.

**Gate**

```text
cargo xtask verify
cargo xtask report-determinism
```

**Commit**

```text
pixels P4.12: pin projective and event compiler dumps
```

### Milestone P4 close

Run `cargo xtask verify-milestone`. The milestone closes only when every accepted scene has a verified finite local event program and every omitted interaction has an explicit exclusion proof.

---

# Milestone P5 — `FrameProgram v1`, image placement, generated renderer actors, and reports

Milestone result: the compiler emits a verified binary frame program, reserves all mutable renderer memory, synthesizes typed renderer actors/glue, places everything in the sealed image, and reports the complete renderer contract. The runtime still returns `UnsupportedFrameState` before rendering.

## Task P5.1 — define `FrameProgram v1` Rust structs

**Purpose**

Freeze the in-memory semantic format before byte encoding.

**Files**

```text
crates/wrela-compiler/src/pixels/program.rs
crates/wrela-compiler/src/pixels/version.rs
crates/wrela-machine/src/pixels.rs
```

**Work**

Define fixed-width records using only integer IDs, offsets, counts, enum tags, and bit-preserved scalar constants. Separate compiler-rich structs from wire structs:

```rust
pub struct FrameProgram { /* rich verified model */ }

#[repr(C)]
pub struct FrameProgramHeaderV1 {
    pub magic: [u8; 8],          // b"WRELAPX\0"
    pub version: u16,            // 1
    pub header_bytes: u16,
    pub flags: u32,
    pub total_bytes: u32,
    pub renderer_index: u16,
    pub reserved0: u16,
    pub numeric_revision: u32,
    pub formal_revision: u32,
    pub table_count: u16,
    pub reserved1: [u8; 14],
    pub digest: [u8; 32],
}
```

The digest field is zero while hashing and then filled with SHA-256 of the complete encoded bytes with that field zeroed. Document this exactly.

Wire record rules:

- little-endian;
- no Rust enum layout on disk;
- no `usize`;
- no raw bool; use `u8` 0/1 and verify;
- no implicit struct padding; encoder writes fields explicitly;
- table offsets from frame-program base;
- all tables 16-byte aligned, hot SIMD tables 64-byte aligned;
- counts are element counts, not bytes;
- reserved bytes zero;
- strings absent except a compact optional debug-name table excluded from runtime-required flags.

**Acceptance criteria**

- `wrela-machine` contains only shared format/constants, no compiler analysis.
- Wire structs have explicit size assertions.
- Every rich record has a wire counterpart or is intentionally compiler-only and documented.
- Version/revision constants appear in one location.
- Header magic/version corruption tests exist.

**Gate**

```text
cargo xtask verify
```

**Commit**

```text
pixels P5.1: define FrameProgram v1 records
```

## Task P5.2 — implement deterministic encoder

**Purpose**

Serialize a verified projective program without depending on Rust layout.

**Files**

```text
crates/wrela-compiler/src/pixels/encode.rs
crates/wrela-compiler/src/pixels/program.rs
crates/wrela-machine/src/pixels.rs
```

**Work**

Implement a local `Writer`:

```rust
struct Writer {
    bytes: Vec<u8>,
}

impl Writer {
    fn align(&mut self, n: usize) -> Result<(), PixelsError>;
    fn u8(&mut self, v: u8);
    fn u16(&mut self, v: u16);
    fn u32(&mut self, v: u32);
    fn i32(&mut self, v: i32);
    fn u64(&mut self, v: u64);
    fn f32_bits(&mut self, bits: u32);
    fn bytes(&mut self, b: &[u8]);
}
```

Encoding is two-pass:

1. encode each table in fixed order, recording offsets/counts;
2. write header/table directory and reassemble final bytes;
3. zero digest field, hash, fill digest;
4. run byte-level verifier before returning.

Table order is fixed by §4.14. Never derive order from enum declaration iteration.

**Acceptance criteria**

- Encoding same program twice yields identical bytes.
- All checked conversions fail with internal/size error, never truncate.
- Every alignment padding byte is zero.
- Digest algorithm has unit vectors.
- Golden frame-program binary is checked as SHA-256 plus a hex header/table summary; do not check large raw binaries into textual expected files unless harness already supports fixture bytes.

**Gate**

```text
cargo xtask verify
```

**Commit**

```text
pixels P5.2: encode verified frame programs
```

## Task P5.3 — implement hostile binary decoder and verifier

**Purpose**

Prove the binary format is independently checkable and fuzzable.

**Files**

```text
crates/wrela-compiler/src/pixels/decode.rs
crates/wrela-compiler/src/pixels/binary_verify.rs
crates/xtask/src/fuzz.rs
```

**Work**

Implement a bounds-checked cursor with no unsafe code. Validate header/digest/table directory before allocating table vectors. Limit total bytes to machine ceiling.

Decode all records into rich structs and rerun semantic `verify::check_program` after wire checks. Add a Pixels target to existing fuzz infrastructure that mutates valid encoded frame programs and arbitrary bytes.

Fuzz outcomes are only:

- decoded/verified program;
- structured `DecodeError`;
- never panic, OOM, hang, or out-of-bounds.

**Acceptance criteria**

- `decode(encode(p)) == p` for all permanent fixtures.
- Single-bit mutation corpus covers every header/table/enum/reserved field.
- Truncated bytes at every offset return error.
- Overlapping table/overflow attacks return error before allocation.
- Fuzz smoke is in `verify`; broad run remains the repository fuzz lane.

**Gate**

```text
cargo xtask verify
cargo xtask fuzz pixels --iters 10000 --seed 1
```

**Commit**

```text
pixels P5.3: verify and fuzz FrameProgram bytes
```

## Task P5.4 — compile renderer programs during image build

**Purpose**

Insert Pixels compilation at the single correct point in the existing build pipeline.

**Files**

```text
crates/wrela-compiler/src/bin/wrela.rs
crates/wrela-compiler/src/lib.rs
crates/wrela-compiler/src/pixels/mod.rs
crates/wrela-compiler/src/eval/image_checks.rs
```

**Work**

After all modules are semantically checked and the image graph is evaluated/sealed, but before runtime placement/codegen finalization:

1. parse/validate each renderer config;
2. call `pixels::compile` in renderer index order;
3. store `Vec<CompiledRenderer>` in the build context;
4. make dump stages consume these values;
5. pass generated actor/glue requirements into closure/root synthesis;
6. pass encoded programs/state layouts into layout/report.

Do not call Pixels for images with zero renderers. A failure aborts the build before ordinary guest codegen so no partial image artifact is written.

**Acceptance criteria**

- `--stage=field-graph` now runs the complete P4 compiler.
- `--stage=frame-program` decodes its own encoded bytes before dumping.
- Multiple renderers compile independently with stable indexes.
- Ordinary nonrenderer builds are byte-identical to pre-task baseline.
- Compiler timing report, if any, names Pixels separately but is not part of checked goldens.

**Gate**

```text
cargo xtask verify
cargo xtask repro
```

**Commit**

```text
pixels P5.4: compile frame programs in sealed image builds
```

## Task P5.5 — reserve `frameprog` and `pixelsdata` image sections

**Purpose**

Place immutable and mutable renderer data without disturbing existing rtdata invariants.

**Files**

```text
crates/wrela-machine/src/layout.rs
crates/wrela-compiler/src/layout.rs
crates/wrela-compiler/src/layout/place.rs
crates/wrela-compiler/src/layout/report_lines.rs
```

**Work**

Implement §6.10 packing. Keep `RTDATA_BASE` and `RTDATA_SIZE_MAX` unchanged. Compute:

```text
rtdata_end = RTDATA_BASE + runtime.total_bytes
frameprog_base = align_up(rtdata_end, 64 KiB)
frameprog_end = packed immutable programs
pixels_state_base = align_up(frameprog_end, 64 KiB)
pixels_state_end = packed mutable renderer state/framebuffers/probes
```

Add sections named exactly:

```text
frameprog
pixelsdata
```

If multiple renderer programs are noncontiguous because of alignment, section covers padding and each placement reports exact subrange.

`pixelsdata` is zero-initialized reservation, not stored zero bytes in the image blob. Record reservation separately from blob length where layout already supports BSS-like regions; otherwise implement the minimal explicit reservation mechanism rather than materializing hundreds of MiB in the image file.

**Acceptance criteria**

- Existing code/rodata/rtdata addresses are unchanged for nonrenderer images.
- Renderer section addresses are deterministic.
- All checked ranges fit the machine profile and do not overlap stacks/devices/framebuffer windows.
- Image report lists section/reservation bytes separately.
- Boundary tests cover exact max, one byte over, checked-add overflow, and alignment padding.

**Gate**

```text
cargo xtask verify
cargo xtask verify-milestone
```

**Commit**

```text
pixels P5.5: place frame programs and renderer state
```

## Task P5.6 — append frame-program bytes to the image

**Purpose**

Make immutable renderer data available at guest addresses.

**Files**

```text
crates/wrela-compiler/src/layout.rs
crates/wrela-compiler/src/layout/harness.rs
crates/wrela-compiler/src/pixels/encode.rs
```

**Work**

At layout:

- pad blob to each renderer frame-program placement;
- append exact encoded bytes;
- assert final cursor equals computed frameprog end;
- leave mutable pixelsdata as zero reservation;
- include renderer bytes in image digest/report;
- expose `RendererPlacement` in `ImageLayout`.

Add a layout test that reads bytes back from `ImageLayout.blob`, decodes, verifies digest, and compares to compiler rich program.

**Acceptance criteria**

- Blob contains exact frame-program bytes at reported address.
- No host path or pointer is encoded.
- Corrupting one image byte fails the decoder/digest test.
- Multiple renderer programs have correct independent bases/digests.
- Nonrenderer image bytes remain unchanged.

**Gate**

```text
cargo xtask verify
cargo xtask repro
```

**Commit**

```text
pixels P5.6: seal frame-program bytes into images
```

## Task P5.7 — generate renderer configuration module

**Purpose**

Expose table addresses/capacities to Wrela runtime code without runtime decoding/allocation.

**Files**

```text
crates/wrela-compiler/src/pixels/glue.rs
crates/wrela-compiler/src/loader.rs
crates/wrela-compiler/src/rtconfig.rs
stdlib/core/render_program.wr
```

**Work**

Generate module address `core.__image_pixels` with constants and placed static views:

```wrela
module __image_pixels

const N_RENDERERS: usize = 1
const R0_FRAMEPROG_BASE: usize = 0x...
const R0_FRAMEPROG_BYTES: usize = ...
const R0_STATE_BASE: usize = 0x...
const R0_STATE_BYTES: usize = ...
const R0_WIDTH: usize = 1920
const R0_HEIGHT: usize = 1080
const R0_TILE_W: usize = 64
const R0_TILE_H: usize = 32
const R0_MAX_FEATURES_TILE: usize = ...
# all exact capacities...
```

Also generate `@layout(runtime)` view structs for header/table records and `@placed` static roots at frame-program bases. Array extents are exact generated constants.

Generated source must parse/type-check through the ordinary compiler, like `core.__image_runtime`. It contains no field equations as executable source; it exposes table layouts only.

**Acceptance criteria**

- `--stage=rtconfig` either gains a clearly separated `PixelsConfig` block or add `--stage=pixelsconfig`; choose `pixelsconfig` to avoid changing existing rtconfig goldens unnecessarily.
- Generated module has a stable dump.
- Every runtime capacity/address comes from compiler placement, not duplicated arithmetic in Wrela.
- Stubs support zero-renderer images.
- Pool ceilings fail before generating invalid array extents.

**Gate**

```text
cargo xtask verify
cargo xtask report-determinism
```

**Commit**

```text
pixels P5.7: generate renderer table and state views
```

## Task P5.8 — synthesize renderer coordinator/worker actors

**Purpose**

Make renderer execution ordinary Wrela actors with closed capacity and placement.

**Files**

```text
crates/wrela-compiler/src/pixels/glue.rs
crates/wrela-compiler/src/eval/image.rs
crates/wrela-compiler/src/eval/image_checks.rs
crates/wrela-compiler/src/placement.rs
crates/wrela-compiler/src/layout/rtdata.rs
crates/wrela-compiler/src/lower.rs
stdlib/core/render_actor.wr
```

**Work**

Synthesize one coordinator and one worker instance per assigned render core. Prefer instantiating standard generic structs from `render_actor.wr` through generated image declarations rather than manufacturing typed AST by hand.

Generated graph edges:

- application actor → coordinator handle;
- coordinator → each worker handle;
- coordinator → display driver handle;
- workers have no display handle and no cross-worker handles.

Mailbox capacities:

- coordinator: one render request plus one completion per worker plus one display completion;
- worker: one job;
- derive exact existing mailbox/ring capacity facts.

Placement:

- coordinator defaults to core 0 unless renderer placement solver finds a lower published load and spec permits;
- exactly one worker on each render core;
- workers own disjoint workspace/framebuffer tile ranges;
- generated actor bytes/work participate in existing placement report.

Until sweep exists, `render` returns `Err(UnsupportedFrameState)` after validating frame input and without touching display.

**Acceptance criteria**

- Renderer handle has real actor identity and ordinary admission semantics.
- Generated actors appear in typed/FlowWir/MachineWir/placement/report dumps.
- Cross-core rings are generated by existing machinery.
- No custom scheduler/work stealing is added.
- Boot fixture can create coordinator/workers and call render, receiving the expected error deterministically.

**Gate**

```text
cargo xtask verify
cargo xtask verify-milestone
```

**Commit**

```text
pixels P5.8: synthesize bounded renderer actors
```

## Task P5.9 — root generated renderer functions and constants

**Purpose**

Ensure dead-code elimination retains exactly the runtime paths required by declared renderers.

**Files**

```text
crates/wrela-compiler/src/lower.rs
crates/wrela-compiler/src/flowwir_lower.rs
crates/wrela-compiler/src/pixels/glue.rs
```

**Work**

Add generated function keys to the existing image force-root calculation only when a renderer exists. Root:

- coordinator public render method;
- worker job method;
- generated numeric dispatch helpers referenced indirectly through table tags;
- display present path;
- runtime abort/failure path already required.

Do not force-root every possible primitive/material kernel. `FrameProgram` record kind census determines the exact palette and generated dispatch table. Unsupported/missing palette entry is an internal build error.

**Acceptance criteria**

- Declared scene emits only kernels for used record kinds plus fixed core orchestration.
- Removing a primitive from source can remove its unused kernel deterministically.
- No indirect function pointer is required; dispatch is bounded switch/match over record tags.
- Cost report never assigns zero to a used renderer method because a key was omitted.

**Gate**

```text
cargo xtask verify
```

**Commit**

```text
pixels P5.9: root the exact renderer kernel palette
```

## Task P5.10 — implement full frame-program/report dumps

**Purpose**

Pin serialized program, layout, and image facts before runtime rendering.

**Files**

```text
crates/wrela-compiler/src/pixels/dump.rs
crates/wrela-compiler/src/pixels/report.rs
crates/wrela-compiler/src/report.rs
crates/wrela-compiler/src/bin/wrela.rs
tests/golden/check-pixels-*/expected/frame-program.txt
tests/golden/check-pixels-*/expected/render-layout.txt
tests/golden/check-pixels-*/expected/report.txt
```

**Work**

Implement §8.3–8.5 completely. Add `pixelsconfig` dump if selected in P5.7. Report compiler/rich counts, wire bytes, mutable reservation, generated actors, worker tile ranges, and fallback policy.

Do not report expected/estimated frame rate. Existing cost section later reports emitted code proxy cycles; renderer report is structural and exact.

**Acceptance criteria**

- Dumps are generated from decoded bytes and actual `ImageLayout`, not parallel estimates.
- Table counts/offsets/digests match encoder.
- Report names formal/numeric revision.
- Renderer memory contributes to image peak memory and profile refusal.
- All permanent fixtures have reviewed pinned outputs.

**Gate**

```text
cargo xtask verify
cargo xtask report-determinism
cargo xtask repro
```

**Commit**

```text
pixels P5.10: pin frame-program and renderer-layout reports
```

## Task P5.11 — add renderer binary/layout fuzz and reproduction lanes

**Purpose**

Make the new sealed artifact as rigorously checked as existing compiler stages.

**Files**

```text
crates/xtask/src/main.rs
crates/xtask/src/fuzz.rs
crates/xtask/src/pixels_repro.rs
crates/xtask/src/golden.rs
```

**Work**

Add:

```text
cargo xtask fuzz pixels
cargo xtask pixels-repro
```

`pixels-repro` follows §8.7. Fuzz target covers symbolic field source mutations through compiler where cheap and encoded program bytes for broad mutation.

Classify expensive whole-corpus reproduction into milestone lane; keep one plane and one smooth-CSG smoke case in ordinary verify.

**Acceptance criteria**

- No new default lane exceeds locked test budget.
- Fresh-directory reproduction compares exact image bytes.
- Fuzz findings are promoted to permanent tests before fixes.
- Decoder/compiler never panics on fuzz inputs.

**Gate**

```text
cargo xtask verify
cargo xtask verify-milestone
```

**Commit**

```text
pixels P5.11: gate frame-program fuzz and reproducibility
```

### Milestone P5 close

Run `cargo xtask verify-milestone`. The milestone closes when renderer images boot, contain verified frame-program bytes and exact state reservations, expose real renderer actors, and deterministically return `UnsupportedFrameState` without presentation.

---

# Milestone P6 — verified numeric kernels and cross-language correspondence

Milestone result: every runtime proof predicate and arithmetic kernel exists in three forms—Lean theorem/model, safe Rust compiler reference, and scalar Wrela implementation—with permanent differential tests. SIMD variants may be added later but cannot change semantics.

## Task P6.1 — implement shared numeric test-vector format

**Purpose**

Drive identical cases through Rust and Wrela without a new serialization dependency.

**Files**

```text
crates/wrela-compiler/src/pixels/reference/mod.rs
crates/wrela-compiler/src/pixels/test_vectors.rs
stdlib/core/render_test_vectors.wr
crates/xtask/src/pixels_vectors.rs
formal/pixels/KERNELS.txt
```

**Work**

Define a simple generated line format with fixed integer/hex fields, for example:

```text
iv_add lo_a=-3 hi_a=7 exp_a=-12 lo_b=2 hi_b=9 exp_b=-12 out_lo=-1 out_hi=16 out_exp=-12
```

Compiler unit tests generate vectors deterministically from fixed seeds plus hand edge cases. For boot differential fixtures, xtask converts a bounded vector subset into generated Wrela constants under `core.__pixels_vectors`.

Do not parse JSON/TOML in guest. Host parser is hand-written and strict.

**Acceptance criteria**

- Unknown key, duplicate key, malformed integer, and overflow fail.
- Vector file order is stable by kernel then case ID.
- Every kernel manifest row names at least one edge vector and one generated vector family.
- Vector generation never depends on host float textual formatting; use bits or integers.

**Gate**

```text
cargo xtask verify
```

**Commit**

```text
pixels P6.1: add cross-language numeric vectors
```

## Task P6.2 — implement `Iv32` and checked dyadic arithmetic

**Purpose**

Provide exact branch-free verifier arithmetic with explicit overflow failure.

**Files**

```text
crates/wrela-compiler/src/pixels/reference/iv32.rs
stdlib/core/render_interval.wr
formal/pixels/Pixels/Dyadic.lean
formal/pixels/Pixels/Interval.lean
formal/pixels/KERNELS.txt
```

**Work**

Runtime type:

```wrela
struct Iv32:
    lo: i32
    hi: i32
    exponent: i8
```

Operations return `Result[Iv32, NumericError]` where machine overflow or unsupported exponent alignment is possible. Provide:

- `contains_zero`, `strict_positive`, `strict_negative`;
- `add`, `subtract`, `negate`;
- `multiply` using i64 widened products;
- `square`;
- `scale_pow2` checked;
- `intersect` returning `Option[Iv32]`;
- `hull`;
- `min`, `max`, `abs`;
- monotone affine evaluation;
- conversion from f32 bits plus supplied ULP/radius;
- comparison predicates used by q-order/byte checks.

Exponent normalization policy is fixed:

- align to the coarser exponent that avoids left-shift overflow;
- shift finer mantissas outward using floor for low and ceil for high;
- never silently saturate;
- exponent range `[-96, 63]` in v1.

**Acceptance criteria**

- Rust and Wrela scalar outputs agree on all vectors.
- Exhaustive tests cover all i8 endpoint values for reduced-width model and selected i32 boundaries.
- Lean containment theorems build.
- Every failure is explicit `NumericError`, never wraparound.
- Generated AArch64 uses widened multiply for interval multiply as expected; assembly shape is inspected later, not asserted here.

**Gate**

```text
cargo xtask verify
cargo xtask pixels-formal
```

**Commit**

```text
pixels P6.2: implement checked dyadic intervals
```

## Task P6.3 — implement polynomial evaluation and exact quadratic range

**Purpose**

Evaluate low-degree equations tightly and correctly at runtime.

**Files**

```text
crates/wrela-compiler/src/pixels/reference/poly.rs
stdlib/core/render_program.wr
stdlib/core/render_interval.wr
formal/pixels/Pixels/Bernstein.lean
formal/pixels/KERNELS.txt
```

**Work**

Implement scalar candidate f32/f64 and verifier `Iv32` paths for:

- Horner univariate degree 1–8;
- sparse multivariate term evaluation over `(u,v,q,t)`;
- derivative program evaluation;
- de Casteljau subdivision for fixed degree;
- Bernstein coefficient sign test;
- exact quadratic range over `[0,1]^2` using all candidate extrema;
- Taylor polynomial plus interval remainder.

The exact quadratic range routine:

1. evaluate four corners;
2. solve interior stationary point if Hessian determinant nonzero and point lies in rectangle;
3. solve one-dimensional stationary point on each of four edges;
4. evaluate all valid candidates;
5. outward-convert min/max to verifier interval.

Degenerate linear/constant edge/interior cases are explicit branches.

**Acceptance criteria**

- Rust/Wrela agree on vectors.
- Quadratic range contains dense deterministic samples and analytic extrema fixtures.
- Corner+center-only implementation would fail a pinned positive control.
- Bernstein subdivision preserves coefficient/domain mapping.
- No runtime allocation; coefficient arrays have generated fixed maxima.

**Gate**

```text
cargo xtask verify
cargo xtask pixels-formal
```

**Commit**

```text
pixels P6.3: verify low-degree polynomial ranges
```

## Task P6.4 — implement bounded root isolation

**Purpose**

Find all feature/event roots in a finite interval with explicit completeness/failure.

**Files**

```text
crates/wrela-compiler/src/pixels/reference/root.rs
stdlib/core/render_events.wr
formal/pixels/Pixels/RootIsolation.lean
formal/pixels/KERNELS.txt
```

**Work**

Implement fixed-capacity front-to-back interval subdivision:

```wrela
enum RootOutcome:
    Roots(count: u16)
    CertifiedNone
    Unresolved(reason: RootReason)
```

The caller supplies output storage and stack arrays sized by compiler capacity. Algorithm:

1. push full interval;
2. evaluate Bernstein/interval range;
3. discard if zero excluded;
4. if derivative sign excludes zero and endpoints bracket, isolate by bisection;
5. if polynomial sign-variation/root-count rule proves exact root count, subdivide until each root interval meets q/x tolerance;
6. otherwise split at exact midpoint;
7. process left before right;
8. merge only overlapping intervals proven to contain the same unique root;
9. return all roots sorted.

Tangency without sign change is handled by derivative/discriminant/root-count predicates, not converted to miss.

**Acceptance criteria**

- Plane, sphere, torus multi-root, tangent double-root, close roots, and no-root fixtures pass.
- All roots inside domain are returned or outcome is `Unresolved`; no partial list labeled complete.
- Root count never exceeds compiler capacity; overflow is `EventCapacityExceeded`/`SheetCapacityExceeded`.
- Rust/Wrela vector outputs agree exactly on interval endpoints/counts/reasons.
- Lean bracket/subdivision completeness theorems build for the used predicates.

**Gate**

```text
cargo xtask verify
cargo xtask pixels-formal
```

**Commit**

```text
pixels P6.4: isolate complete bounded root sets
```

## Task P6.5 — implement monotone tube and Krawczyk predicates

**Purpose**

Certify one root sheet continuously across a run domain.

**Files**

```text
crates/wrela-compiler/src/pixels/reference/certificate.rs
stdlib/core/render_sweep.wr
formal/pixels/Pixels/Krawczyk.lean
formal/pixels/Pixels/RunCertificate.lean
formal/pixels/KERNELS.txt
```

**Work**

Implement two fixed tiers:

**Tier 1: monotone tube**

- evaluate `G` at lower/upper q tube faces over x domain;
- require opposite uniform signs;
- evaluate `G_q` over tube and require one strict sign;
- accept one unique root per x.

**Tier 2: scalar parametric Krawczyk**

```text
A = reciprocal(center G_q) candidate, enclosed
K(E) = -A*R(X) + (1 - A*G_q(X, q_hat+E))*E
accept when K(E) strictly inside E
```

Use dyadic verifier intervals for final inclusion. Candidate arithmetic may construct `A`, `q_hat`, and initial E, but acceptance uses enclosed values.

Return detailed margins:

```wrela
struct RootCertificate:
    correction: Iv32
    derivative_margin: Iv32
    face_margin: Iv32
    contraction_margin: Iv32
    method: RootCertMethod
```

**Acceptance criteria**

- Plane accepts Tier 1 over full row absent events.
- Curved regular sheets accept one tier on permanent fixtures.
- Grazing/silhouette domain rejects before miscertification.
- Failed contraction is ordinary false, not error; numeric overflow/nonfinite is error.
- Rust/Wrela predicates agree; Lean uniqueness theorems build.

**Gate**

```text
cargo xtask verify
cargo xtask pixels-formal
```

**Commit**

```text
pixels P6.5: certify regular root tubes
```

## Task P6.6 — implement q-order and CSG event kernels

**Purpose**

Prove front order and update composite occupancy cheaply.

**Files**

```text
crates/wrela-compiler/src/pixels/reference/order.rs
crates/wrela-compiler/src/pixels/reference/csg.rs
stdlib/core/render_sweep.wr
formal/pixels/Pixels/QOrder.lean
formal/pixels/Pixels/Csg.lean
formal/pixels/KERNELS.txt
```

**Work**

Implement:

- strict interval q comparison (`a.lo > b.hi` means a nearer);
- adjacent order check and minimum slack;
- deterministic insertion sort for small row-start root lists, with explicit max;
- stable tie classification as event/ambiguity, never arbitrary order;
- CSG stack program evaluation;
- oriented object-bit toggle;
- Boolean influence/cofactor skip;
- first composite occupancy transition selection.

Root/event ordering key for equal disjoint intervals is `(q interval, feature ID, root orientation)` only after equality has been classified as a corridor. Do not use ID to decide visibility in an unresolved tie.

**Acceptance criteria**

- Close-plane fixture reports ambiguity/corridor until refined.
- All-pairs exact order and adjacent order agree for sorted lists.
- CSG stack agrees exhaustively with compiler expression fixtures.
- Noninfluential boundary skip leaves composite occupancy unchanged.
- Rust/Wrela and Lean contracts agree.

**Gate**

```text
cargo xtask verify
cargo xtask pixels-formal
```

**Commit**

```text
pixels P6.6: verify q order and CSG crossings
```

## Task P6.7 — implement fixed-q setup and recurrence

**Purpose**

Make the pixel-depth hot loop exact integer work with bounded real error.

**Files**

```text
crates/wrela-compiler/src/pixels/reference/fixed_q.rs
stdlib/core/render_raster.wr
formal/pixels/Pixels/FixedQ.lean
formal/pixels/KERNELS.txt
```

**Work**

Define:

```wrela
struct QRun4:
    q: i32x4
    dq: i32x4
    ddq: i32x4
    exponent: i8
    error_radius: i32
```

Setup chooses a shared exponent from certified q/dq/ddq maxima for one microtile width. It must prove all recurrence states and comparisons remain in i32 range. Quantize each coefficient outward and accumulate:

- source q-model error;
- coefficient conversion radius;
- recurrence rounding radius;
- derivative/Taylor remainder;
- microtile reset radius.

Packet step advances four lane states. Scalar mirror advances each independently. Reset at generated microtile width; v1 default is 32 pixels but compiler may choose a smaller power of two to satisfy range/error, never larger than 64.

**Acceptance criteria**

- Packet integer outputs equal scalar outputs bit-for-bit.
- Real q truth samples remain within q code ± error radius.
- Quantized q-order is accepted only when original slack exceeds both radii.
- Near-overflow fixture chooses smaller microtile or fails setup explicitly.
- Lean recurrence/error/no-overflow conditional theorems build.

**Gate**

```text
cargo xtask verify
cargo xtask pixels-formal
```

**Commit**

```text
pixels P6.7: implement certified fixed-q recurrence
```

## Task P6.8 — implement analytic coverage kernels

**Purpose**

Compute stable subpixel event coverage without MSAA or stochastic sampling.

**Files**

```text
crates/wrela-compiler/src/pixels/reference/coverage.rs
stdlib/core/render_coverage.wr
formal/pixels/Pixels/Coverage.lean
formal/pixels/KERNELS.txt
```

**Work**

V1 supports a unit box pixel filter. Implement:

- exact half-plane area in unit square from clipped polygon;
- quadratic-curve local approximation split into monotone segments;
- conservative lower/upper area via curve strip and line integrals;
- foreground/background side classification from event orientation;
- half-open ownership for curve exactly on pixel/tile boundary;
- coverage-to-color error budget using local color contrast bounds.

No supersample mask is used for acceptance. Deterministic dense quadrature may exist only in host oracle tests.

**Acceptance criteria**

- Axis-aligned, diagonal, corner-touching, subpixel-thin, and high-curvature fixtures pass.
- Coverage interval contains high-precision host integration.
- Shared tile boundaries neither drop nor double-count edge coverage.
- Rust/Wrela intervals agree.
- Lean half-plane and strip/color bounds build.

**Gate**

```text
cargo xtask verify
cargo xtask pixels-formal
```

**Commit**

```text
pixels P6.8: integrate certified event coverage
```

## Task P6.9 — implement normal and material bound kernels

**Purpose**

Carry geometric uncertainty into deterministic shading decisions.

**Files**

```text
crates/wrela-compiler/src/pixels/reference/normal.rs
crates/wrela-compiler/src/pixels/reference/material.rs
stdlib/core/render_material.wr
formal/pixels/Pixels/Normal.lean
formal/pixels/Pixels/MaterialBound.lean
```

**Work**

Implement:

- inverse-depth normal reconstruction;
- interval normal cone from q/q_u/q_v bounds;
- safe normalization with lower norm bound;
- dot-product interval against light/view cones;
- material scalar/vector interval evaluation;
- centered first/second moment propagation;
- separable/low-rank proposal residual verification primitive;
- filter footprint bounds.

A zero-containing normal norm is certificate failure, not arbitrary fallback normal. The sweep may refine or exact-evaluate the field gradient in a bounded local rebuild.

**Acceptance criteria**

- Plane normal exact and constant.
- Sphere/curved normals match independent gradient within cone.
- Kink/feature boundary requires event coverage path.
- Rust/Wrela bound kernels agree.
- Formal normal/moment/error theorems build.

**Gate**

```text
cargo xtask verify
cargo xtask pixels-formal
```

**Commit**

```text
pixels P6.9: bound reconstructed normals and materials
```

## Task P6.10 — implement transfer, transparency-tail, and byte kernels

**Purpose**

Complete output-referred proof arithmetic.

**Files**

```text
crates/wrela-compiler/src/pixels/reference/transfer.rs
crates/wrela-compiler/src/pixels/reference/display.rs
stdlib/core/render_transfer.wr
formal/pixels/Pixels/Compositing.lean
formal/pixels/Pixels/TransparencyTail.lean
formal/pixels/Pixels/DisplayByte.lean
formal/pixels/KERNELS.txt
```

**Work**

Implement premultiplied transfer `(rgb,t)` composition, balanced tree summary, local replacement, and tail cutoff.

Implement post path:

- exposure multiply interval;
- fixed 3×3 color transform interval;
- monotone tone LUT interval lookup/interpolation;
- monotone transfer LUT;
- exact u8 quantization with specified round-to-nearest-ties rule;
- endpoint singleton predicate;
- per-channel and RGB fixed-code result.

LUT interpolation uses integer fixed-point with compiler-proved table/domain range. Tone/transfer LUTs are checked monotone at compile time and encoded in frame program.

**Acceptance criteria**

- Composition associativity holds in formal reals; machine implementation includes rounding radius in interval path.
- Bright low-alpha tail fixture is not cut early.
- Endpoint singleton never accepts an interval crossing a code boundary.
- Rust/Wrela outputs agree for all vectors.
- Formal theorems build.

**Gate**

```text
cargo xtask verify
cargo xtask pixels-formal
```

**Commit**

```text
pixels P6.10: verify transfer and displayed bytes
```

## Task P6.11 — implement kernel manifest and differential boot lane

**Purpose**

Make cross-language correspondence a permanent repository gate.

**Files**

```text
formal/pixels/KERNELS.txt
crates/xtask/src/pixels_formal.rs
crates/xtask/src/pixels_vectors.rs
stdlib/tests/pixels_numeric.wr
tests/golden/boot-pixels-numeric/
```

**Work**

Complete theorem-to-kernel mapping. Generate a Wrela numeric test image that runs scalar kernels against embedded vectors and prints/digests results. Host xtask computes expected results through Rust reference.

The boot lane covers a focused deterministic subset in `verify`; `verify-milestone` runs the complete vector set.

**Acceptance criteria**

- Every required kernel has Lean, Rust, scalar Wrela, and vector references.
- Guest output equals host expected bytes.
- Missing/renamed symbol breaks manifest gate.
- No expected result is authored twice by hand.
- Numeric boot test contains no renderer scene logic.

**Gate**

```text
cargo xtask verify
cargo xtask verify-milestone
```

**Commit**

```text
pixels P6.11: gate theorem-to-kernel correspondence
```

## Task P6.12 — close formal trust-boundary theorems

**Purpose**

Finish the generic mathematical foundation before runtime consumes certificates.

**Files**

```text
formal/pixels/Pixels/RunCertificate.lean
formal/pixels/Pixels/EventCover.lean
formal/pixels/Pixels/TrustBoundary.lean
formal/pixels/Pixels.lean
formal/pixels/EXPECTED_AXIOMS.txt
```

**Work**

Prove and expose final named theorems:

```text
run_certificate_first_visible
complete_event_cell_preserves_structure
fixed_q_winner_matches_real_winner
coverage_composite_within_budget
transparent_summary_within_budget
display_singleton_is_exact
kinetic_slack_preserves_run
```

`TrustBoundary.lean` combines them but keeps compiler hypotheses explicit. It must not assert candidate/event completeness without a hypothesis.

Normalize and pin `#print axioms` output.

**Acceptance criteria**

- No admissions/project axioms.
- Every theorem hypothesis maps to a concrete record field or compiler verifier fact documented inline.
- Unused stronger hypotheses are removed.
- Kernel manifest references final theorem names.
- `cargo xtask pixels-formal` is green from a clean formal build.

**Gate**

```text
cargo xtask pixels-formal
cargo xtask verify-milestone
```

**Commit**

```text
pixels P6.12: close the renderer formal trust boundary
```

### Milestone P6 close

Run `cargo xtask verify-milestone`. Do not implement the sweep until all certificate predicates it will trust have scalar Rust/Wrela correspondence and the formal trust-boundary theorem set is green.

---

# Milestone P7 — from-scratch validated scanline sweep

Milestone result: the generated renderer constructs complete, exact visibility from only `FrameProgram`, current frame inputs, and bounded workspace. It handles first frame, camera cuts, and arbitrary valid frame changes. It emits certified runs or returns a render error; it never needs previous-frame state for correctness.

## Task P7.1 — implement zero-allocation `FrameProgramView`

**Purpose**

Read sealed program tables safely from placed image memory.

**Files**

```text
stdlib/core/render_program.wr
stdlib/core/render_interval.wr
stdlib/tests/pixels_program_view.wr
tests/golden/boot-pixels-program-view/
```

**Work**

Implement generated-layout readers over `Static[FrameProgramHeaderV1]` and placed arrays. All accessors use compiler-known counts and checked indexes:

```wrela
struct FrameProgramView[R]:
    # opaque placed roots/generated constants

fn header(read self) -> FrameProgramHeader
fn feature(read self, id: FeatureId) -> FeatureRecord
fn tile_features(read self, tile: TileId) -> IdSlice[FeatureId]
fn event(read self, id: EventId) -> EventRecord
fn coeff(read self, id: CoeffId, read snapshot: CoeffSnapshot) -> f32
```

Do not expose arbitrary byte offsets to renderer code. Generated constants map table IDs to placed static fields. Any index beyond compiler-generated capacity is `InternalProgramViolation`.

At renderer initialization, check header magic/version/digest against generated constants once. The guest does not recompute SHA-256 per frame; boot/image integrity already covers bytes. It verifies cheap header/table counts and reserved flags.

**Acceptance criteria**

- Program-view boot fixture reads representative records and prints expected stable values.
- No dynamic allocation or pointer arithmetic surface exists in Wrela source.
- Index failure returns explicit internal violation.
- All table loads have exact `@layout(runtime)` offsets checked by compiler layout tests.
- Accessors are pure and nonsuspending.

**Gate**

```text
cargo xtask verify
cargo xtask verify-milestone
```

**Commit**

```text
pixels P7.1: read sealed frame programs in Wrela
```

## Task P7.2 — implement frame input snapshot and validation

**Purpose**

Convert owned `RenderFrame[P]` into the exact finite coefficient state used by workers.

**Files**

```text
stdlib/core/render.wr
stdlib/core/render_actor.wr
crates/wrela-compiler/src/pixels/glue.rs
tests/golden/boot-pixels-frame-input/
```

**Work**

Generated snapshot function reads parameter fields by compiled field-index paths and packs only used scalar slots. It also canonicalizes camera/light/exposure/post inputs.

Validation:

- every scalar finite;
- every scalar inside declared `@range`;
- delta from previous presented snapshot inside `@rate` only when kinetic reuse is requested; out-of-rate remains legal for from-scratch sweep but invalidates all reuse;
- camera basis valid and canonicalized;
- near/far/output mode equal sealed config;
- light count/types equal frame program configuration;
- immutable texture/table IDs match image data;
- no input aliases renderer mutable state.

A frame outside declared `@range` returns `UnsupportedFrameState`; the compiler’s proofs do not apply. Do not clamp.

Snapshot is copied into each worker’s fixed job record. It contains no source struct padding or unused fields.

**Acceptance criteria**

- Packed offsets agree with compiler ParamTable dump.
- Field ownership P is returned on every error path.
- NaN/infinity/out-of-range controls return exact error before touching framebuffer.
- From-scratch rendering does not require a previous snapshot.
- Snapshot bytes/digest are deterministic.

**Gate**

```text
cargo xtask verify
cargo xtask verify-milestone
```

**Commit**

```text
pixels P7.2: validate and pack frame coefficients
```

## Task P7.3 — define worker workspace and reset protocol

**Purpose**

Materialize every compile-time capacity as fixed per-worker storage.

**Files**

```text
stdlib/core/render_sweep.wr
stdlib/core/render_actor.wr
crates/wrela-compiler/src/pixels/glue.rs
```

**Work**

Generate one `RendererWorkspaceR<N...>` layout per renderer containing fixed arrays:

```wrela
struct RendererWorkspaceR0:
    roots: [RootRecord; R0_MAX_ROOTS_ROW]
    roots_tmp: [RootRecord; R0_MAX_ROOTS_ROW]
    events: [IsolatedEvent; R0_MAX_EVENTS_ROW]
    event_stack: [EventCell; R0_MAX_EVENT_STACK]
    root_stack: [RootCell; R0_MAX_ROOT_STACK]
    active_features: [FeatureId; R0_MAX_FEATURES_TILE]
    active_sheets: [SheetRecord; R0_MAX_SHEETS_ROW]
    runs: [CertifiedRun; R0_MAX_RUNS_TILE]
    rebuild: [RebuildCell; R0_MAX_REBUILD]
    transfer_nodes: [TransferNode; R0_MAX_TRANSFER_NODES]
    # counters and scratch only
```

Workspace lives in the worker’s assigned mutable state region. Reset sets counts/generation markers; it does not zero entire arrays unless required for determinism/security. Every accessor checks count before read.

Use generation tags only if wrap is impossible over image lifetime or checked; otherwise reset explicit counts and overwrite live slots.

**Acceptance criteria**

- Generated layout bytes equal capacity report.
- No `List`/heap collection appears in sweep modules.
- Every push returns capacity error.
- Reset leaves no previous-frame record reachable through current counts.
- Two worker workspaces never overlap in layout.

**Gate**

```text
cargo xtask verify
```

**Commit**

```text
pixels P7.3: allocate fixed renderer workspaces
```

## Task P7.4 — enumerate complete row-start feature candidates

**Purpose**

Start each tile row from structural completeness rather than samples or prior hits.

**Files**

```text
stdlib/core/render_sweep.wr
stdlib/core/render_program.wr
crates/wrela-compiler/src/pixels/reference/sweep.rs
```

**Work**

For tile `(tx,ty)` and local row `y`:

1. fetch tile feature ID slice;
2. reject features whose half-open row span excludes `y`;
3. evaluate runtime coefficient bounds for the row/parameter snapshot;
4. apply compiler-emitted static/dynamic exclusion predicates;
5. retain all remaining feature IDs in ascending order;
6. record exclusion reason/margin counters for diagnostics, not decisions beyond the predicate itself.

No screen sample or q solve is used to decide whether a feature is a candidate. Support shells and projected spans are the completeness mechanism.

For a feature whose coefficient/runtime bound cannot be evaluated due to numeric failure, return `CertificateExhausted`/`InternalProgramViolation` according to cause; do not omit it.

**Acceptance criteria**

- Enclosed/thin feature controls retain their feature at the affected row.
- Candidate enumeration agrees with a slow host overlap/reference filter.
- Candidate order is stable.
- Every omitted tile feature has a passed exclusion predicate with positive margin.
- Counters distinguish static compiler exclusion from runtime row exclusion.

**Gate**

```text
cargo xtask verify
```

**Commit**

```text
pixels P7.4: enumerate complete row feature sets
```

## Task P7.5 — isolate every feature root at row start

**Purpose**

Build the complete ordered boundary-event list for one x position.

**Files**

```text
stdlib/core/render_sweep.wr
stdlib/core/render_events.wr
crates/wrela-compiler/src/pixels/reference/sweep.rs
```

**Work**

At initial x = tile’s left pixel-center coordinate:

- for each candidate feature, evaluate its positive q domain intersected with near/far;
- use analytic affine/quadratic candidate where available, then verifier;
- use complete bounded root isolation for torus/deformation/ambiguous cases;
- validate feature predicates and identity at each root interval;
- compute orientation interval; reject/treat event corridor if orientation contains zero;
- insert all valid roots into fixed root array;
- retain multiple roots of the same feature/object;
- sort by strict q order using interval refinement as needed;
- if intervals still overlap after fixed refinement ladder, create a zero-width/one-pixel ambiguity corridor endpoint rather than choosing order.

`RootRecord`:

```wrela
struct RootRecord:
    feature: FeatureId
    object: ObjectId
    identity_set: IdentitySetId
    q: Iv32
    orientation: RootOrientation
    validity_margin: Iv32
    root_certificate: RootCertificate
```

**Acceptance criteria**

- Plane/sphere/torus/capsule fixtures produce all expected roots.
- No sign-changing-only assumption misses tangencies.
- Root records are front-to-back (larger q first) when strict order is certified.
- Duplicate shared-feature boundary roots are deduplicated only with proof of same geometric crossing and deterministic owner rule.
- Failure to separate close roots is an event corridor/rebuild, not ID tie-break.

**Gate**

```text
cargo xtask verify
cargo xtask verify-milestone
```

**Commit**

```text
pixels P7.5: construct complete row-start roots
```

## Task P7.6 — evaluate the hard-CSG occupancy sweep

**Purpose**

Choose exact composite boundaries from ordered object crossings.

**Files**

```text
stdlib/core/render_sweep.wr
stdlib/core/render_program.wr
crates/wrela-compiler/src/pixels/reference/csg.rs
```

**Work**

Initialize object occupancy at near plane using compiler-emitted outside/inside seed programs. The usual closed-object camera-outside case is all false, but do not assume it; camera may begin inside an object within declared bounds.

Sweep roots front-to-back:

1. read current composite CSG occupancy;
2. if Boolean influence says object bit is irrelevant, toggle bit and continue without composite transition work;
3. toggle object occupancy according to oriented crossing;
4. evaluate composite occupancy;
5. when composite value changes, this root is a composite boundary;
6. the first outside→inside boundary is the visible opaque surface for an opaque ray;
7. retain ordered composite transitions for transparent/CSG layers.

At coincident/corridor roots, do not apply arbitrary sequential toggles. Isolate/refine the corridor or evaluate the full local arrangement through bounded rebuild.

**Acceptance criteria**

- Union, intersection, subtraction fixtures select exact expected boundary/identity.
- Camera-inside fixture returns first exit boundary correctly.
- Noninfluential internal object boundaries are skipped without changing output.
- Coincident boundaries take corridor path.
- Wrela output agrees with Rust CSG reference for deterministic root lists.

**Gate**

```text
cargo xtask verify
```

**Commit**

```text
pixels P7.6: sweep exact hard-CSG occupancy
```

## Task P7.7 — isolate all x-domain event endpoints for a row

**Purpose**

Partition a row into maximal domains where roots, features, identities, and order can be certified unchanged.

**Files**

```text
stdlib/core/render_events.wr
stdlib/core/render_sweep.wr
crates/wrela-compiler/src/pixels/reference/events.rs
```

**Work**

For the tile row x-domain:

1. fetch tile event-generator IDs;
2. filter by row/domain predicates;
3. isolate every event x interval using P6.4 kernels;
4. include tile left/right boundaries;
5. include fixed-q microtile reset boundaries;
6. sort event intervals by x;
7. merge intervals only when overlap means one unresolved event corridor; preserve all event IDs in that corridor;
8. produce alternating open regular domains and closed conservative event corridors.

Event roots include silhouette, feature validity, repeat, smooth band/tie, material, near/far, and q-order swap families.

A generator that returns unresolved causes deterministic x subdivision until its compiler-bounded depth. If still unresolved, the corridor covers that final cell; it is not discarded.

**Acceptance criteria**

- Every permanent edge control creates a corridor touching the true event pixel set.
- No regular domain contains a sampled sign change in host exhaustive fixture checks.
- Event endpoints use half-open ownership so adjacent tiles/rows agree.
- Multiple simultaneous event IDs remain attached to one corridor.
- Capacity overflow returns `EventCapacityExceeded` before writing past storage.

**Gate**

```text
cargo xtask verify
cargo xtask verify-milestone
```

**Commit**

```text
pixels P7.7: partition rows by complete events
```

## Task P7.8 — construct implicit-jet run candidates

**Purpose**

Predict all active root sheets across one regular x-domain with one evaluation per sheet rather than repeated solving.

**Files**

```text
stdlib/core/render_sweep.wr
stdlib/core/render_program.wr
crates/wrela-compiler/src/pixels/reference/sweep.rs
```

**Work**

At regular-domain left anchor or center:

- take root interval/candidate from current root list;
- evaluate derivative bundle at certified center root;
- require candidate `G_q` away from zero before division;
- compute `q_x`, `q_xx`, and optionally q parameter/time derivatives through implicit formulas;
- construct quadratic x model and conservative initial error interval;
- for a smooth object, evaluate only the active leaf/branch cluster proven by support/branch predicates;
- if candidate jet is nonsmooth/grazing, stop regular run at corridor or invoke local rebuild tier.

Candidate values may be f64 in Rust reference and f32 in guest; verifier intervals enclose them. The candidate itself proves nothing.

**Acceptance criteria**

- Plane model is exactly affine and has zero quadratic residual aside from representation radius.
- Sphere regular rows produce expected derivatives.
- Smooth blend uses active local cluster, not full field tape.
- Candidate generation failure does not remove the root.
- Candidate counters and active-leaf count are available for report/runtime diagnostics.

**Gate**

```text
cargo xtask verify
```

**Commit**

```text
pixels P7.8: predict regular root sheets with jets
```

## Task P7.9 — certify complete regular runs

**Purpose**

Turn candidates into proof-carrying scanline runs.

**Files**

```text
stdlib/core/render_sweep.wr
crates/wrela-compiler/src/pixels/reference/sweep.rs
formal/pixels/Pixels/RunCertificate.lean
```

**Work**

For each regular domain and every active root:

1. certify root existence/uniqueness with monotone tube or Krawczyk;
2. certify feature validity and orientation;
3. certify active smooth branch/identity set;
4. certify no omitted feature root using row candidate completeness plus runtime exclusions;
5. certify adjacent q order for the complete root list;
6. certify hard-CSG event sequence unchanged;
7. select visible opaque root and ordered transparent transitions;
8. compute maximum x endpoint before any residual, validity, order, branch, numeric, or fixed-q margin expires;
9. shorten run to the earliest endpoint;
10. emit `CertifiedRun` for `[x0,x1)`.

`CertifiedRun` must include concrete evidence fields matching the Lean theorem hypotheses:

```wrela
struct CertifiedRun:
    x0: u16
    x1: u16
    visible: SheetId
    sheet_range_start: u16
    sheet_count: u16
    q_model: QModel
    q_error: Iv32
    q_order_slack: Iv32
    root_slack: Iv32
    identity: IdentitySetId
    normal_model: NormalModel
    event_left: EventCorridorId
    event_right: EventCorridorId
    proof_owner: ProofMarginKind
```

Do not store arbitrary proof trees at runtime; store all values needed to recheck/transport plus minimum margins and owner.

**Acceptance criteria**

- Every accepted run passes host dense/oracle scoring, but oracle is never passed to renderer.
- Plane fixture can emit one run per row apart from tile/microtile boundaries.
- No run crosses a compiled event corridor.
- Root/feature/order/CSG completeness is explicit in debug proof dump.
- Lean run theorem applies directly to record invariants.

**Gate**

```text
cargo xtask verify
cargo xtask pixels-formal
cargo xtask verify-milestone
```

**Commit**

```text
pixels P7.9: emit complete certified visibility runs
```

## Task P7.10 — implement the bounded local rebuild ladder

**Purpose**

Resolve difficult regular/event domains without an unbounded or hidden dense fallback.

**Files**

```text
stdlib/core/render_sweep.wr
stdlib/core/render_events.wr
crates/wrela-compiler/src/pixels/reference/rebuild.rs
```

**Work**

The fixed ladder is:

1. **x split:** bisect the domain at dyadic midpoint;
2. **q split:** subdivide ambiguous root q intervals front-to-back;
3. **feature split:** activate a more specific primitive feature/validity domain;
4. **branch split:** split smooth band/tie predicate domain and evaluate both local clusters;
5. **event arrangement:** isolate all event predicates in the cell and enumerate their finite side states;
6. **pixel cell:** reduce x to one output pixel’s filter domain and run complete interval root/event isolation;
7. **subpixel event integration:** for an event-containing pixel, integrate both certified side runs with conservative curve coverage;
8. **failure:** return `CertificateExhausted` if any exact visibility/coverage obligation remains unresolved.

There is no generic sphere-tracing fallback in `AaaByteExact`. There is no “trace pixel center and hope” fallback. The semantic scalar tape may be evaluated inside interval/Taylor isolation for unsupported local smooth combinations already admitted by compiler, but all-root completeness remains required.

Each rebuild cell has fixed depth/record caps from `FrameProgram`. The runtime records the terminal reason.

**Acceptance criteria**

- Close-depth, tangency, silhouette, repeat-boundary, smooth-tie, and material-edge controls resolve or fail explicitly.
- No path exceeds generated arrays/depth.
- Pixel-cell path still certifies complete roots over its required point/filter domain.
- A deliberately pathological accepted scene returns `CertificateExhausted` and leaves prior frame displayed.
- Rebuild choices are fixed, not heuristic thresholds tuned at runtime.

**Gate**

```text
cargo xtask verify
cargo xtask verify-milestone
```

**Commit**

```text
pixels P7.10: resolve bounded visibility corridors
```

## Task P7.11 — carry runs across adjacent rows as proposals only

**Purpose**

Reduce candidate work without making row coherence a correctness dependency.

**Files**

```text
stdlib/core/render_sweep.wr
crates/wrela-compiler/src/pixels/reference/sweep.rs
```

**Work**

After completing row y, use its sheet/event/run states as candidate seeds for row y+1 within the same tile. Before use:

- intersect with y+1 projected feature set;
- transport q/events using compiled v derivatives/remainders;
- validate every candidate through the ordinary row-start/root/event certificates;
- enumerate any y+1 feature absent from proposal from the complete tile index;
- discard proposal wholesale on camera/input discontinuity.

A configuration switch used only in tests forces `RowProposal.Disabled`. Displayed output and success/failure must remain identical.

**Acceptance criteria**

- Disabled/enabled produce identical frame bytes and error outcomes.
- Enclosed feature absent from prior row is still discovered from structural index.
- Proposal cannot suppress a feature/event.
- Counters distinguish proposed/revalidated/new records.
- No previous tile/frame data is required.

**Gate**

```text
cargo xtask verify
```

**Commit**

```text
pixels P7.11: reuse row structure as validated proposals
```

## Task P7.12 — implement tile sweep orchestration

**Purpose**

Construct all rows/runs in one owned tile with deterministic ordering.

**Files**

```text
stdlib/core/render_sweep.wr
stdlib/core/render_actor.wr
crates/wrela-compiler/src/pixels/reference/frame.rs
```

**Work**

`render_tile`:

1. reset workspace counts;
2. load immutable tile indexes;
3. for rows ascending y:
   - enumerate candidates;
   - isolate events;
   - build row-start roots;
   - construct/certify runs;
   - resolve corridors;
   - store runs/event coverage records in tile workspace;
4. require exact half-open coverage of every pixel x in every row by either regular run or event-coverage record;
5. invoke later shading/raster stages; until P8, write a debug identity/q image into tile buffer;
6. return owned completed tile.

Add a debug validation function in Rust reference that checks run/corridor coverage has no gaps/overlaps and every row endpoint equals tile bounds.

**Acceptance criteria**

- Every tile row has exact domain partition.
- Tile order and row order deterministic.
- No unresolved record reaches debug raster; it becomes frame error.
- Plane/hard-CSG/smooth/repeat/deform permanent fixtures construct complete visibility tiles.
- Debug identity/q output matches host oracle fixture expectations.

**Gate**

```text
cargo xtask verify
cargo xtask verify-milestone
```

**Commit**

```text
pixels P7.12: construct complete visibility tiles
```

## Task P7.13 — implement coordinator/worker frame execution

**Purpose**

Run the from-scratch sweep across all sealed render cores with deterministic ownership.

**Files**

```text
stdlib/core/render_actor.wr
stdlib/core/render.wr
crates/wrela-compiler/src/pixels/glue.rs
tests/golden/boot-pixels-plane/
```

**Work**

Coordinator render turn:

1. own/validate frame input;
2. select back-buffer tile list;
3. partition tile IDs by generated ranges;
4. start one worker job per worker through existing actor/group machinery;
5. each worker sweeps assigned tiles ascending;
6. collect results in worker index order, not completion order;
7. on any error, cancel/drain group and return error without present;
8. on success, assemble tile list ascending tile ID;
9. until P8 display integration, compute/debug frame digest and return success without scanout.

Jobs are bounded and one frame render occupies one coordinator turn/child group according to existing progress rules. Long loops include compiler-proven bounds/checkpoints consistent with frame deadline semantics.

**Acceptance criteria**

- Single-core and four-core builds produce identical debug frame digest.
- Worker completion order perturbation does not alter output.
- One worker failure prevents global success/presentation.
- No worker writes another worker’s tiles/workspace.
- Actor/ring/mailbox capacities remain proven by existing compiler.

**Gate**

```text
cargo xtask verify
cargo xtask verify-milestone
```

**Commit**

```text
pixels P7.13: execute complete sweeps across render workers
```

## Task P7.14 — add independent host visibility oracle and score-only gate

**Purpose**

Validate implementation correctness without letting oracle data influence runtime decisions.

**Files**

```text
crates/wrela-compiler/src/pixels/reference/oracle.rs
crates/xtask/src/pixels_conformance.rs
crates/xtask/src/main.rs
tests/pixels_truth/
```

**Work**

Implement host f64 independent all-root oracle over field semantic graph:

- interval root isolation along each exact pixel/sample ray;
- complete roots, CSG occupancy, identity, q, independent gradient normal;
- event/coverage high-precision oracle for selected edge pixels;
- explicit unresolved rather than miss.

The oracle consumes source/frame program and rendered output only after rendering. It is not linked into guest and not passed to sweep.

Add `cargo xtask pixels-conformance` comparing:

- hit/miss;
- first identity;
- q interval containment;
- normal cone containment;
- event coverage/color interval;
- unresolved counts;
- complete output bytes once P9 exists.

**Acceptance criteria**

- Information-firewall test gives two scenes identical at any legacy lattice but different enclosed feature; sweep output differs correctly from structural data.
- Oracle unresolved is a conformance failure for flagship fixtures.
- Accepted run/corridor failures are zero.
- Conformance command is deterministic and score-only.
- Runtime source has no import/dependency on oracle module.

**Gate**

```text
cargo xtask verify
cargo xtask pixels-conformance
cargo xtask verify-milestone
```

**Commit**

```text
pixels P7.14: gate sweep visibility against an independent oracle
```

## Task P7.15 — replace placeholder render failure with debug-frame success

**Purpose**

Complete the production from-scratch visibility path before final shading/display.

**Files**

```text
stdlib/core/render_actor.wr
stdlib/core/render_sweep.wr
tests/golden/boot-pixels-*/
```

**Work**

Return `RenderedFrame[P]` with a deterministic debug visibility image:

- RGB encodes stable object/material ID;
- alpha/auxiliary digest encodes q interval class;
- event pixels use coverage between adjacent identities;
- background fixed code.

This debug mode is compiler-internal and not a source profile. It is removed in P9 after full shading, but its goldens remain host conformance fixtures.

**Acceptance criteria**

- All opaque permanent visibility fixtures render successfully from scratch.
- No `UnsupportedFrameState` remains for supported valid inputs.
- Every frame pixel is written exactly once.
- Conformance has zero visibility/identity/root failures.
- Kinetic state is not read.

**Gate**

```text
cargo xtask verify
cargo xtask pixels-conformance
cargo xtask verify-milestone
```

**Commit**

```text
pixels P7.15: complete from-scratch certified visibility
```

### Milestone P7 close

Run the full milestone gate. This is the architectural correctness gate: a valid frame is constructed from scratch with no dense truth, no legacy sample lattice, no previous frame, and no guessed pixels. Later milestones may lower cost and add quality, but cannot weaken this path.

---

# Milestone P8 — fixed-q rasterization, analytic event coverage, tile buffers, and display presentation

Milestone result: certified visibility runs become complete scanout-resolution tiles using fixed-q packet recurrence and analytic edge coverage. The display driver/VMM presents little-endian `Bgra8Srgb` tile lists and replay records exact frame digests. Shading is still the deterministic debug identity material until P9.

## Task P8.1 — fix the scanout pixel and tile contract

**Purpose**

Remove every ambiguity between HDR proof values, stored bytes, guest memory, and host presentation.

**Files**

```text
docs/language/06-machine.md
docs/language/07-pixels.md
crates/wrela-machine/src/display.rs
stdlib/drivers/display.wr
```

**Work**

Define machine-v1 format:

```text
PixelFormat.Bgra8Srgb
memory byte 0 = blue encoded u8
memory byte 1 = green encoded u8
memory byte 2 = red encoded u8
memory byte 3 = 255
```

The RGB codes are already through the sealed transfer function. Host must present them without an additional color transform except platform-required declaration of sRGB interpretation.

Define renderer tile geometry:

```text
TILE_WIDTH  = 64
TILE_HEIGHT = 32
TILE_STRIDE = 256 bytes
TILE_BYTES  = 8192
```

Every tile allocation is full size. Right/bottom partial tiles declare visible width/height; unused bytes are zero before first use and remain zero. Tile IDs are row-major.

Display list descriptor:

```rust
#[repr(C)]
pub struct DisplayTileDescV1 {
    pub guest_addr: u64,
    pub x: u16,
    pub y: u16,
    pub width: u16,
    pub height: u16,
    pub stride_bytes: u16,
    pub format: u8,
    pub reserved: [u8; 5],
}
```

**Acceptance criteria**

- Machine, compiler, driver, VMM, and goldens use one format constant.
- Alpha byte is always 255, including background.
- Partial-tile padding is deterministic zero and excluded from visible image comparison but included in raw tile digest if report says so; define both digests distinctly.
- Tile count/bytes derive exactly for arbitrary positive mode dimensions within ceiling.
- Endianness fixture proves in-memory bytes.

**Gate**

```text
cargo xtask verify
```

**Commit**

```text
pixels P8.1: seal the BGRA scanout tile contract
```

## Task P8.2 — define final run raster records

**Purpose**

Separate proof-rich sweep state from compact hot-loop setup.

**Files**

```text
stdlib/core/render_raster.wr
stdlib/core/render_sweep.wr
crates/wrela-compiler/src/pixels/reference/raster.rs
```

**Work**

At the end of sweep/shading setup, convert each regular run to:

```wrela
struct RasterRun:
    x0: u16
    x1: u16
    q: QRunSetup
    q_u: AffineRunSetup
    q_v: AffineRunSetup
    identity: IdentitySetId
    material_summary: MaterialSummaryId
    light_summary: LightSummaryId
    proof_code: OutputProofCode
```

Event pixels use:

```wrela
struct EventPixel:
    x: u16
    coverage: Iv32
    front_run: RunId
    back_run: RunId
    event_ids: IdSlice[EventId]
```

`RasterRun` contains no feature arrays or root-isolation stack. It is emitted only after all proof decisions are complete. Any output interval already proven to one code may store the code directly and skip material/light evaluation for that channel.

**Acceptance criteria**

- Conversion preserves half-open run domain exactly.
- Every run fits fixed-q setup or is split before conversion.
- Proof code identifies which stages are already singleton/constant.
- Run/event arrays fit generated capacities.
- Rust reference validates no row gap/overlap after conversion.

**Gate**

```text
cargo xtask verify
```

**Commit**

```text
pixels P8.2: lower certified runs to raster records
```

## Task P8.3 — implement scalar fixed-q raster

**Purpose**

Produce exact debug depth/identity pixels before packetization.

**Files**

```text
stdlib/core/render_raster.wr
crates/wrela-compiler/src/pixels/reference/raster.rs
stdlib/tests/pixels_raster.wr
```

**Work**

Implement `raster_run_scalar`:

1. initialize q/q derivatives at x0 from certified setup;
2. for x in `[x0,x1)`:
   - obtain q code and enclosing radius;
   - reconstruct normal inputs;
   - evaluate debug identity color;
   - write BGRA bytes once;
   - advance recurrence;
3. reset at every microtile boundary using the next setup anchor;
4. assert final recurrence index equals x1.

No per-pixel field evaluation, root solve, q-buffer search, dynamic material dispatch, or reciprocal for the debug image. If world position is not needed, q remains fixed-point.

**Acceptance criteria**

- Output matches Rust scalar reference byte-for-byte.
- Every pixel write address stays within visible/full tile extent.
- Partial tile/padding rules hold.
- Recurrence error remains within certificate at every checked sample.
- Scalar path is retained permanently as packet differential oracle.

**Gate**

```text
cargo xtask verify
```

**Commit**

```text
pixels P8.3: rasterize certified runs scalarly
```

## Task P8.4 — implement `i32x4` fixed-q packet raster

**Purpose**

Turn the hot visibility raster into vector integer additions/stores.

**Files**

```text
stdlib/core/render_raster.wr
crates/wrela-compiler/src/mwir.rs
crates/wrela-compiler/src/codegen.rs
crates/wrela-compiler/src/a64.rs
stdlib/tests/pixels_raster.wr
```

**Work**

Implement `raster_run4` processing four consecutive pixels. Use existing/implemented `i32x4` operations:

- vector add;
- vector compare/mask where needed;
- lane narrowing/packing only through named methods;
- aligned/unaligned stores defined by stdlib vector contract.

Handle run prefix/suffix of 1–3 pixels with scalar oracle. Main body uses packet recurrence. Keep q/dq/ddq in vector locals across loop iterations so register allocator can retain them.

Add missing backend vector operations one at a time with `CostRule` tags and emitted-word tests. Do not add explicit vector syntax beyond existing types.

**Acceptance criteria**

- Packet output equals scalar output for all vector fixtures and complete debug frames.
- Generated MachineWir has one vector loop and bounded scalar edges.
- No hidden stack slot traffic is assumed away; emitted assembly/report records it.
- All vector operations have exact lane semantics and first-fault behavior where checked arithmetic applies; fixed-q uses ranges proving ordinary add cannot fault.
- AArch64 code uses 128-bit ASIMD operations for main loop.

**Gate**

```text
cargo xtask verify
cargo xtask diff-eval
```

**Commit**

```text
pixels P8.4: vectorize fixed-q run rasterization
```

## Task P8.5 — reconstruct normals and optional world position per packet

**Purpose**

Supply stable geometric inputs to shading without field gradients on regular runs.

**Files**

```text
stdlib/core/render_raster.wr
stdlib/core/render_material.wr
crates/wrela-compiler/src/pixels/reference/normal.rs
```

**Work**

For each packet:

- evaluate/advance q, q_u, q_v;
- construct camera-space unnormalized normal `(q_u, q_v, q-u*q_u-v*q_v)`;
- transform by camera basis to world normal;
- normalize using explicit `rsqrt` Newton sequence defined by stdlib numeric contract;
- use normal cone certificate to skip normalization only when a shading summary does not need exact direction;
- compute world position only when the material/light summary declares it necessary, using one reciprocal per lane and raw projective ray.

Generated material dependency flags decide whether world position, view direction, tangent frame, or only normal/material identity is needed.

**Acceptance criteria**

- Plane normals are exact/constant after normalization contract.
- Curved normals lie inside certified cone and match host reference.
- Position computation is absent from debug/simple material code when unused.
- `rsqrt` sequence is bit-identical dev/release and has differential tests.
- No field-gradient call occurs in regular runs.

**Gate**

```text
cargo xtask verify
cargo xtask diff-eval
```

**Commit**

```text
pixels P8.5: reconstruct run geometry in packets
```

## Task P8.6 — rasterize analytic event coverage

**Purpose**

Write silhouettes, CSG ties, material edges, and depth swaps without missed or double-written pixels.

**Files**

```text
stdlib/core/render_coverage.wr
stdlib/core/render_raster.wr
crates/wrela-compiler/src/pixels/reference/coverage.rs
```

**Work**

For each `EventPixel`:

1. evaluate conservative coverage interval from event curve model;
2. obtain front/back debug colors or later shading intervals at the pixel;
3. premultiply/blend using exact interval arithmetic;
4. if output channel interval maps to one byte, write it;
5. otherwise invoke fixed event-coverage refinement: curve subdivision, side shading refinement, then exact pixel-domain interval integration;
6. if still not singleton under `AaaByteExact`, return `CertificateExhausted`.

For a true geometry coverage edge, both side runs may have different depth/identity. For a material-only edge, geometry/normal can be shared. For a depth swap, divide pixel coverage by swap curve and use each side’s winner.

**Acceptance criteria**

- Thin/enclosed features survive subpixel coverage.
- High-contrast diagonal silhouette has stable exact bytes against host interval oracle.
- Tile boundary event ownership is exact.
- No MSAA/TAA sample pattern is used.
- Event pixels are written exactly once after regular runs skip their corridor-owned pixels.

**Gate**

```text
cargo xtask verify
cargo xtask pixels-conformance
```

**Commit**

```text
pixels P8.6: rasterize certified event coverage
```

## Task P8.7 — implement tile buffer ownership and deterministic clearing

**Purpose**

Move completed scanout tiles safely between workers, coordinator, and display driver.

**Files**

```text
stdlib/core/render_actor.wr
stdlib/core/render_raster.wr
stdlib/drivers/display.wr
crates/wrela-compiler/src/pixels/glue.rs
```

**Work**

Generate a nominal pool binding for renderer tile buffers. Each worker receives/owns a fixed subset or one reusable tile slot plus output ownership protocol as determined by exact frame scheduling.

Double buffering uses two complete tile-list generations:

- front generation owned by display/scanout until release;
- back generation distributed to workers;
- coordinator cannot reuse front tiles before display completion;
- failure retains front generation and reclaims back generation deterministically.

On image boot, zero all tile bytes once. On subsequent frames, every visible pixel is overwritten; padding remains zero. A debug assertion/test tracks per-visible-pixel write generation in host/reference only, not production guest memory.

**Acceptance criteria**

- Ownership checker proves no concurrent writes/display reads.
- Exact tile-buffer pool bytes match layout report.
- Failure/cancellation returns every back tile to its pool.
- Front buffer persists across failed frame.
- No full-frame clear occurs per frame.

**Gate**

```text
cargo xtask verify
cargo xtask verify-milestone
```

**Commit**

```text
pixels P8.7: own double-buffered scanout tiles
```

## Task P8.8 — implement machine-v1 display driver queue

**Purpose**

Submit guest tile lists to the sealed display device with one frame doorbell.

**Files**

```text
stdlib/drivers/display.wr
crates/wrela-machine/src/display.rs
crates/wrela-vmm/src/display/mod.rs
crates/wrela-vmm/src/display/hvf.rs
crates/wrela-vmm/src/display/kvm.rs
crates/wrela-vmm/src/replay.rs
```

**Work**

Define display queue/shared-memory contract consistent with machine chapter:

- one descriptor chain names frame sequence, mode, format, tile-list address/count;
- each tile descriptor names guest-owned blob pages;
- `transfer` absent/no-op;
- publish uses existing queue release ordering;
- one doorbell per frame;
- completion means host presentation accepted and prior front generation can be reclaimed according to vsync contract;
- vsync arrives through frame vector;
- malformed descriptors are device errors and never host out-of-bounds reads.

macOS backend:

- create `BGRA8Unorm_sRGB` Metal texture/layer path or equivalent platform surface;
- gather tile rows without changing bytes/color;
- present on requested vsync.

Linux backend:

- gather into DRM dumb buffer/Mesa present path preserving byte format;
- no GPU shading/geometry.

The exact backend implementation may use host APIs already allowed by the VMM crate; no renderer logic moves host-side.

**Acceptance criteria**

- Portable device-model tests validate all descriptors/ranges/format/mode.
- HVF and KVM backends produce identical visible frame digest before presentation.
- One frame submission causes one recorded output event/doorbell.
- Host never reads outside declared tile visible/full bounds.
- Display error reaches coordinator as `DisplayFailure` with tile ownership recovered.

**Gate**

```text
cargo xtask verify
cargo xtask verify-milestone
```

**Commit**

```text
pixels P8.8: present BGRA tile lists through machine v1
```

## Task P8.9 — integrate presentation in renderer coordinator

**Purpose**

Make a successful render atomically become the next displayed frame.

**Files**

```text
stdlib/core/render_actor.wr
stdlib/drivers/display.wr
tests/golden/boot-pixels-plane/
```

**Work**

After all worker tiles succeed:

1. assemble descriptor list ascending tile ID;
2. compute visible frame digest and raw tile digest through generated bounded digest routine;
3. call display driver handoff/present method;
4. await/observe completion according to driver contract;
5. swap front/back generation only on success;
6. return `RenderedFrame[P]` with frame sequence and ownership of P.

Do not let a late present failure mark back buffer as front. Deadline behavior follows existing actor call/deadline rules.

**Acceptance criteria**

- First successful debug frame appears in VMM boot golden.
- Failed frame leaves prior digest/front buffer.
- Frame sequence increments only on successful present.
- Tile descriptor order is deterministic independent of worker completion.
- Single-core/four-core visible digest identical.

**Gate**

```text
cargo xtask verify
cargo xtask verify-milestone
```

**Commit**

```text
pixels P8.9: atomically present completed renderer frames
```

## Task P8.10 — add frame digest and replay conformance

**Purpose**

Make replay reproduce exactly what was displayed.

**Files**

```text
crates/wrela-vmm/src/replay.rs
crates/wrela-machine/src/report.rs
crates/xtask/src/pixels_conformance.rs
tests/golden/boot-pixels-*/
```

**Work**

Record per successful frame:

```text
renderer index
frame sequence
mode/format
tile descriptor digest
visible pixel digest
raw tile/padding digest
vsync/checkpoint association
```

Replay suppresses host presentation if existing replay policy requires and verifies guest submission/digests against log. Any divergence reports first frame and digest class.

**Acceptance criteria**

- Record then replay permanent debug fixtures with zero divergence.
- Changing one pixel or padding byte identifies visible/raw class correctly.
- Failed/unpresented frame is not logged as output.
- Replay ordering composes with existing cross-core admission/checkpoint log.
- Report names format/replay contract revision.

**Gate**

```text
cargo xtask verify
cargo xtask verify-milestone
```

**Commit**

```text
pixels P8.10: record exact displayed frame digests
```

## Task P8.11 — complete debug visibility conformance

**Purpose**

Lock visibility/raster/display correctness before AAA shading can obscure it.

**Files**

```text
crates/xtask/src/pixels_conformance.rs
tests/golden/boot-pixels-*/
tests/pixels_truth/
```

**Work**

Run all opaque visibility fixtures through full guest/VMM presentation. Compare visible debug output to host oracle and assert:

- zero hit/miss disagreement;
- zero first-identity disagreement;
- every reconstructed q inside certified interval;
- every normal inside certified cone;
- zero missed event coverage;
- exact expected debug bytes/digest;
- zero unresolved frame.

Preserve separate controls for plane, hard CSG, smooth CSG, repeat, displacement, close depth, thin feature, enclosed feature, material edge.

**Acceptance criteria**

- All assertions zero.
- Conformance does not alter render inputs/decisions.
- Legacy fieldprobe result remains untouched/historical.
- Debug visibility path remains runnable after P9 for regression diagnosis.
- Kinetic mode is forced disabled.

**Gate**

```text
cargo xtask pixels-conformance
cargo xtask verify-milestone
```

**Commit**

```text
pixels P8.11: lock full visibility and scanout conformance
```

### Milestone P8 close

Run `cargo xtask verify-milestone`. The renderer now constructs and presents every supported frame from scratch with certified geometry and analytic coverage. Do not add temporal reuse before full AAA shading/output correctness is complete.

---

# Milestone P9 — AAA material, texture, lighting, shadow, filtering, and output-byte verification

Milestone result: debug identity color is replaced by deterministic physically based shading and complete output-byte certification. Every supported frame produces final `Bgra8Srgb` bytes with explicit coverage/shading/shadow/filter/post error budgets. No stochastic sampling or denoising exists.

## Task P9.1 — fix the working color and filmic-output contract

**Purpose**

Give every material/light/post operation one exact color interpretation.

**Files**

```text
docs/language/07-pixels.md
stdlib/core/render.wr
stdlib/core/render_transfer.wr
stdlib/data/pixels/filmic_v1_u16.bin
stdlib/data/pixels/srgb_v1_u16.bin
crates/wrela-compiler/src/pixels/tables.rs
crates/wrela-compiler/src/pixels/program.rs
```

**Work**

Working space:

- scene radiance and material RGB are linear Rec.709/sRGB primaries, D65;
- negative radiance is conservatively bounded then clamped to zero immediately before tone mapping; source materials/lights with negative declared values are rejected unless an explicit signed procedural intermediate resolves nonnegative;
- final output is per-channel filmic mapping followed by the sealed sRGB transfer table and u8 quantization.

`ToneCurve.FilmicV1` canonical generator formula:

```text
A=0.15 B=0.50 C=0.10 D=0.20 E=0.02 F=0.30 W=11.2
h(x)=((x*(A*x+C*B)+D*E)/(x*(A*x+B)+D*F))-E/F
f(x)=clamp(h(x)/h(W),0,1)
```

The runtime does not evaluate the rational curve. The repository stores a canonical 4097-entry u16 LUT over log2 input domain `[-16,+16]`, with piecewise-linear interpolation. `srgb_v1_u16.bin` is a canonical 4097-entry u16 LUT over `[0,1]` for the standard sRGB OETF. The checked-in bytes, dimensions, domains, and SHA-256 are the numeric contract; regeneration is maintainer-only.

Add `tools/gen_pixels_tables.rs` as a standalone Rust source compiled/run only deliberately. It may use f64 formula evaluation to propose tables, but regeneration writes a candidate file and refuses to overwrite canonical bytes without `--accept`. Verification checks digest and monotonicity, not host regeneration equivalence.

Embed LUTs into `FrameProgram` or shared immutable rodata by digest/reference; do not duplicate per renderer.

**Acceptance criteria**

- Compiler verifies exact byte length, endpoints, monotonicity, and digest.
- Runtime interpolation is integer/fixed-point and deterministic.
- Formal byte theorem relies on the verified monotone table, not an unproved analytic formula.
- Color/channel order is tested end-to-end through VMM.
- Numeric-contract revision changes if table bytes/domain/interpolation changes.

**Gate**

```text
cargo xtask verify
cargo xtask pixels-formal
```

**Commit**

```text
pixels P9.1: seal the working color and filmic tables
```

## Task P9.2 — define the v1 physically based material model

**Purpose**

Provide a closed, high-quality BRDF that can be bounded and packetized.

**Files**

```text
stdlib/core/render.wr
stdlib/core/render_material.wr
crates/wrela-compiler/src/pixels/material.rs
crates/wrela-compiler/src/pixels/material_graph.rs
```

**Work**

`MaterialSample.standard` fields:

```wrela
base_color: Rgb       # linear, each [0,1]
metallic: f32         # [0,1]
roughness: f32        # [0.02,1]
specular: f32         # dielectric F0 scale [0,1]
emissive: Rgb         # finite nonnegative, bounded by profile
opacity: f32          # [0,1]
normal_detail: NormalDetail
```

Convenience `clay` and `porcelain` lower to fixed `standard` values.

BRDF is fixed:

- Lambert/Burley-style diffuse with energy reduced by metallic and Fresnel;
- isotropic GGX/Trowbridge-Reitz NDF;
- height-correlated Smith visibility;
- Schlick Fresnel with `F0 = mix(0.08*specular, base_color, metallic)`;
- roughness parameter `alpha = max(roughness*roughness, 0.0004)`;
- no clearcoat, sheen, anisotropy, subsurface, transmission, or iridescence in v1;
- emissive adds after reflected direct/GI;
- opacity is handled by ordered transfer, not by scaling geometry coverage.

Write formulas and denominator clamps explicitly in `07-pixels.md`. Clamps must have physical/domain reasons and interval rules, not artifact hiding.

**Acceptance criteria**

- Material constructors enforce ranges.
- Compiler emits a closed material feature flag set.
- Scalar Rust/Wrela BRDF agree on permanent vectors.
- White furnace host test verifies bounded energy for the supported parameter grid within a documented numeric radius.
- No unsupported lobe silently maps to standard.

**Gate**

```text
cargo xtask verify
```

**Commit**

```text
pixels P9.2: fix the standard diffuse GGX material
```

## Task P9.3 — implement deterministic texture assets and sampling

**Purpose**

Support production surface detail without unbounded procedural evaluation or aliasing.

**Files**

```text
stdlib/core/render.wr
stdlib/core/render_material.wr
crates/wrela-compiler/src/pixels/texture.rs
crates/wrela-compiler/src/pixels/program.rs
crates/wrela-compiler/src/pixels/encode.rs
```

**Work**

Add compiler-known immutable `Texture2D[Format,W,H]` assets. V1 formats:

- `Rgb8Srgb` base color;
- `Rgb8Linear` data;
- `Rg8Snorm` tangent/object-space slope map;
- `R8Linear` scalar mask.

Compiler builds and stores:

- full deterministic mip chain to 1×1;
- per-mip/channel min/max pyramid for interval bounds;
- slope first/second moments for `Rg8Snorm`;
- SHA-256 asset digest;
- fixed address/table record.

Filtering:

- bilinear within mip;
- trilinear between adjacent mips;
- deterministic 4-tap anisotropic along major footprint axis when ratio > 2, with positions `[-3/8,-1/8,1/8,3/8]` of the projected major footprint and normalized equal weights;
- maximum anisotropy 4 in v1;
- wrap modes clamp and repeat only; repeat boundaries are explicit material events when they create discontinuities;
- mip selected from certified UV derivative footprint, rounded outward.

UV sources:

- analytic primitive UV for sphere/cylinder/torus/plane;
- box/round-box feature-local UV;
- object/world triplanar projection;
- no arbitrary runtime UV topology function in v1.

**Acceptance criteria**

- Mip generation is byte-deterministic and independently decodable.
- Min/max pyramid encloses all footprint samples.
- Texture asset bytes contribute to build identity/memory report.
- Seam/wrap events are represented or filtered continuously.
- Host high-resolution texture oracle lies inside runtime sample interval.

**Gate**

```text
cargo xtask verify
cargo xtask repro
```

**Commit**

```text
pixels P9.3: compile deterministic filtered textures
```

## Task P9.4 — compile material dependency and summary programs

**Purpose**

Evaluate smooth interior shading once per run/subrun when a verified summary suffices.

**Files**

```text
crates/wrela-compiler/src/pixels/material.rs
crates/wrela-compiler/src/pixels/program.rs
stdlib/core/render_material.wr
formal/pixels/Pixels/MaterialBound.lean
```

**Work**

For each material identity compile:

- required surface inputs bitset;
- scalar/vector expression program;
- parameter dependencies;
- texture/UV footprint requirements;
- first/second derivative programs over x/y where available;
- output range and mixed-derivative bounds;
- supported summary basis.

Summary ladder fixed for v1:

1. constant interval summary;
2. affine x summary;
3. quadratic x summary;
4. separable rank up to 4 over tile `(x,y)` using deterministic cross pivots;
5. per-pixel material evaluation.

Every summary carries an a posteriori residual interval computed from exact sampled anchor evaluations plus derivative/Taylor/min-max texture bounds. A proposer result without residual is never accepted.

Deterministic rank pivot:

- candidate grid is fixed 5×5 normalized tile coordinates;
- choose location with greatest current interval residual upper bound;
- tie by y then x;
- stop at rank 4 or output budget.

**Acceptance criteria**

- Constant clay/porcelain use constant summaries where geometry/light permit.
- Procedural/texture summaries either verify or fall to exact per-pixel material evaluation.
- Rank is never assumed from scene class.
- Summary plus residual contains host per-pixel material results.
- Compiler/runtime counts and capacities include anchors/basis storage.

**Gate**

```text
cargo xtask verify
cargo xtask pixels-formal
```

**Commit**

```text
pixels P9.4: compile verified material summaries
```

## Task P9.5 — implement normal-detail moment filtering

**Purpose**

Remove specular shimmer from subpixel normal/slope detail deterministically.

**Files**

```text
stdlib/core/render_material.wr
crates/wrela-compiler/src/pixels/texture.rs
crates/wrela-compiler/src/pixels/reference/moments.rs
formal/pixels/Pixels/MaterialBound.lean
```

**Work**

For each filtered footprint obtain slope moments:

```text
E[sx], E[sy], E[sx²], E[sx sy], E[sy²]
```

For analytic procedural normal detail, compiler emits exact/bounded moments over supported sinusoidal spectra. For texture slope maps, mip moment pyramids provide bounds.

Convert moments into:

- mean perturbed normal;
- covariance/variance roughness adjustment;
- conservative BRDF curvature error bound.

If moment model’s output interval exceeds budget, refine mip/footprint subdivision or evaluate a bounded deterministic tap set. Do not sample one normal and hope.

V1 does not support arbitrary tangent-space normal map orientation on topology without a compiled tangent frame. Triplanar/object-space slopes are the default field-native path.

**Acceptance criteria**

- Distant high-frequency normal fixture has stable frame bytes under subpixel camera motion.
- Flat/constant detail reduces exactly to original material.
- Moment-filtered BRDF interval contains dense high-resolution host integration.
- No stochastic sample phase exists.
- Formal moment/error lemmas build.

**Gate**

```text
cargo xtask verify
cargo xtask pixels-formal
```

**Commit**

```text
pixels P9.5: filter subpixel normal detail by moments
```

## Task P9.6 — implement direct-light evaluation and bounds

**Purpose**

Shade certified geometry under the complete supported light set.

**Files**

```text
stdlib/core/render_light.wr
stdlib/core/render_material.wr
crates/wrela-compiler/src/pixels/light.rs
crates/wrela-compiler/src/pixels/reference/light.rs
```

**Work**

Supported lights:

- directional: normalized direction, RGB radiance;
- point: position, RGB intensity, finite radius clamp to avoid singular source;
- rectangle: center, orthonormal axes, half extents, RGB radiance;
- disk: center, normal, radius, RGB radiance.

Compiler validates finite ranges/rates and emits light coefficient slots, world bounds, maximum incident radiance, and influence bounds.

Direct BRDF evaluation:

- directional/point use one representative direction plus interval direction/radiance bounds over run;
- area lights integrate BRDF × cosine × visibility over source domain through P9.8;
- normal-cone tests can prove whole run unlit or front-lit;
- distance attenuation for point is `1/max(r², radius²)` with explicit interval;
- contribution is culled only if complete encoded impact fits assigned budget.

**Acceptance criteria**

- Light movement outside declared range rejected at frame input.
- Normal-cone unlit classification never false-lights host samples.
- Point singularity impossible by source contract.
- Scalar/packet light math agrees.
- Contribution bounds flow to display scheduler.

**Gate**

```text
cargo xtask verify
```

**Commit**

```text
pixels P9.6: evaluate bounded direct lighting
```

## Task P9.7 — implement certified secondary visibility

**Purpose**

Answer shadow/AO/probe visibility using the same complete structural scene without screen projection assumptions.

**Files**

```text
stdlib/core/render_light.wr
stdlib/core/render_program.wr
crates/wrela-compiler/src/pixels/reference/secondary.rs
formal/pixels/Pixels/RootIsolation.lean
```

**Work**

Define a secondary ray/segment:

```wrela
struct SegmentQuery:
    origin: Vec3
    direction: Vec3
    t_min: f32
    t_max: f32
    exclude_feature: FeatureId
```

Candidate discovery uses world/object/feature bounds indexed by a compiler-emitted coarse uniform object grid or bounding interval list? To avoid a rejected volumetric field octree and runtime O(scene), use the existing object/feature bounding hierarchy compiled as a flat BVH over **surface object bounds**:

- build deterministic binary BVH with median split on largest centroid extent, tie x/y/z then stable object ID;
- leaves contain object IDs, not volumetric field samples;
- runtime traverses with fixed stack capacity;
- BVH is allowed because it culls object bounds for secondary rays and does not approximate/replace the SDF or primary sweep.

For each candidate object/feature isolate all roots along t, apply feature validity, sort, run CSG occupancy, and return:

```wrela
enum SegmentVisibility:
    Clear
    Blocked(first_t: Iv32, identity: IdentitySetId)
    Unresolved(RenderError)
```

Offset origin along certified normal by profile epsilon derived from q/position/normal error and world scale; interval proof ensures start is outside excluded surface. Exclude only the exact originating feature within the offset corridor, not the whole object.

**Acceptance criteria**

- BVH traversal and brute-force feature enumeration agree in host tests.
- Thin blocker controls are not skipped.
- Self-shadow acne and light leaks are absent in scale-sweep fixtures.
- CSG subtraction/intersection shadows use exact occupancy.
- No primary tile’s pruned/indexed feature list is reused for unrelated secondary rays.

**Gate**

```text
cargo xtask verify
cargo xtask pixels-formal
```

**Commit**

```text
pixels P9.7: answer complete secondary visibility
```

## Task P9.8 — implement area-light source integration

**Purpose**

Produce deterministic, band-free soft shadows for rectangle and disk lights.

**Files**

```text
stdlib/core/render_light.wr
crates/wrela-compiler/src/pixels/reference/area_light.rs
formal/pixels/Pixels/MaterialBound.lean
```

**Work**

Integrate over normalized emitter domain:

```text
rectangle: (s,t) in [-1,1]²
disk: concentric square-to-disk map with explicit Jacobian
```

Use fixed adaptive dyadic subdivision with interval contribution bounds. For each emitter cell:

1. bound emitter position/direction/distance/cosines/BRDF;
2. classify visibility for the entire cell when blocker/order margins permit using segment-query interval bundle;
3. if certainly clear, add full radiance interval integral;
4. if certainly blocked, add zero;
5. otherwise bound contribution upper/lower and compare cell uncertainty to assigned encoded budget;
6. subdivide into four in Morton order if needed and depth/capacity permits;
7. at terminal depth, accept only when remaining radiance interval fits budget; otherwise `CertificateExhausted`.

Candidate point visibility queries at cell centers may propose blockers, but full-cell proof decides.

For a stable single blocker edge, compiler/runtime may build a one-dimensional transition summary, but it must be validated against the same source integral and is only an acceleration.

**Acceptance criteria**

- One-edge penumbra fixture is smooth and stable under motion.
- Multiple blockers and near-field light fixtures remain bounded/correct.
- No stochastic shadow rays/noise/denoiser.
- Integrated interval contains a high-resolution host source integral.
- Capacity/depth exhaustion is explicit.

**Gate**

```text
cargo xtask verify
cargo xtask pixels-conformance
```

**Commit**

```text
pixels P9.8: integrate certified area-light visibility
```

## Task P9.9 — implement deterministic AO taps

**Purpose**

Add local contact shading without a volumetric bake or stochastic rays.

**Files**

```text
stdlib/core/render_light.wr
crates/wrela-compiler/src/pixels/reference/ao.rs
```

**Work**

Renderer config adds sealed `ao_radius`, `ao_strength`. V1 uses five normalized distances:

```text
[1/16, 1/8, 1/4, 1/2, 1] * ao_radius
weights [0.40, 0.25, 0.16, 0.11, 0.08]
```

At `P + n*s_i`, evaluate conservative scene distance/lower-bound program using secondary candidate BVH and active semantic field evaluator. Define:

```text
occ_i = clamp((s_i - max(distance_lower,0)) / s_i, 0, 1)
AO = clamp(1 - ao_strength * sum(weight_i * occ_i), 0, 1)
```

Use intervals for lower/upper AO. If normal/position uncertainty broadens taps beyond output budget, refine geometry/shading or perform exact local interval evaluations. Taps do not determine primary visibility.

**Acceptance criteria**

- Open plane AO is 1 within exact radius.
- Contact/crevice fixtures darken deterministically.
- AO interval contains dense host reference.
- No full-sphere directions or random kernel.
- AO contribution can be skipped only through display budget proof.

**Gate**

```text
cargo xtask verify
```

**Commit**

```text
pixels P9.9: add deterministic normal-distance AO
```

## Task P9.10 — implement shading run summaries and packet evaluation

**Purpose**

Amortize material/light work across certified structure while retaining exact output bounds.

**Files**

```text
stdlib/core/render_material.wr
stdlib/core/render_light.wr
stdlib/core/render_raster.wr
crates/wrela-compiler/src/pixels/reference/shade.rs
```

**Work**

For each run/tile material-light pair construct the fixed summary ladder from P9.4. Summary records contain candidate coefficients plus HDR interval residual per channel.

Packet pixel evaluation:

- advance basis functions by forward differences where polynomial;
- evaluate separable rank terms in SoA;
- add exact/per-pixel residual-sensitive terms only where required;
- combine diffuse/specular/emissive/AO/shadow;
- carry an HDR interval alongside candidate RGB only until byte singleton is proven;
- once all channels fixed, store bytes without further floating work.

Per-pixel exact shading fallback is permitted and bounded by pixel/run capacity; it still uses certified geometry and deterministic BRDF/visibility. It is not a primary visibility fallback.

**Acceptance criteria**

- Constant material/light plane shares one summary across maximal runs.
- Summary output interval contains scalar per-pixel reference.
- Scalar/packet candidate bytes agree after verifier.
- Unsupported high-frequency material reaches exact per-pixel path or build/runtime explicit failure, never unchecked rank approximation.
- Runtime counters identify summary ranks and exact-shaded pixels for diagnostics, not acceptance tuning.

**Gate**

```text
cargo xtask verify
cargo xtask diff-eval
```

**Commit**

```text
pixels P9.10: shade certified runs with verified summaries
```

## Task P9.11 — implement the display-error budget and refinement queue

**Purpose**

Stop every approximation at one common output criterion and choose deterministic refinements.

**Files**

```text
stdlib/core/render_transfer.wr
stdlib/core/render_actor.wr
crates/wrela-compiler/src/pixels/reference/scheduler.rs
formal/pixels/Pixels/DisplayByte.lean
```

**Work**

Each unresolved output unit (run channel/event pixel/area-light cell/transparent tail) maintains contributors:

```wrela
enum ErrorSource:
    Visibility
    Coverage
    Normal
    Material
    Texture
    DirectLight
    Shadow
    Ao
    Gi
    Transparency
    Post
    Temporal
    Quantization
```

A refinement option declares:

- source;
- current interval code span upper bound;
- guaranteed bound reduction order/factor from its certificate model;
- static worst-case operation count;
- remaining depth/capacity;
- deterministic payload ID.

Select greatest guaranteed code-span reduction per operation using integer cross multiplication. Tie by `ErrorSource` enum order then payload ID. Apply one refinement, recompute complete output interval, stop when all channel endpoint codes equal.

If no option remains and codes differ, return `CertificateExhausted`. Do not choose the candidate’s nearest byte.

There is no claim of a global submodular approximation ratio.

**Acceptance criteria**

- Queue ordering deterministic across cores/hosts.
- No floating division in priority comparison.
- Every refinement strictly decreases a discrete measure `(remaining depths, interval widths)` or terminates, proving bounded progress.
- Exact small fixtures compare scheduler result to exhaustive refinement and produce same final bytes.
- Formal byte singleton theorem gates success.

**Gate**

```text
cargo xtask verify
cargo xtask pixels-formal
```

**Commit**

```text
pixels P9.11: schedule refinements in display units
```

## Task P9.12 — replace debug output with final filmic BGRA output

**Purpose**

Complete final opaque AAA framebuffer generation.

**Files**

```text
stdlib/core/render_raster.wr
stdlib/core/render_actor.wr
stdlib/core/render_transfer.wr
tests/golden/boot-pixels-*/
```

**Work**

For every regular/event pixel:

1. evaluate candidate HDR color and complete interval;
2. run refinement queue until each encoded channel singleton;
3. write exact B,G,R codes and alpha 255;
4. preserve debug visibility mode behind compiler-internal conformance flag, not source option.

Background is an explicit environment material/light color with its own fixed interval. It is not an implicit zero after failed visibility.

Update frame digests/goldens from debug to final output while retaining separate debug conformance expected files.

**Acceptance criteria**

- All opaque permanent fixtures produce final filmic bytes.
- Every stored channel had a singleton proof or exact zero-width arithmetic path.
- No output candidate is quantized without endpoint agreement.
- Host framebuffer oracle lies within HDR intervals and final bytes agree.
- No unresolved frame.

**Gate**

```text
cargo xtask verify
cargo xtask pixels-conformance
cargo xtask verify-milestone
```

**Commit**

```text
pixels P9.12: emit final byte-certified opaque frames
```

## Task P9.13 — add motion/lighting/material quality conformance

**Purpose**

Lock visible AAA properties, not only static geometry correctness.

**Files**

```text
crates/xtask/src/pixels_conformance.rs
tests/golden/boot-pixels-quality/
tests/pixels_truth/quality/
```

**Work**

Permanent deterministic sequences:

- slow subpixel camera pan across diagonal silhouettes;
- glossy highlight moving across sphere/porcelain;
- high-frequency slope texture receding in depth;
- rectangle-light penumbra crossing hard/smooth geometry;
- thin blade crossing bright/dark background;
- material edge with identical depth;
- AO contact separation;
- exposure change across filmic shoulder.

Compare:

- exact final frame digest per frame;
- no false-lit/false-shadowed host samples;
- HDR truth inside renderer intervals;
- event coverage containment;
- temporal byte changes against deterministic expected sequence;
- no stochastic variation across repeated runs.

Do not use a single perceptual aggregate to hide visibility/shadow errors.

**Acceptance criteria**

- All sequence digests stable.
- No visibility/identity/shadow classification failures.
- High-frequency detail does not alternate unpredictably under subpixel motion.
- Re-running identical sequence produces byte-identical frames.
- Single/four-core outputs identical.

**Gate**

```text
cargo xtask pixels-conformance
cargo xtask verify-milestone
```

**Commit**

```text
pixels P9.13: lock opaque AAA quality sequences
```

### Milestone P9 close

Run `cargo xtask verify-milestone`. Opaque rendering is now feature-complete and byte-certified. Any later transparency/GI/temporal work composes into the same output verifier and cannot bypass it.

---

# Milestone P10 — ordered transparency and deterministic probe GI

Milestone result: the renderer handles bounded semitransparent surface stacks and deterministic diffuse global illumination. Both compose into the existing HDR interval and display-byte verifier. V1 transparency is absorptive/emissive alpha compositing with no refraction.

## Task P10.1 — classify opaque and transparent material identities

**Purpose**

Make layer semantics and maximum stack capacity compile-time facts.

**Files**

```text
crates/wrela-compiler/src/pixels/material.rs
crates/wrela-compiler/src/pixels/capacities.rs
crates/wrela-compiler/src/pixels/program.rs
stdlib/core/render.wr
```

**Work**

Classify each material identity over its complete parameter range:

```rust
enum OpacityClass {
    Opaque,                  // opacity exactly 1
    Transparent { lo: f64, hi: f64 }, // 0 <= lo <= hi < 1 or includes 1
    Invisible,               // opacity exactly 0 and emissive exactly 0
    Parameterized,           // finite interval; runtime class may vary
}
```

Invisible surfaces still affect CSG occupancy/geometry but do not emit a transfer layer unless material/event semantics require. Parameterized opacity boundaries (`opacity == 0` or `1`) become material events if class changes affect layer topology; otherwise runtime can conservatively treat as transparent.

Derive max transparent layers per run/pixel from complete ordered composite transitions and feature overlap. Count front/back boundaries separately.

V1 explicitly rejects:

- refractive direction changes;
- volumetric participating media;
- stochastic alpha test;
- order-independent transparency approximation;
- unbounded particle/hair layer count.

**Acceptance criteria**

- Opaque fixture uses no transfer tree beyond one absorbing layer.
- Transparent-tail fixture has exact layer capacity.
- Zero-opacity nonemissive layer is safely skipped through material proof.
- Parameterized class changes are event-tracked or conservatively transparent.
- Capacity overflow is compile-time `P015`.

**Gate**

```text
cargo xtask verify
```

**Commit**

```text
pixels P10.1: classify bounded transparent layers
```

## Task P10.2 — build ordered transfer layers from the CSG sweep

**Purpose**

Convert the complete front-to-back composite boundary sequence into shading/compositing work.

**Files**

```text
stdlib/core/render_sweep.wr
stdlib/core/render_transfer.wr
crates/wrela-compiler/src/pixels/reference/transfer.rs
```

**Work**

For each run, retain every composite boundary transition until:

- an opaque surface is reached;
- q falls behind far plane/background;
- compiler layer capacity exhausted (error);
- transparent tail is later proven negligible.

A layer record:

```wrela
struct SurfaceLayer:
    sheet: SheetId
    identity: IdentitySetId
    q_model: QModel
    q_error: Iv32
    orientation: RootOrientation
    material_summary: MaterialSummaryId
```

Shading is sided according to material contract and orientation. The same geometric boundary cannot be duplicated by adjacent primitive features after deduplication.

At event/depth-swap corridors, layer order/coverage is represented separately per side and integrated analytically.

**Acceptance criteria**

- Ordered layer list agrees with host all-root/CSG oracle.
- Opaque first layer terminates deeper visibility work.
- Transparent layers retain exact q order/slack.
- Coincident transparent surfaces use event corridor/rebuild, not arbitrary ID order.
- Layer capacity is enforced before writes.

**Gate**

```text
cargo xtask verify
cargo xtask pixels-conformance
```

**Commit**

```text
pixels P10.2: emit exact ordered surface layers
```

## Task P10.3 — implement balanced transfer trees

**Purpose**

Compose transparent stacks associatively and prepare for local kinetic repairs.

**Files**

```text
stdlib/core/render_transfer.wr
crates/wrela-compiler/src/pixels/reference/transfer.rs
formal/pixels/Pixels/Compositing.lean
```

**Work**

Each shaded layer yields premultiplied transfer:

```text
C = coverage * opacity * shaded_rgb
T = 1 - coverage * opacity
```

For regular interior pixels coverage is 1. Event coverage is interval-valued and enters both C/T bounds.

Store leaves in front-to-back order and build a balanced array tree sized to next power of two from sealed max layers. Identity leaves are `(0,1)`. Parent composition order is left/front then right/back.

For runs where all layer transfer summaries are low-degree/separable, tree nodes also store summary coefficients/residual. Otherwise compose per pixel. The verifier interval always follows the same order.

**Acceptance criteria**

- Balanced and linear front-to-back composition agree within arithmetic interval and exact candidate bits where operation order is intentionally matched.
- Tree storage has fixed maximum and deterministic leaf placement.
- Local leaf replacement updates only ancestors.
- Opaque prefix yields residual T exactly/interval containing zero and can absorb tail.
- Formal monoid/local-repair theorems build.

**Gate**

```text
cargo xtask verify
cargo xtask pixels-formal
```

**Commit**

```text
pixels P10.3: compose transparent stacks as transfer trees
```

## Task P10.4 — implement certified transparency-tail termination

**Purpose**

Avoid shading deep transparent layers that cannot affect stored output.

**Files**

```text
stdlib/core/render_transfer.wr
crates/wrela-compiler/src/pixels/material.rs
formal/pixels/Pixels/TransparencyTail.lean
```

**Work**

Compiler emits a finite maximum radiance/deviation bound per material/light/background class. Runtime maintains prefix transmittance interval `T` and suffix proxy/deviation bound `ΔL`.

Tail may be replaced by proxy only when complete post sensitivity proves:

```text
upper(T) * upper(ΔL) * upper(post_sensitivity) < assigned_linear_or_encoded_budget
```

For direct byte proof, compose prefix plus proxy and add tail HDR interval, then run final endpoint singleton. The simple product predicate is an early sufficient test, not the final byte decision.

Suffix proxies fixed for v1:

- environment/background radiance interval;
- precomputed static transfer summary for immutable identical foliage stack where compiler proves it;
- zero only when suffix radiance bound is zero.

**Acceptance criteria**

- Bright-tail control continues traversal despite low opacity.
- Once tail condition holds, adding more nonnegative-opacity layers cannot invalidate the bound without changed suffix radiance contract.
- Runtime never drops layers based only on layer count or transmittance scalar candidate.
- Formal tail theorems build.
- Host exact full-stack bytes equal early-out bytes.

**Gate**

```text
cargo xtask verify
cargo xtask pixels-formal
```

**Commit**

```text
pixels P10.4: terminate invisible transparent tails
```

## Task P10.5 — define deterministic probe-GI semantics

**Purpose**

Make diffuse GI a closed renderer model rather than an unbounded approximation claim.

**Files**

```text
docs/language/07-pixels.md
stdlib/core/render.wr
stdlib/core/render_probe.wr
crates/wrela-compiler/src/pixels/probe.rs
```

**Work**

V1 probe model is normative renderer semantics:

- three nested camera-centered clipmap levels;
- each level dimensions `16 × 8 × 16` probes;
- base spacing from sealed `ProbeConfig.base_spacing`; each next level spacing ×4;
- each probe stores 9 real spherical-harmonic coefficients per RGB channel for diffuse incident radiance, plus validity/version;
- coefficients stored f32 candidate plus verifier radius;
- 32 fixed unit directions per probe from a checked-in direction table with solid-angle weights summing to `4π` within stored interval;
- each direction traces one complete secondary segment to scene far/environment;
- hit contribution is outgoing diffuse/emissive approximation from a bounded one-bounce material/light query; miss contribution is environment;
- no random rotation or stochastic sequence;
- accumulation order is direction ID ascending, then channel/coefficient order;
- probe interpolation is trilinear within one level plus deterministic blend between two levels based on camera distance.

This is deterministic finite one-bounce probe GI. The renderer guarantees numeric/output correctness relative to this model, not equality to the full rendering equation.

Compiler emits direction/SH basis tables and capacities. Probe config may reduce levels/dims but cannot exceed v1 maxima; defaults above are flagship.

**Acceptance criteria**

- Direction/weight/SH tables have fixed digests and are immutable numeric-contract data.
- Probe memory exact and reported.
- No RNG/time-dependent direction phase.
- GI semantics are fully stated in docs.
- Zero-GI configuration is explicit source config, not hidden fallback.

**Gate**

```text
cargo xtask verify
```

**Commit**

```text
pixels P10.5: fix deterministic diffuse probe GI
```

## Task P10.6 — implement probe initialization

**Purpose**

Ensure the first presented frame has fully defined GI state.

**Files**

```text
stdlib/core/render_probe.wr
stdlib/core/render_actor.wr
crates/wrela-compiler/src/pixels/glue.rs
```

**Work**

Before the first frame using GI is presented:

1. place clipmaps at camera snapped to finest-level spacing;
2. update every probe in all levels in deterministic probe ID order, partitioned across workers by contiguous probe ranges;
3. trace 32 directions per probe;
4. combine worker results in probe ID order;
5. mark all probes valid for current scene/light/material dependency versions;
6. only then shade/present frame.

Initialization is finite and bounded by compiled capacities. It may span multiple actor turns/checkpoints internally but the public first render call does not return success until complete. Cancellation returns frame input and leaves probes invalid.

**Acceptance criteria**

- First GI frame is independent of uninitialized memory/previous runs.
- Single/four-core probe coefficients and frame bytes identical.
- All probe writes owned/disjoint.
- Initialization interruption cannot mark partial state valid.
- Zero-level/no-GI config skips initialization exactly.

**Gate**

```text
cargo xtask verify
cargo xtask verify-milestone
```

**Commit**

```text
pixels P10.6: initialize deterministic probe state
```

## Task P10.7 — implement probe update and invalidation

**Purpose**

Keep GI state coherent with changing scene coefficients and clipmap movement.

**Files**

```text
stdlib/core/render_probe.wr
stdlib/core/render_actor.wr
crates/wrela-compiler/src/pixels/probe.rs
```

**Work**

Compiler emits dependency/swept-bound records per object/light/material. At frame start:

- compare current/previous dependency slots;
- compute conservative swept AABB for changed geometry over the frame interval;
- invalidate probes whose support radius intersects swept AABB;
- invalidate all probes influenced by changed environment/exposure only where GI semantics depend on it; exposure/post do not invalidate radiance probes;
- invalidate affected probes for light/material/emissive changes using compiler influence bounds;
- when camera clipmap snaps, remap retained cells and mark newly exposed cells invalid;
- update every invalid probe before presenting the frame.

No fixed per-frame update budget may leave stale probes in `AaaByteExact`. If invalid count exceeds capacity (which should equal all probes), internal error. Work can be large but remains correct.

**Acceptance criteria**

- Static frame updates zero probes.
- Rigid moving object invalidates exactly a conservative neighborhood.
- Camera clipmap shift retains overlapping world-coordinate probes exactly.
- Changed direct-only post setting does not invalidate probes.
- Presented frame never reads invalid probe.

**Gate**

```text
cargo xtask verify
```

**Commit**

```text
pixels P10.7: update all invalidated GI probes
```

## Task P10.8 — shade from probe SH with numeric bounds

**Purpose**

Add smooth diffuse GI to the certified structure and output verifier.

**Files**

```text
stdlib/core/render_probe.wr
stdlib/core/render_light.wr
stdlib/core/render_material.wr
crates/wrela-compiler/src/pixels/reference/probe.rs
```

**Work**

At shaded surface:

- choose level pair from world position/camera distance;
- fetch eight probes per level;
- trilinearly interpolate SH coefficients with interval radii;
- evaluate cosine-convolved SH irradiance at normal/normal cone;
- blend levels deterministically;
- multiply diffuse albedo/energy factor;
- add to HDR candidate and interval;
- use compiler/runtime importance bound to skip a probe/level only when output budget proves irrelevance.

Avoid light leaks with deterministic visibility weight:

- each probe also stores six axis distance moments from the 32 rays;
- compare surface-to-probe vector/distance against directional mean/min-distance interval;
- downweight probes whose recorded occluder lies in front using a fixed smooth function;
- clamp weights and renormalize; if all zero, GI is zero for that sample.

This leak-reduction function is part of renderer semantics and documented.

**Acceptance criteria**

- Open diffuse environment produces expected smooth irradiance.
- Wall-separated control reduces leaks relative to unweighted interpolation and matches normative host model exactly.
- Probe candidate/interval contains host scalar evaluation.
- No invalid probe read.
- Summary/packet shading includes GI without changing byte proof rules.

**Gate**

```text
cargo xtask verify
cargo xtask pixels-conformance
```

**Commit**

```text
pixels P10.8: shade deterministic bounded probe GI
```

## Task P10.9 — integrate transparency and GI into final frame path

**Purpose**

Complete the full v1 lighting/compositing stack.

**Files**

```text
stdlib/core/render_actor.wr
stdlib/core/render_raster.wr
stdlib/core/render_transfer.wr
stdlib/core/render_probe.wr
tests/golden/boot-pixels-transparent/
tests/golden/boot-pixels-gi/
```

**Work**

- initialize/update probes before worker shading jobs;
- shade every visible/transparent layer with direct+AO+GI+emissive;
- compose transfer tree front-to-back over environment;
- apply coverage at event pixels correctly per layer/side;
- run tail termination/refinement;
- run filmic/transfer singleton proof;
- write final bytes/present.

Debug visibility path continues to bypass shading/GI/transparency for conformance.

**Acceptance criteria**

- Transparent stack and GI fixtures produce final exact digests.
- Full-stack host reference and guest bytes agree.
- No stale probe/unfinished transfer tree can be presented.
- Opaque fixtures remain byte-stable unless the intentionally added GI config changes expected output.
- Failure preserves prior front buffer.

**Gate**

```text
cargo xtask verify
cargo xtask pixels-conformance
cargo xtask verify-milestone
```

**Commit**

```text
pixels P10.9: complete transparent GI frame composition
```

## Task P10.10 — add full lighting/transparency conformance sequences

**Purpose**

Lock the remaining AAA output classes before temporal maintenance.

**Files**

```text
crates/xtask/src/pixels_conformance.rs
tests/pixels_truth/transparent/
tests/pixels_truth/gi/
tests/golden/boot-pixels-quality/
```

**Work**

Add sequences:

- 1, 2, 8, and max transparent layers;
- bright emissive tail behind low-opacity layers;
- transparent depth swap;
- transparent silhouette coverage;
- static GI room;
- moving emissive object/probe invalidation;
- camera clipmap shift;
- thin wall leak control;
- area light + transparency interaction.

Compare exact normative host model, intervals, and final bytes. Physical/path-traced comparison may be a visual developer aid but is not a gate or source of expected bytes.

**Acceptance criteria**

- Zero normative model divergence.
- Tail early-out exact final bytes.
- Probe invalidation/remap exact.
- Repeated identical runs deterministic.
- Single/four-core outputs identical.

**Gate**

```text
cargo xtask pixels-conformance
cargo xtask verify-milestone
```

**Commit**

```text
pixels P10.10: lock transparent and GI conformance
```

### Milestone P10 close

Run `cargo xtask verify-milestone`. The renderer is now visually and semantically complete from scratch. Temporal work begins only after this point and must prove equivalence to rebuilding the same normative frame state.

---

# Milestone P11 — kinetic proof maintenance, static reuse, and validated shading transport

Milestone result: the renderer reuses certified structure and shading between frames when explicit event/margin proofs remain valid. Camera cuts, out-of-rate changes, singular events, or uneconomic repair invoke bounded tile/full from-scratch sweep. A compile/test switch disabling all kinetic paths produces identical success/failure and displayed bytes.

## Task P11.1 — implement complete frame dependency digests

**Purpose**

Make exact static-frame reuse and invalidation depend on every output-affecting input.

**Files**

```text
stdlib/core/render_actor.wr
stdlib/core/render_probe.wr
crates/wrela-compiler/src/pixels/params.rs
crates/wrela-compiler/src/pixels/glue.rs
formal/pixels/Pixels/Kinetic.lean
```

**Work**

Generate fixed-order digests/subdigests:

```wrela
struct FrameDeps:
    geometry: Digest256
    camera: Digest256
    material: Digest256
    lights: Digest256
    probes: Digest256
    exposure_post: Digest256
    output_mode: Digest256
    all: Digest256
```

Use the repository’s sealed SHA-256 implementation over canonical packed bytes. Include:

- every used parameter slot by `ParamUse`;
- camera basis/eye/projection;
- all lights;
- texture/table IDs;
- probe state version and clipmap placement;
- exposure/tone/transfer IDs;
- output size/format;
- renderer/frame-program digest;
- deterministic frame/shading phase where it affects output.

Do not include unused source struct bytes or uninitialized padding.

Static reuse is allowed only when `all` equals previous successfully presented frame’s digest. Reuse submits/presents the existing front generation according to display semantics or returns success without rewriting it if machine contract permits repeated scanout.

**Acceptance criteria**

- Changing each dependency class changes `all` and expected subdigest only.
- Changing unused P field changes none.
- Failed frame does not update previous-presented digest.
- Static repeated frames perform zero sweep/shading/probe writes and preserve exact visible digest.
- Formal dependency equality theorem is instantiated/documented.

**Gate**

```text
cargo xtask verify
cargo xtask pixels-formal
```

**Commit**

```text
pixels P11.1: reuse exactly unchanged frames
```

## Task P11.2 — compile and evaluate temporal derivative programs

**Purpose**

Transport roots/events/shading with conservative first/second-order bounds.

**Files**

```text
crates/wrela-compiler/src/pixels/derivatives.rs
crates/wrela-compiler/src/pixels/program.rs
stdlib/core/render_sweep.wr
stdlib/core/render_events.wr
formal/pixels/Pixels/Kinetic.lean
```

**Work**

Compiler emits per feature/event/material/light summary:

- derivative with respect to every influencing parameter slot;
- second derivative/mixed interaction bounds needed for one-frame delta;
- program to combine actual parameter deltas into candidate time derivative;
- interval remainder using declared `max_delta`/`max_second_delta` and actual deltas;
- validity domain.

Runtime computes:

```text
q_t = -G_t/G_q
q_next ∈ q + q_t*dt + remainder
```

for regular sheets. Event curve x/y transport uses implicit derivative of its event predicate. Shading summary transport uses compiled parameter/time derivatives and residual.

Actual frame delta is normalized to one presentation interval. Skipped/late frames scale bounds with checked integer/rational dt; beyond compiler supported temporal box, invalidate and rebuild.

**Acceptance criteria**

- Zero deltas produce exact zero transport/remainder where expressions static.
- Derivative programs use only influencing slots.
- Transport intervals contain from-scratch next-frame root/event/shading truth on deterministic sequences.
- `G_q` containing zero invalidates transport.
- Formal implicit-flow/remainder theorems build.

**Gate**

```text
cargo xtask verify
cargo xtask pixels-formal
```

**Commit**

```text
pixels P11.2: emit bounded temporal transport programs
```

## Task P11.3 — define persistent kinetic state

**Purpose**

Store only the proof state needed to validate/repair the next frame.

**Files**

```text
stdlib/core/render_sweep.wr
stdlib/core/render_events.wr
stdlib/core/render_actor.wr
crates/wrela-compiler/src/pixels/capacities.rs
```

**Work**

Per tile store:

```wrela
struct KineticTileState:
    valid: bool
    source_frame: u64
    geometry_digest: Digest256
    camera_digest: Digest256
    run_count: u16
    event_count: u16
    braid_count: u16
    min_slack: Iv32
    min_slack_owner: ProofMarginKind
    next_event_dt: Iv32
    # fixed arrays of transported run/event/braid summaries
```

Do not persist root-isolation stacks, transient rebuild queues, or candidate sample caches. Persistent records are exact snapshots of certified runs/events/layers plus transport derivatives/margins.

Use two kinetic generations: last presented and next candidate. Only swap after successful presentation, parallel to framebuffers/probes. A failed frame leaves prior kinetic state valid.

**Acceptance criteria**

- State bytes derive/report exactly.
- Reset/invalidate cannot expose stale next-generation counts.
- Successful frame atomically commits framebuffer, probes, dependency digest, and kinetic state.
- Failed frame commits none.
- Static-frame path can use prior state without mutation.

**Gate**

```text
cargo xtask verify
```

**Commit**

```text
pixels P11.3: persist certified frame structure safely
```

## Task P11.4 — implement compressed slack validation

**Purpose**

Reject or retain previous proofs with a small integer common-path predicate.

**Files**

```text
stdlib/core/render_events.wr
stdlib/core/render_sweep.wr
crates/wrela-compiler/src/pixels/reference/kinetic.rs
formal/pixels/Pixels/Kinetic.lean
```

**Work**

For each persistent run/event/braid, retain component margins in debug/diagnostic builds and the minimum margin plus owner in production record. Compiler-generated transport program supplies conservative perturbation contributions:

```text
space + parameter + time + camera + light/material + quantization
```

A compressed record survives only when the upper perturbation is strictly less than the lower stored margin. Equality fails closed.

For a record whose minimum owner changes after transport, recompute all component margins before compressing next generation. Do not subtract perturbation repeatedly from one stale scalar across many frames; revalidate against current equations at least every generated maximum macroframe length, and immediately when any dependency changed.

Set v1 maximum kinetic carry length to 8 presented frames. On the 9th, revalidate/rebuild even if slack remains. This fixed bound limits accumulated arithmetic/remainder and avoids indefinite proof aging.

**Acceptance criteria**

- Compressed predicate matches full component check on all vectors.
- Equality/overflow invalidates.
- No record persists more than 8 frames without fresh verification.
- Static digest-equal frame reuse is separate and may persist indefinitely because inputs are equal.
- Formal margin theorem builds.

**Gate**

```text
cargo xtask verify
cargo xtask pixels-formal
```

**Commit**

```text
pixels P11.4: validate compressed kinetic proof slack
```

## Task P11.5 — schedule possible event failures conservatively

**Purpose**

Know which local predicates need re-evaluation without polling every event every frame.

**Files**

```text
stdlib/core/render_events.wr
crates/wrela-compiler/src/pixels/reference/kinetic.rs
```

**Work**

For each event predicate with current signed interval value `V`, first derivative interval `D`, and second-order remainder rate `R`, compute a conservative earliest possible zero time over `dt >= 0`. Use this fixed hierarchy:

1. if `V` contains zero: due now;
2. if derivative/remainder cannot move toward zero: infinity within macroframe;
3. linear lower bound `distance_to_zero / max_toward_speed` gives candidate;
4. second-order bound solves conservative quadratic inequality using outward dyadic arithmetic;
5. round time down to presentation-frame ticks;
6. cap at macroframe length 8.

Store events in a fixed binary min-heap keyed by due frame, tile ID, event ID. Heap capacity is exact emitted event count. Update changed event keys through deterministic rebuild of the tile heap slice rather than pointer mutation complexity.

At each frame, only due events plus records invalidated by dependency digests are fully re-evaluated. Nondue certificate remains valid by bound.

**Acceptance criteria**

- Predicted due time never exceeds actual first sign-zero in deterministic from-scratch comparisons.
- Zero-rate events schedule infinity/static.
- Heap order deterministic.
- No missed event when frame skips multiple ticks.
- Numeric failure schedules due now, never infinity.

**Gate**

```text
cargo xtask verify
```

**Commit**

```text
pixels P11.5: schedule conservative event expiry
```

## Task P11.6 — transport and revalidate run/event geometry

**Purpose**

Update a tile without reconstructing it when all local structure remains certified.

**Files**

```text
stdlib/core/render_sweep.wr
stdlib/core/render_events.wr
stdlib/core/render_actor.wr
```

**Work**

For each tile whose dependencies changed but state may survive:

1. transport event curves/endpoints with derivative/remainder;
2. transport each sheet q/q derivatives;
3. evaluate compressed slack;
4. rerun cheap root/feature/order predicates at transported anchors;
5. rebuild fixed-q setup and coverage records for current coordinates;
6. refresh output/shading summaries or mark them for P11.10 transport;
7. write candidate state into next generation.

A transported run may shrink/expand only within neighboring event corridor bounds and tile domain. If event order changes, endpoints overlap, or any proof fails, mark affected tile/domain for repair. Transport cannot invent/delete runs.

**Acceptance criteria**

- Transported static/slow sequences produce same visibility runs/bytes as from-scratch mode.
- Run domains remain exact partition after transport.
- No event crossing occurs in a retained regular run.
- Transported records receive fresh current-frame margins before commit.
- Failure marks repair, not stale reuse.

**Gate**

```text
cargo xtask verify
cargo xtask pixels-conformance
```

**Commit**

```text
pixels P11.6: transport certified frame structure
```

## Task P11.7 — maintain adjacent q-order braids

**Purpose**

Preserve complete layer/root order with O(adjacent relations) certificates.

**Files**

```text
stdlib/core/render_events.wr
stdlib/core/render_transfer.wr
crates/wrela-compiler/src/pixels/reference/kinetic.rs
formal/pixels/Pixels/QOrder.lean
```

**Work**

For each regular run store front-to-back sheet/layer IDs and adjacent q-order slack. On transport:

- evaluate all adjacent relations in packets where possible;
- if all strict, complete order survives by theorem;
- if one relation fails and exactly one `DepthSwap` event is due in the same domain, isolate swap and use P11.8 handler;
- if multiple/nonadjacent relations fail, rebuild affected domain;
- transparent transfer-tree leaf order follows the braid.

Do not monitor all pairwise relations. Do not assume a failed relation means an actual swap; it means local proof expired.

**Acceptance criteria**

- Adjacent checks imply same total order as from-scratch root sorting.
- Close-depth/depth-swap sequence repairs or rebuilds without wrong frame.
- Transparent tree order always matches braid/current from-scratch order.
- Packet/scalar failure counts agree.
- Formal braid theorems build.

**Gate**

```text
cargo xtask verify
cargo xtask pixels-formal
```

**Commit**

```text
pixels P11.7: maintain kinetic occlusion braids
```

## Task P11.8 — implement the limited local surgery set

**Purpose**

Handle simple certified combinatorial events directly and rebuild everything else.

**Files**

```text
stdlib/core/render_events.wr
stdlib/core/render_sweep.wr
crates/wrela-compiler/src/pixels/program.rs
```

**Work**

V1 surgery handlers are deliberately limited:

1. **Adjacent depth swap**
   - preconditions: exactly two regular sheets, one isolated transverse `q_a-q_b=0`, no silhouette/feature/CSG/material event in corridor, derivative excludes zero;
   - action: split at event curve, swap adjacent order on certified side, rebuild visible/transfer summaries locally.

2. **Material predicate side change**
   - preconditions: isolated transverse material event, geometry/run unchanged;
   - action: split/update material identity/summary.

3. **Repeat/feature boundary crossing without root birth**
   - preconditions: one feature-validity handoff between two representations of the same geometric surface, q/root intervals overlap and identity/orientation agree;
   - action: replace feature ID and recertify.

Everything else—including silhouettes/root birth/death, tangencies, smooth-band cluster topology changes, coincident events, CSG occupancy topology changes, camera crossing a surface, and any nongeneric event—invokes bounded local from-scratch sweep over compiler-emitted affected tile/domain closure.

No swallowtail/cusp/Puiseux runtime handler is required in v1. The compiler may emit local Taylor models to aid rebuild, but correctness comes from the sweep.

**Acceptance criteria**

- Each handler checks every precondition at runtime.
- Failing one precondition rebuilds; no partial surgery.
- Handler output equals from-scratch result on event sequences.
- Simultaneous-event fixture always rebuilds.
- Surgery counters are diagnostic only.

**Gate**

```text
cargo xtask verify
cargo xtask pixels-conformance
```

**Commit**

```text
pixels P11.8: repair only certified simple events
```

## Task P11.9 — choose local repair versus full sweep by sealed cost bounds

**Purpose**

Avoid heuristic storm thresholds and guarantee an upper-bounded recovery path.

**Files**

```text
stdlib/core/render_actor.wr
crates/wrela-compiler/src/pixels/cost_bounds.rs
crates/wrela-compiler/src/pixels/program.rs
```

**Work**

Compiler emits conservative static work weights for:

- full tile sweep;
- each local rebuild domain class;
- transport/revalidation;
- surgery handler;
- full frame sweep.

At frame start, after identifying invalid/expired tiles, compute checked sum of local worst-case weights. Choose local repair only if:

```text
transport_weight + local_repair_weight < full_sweep_weight
```

Equality chooses full sweep. A camera cut, output-mode change, out-of-range temporal delta, invalid previous state, or changed frame-program digest chooses full sweep immediately.

Weights are versioned structural operation counts, not claimed hardware cycles. They exist only to choose between two semantically equivalent paths deterministically.

**Acceptance criteria**

- Choice deterministic and input-derived.
- Full sweep path always available for valid input.
- A synthetic surgery storm chooses full sweep.
- Changing weights changes build/numeric revision and report.
- Both paths produce identical output/error.

**Gate**

```text
cargo xtask verify
```

**Commit**

```text
pixels P11.9: choose rebuilds from sealed work bounds
```

## Task P11.10 — transport shading between `shade_hz` frames

**Purpose**

Allow 30 Hz expensive shading with 60 Hz presentation only when the exact displayed bytes remain certified.

**Files**

```text
stdlib/core/render_material.wr
stdlib/core/render_light.wr
stdlib/core/render_actor.wr
formal/pixels/Pixels/Kinetic.lean
```

**Work**

A frame with `frame_index % (refresh_hz/shade_hz) == 0` is a base shade frame and computes current shading normally.

Intermediate frame:

1. update/repair visibility structure at full refresh rate;
2. transport material/light/shadow/AO/GI summary candidates using compiled time/parameter derivatives and remainder;
3. update event coverage and disocclusions from current geometry;
4. evaluate complete HDR interval for transported shading;
5. if final channel bytes singleton, use transported candidate;
6. otherwise shade that run/pixel/current frame exactly now.

There is no optical-flow image warp and no reuse of a previous color at newly visible geometry. Transport follows the certified current sheet/material identity.

GI probes still update according to P10 before shading/transport; a changed probe contribution invalidates/expands shading transport.

**Acceptance criteria**

- `shade_hz=refresh_hz` equals ordinary path.
- Intermediate transported frames equal from-scratch fully shaded bytes on conformance sequences.
- Disoccluded pixels never sample old background/foreground color.
- Failed byte proof triggers current shading, not approximate output.
- Formal transport/slack theorem applies.

**Gate**

```text
cargo xtask verify
cargo xtask pixels-conformance
cargo xtask pixels-formal
```

**Commit**

```text
pixels P11.10: transport shading only under byte proof
```

## Task P11.11 — implement deterministic crisp temporal policy

**Purpose**

Finish temporal presentation without TAA ghosting or stochastic jitter.

**Files**

```text
stdlib/core/render.wr
stdlib/core/render_actor.wr
docs/language/07-pixels.md
```

**Work**

V1 flagship policy is `TemporalPolicy.Crisp`:

- no camera jitter;
- no history color blend;
- no temporal antialiasing;
- no stochastic dither phase;
- geometry/event coverage evaluated at current presentation time;
- shading may transport only under P11.10 byte proof;
- output is one exact current-time frame.

Do not implement motion blur in v1. The analytic coverage and filtered material/detail paths supply spatial antialiasing; 60 Hz current-time geometry supplies motion clarity.

`RenderProfile.AaaByteExact` fixes this policy. A future motion-blur profile requires a new numeric/profile revision and separate temporal integration theorem.

**Acceptance criteria**

- Source cannot enable hidden TAA/jitter in profile v1.
- Repeated static frames exact.
- Moving edge sequences show no history ghosts because no history blend exists.
- Documentation states the chosen temporal aesthetic.
- Replay captures exact current-time inputs/frame index.

**Gate**

```text
cargo xtask verify
```

**Commit**

```text
pixels P11.11: seal the crisp temporal profile
```

## Task P11.12 — add kinetic-disable equivalence mode

**Purpose**

Permanently prove temporal maintenance is an optimization only.

**Files**

```text
stdlib/core/render_actor.wr
crates/wrela-compiler/src/pixels/glue.rs
crates/xtask/src/pixels_conformance.rs
```

**Work**

Add a compiler-internal generated constant, exposed only to test builds:

```text
KINETIC_MODE = Disabled | Enabled
```

Disabled forces:

- no static framebuffer reuse except optional identical-input present semantics? For strict comparison, force full sweep+shade every frame;
- no row/frame proposals from previous frame;
- no transport/surgery/braid reuse;
- full probe update semantics remain identical but can still retain normative probe state because GI model itself is temporal state; add a separate `PROBE_REBUILD_ALL` conformance mode when comparing probe implementation.

Conformance renders every temporal sequence both modes and compares per-frame success/error and visible bytes.

**Acceptance criteria**

- All sequences byte-identical enabled/disabled.
- Any mismatch prints first frame/tile/pixel/channel and both path diagnostics.
- Kinetic-disabled setting is not author-facing and does not ship as a runtime branch in release image; build specialization removes it.
- Error outcomes match exactly.

**Gate**

```text
cargo xtask pixels-conformance --kinetic-diff
cargo xtask verify-milestone
```

**Commit**

```text
pixels P11.12: prove kinetic maintenance byte-equivalent
```

## Task P11.13 — lock temporal event and camera-cut sequences

**Purpose**

Cover every maintenance/rebuild class permanently.

**Files**

```text
crates/xtask/src/pixels_conformance.rs
tests/pixels_truth/kinetic/
tests/golden/boot-pixels-kinetic/
```

**Work**

Sequences:

- static 120 frames;
- rigid translation;
- smooth sword swing;
- adjacent depth swap;
- material threshold crossing;
- repeat feature handoff;
- silhouette birth/death forcing local rebuild;
- simultaneous event forcing rebuild;
- camera translation/orbit;
- 15.75°/frame whip;
- camera cut;
- in-range then out-of-rate delta;
- shading transport with disocclusion;
- transparent order swap;
- probe invalidation/clipmap shift.

For each compare enabled/disabled, one/four core, record/replay, and expected final digests.

**Acceptance criteria**

- Zero byte/error divergence.
- No stale-state use after cut/failure.
- Surgery used only in its three certified classes.
- Full sweep selected when sealed work bound says so.
- All successful frames have zero unresolved output.

**Gate**

```text
cargo xtask pixels-conformance --all-temporal
cargo xtask verify-milestone
```

**Commit**

```text
pixels P11.13: lock kinetic and cut conformance
```

### Milestone P11 close

Run `cargo xtask verify-milestone`. The maintained-frame architecture is complete only when it is byte-equivalent to rebuilding every frame and all complex events safely route to local/full validated sweep.

---

# Milestone P12 — generated coefficient programs, SIMD/backend closure, and compiler cost admission

Milestone result: renderer hot paths execute through real Wrela/AArch64 code with no modeled packet fiction. The compiler evaluates coefficients once per frame, emits only the used primitive/material kernel palette, proves vector/scalar equivalence, reports actual assembly/register/slot traffic, and applies the existing A76 cost model to the sealed renderer workload.

## Task P12.1 — generate one per-frame coefficient evaluator

**Purpose**

Move arbitrary parameter expression work out of root/event/pixel loops.

**Files**

```text
crates/wrela-compiler/src/pixels/glue.rs
crates/wrela-compiler/src/pixels/scalar.rs
stdlib/core/render_actor.wr
stdlib/core/render_program.wr
```

**Work**

Generate per renderer:

```wrela
fn __wrela_renderer_0_eval_coefficients(
    read params: ParamSnapshotR0,
    mut out: CoeffSnapshotR0,
) -> Result[unit, RenderError]
```

The body evaluates canonical scalar DAG nodes in topological ID order exactly once per frame:

- f32 candidate value;
- `Iv32` verifier interval over current input conversion/range radius;
- optional first/second temporal derivative values/intervals;
- validity flags for denominators/normalize domains;
- precomputed camera/projective coefficients;
- light/material scalar coefficients.

Generated source uses named locals for short-lived chains and writes only coefficients referenced by runtime records. Constant/zero-rate nodes are placed immutable in frame program and omitted from per-frame evaluator.

Coordinator computes coefficients once and sends/copies the fixed snapshot to workers. Workers do not interpret scalar DAGs independently.

**Acceptance criteria**

- Coefficient snapshot bytes/count match report.
- Candidate/interval values agree with Rust symbolic reference on parameter-corner/random vectors.
- Static coefficients generate no runtime instructions.
- Common subexpressions evaluated once.
- Nonfinite/domain failure aborts before worker jobs/framebuffer writes.

**Gate**

```text
cargo xtask verify
cargo xtask diff-eval
```

**Commit**

```text
pixels P12.1: evaluate renderer coefficients once per frame
```

## Task P12.2 — generate the exact used kernel palette

**Purpose**

Avoid a whole-scene scalar tape interpreter while retaining compact structural data.

**Files**

```text
crates/wrela-compiler/src/pixels/glue.rs
crates/wrela-compiler/src/pixels/program.rs
stdlib/core/render_program.wr
stdlib/core/render_material.wr
stdlib/core/render_light.wr
```

**Work**

From frame-program kind census, generate bounded dispatch functions containing only used cases:

```wrela
fn eval_feature_value(kind: FeatureKind, ...)
fn eval_feature_derivatives(kind: FeatureKind, ...)
fn eval_event(kind: EventKind, ...)
fn eval_material(material: MaterialId, ...)
fn eval_light(light: LightKind, ...)
```

Dispatch is a Wrela `match` over dense generated enum/tag. Each case calls hand-authored standard kernel with coefficient record. No function pointers, JIT, runtime codegen, or indirect host calls.

For material identities, generate direct evaluator functions from canonical MaterialGraph or a small op program evaluated over packets. Select specialization when material node count ≤ generated threshold 64; larger accepted graph uses bounded op interpreter. Threshold is a code-size rule, fixed in numeric/profile revision, not runtime tuning.

Semantic field tape remains in frame program only for bounded local interval fallback and host differential validation. It is never the common regular-run path.

**Acceptance criteria**

- Unused primitive/material/light cases absent from generated typed/MachineWir dumps.
- Used tag always has one case; missing case internal build error.
- Specialized and op-program material evaluators agree.
- Dispatch count/bytes reported.
- No scene-wide field tape evaluation in regular run/raster/shading call graph.

**Gate**

```text
cargo xtask verify
cargo xtask diff-eval
```

**Commit**

```text
pixels P12.2: generate the used renderer kernel palette
```

## Task P12.3 — complete SIMD operation lowering required by Pixels

**Purpose**

Implement the closed 128-bit vector semantics already promised by the language/library contract.

**Files**

```text
crates/wrela-compiler/src/sema/types.rs
crates/wrela-compiler/src/sema/bodies.rs
crates/wrela-compiler/src/flowwir.rs
crates/wrela-compiler/src/mwir.rs
crates/wrela-compiler/src/lower.rs
crates/wrela-compiler/src/codegen.rs
crates/wrela-compiler/src/a64.rs
stdlib/core/simd.wr
```

**Work**

Implement only operations used by renderer plus complete contract-required peers:

- `i32x4`, `u32x4`, `f32x4` construction/splat;
- aligned/unaligned load/store from placed arrays/Bytes;
- add/sub/mul;
- f32 FMA with source-defined arithmetic sequence where bit identity requires it;
- min/max/abs;
- comparisons to mask;
- bit select/blend;
- lane extract/insert;
- widening i32×i32 to two i64x2 product halves for verifier helpers where packetized;
- conversions i32↔f32 with fixed rounding semantics;
- shuffle patterns used by transpose/SoA;
- reciprocal/rsqrt explicit Newton sequences through stdlib, not backend-only transformation.

Every operation has sema, FlowWir, MachineWir, A64 encoding, cost rule, diff-eval, and emitted-word tests. Do not add general arbitrary shuffle if fixed named shuffles suffice.

**Acceptance criteria**

- SIMD scalar-lane semantics match scalar operations.
- Compiler refuses vector operations in ISR as existing float rule requires.
- No dev/release arithmetic divergence.
- Generated code uses NEON instructions, no scalar fallback loop.
- Intrinsic/cost/emitted instruction censuses updated and exact.

**Gate**

```text
cargo xtask verify
cargo xtask diff-eval
cargo xtask verify-milestone
```

**Commit**

```text
pixels P12.3: complete the renderer NEON operation set
```

## Task P12.4 — packetize proof predicates and shading/raster kernels

**Purpose**

Use SIMD where lanes share one operation and keep divergent algorithms scalar/batched explicitly.

**Files**

```text
stdlib/core/render_interval.wr
stdlib/core/render_events.wr
stdlib/core/render_raster.wr
stdlib/core/render_material.wr
stdlib/core/render_light.wr
stdlib/core/render_transfer.wr
```

**Work**

Required packet kernels:

- four q recurrence pixels;
- four normal/position/shading pixels;
- four transfer compositions or four pixels through one layer;
- eight/suitable adjacent q certificate comparisons using i32 lanes where packed;
- four event predicate evaluations with same program;
- four interval endpoint affine/polynomial evaluations when exponents align;
- four texture taps/BRDF evaluations.

Do not force packetization across divergent root-isolation stacks. Batch independent same-kind candidate cells only when they already exist; scalar fixed-stack algorithm remains semantic implementation.

Use SoA for vectors/points/colors. Convert AoS records to SoA once at run setup, not inside pixel loop.

**Acceptance criteria**

- Every packet kernel has scalar differential test and manifest mapping.
- Packet use does not change subdivision/root/event decisions.
- No lane masking hides unresolved lane; collect any failure and process lane scalarly/rebuild.
- SoA conversion work counted/reported.
- Hot loops contain no dynamic allocation or generic trait dispatch.

**Gate**

```text
cargo xtask verify
cargo xtask diff-eval
```

**Commit**

```text
pixels P12.4: packetize uniform renderer kernels
```

## Task P12.5 — add renderer-specific codegen conventions and hot-function assertions

**Purpose**

Keep recurrence/packet state in registers and make spills/frames visible as build facts.

**Files**

```text
crates/wrela-compiler/src/convention.rs
crates/wrela-compiler/src/regalloc.rs
crates/wrela-compiler/src/report.rs
crates/wrela-compiler/src/pixels/glue.rs
```

**Work**

Mark generated hot functions through existing metadata/canonical key list, not a user-facing attribute. For each:

- prefer leaf/frameless lowering;
- reserve vector locals q/dq/ddq/normal/color across loop;
- avoid calls by inlining small fixed kernels under existing code-size budget;
- tail-call cold refinement/error paths where legal;
- report frame size, resident vector/general registers, spills, calls, loop instruction count, and slot memory references.

Do not hardcode physical registers in source or bypass allocator. Add a post-codegen assertion only for invariant-level requirements:

- raster main loop contains vector instructions;
- no call inside main pixel loop;
- no stack memory op inside fixed-q recurrence loop after setup;
- loop body below a fixed instruction ceiling documented in cost table.

If allocator cannot meet an assertion, fix live range/code shape; do not delete assertion or claim register residence.

**Acceptance criteria**

- Report names all generated hot functions and assembly facts.
- Fixed-q loop is frameless/call-free and q state register resident.
- Shading loop spill count is explicit; no false claim.
- Assertions inspect decoded MachineWir/emitted instructions robustly, not text substring alone where possible.
- Existing convention tests remain green.

**Gate**

```text
cargo xtask verify
cargo xtask verify-milestone
```

**Commit**

```text
pixels P12.5: lock renderer hot-loop code shape
```

## Task P12.6 — add Pixels cost dimensions and instruction weights

**Purpose**

Score real emitted renderer code and memory traffic through the existing A76 model.

**Files**

```text
crates/wrela-compiler/src/cost/rule.rs
crates/wrela-compiler/src/cost/oracles.rs
bench/a76-pi5.toml
bench/thresholds.toml
tests/census.toml
```

**Work**

Add/split cost dimensions needed by actual emitted instructions:

- ASIMD f32 add/mul/FMA/min/max;
- ASIMD i32 add/compare/select;
- widening multiply;
- scalar/vector reciprocal/rsqrt Newton sequences as their actual words;
- square root if still emitted;
- table/texture loads;
- vector stores;
- branch/match dispatch;
- L1 load/store bytes and store-data/V-pipe contention dimensions already modeled by existing framework;
- cache-line/framebuffer write traffic;
- display descriptor/doorbell.

Source weights from existing A76/SOG inventory discipline. Where exact throughput row is unavailable, use an explicit conservative range/sweep dimension and state what future hardware counter narrows it; do not choose optimistic endpoint for admission.

Update dense row inventory and rule census. Every emitted renderer word must map to at least one cost dimension.

**Acceptance criteria**

- Cost dimension inventory remains dense and fully claimed.
- No renderer instruction has unknown/zero accidental cost.
- Vector and scalar paths score actual emitted words, not FLOP estimates.
- Memory refs classify stack/static/frameprog/pixelsdata/framebuffer/probe distinctly where model supports.
- Conservative endpoint used for build admission.

**Gate**

```text
cargo xtask verify
cargo xtask verify-milestone
```

**Commit**

```text
pixels P12.6: cost the emitted A76 renderer instructions
```

## Task P12.7 — attach sealed renderer workloads to generated functions

**Purpose**

Turn frame-program capacities/structure into exact bounded loop counts for cost and deadlines.

**Files**

```text
crates/wrela-compiler/src/pixels/workload.rs
crates/wrela-compiler/src/cost/workload.rs
crates/wrela-compiler/src/report.rs
```

**Work**

Generate workload facts per renderer/config:

```text
full_sweep:
  tiles
  rows_per_tile
  feature candidates per tile/row upper bounds
  root/event subdivisions
  runs/event pixels
  material/light/texture operations
  area-light cells
  AO taps
  probe updates (initial/all-invalid and static-frame variants)
  transparency layers/tree nodes
  output pixels/packets

kinetic_frame:
  transport records
  due events
  local rebuild closure
  shading transport/refinement
```

For release admission, use the worst valid presented-frame path excluding first-frame probe initialization if the application can perform initialization before entering steady display deadline; report/init deadline separately. Camera cut/full sweep is included. The absolute pathological `CertificateExhausted` path may return error before completing a frame and is reported as failure bound, not admitted success.

Attach counts to exact generated function keys/loops. Do not multiply unrelated averages. Per-core work uses actual tile/probe partition maxima.

**Acceptance criteria**

- Workload report traces every count to frame-program table/capacity.
- Full sweep includes no kinetic discount.
- Single/four-core partition sums and max-core values exact.
- Generated hot function missing workload is build error.
- Report separates initialization, successful full sweep, kinetic valid, and failure/rebuild upper bounds.

**Gate**

```text
cargo xtask verify
```

**Commit**

```text
pixels P12.7: attach sealed frame workloads to cost
```

## Task P12.8 — enforce renderer deadline and memory admission

**Purpose**

Make the declared 1080p60 profile a compiler proof obligation, not a runtime wish.

**Files**

```text
crates/wrela-compiler/src/pixels/admission.rs
crates/wrela-compiler/src/cost/mod.rs
crates/wrela-compiler/src/report.rs
bench/thresholds.toml
```

**Work**

For `RenderProfile.AaaByteExact`:

- compute per-core conservative proxy cycles for successful full-sweep frame at declared mode/refresh;
- include scheduler/orchestration/display submission and memory traffic;
- compare against frame-period budget at configured/pinned flagship clock using conservative cost endpoint and existing core load/placement;
- reserve explicit headroom factor in `bench/thresholds.toml`; set v1 admission to at most 80% of modeled per-core frame budget, leaving 20% for model error/interrupt/display variance;
- check steady-state memory < 1 GiB profile after all image/runtime/framebuffer/probe state;
- check first-frame initialization against separate renderer initialization deadline (default 2 seconds, source-configurable only downward in flagship profile);
- refuse image with a detailed cost why-chain if any fails.

Until real Pi calibration in P13 locks model dimensions, `AaaByteExact` remains buildable only under an explicit repository-internal `pixels_unlocked` feature for implementation fixtures. P13 removes that escape hatch before activation. The escape hatch is not source syntax and cannot ship release images.

**Acceptance criteria**

- Admission uses full from-scratch path, not typical kinetic frame.
- Over-budget fixture fails with per-core term breakdown.
- Memory/init-deadline fixtures fail correctly.
- Report prints budget, modeled range, conservative endpoint, headroom, and provenance.
- No threshold auto-adjust/update command in ordinary build.

**Gate**

```text
cargo xtask verify
```

**Commit**

```text
pixels P12.8: admit renderer images against full-sweep budgets
```

## Task P12.9 — archive Cortex-A76 assembly evidence

**Purpose**

Make target code shape reviewable and pinned before hardware conformance.

**Files**

```text
crates/xtask/src/pixels_asm.rs
tests/golden/pixels-asm/
bench/pixels-a76.md
```

**Work**

Add `cargo xtask pixels-asm` that builds the locked acceptance renderer for AArch64 target and emits normalized assembly/MachineWir summaries for:

- coefficient evaluator;
- interval/q-order predicates;
- fixed-q raster loop;
- normal/BRDF packet loop;
- texture filter loop;
- transfer composition;
- event/root isolation inner loops;
- probe SH evaluation;
- display descriptor path.

Normalize addresses, local labels, and build paths while preserving instruction sequence, registers, stack offsets, and branch structure. Check in compact summaries plus full assembly artifact under a generated ignored/artifact path if repository policy permits; golden only load-bearing loops.

**Acceptance criteria**

- AArch64 target code actually builds.
- Golden asserts no calls/stack ops in fixed-q hot loop.
- Instruction/cost report counts agree with assembly decoder.
- No claim of Pi cycles from assembly alone.
- Changing a load/store/spill produces reviewed golden diff.

**Gate**

```text
cargo xtask pixels-asm --check
cargo xtask verify-milestone
```

**Commit**

```text
pixels P12.9: pin Cortex-A76 renderer assembly shape
```

## Task P12.10 — close backend differential and cost reports

**Purpose**

Prove optimized release code preserves all scalar/formal decisions and is fully accounted.

**Files**

```text
crates/xtask/src/pixels_conformance.rs
crates/wrela-compiler/src/report.rs
tests/golden/check-pixels-*/expected/report.txt
```

**Work**

Run complete permanent corpus under:

- scalar kernel build;
- SIMD packet build;
- kinetic disabled/enabled;
- dev/release;
- one/four core;
- record/replay.

Compare final bytes/error outcomes. Append report sections:

```text
RendererCost
  path=full-sweep
  path=kinetic-valid
  path=probe-init
  path=bounded-failure
  core=...
  proxy_cycles_low/high
  bytes_read/written by memory class
  dispatches
  spills
  headroom
```

**Acceptance criteria**

- All build variants byte/error equivalent.
- Every renderer cost row has provenance and nonzero workload where used.
- Report has no modeled hardware measurement language.
- Admission result computed from report data, not handwritten verdict.
- All permanent reports pinned.

**Gate**

```text
cargo xtask pixels-conformance --all-build-modes
cargo xtask verify-milestone
```

**Commit**

```text
pixels P12.10: lock renderer backend and cost equivalence
```

### Milestone P12 close

Run `cargo xtask verify-milestone`. The renderer must now execute as real emitted AArch64/NEON code, remain byte-equivalent across all optimized/scalar modes, and pass conservative compiler cost/memory admission under the temporary implementation unlock.

---

# Milestone P13 — hardware conformance, model lock, language activation, and release closure

Milestone result: the temporary Pixels implementation unlock is removed. The compiler’s conservative A76 model is calibrated against the exact emitted acceptance image, the Pi 5 acceptance workloads meet the hard deadline/thermal/determinism gates, all formal and conformance gates are green, and `07-pixels.md` is active normative language/machine behavior.

## Task P13.1 — create the Wrela acceptance images

**Purpose**

Exercise the complete production renderer with fixed authored scenes rather than fieldprobe.

**Files**

```text
examples/pixels_colonnade/
examples/pixels_melee/
examples/pixels_quality/
examples/pixels_acceptance/
tests/golden/check-pixels-acceptance/
bench/pixels-acceptance.toml
```

**Work**

Port/build in Wrela:

- `colonnade-flat`: planes, round boxes, cylinders, repetition, hard CSG, material identities;
- `colonnade`: same plus bounded two-octave displacement;
- `melee`: four smooth-min figures, blade, torus, ground, rigid/parameterized sword motion;
- `quality`: textures, glossy porcelain, clay, rectangle/disk lights, transparency, GI room.

Acceptance image includes deterministic camera/animation scripts:

```text
full_sweep_colonnade_flat: kinetic disabled, camera cut/pose changes every frame
full_sweep_colonnade: kinetic disabled, displacement and camera motion
full_sweep_melee: kinetic disabled, sword motion and camera orbit
kinetic_melee: enabled swing/orbit
quality_sequence: lights, transparency, GI, subpixel motion
static_sequence: exact static reuse
cut_sequence: repeated hard cuts
```

All scripts use exact frame-indexed coefficients, no wall clock or input device. Expected frame digests are checked in for selected frames and rolling sequence digest.

`bench/pixels-acceptance.toml` is not a tunable benchmark file; it pins mode, frame count, scene path, expected digest, and hard conformance thresholds.

**Acceptance criteria**

- Images compile with implementation unlock and pass host/VMM conformance.
- Scenes use ordinary public `@field`/`@material`/`Image.renderer` source.
- No fieldprobe crate/source imported.
- Frame scripts deterministic under replay.
- Acceptance report contains complete capacities/costs.

**Gate**

```text
cargo xtask verify
cargo xtask pixels-conformance --acceptance
```

**Commit**

```text
pixels P13.1: add production renderer acceptance images
```

## Task P13.2 — add Pi 5 conformance runner and provenance

**Purpose**

Collect reproducible deadline/cycle/thermal evidence from the actual target without relabeling model output.

**Files**

```text
crates/xtask/src/pixels_pi.rs
bench/pixels-acceptance.toml
bench/pixels-pi5-lock.toml
docs/hardware/pixels-pi5.md
```

**Work**

Add `cargo xtask pixels-pi-conformance` runnable only on the configured Pi host. It records:

- board model/revision and RAM size;
- CPU model/core count;
- kernel/VMM/image commit/digests;
- governor/min/max/current frequency;
- thermal/throttling flags before/after;
- display mode/refresh/format;
- per-frame present sequence/vsync misses;
- cycles, instructions, branches, branch misses, L1D/LLC refs/misses, context switches where supported;
- guest/VMM memory bytes/RSS;
- temperature/frequency time series at fixed one-second cadence;
- frame/sequence digests.

Required fixture hardware for flagship lock:

```text
Raspberry Pi 5 1 GiB
stock 2.4 GHz maximum, no overclock
64-bit Linux/KVM host supported by Wrela VMM
official/adequate 27 W power supply
active cooler
1920×1080 60 Hz display path
performance governor during conformance
```

If counters are unavailable, the run is incomplete and cannot lock the model. The tool writes an untrusted raw report and a normalized candidate lock; it never updates the checked-in lock automatically.

**Acceptance criteria**

- Tool refuses nonmatching board/mode/governor/throttle preconditions.
- Model and hardware measurements are labeled separately.
- Raw and normalized reports include digests/provenance.
- Rerun with same image/script produces same frame digests.
- No network dependency during run.

**Gate**

```text
cargo xtask verify
# target-only:
cargo xtask pixels-pi-conformance --all
```

**Commit**

```text
pixels P13.2: add target hardware conformance runner
```

## Task P13.3 — calibrate and lock A76 cost dimensions

**Purpose**

Narrow conservative model ranges using actual emitted loops and Pi counters, then preserve headroom.

**Files**

```text
bench/a76-pi5.toml
bench/pixels-pi5-lock.toml
bench/thresholds.toml
crates/wrela-compiler/src/cost/oracles.rs
tests/census.toml
```

**Work**

For each generated hot loop/path, use hardware counters and emitted instruction counts to resolve:

- dispatch/branch cost;
- load/store/cache traffic;
- store-data/V-pipe contention;
- reciprocal/rsqrt/sqrt throughput;
- texture/framebuffer/probe memory cost;
- actor/VMM/display overhead.

Update cost dimensions only where the measurement isolates the term or gives a conservative bound. Preserve broad range where attribution is mixed. Document exact acceptance image/function/counter equation for each locked row.

Admission continues to use the conservative endpoint and 20% headroom. Do not set model equal to one measured run. The lock must encompass all repeated conformance runs and include measurement variance.

**Acceptance criteria**

- Every changed cost row cites target report/function.
- Modeled per-path range contains observed proxy-equivalent cycles.
- Conservative endpoint remains above observed worst repeated run.
- Admission headroom still passes acceptance images.
- Inventory/census/goldens updated deliberately.

**Gate**

```text
cargo xtask verify
cargo xtask verify-milestone
```

**Commit**

```text
pixels P13.3: lock renderer A76 cost calibration
```

## Task P13.4 — pass the hard full-sweep 1080p60 gate

**Purpose**

Prove temporal maintenance is not hiding a renderer that misses the flagship deadline after cuts/whips.

**Files**

```text
bench/pixels-pi5-lock.toml
docs/hardware/pixels-pi5.md
tests/golden/check-pixels-acceptance/expected/report.txt
```

**Work**

Run with kinetic disabled and probes initialized before timed sequence:

```text
scene colonnade-flat: 3,600 frames (60 s)
scene colonnade:      3,600 frames
scene melee:          3,600 frames
```

Each script forces valid camera/animation changes every frame and includes cuts/whip segments that require full sweep. Hard criteria:

- exactly 60 presents per second under display/vsync sequence;
- zero missed vsync/deadline failure;
- zero `RenderError`;
- exact expected rolling frame digest;
- no thermal/throttling flag;
- CPU frequency never below configured minimum due to thermal/power throttling;
- memory within 1 GiB profile and no host OOM/swap activity affecting run;
- measured maximum successful frame work inside compiler’s admitted conservative range/headroom.

Do not average away a missed frame. Any miss fails.

If this task fails, fix implementation/codegen/costly semantics while preserving all prior correctness/quality contracts. Do not enable kinetic mode, lower resolution, lower refresh, loosen output proof, or change acceptance script.

**Acceptance criteria**

- All three scene runs meet every hard criterion.
- Raw reports and normalized lock reviewed/checksummed.
- Compiler report’s full-sweep admission passes without unlock.
- Host/VMM frame digest matches prior conformance.

**Gate**

```text
cargo xtask pixels-pi-conformance --full-sweep
```

**Commit**

```text
pixels P13.4: lock full-sweep 1080p60 conformance
```

## Task P13.5 — pass the sustained AAA/thermal gate

**Purpose**

Validate complete quality, transparency, GI, kinetic maintenance, and long-run stability.

**Files**

```text
bench/pixels-pi5-lock.toml
docs/hardware/pixels-pi5.md
```

**Work**

Run 30 minutes / 108,000 presented frames with the combined deterministic acceptance script cycling:

- static reuse;
- kinetic melee motion;
- camera orbit/whip/cuts;
- textured glossy quality scene;
- area-light penumbrae;
- transparency;
- moving object/light probe invalidation;
- shade_hz transport.

Hard criteria:

- zero missed vsync;
- zero render/display/probe/capacity errors;
- exact rolling digest and selected frame digests;
- zero replay divergence;
- no throttling/power flags;
- temperature stays below the board’s throttle threshold with active cooling;
- no unbounded memory growth; RSS/guest memory returns within fixed envelope after cycles;
- all hardware counters/clock samples present.

Repeat the complete run three times after cold reboot. All frame digests identical; counter/temperature variation remains inside documented lock envelope.

**Acceptance criteria**

- Three green sustained runs.
- No performance/quality fallback mode activated.
- Kinetic-disabled selected short comparison sequence remains byte-identical.
- Locked report includes worst frame, not only average/p95.

**Gate**

```text
cargo xtask pixels-pi-conformance --sustained --repeat 3
```

**Commit**

```text
pixels P13.5: lock sustained AAA renderer conformance
```

## Task P13.6 — remove the implementation unlock and activate profile admission

**Purpose**

Prevent uncalibrated/over-budget flagship renderer images from shipping.

**Files**

```text
crates/wrela-compiler/src/pixels/admission.rs
crates/wrela-compiler/src/bin/wrela.rs
crates/wrela-compiler/Cargo.toml
bench/thresholds.toml
tests/golden/err-pixels-cost/
```

**Work**

- Delete `pixels_unlocked` feature/env/internal bypass.
- `RenderProfile.AaaByteExact` always runs full cost/memory/formal-revision admission.
- Test fixtures that intentionally exceed budgets use rejected goldens, not bypass.
- Development can use smaller output/configurations that pass admission; there is no “ignore cost” source flag.
- Add explicit compiler error if cost table/lock revision is missing or mismatched.

**Acceptance criteria**

- Acceptance images build normally.
- Over-budget images fail.
- Grep/census finds no unlock/bypass.
- Report verdict and raw facts agree.
- Nonrenderer builds unaffected.

**Gate**

```text
cargo xtask verify
cargo xtask verify-milestone
```

**Commit**

```text
pixels P13.6: activate sealed renderer admission
```

## Task P13.7 — run complete formal, fuzz, differential, and replay closure

**Purpose**

Produce one final green trust-chain result after hardware-driven code/model edits.

**Files**

```text
formal/pixels/EXPECTED_AXIOMS.txt
formal/pixels/KERNELS.txt
tests/golden/pixels-asm/
```

**Work**

Run and pin:

```text
cargo xtask verify
cargo xtask verify-milestone
cargo xtask pixels-formal
cargo xtask pixels-repro
cargo xtask pixels-conformance --all
cargo xtask pixels-conformance --all-build-modes
cargo xtask pixels-conformance --kinetic-diff
cargo xtask fuzz pixels --locked-corpus
cargo xtask pixels-asm --check
```

Promote every new fuzz/differential finding to a permanent focused test before fixing. Update no golden blindly.

**Acceptance criteria**

- All commands green from clean checkout/toolchains.
- No admissions/unexpected axioms.
- No semantic/output divergence.
- No frame-program decoder crash.
- Assembly/cost/report/reproduction goldens stable.

**Gate**

The command set above.

**Commit**

```text
pixels P13.7: close the renderer trust chain
```

## Task P13.8 — finalize normative specs and decision records

**Purpose**

Make repository documentation describe the shipped renderer rather than the abandoned sample-first design.

**Files**

```text
docs/language/07-pixels.md
docs/language/04-compiler.md
docs/language/05-library.md
docs/language/06-machine.md
docs/designs/pixels.md
docs/designs/pixels-spikes-plan.md
docs/hardware/pixels-pi5.md
README.md
```

**Work**

- Remove implementation-status wording from `07-pixels.md`; mark normative revision.
- State exact supported field/material/profile subset and build errors.
- Document `FrameProgram v1`, memory sections, actors, display format, replay, numeric/formal revisions.
- Document from-scratch sweep, kinetic equivalence, AAA model, and failure semantics.
- Preserve fieldprobe documents as historical evidence that rejected the old online baseline; add a clear supersession note pointing to production plan/results.
- Replace modeled/unmeasured statements only with actual locked hardware facts from P13.4–5.
- Do not claim universal rendering or full physical GI.
- Add source tutorial using public API and explain common diagnostics/cost report.

**Acceptance criteria**

- No active doc tells implementers to run a spike before FieldWir/Pixels work.
- No contradiction between source API/spec/compiler implementation.
- Hardware facts cite locked report/provenance.
- Unsupported features listed plainly.
- All doc links/goldens pass.

**Gate**

```text
cargo xtask verify
```

**Commit**

```text
pixels P13.8: activate the production Pixels specification
```

## Task P13.9 — add the release conformance command

**Purpose**

Give maintainers one command that states whether Pixels may ship.

**Files**

```text
crates/xtask/src/main.rs
crates/xtask/src/pixels_release.rs
bench/pixels-pi5-lock.toml
```

**Work**

Add:

```text
cargo xtask pixels-release-check
```

Host portion runs:

- verify-milestone;
- formal;
- reproduction;
- complete conformance/differential;
- assembly check;
- report/admission check;
- validates checked-in Pi lock report signatures/digests and commit compatibility.

On configured Pi with `--hardware`, it reruns short full-sweep and sustained-smoke subsets and validates no drift from lock. Full 30-minute repeated lock remains explicit release-candidate procedure, not every local command.

Output ends with one computed verdict:

```text
PixelsRelease revision=<...> PASS=true
```

No handwritten PASS line. Every constituent fact is printed before verdict.

**Acceptance criteria**

- Missing/stale formal/cost/hardware lock produces PASS=false.
- Command never updates locks/goldens.
- Verdict computed from result structs.
- Repository release checklist invokes it.

**Gate**

```text
cargo xtask pixels-release-check
```

**Commit**

```text
pixels P13.9: add the renderer release gate
```

## Task P13.10 — final repository cleanup and ownership census

**Purpose**

Remove transitional code and ensure every renderer surface is accounted for.

**Files**

```text
crates/wrela-compiler/src/sema/intrinsics.rs
tests/census.toml
crates/xtask/src/census.rs
Cargo.toml
AGENTS.md
```

**Work**

- Remove placeholder debug-only branches no longer needed, retaining explicit conformance modes behind generated test builds.
- Census all Pixels intrinsics, dump stages, frame-program tags, event kinds, feature kinds, material/light kinds, cost rules, generated symbol families, formal kernel mappings, and report sections.
- Fail closed when implementation grows without updating the relevant written-down list.
- Confirm no new external Cargo dependencies were added; if a prior task accidentally added one, replace it with local direct code as required by repository policy.
- Add Pixels module ownership/map to `AGENTS.md` without changing repository behavioral rules.

**Acceptance criteria**

- Censuses equal producer/consumer sites.
- No TODO/placeholder/unimplemented path reachable in supported profile.
- No implementation unlock.
- No dependency growth.
- Full release check green.

**Gate**

```text
cargo xtask pixels-release-check
cargo xtask verify-milestone
```

**Commit**

```text
pixels P13.10: close renderer ownership and cleanup
```

### Milestone P13 close

The milestone closes only after the full release command is green, the Pi lock is valid for the current renderer/numeric/cost revisions, and the temporary implementation unlock is gone.

---

## 11. Permanent correctness and conformance matrix

The following matrix is normative. A fixture may gain assertions, but no row may be deleted or merged in a way that stops testing its named failure class.

| fixture / lane | protects | first milestone active | final assertions |
|---|---|---:|---|
| `check-pixels-plane` | projective cancellation; affine inverse depth; full-row regular run | P2 | one plane object/feature; exact q; no false event; final digest |
| `check-pixels-hard-csg` | union/intersection/subtraction occupancy ordering | P3 | complete roots; exact first composite transition; identity |
| `check-pixels-smooth-csg` | support-shell completeness; active smooth cluster | P3 | leaf support; unique root runs; normal/material continuity |
| `check-pixels-repeat` | finite instances; wrap event; negative index ordering | P3 | no cross-wrap certificate; exact visible instance |
| `check-pixels-displace` | bounded deformation/Taylor remainder | P3 | no unsafe sphere step; root tube/normal/output containment |
| `check-pixels-close-depth` | overlapping q intervals/order swap | P4 | no ID tie-break; event corridor; exact side winners |
| `check-pixels-thin-feature` | subpixel structural discovery | P3 | feature retained; analytic coverage; no false background |
| `check-pixels-enclosed-feature` | sample-lattice information firewall | P3 | feature found from bound/support despite identical legacy samples |
| `check-pixels-material-edge` | nondepth discontinuity | P3 | material event, same geometry q, exact side bytes |
| `check-pixels-transparent-tail` | ordered layers/bright suffix | P10 | no premature tail cut; exact transfer bytes |
| `check-pixels-area-light` | deterministic soft shadow integration | P9 | no false-lit/false-shadowed; interval contains source integral |
| `check-pixels-kinetic` | transport/event/rebuild equivalence | P11 | enabled/disabled byte/error equivalence |
| `check-pixels-camera-inside` | initial occupancy and exit boundary | P7 | exact first exit and normal orientation |
| `check-pixels-torus-roots` | quartic multiple roots | P4 | all positive roots isolated/ordered |
| `check-pixels-tangent` | root without sign change | P6 | tangent isolated or explicit corridor, never miss |
| `check-pixels-simultaneous-event` | nongeneric event safety | P11 | surgery refused; local/full rebuild exact |
| `check-pixels-tile-boundary` | half-open ownership | P8 | no gap/double write; identical one/four core |
| `check-pixels-fixed-q-range` | integer error/overflow | P6 | reset/split or explicit failure; packet=scalar |
| `check-pixels-texture-seam` | UV wrap/filter event | P9 | bounded filtered output; stable seam ownership |
| `check-pixels-normal-moments` | subpixel specular stability | P9 | interval contains dense integration; stable sequence |
| `check-pixels-probe-wall` | GI leak reduction/validity | P10 | normative model agreement; no invalid probe read |
| `check-pixels-probe-shift` | clipmap remap/invalidation | P10 | world-coordinate retention; exact updates |
| `err-pixels-unsupported-op` | closed profile subset | P1 | `P004`, exact source/call-chain diagnostic |
| `err-pixels-missing-range` | finite proof domain | P1 | `P005` at influencing path |
| `err-pixels-missing-rate` | temporal proof domain | P1 | `P006` at changing influencing path |
| `err-pixels-topology-branch` | fixed topology | P2 | `P003`, both arm shapes shown |
| `err-pixels-repeat-unbounded` | finite instance count | P3 | `P012`, world/period contributors |
| `err-pixels-capacity` | no runtime allocation/overflow | P3 | `P015`, exact why-chain |
| `err-pixels-projective-zero` | positive denominator/q | P4 | `P016` |
| `err-pixels-fixed-q` | representable hot state | P6 | `P017` |
| `err-pixels-tone-table` | monotone byte proof | P9 | `P018` |
| `err-pixels-cost` | full-sweep deadline admission | P12 | modeled range/headroom why-chain |
| `boot-pixels-numeric` | Lean/Rust/Wrela scalar correspondence | P6 | exact vector digest |
| `boot-pixels-plane` | full guest/VMM path | P8 | exact visible/raw tile/frame/replay digests |
| `boot-pixels-quality` | complete AAA stack | P9 | selected frame and rolling sequence digests |
| `boot-pixels-transparent` | transfer tree/tail | P10 | normative host model and guest digest |
| `boot-pixels-gi` | probe init/update/interpolation | P10 | normative host model and guest digest |
| `boot-pixels-kinetic` | temporal maintenance | P11 | all mode/core/replay comparisons |
| `pixels-asm` | A76 hot-loop shape | P12 | instructions/registers/stack/calls/cost counts |
| `pixels-pi5-lock` | real target deadline/thermal | P13 | hard zero-miss/digest/thermal/counter gates |

### 11.1 Oracle separation

There are four distinct references and they must never be confused:

1. **Source semantics** — scalar `FieldGraph`/`MaterialGraph` meaning.
2. **Independent host oracle** — f64/interval all-root and normative shading implementation used only after rendering.
3. **Compiler proof objects** — ranges, support, events, exclusions, capacities included in `FrameProgram`.
4. **Guest verifier/runtime** — consumes only frame program/current inputs and produces bytes/errors.

A test may compare 2 against 4. The renderer may not read 2. The compiler may derive 3 from 1. Fieldprobe is neither 1 nor 2 for production and remains historical only.

### 11.2 Tolerances

`AaaByteExact` does not accept a final floating tolerance. Intermediate algorithms have fixed numeric enclosures and subdivision limits, but success means:

```text
all exact possible channel values -> one stored u8 code
```

Geometry conformance additionally reports q/normal/coverage intervals, but those are diagnostics and proof inputs. They are not a substitute for final byte equality.

---

## 12. Repository ownership map after completion

### 12.1 Compiler

```text
crates/wrela-compiler/src/pixels/
  mod.rs                 pass driver/public API
  version.rs             frame/numeric/formal revision constants
  diagnostics.rs         P001–P025 and internal errors
  config.rs              RendererDecl -> RendererConfig
  ids.rs                 stable ID newtypes
  arena.rs               append-only deterministic arenas
  symbolic.rs            dedicated typed-body interpreter
  legality.rs            closed source-subset checks
  quota.rs               symbolic compiler ceilings
  scalar.rs              scalar DAG
  graph.rs               field DAG
  material_graph.rs      material DAG
  field_intrinsics.rs    closed field lowering
  material_intrinsics.rs closed material lowering
  canonicalize.rs        exact folds/CSE/topology equality
  params.rs              used field paths/snapshot/dependency digests
  bounds.rs              compiler f64 intervals
  derivative_bounds.rs   world/parameter/time derivative contracts
  world_bounds.rs        structural AABBs
  support.rs             smooth-CSG support budgets
  objects.rs             maximal smooth object partition
  csg.rs                 Boolean occupancy/influence program
  primitive.rs           primitive semantics/templates
  features.rs            fused feature decomposition
  repeat.rs              finite instance expansion
  deform.rs              bounded deformation programs
  material.rs            events/summaries/opacity/texture dependencies
  texture.rs             asset/mip/minmax/moment compiler
  camera.rs              canonical camera coefficients
  polynomial.rs          bounded polynomial construction
  projective.rs          inverse-q feature equations
  derivatives.rs         projective/Taylor/temporal bundles
  projection_bounds.rs   screen/tile/row spans
  event_kinds.rs         closed event tag set
  events.rs              local event generators
  competition.rs         sparse q-order pairs
  exclusions.rs          explicit omitted-subject proofs
  index.rs               immutable local indexes/BVH
  capacities.rs          exact storage maxima
  program.rs             rich FrameProgram model
  verify.rs              structural/projective/program verifier
  encode.rs              v1 binary writer
  decode.rs              hostile decoder
  binary_verify.rs       wire validation
  state.rs               mutable renderer layout
  glue.rs                generated Wrela config/actors/functions
  workload.rs            exact loop/workload attachment
  cost_bounds.rs         path structural work bounds
  admission.rs           mode/deadline/memory gate
  dump.rs                three stable stages
  report.rs              renderer report facts
  tables.rs              immutable tone/transfer/direction assets
  test_vectors.rs        cross-language numeric vectors
  reference/
    interval.rs
    iv32.rs
    poly.rs
    root.rs
    certificate.rs
    order.rs
    csg.rs
    fixed_q.rs
    coverage.rs
    normal.rs
    material.rs
    transfer.rs
    display.rs
    sweep.rs
    events.rs
    rebuild.rs
    frame.rs
    light.rs
    secondary.rs
    area_light.rs
    ao.rs
    shade.rs
    scheduler.rs
    probe.rs
    kinetic.rs
```

### 12.2 Wrela standard library and driver

```text
stdlib/core/field.wr
stdlib/core/render.wr
stdlib/core/render_interval.wr
stdlib/core/render_program.wr
stdlib/core/render_events.wr
stdlib/core/render_sweep.wr
stdlib/core/render_coverage.wr
stdlib/core/render_material.wr
stdlib/core/render_light.wr
stdlib/core/render_transfer.wr
stdlib/core/render_probe.wr
stdlib/core/render_raster.wr
stdlib/core/render_actor.wr
stdlib/core/render_test_vectors.wr
stdlib/core/simd.wr
stdlib/drivers/display.wr
stdlib/data/pixels/*
```

### 12.3 Machine/VMM

```text
crates/wrela-machine/src/pixels.rs
crates/wrela-machine/src/display.rs
crates/wrela-machine/src/layout.rs
crates/wrela-vmm/src/display/*
crates/wrela-vmm/src/replay.rs
```

### 12.4 Formal

```text
formal/pixels/
```

Only generic mathematics and theorem-to-kernel manifests live here. Do not copy compiler source or large generated scene facts into Lean.

### 12.5 Tests, examples, and hardware locks

```text
tests/golden/check-pixels-*
tests/golden/err-pixels-*
tests/golden/boot-pixels-*
tests/golden/pixels-asm/
tests/pixels_truth/
examples/pixels_*/
bench/pixels-acceptance.toml
bench/pixels-pi5-lock.toml
docs/hardware/pixels-pi5.md
```

---

## 13. Milestone dependency and invariant ladder

A later milestone may rely only on the invariant published by every earlier closed milestone.

| milestone | invariant after close |
|---|---|
| P0 | source/runtime/formal contract, empty stages, and permanent corpus are pinned |
| P1 | renderer declarations are typed/sealed and have finite source metadata |
| P2 | roots compile to deterministic exact symbolic graphs |
| P3 | structural scene, support completeness, features, CSG, and capacities are verified |
| P4 | projective features and all local event interactions are emitted or explicitly excluded |
| P5 | verified frame program/state/actors are sealed in ordinary Wrela images |
| P6 | every trusted numeric predicate has Lean/Rust/Wrela scalar correspondence |
| P7 | complete visibility is constructed from scratch with no prior state or sample oracle |
| P8 | visibility becomes exact presented BGRA tiles with analytic coverage |
| P9 | complete opaque AAA output is byte-certified |
| P10 | transparency and probe GI compose into the same byte proof |
| P11 | kinetic maintenance is byte/error-equivalent to full rebuild |
| P12 | real SIMD AArch64 code is fully costed and compiler-admitted |
| P13 | actual Pi 5 and full trust-chain conformance activate the profile |

Do not parallelize tasks across an invariant boundary. Worktrees may parallelize independent tests/documentation inside one task only when one owner integrates and runs the exact task gate.

---

## 14. Exact commit order

The expected linear history is the task order in this document. The compact list below is the execution queue. A commit must not combine adjacent tasks just because one is small; stable dump/test boundaries are deliberate.

```text
P0.1  production Pixels contract
P0.2  stage/module scaffolding
P0.3  Lean project skeleton
P0.4  permanent renderer corpus

P1.1  field/render stdlib surface
P1.2  field/material typed attrs
P1.3  parameter ranges/rates
P1.4  closed field intrinsic typing
P1.5  finite body legality
P1.6  Image.renderer declaration
P1.7  sealed renderer checks
P1.8  source/image dumps

P2.1  symbolic arenas/IDs
P2.2  scalar symbolic graph
P2.3  symbolic calls/control
P2.4  field graph lowering
P2.5  material graph lowering
P2.6  exact canonicalization/CSE
P2.7  symbolic graph dump

P3.1  coefficient dependencies
P3.2  scalar value intervals
P3.3  derivative/Lipschitz bounds
P3.4  structural world bounds
P3.5  smooth support shells
P3.6  smooth objects/hard CSG
P3.7  fused features
P3.8  repeats/deformations
P3.9  material events
P3.10 structural capacities
P3.11 structural verifier
P3.12 structural proof dump

P4.1  inverse-q camera model
P4.2  bounded polynomial programs
P4.3  plane/quadric q equations
P4.4  torus/deformation programs
P4.5  derivative bundles
P4.6  projected spans
P4.7  feature event generators
P4.8  q competition/swap generators
P4.9  exclusion records
P4.10 local indexes
P4.11 projective/event verifier
P4.12 projective/event dumps

P5.1  FrameProgram v1 records
P5.2  deterministic encoder
P5.3  hostile decoder/fuzz
P5.4  image-build integration
P5.5  frameprog/pixelsdata placement
P5.6  append immutable bytes
P5.7  generated Pixels config
P5.8  generated renderer actors
P5.9  exact kernel reachability
P5.10 program/layout/report dumps
P5.11 binary/repro gates

P6.1  numeric vector format
P6.2  checked dyadic intervals
P6.3  polynomial/range kernels
P6.4  complete root isolation
P6.5  run root certificates
P6.6  q-order/CSG kernels
P6.7  fixed-q recurrence
P6.8  analytic coverage
P6.9  normal/material bounds
P6.10 transfer/display kernels
P6.11 theorem-kernel boot lane
P6.12 formal trust boundary

P7.1  FrameProgram guest views
P7.2  frame snapshot validation
P7.3  fixed worker workspaces
P7.4  complete row candidates
P7.5  all row-start roots
P7.6  hard-CSG root sweep
P7.7  row event partition
P7.8  implicit-jet candidates
P7.9  complete certified runs
P7.10 bounded local rebuild
P7.11 row proposals
P7.12 tile sweep
P7.13 multiworker execution
P7.14 independent host oracle
P7.15 complete debug visibility

P8.1  BGRA tile contract
P8.2  raster records
P8.3  scalar fixed-q raster
P8.4  packet fixed-q raster
P8.5  packet geometry reconstruction
P8.6  event coverage raster
P8.7  double-buffer tile ownership
P8.8  display device/VMM
P8.9  atomic presentation
P8.10 frame replay digests
P8.11 visibility/scanout conformance

P9.1  working color/filmic tables
P9.2  standard diffuse-GGX material
P9.3  deterministic textures
P9.4  material summaries
P9.5  normal-detail moments
P9.6  direct lights
P9.7  secondary visibility
P9.8  area-light integration
P9.9  AO taps
P9.10 shading summaries/packets
P9.11 display-unit refinement queue
P9.12 final byte-certified opaque output
P9.13 opaque quality sequences

P10.1 opacity classification
P10.2 ordered surface layers
P10.3 transfer trees
P10.4 transparent-tail proof
P10.5 deterministic probe model
P10.6 probe initialization
P10.7 probe invalidation/update
P10.8 bounded SH shading
P10.9 final transparency/GI integration
P10.10 transparency/GI sequences

P11.1 frame dependency digests
P11.2 temporal derivative programs
P11.3 persistent kinetic state
P11.4 compressed slack
P11.5 event expiry schedule
P11.6 structure transport
P11.7 q-order braids
P11.8 limited local surgery
P11.9 sealed repair/full-sweep choice
P11.10 byte-proven shading transport
P11.11 crisp temporal policy
P11.12 kinetic-disable equivalence
P11.13 temporal/cut sequences

P12.1 per-frame coefficient evaluator
P12.2 used kernel palette
P12.3 NEON operation closure
P12.4 packet proof/shading kernels
P12.5 hot-loop code-shape assertions
P12.6 A76 cost dimensions
P12.7 renderer workloads
P12.8 deadline/memory admission
P12.9 A76 assembly artifacts
P12.10 backend/cost equivalence

P13.1 Wrela acceptance images
P13.2 Pi conformance runner
P13.3 A76 calibration lock
P13.4 full-sweep 1080p60 gate
P13.5 sustained AAA/thermal gate
P13.6 remove implementation unlock
P13.7 final trust-chain closure
P13.8 activate normative specs
P13.9 release-check command
P13.10 ownership/census cleanup
```

---

## 15. Executor prohibitions

The implementation agent must not:

1. add a GPU, host-side renderer, compute shader, Metal shader, Vulkan pipeline, OpenGL path, or host image decoder;
2. use fieldprobe as production code or copy its sample-first quadtree into the renderer;
3. use dense truth, a dense depth frame, or a dense edge mask to choose candidates/runs/refinement;
4. use previous-frame state as the only source of geometry/event discovery;
5. return background on step/depth/cap/nonfinite/certificate failure;
6. accept a patch/run from sampled agreement without a complete structural/root/order proof;
7. use normalized ray distance as the primary patch coordinate;
8. normalize camera rays inside projective feature evaluation;
9. flatten fused boxes/capsules/round boxes back to kink-heavy generic SDF algebra for primary certificates;
10. infer finite repetition by a sampled camera frame;
11. hide unsupported field/material operations in an opaque per-pixel evaluator under `AaaByteExact`;
12. add runtime allocation, hash tables, pointer trees, work stealing, or nondeterministic completion-order accumulation;
13. use stochastic AA, stochastic shadows, stochastic GI, temporal jitter, TAA, denoising, or stochastic dither;
14. use a shadow map or volumetric light/AO bake;
15. use a full global aspect graph or whole-scene resultant as a prerequisite;
16. assume generic catastrophe normal forms without checked preconditions;
17. make alpha theory alone a run-domain proof;
18. use a low-rank/material summary without an a posteriori residual bound;
19. drop transparent layers based only on candidate transmittance;
20. leave invalid probes in a presented frame;
21. transport old color into a disoccluded pixel;
22. claim kinetic correctness without disabled/full-sweep byte equivalence;
23. claim register residency, SIMD speed, A76 cycles, or Pi frame rate without the required emitted-assembly/hardware evidence;
24. loosen tolerances, error budgets, subdivision depths, costs, or capacities merely to turn a failing fixture green;
25. add an external Cargo dependency;
26. skip a formal gate because Lean is unavailable in the milestone/release environment;
27. update a golden or hardware lock automatically;
28. combine tasks across stable dump boundaries;
29. delete unfavorable historical evidence;
30. present a partial back buffer.

---

## 16. Final delivered system

When P13.10 is complete, Wrela contains:

- a typed, closed `@field` language for supported implicit geometry;
- a typed `@material` language for deterministic diffuse-GGX materials and textures;
- finite parameter range/rate contracts;
- `Image.renderer` as a sealed image declaration;
- a dedicated symbolic field/material compiler;
- structural smooth-object/hard-CSG decomposition;
- complete smooth support shells and fused features;
- inverse-depth projective feature programs;
- complete local visibility/material/order event generators with explicit exclusions;
- exact binary `FrameProgram v1` plus mutable renderer-state placement;
- generated Wrela coordinator/worker actors and coefficient/kernel palette;
- a from-scratch validated scanline sweep;
- fixed-q NEON rasterization and analytic event coverage;
- deterministic textures, material filtering, direct/area lights, AO, transparent transfer, and probe GI;
- display-byte-referred error/refinement calculus;
- kinetic proof maintenance that is optional and byte-equivalent to full rebuild;
- BGRA tile-list display and replay frame digests;
- Lean proofs of the generic trust-boundary mathematics;
- Rust/Wrela scalar/packet differential correspondence;
- emitted A76 assembly and compiler cost admission;
- locked Pi 5 1080p60/full-quality conformance;
- one release command computing the final verdict.

The single operational rule remains:

> The compiler describes every possible visible interaction admitted by the profile. The from-scratch sweep proves the current frame. The runtime may maintain that proof, but it may never replace a missing proof with a guess.
