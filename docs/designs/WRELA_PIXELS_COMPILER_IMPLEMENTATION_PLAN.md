# Wrela Pixels compiler and runtime implementation plan

**Status:** EXECUTION PROGRAM — implementation begins only after milestone P-1 reconciles this document with the current repository and closes the named proof/API gaps. There are no experimental runtime lanes and no fieldprobe dependency.

**Repository basis:** `rywible/wrela8`, branch `pixels-mp-1`, commit `e9f6bcb106fca3af8b8bb2fb57dd11fcdc4c4031` (2026-08-02). Paths marked `new at P-1 basis` did not exist at that commit. The reconciliation inventory is `docs/designs/wrela-pixels-reconciliation.md`.

**Primary owner:** `crates/wrela-compiler`.

**Runtime owner:** Wrela standard library under `stdlib/core/` and the machine-v1 display driver under `stdlib/drivers/`.

**Formal owner:** `formal/pixels/`, a pinned Lean project outside the Cargo dependency graph.

**Historical evidence:** `crates/wrela-fieldprobe` remains checked in as the record that rejected the sample-first quadtree certificate. No task in this plan changes fieldprobe or uses dense truth to make an online decision.

---

## How to execute this plan

This document is intentionally written for an implementation agent that should not need to choose architecture, invent proof policy, or infer task ordering.

Execution rules:

1. Work milestones and tasks strictly in numerical order. Unless a task says otherwise, its prerequisites are every earlier task in the same milestone plus the preceding milestone's close gate.
2. Treat each `Task Px.y` as one review unit and one commit unless the task explicitly names multiple independently gated commits.
3. Every task must contain **Requires**, **Produces**, **Files**, **Contract/dump delta**, **Work**, **Tests**, **Focused checks**, **Repository gate**, and **Stop conditions**. The older four-part task entries below are not executable until P-1.1 mechanically upgrades them to this schema.
4. `Files` distinguishes `new` from `modified`; paths must exist at the recorded repository basis unless marked `new`.
5. Run focused checks while diagnosing, then run exactly `cargo xtask verify` before every task commit and at every milestone close. Fuzzing is a separate discovery lane and never substitutes for the required gate.
6. Never skip a stable-dump task. The dumps are the compatibility boundary between compiler stages and the forensic record when a later invariant fails.
7. Do not begin runtime optimization before the from-scratch validated sweep is correct. Kinetic reuse, packet SIMD, and cycle-proxy admission are deliberately downstream.
8. Beginning with P7.9, record deterministic certificate telemetry: run-length distributions, proof methods/shapes, expiry causes, active predicate counts, margin owners, subdivision depths, and bounded-rebuild reasons. P8 locks the schema and adversarial visibility corpus and records an informational cost trend. P9 adds shading/output refinement ownership. These reports do not become non-regression/admission gates until P12 and never replace exact structural workload bounds.
9. Section 13 is the invariant ladder. Section 14 is the exact commit order. Section 15 is a hard prohibition list.

Dependency policy:

- “No new dependencies” means no new external Cargo dependencies.
- Lean 4.30.0 and its pinned Mathlib revision are an explicitly approved proof-tool dependency outside the Cargo and shipped-image graphs.
- Ordinary builds and tests must not require network access after the formal toolchain cache is installed and verified by P-1.1.
- Section 12.1 is the authoritative final ownership map. Section 3.2 is only the P0/P1 seed layout.

Milestone map:

| milestone | primary deliverable | review units |
|---|---|---:|
| P-1 | repository reconciliation, source ABI proof, smooth-object soundness, vertical walking skeleton | 6 |
| P0 | normative contract, compiler/formal scaffolding, permanent fixtures | 4 |
| P1 | typed `@field`/`@material` source surface and sealed `Image.renderer` declaration | 8 |
| P2 | deterministic dedicated symbolic field/material compiler | 7 |
| P3 | structural bounds, smooth support, objects, fused features, capacities | 12 |
| P4 | projective inverse-depth programs and complete local event/exclusion system | 12 |
| P5 | binary `FrameProgram v1`, image placement, generated renderer actors | 11 |
| P6 | verified dyadic/numeric kernels and Lean–Rust–Wrela correspondence | 12 |
| P7 | correct from-scratch validated scanline sweep | 15 |
| P8 | fixed-q raster, analytic coverage, display-byte output, replay | 15 |
| P9 | deterministic AAA material, texture, lighting, shadow, AO, filtering | 13 |
| P10 | ordered transparency and deterministic probe GI | 10 |
| P11 | optional kinetic proof maintenance with full-sweep byte equivalence | 13 |
| P12 | generated kernel palette, one-ISA lowering, A76 cost admission | 15 |
| P13 | exact A76 cycle-proxy conformance, normative activation, ownership closure | 10 |

P-1 supplies a plane-only 64×32 headless vertical walking skeleton so the source ABI, image declaration, display path, and end-to-end dump boundaries are exercised before the deep proof machinery is built. The first production-capable correctness boundary remains the end of P8: a field scene can be compiled, swept from scratch, rasterized, and presented without temporal state. P9–P10 establish the full-quality image contract. P11 reduces recurring work without changing bytes. P12–P13 establish that the generated implementation fits and sustains the target machine profile.

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
    AoConfig,
    Camera,
    CameraBounds,
    LightConfig,
    MaterialSample,
    ProbeConfig,
    RgbRange,
    RenderFrame,
    RenderProfile,
    ScalarRange,
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
fn shade(surface: SurfaceContext[MaterialId], read s: SceneParams) -> MaterialSample:
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
        camera_bounds=CameraBounds.gameplay_default(),
        light_config=LightConfig.gameplay_default(),
        exposure_range=ScalarRange(min=-8.0, max=8.0),
        environment_range=RgbRange.black_to_hdr_white(),
        ao=AoConfig.deterministic_v1(),
        probes=ProbeConfig.disabled(),
        initialization_deadline_ms=250,
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

The public frame value is:

```wrela
struct RenderFrame[P]:
    params: P
    camera: Camera
    lights: LightFrame
    exposure: f32
    environment: Rgb
    frame_index: u64
```

`LightFrame` is a fixed-capacity sealed value whose identities and topology are declared by `light_config`; a frame may change only the bounded numeric values of those declared lights. `camera`, `lights`, `exposure`, and `environment` are validated against the renderer declaration before any worker writes output.

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
9. A proof or capacity failure prevents presentation and returns `RenderError`; it never becomes background, a stale hit, or a guessed color. Acceptance is fail-closed but not universally total over every declared numeric range: release conformance separately proves zero errors for the locked workload.
10. The compiler report publishes frame-program bytes, renderer-state bytes, per-core placement, exact capacity derivations, fallback classes, and generated hot functions.
11. The Lean project builds with no admissions and prints no unexpected axioms for the trust-boundary theorems.
12. The Rust compiler reference, generated Wrela scalar kernels, generated Wrela SIMD kernels, and host oracle agree on all permanent differential fixtures; every hot workload satisfies its one-ISA instruction obligation with no missed or illegal idiom.
13. The machine-v1 display conformance lane presents the exact expected frame digests.
14. The flagship A76/Pi 5 target profile is admitted at 1080p60 by the exact sealed renderer cycle proxy, with every acceptance frame below budget, no unresolved frame, and no output divergence during the locked workload. Physical hardware execution is not a conformance input.

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

A compiler exclusion proved over the complete declared parameter box and
spatial domain remains valid even when an influencing parameter has nonzero
rate. Polynomial exclusions prefer Bernstein coefficient signs and bounded
subdivision. Every globally removed runtime predicate retains a stable
exclusion record and proof payload; an inconclusive proof emits the ordinary
runtime predicate.

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
fn name(surface: SurfaceContext[M], read params: P) -> MaterialSample
```

`M` is the one nominal material-identity enum used by every `mark(..., material=...)` reachable from this renderer. The compiler infers it from the field graph and requires the material function to name the same enum. The `params` argument may be omitted. Material code may branch on the compile-time-dense `surface.material` identifier. Other runtime control flow is accepted only if the material compiler can represent both branches and prove their boundary; otherwise `AaaByteExact` rejects it.

#### `@range(min=..., max=...)`

Allowed on numeric fields reachable from a renderer parameter type.

- On `f32`, both endpoints are finite `f32` literals and `min <= max`.
- On `Vec2`/`Vec3`/`Rgb`, the same range applies component-wise.
- Arrays and structs do not accept a recursive shorthand. Annotate each influencing scalar/vector field so the source path and diagnostic remain unambiguous.
- Integer and enum values do not need a numeric range.
- Every geometry coefficient must resolve to exactly one range.

#### `@rate(max_delta=..., max_second_delta=...)`

Optional. It enables kinetic transport for that path.

- Values are finite, nonnegative `f32` literals in units per rendered frame.
- Missing `@rate` does not reject the renderer and does not create a hidden runtime state. It simply disables kinetic reuse whenever that path changes.
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

Authors cannot assert arbitrary displacement bounds in v1. `sinusoidal_displace` is the only public deformation constructor; the compiler lowers it to the internal `BoundedDisplace` node and derives amplitude, gradient, Hessian, and third-derivative bounds from its closed compile-time form. Accepting user-supplied deformation contracts is deferred until a proof-carrying source mechanism exists. Arbitrary `Field + f32` is impossible because `Field` is opaque.

Every listed intrinsic has a real scalar Wrela body. It must not be a `panic` placeholder: that body is the normative fallback/source semantics and is differentially tested against symbolic lowering.

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
    camera_bounds=CameraBounds(...),
    light_config=LightConfig(...),
    exposure_range=ScalarRange(...),
    environment_range=RgbRange(...),
    ao=AoConfig(...),
    probes=ProbeConfig(...),
    initialization_deadline_ms=...,
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
- camera, light, exposure, environment, AO, and probe declarations are finite, compile-time, and provide every runtime input bound needed by the compiler;
- `initialization_deadline_ms` is positive and includes worst-case deterministic probe initialization when probes are enabled;
- output extent matches the display declaration;
- only one renderer may own a given display declaration;
- a renderer declaration participates in the image-construction DAG;
- the renderer actor and its internal worker actors receive deterministic core placement.

The call returns `ImageDecl[Renderer[P]]`. `handle()` returns `Actor[Renderer[P]]` and follows the existing image-declaration handle rules. P-1.2 implements the deliberately narrow generic actor-handle support needed for this exact sealed declaration; arbitrary generic actors remain unsupported.

### 2.6 Runtime result and failure semantics

Add to `stdlib/core/render.wr`:

```wrela
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

struct RenderedFrame[P]:
    params: P
    frame_index: u64
    displayed_digest: [u8; 32]
    rebuilt_tiles: u32
    reused_tiles: u32
```

No `RenderError` path flushes the partially built back buffer. The previous complete framebuffer remains on screen. The error returns to the caller; the image’s ordinary failure policy decides what the application does next.

Static table/workspace capacity mismatch should be impossible after a successful build and maps to `InternalInvariant`; bounded certificate/root/event refinement exhaustion is an expected fail-closed runtime outcome with its §2.6 variant. Neither is interpreted as a scene miss.

This enum is the sole runtime error contract. Later runtime sections and tasks may add internal causes mapped into these variants, but may not define a second public `RenderError`. “Supported frame” means a frame that satisfies the declaration contract and completes its bounded certificates; the API deliberately permits a valid in-range frame to return an exhaustion error. The P13 acceptance workload has the stronger zero-error requirement.

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

### 3.2 P0/P1 seed compiler module layout

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

Compiler `MaterialId` is a dense internal ID assigned from the renderer’s one nominal source enum `M`; wire/runtime records never store the source enum’s Rust layout or assume its numeric discriminant.

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
    Neg { child: FieldId },
    Transform { child: FieldId, transform: TransformProgram },
    FiniteRepeat { child: FieldId, axis: Axis, first: i32, count: u32, period: ScalarId },
    BoundedDisplace {
        base: FieldId,
        displacement: ScalarId,
        contract: DerivedDeformContract,
    },
    Mark {
        child: FieldId,
        object_source: CanonicalIdentity,
        material_source: CanonicalIdentity,
    },
}

struct DerivedDeformContract {
    amplitude_bound: ScalarId,
    gradient_bound: ScalarId,
    hessian_bound: ScalarId,
    third_derivative_bound: ScalarId,
    derivation: ClosedDeformDerivation,
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

For each maximal smooth object the compiler also emits:

```rust
struct SmoothObjectRootProgram {
    object: ObjectId,
    scalar_root: ScalarId,
    candidate_leaf_slabs: IdRange,
    support_certificate: SupportCertificateId,
    root_isolation_capacity: u32,
}
```

Support shells prove where an object root may occur; they do not prove that a
leaf itself is zero. Candidate leaf `q` slabs are conservative sublevel domains
where `leaf <= accumulated_support_budget`, not neighborhoods inferred only
from leaf zeros. Polynomial leaf-sublevel boundaries may be isolated with
Bernstein sign variation and subdivision. Runtime isolation still evaluates
`scalar_root`, the complete composed smooth-object scalar, throughout every
retained slab. The permanent regression `a=b=k/4` must find the smooth-min zero
even though neither child is zero.

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

`FrameProgram v1` is little-endian, offset-based, pointer-free, and directory-based. P5 freezes this outer envelope and its table-kind namespace. P9–P11 may fill predeclared table kinds, but may not change the header or directory representation. Version 1 remains internal until P11 closes and becomes an activated compatibility promise only at P13.

Header, exactly 80 bytes:

```rust
#[repr(C)]
struct FrameProgramHeaderV1 {
    magic: [u8; 8],          // b"WRELAPX\0"
    version: u16,            // 1
    header_bytes: u16,       // 80
    flags: u32,
    total_bytes: u32,
    renderer_index: u16,
    reserved0: u16,
    numeric_revision: u32,
    formal_revision: u32,
    table_count: u16,
    reserved1: [u8; 14],
    digest: [u8; 32],
}

#[repr(C)]
struct FrameProgramTableV1 {
    kind: u16,
    record_bytes: u16,
    count: u32,
    offset: u32,
    byte_len: u32,
}
```

The table directory starts at byte 80 and contains exactly `table_count` 16-byte entries sorted by the explicit versioned table-kind number. The v1 namespace predeclares scalar, field, object, feature, material, parameter, event, CSG, fixed-domain, immediate, camera/light/post, texture, shading-summary, transparency, probe, kinetic, and optional debug-name tables. A table not yet populated has `count=0`, `offset=0`, and `byte_len=0`.

The digest field is zero while hashing and then filled with SHA-256 of the complete encoded bytes with that field zeroed. The encoder must assert the Rust structs are not used for serialization. Fields are written explicitly in order so host padding cannot affect bytes.

All table offsets are 16-byte aligned. Records have explicit encoded sizes and reserved bytes set to zero. The decoder rejects:

- wrong magic/version/header size;
- an unsorted, duplicate, unknown-required, or inconsistent table-directory entry;
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

Before isolating the complete object scalar, use compiler-emitted candidate
leaf sublevel slabs to partition the q domain. Polynomial slabs isolate
boundaries of `leaf - accumulated_support_budget` with the fixed Bernstein
sign-variation/subdivision kernel. Analytic affine/quadratic feature roots may
refine those proposals. Leaf zeros alone never establish completeness; every
retained slab is checked against the complete smooth-object scalar.

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

For a supported polynomial on a q interval, Bernstein range exclusion and
proved sign-variation/root-count predicates run before generic interval
evaluation. An inconclusive count continues with derivative contraction or
subdivision and never labels a partial root list complete.

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

For an algebraic/Taylor polynomial whose composition remains inside the sealed
degree/term shapes, the preferred monotone-tube representation is Bernstein
coefficient form:

1. generated checked dyadic schedules compose
   `G(x,q_hat(x)-eps)` and `G(x,q_hat(x)+eps)`;
2. candidate-conversion and Taylor remainder radii widen every affected
   coefficient outward;
3. complete coefficient scans prove the two strict face signs;
4. de Casteljau subdivision tightens an inconclusive hull.

These are integer verifier kernels. Floating FMA/dot evaluation may construct
a candidate but has no acceptance authority. Subdivision does not reduce
degree. A shape/term overflow or checked arithmetic failure falls back to the
ordinary interval/Taylor tube rather than rejecting an otherwise supported
renderer.

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

A condition/homotopy-style estimate may propose a transport step, temporal
center, or revalidation order, but acceptance remains the complete dyadic
first/second-order remainder and root/event/order/identity/shading/
quantization slack check. Diagnostics classify scheduled predicate expiry,
isolated simple events, simultaneous/degenerate events, and event storms. A
storm changes only the deterministic choice between local repair and the
equivalent full sweep.

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
    pub material_identity_ty: types::Type,
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
    pub light_config: LightConfig,
    pub exposure_range: ScalarRange,
    pub environment_range: RgbRange,
    pub ao: AoConfig,
    pub probes: ProbeConfig,
    pub initialization_deadline_ms: u32,
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
- every path eligible for kinetic reuse has valid `@rate` metadata; a changing path without it invalidates reuse and remains legal for a from-scratch frame;
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
    verify::check_symbolic(&cx)?;
    params::collect_and_validate(&mut cx)?;
    bounds::propagate_values(&mut cx)?;
    bounds::propagate_derivatives(&mut cx)?;
    support::propagate(&mut cx)?;
    repeat::compile_finite_instances(&mut cx)?;
    deform::compile_derived_contracts(&mut cx)?;
    objects::partition(&mut cx)?;
    csg::compile_boolean_program(&mut cx)?;
    features::decompose(&mut cx)?;
    verify::check_structural(&cx)?;
    projective::compile_features(&mut cx)?;
    derivatives::compile(&mut cx)?;
    projection_bounds::compile(&mut cx)?;
    events::compile_generators(&mut cx)?;
    competition::compile_q_pairs(&mut cx)?;
    exclusions::compile(&mut cx)?;
    event_index::compile(&mut cx)?;
    verify::check_projective_events(&cx)?;
    capacities::derive_structural(&mut cx)?;
    let program = program::finish(&mut cx)?;
    let program = verify::check_program(program)?;
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

P9–P11 extend `program::finish` with the predeclared material, transparency, probe, and kinetic tables after their respective verified compilers exist. P12 replaces interpreted coefficient records with generated evaluators; no earlier milestone may pretend that evaluator already exists. No task may reorder this pipeline without updating the stable dump and the documented invariant consumed by every later pass.

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

A renderer never flushes a partially built back buffer. The sole public error type is the enum in §2.6. Internal root/event/sheet/run/layer/probe capacity causes map to `CapacityExceeded(RenderCapacity)`; corrupt compiler-generated indices map to `InternalInvariant(RenderInvariant)`; frame contract failures map to `ParameterOutOfRange`, `NonFiniteInput`, or `FrameContractMismatch`; device failures map to `Display(DisplayError)`.

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

The scalar function is the differential oracle for the packet function. The packet function uses only closed SIMD operations from `05-library.md §8.1`; there is no inline assembly, guest feature check, or runtime dispatch. The VMM’s ordinary boot validation rejects a host that lacks the one sealed baseline.

### 6.16 One-ISA workload and instruction-selection contract

Machine-v1 has one sealed ISA and no runtime feature dispatch. For Pixels, the baseline is amended explicitly to `AArch64 ARMv8.2-A + FP/NEON/ASIMD + FEAT_DotProd`; both the emitted-word verifier and VMM boot feature check enforce that exact baseline. The compiler must use the strongest semantics-preserving instruction family for every recognized hot workload shape:

| workload shape | canonical machine-v1 family |
|---|---|
| packed signed/unsigned 8-bit dot accumulation into four i32 lanes | `SDOT` / `UDOT` |
| f32 vector dot, SH evaluation, matrix/color rows | `FMUL` plus dependency-aware `FMLA` |
| widening signed/unsigned integer multiply-accumulate | `SMLAL` / `UMLAL` |
| horizontal integer reductions | `ADDV`, `SADDLV`, or `UADDLV` as signedness/range requires |
| mask select and conditional lane merge | `BSL`, `BIT`, or `BIF` according to operand reuse |
| fixed transposes/deinterleave/interleave | `TRN*`, `UZP*`, `ZIP*`, or structure load/store when alignment and alias proofs permit |
| checked narrowing/packing | `SQXTN`, `UQXTN`, or `XTN` only when the scalar contract proves the corresponding behavior |
| reciprocal and reciprocal-square-root refinement | `FRECPE/FRECPS` and `FRSQRTE/FRSQRTS` with the source-fixed iteration sequence |
| contiguous paired scalar/vector memory operations | paired or structure load/store only when placement, alignment, ownership, and fault behavior are identical |

This table names families, not a license to change arithmetic. `SDOT`/`UDOT` apply only to exact 8-bit dot workloads; the compiler must not quantize f32 SH, lighting, normals, or material math merely to select dot-product instructions. NaN, signed-zero, overflow, first-fault, reduction order, and FMA contraction must match the Wrela source contract. A nearly matching pattern that changes any of those semantics stays on the correct longer sequence.

For a workload with more than one legal sequence, enumerate the finite candidates and select the lowest exact cycle-proxy total for that kernel’s sealed alignment/dependency/workload facts, subject to hot-text budgets; tie by versioned sequence ID. “Use the ISA” means selecting the best proved sequence, not blindly replacing an operation with a fashionable opcode.

Add a versioned `PixelsIsaObligation` census from generated workload/kernel kind to expected instruction family and sequence ID. After final codegen, an emitted-word decoder proves each required idiom is present in the named loop and that disallowed scalarized/redundant forms are absent. A missed legal idiom, a better legal sequence left unselected, an unpriced emitted opcode, or a workload with no obligation is an internal build failure. Differential tests cover both positive selections and “must not select” boundary cases.

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
| `P006` | `` rate for `<path>` is negative, non-finite, or not representable `` |
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
| `P025` | `` renderer-generated image memory exceeds the sealed machine-v1 guest profile `` |

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

An `axiom` imported from Mathlib may appear in `#print axioms` output only if documented in `formal/pixels/EXPECTED_AXIOMS.txt`. The expected initial list is ordinary classical/propositional extensionality machinery used by Mathlib, not project-defined assumptions.

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
  ordered : lo ≤ hi

def Iv32.denote (exponent : Int) (x : Iv32) : Set Rat := ...
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

- univariate degree ≤ 8;
- bivariate degree ≤ 6 in each variable;
- trivariate Taylor models with a separately certified remainder bound;
- trivariate sparse Taylor model with explicit interval remainder.

Prove:

- Bernstein partition of unity;
- coefficient convex-hull enclosure;
- positivity/negativity from coefficient signs;
- derivative coefficient construction;
- de Casteljau subdivision preserving the represented polynomial;
- checked composition from interval source/candidate coefficients encloses the
  exact composed polynomial;
- opposite strict signs for the complete Bernstein coefficient arrays of the
  two tube faces imply the uniform face-sign hypotheses used by the monotone
  tube theorem;
- the exact bounded sign-variation predicates used by root isolation,
  including their inconclusive case;
- strict coefficient sign over a normalized spatial/parameter box remains a
  valid exclusion over every point in that box;
- exact quadratic rectangle range candidate completeness: corners, interior stationary point, and edge stationary points;
- Taylor polynomial plus remainder containment.

Formalize only the finite composition shapes emitted by P4.2. Subdivision
tightens coefficient hulls and preserves degree; it is never presented as
degree reduction. Do not build a generic computer algebra library.

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

Create `formal/pixels/KERNELS.txt` using the fixed line format parsed by xtask without a new dependency. Each row names:

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
- Bernstein composition containment, complete coefficient sign scan, and
  de Casteljau subdivision;
- Bernstein sign-variation/root-count predicates and global box strict-sign
  exclusion;
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

Add to `verify-deep`:

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
8. run `cargo xtask verify` at milestone close;
9. never update a golden before reading and explaining the diff;
10. never loosen a numeric tolerance, capacity, or error budget to make a test pass;
11. never use fieldprobe output, dense truth, or previous-frame data to make a renderer decision;
12. never reinterpret `Unresolved` or `RenderError` as background or success.

The task descriptions below already choose the algorithm. When implementation reveals an internal contradiction, stop at that task, preserve the failing fixture, and fix the contradiction in the plan/documentation before proceeding. Do not substitute a different renderer architecture inside a code commit.

---

# Milestone P-1 — repository reconciliation and vertical walking skeleton

Milestone result: the plan is rebased onto real repository paths and language capabilities; the public renderer declaration type-checks; one canonical contract exists for every shared type/format; the two soundness gaps have positive proof artifacts; and a plane-only 64×32 image travels from `Image.renderer` through a guest display driver to a deterministic headless digest. This milestone is not the renderer implementation and its plane-only restriction is explicit.

## Task P-1.1 — reconcile the plan with repository reality

**Requires:** the repository basis named at the top of this document.

**Produces:** a path/toolchain inventory, an updated basis commit, a minimal buildable formal project, and every P0–P13 task rewritten to the executor schema in §10.0.

**Files:** this plan, `AGENTS.md` (modified only to clarify the already-approved non-Cargo Lean tool dependency), `docs/designs/wrela-pixels-reconciliation.md` (new), and the minimal `formal/pixels/{lakefile.toml,lean-toolchain,Pixels.lean}` project files (new).

**Contract/dump delta:** none; this is an execution-control change.

**Work:** verify every listed path with `rg --files`; mark future files `new`; replace stale paths; record the current generic-actor, image-layout, AArch64 codegen, ISA-feature ledger, emitted-word audit, cycle-proxy, dump, and xtask extension points; verify that every conforming machine-v1 host class can satisfy the planned `FEAT_DotProd` baseline before P12 activates it; create and locally cache the minimal Lean 4.30.0/Mathlib project required by P-1.5; mechanically add Requires/Produces/Contract/dump delta/Tests/Focused checks/Repository gate/Stop conditions to every later task. Section 12.1 remains the final ownership authority.

**Tests:** an xtask plan-lint rejects missing task fields, nonexistent unmarked paths, duplicate task IDs, dump-stage drift, and a mismatch between §14 and task headings.

**Focused checks:** `cargo xtask pixels-plan-lint`.

**Repository gate:** `cargo xtask verify`.

**Stop conditions:** any required change would add a Cargo dependency, the pinned formal toolchain cannot be reproduced, or a path cannot be assigned a single owner.

## Task P-1.2 — prove and implement the public source ABI

**Requires:** P-1.1.

**Produces:** a type-checking minimal declaration using `Renderer[P]`, `RenderFrame[P]`, `SurfaceContext[M]`, and `Actor[Renderer[P]]`.

**Files:** `stdlib/core/field.wr` (new), `stdlib/core/render.wr` (new), the existing sema type/check modules identified by P-1.1, and `tests/golden/check-pixels-source-abi/` (new).

**Contract/dump delta:** adds only the narrow sealed generic actor-handle rule required by `ImageDecl[Renderer[P]].handle()`; arbitrary generic actors remain rejected.

**Work:** define the exact source types in §§0 and 2; prove one nominal material enum flows from every `mark` into `SurfaceContext[M]`; make `RenderFrame[P]` and `RenderedFrame[P]` ownership type-check; reject mismatched `P` or `M`; give every field intrinsic a real scalar body.

**Tests:** positive source-ABI golden plus mismatched parameter enum, mismatched material enum, arbitrary generic actor, and ownership misuse diagnostics.

**Focused checks:** the focused sema tests and the named goldens.

**Repository gate:** `cargo xtask verify`.

**Stop conditions:** the API requires general-purpose higher-kinded types, runtime allocation, or a second actor ABI.

## Task P-1.3 — establish the display ABI and headless sink

**Requires:** P-1.2.

**Produces:** shared machine-v1 display records, a guest driver queue skeleton, a VMM headless consumer, and deterministic presented-frame digests.

**Files:** `crates/wrela-machine/src/pixels.rs` (new), `stdlib/drivers/display.wr` (new), `crates/wrela-vmm/src/display.rs` (new), `crates/wrela-vmm/src/replay.rs` (new), and focused machine/VMM tests.

**Contract/dump delta:** fixes tile ownership, queue capacity, sequence numbering, BGRA8 byte order, digest scope, and failure ownership before renderer code depends on them.

**Work:** implement only the device model, queue/ownership state machine, guest-facing driver surface, and headless digest sink. macOS/HVF and Linux/KVM presentation backends remain P8 work.

**Tests:** hostile queue transitions, duplicate/missing tiles, digest ordering, and driver error ownership.

**Focused checks:** focused machine/VMM/display tests.

**Repository gate:** `cargo xtask verify`.

**Stop conditions:** the contract requires VMM-side rendering, host pixel modification, guest allocation, or platform-specific state in the shared ABI.

## Task P-1.4 — seal canonical contracts and consistency checks

**Requires:** P-1.3.

**Produces:** one canonical definition each for `RenderError`, `FieldKind`, `Iv32`, `FrameProgramHeaderV1`, the three Pixels dump names, theorem-manifest filenames, and the renderer declaration labels.

**Files:** this plan, `docs/language/07-pixels.md` (new draft), and the plan-lint implementation from P-1.1.

**Contract/dump delta:** `field-graph`, `frame-program`, and `render-layout` are the only Pixels dumps; `FrameProgramHeaderV1` is the 80-byte directory header in §4.14; `KERNELS.txt` and `EXPECTED_AXIOMS.txt` are canonical.

**Work:** make plan-lint extract and compare repeated fenced definitions and stable names. Any later section refers to the canonical section rather than copying a divergent public definition.

**Tests:** mutation fixtures demonstrate that a duplicate error enum, changed header size/magic, fourth dump, or `.md`/`.txt` drift fails lint.

**Focused checks:** `cargo xtask pixels-plan-lint`.

**Repository gate:** `cargo xtask verify`.

**Stop conditions:** two consumers genuinely require incompatible formats; resolve that as a versioned format decision before continuing.

## Task P-1.5 — close smooth-object and deformation soundness

**Requires:** P-1.4.

**Produces:** a positive reference/formal proof that the candidate generator is complete for smooth objects, plus compiler-derived deformation contracts.

**Files:** `formal/pixels/Pixels/SmoothObject.lean` (new), `formal/pixels/Pixels/Deformation.lean` (new), Rust reference tests under the P-1.1 ownership path, and `tests/golden/check-pixels-smooth-interior-root/` (new).

**Contract/dump delta:** a `SmoothObjectRootProgram` links support-shell inclusion to primitive `q` slabs and isolates the full composed object scalar, not merely roots of leaves. Public arbitrary `bounded_displace` is removed; `sinusoidal_displace` lowers to a compiler-derived contract.

**Work:** cover the regression `a=b=k/4`, where both leaves are nonzero while smooth-min is zero; prove saturation outside the shell and complete root coverage inside it; derive amplitude/gradient/Hessian/third-derivative bounds from every accepted deformation form.

**Tests:** smooth-interior zero, saturated branch, nested smooth CSG, and intentionally false deformation-bound fixtures.

**Focused checks:** focused Rust reference tests and `cargo xtask pixels-formal`.

**Repository gate:** `cargo xtask verify`.

**Stop conditions:** any accepted scene can have an object root without a root-program candidate, or any deformation bound depends on an author assertion.

## Task P-1.6 — ship a plane-only vertical walking skeleton

**Requires:** P-1.5.

**Produces:** a bootable 64×32 plane-only renderer image whose source declaration, frame-program envelope, generated actor, scalar raster, guest display driver, VMM sink, and stable dumps produce one locked digest.

**Files:** the compiler/runtime paths established above, `tests/golden/boot-pixels-walking-skeleton/` (new), and its expected three dump files.

**Contract/dump delta:** installs the end-to-end boundaries only. The program rejects every field kind except one marked plane with a specific diagnostic; it is deleted/replaced by P7/P8, not generalized opportunistically.

**Work:** use scalar code only, no event system, kinetic state, SIMD, lighting, or proof claim beyond the analytic plane. Record an informational code-size, memory, and frame-cost baseline.

**Tests:** exact source/dump/image/digest reproducibility and explicit rejection of a sphere or second plane.

**Focused checks:** the skeleton golden and headless replay test.

**Repository gate:** `cargo xtask verify`.

**Stop conditions:** any pixel is created by the host, any boundary bypasses ordinary image construction, or the digest is nondeterministic.

### Milestone P-1 close

Run `cargo xtask verify`. P0 may start only when the plan lint, source ABI, soundness regressions, and vertical walking skeleton are all green.

---

# Milestone P0 — contract, scaffolding, and permanent fixtures

Milestone result: the repository knows that Pixels is a production compiler subsystem, has stable stage scaffolding, a pinned formal project, and a fixed permanent fixture corpus. The deliberately isolated P-1 plane skeleton remains as an end-to-end boundary test; no additional production renderer semantics exist yet.

## Task P0.1 — add the normative implementation chapter

**Requires:** the preceding milestone close gate.

**Produces:** Place the closed source/compiler/runtime contract in the repository before code depends on it.

**Files:**

```text
docs/language/07-pixels.md # new at P-1 basis
docs/language/04-compiler.md
docs/language/05-library.md
docs/language/06-machine.md
docs/designs/pixels.md
```

**Contract/dump delta:** Only the contract and stable-dump changes explicitly named by this task are permitted.

**Work:**

- Add `07-pixels.md` containing sections 0–5 of this plan in normative form.
- After this task, `docs/language/07-pixels.md` is authoritative for language/runtime behavior and this design document is authoritative only for implementation ordering, ownership, and gates. A semantic change must update the language chapter first and then reconcile this plan in the same commit.
- Amend compiler chapter §5 to name `FieldGraph` and `FrameProgram` as compiler-owned data, not executable IR.
- Add `@field`, `@material`, `@range`, `@rate`, `Image.renderer`, renderer public types, and the one-ISA workload/instruction-selection obligation to the library chapter; record `FEAT_DotProd` as a planned P12 machine-v1 baseline addition rather than silently using it.
- Add `frameprog`/`pixelsdata` image regions and generated renderer actors to machine chapter while preserving machine-v1 display semantics.
- Mark the existing `docs/designs/pixels.md` historical measurements as evidence only and link to the normative chapter.
- Preserve the unfavorable online fieldprobe result. Do not rewrite history to imply it validated this renderer.

**Tests:**

- Every new source spelling appears in exactly one normative chapter.
- The chapters state that the validated sweep is correct without kinetic state.
- The chapters state that `AaaByteExact` rejects unsupported source at build time.
- No normative statement cites modeled/Pi-unmeasured performance as fact.
- Documentation links resolve relative to their files.

**Focused checks:** Run the focused tests, fixtures, dumps, and commands named by this task before the repository gate.

**Repository gate:**

```text
cargo xtask verify
```

**Stop conditions:** Stop if an acceptance condition cannot be met without changing a closed architectural decision, weakening a proof/failure boundary, or adding an external Cargo dependency.

**Commit**

```text
pixels P0.1: write the production Pixels contract
```

## Task P0.2 — formalize compiler module scaffolding and zero-renderer dumps

**Requires:** P0.1.

**Produces:** Promote the P-1 boundary-test scaffolding into its permanent module ownership and establish canonical zero-renderer behavior.

**Files:**

```text
crates/wrela-compiler/src/lib.rs
crates/wrela-compiler/src/pixels/mod.rs # new at P-1 basis
crates/wrela-compiler/src/pixels/diagnostics.rs # new at P-1 basis
crates/wrela-compiler/src/pixels/dump.rs # new at P-1 basis
crates/wrela-compiler/src/bin/wrela.rs
tests/golden/check-pixels-empty/input.wr # new at P-1 basis
tests/golden/check-pixels-empty/expected/field-graph.txt # new at P-1 basis
tests/golden/check-pixels-empty/expected/frame-program.txt # new at P-1 basis
tests/golden/check-pixels-empty/expected/render-layout.txt # new at P-1 basis
```

**Contract/dump delta:** Only the contract and stable-dump changes explicitly
named by this task are permitted. P0.2 corrects the preexisting P-1
walking-skeleton envelope that wrote its semantic seed into the v1
`table_count` and reserved bytes. The corrected dump reports a zero-table v1
header and labels the P-1 semantic seed as generated-actor metadata; the
locked displayed-frame digest does not change.

**Work:**

- Retain/add `pub mod pixels;` from the P-1 skeleton and remove any temporary P-1-only module names.
- Define `PixelsError` and `PixelsDiagnostic` without renderer behavior.
- Add CLI stage parsing for `field-graph`, `frame-program`, `render-layout`, and `--renderer=<index>`.
- For an image with no renderer, dumps print version headers plus `Renderers count=0`; the plane-skeleton fixture prints its explicitly restricted minimal records.
- Keep the P-1 displayed-frame digest while moving its compatibility seed out
  of the canonical v1 header. Assert `table_count=0` and every reserved byte is
  zero.
- A renderer index on an image with no renderer is a clear build error.
- Add the three stages to CLI help and stage-validation tests.

**Code shape**

```rust
pub enum PixelsDumpStage {
    FieldGraph,
    FrameProgram,
    RenderLayout,
}

pub fn dump_zero_renderers(stage: PixelsDumpStage) -> String;
```

**Tests:**

- All three dump stages produce byte-stable zero-renderer outputs. The P-1
  skeleton retains its reviewed displayed-frame digest; its frame-program dump
  changes only for the malformed-header correction named above.
- Existing stage behavior and usage text remain unchanged except for additions.
- Unknown `--renderer` use is rejected, not ignored.
- No renderer code is imported by sema, eval, lower, or layout yet.

**Focused checks:** Run the focused tests, fixtures, dumps, and commands named by this task before the repository gate.

**Repository gate:**

```text
cargo xtask verify
```

**Stop conditions:** Stop if an acceptance condition cannot be met without changing a closed architectural decision, weakening a proof/failure boundary, or adding an external Cargo dependency.

**Commit**

```text
pixels P0.2: add stage and module scaffolding
```

## Task P0.3 — create the formal project skeleton

**Requires:** P0.2.

**Produces:** Pin the formal environment and make admission checks permanent before theorem work.

**Files:**

```text
formal/pixels/lean-toolchain # new at P-1 basis
formal/pixels/lakefile.toml # new at P-1 basis
formal/pixels/Pixels.lean # new at P-1 basis
formal/pixels/Pixels/TrustBoundary.lean # new at P-1 basis
formal/pixels/README.md # new at P-1 basis
formal/pixels/EXPECTED_AXIOMS.txt # new at P-1 basis
crates/xtask/src/pixels_formal.rs # new at P-1 basis
crates/xtask/src/main.rs
```

**Contract/dump delta:** Only the contract and stable-dump changes explicitly named by this task are permitted.

**Work:**

- Pin Lean/Mathlib `v4.30.0`.
- Add a trivial theorem with `#print axioms`.
- Implement comment/string-aware forbidden-token scanning.
- Add `cargo xtask pixels-formal` and `pixels-formal-scan`.
- `verify` runs the scan only.
- `verify-deep` runs the complete formal command.
- Missing Lean in the milestone environment fails closed with installation instructions; it is not silently skipped.

**Tests:**

- No project source contains an admission.
- `pixels-formal-scan` is platform portable.
- Formal build output is normalized before comparison.
- The ordinary Cargo dependency graph is unchanged.

**Focused checks:** Run the focused tests, fixtures, dumps, and commands named by this task before the repository gate.

**Repository gate:**

```text
cargo xtask verify
```

**Stop conditions:** Stop if an acceptance condition cannot be met without changing a closed architectural decision, weakening a proof/failure boundary, or adding an external Cargo dependency.

**Commit**

```text
pixels P0.3: pin the Lean verification project
```

## Task P0.4 — install the permanent renderer fixture corpus

**Requires:** P0.3.

**Produces:** Name every correctness class before implementation so later code cannot select only favorable examples.

**Files:**

```text
tests/golden/check-pixels-plane/ # new at P-1 basis
tests/golden/check-pixels-hard-csg/ # new at P-1 basis
tests/golden/check-pixels-smooth-csg/ # new at P-1 basis
tests/golden/check-pixels-repeat/ # new at P-1 basis
tests/golden/check-pixels-displace/ # new at P-1 basis
tests/golden/check-pixels-close-depth/ # new at P-1 basis
tests/golden/check-pixels-thin-feature/ # new at P-1 basis
tests/golden/check-pixels-enclosed-feature/ # new at P-1 basis
tests/golden/check-pixels-material-edge/ # new at P-1 basis
tests/golden/check-pixels-transparent-tail/ # new at P-1 basis
tests/golden/check-pixels-area-light/ # new at P-1 basis
tests/golden/check-pixels-kinetic/ # new at P-1 basis
tests/golden/err-pixels-unsupported-op/ # new at P-1 basis
tests/golden/err-pixels-missing-range/ # new at P-1 basis
tests/golden/err-pixels-rate/ # new at P-1 basis
tests/golden/err-pixels-topology-branch/ # new at P-1 basis
stdlib/tests/pixels_contract.wr # new at P-1 basis
tests/census.toml
tests/pixels-cases.txt # new at P-1 basis
```

**Contract/dump delta:** Only the contract and stable-dump changes explicitly named by this task are permitted.

**Work:**

- Add source fixtures with expected placeholder errors saying the production Pixels stage is not implemented.
- Create every fixture named in §11, not only the representative paths printed above, and record all names in both the existing census and `tests/pixels-cases.txt`.
- Geometry is deterministic and documented in a `README.md` inside each complex fixture.
- Thin/enclosed/close-depth cases use exact integer or dyadic source constants, not random placement.
- Add expected final-frame digest placeholders only where the golden harness already supports boot output; do not invent a second fixture system.

**Tests:**

- Every fixture is discovered by ordinary golden enumeration.
- Each adversarial scene states the failure class it protects.
- No fixture uses a dense edge mask or precomputed renderer hints as source input.
- The test census refuses accidental deletion.
- Plan lint proves the §11 fixture-name set, `tests/pixels-cases.txt`, and discovered golden directories are identical; later milestones replace placeholders without silently adding or dropping correctness classes.

**Focused checks:** Run the focused tests, fixtures, dumps, and commands named by this task before the repository gate.

**Repository gate:**

```text
cargo xtask verify
```

**Stop conditions:** Stop if an acceptance condition cannot be met without changing a closed architectural decision, weakening a proof/failure boundary, or adding an external Cargo dependency.

**Commit**

```text
pixels P0.4: pin the permanent renderer corpus
```

### Milestone P0 close

Run:

```text
cargo xtask verify
```

The milestone is closed when documentation, empty dumps, formal skeleton, and fixture census are all pinned. No placeholder may say “choose later”; it may only say “implemented in task Px.y.”

---

# Milestone P1 — source surface, semantic checks, and image declaration

Milestone result: Wrela can type-check the complete `@field`, `@material`, ranges/rates, and `Image.renderer` surface; the sealed image graph contains renderer declarations. This completes the minimal ABI proved in P-1.2. No general symbolic graph or production frame program is emitted yet.

## Task P1.1 — add standard-library field and renderer types

**Requires:** the preceding milestone close gate.

**Produces:** Make the source API parse and type-check using ordinary Wrela declarations while keeping constructors sealed.

**Files:**

```text
stdlib/core/field.wr # new at P-1 basis
stdlib/core/render.wr # new at P-1 basis
stdlib/tests/pixels_contract.wr # new at P-1 basis
```

**Contract/dump delta:** Only the contract and stable-dump changes explicitly named by this task are permitted.

**Work:**

Define:

- `Vec2`, `Vec3`, `Vec4`, `Rgb`, `Aabb`, `Camera`, `CameraBounds`;
- opaque `Field`;
- `ObjectId`, `MaterialId` as user enums accepted by `mark`;
- primitive/composition function signatures from §2.2;
- `SurfaceContext[M]`;
- `MaterialSample` closed constructors and `LightFrame`/`LightConfig` sealed fixed-capacity declarations;
- `ScalarRange`, `RgbRange`, `AoConfig`, and `ProbeConfig`;
- `RenderProfile`, `ToneCurve`, `RenderFrame[P]`, `RenderedFrame[P]`, `RenderError`;
- opaque `Renderer[P]` actor handle surface;
- `Image.renderer[P]` declaration signature in the image-builder surface.

Every compiler-recognized field constructor has real scalar Wrela semantics. No field operation may use `panic("compiler intrinsic")` as its normative body.

**Tests:**

- Source examples in §0 parse.
- User code cannot construct arbitrary `Field` storage or access its representation.
- `MaterialSample` constructors validate finite/clamped source arguments at runtime where appropriate.
- Existing imports see no ambiguous names.
- No compiler intrinsic exists solely because a normal Wrela body would suffice.

**Focused checks:** Run the focused tests, fixtures, dumps, and commands named by this task before the repository gate.

**Repository gate:**

```text
cargo xtask verify
```

**Stop conditions:** Stop if an acceptance condition cannot be met without changing a closed architectural decision, weakening a proof/failure boundary, or adding an external Cargo dependency.

**Commit**

```text
pixels P1.1: add field and render library surfaces
```

## Task P1.2 — classify `@field` and `@material` attributes

**Requires:** P1.1.

**Produces:** Carry annotations through declaration and typed-body checking.

**Files:**

```text
crates/wrela-compiler/src/sema/types.rs
crates/wrela-compiler/src/sema/bodies.rs
crates/wrela-compiler/src/sema/typed.rs
crates/wrela-compiler/src/sema/mod.rs
tests/golden/check-pixels-plane/ # new at P-1 basis
tests/golden/err-pixels-field-signature/ # new at P-1 basis
tests/golden/err-pixels-material-signature/ # new at P-1 basis
```

**Contract/dump delta:** Only the contract and stable-dump changes explicitly named by this task are permitted.

**Work:**

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
@material fn m(surface: SurfaceContext[M]) -> MaterialSample
@material fn m(surface: SurfaceContext[M], read params: P) -> MaterialSample
```

Parameter names are not semantic. Order and types are.

**Tests:**

- Every invalid shape has a focused golden with `P001` or `P002`.
- Typed dumps contain canonical root keys and parameter indexes.
- Generic helper instantiations retain root call resolution.
- Attribute handling is deterministic across module import order.

**Focused checks:** Run the focused tests, fixtures, dumps, and commands named by this task before the repository gate.

**Repository gate:**

```text
cargo xtask verify
```

**Stop conditions:** Stop if an acceptance condition cannot be met without changing a closed architectural decision, weakening a proof/failure boundary, or adding an external Cargo dependency.

**Commit**

```text
pixels P1.2: type field and material roots
```

## Task P1.3 — implement `@range` and `@rate` attributes

**Requires:** P1.2.

**Produces:** Capture the finite parameter domain needed by all later proofs.

**Files:**

```text
crates/wrela-compiler/src/sema/types.rs
crates/wrela-compiler/src/sema/bodies.rs
crates/wrela-compiler/src/sema/typed.rs
crates/wrela-compiler/src/sema/attrs.rs # new at P-1 basis
tests/golden/check-pixels-ranges/ # new at P-1 basis
tests/golden/err-pixels-range/ # new at P-1 basis
tests/golden/err-pixels-rate/ # new at P-1 basis
```

**Contract/dump delta:** Only the contract and stable-dump changes explicitly named by this task are permitted.

**Work:**

Create one reusable attribute parser for numeric named arguments. Accept attributes on scalar fields of data structs reachable from renderer parameter type `P`.

Rules:

- `@range` requires exactly `min` and `max`;
- values are comptime finite scalar constants convertible to the field scalar type;
- `min <= max`;
- integer ranges are exact;
- a vector field may use one component-wise shorthand; arrays and structs require annotations on each influencing scalar/vector field and do not inherit recursively;
- `@rate` requires `max_delta >= 0` and `max_second_delta >= 0`;
- rate units are per presented frame at declared `refresh_hz`;
- a zero rate means statically unchanged after initialization;
- attributes on non-render-parameter fields are legal but ignored by Pixels and still print in typed metadata only if semantically retained.

**Tests:**

- Range/rate metadata is keyed by stable field-index paths, not source field names alone.
- Rename with identical layout changes the source digest but not field-path ordering.
- NaN, infinity, reversed range, unknown label, duplicate label, and nonconstant values are rejected.
- Exact diagnostic spans point at the bad attribute argument.

**Focused checks:** Run the focused tests, fixtures, dumps, and commands named by this task before the repository gate.

**Repository gate:**

```text
cargo xtask verify
```

**Stop conditions:** Stop if an acceptance condition cannot be met without changing a closed architectural decision, weakening a proof/failure boundary, or adding an external Cargo dependency.

**Commit**

```text
pixels P1.3: record parameter range and rate contracts
```

## Task P1.4 — add closed field intrinsic typing

**Requires:** P1.3.

**Produces:** Recognize the field operation surface without allowing arbitrary `Field` manipulation.

**Files:**

```text
crates/wrela-compiler/src/sema/intrinsics.rs
crates/wrela-compiler/src/sema/bodies.rs
crates/wrela-compiler/src/sema/types.rs
stdlib/core/field.wr # new at P-1 basis
tests/golden/check-pixels-field-ops/ # new at P-1 basis
tests/golden/err-pixels-field-private/ # new at P-1 basis
```

**Contract/dump delta:** Only the contract and stable-dump changes explicitly named by this task are permitted.

**Work:**

- Add every field constructor/combinator to the written-down intrinsic census.
- Type labels and generic arguments exactly.
- `mark` accepts only comptime enum variants for object/material identity in v1.
- `finite_repeat_x/y/z` each encode exactly one axis and require positive finite period plus compile-time `first` and `count`; multidimensional repetition is explicit nesting in source order.
- transforms accept only rigid/uniform-scale operations in v1; nonuniform scale is a build error.
- public `sinusoidal_displace` lowers to internal `BoundedDisplace` with compiler-derived amplitude, gradient, Hessian, and third-derivative bounds; authors cannot supply these bounds.
- ordinary arithmetic on `Field` is unavailable.
- field values cannot be stored in user structs, arrays, statics, actors, messages, or returned from non-`@field` public APIs.

**Tests:**

- Intrinsic census equals all producer sites.
- Every field op is typed in one central match.
- `Field` cannot escape its root expression graph.
- Existing intrinsic diagnostics/census remain green.

**Focused checks:** Run the focused tests, fixtures, dumps, and commands named by this task before the repository gate.

**Repository gate:**

```text
cargo xtask verify
```

**Stop conditions:** Stop if an acceptance condition cannot be met without changing a closed architectural decision, weakening a proof/failure boundary, or adding an external Cargo dependency.

**Commit**

```text
pixels P1.4: seal and type the field operation set
```

## Task P1.5 — enforce field/material body legality

**Requires:** P1.4.

**Produces:** Reject source forms that prevent finite structural compilation.

**Files:**

```text
crates/wrela-compiler/src/pixels/legality.rs # new at P-1 basis
crates/wrela-compiler/src/sema/mod.rs
tests/golden/err-pixels-topology-branch/ # new at P-1 basis
tests/golden/err-pixels-field-recursion/ # new at P-1 basis
tests/golden/err-pixels-field-loop/ # new at P-1 basis
tests/golden/err-pixels-field-effects/ # new at P-1 basis
```

**Contract/dump delta:** Only the contract and stable-dump changes explicitly named by this task are permitted.

**Work:**

Walk transitive callees from each root and classify operations.

For field roots reject:

- recursion of any kind;
- `while`;
- `for` without a comptime exact extent;
- `await`, `send`, actor calls, groups;
- placed/static mutation, MMIO, entropy, time, panic on a reachable path;
- any runtime branch in a field root or helper whose condition depends on coordinate or parameter data;
- function values or indirect calls;
- dynamic indexing whose finite alternatives cannot be unrolled.

Material roots may branch on material identity, explicit event-classified scalar predicates, and ordinary bounded values. A material discontinuity affecting output must be surfaced later as an event predicate.

**Tests:**

- Every rejected effect names the transitive call chain.
- Recursive SCC diagnostics list all cycle members.
- Fixed loops are unrolled deterministically in source order.
- No body is accepted because its unsupported branch happened to be unreachable under one sample.

**Focused checks:** Run the focused tests, fixtures, dumps, and commands named by this task before the repository gate.

**Repository gate:**

```text
cargo xtask verify
```

**Stop conditions:** Stop if an acceptance condition cannot be met without changing a closed architectural decision, weakening a proof/failure boundary, or adding an external Cargo dependency.

**Commit**

```text
pixels P1.5: enforce finite renderer body legality
```

## Task P1.6 — implement `Image.renderer` intrinsic construction

**Requires:** P1.5.

**Produces:** Record renderer declarations through ordinary comptime image evaluation.

**Files:**

```text
crates/wrela-compiler/src/sema/intrinsics.rs
crates/wrela-compiler/src/sema/bodies.rs
crates/wrela-compiler/src/eval/interp.rs
crates/wrela-compiler/src/eval/image.rs
crates/wrela-compiler/src/eval/value.rs
crates/wrela-compiler/src/report.rs
tests/golden/check-pixels-plane/ # new at P-1 basis
tests/golden/err-pixels-renderer-decl/ # new at P-1 basis
```

**Contract/dump delta:** Only the contract and stable-dump changes explicitly named by this task are permitted.

**Work:**

- Add `Image.renderer` to the image-builder intrinsic surface and intrinsic census.
- Resolve `P` as the type argument.
- Preserve function references as `Value::Fn` in declaration args.
- Add `ImageDeclRef::Renderer` rendering and recursive declaration-reference scanning.
- Add renderer blocks to the `--stage=image` dump and ordinary report.
- `renderer.handle()` produces the typed deferred `Actor[Renderer[P]]` reference established in P-1.2. P5 resolves that declaration reference to a generated actor identity; P1 must not fake a numeric actor ID.

**Tests:**

- Multiple renderers preserve source construction order.
- Renderer declaration references participate in DAG cycle checks.
- Two renderers may share field/material functions but not claim the same display driver.
- Unknown/duplicate labels are rejected during sema/eval, not ignored.
- Image dump round-trips deterministic enum/function renderings.

**Focused checks:** Run the focused tests, fixtures, dumps, and commands named by this task before the repository gate.

**Repository gate:**

```text
cargo xtask verify
```

**Stop conditions:** Stop if an acceptance condition cannot be met without changing a closed architectural decision, weakening a proof/failure boundary, or adding an external Cargo dependency.

**Commit**

```text
pixels P1.6: record renderer image declarations
```

## Task P1.7 — validate sealed renderer declarations

**Requires:** P1.6.

**Produces:** Make an image with a renderer a closed, self-consistent build fact.

**Files:**

```text
crates/wrela-compiler/src/eval/image_checks.rs
crates/wrela-compiler/src/pixels/config.rs # new at P-1 basis
crates/wrela-compiler/src/pixels/diagnostics.rs # new at P-1 basis
tests/golden/err-pixels-renderer-display/ # new at P-1 basis
tests/golden/err-pixels-renderer-params/ # new at P-1 basis
tests/golden/err-pixels-renderer-mode/ # new at P-1 basis
```

**Contract/dump delta:** Only the contract and stable-dump changes explicitly named by this task are permitted.

**Work:**

Implement §6.4–6.5. Centralize enum variant decoding, integer/floating extraction, function-ref extraction, display reference validation, mode checks, and profile validation.

No renderer compilation occurs yet. `check_sealed` only returns a validated `RendererConfig` side table keyed by declaration index. Store it in a new build-closure structure rather than recomputing it from `ImageGraph` in every stage.

**Tests:**

- Every required argument has one diagnostic.
- The complete required label set is exactly §2.5, including camera/light/exposure/environment/AO/probe bounds and initialization deadline.
- Every function reference resolves to a matching annotated root.
- Cross-module roots work through the ordinary checked closure.
- Parameter type equality is structural/canonical, not string comparison.
- `refresh_hz % shade_hz != 0` is rejected.
- Non-finite camera/world/light values are rejected.

**Focused checks:** Run the focused tests, fixtures, dumps, and commands named by this task before the repository gate.

**Repository gate:**

```text
cargo xtask verify
```

**Stop conditions:** Stop if an acceptance condition cannot be met without changing a closed architectural decision, weakening a proof/failure boundary, or adding an external Cargo dependency.

**Commit**

```text
pixels P1.7: seal renderer configuration contracts
```

## Task P1.8 — complete P1 dumps and fixtures

**Requires:** P1.7.

**Produces:** Pin source/typed/image behavior before symbolic compilation.

**Files:**

```text
crates/wrela-compiler/src/sema/typed.rs
crates/wrela-compiler/src/eval/image.rs
crates/wrela-compiler/src/report.rs
tests/golden/check-pixels-*/expected/check.txt # new at P-1 basis
tests/golden/check-pixels-*/expected/typed.txt # new at P-1 basis
tests/golden/check-pixels-*/expected/image.txt # new at P-1 basis
```

**Contract/dump delta:** Only the contract and stable-dump changes explicitly named by this task are permitted.

**Work:**

- Update only P1-relevant fixtures.
- Add unit tests for renderer ordering, function references, path metadata, and construction DAG behavior.
- Add a report-determinism case with two modules and two renderer declarations.
- Ensure no field/frame-program dump contains implementation data yet; it prints `Compilation status=not-run` with renderer config, not placeholder failure.

**Tests:**

- P1 accepted fixtures reach sealed image configuration.
- All P1 rejected fixtures fail before lower/codegen.
- Report determinism passes under reversed filesystem/module discovery order.
- Existing non-Pixels image goldens change only where enum rendering gained `Renderer` support, with reviewed diffs.

**Focused checks:** Run the focused tests, fixtures, dumps, and commands named by this task before the repository gate.

**Repository gate:**

```text
cargo xtask verify
cargo xtask report-determinism
```

**Stop conditions:** Stop if an acceptance condition cannot be met without changing a closed architectural decision, weakening a proof/failure boundary, or adding an external Cargo dependency.

**Commit**

```text
pixels P1.8: pin renderer source and image dumps
```

### Milestone P1 close

Run `cargo xtask verify`. The milestone is closed only when the complete source API type-checks and every bad declaration fails before symbolic compilation.

---

# Milestone P2 — dedicated symbolic field and material compiler

Milestone result: accepted roots compile into deterministic scalar, field, and material graphs with exact source identities. No geometric bounds, feature decomposition, or binary frame program exists yet.

## Task P2.1 — implement stable arenas and IDs

**Requires:** the preceding milestone close gate.

**Produces:** Create deterministic storage for all symbolic nodes.

**Files:**

```text
crates/wrela-compiler/src/pixels/ids.rs # new at P-1 basis
crates/wrela-compiler/src/pixels/arena.rs # new at P-1 basis
crates/wrela-compiler/src/pixels/scalar.rs # new at P-1 basis
crates/wrela-compiler/src/pixels/graph.rs # new at P-1 basis
crates/wrela-compiler/src/pixels/material_graph.rs # new at P-1 basis
crates/wrela-compiler/src/pixels/mod.rs # new at P-1 basis
```

**Contract/dump delta:** Only the contract and stable-dump changes explicitly named by this task are permitted.

**Work:**

Implement newtype IDs and append-only arenas. IDs are assigned only after child nodes exist. Provide checked getters returning internal errors, not panics on user-triggerable paths.

Node equality for canonicalization is structural and excludes source span. Keep `NodeOrigin { primary, expansion_chain }` in a side table keyed by ID.

**Tests:**

- IDs format exactly as §4.1.
- Arena iteration is insertion order.
- Origin side tables cover every node.
- No node owns a `HashMap`, `Rc`, `Arc`, trait object, or closure.
- Unit tests cover stale/wrong-arena ID detection in debug helpers.

**Focused checks:** Run the focused tests, fixtures, dumps, and commands named by this task before the repository gate.

**Repository gate:**

```text
cargo xtask verify
```

**Stop conditions:** Stop if an acceptance condition cannot be met without changing a closed architectural decision, weakening a proof/failure boundary, or adding an external Cargo dependency.

**Commit**

```text
pixels P2.1: add deterministic symbolic arenas
```

## Task P2.2 — implement scalar symbolic values

**Requires:** P2.1.

**Produces:** Compile ordinary scalar/vector expressions referenced by field/material operations.

**Files:**

```text
crates/wrela-compiler/src/pixels/symbolic.rs # new at P-1 basis
crates/wrela-compiler/src/pixels/scalar.rs # new at P-1 basis
crates/wrela-compiler/src/pixels/params.rs # new at P-1 basis
crates/wrela-compiler/src/pixels/diagnostics.rs # new at P-1 basis
```

**Contract/dump delta:** Only the contract and stable-dump changes explicitly named by this task are permitted.

**Work:**

Implement `SymValue` from §4.2 and scalar node kinds for:

- all scalar constants preserving exact source f32/f64 bits;
- parameter field paths;
- vector construction/projection;
- checked and wrapping integer arithmetic where used for compile-time indexing;
- float add/sub/mul/div/neg;
- min/max/abs/clamp;
- sqrt/rsqrt/sin/cos with fixed semantic op IDs;
- dot, cross, length, normalize as fused scalar/vector nodes;
- comparisons used only in material event branches;
- tuple/struct temporary values needed by helper functions.

Parameter paths are resolved through typed field indices. Store human spelling only in diagnostics/dumps.

**Tests:**

- Constant bit patterns, including negative zero, survive the graph dump.
- Parameter path collection is exact and deterministic.
- Unsupported scalar operation reports `P004` with call chain.
- Division/reciprocal records a denominator proof obligation; it is not assumed nonzero.
- Fused operations retain source-level semantics through explicit op definitions.

**Focused checks:** Run the focused tests, fixtures, dumps, and commands named by this task before the repository gate.

**Repository gate:**

```text
cargo xtask verify
```

**Stop conditions:** Stop if an acceptance condition cannot be met without changing a closed architectural decision, weakening a proof/failure boundary, or adding an external Cargo dependency.

**Commit**

```text
pixels P2.2: compile scalar renderer expressions
```

## Task P2.3 — implement symbolic call/control evaluation

**Requires:** P2.2.

**Produces:** Evaluate renderer roots and helpers without reusing generic comptime values.

**Files:**

```text
crates/wrela-compiler/src/pixels/symbolic.rs # new at P-1 basis
crates/wrela-compiler/src/pixels/legality.rs # new at P-1 basis
crates/wrela-compiler/src/pixels/quota.rs # new at P-1 basis
```

**Contract/dump delta:** Only the contract and stable-dump changes explicitly named by this task are permitted.

**Work:**

Implement:

- lexical environment stack;
- typed local assignment and immutable value semantics;
- direct calls by `CalleeKey`;
- canonical generic instance lookup;
- `let`, expression statement, return;
- `if` with compile-time condition;
- exact bounded `for` unrolling;
- `match` over compile-time enum or explicit material/object identity;
- call-depth, node-count, loop-expansion, and symbolic-memory quotas;
- error stack preserving root/helper call chain.

No `while`, `await`, send, closure invocation, mutation through aliases, or exception-like recovery.

**Tests:**

- The evaluator is total over the accepted legality subset.
- Quota exhaustion is a `pixels` build error, not panic or partial graph.
- Identical helper calls can later CSE but preserve all source origins.
- A field runtime branch is rejected before evaluation; material branch arms are both compiled and neither is selected by a sample value.
- Fixed loop expansion order is ascending source iteration order.

**Focused checks:** Run the focused tests, fixtures, dumps, and commands named by this task before the repository gate.

**Repository gate:**

```text
cargo xtask verify
```

**Stop conditions:** Stop if an acceptance condition cannot be met without changing a closed architectural decision, weakening a proof/failure boundary, or adding an external Cargo dependency.

**Commit**

```text
pixels P2.3: evaluate finite renderer bodies symbolically
```

## Task P2.4 — lower field operations into `FieldGraph`

**Requires:** P2.3.

**Produces:** Build the exact structural field expression from the closed intrinsic surface.

**Files:**

```text
crates/wrela-compiler/src/pixels/symbolic.rs # new at P-1 basis
crates/wrela-compiler/src/pixels/graph.rs # new at P-1 basis
crates/wrela-compiler/src/pixels/field_intrinsics.rs # new at P-1 basis
```

**Contract/dump delta:** Only the contract and stable-dump changes explicitly named by this task are permitted.

**Work:**

For every field intrinsic, parse labels once and emit the canonical `FieldKind` from §4.4. Primitive details use §4.5; do not copy a second shape into this task. `sinusoidal_displace` emits `BoundedDisplace` with `DerivedDeformContract`.

Do not immediately flatten transforms or marks. Preserve source structure.

**Tests:**

- Every closed field op has one lowering path and one unit test.
- Missing/duplicate labels cannot reach this pass.
- Object/material keys remain nominal enum identity, not bit masks.
- Transform composition preserves source order.
- Field graph can represent the complete permanent fixture source set.

**Focused checks:** Run the focused tests, fixtures, dumps, and commands named by this task before the repository gate.

**Repository gate:**

```text
cargo xtask verify
```

**Stop conditions:** Stop if an acceptance condition cannot be met without changing a closed architectural decision, weakening a proof/failure boundary, or adding an external Cargo dependency.

**Commit**

```text
pixels P2.4: lower the closed field graph
```

## Task P2.5 — lower material operations into `MaterialGraph`

**Requires:** P2.4.

**Produces:** Capture material semantics and output dependencies structurally.

**Files:**

```text
crates/wrela-compiler/src/pixels/material_graph.rs # new at P-1 basis
crates/wrela-compiler/src/pixels/material_intrinsics.rs # new at P-1 basis
crates/wrela-compiler/src/pixels/symbolic.rs # new at P-1 basis
```

**Contract/dump delta:** Only the contract and stable-dump changes explicitly named by this task are permitted.

**Work:**

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

**Tests:**

- Every `MaterialSample` field has a graph source or constructor default.
- Alpha is clamped only according to source constructor semantics, not compiler convenience.
- Material identity match compiles into a finite table keyed by nominal variant.
- Unsupported procedural texture or indirect call is `P004`/`P014`.
- Graph dump identifies all parameter and surface-context dependencies.

**Focused checks:** Run the focused tests, fixtures, dumps, and commands named by this task before the repository gate.

**Repository gate:**

```text
cargo xtask verify
```

**Stop conditions:** Stop if an acceptance condition cannot be met without changing a closed architectural decision, weakening a proof/failure boundary, or adding an external Cargo dependency.

**Commit**

```text
pixels P2.5: compile material semantics
```

## Task P2.6 — canonicalize and CSE symbolic graphs

**Requires:** P2.5.

**Produces:** Produce one deterministic graph independent of helper inlining accidents while preserving exact semantics.

**Files:**

```text
crates/wrela-compiler/src/pixels/canonicalize.rs # new at P-1 basis
crates/wrela-compiler/src/pixels/scalar.rs # new at P-1 basis
crates/wrela-compiler/src/pixels/graph.rs # new at P-1 basis
crates/wrela-compiler/src/pixels/material_graph.rs # new at P-1 basis
```

**Contract/dump delta:** Only the contract and stable-dump changes explicitly named by this task are permitted.

**Work:**

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

Use a deterministic structural key enum, not serialized debug text.

**Tests:**

- Canonicalization is idempotent.
- Running it twice produces byte-identical dumps and IDs.
- Differential unit tests compare pre/post scalar evaluation over deterministic input grids.
- Smooth-min one-ulp/saturation fixtures remain exact.
- All coordinate/parameter-dependent field branches fail with `P003`, including equal-topology arms.

**Focused checks:** Run the focused tests, fixtures, dumps, and commands named by this task before the repository gate.

**Repository gate:**

```text
cargo xtask verify
```

**Stop conditions:** Stop if an acceptance condition cannot be met without changing a closed architectural decision, weakening a proof/failure boundary, or adding an external Cargo dependency.

**Commit**

```text
pixels P2.6: canonicalize renderer graphs exactly
```

## Task P2.7 — emit complete `field-graph` dumps

**Requires:** P2.6.

**Produces:** Pin the symbolic representation before geometric proof work.

**Files:**

```text
crates/wrela-compiler/src/pixels/dump.rs # new at P-1 basis
crates/wrela-compiler/src/bin/wrela.rs
tests/golden/check-pixels-*/expected/field-graph.txt # new at P-1 basis
```

**Contract/dump delta:** Only the contract and stable-dump changes explicitly named by this task are permitted.

**Work:**

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

**Tests:**

- All accepted permanent fixtures produce stable graph dumps.
- Unsupported fixtures fail before a partial dump is emitted.
- Reordering independent source helper declarations does not change canonical node order when call graph/semantics are unchanged.
- Round-trip float formatting reproduces bits in a parser unit test.

**Focused checks:** Run the focused tests, fixtures, dumps, and commands named by this task before the repository gate.

**Repository gate:**

```text
cargo xtask verify
cargo xtask report-determinism
```

**Stop conditions:** Stop if an acceptance condition cannot be met without changing a closed architectural decision, weakening a proof/failure boundary, or adding an external Cargo dependency.

**Commit**

```text
pixels P2.7: pin symbolic field and material dumps
```

### Milestone P2 close

Run `cargo xtask verify`. The graph dump becomes a compatibility artifact; later changes must be deliberate and reviewed.

---

# Milestone P3 — structural proofs, bounds, objects, features, and capacities

Milestone result: every field/material graph is converted into a finite structural scene with conservative parameter/value/derivative bounds, complete smooth-CSG support shells, maximal smooth objects, explicit hard-CSG logic, fused primitive features, and exact compile-time capacity derivations. Projective equations and runtime events are not yet emitted.

## Task P3.1 — collect exact renderer parameter dependencies

**Requires:** the preceding milestone close gate.

**Produces:** Determine the smallest coefficient snapshot and the complete frame dependency tuple.

**Files:**

```text
crates/wrela-compiler/src/pixels/params.rs # new at P-1 basis
crates/wrela-compiler/src/pixels/report.rs # new at P-1 basis
crates/wrela-compiler/src/pixels/dump.rs # new at P-1 basis
```

**Contract/dump delta:** Only the contract and stable-dump changes explicitly named by this task are permitted.

**Work:**

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

**Tests:**

- A parameter used in both geometry and material has one slot and two uses.
- Unused fields contribute zero renderer-state bytes.
- Every referenced numeric path has `@range`; missing `@rate` merely disables kinetic reuse for changes to that path.
- Static zero-rate parameters are marked immutable and may be folded later.
- Dependency dump is independent of source field spelling.

**Focused checks:** Run the focused tests, fixtures, dumps, and commands named by this task before the repository gate.

**Repository gate:**

```text
cargo xtask verify
```

**Stop conditions:** Stop if an acceptance condition cannot be met without changing a closed architectural decision, weakening a proof/failure boundary, or adding an external Cargo dependency.

**Commit**

```text
pixels P3.1: derive renderer coefficient dependencies
```

## Task P3.2 — propagate scalar value intervals

**Requires:** P3.1.

**Produces:** Give every scalar/vector node a conservative finite range over the complete declared parameter/world domain.

**Files:**

```text
crates/wrela-compiler/src/pixels/bounds.rs # new at P-1 basis
crates/wrela-compiler/src/pixels/scalar.rs # new at P-1 basis
crates/wrela-compiler/src/pixels/reference/interval.rs # new at P-1 basis
formal/pixels/Pixels/Interval.lean # new at P-1 basis
```

**Contract/dump delta:** Only the contract and stable-dump changes explicitly named by this task are permitted.

**Work:**

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

**Tests:**

- 100,000 deterministic random point checks per operation are permanent bug-finders.
- Analytic edge cases cover signed zero, subnormal, extrema, critical trigonometric points, reciprocal near zero, and normalization near zero.
- Range propagation is one topological pass.
- No interval intersection feeds back into predecessor values.
- Lean interval module proves the abstract operations used by runtime; compiler f64 implementation has differential containment tests.

**Focused checks:** Run the focused tests, fixtures, dumps, and commands named by this task before the repository gate.

**Repository gate:**

```text
cargo xtask verify
cargo xtask pixels-formal
```

**Stop conditions:** Stop if an acceptance condition cannot be met without changing a closed architectural decision, weakening a proof/failure boundary, or adding an external Cargo dependency.

**Commit**

```text
pixels P3.2: propagate conservative scalar ranges
```

## Task P3.3 — propagate derivative and Lipschitz bounds

**Requires:** P3.2.

**Produces:** Compute local/global bounds required by root isolation, continuation, displacement, filtering, and kinetic transport.

**Files:**

```text
crates/wrela-compiler/src/pixels/derivative_bounds.rs # new at P-1 basis
crates/wrela-compiler/src/pixels/bounds.rs # new at P-1 basis
formal/pixels/Pixels/SmoothMin.lean # new at P-1 basis
```

**Contract/dump delta:** Only the contract and stable-dump changes explicitly named by this task are permitted.

**Work:**

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

**Tests:**

- Melee-like nested one-Lipschitz primitives derive `L <= 1` absent displacement/scale.
- The displacement fixture derives its explicit bound from declared frequencies/amplitudes, not global `4`.
- Every derivative bound has a source rule ID in the dump.
- Randomized gradient/Hessian samples never exceed the bound.
- Kink-containing domains are marked nonsmooth rather than assigned arbitrary derivatives.

**Focused checks:** Run the focused tests, fixtures, dumps, and commands named by this task before the repository gate.

**Repository gate:**

```text
cargo xtask verify
cargo xtask pixels-formal
```

**Stop conditions:** Stop if an acceptance condition cannot be met without changing a closed architectural decision, weakening a proof/failure boundary, or adding an external Cargo dependency.

**Commit**

```text
pixels P3.3: derive renderer derivative contracts
```

## Task P3.4 — compute structural world bounds

**Requires:** P3.3.

**Produces:** Bound every field subtree and primitive independently of a screen sample.

**Files:**

```text
crates/wrela-compiler/src/pixels/world_bounds.rs # new at P-1 basis
crates/wrela-compiler/src/pixels/graph.rs # new at P-1 basis
crates/wrela-compiler/src/pixels/diagnostics.rs # new at P-1 basis
```

**Contract/dump delta:** Only the contract and stable-dump changes explicitly named by this task are permitted.

**Work:**

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

**Tests:**

- `control-enclosed-feature` is discoverable solely from its primitive bound.
- Thin features retain nonempty conservative bounds even below one output pixel.
- Repeat instance enumeration is exact and finite.
- Empty intersections are pruned with a stable reason.
- Unbounded geometry outside explicit world clipping is rejected with `P012`/`P016` as appropriate.

**Focused checks:** Run the focused tests, fixtures, dumps, and commands named by this task before the repository gate.

**Repository gate:**

```text
cargo xtask verify
```

**Stop conditions:** Stop if an acceptance condition cannot be met without changing a closed architectural decision, weakening a proof/failure boundary, or adding an external Cargo dependency.

**Commit**

```text
pixels P3.4: derive complete structural world bounds
```

## Task P3.5 — propagate smooth-CSG support budgets

**Requires:** P3.4.

**Produces:** Prove that expanded leaf shells form a complete candidate source for smooth composites.

**Files:**

```text
crates/wrela-compiler/src/pixels/support.rs # new at P-1 basis
crates/wrela-compiler/src/pixels/graph.rs # new at P-1 basis
formal/pixels/Pixels/SupportTree.lean # new at P-1 basis
```

**Contract/dump delta:** Only the contract and stable-dump changes explicitly named by this task are permitted.

**Work:**

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

Balance only compiler-generated associative smooth trees if source bit semantics explicitly define reassociation. For v1, preserve authored tree and report its maximum support depth; do not silently reassociate.

**Tests:**

- Every smooth composite zero has at least one leaf shell in the formal model.
- Every smooth composite zero lies in a `SmoothObjectRootProgram` domain whose full composed scalar is isolated; completeness never assumes a primitive leaf is zero.
- Per-leaf shell expansion is finite.
- Gap-sensitive programs never exceed the coarse max budget.
- Unit tests cover nested, saturated, equal-child, and varying-k trees.
- Dump prints the support path producing each maximum.

**Focused checks:** Run the focused tests, fixtures, dumps, and commands named by this task before the repository gate.

**Repository gate:**

```text
cargo xtask verify
cargo xtask pixels-formal
```

**Stop conditions:** Stop if an acceptance condition cannot be met without changing a closed architectural decision, weakening a proof/failure boundary, or adding an external Cargo dependency.

**Commit**

```text
pixels P3.5: prove smooth surface support shells
```

## Task P3.6 — partition maximal smooth objects and compile hard CSG

**Requires:** P3.5.

**Produces:** Separate local smooth root problems from global Boolean occupancy.

**Files:**

```text
crates/wrela-compiler/src/pixels/objects.rs # new at P-1 basis
crates/wrela-compiler/src/pixels/csg.rs # new at P-1 basis
crates/wrela-compiler/src/pixels/graph.rs # new at P-1 basis
formal/pixels/Pixels/Csg.lean # new at P-1 basis
```

**Contract/dump delta:** Only the contract and stable-dump changes explicitly named by this task are permitted.

**Work:**

Partition at hard operations:

- a maximal subtree containing primitives, transforms, structurally enumerated finite-repeat instances, compiler-derived bounded displacement, and smooth min/max is one `SmoothObject`;
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

**Tests:**

- CSG stack depth is computed exactly and finite.
- Compiler tree evaluation and stack program agree exhaustively for up to 12 objects and deterministically sampled assignments beyond that.
- Hard union of marked objects preserves independent identity until the visible crossing is selected.
- Smooth blends remain within one object and are not represented as Boolean toggles.
- Object ordering is stable by canonical root structural key then source origin.
- P3.6 performs the finite structural instance enumeration required for partitioning; P3.8 later compiles the reusable projective/event templates and derived numeric bounds for those already-enumerated instances.

**Focused checks:** Run the focused tests, fixtures, dumps, and commands named by this task before the repository gate.

**Repository gate:**

```text
cargo xtask verify
cargo xtask pixels-formal
```

**Stop conditions:** Stop if an acceptance condition cannot be met without changing a closed architectural decision, weakening a proof/failure boundary, or adding an external Cargo dependency.

**Commit**

```text
pixels P3.6: partition smooth objects and compile CSG occupancy
```

## Task P3.7 — decompose fused primitive features

**Requires:** P3.6.

**Produces:** Replace expanded SDF kinks with exact geometric features and validity predicates.

**Files:**

```text
crates/wrela-compiler/src/pixels/features.rs # new at P-1 basis
crates/wrela-compiler/src/pixels/primitive.rs # new at P-1 basis
crates/wrela-compiler/src/pixels/graph.rs # new at P-1 basis
```

**Contract/dump delta:** Only the contract and stable-dump changes explicitly named by this task are permitted.

**Work:**

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

**Tests:**

- Union of feature-validity domains covers each primitive boundary.
- No feature domain includes a geometrically different primitive branch.
- Rounded-box face interiors no longer carry artificial `abs/max/sqrt` derivative ambiguity.
- Feature count and bound expansion are exact in dumps.
- Primitive scalar reference remains available for semantic validation.

**Focused checks:** Run the focused tests, fixtures, dumps, and commands named by this task before the repository gate.

**Repository gate:**

```text
cargo xtask verify
```

**Stop conditions:** Stop if an acceptance condition cannot be met without changing a closed architectural decision, weakening a proof/failure boundary, or adding an external Cargo dependency.

**Commit**

```text
pixels P3.7: compile fused surface features
```

## Task P3.8 — compile repetition and bounded deformation templates

**Requires:** P3.7.

**Produces:** Make discontinuous coordinate wrapping and analytic deformation explicit rather than opaque range operations.

**Files:**

```text
crates/wrela-compiler/src/pixels/repeat.rs # new at P-1 basis
crates/wrela-compiler/src/pixels/deform.rs # new at P-1 basis
crates/wrela-compiler/src/pixels/features.rs # new at P-1 basis
```

**Contract/dump delta:** Only the contract and stable-dump changes explicitly named by this task are permitted.

**Work:**

For repetition:

- enumerate all integer instance indices whose expanded bounds intersect the world AABB over all parameter ranges;
- instantiate affine translation coefficient programs;
- emit wrap-boundary event families only where a moving camera/parameter domain can cross an instance boundary;
- reject a count exceeding the sealed instance ceiling;
- never evaluate a certificate over a domain spanning two instance indices.

For bounded displacement:

- store compiler-derived amplitude, gradient, Hessian, and third-derivative bounds;
- retain base feature projective equation as predictor;
- compile displacement value/derivative programs;
- expand bounds exactly once by amplitude;
- lower every public `sinusoidal_displace` form to the closed derivation named by `DerivedDeformContract`;
- reject any arbitrary scalar helper or author-supplied bound.

**Tests:**

- Repeat fixture contains no runtime modulo/floor inside a fixed instance certificate.
- Instance ordering and IDs are deterministic for negative indices.
- Displacement contracts are derived from source frequency/amplitude constants and checked against the independent reference implementation.
- There is no user contract to understate in v1; an unsupported custom deformation is a build error.
- Cross-wrap domains always create event/split obligations.

**Focused checks:** Run the focused tests, fixtures, dumps, and commands named by this task before the repository gate.

**Repository gate:**

```text
cargo xtask verify
```

**Stop conditions:** Stop if an acceptance condition cannot be met without changing a closed architectural decision, weakening a proof/failure boundary, or adding an external Cargo dependency.

**Commit**

```text
pixels P3.8: compile finite repeats and bounded deformations
```

## Task P3.9 — compile material discontinuity obligations

**Requires:** P3.8.

**Produces:** Ensure depth-smooth surfaces cannot hide output-discontinuous material changes.

**Files:**

```text
crates/wrela-compiler/src/pixels/material.rs # new at P-1 basis
crates/wrela-compiler/src/pixels/material_graph.rs # new at P-1 basis
crates/wrela-compiler/src/pixels/objects.rs # new at P-1 basis
```

**Contract/dump delta:** Only the contract and stable-dump changes explicitly named by this task are permitted.

**Work:**

Classify material graph predicates:

- nominal material identity selection: already tied to geometric identity event;
- threshold on scalar surface/world/parameter expression: emit explicit material event predicate;
- smooth blend/select: compile value/derivative bound and no topological event;
- procedural wrap/step: emit finite period/threshold events when bounded; otherwise reject;
- texture lookup: v1 supports only immutable compiler-known textures with explicit filter and finite dimensions; discontinuities at texel/filter boundaries are represented in the shading error bound, not geometry identity.

Attach event obligations to the owning smooth object/feature set. Material events split shading runs but do not insert geometry roots.

**Tests:**

- `control-material-edge` is visible in the structural event set before rendering.
- A material threshold with unknown finite crossing count is `P014`.
- Smooth material expressions do not create unnecessary hard event records.
- Material event identity is stable and source-spanned.

**Focused checks:** Run the focused tests, fixtures, dumps, and commands named by this task before the repository gate.

**Repository gate:**

```text
cargo xtask verify
```

**Stop conditions:** Stop if an acceptance condition cannot be met without changing a closed architectural decision, weakening a proof/failure boundary, or adding an external Cargo dependency.

**Commit**

```text
pixels P3.9: expose material discontinuities structurally
```

## Task P3.10 — derive exact structural capacities

**Requires:** P3.9.

**Produces:** Reserve all later frame-program and runtime storage without runtime allocation.

**Files:**

```text
crates/wrela-compiler/src/pixels/capacities.rs # new at P-1 basis
crates/wrela-compiler/src/pixels/report.rs # new at P-1 basis
formal/pixels/Pixels/Capacity.lean # new at P-1 basis
bench/thresholds.toml
```

**Contract/dump delta:** Only the contract and stable-dump changes explicitly named by this task are permitted.

**Work:**

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
- fixed-schema diagnostic/conformance certificate-telemetry counter bytes,
  with zero bytes in the uninstrumented production variant;
- output tile and double-buffer bytes;
- probe bytes;
- kinetic certificate bytes.

Where an exact geometric overlap count is expensive, use a proven conservative endpoint sweep over projected bounding intervals. Do not substitute a hand-authored cap without reporting how it was derived.

Define machine-v1 ceilings in one `PixelsCeilings` struct and mirror them in the spec. A build above a ceiling fails with `P015` and why-chain.

**Tests:**

- Every runtime vector/array bound in later Wrela modules traces to one capacity field.
- Arithmetic overflow is a build error before comparison to ceilings.
- Capacity values appear in field-graph dump and report.
- Instrumented and uninstrumented telemetry workspace bytes derive from the
  versioned schema enum counts, never observed scene data.
- A deliberately oversized fixture fails at compile time with exact contributors.
- Formal capacity lemmas build.

**Focused checks:** Run the focused tests, fixtures, dumps, and commands named by this task before the repository gate.

**Repository gate:**

```text
cargo xtask verify
cargo xtask pixels-formal
```

**Stop conditions:** Stop if an acceptance condition cannot be met without changing a closed architectural decision, weakening a proof/failure boundary, or adding an external Cargo dependency.

**Commit**

```text
pixels P3.10: seal structural renderer capacities
```

## Task P3.11 — add the structural program verifier

**Requires:** P3.10.

**Produces:** Refuse incomplete or internally inconsistent scene structures before projective/runtime lowering.

**Files:**

```text
crates/wrela-compiler/src/pixels/verify.rs # new at P-1 basis
crates/wrela-compiler/src/pixels/mod.rs # new at P-1 basis
```

**Contract/dump delta:** Only the contract and stable-dump changes explicitly named by this task are permitted.

**Work:**

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

**Tests:**

- Corruption unit tests remove each required record and receive a specific internal invariant error.
- Verification order is deterministic and reports the lowest stable offending ID.
- No downstream P4 function accepts an unverified mutable compiler context.
- Valid permanent fixtures pass.

**Focused checks:** Run the focused tests, fixtures, dumps, and commands named by this task before the repository gate.

**Repository gate:**

```text
cargo xtask verify
```

**Stop conditions:** Stop if an acceptance condition cannot be met without changing a closed architectural decision, weakening a proof/failure boundary, or adding an external Cargo dependency.

**Commit**

```text
pixels P3.11: verify complete structural renderer programs
```

## Task P3.12 — extend graph dumps with structural proofs

**Requires:** P3.11.

**Produces:** Pin all P3 facts before projective lowering.

**Files:**

```text
crates/wrela-compiler/src/pixels/dump.rs # new at P-1 basis
tests/golden/check-pixels-*/expected/field-graph.txt # new at P-1 basis
```

**Contract/dump delta:** Only the contract and stable-dump changes explicitly named by this task are permitted.

**Work:**

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

**Tests:**

- Every structural proof object has a dump representation.
- Dump has no `pending` line after P3.
- Permanent fixtures pin the intended object/feature/event-obligation counts.
- Report determinism remains green.

**Focused checks:** Run the focused tests, fixtures, dumps, and commands named by this task before the repository gate.

**Repository gate:**

```text
cargo xtask verify
cargo xtask report-determinism
```

**Stop conditions:** Stop if an acceptance condition cannot be met without changing a closed architectural decision, weakening a proof/failure boundary, or adding an external Cargo dependency.

**Commit**

```text
pixels P3.12: pin structural proof dumps
```

### Milestone P3 close

Run `cargo xtask verify`. Do not start projective lowering until all accepted fixtures have a verified structural program and all runtime storage classes have finite capacities.

---

# Milestone P4 — projective surface programs and complete local event generators

Milestone result: the verified structural scene compiles into inverse-view-depth feature equations, derivative/Taylor programs, conservative projected spans, and a complete finite set of local runtime event generators plus explicit exclusion records. No global aspect graph or whole-scene resultant is emitted.

## Task P4.1 — define the camera/projective coefficient model

**Requires:** the preceding milestone close gate.

**Produces:** Use one camera algebra throughout compiler, runtime, reference, and Lean.

**Files:**

```text
crates/wrela-compiler/src/pixels/projective.rs # new at P-1 basis
crates/wrela-compiler/src/pixels/camera.rs # new at P-1 basis
stdlib/core/render.wr # new at P-1 basis
formal/pixels/Pixels/Projective.lean # new at P-1 basis
```

**Contract/dump delta:** Only the contract and stable-dump changes explicitly named by this task are permitted.

**Work:**

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

**Tests:**

- Plane inverse depth is affine in `(u,v)` in compiler and formal model.
- Projective cancellation theorem builds.
- Screen-coordinate convention has permanent corner/center tests.
- Camera handedness and y direction match framebuffer goldens.
- Zero/degenerate up vectors are rejected at source/build boundary.

**Focused checks:** Run the focused tests, fixtures, dumps, and commands named by this task before the repository gate.

**Repository gate:**

```text
cargo xtask verify
cargo xtask pixels-formal
```

**Stop conditions:** Stop if an acceptance condition cannot be met without changing a closed architectural decision, weakening a proof/failure boundary, or adding an external Cargo dependency.

**Commit**

```text
pixels P4.1: fix the inverse-view-depth camera model
```

## Task P4.2 — implement bounded polynomial and rational programs

**Requires:** P4.1.

**Produces:** Represent only the low-degree projective equations and predicates the runtime needs.

**Files:**

```text
crates/wrela-compiler/src/pixels/polynomial.rs # new at P-1 basis
crates/wrela-compiler/src/pixels/program.rs # new at P-1 basis
crates/wrela-compiler/src/pixels/reference/polynomial.rs # new at P-1 basis
formal/pixels/Pixels/Bernstein.lean # new at P-1 basis
```

**Contract/dump delta:** Only the contract and stable-dump changes explicitly named by this task are permitted.

**Work:**

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

Add a deterministic fixed-shape composition planner for polynomial root/tube
specializations. Given a bounded `PolyProgram` and a quadratic `q_hat`, it
emits a straight-line schedule for the two constant-correction face
polynomials and records source degrees, composed degree, term count, temporary
count, and coefficient order. A shape is specialization-eligible only when the
composed univariate degree remains ≤ 8 and every term/temporary fits a sealed
ceiling. Otherwise retain the ordinary interval/Taylor program. Exceeding a
specialization shape is never by itself a source rejection.

The schedule is structural rather than numeric: runtime `q_hat` and coefficient
values remain operands. It may share lifted coefficient products explicitly,
but it must not describe verifier work as floating GEMV/FMA or introduce a
generic computer algebra engine.

For local scanline composition the affine coordinate substitution is
`U = u0 + X`, in addition to `Q = q_hat(X)`. `degree_u` therefore contributes
to composed degree, temporary accounting, and every correction-face
coefficient expansion.

**Tests:**

- Polynomial arithmetic is deterministic and checked for degree/term overflow.
- Different construction orders canonicalize identically.
- Horner and direct-sum reference evaluation agree over permanent fixtures.
- Bernstein conversion supports the fixed degree set and passes formal coefficient enclosure tests.
- Composition schedules agree with direct symbolic substitution for every
  supported degree shape and are deterministic across construction order.
- An over-degree composition selects the interval/Taylor fallback without
  changing source acceptance.
- Exceeding a degree/term ceiling is `P004`/`P015`, not truncation.

**Focused checks:** Run the focused tests, fixtures, dumps, and commands named by this task before the repository gate.

**Repository gate:**

```text
cargo xtask verify
cargo xtask pixels-formal
```

**Stop conditions:** Stop if an acceptance condition cannot be met without changing a closed architectural decision, weakening a proof/failure boundary, or adding an external Cargo dependency.

**Commit**

```text
pixels P4.2: add bounded projective polynomial programs
```

## Task P4.3 — compile projective equations for planar and quadric features

**Requires:** P4.2.

**Produces:** Eliminate routine field marching for the most common feature classes.

**Files:**

```text
crates/wrela-compiler/src/pixels/projective.rs # new at P-1 basis
crates/wrela-compiler/src/pixels/primitive.rs # new at P-1 basis
formal/pixels/Pixels/Primitive.lean # new at P-1 basis
```

**Contract/dump delta:** Only the contract and stable-dump changes explicitly named by this task are permitted.

**Work:**

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

**Tests:**

- Plane, sphere, box-face, round-box corner, capsule-side, cylinder, cone fixtures compile.
- Original primitive scalar zero and projective zero agree in deterministic f64 differential tests.
- Feature validity rejects cap/side/corner roots outside their domains.
- Orientation matches field outside→inside sign convention.
- Formal equivalence theorems build for each feature class.

**Focused checks:** Run the focused tests, fixtures, dumps, and commands named by this task before the repository gate.

**Repository gate:**

```text
cargo xtask verify
cargo xtask pixels-formal
```

**Stop conditions:** Stop if an acceptance condition cannot be met without changing a closed architectural decision, weakening a proof/failure boundary, or adding an external Cargo dependency.

**Commit**

```text
pixels P4.3: compile planar and quadric q equations
```

## Task P4.4 — compile torus and bounded-deformation equations

**Requires:** P4.3.

**Produces:** Cover the nonquadric flagship features without opaque scene-wide ranges.

**Files:**

```text
crates/wrela-compiler/src/pixels/projective.rs # new at P-1 basis
crates/wrela-compiler/src/pixels/deform.rs # new at P-1 basis
formal/pixels/Pixels/Primitive.lean # new at P-1 basis
```

**Contract/dump delta:** Only the contract and stable-dump changes explicitly named by this task are permitted.

**Work:**

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

**Tests:**

- Torus retains multiple ordered roots where present; it never returns only the nearest candidate before CSG/occupancy processing.
- Deformed-plane fixture constructs a root tube around the exact plane predictor.
- Approximation remainder is included in every residual/derivative interval.
- Every custom deformation is rejected in v1; only compiler-derived closed deformation contracts reach this pass.
- Formal torus equivalence and Taylor-with-remainder generic theorems build.

**Focused checks:** Run the focused tests, fixtures, dumps, and commands named by this task before the repository gate.

**Repository gate:**

```text
cargo xtask verify
cargo xtask pixels-formal
```

**Stop conditions:** Stop if an acceptance condition cannot be met without changing a closed architectural decision, weakening a proof/failure boundary, or adding an external Cargo dependency.

**Commit**

```text
pixels P4.4: compile quartic and deformed surface programs
```

## Task P4.5 — compile derivative and Taylor coefficient programs

**Requires:** P4.4.

**Produces:** Generate all runtime continuation data once from structural/projective expressions.

**Files:**

```text
crates/wrela-compiler/src/pixels/derivatives.rs # new at P-1 basis
crates/wrela-compiler/src/pixels/polynomial.rs # new at P-1 basis
crates/wrela-compiler/src/pixels/program.rs # new at P-1 basis
```

**Contract/dump delta:** Only the contract and stable-dump changes explicitly named by this task are permitted.

**Work:**

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

Each cluster names the full composed object scalar as its authoritative root,
the primitive bundle IDs used only as candidate slabs/predictors, and a
complete-domain Taylor/root-tube remainder derived from the scalar derivative
contract. Its object-root capacity covers every bounded-subdivision
root/corridor leaf in every predictor slab. A cluster containing only
primitive predictors is incomplete.

**Tests:**

- Analytic derivatives agree with finite differences only as a bug-finder; exact symbolic differentiation is the implementation.
- Mixed partials canonicalize consistently.
- Nonsmooth feature/branch boundaries are excluded by event predicates, not assigned derivatives.
- Derivative bundles share coefficient subprograms.
- Dumps name derivative degree/term counts and influencing parameter set.

**Focused checks:** Run the focused tests, fixtures, dumps, and commands named by this task before the repository gate.

**Repository gate:**

```text
cargo xtask verify
```

**Stop conditions:** Stop if an acceptance condition cannot be met without changing a closed architectural decision, weakening a proof/failure boundary, or adding an external Cargo dependency.

**Commit**

```text
pixels P4.5: emit projective derivative bundles
```

## Task P4.6 — derive conservative projected feature spans

**Requires:** P4.5.

**Produces:** Drive row/tile candidate discovery from structural bounds with no sampling.

**Files:**

```text
crates/wrela-compiler/src/pixels/projection_bounds.rs # new at P-1 basis
crates/wrela-compiler/src/pixels/camera.rs # new at P-1 basis
crates/wrela-compiler/src/pixels/capacities.rs # new at P-1 basis
```

**Contract/dump delta:** Only the contract and stable-dump changes explicitly named by this task are permitted.

**Work:**

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

**Tests:**

- Every oracle-visible permanent-fixture feature lies inside its projected bound at all deterministic sampled parameter corners.
- Enclosed/thin features produce nonempty spans.
- Pixel-range conversion has exact boundary tests at integer/subpixel edges.
- No finite feature is dropped because its projected area rounds below one pixel.
- Overlap capacity recomputes from these spans and remains within P3 ceiling or fails build.

**Focused checks:** Run the focused tests, fixtures, dumps, and commands named by this task before the repository gate.

**Repository gate:**

```text
cargo xtask verify
```

**Stop conditions:** Stop if an acceptance condition cannot be met without changing a closed architectural decision, weakening a proof/failure boundary, or adding an external Cargo dependency.

**Commit**

```text
pixels P4.6: derive complete projected feature spans
```

## Task P4.7 — compile primitive and feature event generators

**Requires:** P4.6.

**Produces:** Represent every local change in root existence or feature validity.

**Files:**

```text
crates/wrela-compiler/src/pixels/events.rs # new at P-1 basis
crates/wrela-compiler/src/pixels/event_kinds.rs # new at P-1 basis
formal/pixels/Pixels/EventCover.lean # new at P-1 basis
```

**Contract/dump delta:** Only the contract and stable-dump changes explicitly named by this task are permitted.

**Work:**

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

The smooth tie is one equation. The smooth band is the union of the two
branches `a-b-k=0` and `a-b+k=0`, so its capacity accounts for both branches.

**Tests:**

- Each feature kind has a complete event-family constructor.
- Generators outside projected spans are not emitted.
- Event-side labels are deterministic and sufficient to update active feature/identity state.
- Numeric generators carry derivative/remainder programs; no black-box boolean sample.
- Formal conditional event-cover theorem builds.

**Focused checks:** Run the focused tests, fixtures, dumps, and commands named by this task before the repository gate.

**Repository gate:**

```text
cargo xtask verify
cargo xtask pixels-formal
```

**Stop conditions:** Stop if an acceptance condition cannot be met without changing a closed architectural decision, weakening a proof/failure boundary, or adding an external Cargo dependency.

**Commit**

```text
pixels P4.7: compile local feature event generators
```

## Task P4.8 — compile q-order competition pairs and swap events

**Requires:** P4.7.

**Produces:** Monitor only feature sheets that can actually compete for visibility.

**Files:**

```text
crates/wrela-compiler/src/pixels/competition.rs # new at P-1 basis
crates/wrela-compiler/src/pixels/events.rs # new at P-1 basis
crates/wrela-compiler/src/pixels/csg.rs # new at P-1 basis
```

**Contract/dump delta:** Only the contract and stable-dump changes explicitly named by this task are permitted.

**Work:**

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

The latter record names the next-order implicit derivative programs, local
scanline and common-q domains, and the complete-difference ambiguity fallback.
If runtime cannot prove `G_q` away from zero, it discards the Taylor predictor
and keeps the pair ambiguous rather than using the fallback as a sign proof.

Do not compute a whole-scene resultant eliminating q from arbitrary feature equations. At runtime, both sheets are already isolated; the event predicate compares their certified q functions.

**Tests:**

- Every omitted pair has one stable exclusion record and positive margin/domain proof.
- `control-close-depth` pair survives and receives a swap/ambiguity event.
- Nonoverlapping colonnade objects are excluded before event emission.
- CSG-noninfluential interior boundaries are excluded with Boolean cofactor proof.
- Pair count and each pruning reason are dumped/reported.

**Focused checks:** Run the focused tests, fixtures, dumps, and commands named by this task before the repository gate.

**Repository gate:**

```text
cargo xtask verify
cargo xtask pixels-formal
```

**Stop conditions:** Stop if an acceptance condition cannot be met without changing a closed architectural decision, weakening a proof/failure boundary, or adding an external Cargo dependency.

**Commit**

```text
pixels P4.8: compile sparse q-order competitions
```

## Task P4.9 — compile omission/exclusion proof records

**Requires:** P4.8.

**Produces:** Make local event and candidate completeness auditable rather than implicit in compiler control flow.

**Files:**

```text
crates/wrela-compiler/src/pixels/exclusions.rs # new at P-1 basis
crates/wrela-compiler/src/pixels/events.rs # new at P-1 basis
crates/wrela-compiler/src/pixels/verify.rs # new at P-1 basis
formal/pixels/Pixels/Bernstein.lean # new at P-1 basis
```

**Contract/dump delta:** Only the contract and stable-dump changes explicitly named by this task are permitted.

**Work:**

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
    GlobalParameterBoxStrictSign,
    GlobalSpatialBoxStrictSign,
    DuplicateCanonicalFeature,
}

pub struct ExclusionRecord {
    pub subject: ExclusionSubject,
    pub domain: DomainId,
    pub reason: ExclusionReason,
    pub margin: F64Interval,
    pub proof: ProofRecordId,
    pub dependencies: Vec<ProofRecordId>,
}
```

A zero or sign-indefinite margin cannot justify exclusion. Static strict-order
exclusions derived only from a current coefficient snapshot become runtime
invariants only if all dependencies are zero-rate. Separately, a strict-sign
proof over the complete declared parameter box and spatial domain is permanent
even when dependencies have nonzero rate.

For predicates whose spatial and influencing-parameter dependence is
polynomial after bounded P4.2 coefficient lowering, try the fixed hierarchy:

1. exact constant/canonical simplification;
2. outward interval range over the complete box;
3. Bernstein conversion, complete coefficient sign scan, and bounded
   de Casteljau subdivision;
4. emit the runtime predicate when all static methods are inconclusive.

The proof payload records normalized box, polynomial/program ID, degree and
coefficient order, outward conversion radius, subdivision tree, strict sign,
and minimum margin. Do not add an LP, SDP, SOS, external solver, or new Cargo
dependency in v1. A globally excluded subject remains present in the audit
graph and stable dump even though it has no runtime predicate record.

**Tests:**

- Verifier accounts for every enumerated subject exactly once.
- Removing an exclusion/emitted record triggers internal verification failure.
- Exclusion dependencies are acyclic and point to earlier pass facts.
- Report can explain any omitted competition from source feature names to final margin.
- A moving parameter with a complete-box Bernstein strict-sign proof emits no
  runtime predicate and remains valid at every declared parameter corner.
- Perturbing one coefficient to make the global sign inconclusive restores the
  runtime predicate; it is never silently omitted.
- No “default pruned” reason exists.

**Focused checks:** Run the focused tests, fixtures, dumps, and commands named by this task before the repository gate.

**Repository gate:**

```text
cargo xtask verify
cargo xtask pixels-formal
```

**Stop conditions:** Stop if an acceptance condition cannot be met without changing a closed architectural decision, weakening a proof/failure boundary, or adding an external Cargo dependency.

**Commit**

```text
pixels P4.9: record complete event exclusions
```

## Task P4.10 — compile row/tile event indexes

**Requires:** P4.9.

**Produces:** Let runtime retrieve relevant features/events in O(records for tile/row), not O(scene).

**Files:**

```text
crates/wrela-compiler/src/pixels/index.rs # new at P-1 basis
crates/wrela-compiler/src/pixels/program.rs # new at P-1 basis
crates/wrela-compiler/src/pixels/capacities.rs # new at P-1 basis
```

**Contract/dump delta:** Only the contract and stable-dump changes explicitly named by this task are permitted.

**Work:**

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

Refine the P3 structural ceilings into final P4 runtime capacities for candidate features, root/event isolation stacks, competition pairs, row events, sheets, runs, corridors, and index slices. These formulas consume the completed event/exclusion/index tables and are the capacities serialized by P5; P3.10 alone is not labeled final for P4 data.

**Tests:**

- Runtime lookup is two bounds-checked loads plus a contiguous slice.
- Every feature/event appears in every tile its conservative span touches.
- No record appears outside its span unless required halo is documented.
- Index size fits capacity and image ceiling.
- Every P4-derived capacity equals or conservatively encloses an exact completed-table/worklist derivation and is verified before P5.
- Unit tests compare indexed retrieval to a slow full-table overlap filter.

**Focused checks:** Run the focused tests, fixtures, dumps, and commands named by this task before the repository gate.

**Repository gate:**

```text
cargo xtask verify
```

**Stop conditions:** Stop if an acceptance condition cannot be met without changing a closed architectural decision, weakening a proof/failure boundary, or adding an external Cargo dependency.

**Commit**

```text
pixels P4.10: build immutable local renderer indexes
```

## Task P4.11 — verify projective/event completeness

**Requires:** P4.10.

**Produces:** Close the compiler proof boundary before binary emission.

**Files:**

```text
crates/wrela-compiler/src/pixels/verify.rs # new at P-1 basis
crates/wrela-compiler/src/pixels/projective.rs # new at P-1 basis
crates/wrela-compiler/src/pixels/events.rs # new at P-1 basis
```

**Contract/dump delta:** Only the contract and stable-dump changes explicitly named by this task are permitted.

**Work:**

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

**Tests:**

- Corruption tests for every missing event family fail.
- `control-enclosed-feature` completeness is independent of event samples.
- `control-close-depth` cannot be statically ordered.
- Static plane-only scene has no unnecessary silhouette generator.
- All permanent fixtures verify.

**Focused checks:** Run the focused tests, fixtures, dumps, and commands named by this task before the repository gate.

**Repository gate:**

```text
cargo xtask verify
cargo xtask pixels-formal
```

**Stop conditions:** Stop if an acceptance condition cannot be met without changing a closed architectural decision, weakening a proof/failure boundary, or adding an external Cargo dependency.

**Commit**

```text
pixels P4.11: verify complete projective event programs
```

## Task P4.12 — pin projective/event dumps

**Requires:** P4.11.

**Produces:** Make the final compiler math before serialization visible and stable.

**Files:**

```text
crates/wrela-compiler/src/pixels/dump.rs # new at P-1 basis
tests/golden/check-pixels-*/expected/field-graph.txt # new at P-1 basis
```

**Contract/dump delta:** Only the contract and stable-dump changes explicitly named by this task are permitted.

**Work:**

Add projective feature equations, derivative bundle summaries, projected spans, event generators, competition pairs, exclusions, and local indexes to the graph dump.

Print polynomial terms in canonical order with exact coefficient source IDs. Print interval margins as explicit endpoints. Print every exclusion subject once.

**Tests:**

- There is enough dump information to reconstruct why a feature/event is present or absent.
- Counts in dump equal capacity/report counts.
- No pointer/address or host formatting appears.
- Permanent fixture event counts are pinned.
- Report determinism passes.

**Focused checks:** Run the focused tests, fixtures, dumps, and commands named by this task before the repository gate.

**Repository gate:**

```text
cargo xtask verify
cargo xtask report-determinism
```

**Stop conditions:** Stop if an acceptance condition cannot be met without changing a closed architectural decision, weakening a proof/failure boundary, or adding an external Cargo dependency.

**Commit**

```text
pixels P4.12: pin projective and event compiler dumps
```

### Milestone P4 close

Run `cargo xtask verify`. The milestone closes only when every accepted scene has a verified finite local event program and every omitted interaction has an explicit exclusion proof.

---

# Milestone P5 — `FrameProgram v1`, image placement, generated renderer actors, and reports

Milestone result: the compiler emits a verified binary frame program, reserves all mutable renderer memory, synthesizes typed renderer actors/glue, places everything in the sealed image, and reports the complete renderer contract. The production path still returns `FrameContractMismatch` before rendering because the sweep is not installed; only the explicitly tagged P-1 plane-skeleton conformance image may use the temporary analytic plane path.

## Task P5.1 — define `FrameProgram v1` Rust structs

**Requires:** the preceding milestone close gate.

**Produces:** Freeze the v1 outer wire envelope, directory namespace, and all P0–P8 record layouts before byte encoding. P9–P11 populate predeclared table kinds without changing the 80-byte header or 16-byte directory entry.

**Files:**

```text
crates/wrela-compiler/src/pixels/program.rs # new at P-1 basis
crates/wrela-compiler/src/pixels/version.rs # new at P-1 basis
crates/wrela-machine/src/pixels.rs # new at P-1 basis
```

**Contract/dump delta:** Only the contract and stable-dump changes explicitly named by this task are permitted.

**Work:**

Define fixed-width records using only integer IDs, offsets, counts, enum tags, and bit-preserved scalar constants. Separate compiler-rich structs from wire structs:

```rust
pub struct FrameProgram { /* rich verified model */ }
```

Implement `FrameProgramHeaderV1` and `FrameProgramTableV1` exactly once from the canonical definitions in §4.14; do not copy a second field list into task-local documentation or code-generation metadata. Assert header size 80 and directory-entry size 16. The digest field is zero while hashing and then filled with SHA-256 of the complete encoded bytes with that field zeroed.

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

**Tests:**

- `wrela-machine` contains only shared format/constants, no compiler analysis.
- Wire structs have explicit size assertions.
- Every rich record has a wire counterpart or is intentionally compiler-only and documented.
- Version/revision constants appear in one location.
- Header magic/version corruption tests exist.
- Predeclared P9–P11 table kinds round-trip as canonical empty directory entries.

**Focused checks:** Run the focused tests, fixtures, dumps, and commands named by this task before the repository gate.

**Repository gate:**

```text
cargo xtask verify
```

**Stop conditions:** Stop if an acceptance condition cannot be met without changing a closed architectural decision, weakening a proof/failure boundary, or adding an external Cargo dependency.

**Commit**

```text
pixels P5.1: define FrameProgram v1 records
```

## Task P5.2 — implement deterministic encoder

**Requires:** P5.1.

**Produces:** Serialize a verified projective program without depending on Rust layout.

**Files:**

```text
crates/wrela-compiler/src/pixels/encode.rs # new at P-1 basis
crates/wrela-compiler/src/pixels/program.rs # new at P-1 basis
crates/wrela-machine/src/pixels.rs # new at P-1 basis
```

**Contract/dump delta:** Only the contract and stable-dump changes explicitly named by this task are permitted.

**Work:**

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

**Tests:**

- Encoding same program twice yields identical bytes.
- All checked conversions fail with internal/size error, never truncate.
- Every alignment padding byte is zero.
- Digest algorithm has unit vectors.
- Golden frame-program binary is checked as SHA-256 plus a hex header/table summary; do not check large raw binaries into textual expected files unless harness already supports fixture bytes.

**Focused checks:** Run the focused tests, fixtures, dumps, and commands named by this task before the repository gate.

**Repository gate:**

```text
cargo xtask verify
```

**Stop conditions:** Stop if an acceptance condition cannot be met without changing a closed architectural decision, weakening a proof/failure boundary, or adding an external Cargo dependency.

**Commit**

```text
pixels P5.2: encode verified frame programs
```

## Task P5.3 — implement hostile binary decoder and verifier

**Requires:** P5.2.

**Produces:** Prove the binary format is independently checkable and fuzzable.

**Files:**

```text
crates/wrela-compiler/src/pixels/decode.rs # new at P-1 basis
crates/wrela-compiler/src/pixels/binary_verify.rs # new at P-1 basis
crates/xtask/src/fuzz.rs
```

**Contract/dump delta:** Only the contract and stable-dump changes explicitly named by this task are permitted.

**Work:**

Implement a bounds-checked cursor with no unsafe code. Validate header/digest/table directory before allocating table vectors. Limit total bytes to machine ceiling.

Decode all records into rich structs and rerun semantic `verify::check_program` after wire checks. Add a Pixels target to existing fuzz infrastructure that mutates valid encoded frame programs and arbitrary bytes.

Fuzz outcomes are only:

- decoded/verified program;
- structured `DecodeError`;
- never panic, OOM, hang, or out-of-bounds.

**Tests:**

- `decode(encode(p)) == p` for all permanent fixtures.
- Single-bit mutation corpus covers every header/table/enum/reserved field.
- Truncated bytes at every offset return error.
- Overlapping table/overflow attacks return error before allocation.
- Fuzz smoke is in `verify`; broad run remains the repository fuzz lane.

**Focused checks:** Run the focused tests, fixtures, dumps, and commands named by this task before the repository gate.

**Repository gate:**

```text
cargo xtask verify
cargo xtask fuzz pixels --iters 10000 --seed 1
```

**Stop conditions:** Stop if an acceptance condition cannot be met without changing a closed architectural decision, weakening a proof/failure boundary, or adding an external Cargo dependency.

**Commit**

```text
pixels P5.3: verify and fuzz FrameProgram bytes
```

## Task P5.4 — compile renderer programs during image build

**Requires:** P5.3.

**Produces:** Insert Pixels compilation at the single correct point in the existing build pipeline.

**Files:**

```text
crates/wrela-compiler/src/bin/wrela.rs
crates/wrela-compiler/src/lib.rs
crates/wrela-compiler/src/pixels/mod.rs # new at P-1 basis
crates/wrela-compiler/src/eval/image_checks.rs
```

**Contract/dump delta:** Only the contract and stable-dump changes explicitly named by this task are permitted.

**Work:**

After all modules are semantically checked and the image graph is evaluated/sealed, but before runtime placement/codegen finalization:

1. parse/validate each renderer config;
2. call `pixels::compile` in renderer index order;
3. store `Vec<CompiledRenderer>` in the build context;
4. make dump stages consume these values;
5. pass generated actor/glue requirements into closure/root synthesis;
6. pass encoded programs/state layouts into layout/report.

Do not call Pixels for images with zero renderers. A failure aborts the build before ordinary guest codegen so no partial image artifact is written.

**Tests:**

- `--stage=field-graph` now runs the complete P4 compiler.
- `--stage=frame-program` decodes its own encoded bytes before dumping.
- Multiple renderers compile independently with stable indexes.
- Ordinary nonrenderer builds are byte-identical to pre-task baseline.
- Compiler timing report, if any, names Pixels separately but is not part of checked goldens.

**Focused checks:** Run the focused tests, fixtures, dumps, and commands named by this task before the repository gate.

**Repository gate:**

```text
cargo xtask verify
cargo xtask repro
```

**Stop conditions:** Stop if an acceptance condition cannot be met without changing a closed architectural decision, weakening a proof/failure boundary, or adding an external Cargo dependency.

**Commit**

```text
pixels P5.4: compile frame programs in sealed image builds
```

## Task P5.5 — reserve `frameprog` and `pixelsdata` image sections

**Requires:** P5.4.

**Produces:** Place immutable and mutable renderer data without disturbing existing rtdata invariants.

**Files:**

```text
crates/wrela-machine/src/lib.rs
crates/wrela-compiler/src/layout.rs
crates/wrela-compiler/src/layout/place.rs
crates/wrela-compiler/src/layout/report_lines.rs
```

**Contract/dump delta:** Only the contract and stable-dump changes explicitly named by this task are permitted.

**Work:**

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

`pixelsdata` is zero-initialized reservation, not stored zero bytes in the image blob. Record reservation separately from blob length where layout already supports BSS-like regions; otherwise implement the minimal explicit reservation mechanism rather than materializing hundreds of MiB in the image file. Diagnostic/conformance builds include the fixed certificate-telemetry counter reservation derived at P3.10; uninstrumented production builds omit it. Both layouts are deterministic and reported separately.

**Tests:**

- Existing code/rodata/rtdata addresses are unchanged for nonrenderer images.
- Renderer section addresses are deterministic.
- All checked ranges fit the machine profile and do not overlap stacks/devices/framebuffer windows.
- Image report lists section/reservation bytes separately.
- Instrumented telemetry reservation changes only reported mutable-state bytes,
  never frame-program semantics or displayed output.
- Boundary tests cover exact max, one byte over, checked-add overflow, and alignment padding.

**Focused checks:** Run the focused tests, fixtures, dumps, and commands named by this task before the repository gate.

**Repository gate:**

```text
cargo xtask verify
```

**Stop conditions:** Stop if an acceptance condition cannot be met without changing a closed architectural decision, weakening a proof/failure boundary, or adding an external Cargo dependency.

**Commit**

```text
pixels P5.5: place frame programs and renderer state
```

## Task P5.6 — append frame-program bytes to the image

**Requires:** P5.5.

**Produces:** Make immutable renderer data available at guest addresses.

**Files:**

```text
crates/wrela-compiler/src/layout.rs
crates/wrela-compiler/src/layout/harness.rs
crates/wrela-compiler/src/pixels/encode.rs # new at P-1 basis
```

**Contract/dump delta:** Only the contract and stable-dump changes explicitly named by this task are permitted.

**Work:**

At layout:

- pad blob to each renderer frame-program placement;
- append exact encoded bytes;
- assert final cursor equals computed frameprog end;
- leave mutable pixelsdata as zero reservation;
- include renderer bytes in image digest/report;
- expose `RendererPlacement` in `ImageLayout`.

Add a layout test that reads bytes back from `ImageLayout.blob`, decodes, verifies digest, and compares to compiler rich program.

**Tests:**

- Blob contains exact frame-program bytes at reported address.
- No host path or pointer is encoded.
- Corrupting one image byte fails the decoder/digest test.
- Multiple renderer programs have correct independent bases/digests.
- Nonrenderer image bytes remain unchanged.

**Focused checks:** Run the focused tests, fixtures, dumps, and commands named by this task before the repository gate.

**Repository gate:**

```text
cargo xtask verify
cargo xtask repro
```

**Stop conditions:** Stop if an acceptance condition cannot be met without changing a closed architectural decision, weakening a proof/failure boundary, or adding an external Cargo dependency.

**Commit**

```text
pixels P5.6: seal frame-program bytes into images
```

## Task P5.7 — generate renderer configuration module

**Requires:** P5.6.

**Produces:** Expose table addresses/capacities to Wrela runtime code without runtime decoding/allocation.

**Files:**

```text
crates/wrela-compiler/src/pixels/glue.rs # new at P-1 basis
crates/wrela-compiler/src/loader.rs
crates/wrela-compiler/src/rtconfig.rs
stdlib/core/render_program.wr # new at P-1 basis
```

**Contract/dump delta:** Only the contract and stable-dump changes explicitly named by this task are permitted.

**Work:**

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
const R0_CERT_TELEMETRY_BYTES: usize = ... # zero when uninstrumented
# all exact capacities...
```

Also generate `@layout(runtime)` view structs for header/table records and `@placed` static roots at frame-program bases. Array extents are exact generated constants.

Generated source must parse/type-check through the ordinary compiler, like `core.__image_runtime`. It contains no field equations as executable source; it exposes table layouts only.

**Tests:**

- renderer configuration appears inside the canonical `frame-program` dump; do not create a fourth Pixels dump stage.
- Generated module has a stable dump.
- Every runtime capacity/address comes from compiler placement, not duplicated arithmetic in Wrela.
- Stubs support zero-renderer images.
- Pool ceilings fail before generating invalid array extents.

**Focused checks:** Run the focused tests, fixtures, dumps, and commands named by this task before the repository gate.

**Repository gate:**

```text
cargo xtask verify
cargo xtask report-determinism
```

**Stop conditions:** Stop if an acceptance condition cannot be met without changing a closed architectural decision, weakening a proof/failure boundary, or adding an external Cargo dependency.

**Commit**

```text
pixels P5.7: generate renderer table and state views
```

## Task P5.8 — synthesize renderer coordinator/worker actors

**Requires:** P5.7.

**Produces:** Make renderer execution ordinary Wrela actors with closed capacity and placement.

**Files:**

```text
crates/wrela-compiler/src/pixels/glue.rs # new at P-1 basis
crates/wrela-compiler/src/eval/image.rs
crates/wrela-compiler/src/eval/image_checks.rs
crates/wrela-compiler/src/placement.rs
crates/wrela-compiler/src/layout/rtdata.rs
crates/wrela-compiler/src/lower.rs
stdlib/core/render_actor.wr # new at P-1 basis
```

**Contract/dump delta:** Only the contract and stable-dump changes explicitly named by this task are permitted.

**Work:**

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

Until sweep exists, `render` returns `Err(FrameContractMismatch(RenderPath.RendererUnavailable))` after validating frame input and without touching display.

**Tests:**

- Renderer handle has real actor identity and ordinary admission semantics.
- Generated actors appear in typed/FlowWir/MachineWir/placement/report dumps.
- Cross-core rings are generated by existing machinery.
- No custom scheduler/work stealing is added.
- Boot fixture can create coordinator/workers and call render, receiving the expected error deterministically.

**Focused checks:** Run the focused tests, fixtures, dumps, and commands named by this task before the repository gate.

**Repository gate:**

```text
cargo xtask verify
```

**Stop conditions:** Stop if an acceptance condition cannot be met without changing a closed architectural decision, weakening a proof/failure boundary, or adding an external Cargo dependency.

**Commit**

```text
pixels P5.8: synthesize bounded renderer actors
```

## Task P5.9 — root renderer orchestration and bootstrap dispatch

**Requires:** P5.8.

**Produces:** Ensure dead-code elimination retains the orchestration and placeholder dispatch paths required at P5. Exact used-kernel palette generation belongs to P12.2 after all P9–P11 features exist.

**Files:**

```text
crates/wrela-compiler/src/lower.rs
crates/wrela-compiler/src/flowwir_lower.rs
crates/wrela-compiler/src/pixels/glue.rs # new at P-1 basis
```

**Contract/dump delta:** Only the contract and stable-dump changes explicitly named by this task are permitted.

**Work:**

Add generated function keys to the existing image force-root calculation only when a renderer exists. Root:

- coordinator public render method;
- worker job method;
- bootstrap numeric dispatch helpers referenced by the P0–P8 table kinds;
- display present path;
- runtime abort/failure path already required.

Do not force-root every possible future primitive/material kernel. The P5 record-kind census roots the bounded bootstrap dispatcher; P12.2 replaces it with the exact final palette. Unsupported/missing P5 entry is an internal build error.

**Tests:**

- Declared scene emits fixed core orchestration plus only the P5 bootstrap families it references.
- The dump explicitly marks the palette `bootstrap`; it may not claim final exactness.
- No indirect function pointer is required; dispatch is bounded switch/match over record tags.
- Cost report never assigns zero to a used renderer method because a key was omitted.

**Focused checks:** Run the focused tests, fixtures, dumps, and commands named by this task before the repository gate.

**Repository gate:**

```text
cargo xtask verify
```

**Stop conditions:** Stop if an acceptance condition cannot be met without changing a closed architectural decision, weakening a proof/failure boundary, or adding an external Cargo dependency.

**Commit**

```text
pixels P5.9: root renderer bootstrap dispatch
```

## Task P5.10 — implement full frame-program/report dumps

**Requires:** P5.9.

**Produces:** Pin serialized program, layout, and image facts before runtime rendering.

**Files:**

```text
crates/wrela-compiler/src/pixels/dump.rs # new at P-1 basis
crates/wrela-compiler/src/pixels/report.rs # new at P-1 basis
crates/wrela-compiler/src/report.rs
crates/wrela-compiler/src/bin/wrela.rs
tests/golden/check-pixels-*/expected/frame-program.txt # new at P-1 basis
tests/golden/check-pixels-*/expected/render-layout.txt # new at P-1 basis
tests/golden/check-pixels-*/expected/report.txt # new at P-1 basis
```

**Contract/dump delta:** Only the contract and stable-dump changes explicitly named by this task are permitted.

**Work:**

Implement §8.3–8.5 completely. Report compiler/rich counts, wire bytes, mutable reservation, generated actors, worker tile ranges, and fallback policy in the existing three Pixels dumps.

Do not report expected/estimated frame rate. Existing cost section later reports emitted code proxy cycles; renderer report is structural and exact.

**Tests:**

- Dumps are generated from decoded bytes and actual `ImageLayout`, not parallel estimates.
- Table counts/offsets/digests match encoder.
- Report names formal/numeric revision.
- Renderer memory contributes to image peak memory and profile refusal.
- All permanent fixtures have reviewed pinned outputs.

**Focused checks:** Run the focused tests, fixtures, dumps, and commands named by this task before the repository gate.

**Repository gate:**

```text
cargo xtask verify
cargo xtask report-determinism
cargo xtask repro
```

**Stop conditions:** Stop if an acceptance condition cannot be met without changing a closed architectural decision, weakening a proof/failure boundary, or adding an external Cargo dependency.

**Commit**

```text
pixels P5.10: pin frame-program and renderer-layout reports
```

## Task P5.11 — add renderer binary/layout fuzz and reproduction lanes

**Requires:** P5.10.

**Produces:** Make the new sealed artifact as rigorously checked as existing compiler stages.

**Files:**

```text
crates/xtask/src/main.rs
crates/xtask/src/fuzz.rs
crates/xtask/src/pixels_repro.rs # new at P-1 basis
crates/xtask/src/golden.rs
```

**Contract/dump delta:** Only the contract and stable-dump changes explicitly named by this task are permitted.

**Work:**

Add:

```text
cargo xtask fuzz pixels
cargo xtask pixels-repro
```

`pixels-repro` follows §8.7. Fuzz target covers symbolic field source mutations through compiler where cheap and encoded program bytes for broad mutation.

Classify expensive whole-corpus reproduction into milestone lane; keep one plane and one smooth-CSG smoke case in ordinary verify.

**Tests:**

- No new default lane exceeds locked test budget.
- Fresh-directory reproduction compares exact image bytes.
- Fuzz findings are promoted to permanent tests before fixes.
- Decoder/compiler never panics on fuzz inputs.

**Focused checks:** Run the focused tests, fixtures, dumps, and commands named by this task before the repository gate.

**Repository gate:**

```text
cargo xtask verify
```

**Stop conditions:** Stop if an acceptance condition cannot be met without changing a closed architectural decision, weakening a proof/failure boundary, or adding an external Cargo dependency.

**Commit**

```text
pixels P5.11: gate frame-program fuzz and reproducibility
```

### Milestone P5 close

Run `cargo xtask verify`. The milestone closes when renderer images boot, contain verified frame-program bytes and exact state reservations, expose real renderer actors, and deterministically return `FrameContractMismatch(RenderPath.RendererUnavailable)` without presentation.

---

# Milestone P6 — verified numeric kernels and cross-language correspondence

Milestone result: every runtime proof predicate and arithmetic kernel exists in three forms—Lean theorem/model, safe Rust compiler reference, and scalar Wrela implementation—with permanent differential tests. SIMD variants may be added later but cannot change semantics.

## Task P6.1 — implement shared numeric test-vector format

**Requires:** the preceding milestone close gate.

**Produces:** Drive identical cases through Rust and Wrela without a new serialization dependency.

**Files:**

```text
crates/wrela-compiler/src/pixels/reference/mod.rs # new at P-1 basis
crates/wrela-compiler/src/pixels/test_vectors.rs # new at P-1 basis
stdlib/core/render_test_vectors.wr # new at P-1 basis
crates/xtask/src/pixels_vectors.rs # new at P-1 basis
formal/pixels/KERNELS.txt # new at P-1 basis
```

**Contract/dump delta:** Only the contract and stable-dump changes explicitly named by this task are permitted.

**Work:**

Define a simple generated line format with fixed integer/hex fields, for example:

```text
iv_add lo_a=-3 hi_a=7 exp_a=-12 lo_b=2 hi_b=9 exp_b=-12 out_lo=-1 out_hi=16 out_exp=-12
```

Compiler unit tests generate vectors deterministically from fixed seeds plus hand edge cases. For boot differential fixtures, xtask converts a bounded vector subset into generated Wrela constants under `core.__pixels_vectors`.

Do not parse JSON/TOML in guest. Host parser is hand-written and strict.

**Tests:**

- Unknown key, duplicate key, malformed integer, and overflow fail.
- Vector file order is stable by kernel then case ID.
- Every kernel manifest row names at least one edge vector and one generated vector family.
- Vector generation never depends on host float textual formatting; use bits or integers.

**Focused checks:** Run the focused tests, fixtures, dumps, and commands named by this task before the repository gate.

**Repository gate:**

```text
cargo xtask verify
```

**Stop conditions:** Stop if an acceptance condition cannot be met without changing a closed architectural decision, weakening a proof/failure boundary, or adding an external Cargo dependency.

**Commit**

```text
pixels P6.1: add cross-language numeric vectors
```

## Task P6.2 — implement `Iv32` and checked dyadic arithmetic

**Requires:** P6.1.

**Produces:** Provide exact branch-free verifier arithmetic with explicit overflow failure.

**Files:**

```text
crates/wrela-compiler/src/pixels/reference/iv32.rs # new at P-1 basis
stdlib/core/render_interval.wr # new at P-1 basis
formal/pixels/Pixels/Dyadic.lean # new at P-1 basis
formal/pixels/Pixels/Interval.lean # new at P-1 basis
formal/pixels/KERNELS.txt # new at P-1 basis
```

**Contract/dump delta:** Only the contract and stable-dump changes explicitly named by this task are permitted.

**Work:**

Implement the canonical `Iv32`/`FixedDomain` contract from §5.1. Every operation also receives the compiler-selected `FixedDomain` for that value family. Hot intervals never carry or dynamically align exponents. Conversion between domains is a cold, explicit checked operation performed only at declared program boundaries. Operations return `Result[Iv32, NumericError]` where machine overflow is possible. Provide:

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

Domain-conversion policy is fixed:

- source and destination domains are explicit operands;
- shift mantissas outward using floor for low and ceil for high;
- never silently saturate or choose a domain at runtime;
- compiler-selected exponent range is `[-96, 63]` in v1.

**Tests:**

- Rust and Wrela scalar outputs agree on all vectors.
- Exhaustive tests cover all i8 endpoint values for reduced-width model and selected i32 boundaries.
- Lean containment theorems build.
- Every failure is explicit `NumericError`, never wraparound.
- Generated AArch64 uses widened multiply for interval multiply as expected; assembly shape is inspected later, not asserted here.

**Focused checks:** Run the focused tests, fixtures, dumps, and commands named by this task before the repository gate.

**Repository gate:**

```text
cargo xtask verify
cargo xtask pixels-formal
```

**Stop conditions:** Stop if an acceptance condition cannot be met without changing a closed architectural decision, weakening a proof/failure boundary, or adding an external Cargo dependency.

**Commit**

```text
pixels P6.2: implement checked dyadic intervals
```

## Task P6.3 — implement polynomial evaluation and exact quadratic range

**Requires:** P6.2.

**Produces:** Evaluate low-degree equations tightly and correctly at runtime.

**Files:**

```text
crates/wrela-compiler/src/pixels/reference/poly.rs # new at P-1 basis
stdlib/core/render_program.wr # new at P-1 basis
stdlib/core/render_interval.wr # new at P-1 basis
formal/pixels/Pixels/Bernstein.lean # new at P-1 basis
formal/pixels/KERNELS.txt # new at P-1 basis
```

**Contract/dump delta:** Only the contract and stable-dump changes explicitly named by this task are permitted.

**Work:**

Implement scalar candidate f32/f64 and verifier `Iv32` paths for:

- Horner univariate degree 1–8;
- sparse multivariate term evaluation over `(u,v,q,t)`;
- derivative program evaluation;
- generated checked-dyadic composition of supported quadratic-candidate tube
  faces into univariate Bernstein coefficients;
- de Casteljau subdivision for fixed degree;
- Bernstein coefficient sign test;
- bounded Bernstein sign-variation predicates used by root isolation;
- exact quadratic range over `[0,1]^2` using all candidate extrema;
- Taylor polynomial plus interval remainder.

The composition kernel converts every source and `q_hat` coefficient outward,
uses checked widened integer intermediates for the generated schedule, and
adds conversion/Taylor remainder radii to affected output coefficients. It
returns `UnsupportedShape` for an unsealed degree/term schedule and
`NumericError` for arithmetic failure. Neither outcome may be confused with a
negative proof result. A complete coefficient scan is branch-light but remains
integer proof arithmetic; floating FMA/dot results have no acceptance
authority.

The exact quadratic range routine:

1. evaluate four corners;
2. solve interior stationary point if Hessian determinant nonzero and point lies in rectangle;
3. solve one-dimensional stationary point on each of four edges;
4. evaluate all valid candidates;
5. outward-convert min/max to verifier interval.

Degenerate linear/constant edge/interior cases are explicit branches.

**Tests:**

- Rust/Wrela agree on vectors.
- Quadratic range contains dense deterministic samples and analytic extrema fixtures.
- Corner+center-only implementation would fail a pinned positive control.
- Bernstein subdivision preserves coefficient/domain mapping.
- Composed coefficient arrays contain direct exact/rational evaluation over
  deterministic domain vectors.
- Coefficient-face acceptance agrees with interval-tube acceptance wherever
  both decide, and an inconclusive coefficient hull falls through without
  widening tolerance.
- No runtime allocation; coefficient arrays have generated fixed maxima.

**Focused checks:** Run the focused tests, fixtures, dumps, and commands named by this task before the repository gate.

**Repository gate:**

```text
cargo xtask verify
cargo xtask pixels-formal
```

**Stop conditions:** Stop if an acceptance condition cannot be met without changing a closed architectural decision, weakening a proof/failure boundary, or adding an external Cargo dependency.

**Commit**

```text
pixels P6.3: verify low-degree polynomial ranges
```

## Task P6.4 — implement bounded root isolation

**Requires:** P6.3.

**Produces:** Find all feature/event roots in a finite interval with explicit completeness/failure.

**Files:**

```text
crates/wrela-compiler/src/pixels/reference/root.rs # new at P-1 basis
stdlib/core/render_events.wr # new at P-1 basis
formal/pixels/Pixels/RootIsolation.lean # new at P-1 basis
formal/pixels/KERNELS.txt # new at P-1 basis
```

**Contract/dump delta:** Only the contract and stable-dump changes explicitly named by this task are permitted.

**Work:**

Implement fixed-capacity front-to-back interval subdivision:

```wrela
enum RootOutcome:
    Roots(count: u16)
    CertifiedNone
    Unresolved(reason: RootReason)
```

The caller supplies output storage and stack arrays sized by compiler capacity. Algorithm:

1. push full interval;
2. for a supported polynomial, evaluate Bernstein range and the proved
   sign-variation/root-count predicates; otherwise evaluate interval/Taylor
   range;
3. discard if zero excluded;
4. if derivative sign excludes zero and endpoints bracket, isolate by bisection;
5. if polynomial sign-variation/root-count rule proves exact root count, subdivide until each root interval meets q/x tolerance;
6. otherwise split at exact midpoint;
7. process left before right;
8. merge only overlapping intervals proven to contain the same unique root;
9. return all roots sorted.

Tangency without sign change is handled by derivative/discriminant/root-count predicates, not converted to miss.

**Tests:**

- Plane, sphere, torus multi-root, tangent double-root, close roots, and no-root fixtures pass.
- Polynomial leaf-sublevel fixtures isolate boundaries of
  `leaf - accumulated_support_budget`, including a positive leaf with no leaf
  zero.
- All roots inside domain are returned or outcome is `Unresolved`; no partial list labeled complete.
- Root count never exceeds compiler capacity; overflow is `CapacityExceeded(RenderCapacity.Roots)` or `CapacityExceeded(RenderCapacity.Sheets)`.
- Rust/Wrela scalar outputs agree exactly on interval endpoints/counts/reasons.
- Lean bracket/subdivision completeness theorems build for the used predicates.

**Focused checks:** Run the focused tests, fixtures, dumps, and commands named by this task before the repository gate.

**Repository gate:**

```text
cargo xtask verify
cargo xtask pixels-formal
```

**Stop conditions:** Stop if an acceptance condition cannot be met without changing a closed architectural decision, weakening a proof/failure boundary, or adding an external Cargo dependency.

**Commit**

```text
pixels P6.4: isolate complete bounded root sets
```

## Task P6.5 — implement monotone tube and Krawczyk predicates

**Requires:** P6.4.

**Produces:** Certify one root sheet continuously across a run domain.

**Files:**

```text
crates/wrela-compiler/src/pixels/reference/certificate.rs # new at P-1 basis
stdlib/core/render_sweep.wr # new at P-1 basis
formal/pixels/Pixels/Krawczyk.lean # new at P-1 basis
formal/pixels/Pixels/RunCertificate.lean # new at P-1 basis
formal/pixels/KERNELS.txt # new at P-1 basis
```

**Contract/dump delta:** Only the contract and stable-dump changes explicitly named by this task are permitted.

**Work:**

Implement three ordered certificate methods:

**Tier 0: Bernstein coefficient faces**

- when P4.2 emitted a supported composition shape, construct the two tube-face
  coefficient arrays with the P6.3 checked dyadic kernel;
- require every lower-face coefficient to have one strict sign and every
  upper-face coefficient the opposite sign;
- use de Casteljau subdivision of `X` when the hull is inconclusive;
- evaluate `G_q` over the complete tube with the ordinary derivative
  range/Taylor kernel and require one strict sign;
- include source/candidate conversion and Taylor remainder in the coefficient
  bounds.

**Tier 1: interval monotone tube**

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

**Tests:**

- Plane accepts Tier 1 over full row absent events.
- Supported algebraic plane/sphere/torus controls prefer Tier 0 when its
  coefficient hull decides and report the sealed composition shape.
- Curved regular sheets accept one tier on permanent fixtures.
- Over-degree or non-polynomial deformation controls take the interval/Taylor
  fallback without source rejection.
- Grazing/silhouette domain rejects before miscertification.
- Failed contraction is ordinary false, not error; numeric overflow/nonfinite is error.
- Rust/Wrela predicates agree; Lean uniqueness theorems build.

**Focused checks:** Run the focused tests, fixtures, dumps, and commands named by this task before the repository gate.

**Repository gate:**

```text
cargo xtask verify
cargo xtask pixels-formal
```

**Stop conditions:** Stop if an acceptance condition cannot be met without changing a closed architectural decision, weakening a proof/failure boundary, or adding an external Cargo dependency.

**Commit**

```text
pixels P6.5: certify regular root tubes
```

## Task P6.6 — implement q-order and CSG event kernels

**Requires:** P6.5.

**Produces:** Prove front order and update composite occupancy cheaply.

**Files:**

```text
crates/wrela-compiler/src/pixels/reference/order.rs # new at P-1 basis
crates/wrela-compiler/src/pixels/reference/csg.rs # new at P-1 basis
stdlib/core/render_sweep.wr # new at P-1 basis
formal/pixels/Pixels/QOrder.lean # new at P-1 basis
formal/pixels/Pixels/Csg.lean # new at P-1 basis
formal/pixels/KERNELS.txt # new at P-1 basis
```

**Contract/dump delta:** Only the contract and stable-dump changes explicitly named by this task are permitted.

**Work:**

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

**Tests:**

- Close-plane fixture reports ambiguity/corridor until refined.
- All-pairs exact order and adjacent order agree for sorted lists.
- CSG stack agrees exhaustively with compiler expression fixtures.
- Noninfluential boundary skip leaves composite occupancy unchanged.
- Rust/Wrela and Lean contracts agree.

**Focused checks:** Run the focused tests, fixtures, dumps, and commands named by this task before the repository gate.

**Repository gate:**

```text
cargo xtask verify
cargo xtask pixels-formal
```

**Stop conditions:** Stop if an acceptance condition cannot be met without changing a closed architectural decision, weakening a proof/failure boundary, or adding an external Cargo dependency.

**Commit**

```text
pixels P6.6: verify q order and CSG crossings
```

## Task P6.7 — implement fixed-q setup and recurrence

**Requires:** P6.6.

**Produces:** Make the pixel-depth hot loop exact integer work with bounded real error.

**Files:**

```text
crates/wrela-compiler/src/pixels/reference/fixed_q.rs # new at P-1 basis
stdlib/core/render_raster.wr # new at P-1 basis
formal/pixels/Pixels/FixedQ.lean # new at P-1 basis
formal/pixels/KERNELS.txt # new at P-1 basis
```

**Contract/dump delta:** Only the contract and stable-dump changes explicitly named by this task are permitted.

**Work:**

Define:

```wrela
struct QRunScalar:
    q: i32
    dq: i32
    ddq: i32
    domain: FixedDomain
    error_radius: i32
```

Setup chooses a shared exponent from certified q/dq/ddq maxima for one microtile width. It must prove all recurrence states and comparisons remain in i32 range. Quantize each coefficient outward and accumulate:

- source q-model error;
- coefficient conversion radius;
- recurrence rounding radius;
- derivative/Taylor remainder;
- microtile reset radius.

P6 implements and proves only the scalar step. Reset at generated microtile width; v1 default is 32 pixels but the compiler may choose a smaller power of two to satisfy range/error, never larger than 64. P8.4 introduces the first `i32x4` implementation and P12 closes its backend/code-shape obligations.

**Tests:**

- Scalar integer outputs agree with the Rust reference bit-for-bit.
- Real q truth samples remain within q code ± error radius.
- Quantized q-order is accepted only when original slack exceeds both radii.
- Near-overflow fixture chooses smaller microtile or fails setup explicitly.
- Lean recurrence/error/no-overflow conditional theorems build.

**Focused checks:** Run the focused tests, fixtures, dumps, and commands named by this task before the repository gate.

**Repository gate:**

```text
cargo xtask verify
cargo xtask pixels-formal
```

**Stop conditions:** Stop if an acceptance condition cannot be met without changing a closed architectural decision, weakening a proof/failure boundary, or adding an external Cargo dependency.

**Commit**

```text
pixels P6.7: implement certified fixed-q recurrence
```

## Task P6.8 — implement analytic coverage kernels

**Requires:** P6.7.

**Produces:** Compute stable subpixel event coverage without MSAA or stochastic sampling.

**Files:**

```text
crates/wrela-compiler/src/pixels/reference/coverage.rs # new at P-1 basis
stdlib/core/render_coverage.wr # new at P-1 basis
formal/pixels/Pixels/Coverage.lean # new at P-1 basis
formal/pixels/KERNELS.txt # new at P-1 basis
```

**Contract/dump delta:** Only the contract and stable-dump changes explicitly named by this task are permitted.

**Work:**

V1 supports a unit box pixel filter. Implement:

- exact half-plane area in unit square from clipped polygon;
- quadratic-curve local approximation split into monotone segments;
- conservative lower/upper area via curve strip and line integrals;
- foreground/background side classification from event orientation;
- half-open ownership for curve exactly on pixel/tile boundary;
- coverage-to-color error budget using local color contrast bounds.

No supersample mask is used for acceptance. Deterministic dense quadrature may exist only in host oracle tests.

**Tests:**

- Axis-aligned, diagonal, corner-touching, subpixel-thin, and high-curvature fixtures pass.
- Coverage interval contains high-precision host integration.
- Shared tile boundaries neither drop nor double-count edge coverage.
- Rust/Wrela intervals agree.
- Lean half-plane and strip/color bounds build.

**Focused checks:** Run the focused tests, fixtures, dumps, and commands named by this task before the repository gate.

**Repository gate:**

```text
cargo xtask verify
cargo xtask pixels-formal
```

**Stop conditions:** Stop if an acceptance condition cannot be met without changing a closed architectural decision, weakening a proof/failure boundary, or adding an external Cargo dependency.

**Commit**

```text
pixels P6.8: integrate certified event coverage
```

## Task P6.9 — implement normal and material bound kernels

**Requires:** P6.8.

**Produces:** Carry geometric uncertainty into deterministic shading decisions.

**Files:**

```text
crates/wrela-compiler/src/pixels/reference/normal.rs # new at P-1 basis
crates/wrela-compiler/src/pixels/reference/material.rs # new at P-1 basis
stdlib/core/render_material.wr # new at P-1 basis
formal/pixels/Pixels/Normal.lean # new at P-1 basis
formal/pixels/Pixels/MaterialBound.lean # new at P-1 basis
```

**Contract/dump delta:** Only the contract and stable-dump changes explicitly named by this task are permitted.

**Work:**

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

**Tests:**

- Plane normal exact and constant.
- Sphere/curved normals match independent gradient within cone.
- Kink/feature boundary requires event coverage path.
- Rust/Wrela bound kernels agree.
- Formal normal/moment/error theorems build.

**Focused checks:** Run the focused tests, fixtures, dumps, and commands named by this task before the repository gate.

**Repository gate:**

```text
cargo xtask verify
cargo xtask pixels-formal
```

**Stop conditions:** Stop if an acceptance condition cannot be met without changing a closed architectural decision, weakening a proof/failure boundary, or adding an external Cargo dependency.

**Commit**

```text
pixels P6.9: bound reconstructed normals and materials
```

## Task P6.10 — implement transfer, transparency-tail, and byte kernels

**Requires:** P6.9.

**Produces:** Complete output-referred proof arithmetic.

**Files:**

```text
crates/wrela-compiler/src/pixels/reference/transfer.rs # new at P-1 basis
crates/wrela-compiler/src/pixels/reference/display.rs # new at P-1 basis
stdlib/core/render_transfer.wr # new at P-1 basis
formal/pixels/Pixels/Compositing.lean # new at P-1 basis
formal/pixels/Pixels/TransparencyTail.lean # new at P-1 basis
formal/pixels/Pixels/DisplayByte.lean # new at P-1 basis
formal/pixels/KERNELS.txt # new at P-1 basis
```

**Contract/dump delta:** Only the contract and stable-dump changes explicitly named by this task are permitted.

**Work:**

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

**Tests:**

- Composition associativity holds in formal reals; machine implementation includes rounding radius in interval path.
- Bright low-alpha tail fixture is not cut early.
- Endpoint singleton never accepts an interval crossing a code boundary.
- Rust/Wrela outputs agree for all vectors.
- Formal theorems build.

**Focused checks:** Run the focused tests, fixtures, dumps, and commands named by this task before the repository gate.

**Repository gate:**

```text
cargo xtask verify
cargo xtask pixels-formal
```

**Stop conditions:** Stop if an acceptance condition cannot be met without changing a closed architectural decision, weakening a proof/failure boundary, or adding an external Cargo dependency.

**Commit**

```text
pixels P6.10: verify transfer and displayed bytes
```

## Task P6.11 — implement kernel manifest and differential boot lane

**Requires:** P6.10.

**Produces:** Make cross-language correspondence a permanent repository gate.

**Files:**

```text
formal/pixels/KERNELS.txt # new at P-1 basis
crates/xtask/src/pixels_formal.rs # new at P-1 basis
crates/xtask/src/pixels_vectors.rs # new at P-1 basis
stdlib/tests/pixels_numeric.wr # new at P-1 basis
tests/golden/boot-pixels-numeric/ # new at P-1 basis
```

**Contract/dump delta:** Only the contract and stable-dump changes explicitly named by this task are permitted.

**Work:**

Complete theorem-to-kernel mapping. Generate a Wrela numeric test image that runs scalar kernels against embedded vectors and prints/digests results. Host xtask computes expected results through Rust reference.

The boot lane runs the complete deterministic vector set in `verify`; `verify-deep` repeats it as part of the exhaustive release diagnostics.

**Tests:**

- Every P6 required kernel has Lean, Rust, and scalar Wrela references. Packet mappings are added when their implementation lands in P8/P12.
- Guest output equals host expected bytes.
- Missing/renamed symbol breaks manifest gate.
- No expected result is authored twice by hand.
- Numeric boot test contains no renderer scene logic.

**Focused checks:** Run the focused tests, fixtures, dumps, and commands named by this task before the repository gate.

**Repository gate:**

```text
cargo xtask verify
```

**Stop conditions:** Stop if an acceptance condition cannot be met without changing a closed architectural decision, weakening a proof/failure boundary, or adding an external Cargo dependency.

**Commit**

```text
pixels P6.11: gate theorem-to-kernel correspondence
```

## Task P6.12 — close formal trust-boundary theorems

**Requires:** P6.11.

**Produces:** Finish the generic mathematical foundation before runtime consumes certificates.

**Files:**

```text
formal/pixels/Pixels/RunCertificate.lean # new at P-1 basis
formal/pixels/Pixels/EventCover.lean # new at P-1 basis
formal/pixels/Pixels/TrustBoundary.lean # new at P-1 basis
formal/pixels/Pixels.lean # new at P-1 basis
formal/pixels/EXPECTED_AXIOMS.txt # new at P-1 basis
```

**Contract/dump delta:** Only the contract and stable-dump changes explicitly named by this task are permitted.

**Work:**

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

**Tests:**

- No admissions/project axioms.
- Every theorem hypothesis maps to a concrete record field or compiler verifier fact documented inline.
- Unused stronger hypotheses are removed.
- Kernel manifest references final theorem names.
- `cargo xtask pixels-formal` is green from a clean formal build.

**Focused checks:** Run the focused tests, fixtures, dumps, and commands named by this task before the repository gate.

**Repository gate:**

```text
cargo xtask pixels-formal
cargo xtask verify
```

**Stop conditions:** Stop if an acceptance condition cannot be met without changing a closed architectural decision, weakening a proof/failure boundary, or adding an external Cargo dependency.

**Commit**

```text
pixels P6.12: close the renderer formal trust boundary
```

### Milestone P6 close

Run `cargo xtask verify`. Do not implement the sweep until all certificate predicates it will trust have scalar Rust/Wrela correspondence and the formal trust-boundary theorem set is green.

---

# Milestone P7 — from-scratch validated scanline sweep

Milestone result: the generated renderer constructs complete, exact visibility from only `FrameProgram`, current frame inputs, and bounded workspace. It handles first frame, camera cuts, and arbitrary valid frame changes. It emits certified runs or returns a render error; it never needs previous-frame state for correctness.

## P7 as built — recorded deviations that supersede task wording below

These deltas keep every P7 obligation but change the mechanism; where a task
below disagrees with this list, this list is authoritative.

1. **Box-complete collection replaces point-collect-then-box-certify
   (P7.5/P7.8/P7.9).** Root collection widens every primitive coefficient by
   its sealed uv partial bound and runs one bisection walk whose verdicts —
   certified crossing, proven root-free, unresolved corridor — hold over the
   whole query box. Smooth-object tubes are box-evaluated during isolation.
   A span is certified by one box-complete sample; there is no second
   certification pass and no per-feature exclusion back-check, because
   omission of a feature root over the box is impossible by construction.
   The implicit jet (P7.8) is still constructed and remains decision-inert;
   it lives on the certified row record rather than every visibility sample
   so sample copies stay cheap. Every unresolved cell of a walk collapses
   into one conservative corridor per feature (or smooth object) covering
   the deepest unresolved face and everything farther, certified crossings
   nearer than the corridor survive it, and the walk prunes cells already
   inside the corridor — the visibility outcome is unchanged (any corridor
   in front of the boundary fails the sample; corridors behind it are never
   consulted) while ambiguous domains stay linear in the resolved frontier
   instead of exponential in the depth cap. A span overlapping a sealed
   material-boundary event region resolves per pixel so material coverage
   is charged to corridors. Residual assumption: the near-plane CSG
   occupancy seed is evaluated on the centre ray only, exactly as before
   this rework.
2. **Sealed CSG programs compile to straight-line Boolean code (P7.6).** The
   frame-program stack machine is compiled at image generation into a
   per-renderer Boolean function; a program the old interpreter would have
   rejected fails at generation. There is no runtime CSG interpreter.
3. **Worker-error taxonomy (P7.9/P7.10/§2.6).** Codes 4, 42–50 and
   61–77 (analytic silhouette coverage sub-codes, 60 + cause) plus 100 map to
   `CertificateExhausted`; 16 (per-row candidate/proposal records) and 17
   (root/run/corridor records) map to `CapacityExceeded` with the code as the
   capacity kind; 2 maps to `EventIsolationExhausted`, 6 to
   `RootIsolationExhausted`, 7 to `FixedPointRangeExceeded`; everything else
   is an internal invariant.
4. **The coverage integrand carries its owner (P7.10).** A silhouette
   coverage byte with no certified point hit resolves its identity from the
   accepted event's sealed participant list (first feature, else first
   object), then from deterministic corner rays, and otherwise fails closed
   with reserved code 50. Coverage is never silently discarded as background.
5. **Derivative-tube error budget (P7.8, prior finding 8).** The generated
   `object_q_tube` scales its rounding allowance by the cluster's scalar
   schedule size (per-op 2^-23, floored at the historical 2^-16) and adds a
   secant evaluation-error term that scales with 1/width, so narrow cells can
   no longer fake derivative certificates. Remaining assumption, tracked for
   closure by interval evaluation of the schedule: face/radius magnitudes are
   used as the intermediate-magnitude proxy of a running-error bound.
6. **Instrumented versus production layouts are real (P7.3).**
   `wrela test --pixels-telemetry` selects the instrumented renderer layout;
   plain runs and recorded goldens pin the production layout, which stores no
   telemetry or run evidence. Conformance proves telemetry decision-inertness
   by requiring the instrumented displayed-frame digest to equal the golden
   production digest byte-for-byte.
7. **Renderer display widths must divide by 8 (P7.13).** Framebuffer color
   and write-once-marker words are shared u64 cells updated by plain
   read-modify-write; only widths divisible by 8 guarantee no word spans two
   workers' pixels across a row wrap. The mode is rejected at seal time.
8. **Per-pixel conformance transport (P7.14).** Instrumented fixtures emit,
   after their standard observation words, a frame dump: the validated
   snapshot camera and packed parameters followed by the complete debug
   framebuffer. The host oracle consumes that camera/parameter state (never a
   guessed canonical camera), scores every pixel — interior pixels must match
   hit state and identity exactly, silhouette-adjacent pixels must not be
   background when the centre ray hits and may only show an adjacent
   surface's identity, and any oracle-unresolved ray fails the case — and
   proves one/four-worker invariance over the full frame bytes. Identity
   truth comes from the semantic feature leaves (smallest |leaf| at the hit
   point, abstaining on blend seams). The machine console contract was
   enlarged (4096 descriptors, 128 KiB data) to carry the dump; the console
   remains an append-only bump channel. The one-core plane and tangent
   fixtures formerly returned immediately before their instrumented dump
   hooks; both returns now follow the dump loop. The plan lint rejects this
   exact unreachable-hook pattern so production-only green transcripts cannot
   defer the failure to the expensive instrumented lane. The four-worker plane
   control reads worker 3's final-tile run, matching the sole worker's final
   run in the one-worker layout; raw evidence equality now compares the same
   spatial certificate rather than two partition-dependent record-zero slots.
9. **Golden policy.** A `check-pixels-*`/`boot-pixels-*` fixture transcript
   containing a failing test can be neither recorded nor accepted by the
   golden runner; recording a red acceptance fixture is impossible by
   construction.
10. **Machine layout headroom.** The image packing window is 4 MiB
    (`RTDATA_BASE = IMAGE_BASE + 0x40_0000`); branchable text must still fit
    one 2 MiB region per SOG §4.8, which the slimmed sample record keeps
    comfortable. `AdrAddressing` stays parked (DRAM pages moved beyond ADR
    reach when the image base moved), with its reloc machinery kept alive by
    force-enabling it inside the layout unit test.
11. **Order-invariant coincident groups.** When an overlap group shares one
    identity, the composite is currently unoccupied, and every possible
    first toggle occupies it, the first crossing is a composite boundary
    under every ordering of the unresolvably close roots — the selector
    accepts it as a visible hit. A thin slab's enter/exit pair and a
    tangency's coincident surfaces resolve here instead of dissolving into
    event ambiguity.
12. **Graceful work caps everywhere refinement can grind.** Every
    isolation/refinement loop resolves its budget through a graceful check
    that classifies exhaustion (`CertificateExhausted`) strictly before the
    `@budget` trap can fire: the q walks carry visited-cell caps (8192 for
    primary samples, 64 for subpixel arrangement cells, whose separation
    belongs to the uv ladder), the subpixel quadtree caps at 16384 cells,
    and the analytic-coverage refiner at 200000. The byte-driven early stop
    ends refinement the moment no undecided unit can change the rounded
    coverage byte. `CertificateExhausted` reports the worker sub-code in the
    high half of its tile word, so red traces name their cause.
13. **Known P7 residue (open, tracked) — one root cause, four symptoms.**
    The remaining P7 gap is *not* four independent geometry problems and is
    *not* a precision problem. A pixel is decided either by a closed-form
    analytic coverage integrator, or by the subpixel walk. Boundary shapes
    with no integrator force every pixel they cover into the walk; the walk
    then either exhausts its 262144-evaluation budget (worker error 42) or
    merely costs minutes. That single gap produced four symptoms.
    **`check-pixels-displace` is now CLOSED** — and the cause was not a
    missing integrator at all, but a sealed *envelope constant*.
    `SOURCE_TRIG_{VALUE,GRADIENT,HESSIAN,THIRD}_FACTOR_V1` were `4/8/32/128`,
    inflating this scene's displacement bounds to `A = 0.5`, `G = 2.0`
    against authored `0.125`/`0.25`. On a radius-1.0 sphere that widens the
    silhouette to a ~2px band, so the geometry was ambiguous under the
    *sealed* bounds while perfectly determined under the true ones, and the
    walk could never converge on the single pixel sitting on the silhouette
    (measured: x=30, y=12, ray distance 1.007). Re-derived to
    `_V2 = 1.0009765625` with a proof: the factors bound `sup |p^(k)|` of the
    folded source polynomial, which is exactly 1 for true sine and ~1.000006
    for the pinned f32 coefficients, while the f32 evaluation envelope is 12
    rounded operations — an *absolute* ~4e-6 term. A *multiplicative* 4x
    overstated it by six orders of magnitude. The fixture went from 75s-red
    to **13.8s-green**, and the sealed Taylor remainder fell 2993 -> ~23. A
    second blocker sat behind it: `feature_normal` emits no case for a
    displaced feature, which is now treated as *no normal claim* rather than
    synthesizing the primitive's (wrong) normal. The symptoms present during
    the fix wave were
    `check-pixels-hard-csg` (feature boundaries whose `sparse-predicate`
    curve lives at a per-pixel root q rather than a sealed fixed q, so the
    clip-curve treatment of deviation 19 does not reach it), and — as time
    rather than failure — `check-pixels-torus-roots` and
    `boot-pixels-renderer-unavailable`. `check-pixels-enclosed-feature`
    passes but takes 3m24s by the same mechanism. Their closure is recorded
    below; this paragraph remains as the diagnostic history that led to it.

    Three plausible theories were tested and **refuted**; do not re-derive
    them. (i) *Coefficient widening is not the noise floor.* Cutting
    `interval_from_f32`'s `magnitude / 65536 + 64` by 16x left displace and
    hard-csg failing identically, which retires the "extend f32 error terms
    into the walk" plan recorded in earlier notes. (ii) *The refinement loop
    is not the wall-clock cost.* Adding a sound byte-driven early stop to
    `silhouette_coverage`'s refinement quadtree moved
    `check-pixels-enclosed-feature` from 3m28s to 3m24s. The early stop is
    retained (it is correct, and it bounds degenerate curves) but it is not
    the perf lever. (iii) *The large sealed bounds are inert.* displace seals
    `remainder = 2993.35` and torus bounds up to `4.29e6`, but the guest reads
    no representation operands at all: scaling every one of them by 1e-9
    leaves every generated guest numeric kernel byte-identical. Note also
    that `remainder = M3 * delta^3 / 6` is computed over the whole feature
    AABB (`world_delta_abs_bound` = 5.196); because it scales as `delta^3`,
    evaluating the same sealed Taylor model over a pixel-sized cell is ~1e6
    times tighter and needs no change to any sealed constant.

    (b) A surface passing exactly through the camera eye (admitted by a
    parameter range) converges only through deep refinement. This is the
    dominant cost of `boot-pixels-renderer-unavailable`, whose two
    `phase = 0.0` frames put the ground plane through the eye: measured, 51s
    with them and 12s without, against the VMM's 30-second `WALL_CAP`. The
    frame is byte-exact and complete. For a degree-1 ray polynomial the
    covered set is exactly `{ g(q_far) * g(q_near) <= 0 }`, so it is bounded
    by the two clip level sets deviation 19 already computes; when the
    leading coefficient vanishes identically that product is a perfect
    square and the hit set is a measure-zero line, i.e. coverage 0. The cap
    must not be raised to hide this, since it is also the only watchdog on a
    genuinely spinning guest. (c) The busy-guest wall cap only bounds parked
    sleeps; a spinning guest is killed by the harness watchdog, not the VMM.

    (d) *A red fixture was a compiler bug, not a renderer bug.*
    `boot-pixels-frame-input` compared two identical `u64` digests as
    unequal. Root cause in `frame_plan.rs::same_state_interference`: it never
    modelled that a definition writes its register even when the defined
    value is dead, so a dead def could take the register of a value live
    across a suspend and the await flush stored the wrong value. Gated on the
    `FlowStateRegs` optimization, so only optimized builds were affected.
    Fixed with the standard def-vs-live-out rule, with a regression test and
    no golden drift. Any future pixel red should be checked for
    reproducibility *without* the renderer before geometry is suspected.

    **CLOSED (2026-08-09 review remediation).** The remaining symptoms are
    closed in-tree. Hard CSG uses the oriented predicate eliminant and
    integrates sibling predicates jointly; torus roots use the centred
    Hessian bound, adaptive refinement, and event-keyed torus caches; the
    renderer-unavailable clip path uses exact-rational half-plane coverage.
    The union integrator now prefilters at most eight pixel-relevant events
    once per pixel instead of rereading the full tile table for every
    refinement cell. `check-pixels-hard-csg`, `check-pixels-torus-roots`, and
    `boot-pixels-renderer-unavailable` are permanent green runtime fixtures,
    and `check-pixels-enclosed-feature` remains the structural-discovery
    performance witness. There is no open P7 residue under this deviation.

    The canonical standalone torus now has a sealed analytic tier rather than
    depending on the generic arrangement walk for its final coverage byte.
    The compiler emits the normalized discriminant in `X = u^2`, `Y = v^2`
    together with outward coefficient intervals, point/cell P-Q
    classification, and derivative magnitudes. Coefficients are evaluated by
    short compensated-f32 Horner chains (error-free `TwoSum`/`TwoProduct` plus
    an explicit second-order remainder), guarded to the finite camera range
    `0.125 <= eye <= 64`; outside that range the optimization declines. The
    guest evaluates the bivariate polynomial by outward interval Horner and
    carries `(value,error)` at every cached sample. Three deterministic host
    tests range the guarded eye domain against f64 expanded-polynomial truth,
    range representative `(u,v,eye)` points against the complete normalized
    discriminant, and check that every P-Q box classification agrees with a
    dense f64 sample lattice. These are containment tests, not golden-output
    snapshots.

    Refinement uses a three-point affine basis per cell (`f00`, `f10`, `f01`)
    with individual evaluator errors; `f11` is enclosed by that basis plus the
    centred Hessian residual. A split evaluates only the five new shared
    vertices and maps the inherited basis into its four children. The tier
    attempts the fixed precision ladder `1048576`, `64000`, then `4000`; each
    rung remains fail-closed, and the generic arrangement walk is the final
    fallback. This is the measured bridge to the Green's-theorem/conic
    endgame described in the review: it preserves the same rigorous
    second-order certificate while avoiding an unbounded rewrite at P7.

    Finally, the row certifier no longer contains the whole adaptive walk as
    one generated function. Ownership/corner resolution, axis choice, and the
    weighted arrangement walk live in
    `resolve_silhouette_owner`, `choose_arrangement_axis`, and
    `resolve_pixel_arrangement`, with `P7PixelArrangement` as their explicit
    result. Besides making the proof boundaries readable, this cuts the row
    assembly function from roughly 128k to 36k instructions and leaves normal
    branch-region headroom without weakening the layout check.

    The thin-feature close exposed one further analytic case in the same
    validity-predicate tier. An affine-in-q feature can have a leading
    coefficient `A_f` whose sign is not global over the full declared camera
    box even though `A_f` is affine (often constant) in `(u,v)` for a concrete
    frame. The compiler now emits its predicate eliminant in that case. At
    runtime each descriptor records the local sign of `A_f`, and the cell proof
    requires the same strict sign at all four corners before using
    `sign(A_f) (A_f S_p - A_p S_f)` or the forward-root witness. Because the
    admitted `A_f` is affine in `(u,v)`, four equal corner signs prove that it
    cannot cross zero inside the cell; a zero, mixed sign, or nonlinear
    unsealed coefficient declines to the walk. Predicate siblings are recorded
    as classified only after their representative's complete region proof or
    arrangement-boundary proof succeeds. `check-pixels-thin-feature` is the
    permanent runtime regression: it passes with the original 2^24 area ledger,
    2^-12 minimum radius, and 262144-evaluation cap. Experimental deeper
    ledgers, axis-reuse heuristics, and a repr-2-only depth extension did not
    address the cause and were removed.

    Predicate-event branches now emit metadata only (polynomial IDs,
    orientations, curvature bounds, and sibling IDs) and feed one shared
    eliminant evaluator. This prevents constant event IDs from cloning the
    checked polynomial accessor at every branch. The largest field-ops fixture
    falls from the 2.40 MiB rejected layout to about 1.79 MiB of executable
    text, leaving normal headroom below the 2 MiB branch-region boundary.

14. **The event-arrangement tier is the side-state extension of the group
    walk (P7.10 ladder step 5), not a separate pass.** Three mechanisms
    compose. *Partial roots:* a certified crossing whose feature-validity
    predicate is mixed over the query box appends a corridor record with
    the certified (localized) bracket — the validity boundary is a
    silhouette event curve crossing the cell, and failing the collection
    there made every silhouette-straddling cell ambiguous at every scale.
    The smooth walk likewise corridors-izes active-feature ambiguity, with
    an identity-only owner fallback when the owning feature (but not the
    displayed identity) is mixed. *Bracket localization:* every accepted
    certificate bisects toward its sign change until the interval tube
    width, because sheet-scale brackets poison validity (evaluated over the
    whole q-range) and fuse every root into one overlap group. *Side-state
    selection:* the selector builds overlap groups over certified and
    corridor members alike; a group with at least one certified crossing,
    one shared identity, an unoccupied composite, and every possible first
    toggle occupying resolves as a visible hit in every interleaving of
    every corridor side state (the certified member guarantees a toggle
    occurs; the proof margin is the tightest *certified* margin, since a
    corridor's positional uncertainty already lives in the union span).
    Groups still carrying corridors past these rules become integrable
    event samples — never a pass-through, because a corridor's crossing
    parity is unknown and the composite state behind it undefined.
15. **Validity predicates evaluate in f32 (P7.10).** The raw fixed-point
    grid's quantization noise after a degree-8 Horner walk is tens of raw
    units — the same order as the geometric margins of subpixel features —
    which turned measure-zero validity boundaries into wide undecidable
    skins. The generated `feature_valid_filter` now resolves predicate
    signs from f32 endpoint evaluations with three explicit soundness
    terms: the uv box radius, a mean-value derivative bound spanning the q
    bracket, and a relative rounding allowance far above the true f32
    error.
16. **The subpixel tier is an anisotropic priority walk (P7.10 ladder step
    6).** Cells carry explicit unit weights; ambiguous cells split in two
    along one axis. The axis comes from two probe tiers — half decidability
    first (an axis-aligned boundary is walked in O(depth) splits), then
    zeroed-radius attribution (a uniform strip splits only its productive
    axis) — with a larger-axis fallback for oblique boundaries. The walk
    pops the heaviest cell first so bulk regions decide before boundary
    strips refine and the byte-driven early stop fires as soon as the
    remaining strips cannot change the rounded byte, draining lightest-
    first only when the 72-slot frontier nears its bound. The work budget
    counts visibility evaluations (probes included) with a fixed 262144
    ceiling, and exhaustion still reports code 42 carrying the walk state.
17. **Worker error codes are a single-source table.** Every code with
    defined semantics lives in `pixels::worker_errors` (name, class,
    documentation); the guest-side classifier `__wrela_pixels_p7_worker_error_class`
    is generated from it, the host decodes trace words with it, and a unit
    test asserts every `failure_with` literal and every direct
    `__wrela_pixels_p7_worker_error` literal in `render.wr` is either
    registered or a deliberate internal-invariant code. `run_job` preflight
    paths have a separate regression pin requiring invariant codes 5, 8, 9,
    and 11 — the drift that produced the B1 misclassification can no longer
    compile.
18. **Named P7.1/P7.2 fixtures exist retroactively.** `boot-pixels-program-view`
    pins checked sealed-table access and fail-closed indexing;
    `boot-pixels-frame-input` pins exact validation errors, from-scratch
    rendering without a prior snapshot, and snapshot determinism via digest
    equality across identical frames. The conformance harness runs one
    parallel golden pass over its case set (exact-name selection) and a
    two-slot instrumented-run pool instead of fifteen serial invocations.
19. **A clip boundary is a curve, and the coverage integrator takes it as
    one.** The horizon band was never a numeric-noise problem. Measured on
    `boot-pixels-renderer-unavailable`, every box along it resolves cleanly
    at every scale down to radius 1e-7; the selector correctly reports an
    *integrable event*, and the walk then burned its whole 262144-evaluation
    budget only because no analytic integrator claimed the pixel. Two things
    were missing. First, the scene's only silhouette is a
    `LinearLeadingCoefficient`, which the integrator's `representation == 2`
    gate excluded — and for a plane that representation is a *degeneracy
    guard*, not a horizon: its polynomial is the constant plane offset,
    whose zero set is empty (the eye would have to lie in the plane).
    Second, the boundary that actually cuts those pixels is the **far clip**,
    which carries no curve of its own. It is the level set of the owning
    feature's ray polynomial at the sealed clip `q`, so
    `__wrela_pixels_p7_event_polynomial` now emits that polynomial for
    `ClipQ` events, `__wrela_pixels_p7_event_clip_q` seals the `q`, and the
    uv² bounds are taken with `q` fixed at the clip plane rather than over
    the whole q range (using the full range inflates the residual by
    `q_near / q_clip` and stops the byte pinning). Three rules keep it
    sound. A curve that provably misses the pixel bounds nothing and is
    skipped, so a non-crossing curve can never overrule the boundary that
    does cross. Where *every* occupancy-bearing event covering the pixel
    reduces to a curve and every one of those provably misses it, occupancy
    cannot change inside the pixel and the centre ray names the whole area —
    this is what retires the rows just off the horizon, conservatively box
    ambiguous but geometrically uniform. Where two curves cross one pixel
    the occupied region is an intersection of half planes that no
    single-curve area can express, so the presence of a second crossing drops
    the integration back to the subpixel walk even if both individual curves
    round to the same byte. Unlike a discriminant, neither a leading
    coefficient nor a clip
    level set has a sign convention for "occupied", so the side is resolved
    from an occupancy sample at the corner furthest from the curve rather
    than assumed. The result is exact: the far-clip crossing sits 6.25% down
    pixel row 16 and the integrator returns byte 239 = round(0.9375 × 255),
    with 0 above and 255 below.
20. **A pixels acceptance golden must prove a green run, not merely lack the
    word FAILED.** The original lint refused transcripts containing
    `: FAILED`, which a build error, a VMM wall-cap timeout, or an empty
    transcript does not contain — so `--update` recorded
    `boot-pixels-renderer-unavailable`'s 30-second timeout *as its
    expectation*, and every later verify would have ratified that as green.
    The lint now parses a `N passed, 0 failed` summary, requires `N >= 1`, and
    reports which of the two reasons applied. Checking for the absence of a
    failure signature is not the same as checking for the presence of
    success; only the latter is safe to bless automatically.
21. **Compiler-reserved surface is fenced in sema.** Ordinary project modules
    may neither define nor reference identifiers beginning `__wrela_` or
    `RendererWorker`. The check runs on source tokens before specialization
    and therefore closes both spelling-dispatched intrinsic hijacks and
    shadowing of the globally injected Pixels surface. Toolchain stdlib,
    generated modules, live-rtconfig re-checks, and repository-owned golden
    fixtures are trusted compiler inputs; they do not weaken the rule for a
    loaded user package.
22. **Analytic coverage caches are event-scoped.** The projected-union tier
    collects the pixel's relevant repr-2/repr-5 events once (declining beyond
    eight), caches torus magnitude bounds per event, and reuses side occupancy
    only when exactly one curve participates. This both removes the latent
    two-torus cache alias and hoists the full tile-event scan out of the
    refinement loop. The canonical-torus affine sample cache is narrower
    still: it is enabled only for the one-event standalone-torus shape, every
    stored sample carries its own evaluator error, and no sample can be replayed
    for a different event or curve representation.
23. **P7.14 scores concrete frame and run evidence without inventing point
    claims.** Instrumented guests emit the complete 64x32 BGRA frame plus one
    stable 128-byte certified-run record. Coverage-header word 3 now always
    carries row `y`; word-14 bit 3 is set only after an instrumented guest
    recheck proves that the run centre is a same-identity point hit whose q
    overlaps the run tube; the independent host still demands full q
    containment. Sampled normal components are claims only when
    normal-model words 10..13 form an exact point enclosure. This prevents
    deformation and composed-CSG runs
    (which deliberately have no closed-form normal claim) from overloading the
    row coordinate with root metadata. Record-zero selection ignores
    coverage-only hits without that centre witness and preserves the latest
    witnessed run deterministically. The independent decoder validates both
    copies of the claim word before the scorer uses it; a selected background
    run must provide a resolved semantic miss at its centre. Frame scoring pulls its
    four probes inside each pixel so measure-zero shared edges do not become
    sample ownership, but five misses establish background only when a full
    pixel-ray-frustum interval excludes zero; enclosed or thin features between
    probes therefore remain unclassified rather than becoming false misses.
    The 16x16 alpha lattice is a bounded proximity control (±12 codes), not an
    area proof. At terminal root cells, endpoint signs orient simple crossings
    even when a nonsmooth derivative hull spans zero, and merging an adjacent
    unoriented enclosure cannot erase an already proved orientation. Finally,
    parameter-independent finite-repeat event bands partition the semantic ray
    oracle into smooth open segments. Equal nonzero one-sided signs discard the
    dependency-only false root at a repeat tie; an actual sign transition is
    retained as a boundary. The permanent repeat probe-lattice regression
    requires every centre/inset ray to resolve.
24. **Review remediation (2026-08-10).** A read-only review of the milestone
    found one latent soundness hole, one unmet acceptance criterion, and four
    drift hazards. All are closed here.

    *The projected-union tier could claim area it had not proved.* Its
    inference is "the discriminant's sign is fixed over this cell, so this
    member's occupancy is constant over it, so one sample decides the cell",
    and both halves could fail. A sample reads `point_union_occupancy`, which
    answers for the *whole* composite: in a multi-member union, a member whose
    own boundary crosses the cell could answer it, and the tier would then
    credit the full cell to a member that covers only part of it. Separately,
    a fixed discriminant sign proves a constant root count on the whole ray
    *line*, which says nothing about whether those roots fall between the near
    and far planes. Neither could be reached by a permanent fixture — every
    one of them has a single union member — so the tier shipped green.

    The fix is structural. Sampled conclusions are now *deferred* until every
    tracked member's sign over the cell is known: with none ambiguous the cell
    is uniform and one reading decides it in both directions, and with one
    ambiguous no reading can be attributed to a member, so the cell carries
    its whole area as uncertainty instead. The prefilter additionally declines
    any pixel covered by an occupancy-bearing event the tier does not track —
    the `unclassified_boundary` rule its sibling `silhouette_coverage` already
    had — with three exclusions: projected bounds carry zero coverage measure,
    a finite repeat fold re-parameterizes the domain without bounding anything
    its instances' own (tracked) silhouettes do not, and a clip plane is
    admitted per pixel by proving its level set misses, reusing the
    deviation-19 curve treatment
    (`__wrela_pixels_p7_clip_curve_misses_cell`).

    What is deliberately *not* deferred is the structural conclusion that a
    strictly positive quadratic discriminant fills the cell. The distinction
    matters and cost a fixture to get right: that conclusion is about one
    member's own root count, and because the composite here is a pure union
    and union occupancy is monotone in its members, a member hit throughout
    the cell fills the cell whatever the others do. A *sample* carries no such
    attribution — it answers for the whole composite — which is why the sample
    path, and only the sample path, waits for the ambiguity verdict. The
    standalone torus fast paths likewise keep their structural verdicts: their
    preflight proves single-membership *and* window containment.
    `check-pixels-torus-roots` stays green and got faster.

    One gap in this tier is narrowed but not closed: a fixed discriminant sign
    proves a root count on the ray *line*, and the sealed projected `q` span
    cannot witness that those roots fall between the clip planes, because
    `projection_bounds` intersects that span with the clip window before
    sealing it — the answer is "touches both planes" for every feature in
    every scene. What now stands in for it is the per-pixel clip-miss proof
    above, which establishes that the in-window question has the same answer
    across the whole pixel. Turning that into a positive witness wants the
    sign of the clip polynomial the miss proof already computes and discards.

    *P7.11 row proposals are decision-inert by construction, and that is now
    recorded rather than implied.* The proposal computes its revalidation match
    and telemetry but does not seed the attempted span width, because the
    emitted partition is chosen by a halving retry ladder and the debug alpha
    encodes a span-scoped `q` interval — so seeding would change frame bytes
    and violate this task's own "identical bytes" acceptance condition. The two
    are reconcilable only by making the debug image partition-independent,
    which belongs with the P8/P9 rasterization contract that replaces it. What
    *was* a defect is that the three required counters were not
    distinguishable: `proposed` and `revalidated` were charged the same value,
    so the path was unmeasurable. They now mean offered, matched, and newly
    discovered, matching the Rust reference's `ProposalCounts`.

    *The conformance frame scorer silently skipped what it could not prove.*
    An all-miss pixel whose frustum interval straddled zero incremented no
    counter and failed no check, and a single unsubdivided evaluation over the
    whole sealed depth range straddles constantly: `check-pixels-close-depth`
    scored 8 pixels out of 2048. The frustum proof now bisects its widest axis
    under a fixed cell budget, and any pixel still unproven is reported as
    `frame_skipped` so lost coverage is visible in the score line instead of
    being indistinguishable from a pass.

    *Three single-source repairs.* Event kinds and representations were
    restated as inline literals in `render.wr` (`kind[1] == 3`, a hand-written
    occupancy set), so a new occupancy-bearing kind added to the Rust
    vocabulary would have silently unsoundened the "every boundary provably
    misses, so the centre ray names the pixel" rule. `pixels::event_kinds` now
    owns the classification with exhaustive matches, and
    `__wrela_pixels_p7_event_class` is generated from it. The generated
    intrinsic surface needed four hand-edited syncs and had drifted in all of
    them; `pixels::surface` is now the one table the loader stub, both sema
    binding lists, and the drift tests derive from — it retired two sema
    bindings and five stub entries that named functions no generator emits.
    Its sharpest test catches a stdlib module *calling* a generated intrinsic
    it does not import: because the pixels prelude injects those names, such a
    call binds silently and only misbehaves at runtime.

    *The reserved-name fence.* Trust followed `CARGO_MANIFEST_DIR`, so a
    relocated toolchain or a project vendoring its own `stdlib/` — both
    supported by the loader — made the stdlib itself fail the fence. Trust now
    follows the stdlib *layout*. Repository fixtures that legitimately name the
    surface declare it with a `@wrela-compiler-internal` comment directive
    instead of inheriting trust from `tests/golden/`, which is what makes the
    fence testable at all: `err-pixels-reserved-name` is the permanent
    rejection fixture, and it could not have existed while the whole directory
    was trusted.

    *What the strengthened scorer found, and what is now open.* Raising the
    proven-background coverage immediately surfaced pixels on four fixtures
    that the guest paints although the whole ray frustum is provably free of
    the semantic surface. Measured, not inferred: for
    `check-pixels-simultaneous-event` (22,11), `check-pixels-displace` (29,11),
    `check-pixels-repeat` (19,14) and `check-pixels-close-depth` (29,14), a
    dense lattice of roughly 125000 field samples across each pixel frustum
    finds no sign change at all, agreeing with the interval proof.

    Three of the four were a scorer artifact, now fixed. A zero-coverage event
    pixel keeps its identity bytes in RGB while alpha is 0, and the scorer read
    any non-zero RGB as painted; alpha is what decides whether anything is
    displayed, so it decides this now. That cleared
    `check-pixels-close-depth`, `check-pixels-repeat` and
    `check-pixels-simultaneous-event` outright, and each of them now scores
    every pixel of its frame with zero mismatches.

    `check-pixels-displace` is real and open: 28 pixels carry a saturated
    point hit where no surface exists — the same family as the five silent
    correctness bugs the earlier fix wave uncovered, and consistent with that
    fixture's history of the sealed displacement envelope driving decisions.
    They are counted as `frame_phantom`, pinned in
    `tests/pixels_truth/p7-visibility.txt`, and excluded from the pass
    predicate, so the gate fails the moment the count grows while the cause is
    diagnosed rather than papered over by a weakened oracle.

    The diagnosis is narrowed to one tier by guest probes at the affected row
    (`y = 11`, `x = 29..37`), and the next session should start from these
    measurements rather than repeat them. The centre ray carries **no roots at
    all** at every one of those pixels (`debug_probe`: `hit=0, roots=0`), so
    the point path agrees with the oracle that nothing is there. The analytic
    coverage integrator also behaves correctly: `debug_probe_coverage` returns
    `definite=0` for `x = 29..34` — it declines — and `definite=1, byte=0` for
    `x = 35..37`, which is why those three are background. Yet
    `certify_pixel_row` emits, for all nine, an **event sample** (`hit=1,
    event=1`) whose coverage is `255` on the declining pixels and `0` on the
    integrated ones. So the phantom byte is neither a false root nor a wrong
    integral: it is the subpixel arrangement tier resolving a declined pixel
    into a saturated event sample. `select_visibility` returns event samples
    carrying a *placeholder* coverage (254/255) that is not an integrated
    area, and the suspect is that placeholder reaching the framebuffer when
    the arrangement walk resolves no hit area of its own.

    The byte's provenance is settled: `event_coverage` short-circuits
    (`proof_method & 127 == 3 and composition_shape == 4`) and passes the
    arrangement's own byte through, so 255 means the walk set
    `selected_units == coverage_units` — it attributed the entire pixel to a
    hit.

    **Two consumer-side corrections were tried and both are refuted.** First,
    requiring a terminal cell claiming a hit to agree with the *pixel-level*
    seed changed nothing at all (`frame_phantom` stayed at exactly 28): that
    seed is itself an event sample here, not a definite miss, so the guard
    never fired. Second, moving the witness to the *cell's own* centre ray —
    the correct witness, since the centre lies inside the cell — does fire,
    and turns the fixture **red**: withholding those cells leaves the pixel
    unable to pin a byte and it fails closed. That is the important result. It
    means the contradiction is not a stray cell the walk could simply decline;
    the box samples claim hits across enough of the pixel that refusing them
    removes the walk's whole basis for an answer.

    A third attempt went upstream, where that reasoning pointed: a
    bounded-displacement feature's ray polynomial describes the *undisplaced*
    primitive, so a sign change in it over a uv box predicts rather than
    witnesses a crossing of the rendered surface. Downgrading those certified
    crossings to corridors whenever the query box has nonzero radius (with a
    generated per-feature `BoundedDisplace` predicate) also left
    `frame_phantom` at exactly 28. So the hits are not certified crossings of
    the displaced feature's base polynomial either.

    That third attempt was then explained by dumping the root records
    themselves, which is the measurement all three attempts should have
    started from. Over the pixel box at `(29, 11)` and `(30, 11)` the
    collector returns **exactly one record, and it is a corridor**
    (`crossing = 0`, `method = 2`, `feature = 0`) — not a certified crossing.
    So there was nothing for the downgrade to downgrade, and the phantom
    coverage is produced by *corridor* handling rather than by any certified
    root. (The feature's occurrence path is `[Primitive, BoundedDisplace]`, so
    the displaced-feature predicate did match; the attempt was inert for lack
    of certified crossings, not for lack of applying.)

    The last piece: `__wrela_pixels_p7_object_composed_root_r0` seals mask
    `1` for this renderer, so object 0 *is* a composed root and
    `collect_roots_box` routes this feature through `isolate_smooth_object`,
    never through `isolate_power_roots`. That is conclusively why the third
    attempt was inert — its downgrade sat in the branch this scene does not
    execute.

    Taken together the four attempts characterise the defect, and it is not a
    check that can be tightened. A single corridor member makes
    `select_visibility` return `hit = true, event = true` via its
    `group_has_corridor` branch, and the arrangement walk then has to turn a
    pixel whose only evidence is one corridor into a coverage byte. Every
    correction that refuses to manufacture that byte — withholding the cell
    (attempt 2), or corridor-ising the crossings that feed it — makes the
    pixel fail closed instead, because **no tier can currently produce a
    correct byte here**. The bounded-displacement silhouette has no analytic
    coverage treatment: deviation 13 records that `feature_normal` emits no
    case for a displaced feature and that repr-4 has no closed form, and
    `silhouette_coverage` accordingly drops it into the unclassified bucket.
    The 28 pixels are that gap, papered over by the walk attributing full
    coverage to a corridor.

    A fifth attempt used the one rigorous repr-4 tool that already exists.
    `deformation_silhouette_misses` merges two proofs; its *exterior* branch
    (`D + 4aB < 0`) shows every ray of the cell passes strictly outside the
    displaced surface, which is a background proof rather than a mere
    no-crossing proof. Separating the branches and concluding coverage 0 when
    every candidate feature is proven absent also left `frame_phantom` at 28 —
    because these pixels sit in the near-silhouette band (the ray passes about
    1.007 from a radius-1.0 sphere), which is exactly where the sealed
    displacement envelope is too loose to prove absence. That bounds the
    missing work precisely: the exterior test already handles pixels away from
    the surface; what has no treatment is the band the silhouette actually
    passes through.

    **The tier is built, live, and removing phantom pixels: 28 -> 12.**
    `deformation_sphere_miss_model` seals the amplitude `A`, the radius `r`,
    the sine frequency and the phase's parameter slot alongside the existing
    band; `__wrela_pixels_p7_interval_sin` in `render.wr` encloses a sine over
    an interval (endpoints plus the `pi/2 + k pi` extrema inside the range,
    padded outward); and `deformation_silhouette_misses` iterates the
    localization — from the current band, take the `t` window where `|H| <= B`,
    hull the ray's world `x` over it, evaluate the sine there, rebuild
    `B = A_local (2r + A_local)`, and retry the exterior test, stopping when a
    round fails to shrink. Every step is fail-safe: an unmet precondition or a
    non-shrinking round returns the pre-existing verdict, so the worst case is
    exactly today's behaviour, and the whole suite stays green with it in.

    The step that made it work was recognising that the bound must be
    *signed*. A surface point sits at radius `r + d`, so `H = d (2r + d)`,
    increasing in `d`; the ray misses when its closest approach clears the
    furthest the surface gets, which is `d_hi (2r + d_hi)`. The original band
    uses `|d| <= A` and so discards the sign — and at exactly these pixels the
    wave is near its *inward* extreme (measured: `sin ~= -1`, pulling the
    surface to radius ~0.875 while the ray passes at 1.007). A magnitude bound
    can never see that; the signed bound goes negative and the miss falls out
    immediately. The same quantity bounds the window, since a ray can only
    meet the surface where `H(t) <= d_hi (2r + d_hi)`.

    Preconditions were confirmed by probe rather than assumed: this scene
    seals `coordinate_x = ScalarOp::CoordX`, `phase_scalar = Param(0)`, and
    `frequency = [2, 2]`, so the guard admits it and the loop runs.

    The second half came from subdividing. A wide cell means a wide bilinear
    coefficient model, a wide `H`, a wide `t` window and a sine range too broad
    to localize anything, so the public entry point now splits the pixel and
    requires *every* sub-cell to prove its own miss — sound because the
    sub-cells cover the pixel, and unchanged in the failing case because an
    unproven leaf still answers `false`. That took 22 down to 12.

    Then the actual defect surfaced, and it was a sign. `sinusoidal_displace`
    adds `d` to the *field*, so the surface solves `|p - c| - r + d = 0` and
    sits at radius `r - d`: positive `d` pulls it **inward**. Therefore
    `H = -d (2r - d)` is strictly *decreasing* in `d`, and the furthest out the
    surface reaches over a window is at the window's **smallest** `d`. The
    guest had localized from `sine[1]`, the upper end, on the reading that the
    surface sits at `r + d`. That was wrong in both directions at once: it
    discarded every case the localization exists to catch (where the wave runs
    positive across the window, pulls the surface inside `r`, and lets a
    grazing ray through), and on the opposite half it produced a band far below
    the true supremum of `H` — an unsound proof. The two halves cancelled at
    the symmetric extreme `sin in [-1, 1]`, which is exactly where it agrees
    with the sealed `A (2r + A)`, so nothing downstream contradicted it.

    Correcting the band to `-d_lo (2r - d_lo)` took `check-pixels-displace`
    from **28 phantom pixels to 2**, with `frame_interior` still 2018/2018 and
    zero edge violations. Two interval-endpoint errors on the same path were
    corrected with it: `sup (D + 4 a B)` takes `a_hi` only when `B >= 0`, and
    with the localized band now genuinely going negative the supremum moved to
    `a_lo`; and `inf (-D / 4a)` divides by `a_lo` only while `D_hi > 0`,
    switching to `a_hi` once the quotient turns positive.

    With the mechanism finally correct, the two caps were re-measured, and the
    earlier exhaustion readings turned out to be artifacts. The `visited` cap
    was 340 while a full depth-4 sweep walks 341 nodes, so *every* depth
    setting above 4 was truncated back to the same work — which is why "depth 7
    changes nothing" looked like a result. With the cap raised, depth 5 still
    proves nothing depth 4 does not, so depth 4 stands, and the cap is now 400
    so the configured depth is actually reachable. Two further levers were
    built, measured at exactly 2, and removed rather than kept as unreachable
    code: localizing the f32 rounding slack from the cell's own coefficient
    hulls, and localizing the tangency proof beside the exterior one. The
    latter cannot help by construction — its sealed `(r + A)^2 G^2` term does
    not localize and on this scene already exceeds what a grazing ray's
    discriminant can clear, so wherever it could fire the exterior test fired
    first.

    A third construction was built for the straddle case specifically and also
    proves nothing: subdividing the *window* rather than the cell — sixteen
    segments, each with its own sine-derived band, each required to clear the
    quadratic's lower bound there. It is sound (outside the window `H > B` by
    the sealed bound, and every segment band is at most `B` since
    `d -> -d (2r - d)` is decreasing and `d_lo >= -A`) but structurally cannot
    fire: inside the window `H <= B` by construction, so a segment clears its
    band only where the wave runs positive hard enough to drag that band below
    the quadratic's floor, and near closest approach the floor is already at its
    smallest. Removed rather than carried as an unreachable branch.

    The last two pixels then fell to a directed probe and a ten-line veto,
    and the mechanism they exposed is worth keeping. A probe of `(37, 13)`
    with `__wrela_pixels_p7_debug_probe_row` showed sample flags 3 — `ok |
    hit` with the **event bit clear**, coverage 255 — while its correctly
    black neighbour `(36, 13)` showed flags 7 with coverage 0. The pixel was
    painted by a *certified point hit*, not by the arrangement walk. Root
    isolation certifies crossings of the sealed `predictor`, which is the
    **undisplaced** primitive's ray equation (this plan's own derivation notes
    its zero set is not the displaced silhouette), and at a grazing pixel the
    base sphere is crossed while the real surface, pulled inward by the wave,
    is not. That is why every absence-proof refinement above measured exactly
    the same number: they were sharpening a proof the pixel never consulted.

    The fix is a corroboration veto in `certify_pixel_row`. When a pixel's
    visibility sample is a definitive point hit (`ok`, `hit`, no event), and a
    bounded-displacement event covering the pixel *exterior*-proves the
    displaced surface absent from the pixel frustum, the winning object cannot
    be contributing occupancy — its features are removed from the candidate
    list and visibility is re-selected from the remainder, which in a
    single-object scene resolves to background. The distinction between the
    miss proof's two halves is load-bearing and now explicit at the call
    boundary (bit 31 of the event id, kept to one argument by the 8-argument
    codegen limit): the exterior test proves no ray point lies on the surface
    — an absence — while the transversal test proves only that no tangency
    exists, and a surface with no silhouette in the pixel may still be
    crossed. Accepting the transversal answer here would erase genuinely hit
    pixels.

    The veto fails closed through four guards, each defaulting to today's
    sample: the winning object must be the one the sealed miss model speaks
    for; that object must consist of exactly the modelled feature (sealed leaf
    count 1 — the proof says nothing about other members of a composed
    union); the sealed start-inside verdict must agree the ray begins outside
    (a set bit contradicts the absence proof, and the veto stands down rather
    than pick a winner between them); and the pixel must lie in the event's
    sealed span, which is where a false predictor crossing can occur at all —
    it requires the base surface met while the displaced one is not, which is
    the silhouette band. Interior pixels cannot prove absence, because the
    surface really is there, so their certified hits are untouched:
    `frame_interior` stayed 2018/2018 with zero edge violations while
    `frame_phantom` went to 0.

    `check-pixels-displace` now scores `frame_phantom=0 frame_first=none`, and
    the truth file pins that, so any reappearance fails the gate.

    *The analytic tiers' pose condition is now a sealed fact, not a per-frame
    gamble.* The renderer declaration takes an optional `camera_pose=`
    argument (a comptime `Camera`, whose authored constructors are closed and
    always produce the canonical `eye/forward/right/up` shape, so the sealed
    value is exactly twelve floats). `__validate_frame` rejects any frame
    whose camera differs, returning `ParameterOutOfRange`. A renderer that
    pins its pose therefore either always satisfies the analytic tiers'
    admission test or refuses the frame outright — the tier can no longer be
    silently unavailable for one frame and present for the next, which was the
    whole of the finding. `check-pixels-torus-roots` pins its pose and its
    report line moves from `pose_conditional=4 camera_pinned=0` to
    `pose_conditional=0 camera_pinned=1`.

    The label is *optional* by construction: `OPTIONAL_RENDERER_LABELS` is
    checked alongside the required set in both `sema::bodies` and
    `pixels::config`, so every existing declaration compiles unchanged and
    only a renderer that wants the guarantee pays for it. Generalizing the
    analytic torus tier to arbitrary poses remains the alternative and is a
    strictly larger piece of mathematics; pinning is the conservative half,
    and it is the half that removes the invisible cliff.

    *The disclosure half, retained.* The
    standalone-torus tier and the point-sampled quartic preflight admit
    themselves by comparing the runtime camera against the canonical pose for
    exact f32 equality. Both are fail-closed — a scene that leaves the pose
    loses the tier rather than producing a wrong byte — but it loses it *per
    frame*, and nothing at build time said so. The compiler report now carries
    `AnalyticTiers pose_conditional=N` per renderer, counting the tiers whose
    availability depends on a runtime pose test, and every pixels report
    golden pins it: the canonical torus scene reports 4, the plane 0. Turning
    the condition into a *proof* rather than a disclosure needs the declared
    camera bounds to pin an absolute pose, and `CameraBounds` deliberately
    bounds inter-frame motion instead — so a sealed answer requires a new
    field in the renderer declaration. That is a contract change, which §2.5
    reserves, and it belongs with the P12 cost admission work that would
    consume the answer.

## Task P7.1 — implement zero-allocation `FrameProgramView`

**Requires:** the preceding milestone close gate.

**Produces:** Read sealed program tables safely from placed image memory.

**Files:**

```text
stdlib/core/render_program.wr # new at P-1 basis
stdlib/core/render_interval.wr # new at P-1 basis
stdlib/tests/pixels_program_view.wr # new at P-1 basis
tests/golden/boot-pixels-program-view/ # new at P-1 basis
```

**Contract/dump delta:** Only the contract and stable-dump changes explicitly named by this task are permitted.

**Work:**

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

Do not expose arbitrary byte offsets to renderer code. Generated constants map table IDs to placed static fields. Any index beyond compiler-generated capacity is `InternalInvariant(RenderInvariant.ProgramIndex)`.

At renderer initialization, check header magic/version/digest against generated constants once. The guest does not recompute SHA-256 per frame; boot/image integrity already covers bytes. It verifies cheap header/table counts and reserved flags.

**Tests:**

- Program-view boot fixture reads representative records and prints expected stable values.
- No dynamic allocation or pointer arithmetic surface exists in Wrela source.
- Index failure returns explicit internal violation.
- All table loads have exact `@layout(runtime)` offsets checked by compiler layout tests.
- Accessors are pure and nonsuspending.

**Focused checks:** Run the focused tests, fixtures, dumps, and commands named by this task before the repository gate.

**Repository gate:**

```text
cargo xtask verify
```

**Stop conditions:** Stop if an acceptance condition cannot be met without changing a closed architectural decision, weakening a proof/failure boundary, or adding an external Cargo dependency.

**Commit**

```text
pixels P7.1: read sealed frame programs in Wrela
```

## Task P7.2 — implement frame input snapshot and validation

**Requires:** P7.1.

**Produces:** Convert owned `RenderFrame[P]` into the exact finite coefficient state used by workers.

**Files:**

```text
stdlib/core/render.wr # new at P-1 basis
stdlib/core/render_actor.wr # new at P-1 basis
crates/wrela-compiler/src/pixels/glue.rs # new at P-1 basis
tests/golden/boot-pixels-frame-input/ # new at P-1 basis
```

**Contract/dump delta:** Only the contract and stable-dump changes explicitly named by this task are permitted.

**Work:**

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

A frame outside declared `@range` returns `ParameterOutOfRange(path)`; the compiler’s proofs do not apply. Do not clamp.

Snapshot is copied into each worker’s fixed job record. It contains no source struct padding or unused fields.

**Tests:**

- Packed offsets agree with compiler ParamTable dump.
- Field ownership P is returned on every error path.
- NaN/infinity/out-of-range controls return exact error before touching framebuffer.
- From-scratch rendering does not require a previous snapshot.
- Snapshot bytes/digest are deterministic.

**Focused checks:** Run the focused tests, fixtures, dumps, and commands named by this task before the repository gate.

**Repository gate:**

```text
cargo xtask verify
```

**Stop conditions:** Stop if an acceptance condition cannot be met without changing a closed architectural decision, weakening a proof/failure boundary, or adding an external Cargo dependency.

**Commit**

```text
pixels P7.2: validate and pack frame coefficients
```

## Task P7.3 — define worker workspace and reset protocol

**Requires:** P7.2.

**Produces:** Materialize every compile-time capacity as fixed per-worker storage.

**Files:**

```text
stdlib/core/render_sweep.wr # new at P-1 basis
stdlib/core/render_actor.wr # new at P-1 basis
crates/wrela-compiler/src/pixels/glue.rs # new at P-1 basis
```

**Contract/dump delta:** Only the contract and stable-dump changes explicitly named by this task are permitted.

**Work:**

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
    telemetry: CertificateTelemetryCounters
    # fixed counters and scratch only
```

Workspace lives in the worker’s assigned mutable state region. Reset sets counts/generation markers; it does not zero entire arrays unless required for determinism/security. Every accessor checks count before read.

`CertificateTelemetryCounters` contains fixed arrays sized by the versioned
method/shape/expiry/owner/density/subdivision/rebuild enum counts, never by
observed data. Diagnostic/conformance layouts include and report those bytes;
an uninstrumented production layout may omit the record entirely. No
renderer-decision function receives a telemetry reference.

Use generation tags only if wrap is impossible over image lifetime or checked; otherwise reset explicit counts and overwrite live slots.

**Tests:**

- Generated layout bytes equal capacity report.
- No `List`/heap collection appears in sweep modules.
- Every push returns capacity error.
- Reset leaves no previous-frame record reachable through current counts.
- Two worker workspaces never overlap in layout.
- Diagnostic and uninstrumented workspace byte totals are exact, and enabling
  telemetry changes no renderer decision or displayed byte.

**Focused checks:** Run the focused tests, fixtures, dumps, and commands named by this task before the repository gate.

**Repository gate:**

```text
cargo xtask verify
```

**Stop conditions:** Stop if an acceptance condition cannot be met without changing a closed architectural decision, weakening a proof/failure boundary, or adding an external Cargo dependency.

**Commit**

```text
pixels P7.3: allocate fixed renderer workspaces
```

## Task P7.4 — enumerate complete row-start feature candidates

**Requires:** P7.3.

**Produces:** Start each tile row from structural completeness rather than samples or prior hits.

**Files:**

```text
stdlib/core/render_sweep.wr # new at P-1 basis
stdlib/core/render_program.wr # new at P-1 basis
crates/wrela-compiler/src/pixels/reference/sweep.rs # new at P-1 basis
```

**Contract/dump delta:** Only the contract and stable-dump changes explicitly named by this task are permitted.

**Work:**

For tile `(tx,ty)` and local row `y`:

1. fetch tile feature ID slice;
2. reject features whose half-open row span excludes `y`;
3. evaluate runtime coefficient bounds for the row/parameter snapshot;
4. apply compiler-emitted static/dynamic exclusion predicates;
5. retain all remaining feature IDs in ascending order;
6. record exclusion reason/margin counters for diagnostics, not decisions beyond the predicate itself.

No screen sample or q solve is used to decide whether a feature is a candidate. Support shells and projected spans are the completeness mechanism.

For a feature whose coefficient/runtime bound cannot be evaluated due to numeric failure, return `CertificateExhausted` or `InternalInvariant` according to cause; do not omit it.

**Tests:**

- Enclosed/thin feature controls retain their feature at the affected row.
- Candidate enumeration agrees with a slow host overlap/reference filter.
- Candidate order is stable.
- Every omitted tile feature has a passed exclusion predicate with positive margin.
- Counters distinguish static compiler exclusion from runtime row exclusion.

**Focused checks:** Run the focused tests, fixtures, dumps, and commands named by this task before the repository gate.

**Repository gate:**

```text
cargo xtask verify
```

**Stop conditions:** Stop if an acceptance condition cannot be met without changing a closed architectural decision, weakening a proof/failure boundary, or adding an external Cargo dependency.

**Commit**

```text
pixels P7.4: enumerate complete row feature sets
```

## Task P7.5 — isolate every smooth-object root at row start

**Requires:** P7.4.

**Produces:** Build the complete ordered boundary-event list for one x position.

**Files:**

```text
stdlib/core/render_sweep.wr # new at P-1 basis
stdlib/core/render_events.wr # new at P-1 basis
crates/wrela-compiler/src/pixels/reference/sweep.rs # new at P-1 basis
```

**Contract/dump delta:** Only the contract and stable-dump changes explicitly named by this task are permitted.

**Work:**

At initial x = tile’s left pixel-center coordinate:

- construct candidate feature `q` slabs from conservative leaf sublevel domains
  `leaf <= accumulated_support_budget`, union them by smooth object, and
  intersect with positive near/far q;
- for polynomial leaves, use P6.4 to isolate boundaries of
  `leaf - accumulated_support_budget`; leaf zeros alone never seed
  completeness;
- for each resulting domain, evaluate and isolate the object’s `SmoothObjectRootProgram.scalar_root`;
- use an analytic affine/quadratic proposal where available, then verify against the full object scalar;
- use complete bounded root isolation for smooth blends, torus/deformation, and ambiguous cases;
- after finding an object root, validate feature predicates and determine the finite feature/identity set active at that root;
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

**Tests:**

- Plane/sphere/torus/capsule fixtures produce all expected roots.
- `check-pixels-smooth-interior-root` finds `a=b=k/4` without a leaf-root seed.
- A positive leaf with no zero but with a nonempty support-budget sublevel
  still retains the complete smooth-object candidate slab.
- No sign-changing-only assumption misses tangencies.
- Root records are front-to-back (larger q first) when strict order is certified.
- Duplicate shared-feature boundary roots are deduplicated only with proof of same geometric crossing and deterministic owner rule.
- Failure to separate close roots is an event corridor/rebuild, not ID tie-break.

**Focused checks:** Run the focused tests, fixtures, dumps, and commands named by this task before the repository gate.

**Repository gate:**

```text
cargo xtask verify
```

**Stop conditions:** Stop if an acceptance condition cannot be met without changing a closed architectural decision, weakening a proof/failure boundary, or adding an external Cargo dependency.

**Commit**

```text
pixels P7.5: construct complete smooth-object row roots
```

## Task P7.6 — evaluate the hard-CSG occupancy sweep

**Requires:** P7.5.

**Produces:** Choose exact composite boundaries from ordered object crossings.

**Files:**

```text
stdlib/core/render_sweep.wr # new at P-1 basis
stdlib/core/render_program.wr # new at P-1 basis
crates/wrela-compiler/src/pixels/reference/csg.rs # new at P-1 basis
```

**Contract/dump delta:** Only the contract and stable-dump changes explicitly named by this task are permitted.

**Work:**

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

**Tests:**

- Union, intersection, subtraction fixtures select exact expected boundary/identity.
- Camera-inside fixture returns first exit boundary correctly.
- Noninfluential internal object boundaries are skipped without changing output.
- Coincident boundaries take corridor path.
- Wrela output agrees with Rust CSG reference for deterministic root lists.

**Focused checks:** Run the focused tests, fixtures, dumps, and commands named by this task before the repository gate.

**Repository gate:**

```text
cargo xtask verify
```

**Stop conditions:** Stop if an acceptance condition cannot be met without changing a closed architectural decision, weakening a proof/failure boundary, or adding an external Cargo dependency.

**Commit**

```text
pixels P7.6: sweep exact hard-CSG occupancy
```

## Task P7.7 — isolate all x-domain event endpoints for a row

**Requires:** P7.6.

**Produces:** Partition a row into maximal domains where roots, features, identities, and order can be certified unchanged.

**Files:**

```text
stdlib/core/render_events.wr # new at P-1 basis
stdlib/core/render_sweep.wr # new at P-1 basis
crates/wrela-compiler/src/pixels/reference/events.rs # new at P-1 basis
```

**Contract/dump delta:** Only the contract and stable-dump changes explicitly named by this task are permitted.

**Work:**

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

**Tests:**

- Every permanent edge control creates a corridor touching the true event pixel set.
- No regular domain contains a sampled sign change in host exhaustive fixture checks.
- Event endpoints use half-open ownership so adjacent tiles/rows agree.
- Multiple simultaneous event IDs remain attached to one corridor.
- Capacity overflow returns `CapacityExceeded(RenderCapacity.Events)` before writing past storage.

**Focused checks:** Run the focused tests, fixtures, dumps, and commands named by this task before the repository gate.

**Repository gate:**

```text
cargo xtask verify
```

**Stop conditions:** Stop if an acceptance condition cannot be met without changing a closed architectural decision, weakening a proof/failure boundary, or adding an external Cargo dependency.

**Commit**

```text
pixels P7.7: partition rows by complete events
```

## Task P7.8 — construct implicit-jet run candidates

**Requires:** P7.7.

**Produces:** Predict all active root sheets across one regular x-domain with one evaluation per sheet rather than repeated solving.

**Files:**

```text
stdlib/core/render_sweep.wr # new at P-1 basis
stdlib/core/render_program.wr # new at P-1 basis
crates/wrela-compiler/src/pixels/reference/sweep.rs # new at P-1 basis
```

**Contract/dump delta:** Only the contract and stable-dump changes explicitly named by this task are permitted.

**Work:**

At regular-domain left anchor or center:

- take root interval/candidate from current root list;
- evaluate derivative bundle at certified center root;
- require candidate `G_q` away from zero before division;
- compute `q_x`, `q_xx`, and optionally q parameter/time derivatives through implicit formulas;
- construct quadratic x model and conservative initial error interval;
- for a smooth object, evaluate only the active leaf/branch cluster proven by support/branch predicates;
- if candidate jet is nonsmooth/grazing, stop regular run at corridor or invoke local rebuild tier.

Candidate values may be f64 in Rust reference and f32 in guest; verifier intervals enclose them. The candidate itself proves nothing.

**Tests:**

- Plane model is exactly affine and has zero quadratic residual aside from representation radius.
- Sphere regular rows produce expected derivatives.
- Smooth blend uses active local cluster, not full field tape.
- Candidate generation failure does not remove the root.
- Candidate counters and active-leaf count are available for report/runtime diagnostics.

**Focused checks:** Run the focused tests, fixtures, dumps, and commands named by this task before the repository gate.

**Repository gate:**

```text
cargo xtask verify
```

**Stop conditions:** Stop if an acceptance condition cannot be met without changing a closed architectural decision, weakening a proof/failure boundary, or adding an external Cargo dependency.

**Commit**

```text
pixels P7.8: predict regular root sheets with jets
```

## Task P7.9 — certify complete regular runs

**Requires:** P7.8.

**Produces:** Turn candidates into proof-carrying scanline runs.

**Files:**

```text
stdlib/core/render_sweep.wr # new at P-1 basis
crates/wrela-compiler/src/pixels/reference/sweep.rs # new at P-1 basis
crates/wrela-compiler/src/report.rs
formal/pixels/Pixels/RunCertificate.lean # new at P-1 basis
```

**Contract/dump delta:** Add the stable `CertificateTelemetry` report/debug-proof section with versioned enum IDs and histogram bins. It is diagnostic evidence and never a renderer input.

**Work:**

For each regular domain and every active root:

1. certify root existence/uniqueness with Bernstein-face, interval monotone
   tube, or Krawczyk method;
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

Diagnostic builds and conformance runs record, per tile and stable ID:

```text
run length bin
root certificate method and composed degree/term shape
run-ending cause
minimum-margin owner
active feature/sheet/event/predicate counts
leaf-sublevel and active smooth-cluster sizes
root/event subdivision depth
bounded-rebuild terminal reason (zero until P7.10 populates it)
numeric/refinement failure cause
regular/event-corridor pixel counts
```

Workers own local counters and merge them in tile-ID order. Production may
omit component-detail storage, but conformance telemetry must be deterministic.
No runtime proof, scheduler, or quality decision may read these counters.

**Tests:**

- Every accepted run passes host dense/oracle scoring, but oracle is never passed to renderer.
- Plane fixture can emit one run per row apart from tile/microtile boundaries.
- No run crosses a compiled event corridor.
- Root/feature/order/CSG completeness is explicit in debug proof dump.
- One/four-worker telemetry is identical and every run/pixel is charged to
  exactly one stable bin.
- Permanent low-event, grazing, close-depth, smooth-band, and mixed-scale
  controls pin method/expiry ownership without imposing a cycle threshold.
- Lean run theorem applies directly to record invariants.

**Focused checks:** Run the focused tests, fixtures, dumps, and commands named by this task before the repository gate.

**Repository gate:**

```text
cargo xtask verify
cargo xtask pixels-formal
cargo xtask verify
```

**Stop conditions:** Stop if an acceptance condition cannot be met without changing a closed architectural decision, weakening a proof/failure boundary, or adding an external Cargo dependency.

**Commit**

```text
pixels P7.9: emit complete certified visibility runs
```

## Task P7.10 — implement the bounded local rebuild ladder

**Requires:** P7.9.

**Produces:** Resolve difficult regular/event domains without an unbounded or hidden dense fallback.

**Files:**

```text
stdlib/core/render_sweep.wr # new at P-1 basis
stdlib/core/render_events.wr # new at P-1 basis
crates/wrela-compiler/src/pixels/reference/rebuild.rs # new at P-1 basis
```

**Contract/dump delta:** Only the contract and stable-dump changes explicitly named by this task are permitted.

**Work:**

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

Charge every ladder entry, split depth, and terminal reason to the P7.9
`CertificateTelemetry` schema using stable IDs. Successful resolution and
explicit exhaustion are separate terminal classes. The counters remain
diagnostic and cannot reorder the fixed ladder.

**Tests:**

- Close-depth, tangency, silhouette, repeat-boundary, smooth-tie, and material-edge controls resolve or fail explicitly.
- No path exceeds generated arrays/depth.
- Pixel-cell path still certifies complete roots over its required point/filter domain.
- A deliberately pathological accepted scene returns `CertificateExhausted` and leaves prior frame displayed.
- Rebuild choices are fixed, not heuristic thresholds tuned at runtime.
- Every entered rebuild cell has exactly one terminal telemetry class and
  one/four-worker merged counts agree.

**Focused checks:** Run the focused tests, fixtures, dumps, and commands named by this task before the repository gate.

**Repository gate:**

```text
cargo xtask verify
```

**Stop conditions:** Stop if an acceptance condition cannot be met without changing a closed architectural decision, weakening a proof/failure boundary, or adding an external Cargo dependency.

**Commit**

```text
pixels P7.10: resolve bounded visibility corridors
```

## Task P7.11 — carry runs across adjacent rows as proposals only

**Requires:** P7.10.

**Produces:** Reduce candidate work without making row coherence a correctness dependency.

**Files:**

```text
stdlib/core/render_sweep.wr # new at P-1 basis
crates/wrela-compiler/src/pixels/reference/sweep.rs # new at P-1 basis
```

**Contract/dump delta:** Only the contract and stable-dump changes explicitly named by this task are permitted.

**Work:**

After completing row y, use its sheet/event/run states as candidate seeds for row y+1 within the same tile. Before use:

- intersect with y+1 projected feature set;
- transport q/events using compiled v derivatives/remainders;
- validate every candidate through the ordinary row-start/root/event certificates;
- enumerate any y+1 feature absent from proposal from the complete tile index;
- discard proposal wholesale on camera/input discontinuity.

A configuration switch used only in tests forces `RowProposal.Disabled`. Displayed output and success/failure must remain identical.

**Tests:**

- Disabled/enabled produce identical frame bytes and error outcomes.
- Enclosed feature absent from prior row is still discovered from structural index.
- Proposal cannot suppress a feature/event.
- Counters distinguish proposed/revalidated/new records.
- No previous tile/frame data is required.

**Focused checks:** Run the focused tests, fixtures, dumps, and commands named by this task before the repository gate.

**Repository gate:**

```text
cargo xtask verify
```

**Stop conditions:** Stop if an acceptance condition cannot be met without changing a closed architectural decision, weakening a proof/failure boundary, or adding an external Cargo dependency.

**Commit**

```text
pixels P7.11: reuse row structure as validated proposals
```

## Task P7.12 — implement tile sweep orchestration

**Requires:** P7.11.

**Produces:** Construct all rows/runs in one owned tile with deterministic ordering.

**Files:**

```text
stdlib/core/render_sweep.wr # new at P-1 basis
stdlib/core/render_actor.wr # new at P-1 basis
crates/wrela-compiler/src/pixels/reference/frame.rs # new at P-1 basis
```

**Contract/dump delta:** Only the contract and stable-dump changes explicitly named by this task are permitted.

**Work:**

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

**Tests:**

- Every tile row has exact domain partition.
- Tile order and row order deterministic.
- No unresolved record reaches debug raster; it becomes frame error.
- Plane/hard-CSG/smooth/repeat/deform permanent fixtures construct complete visibility tiles.
- Debug identity/q output matches host oracle fixture expectations.

**Focused checks:** Run the focused tests, fixtures, dumps, and commands named by this task before the repository gate.

**Repository gate:**

```text
cargo xtask verify
```

**Stop conditions:** Stop if an acceptance condition cannot be met without changing a closed architectural decision, weakening a proof/failure boundary, or adding an external Cargo dependency.

**Commit**

```text
pixels P7.12: construct complete visibility tiles
```

## Task P7.13 — implement coordinator/worker frame execution

**Requires:** P7.12.

**Produces:** Run the from-scratch sweep across all sealed render cores with deterministic ownership.

**Files:**

```text
stdlib/core/render_actor.wr # new at P-1 basis
stdlib/core/render.wr # new at P-1 basis
crates/wrela-compiler/src/pixels/glue.rs # new at P-1 basis
tests/golden/boot-pixels-plane/ # new at P-1 basis
```

**Contract/dump delta:** Only the contract and stable-dump changes explicitly named by this task are permitted.

**Work:**

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

**Tests:**

- Single-core and four-core builds produce identical debug frame digest.
- Worker completion order perturbation does not alter output.
- One worker failure prevents global success/presentation.
- No worker writes another worker’s tiles/workspace.
- Actor/ring/mailbox capacities remain proven by existing compiler.

**Focused checks:** Run the focused tests, fixtures, dumps, and commands named by this task before the repository gate.

**Repository gate:**

```text
cargo xtask verify
```

**Stop conditions:** Stop if an acceptance condition cannot be met without changing a closed architectural decision, weakening a proof/failure boundary, or adding an external Cargo dependency.

**Commit**

```text
pixels P7.13: execute complete sweeps across render workers
```

## Task P7.14 — add independent host visibility oracle and score-only gate

**Requires:** P7.13.

**Produces:** Validate implementation correctness without letting oracle data influence runtime decisions.

**Files:**

```text
crates/wrela-compiler/src/pixels/reference/oracle.rs # new at P-1 basis
crates/xtask/src/pixels_conformance.rs # new at P-1 basis
crates/xtask/src/main.rs
tests/pixels_truth/ # new at P-1 basis
```

**Contract/dump delta:** Only the contract and stable-dump changes explicitly named by this task are permitted.

**Work:**

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

**Tests:**

- Structural-discovery control compares empty and enclosed-feature programs and requires their sweep outputs to differ from structural data. It makes no uncomputed claim about a legacy sample lattice.
- Oracle unresolved is a conformance failure for flagship fixtures.
- Accepted run/corridor failures are zero.
- Conformance command is deterministic and score-only.
- Runtime source has no import/dependency on oracle module.

**Focused checks:** Run the focused tests, fixtures, dumps, and commands named by this task before the repository gate.

**Repository gate:**

```text
cargo xtask verify
cargo xtask pixels-conformance
cargo xtask verify
```

**Stop conditions:** Stop if an acceptance condition cannot be met without changing a closed architectural decision, weakening a proof/failure boundary, or adding an external Cargo dependency.

**Commit**

```text
pixels P7.14: gate sweep visibility against an independent oracle
```

## Task P7.15 — replace placeholder render failure with debug-frame success

**Requires:** P7.14.

**Produces:** Complete the production from-scratch visibility path before final shading/display.

**Files:**

```text
stdlib/core/render_actor.wr # new at P-1 basis
stdlib/core/render_sweep.wr # new at P-1 basis
tests/golden/boot-pixels-*/ # new at P-1 basis
```

**Contract/dump delta:** Only the contract and stable-dump changes explicitly named by this task are permitted.

**Work:**

Return `RenderedFrame[P]` with a deterministic debug visibility image:

- RGB encodes stable object/material ID;
- alpha/auxiliary digest encodes q interval class;
- event pixels use coverage between adjacent identities;
- background fixed code.

This debug mode is compiler-internal and not a source profile. It is removed in P9 after full shading, but its goldens remain host conformance fixtures.

**Tests:**

- All opaque permanent visibility fixtures render successfully from scratch.
- The plane-only P-1 skeleton is replaced by the complete sweep; any remaining valid-frame failure uses the §2.6 error contract.
- Every frame pixel is written exactly once.
- Conformance has zero visibility/identity/root failures.
- Kinetic state is not read.

**Focused checks:** Run the focused tests, fixtures, dumps, and commands named by this task before the repository gate.

**Repository gate:**

```text
cargo xtask verify
cargo xtask pixels-conformance
cargo xtask verify
```

**Stop conditions:** Stop if an acceptance condition cannot be met without changing a closed architectural decision, weakening a proof/failure boundary, or adding an external Cargo dependency.

**Commit**

```text
pixels P7.15: complete from-scratch certified visibility
```

### Milestone P7 close

Run `cargo xtask verify`. This is the architectural correctness gate: a valid frame is constructed from scratch with no dense truth, no legacy sample lattice, no previous frame, and no guessed pixels. Later milestones may lower cost and add quality, but cannot weaken this path.

---

# Milestone P8 — fixed-q rasterization, analytic event coverage, tile buffers, and display presentation

Milestone result: certified visibility runs become complete scanout-resolution tiles using fixed-q packet recurrence and analytic edge coverage. The display driver/VMM presents little-endian `Bgra8Srgb` tile lists and replay records exact frame digests. Shading is still the deterministic debug identity material until P9.

## Task P8.1 — fix the scanout pixel and tile contract

**Requires:** the preceding milestone close gate.

**Produces:** Remove every ambiguity between HDR proof values, stored bytes, guest memory, and host presentation.

**Files:**

```text
docs/language/06-machine.md
docs/language/07-pixels.md # new at P-1 basis
crates/wrela-machine/src/pixels.rs # new at P-1 basis
stdlib/drivers/display.wr # new at P-1 basis
```

**Contract/dump delta:** Only the contract and stable-dump changes explicitly named by this task are permitted.

**Work:**

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

**Tests:**

- Machine, compiler, driver, VMM, and goldens use one format constant.
- Alpha byte is always 255, including background.
- Partial-tile padding is deterministic zero and excluded from visible image comparison but included in raw tile digest if report says so; define both digests distinctly.
- Tile count/bytes derive exactly for arbitrary positive mode dimensions within ceiling.
- Endianness fixture proves in-memory bytes.

**Focused checks:** Run the focused tests, fixtures, dumps, and commands named by this task before the repository gate.

**Repository gate:**

```text
cargo xtask verify
```

**Stop conditions:** Stop if an acceptance condition cannot be met without changing a closed architectural decision, weakening a proof/failure boundary, or adding an external Cargo dependency.

**Commit**

```text
pixels P8.1: seal the BGRA scanout tile contract
```

## Task P8.2 — define final run raster records

**Requires:** P8.1.

**Produces:** Separate proof-rich sweep state from compact hot-loop setup.

**Files:**

```text
stdlib/core/render_raster.wr # new at P-1 basis
stdlib/core/render_sweep.wr # new at P-1 basis
crates/wrela-compiler/src/pixels/reference/raster.rs # new at P-1 basis
```

**Contract/dump delta:** Only the contract and stable-dump changes explicitly named by this task are permitted.

**Work:**

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

**Tests:**

- Conversion preserves half-open run domain exactly.
- Every run fits fixed-q setup or is split before conversion.
- Proof code identifies which stages are already singleton/constant.
- Run/event arrays fit generated capacities.
- Rust reference validates no row gap/overlap after conversion.

**Focused checks:** Run the focused tests, fixtures, dumps, and commands named by this task before the repository gate.

**Repository gate:**

```text
cargo xtask verify
```

**Stop conditions:** Stop if an acceptance condition cannot be met without changing a closed architectural decision, weakening a proof/failure boundary, or adding an external Cargo dependency.

**Commit**

```text
pixels P8.2: lower certified runs to raster records
```

## Task P8.3 — implement scalar fixed-q raster

**Requires:** P8.2.

**Produces:** Produce exact debug depth/identity pixels before packetization.

**Files:**

```text
stdlib/core/render_raster.wr # new at P-1 basis
crates/wrela-compiler/src/pixels/reference/raster.rs # new at P-1 basis
stdlib/tests/pixels_raster.wr # new at P-1 basis
```

**Contract/dump delta:** Only the contract and stable-dump changes explicitly named by this task are permitted.

**Work:**

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

**Tests:**

- Output matches Rust scalar reference byte-for-byte.
- Every pixel write address stays within visible/full tile extent.
- Partial tile/padding rules hold.
- Recurrence error remains within certificate at every checked sample.
- Scalar path is retained permanently as packet differential oracle.

**Focused checks:** Run the focused tests, fixtures, dumps, and commands named by this task before the repository gate.

**Repository gate:**

```text
cargo xtask verify
```

**Stop conditions:** Stop if an acceptance condition cannot be met without changing a closed architectural decision, weakening a proof/failure boundary, or adding an external Cargo dependency.

**Commit**

```text
pixels P8.3: rasterize certified runs scalarly
```

## Task P8.4 — implement `i32x4` fixed-q packet raster

**Requires:** P8.3.

**Produces:** Turn the hot visibility raster into vector integer additions/stores.

**Files:**

```text
stdlib/core/render_raster.wr # new at P-1 basis
crates/wrela-compiler/src/mwir.rs
crates/wrela-compiler/src/codegen.rs
crates/wrela-compiler/src/encode.rs
stdlib/tests/pixels_raster.wr # new at P-1 basis
```

**Contract/dump delta:** Only the contract and stable-dump changes explicitly named by this task are permitted.

**Work:**

Implement `raster_run4` processing four consecutive pixels. Use existing/implemented `i32x4` operations:

- vector add;
- vector compare/mask where needed;
- lane narrowing/packing only through named methods;
- aligned/unaligned stores defined by stdlib vector contract.

Handle run prefix/suffix of 1–3 pixels with scalar oracle. Main body uses packet recurrence. Keep q/dq/ddq in vector locals across loop iterations so register allocator can retain them.

Add missing backend vector operations one at a time with `CostRule` tags and emitted-word tests. Do not add explicit vector syntax beyond existing types.

**Tests:**

- Packet output equals scalar output for all vector fixtures and complete debug frames.
- Generated MachineWir has one vector loop and bounded scalar edges.
- No hidden stack slot traffic is assumed away; emitted assembly/report records it.
- All vector operations have exact lane semantics and first-fault behavior where checked arithmetic applies; fixed-q uses ranges proving ordinary add cannot fault.
- AArch64 code uses 128-bit ASIMD operations for main loop.

**Focused checks:** Run the focused tests, fixtures, dumps, and commands named by this task before the repository gate.

**Repository gate:**

```text
cargo xtask verify
cargo xtask diff-eval
```

**Stop conditions:** Stop if an acceptance condition cannot be met without changing a closed architectural decision, weakening a proof/failure boundary, or adding an external Cargo dependency.

**Commit**

```text
pixels P8.4: vectorize fixed-q run rasterization
```

## Task P8.5 — reconstruct normals and optional world position per packet

**Requires:** P8.4.

**Produces:** Supply stable geometric inputs to shading without field gradients on regular runs.

**Files:**

```text
stdlib/core/render_raster.wr # new at P-1 basis
stdlib/core/render_material.wr # new at P-1 basis
crates/wrela-compiler/src/pixels/reference/normal.rs # new at P-1 basis
```

**Contract/dump delta:** Only the contract and stable-dump changes explicitly named by this task are permitted.

**Work:**

For each packet:

- evaluate/advance q, q_u, q_v;
- construct camera-space unnormalized normal `(q_u, q_v, q-u*q_u-v*q_v)`;
- transform by camera basis to world normal;
- normalize using explicit `rsqrt` Newton sequence defined by stdlib numeric contract;
- use normal cone certificate to skip normalization only when a shading summary does not need exact direction;
- compute world position only when the material/light summary declares it necessary, using one reciprocal per lane and raw projective ray.

Generated material dependency flags decide whether world position, view direction, tangent frame, or only normal/material identity is needed.

**Tests:**

- Plane normals are exact/constant after normalization contract.
- Curved normals lie inside certified cone and match host reference.
- Position computation is absent from debug/simple material code when unused.
- `rsqrt` sequence is bit-identical dev/release and has differential tests.
- No field-gradient call occurs in regular runs.

**Focused checks:** Run the focused tests, fixtures, dumps, and commands named by this task before the repository gate.

**Repository gate:**

```text
cargo xtask verify
cargo xtask diff-eval
```

**Stop conditions:** Stop if an acceptance condition cannot be met without changing a closed architectural decision, weakening a proof/failure boundary, or adding an external Cargo dependency.

**Commit**

```text
pixels P8.5: reconstruct run geometry in packets
```

## Task P8.6 — rasterize analytic event coverage

**Requires:** P8.5.

**Produces:** Write silhouettes, CSG ties, material edges, and depth swaps without missed or double-written pixels.

**Files:**

```text
stdlib/core/render_coverage.wr # new at P-1 basis
stdlib/core/render_raster.wr # new at P-1 basis
crates/wrela-compiler/src/pixels/reference/coverage.rs # new at P-1 basis
```

**Contract/dump delta:** Only the contract and stable-dump changes explicitly named by this task are permitted.

**Work:**

For each `EventPixel`:

1. evaluate conservative coverage interval from event curve model;
2. obtain front/back debug colors or later shading intervals at the pixel;
3. premultiply/blend using exact interval arithmetic;
4. if output channel interval maps to one byte, write it;
5. otherwise invoke fixed event-coverage refinement: curve subdivision, side shading refinement, then exact pixel-domain interval integration;
6. if still not singleton under `AaaByteExact`, return `CertificateExhausted`.

For a true geometry coverage edge, both side runs may have different depth/identity. For a material-only edge, geometry/normal can be shared. For a depth swap, divide pixel coverage by swap curve and use each side’s winner.

**Tests:**

- Thin/enclosed features survive subpixel coverage.
- High-contrast diagonal silhouette has stable exact bytes against host interval oracle.
- Tile boundary event ownership is exact.
- No MSAA/TAA sample pattern is used.
- Event pixels are written exactly once after regular runs skip their corridor-owned pixels.

**Focused checks:** Run the focused tests, fixtures, dumps, and commands named by this task before the repository gate.

**Repository gate:**

```text
cargo xtask verify
cargo xtask pixels-conformance
```

**Stop conditions:** Stop if an acceptance condition cannot be met without changing a closed architectural decision, weakening a proof/failure boundary, or adding an external Cargo dependency.

**Commit**

```text
pixels P8.6: rasterize certified event coverage
```

## Task P8.7 — implement tile buffer ownership and deterministic clearing

**Requires:** P8.6.

**Produces:** Move completed scanout tiles safely between workers, coordinator, and display driver.

**Files:**

```text
stdlib/core/render_actor.wr # new at P-1 basis
stdlib/core/render_raster.wr # new at P-1 basis
stdlib/drivers/display.wr # new at P-1 basis
crates/wrela-compiler/src/pixels/glue.rs # new at P-1 basis
```

**Contract/dump delta:** Only the contract and stable-dump changes explicitly named by this task are permitted.

**Work:**

Generate a nominal pool binding for renderer tile buffers. Each worker receives/owns a fixed subset or one reusable tile slot plus output ownership protocol as determined by exact frame scheduling.

Double buffering uses two complete tile-list generations:

- front generation owned by display/scanout until release;
- back generation distributed to workers;
- coordinator cannot reuse front tiles before display completion;
- failure retains front generation and reclaims back generation deterministically.

On image boot, zero all tile bytes once. On subsequent frames, every visible pixel is overwritten; padding remains zero. A debug assertion/test tracks per-visible-pixel write generation in host/reference only, not production guest memory.

**Tests:**

- Ownership checker proves no concurrent writes/display reads.
- Exact tile-buffer pool bytes match layout report.
- Failure/cancellation returns every back tile to its pool.
- Front buffer persists across failed frame.
- No full-frame clear occurs per frame.

**Focused checks:** Run the focused tests, fixtures, dumps, and commands named by this task before the repository gate.

**Repository gate:**

```text
cargo xtask verify
```

**Stop conditions:** Stop if an acceptance condition cannot be met without changing a closed architectural decision, weakening a proof/failure boundary, or adding an external Cargo dependency.

**Commit**

```text
pixels P8.7: own double-buffered scanout tiles
```

## Task P8.8 — complete machine-v1 display device and presentation backends

**Requires:** P8.7.

**Produces:** Extend the P-1.3 shared ABI/headless sink into production host backends without moving renderer behavior host-side. This outlier task explicitly permits five commits, P8.8a–P8.8e; each is a separate review unit and passes `cargo xtask verify`.

**Files:**

```text
stdlib/drivers/display.wr # new at P-1 basis
crates/wrela-machine/src/lib.rs
crates/wrela-machine/src/pixels.rs # new at P-1 basis
crates/wrela-vmm/src/devices.rs
crates/wrela-vmm/src/display/mod.rs        # new at P-1 basis
crates/wrela-vmm/src/display/headless.rs   # new at P-1 basis
crates/wrela-vmm/src/display/hvf.rs        # new at P-1 basis
crates/wrela-vmm/src/display/kvm.rs        # new at P-1 basis
crates/wrela-vmm/src/replay.rs             # new at P-1 basis
```

**Contract/dump delta:** Only the contract and stable-dump changes explicitly named by this task are permitted.

**Work:**

**P8.8a — shared device model.** Complete portable validation and state transitions for the contract fixed in P-1.3: one descriptor chain names frame sequence, mode, format, and tile-list address/count; descriptors name guest-owned pages; publish uses release ordering; one doorbell represents one frame; completion releases the prior front generation; malformed input is a device error and never an out-of-bounds host read.

**P8.8b — guest driver.** Complete publish/release ordering, frame-vector handling, cancellation, and ownership recovery in `stdlib/drivers/display.wr`.

**P8.8c — headless/replay backend.** Make the digest sink production-complete, including malformed-input recording and byte-identical replay.

**P8.8d — macOS/HVF backend.** Create a `BGRA8Unorm_sRGB` Metal texture/layer path or equivalent platform surface, gather tile rows without changing bytes/color, and present on requested vsync. Backend execution belongs to the signed macOS lane. A local `HV_DENIED`/missing-entitlement result is an environment skip with recorded reason only during focused development; it is a hard failure in `verify-deep` on the configured signed runner.

**P8.8e — Linux/KVM backend.** Gather into a DRM dumb buffer/Mesa present path preserving byte format. There is no GPU shading or geometry.

Host APIs already allowed by the VMM crate may be used; no renderer logic moves host-side.

**Tests:**

- Portable device-model tests validate all descriptors/ranges/format/mode.
- HVF, KVM, and headless backends produce the same visible-frame digest before presentation.
- One frame submission causes one recorded output event/doorbell.
- Host never reads outside declared tile visible/full bounds.
- Display error reaches coordinator as `Display(DisplayError)` with tile ownership recovered.

**Focused checks:** Run the focused tests, fixtures, dumps, and commands named by this task before the repository gate.

**Repository gate:**

```text
# after each P8.8a–P8.8e review unit
cargo xtask verify
```

**Stop conditions:** Stop if an acceptance condition cannot be met without changing a closed architectural decision, weakening a proof/failure boundary, or adding an external Cargo dependency.

**Commits**

```text
pixels P8.8a: complete the portable display device model
pixels P8.8b: complete the guest display driver
pixels P8.8c: complete headless display replay
pixels P8.8d: present BGRA tiles through macOS HVF
pixels P8.8e: present BGRA tiles through Linux KVM
```

## Task P8.9 — integrate presentation in renderer coordinator

**Requires:** P8.8.

**Produces:** Make a successful render atomically become the next displayed frame.

**Files:**

```text
stdlib/core/render_actor.wr # new at P-1 basis
stdlib/drivers/display.wr # new at P-1 basis
tests/golden/boot-pixels-plane/ # new at P-1 basis
```

**Contract/dump delta:** Only the contract and stable-dump changes explicitly named by this task are permitted.

**Work:**

After all worker tiles succeed:

1. assemble descriptor list ascending tile ID;
2. compute visible frame digest and raw tile digest through generated bounded digest routine;
3. call display driver handoff/present method;
4. await/observe completion according to driver contract;
5. swap front/back generation only on success;
6. return `RenderedFrame[P]` with frame sequence and ownership of P.

Do not let a late present failure mark back buffer as front. Deadline behavior follows existing actor call/deadline rules.

**Tests:**

- First successful debug frame appears in VMM boot golden.
- Failed frame leaves prior digest/front buffer.
- Frame sequence increments only on successful present.
- Tile descriptor order is deterministic independent of worker completion.
- Single-core/four-core visible digest identical.

**Focused checks:** Run the focused tests, fixtures, dumps, and commands named by this task before the repository gate.

**Repository gate:**

```text
cargo xtask verify
```

**Stop conditions:** Stop if an acceptance condition cannot be met without changing a closed architectural decision, weakening a proof/failure boundary, or adding an external Cargo dependency.

**Commit**

```text
pixels P8.9: atomically present completed renderer frames
```

## Task P8.10 — add frame digest and replay conformance

**Requires:** P8.9.

**Produces:** Make replay reproduce exactly what was displayed.

**Files:**

```text
crates/wrela-vmm/src/replay.rs # new at P-1 basis
crates/wrela-machine/src/report.rs
crates/xtask/src/pixels_conformance.rs # new at P-1 basis
tests/golden/boot-pixels-*/ # new at P-1 basis
```

**Contract/dump delta:** Only the contract and stable-dump changes explicitly named by this task are permitted.

**Work:**

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

**Tests:**

- Record then replay permanent debug fixtures with zero divergence.
- Changing one pixel or padding byte identifies visible/raw class correctly.
- Failed/unpresented frame is not logged as output.
- Replay ordering composes with existing cross-core admission/checkpoint log.
- Report names format/replay contract revision.

**Focused checks:** Run the focused tests, fixtures, dumps, and commands named by this task before the repository gate.

**Repository gate:**

```text
cargo xtask verify
```

**Stop conditions:** Stop if an acceptance condition cannot be met without changing a closed architectural decision, weakening a proof/failure boundary, or adding an external Cargo dependency.

**Commit**

```text
pixels P8.10: record exact displayed frame digests
```

## Task P8.11 — complete debug visibility conformance

**Requires:** P8.10.

**Produces:** Lock visibility/raster/display correctness before AAA shading can obscure it.

**Files:**

```text
crates/xtask/src/pixels_conformance.rs # new at P-1 basis
tests/golden/boot-pixels-*/ # new at P-1 basis
tests/pixels_truth/ # new at P-1 basis
```

**Contract/dump delta:** Only the contract and stable-dump changes explicitly named by this task are permitted.

**Work:**

Run all opaque visibility fixtures through full guest/VMM presentation. Compare visible debug output to host oracle and assert:

- zero hit/miss disagreement;
- zero first-identity disagreement;
- every reconstructed q inside certified interval;
- every normal inside certified cone;
- zero missed event coverage;
- exact expected debug bytes/digest;
- zero unresolved frame.

Preserve separate controls for plane, hard CSG, smooth CSG, repeat, displacement, close depth, thin feature, enclosed feature, material edge.

Grade the permanent visibility corpus by event/predicate density and include
deterministic adversarial distributions for:

- grazing silhouettes and near-tangencies;
- overlapping smooth-band/support sublevels;
- close adjacent q sheets and depth-swap corridors;
- mixed near/far scale that stresses fixed-domain exponent selection;
- repeated clutter with sparse and dense projected overlap.

For each class, archive the P7.9 `CertificateTelemetry` section alongside the
visibility result. P8 gates schema completeness, deterministic accounting,
zero unresolved acceptance frames, and explicit coverage of every class. Run
lengths and expiry-owner distributions remain informational until P12.

**Tests:**

- All assertions zero.
- Conformance does not alter render inputs/decisions.
- Legacy fieldprobe result remains untouched/historical.
- Debug visibility path remains runnable after P9 for regression diagnosis.
- Kinetic mode is forced disabled.
- One/four-core telemetry and density-class IDs are identical.
- Every adversarial class has nonzero intended event/predicate activity and no
  unclassified run ending or refinement cause.

**Focused checks:** Run the focused tests, fixtures, dumps, and commands named by this task before the repository gate.

**Repository gate:**

```text
cargo xtask pixels-conformance
cargo xtask verify
```

**Stop conditions:** Stop if an acceptance condition cannot be met without changing a closed architectural decision, weakening a proof/failure boundary, or adding an external Cargo dependency.

**Commit**

```text
pixels P8.11: lock full visibility and scanout conformance
```

### Milestone P8 close

Run `cargo xtask verify`. The renderer now attempts every contract-valid frame from scratch with certified geometry and analytic coverage, presents only on complete success, and returns the §2.6 error on bounded exhaustion. The locked P8 conformance workload must present with zero errors, and its versioned certificate-telemetry schema/adversarial density classes must be complete and deterministic. Telemetry is not yet a performance admission gate. Do not add temporal reuse before full AAA shading/output correctness is complete.

---

# Milestone P9 — AAA material, texture, lighting, shadow, filtering, and output-byte verification

Milestone result: debug identity color is replaced by deterministic physically based shading and complete output-byte certification. Every supported frame produces final `Bgra8Srgb` bytes with explicit coverage/shading/shadow/filter/post error budgets. No stochastic sampling or denoising exists.

## Task P9.1 — fix the working color and filmic-output contract

**Requires:** the preceding milestone close gate.

**Produces:** Give every material/light/post operation one exact color interpretation.

**Files:**

```text
docs/language/07-pixels.md # new at P-1 basis
stdlib/core/render.wr # new at P-1 basis
stdlib/core/render_transfer.wr # new at P-1 basis
stdlib/data/pixels/filmic_v1_u16.bin # new at P-1 basis
stdlib/data/pixels/srgb_v1_u16.bin # new at P-1 basis
crates/wrela-compiler/src/pixels/tables.rs # new at P-1 basis
crates/wrela-compiler/src/pixels/program.rs # new at P-1 basis
```

**Contract/dump delta:** Only the contract and stable-dump changes explicitly named by this task are permitted.

**Work:**

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

The runtime does not evaluate the rational curve. The repository stores a canonical 4097-entry u16 LUT over log2 input domain `[-16,+16]`, with piecewise-linear interpolation. Radiance exactly zero maps directly to LUT output zero without taking `log2`; positive values below/above the domain clamp to the first/last LUT input. `srgb_v1_u16.bin` is a canonical 4097-entry u16 LUT over `[0,1]` for the standard sRGB OETF. The checked-in bytes, dimensions, domains, and SHA-256 are the numeric contract; regeneration is maintainer-only.

Add `tools/gen_pixels_tables.rs` as a standalone Rust source compiled/run only deliberately. It may use f64 formula evaluation to propose tables, but regeneration writes a candidate file and refuses to overwrite canonical bytes without `--accept`. Verification checks digest and monotonicity, not host regeneration equivalence.

Embed LUTs into `FrameProgram` or shared immutable rodata by digest/reference; do not duplicate per renderer.

**Tests:**

- Compiler verifies exact byte length, endpoints, monotonicity, and digest.
- Runtime interpolation is integer/fixed-point and deterministic.
- Formal byte theorem relies on the verified monotone table, not an unproved analytic formula.
- Color/channel order is tested end-to-end through VMM.
- Numeric-contract revision changes if table bytes/domain/interpolation changes.

**Focused checks:** Run the focused tests, fixtures, dumps, and commands named by this task before the repository gate.

**Repository gate:**

```text
cargo xtask verify
cargo xtask pixels-formal
```

**Stop conditions:** Stop if an acceptance condition cannot be met without changing a closed architectural decision, weakening a proof/failure boundary, or adding an external Cargo dependency.

**Commit**

```text
pixels P9.1: seal the working color and filmic tables
```

## Task P9.2 — define the v1 physically based material model

**Requires:** P9.1.

**Produces:** Provide a closed, high-quality BRDF that can be bounded and packetized.

**Files:**

```text
stdlib/core/render.wr # new at P-1 basis
stdlib/core/render_material.wr # new at P-1 basis
crates/wrela-compiler/src/pixels/material.rs # new at P-1 basis
crates/wrela-compiler/src/pixels/material_graph.rs # new at P-1 basis
```

**Contract/dump delta:** Only the contract and stable-dump changes explicitly named by this task are permitted.

**Work:**

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

**Tests:**

- Material constructors enforce ranges.
- Compiler emits a closed material feature flag set.
- Scalar Rust/Wrela BRDF agree on permanent vectors.
- White furnace host test verifies bounded energy for the supported parameter grid within a documented numeric radius.
- No unsupported lobe silently maps to standard.

**Focused checks:** Run the focused tests, fixtures, dumps, and commands named by this task before the repository gate.

**Repository gate:**

```text
cargo xtask verify
```

**Stop conditions:** Stop if an acceptance condition cannot be met without changing a closed architectural decision, weakening a proof/failure boundary, or adding an external Cargo dependency.

**Commit**

```text
pixels P9.2: fix the standard diffuse GGX material
```

## Task P9.3 — implement deterministic texture assets and sampling

**Requires:** P9.2.

**Produces:** Support production surface detail without unbounded procedural evaluation or aliasing.

**Files:**

```text
stdlib/core/render.wr # new at P-1 basis
stdlib/core/render_material.wr # new at P-1 basis
crates/wrela-compiler/src/pixels/texture.rs # new at P-1 basis
crates/wrela-compiler/src/pixels/program.rs # new at P-1 basis
crates/wrela-compiler/src/pixels/encode.rs # new at P-1 basis
```

**Contract/dump delta:** Only the contract and stable-dump changes explicitly named by this task are permitted.

**Work:**

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

**Tests:**

- Mip generation is byte-deterministic and independently decodable.
- Min/max pyramid encloses all footprint samples.
- Texture asset bytes contribute to build identity/memory report.
- Seam/wrap events are represented or filtered continuously.
- Host high-resolution texture oracle lies inside runtime sample interval.

**Focused checks:** Run the focused tests, fixtures, dumps, and commands named by this task before the repository gate.

**Repository gate:**

```text
cargo xtask verify
cargo xtask repro
```

**Stop conditions:** Stop if an acceptance condition cannot be met without changing a closed architectural decision, weakening a proof/failure boundary, or adding an external Cargo dependency.

**Commit**

```text
pixels P9.3: compile deterministic filtered textures
```

## Task P9.4 — compile material dependency and summary programs

**Requires:** P9.3.

**Produces:** Evaluate smooth interior shading once per run/subrun when a verified summary suffices.

**Files:**

```text
crates/wrela-compiler/src/pixels/material.rs # new at P-1 basis
crates/wrela-compiler/src/pixels/program.rs # new at P-1 basis
stdlib/core/render_material.wr # new at P-1 basis
formal/pixels/Pixels/MaterialBound.lean # new at P-1 basis
```

**Contract/dump delta:** Only the contract and stable-dump changes explicitly named by this task are permitted.

**Work:**

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

**Tests:**

- Constant clay/porcelain use constant summaries where geometry/light permit.
- Procedural/texture summaries either verify or fall to exact per-pixel material evaluation.
- Rank is never assumed from scene class.
- Summary plus residual contains host per-pixel material results.
- Compiler/runtime counts and capacities include anchors/basis storage.

**Focused checks:** Run the focused tests, fixtures, dumps, and commands named by this task before the repository gate.

**Repository gate:**

```text
cargo xtask verify
cargo xtask pixels-formal
```

**Stop conditions:** Stop if an acceptance condition cannot be met without changing a closed architectural decision, weakening a proof/failure boundary, or adding an external Cargo dependency.

**Commit**

```text
pixels P9.4: compile verified material summaries
```

## Task P9.5 — implement normal-detail moment filtering

**Requires:** P9.4.

**Produces:** Remove specular shimmer from subpixel normal/slope detail deterministically.

**Files:**

```text
stdlib/core/render_material.wr # new at P-1 basis
crates/wrela-compiler/src/pixels/texture.rs # new at P-1 basis
crates/wrela-compiler/src/pixels/reference/moments.rs # new at P-1 basis
formal/pixels/Pixels/MaterialBound.lean # new at P-1 basis
```

**Contract/dump delta:** Only the contract and stable-dump changes explicitly named by this task are permitted.

**Work:**

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

**Tests:**

- Distant high-frequency normal fixture has stable frame bytes under subpixel camera motion.
- Flat/constant detail reduces exactly to original material.
- Moment-filtered BRDF interval contains dense high-resolution host integration.
- No stochastic sample phase exists.
- Formal moment/error lemmas build.

**Focused checks:** Run the focused tests, fixtures, dumps, and commands named by this task before the repository gate.

**Repository gate:**

```text
cargo xtask verify
cargo xtask pixels-formal
```

**Stop conditions:** Stop if an acceptance condition cannot be met without changing a closed architectural decision, weakening a proof/failure boundary, or adding an external Cargo dependency.

**Commit**

```text
pixels P9.5: filter subpixel normal detail by moments
```

## Task P9.6 — implement direct-light evaluation and bounds

**Requires:** P9.5.

**Produces:** Shade certified geometry under the complete supported light set.

**Files:**

```text
stdlib/core/render_light.wr # new at P-1 basis
stdlib/core/render_material.wr # new at P-1 basis
crates/wrela-compiler/src/pixels/light.rs # new at P-1 basis
crates/wrela-compiler/src/pixels/reference/light.rs # new at P-1 basis
```

**Contract/dump delta:** Only the contract and stable-dump changes explicitly named by this task are permitted.

**Work:**

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

**Tests:**

- Light movement outside declared range rejected at frame input.
- Normal-cone unlit classification never false-lights host samples.
- Point singularity impossible by source contract.
- Scalar/packet light math agrees.
- Contribution bounds flow to display scheduler.

**Focused checks:** Run the focused tests, fixtures, dumps, and commands named by this task before the repository gate.

**Repository gate:**

```text
cargo xtask verify
```

**Stop conditions:** Stop if an acceptance condition cannot be met without changing a closed architectural decision, weakening a proof/failure boundary, or adding an external Cargo dependency.

**Commit**

```text
pixels P9.6: evaluate bounded direct lighting
```

## Task P9.7 — implement certified secondary visibility

**Requires:** P9.6.

**Produces:** Answer shadow/AO/probe visibility using the same complete structural scene without screen projection assumptions.

**Files:**

```text
stdlib/core/render_light.wr # new at P-1 basis
stdlib/core/render_program.wr # new at P-1 basis
crates/wrela-compiler/src/pixels/reference/secondary.rs # new at P-1 basis
formal/pixels/Pixels/RootIsolation.lean # new at P-1 basis
```

**Contract/dump delta:** Only the contract and stable-dump changes explicitly named by this task are permitted.

**Work:**

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

**Tests:**

- BVH traversal and brute-force feature enumeration agree in host tests.
- Thin blocker controls are not skipped.
- Self-shadow acne and light leaks are absent in scale-sweep fixtures.
- CSG subtraction/intersection shadows use exact occupancy.
- No primary tile’s pruned/indexed feature list is reused for unrelated secondary rays.

**Focused checks:** Run the focused tests, fixtures, dumps, and commands named by this task before the repository gate.

**Repository gate:**

```text
cargo xtask verify
cargo xtask pixels-formal
```

**Stop conditions:** Stop if an acceptance condition cannot be met without changing a closed architectural decision, weakening a proof/failure boundary, or adding an external Cargo dependency.

**Commit**

```text
pixels P9.7: answer complete secondary visibility
```

## Task P9.8 — implement area-light source integration

**Requires:** P9.7.

**Produces:** Produce deterministic, band-free soft shadows for rectangle and disk lights.

**Files:**

```text
stdlib/core/render_light.wr # new at P-1 basis
crates/wrela-compiler/src/pixels/reference/area_light.rs # new at P-1 basis
formal/pixels/Pixels/MaterialBound.lean # new at P-1 basis
```

**Contract/dump delta:** Only the contract and stable-dump changes explicitly named by this task are permitted.

**Work:**

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

**Tests:**

- One-edge penumbra fixture is smooth and stable under motion.
- Multiple blockers and near-field light fixtures remain bounded/correct.
- No stochastic shadow rays/noise/denoiser.
- Integrated interval contains a high-resolution host source integral.
- Capacity/depth exhaustion is explicit.

**Focused checks:** Run the focused tests, fixtures, dumps, and commands named by this task before the repository gate.

**Repository gate:**

```text
cargo xtask verify
cargo xtask pixels-conformance
```

**Stop conditions:** Stop if an acceptance condition cannot be met without changing a closed architectural decision, weakening a proof/failure boundary, or adding an external Cargo dependency.

**Commit**

```text
pixels P9.8: integrate certified area-light visibility
```

## Task P9.9 — implement deterministic AO taps

**Requires:** P9.8.

**Produces:** Add local contact shading without a volumetric bake or stochastic rays.

**Files:**

```text
stdlib/core/render_light.wr # new at P-1 basis
crates/wrela-compiler/src/pixels/reference/ao.rs # new at P-1 basis
```

**Contract/dump delta:** Only the contract and stable-dump changes explicitly named by this task are permitted.

**Work:**

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

**Tests:**

- Open plane AO is 1 within exact radius.
- Contact/crevice fixtures darken deterministically.
- AO interval contains dense host reference.
- No full-sphere directions or random kernel.
- AO contribution can be skipped only through display budget proof.

**Focused checks:** Run the focused tests, fixtures, dumps, and commands named by this task before the repository gate.

**Repository gate:**

```text
cargo xtask verify
```

**Stop conditions:** Stop if an acceptance condition cannot be met without changing a closed architectural decision, weakening a proof/failure boundary, or adding an external Cargo dependency.

**Commit**

```text
pixels P9.9: add deterministic normal-distance AO
```

## Task P9.10 — implement shading run summaries and packet evaluation

**Requires:** P9.9.

**Produces:** Amortize material/light work across certified structure while retaining exact output bounds.

**Files:**

```text
stdlib/core/render_material.wr # new at P-1 basis
stdlib/core/render_light.wr # new at P-1 basis
stdlib/core/render_raster.wr # new at P-1 basis
crates/wrela-compiler/src/pixels/reference/shade.rs # new at P-1 basis
```

**Contract/dump delta:** Only the contract and stable-dump changes explicitly named by this task are permitted.

**Work:**

For each run/tile material-light pair construct the fixed summary ladder from P9.4. Summary records contain candidate coefficients plus HDR interval residual per channel.

Packet pixel evaluation:

- advance basis functions by forward differences where polynomial;
- evaluate separable rank terms in SoA;
- add exact/per-pixel residual-sensitive terms only where required;
- combine diffuse/specular/emissive/AO/shadow;
- carry an HDR interval alongside candidate RGB only until byte singleton is proven;
- once all channels fixed, store bytes without further floating work.

Per-pixel exact shading fallback is permitted and bounded by pixel/run capacity; it still uses certified geometry and deterministic BRDF/visibility. It is not a primary visibility fallback.

**Tests:**

- Constant material/light plane shares one summary across maximal runs.
- Summary output interval contains scalar per-pixel reference.
- Scalar/packet candidate bytes agree after verifier.
- Unsupported high-frequency material reaches exact per-pixel path or build/runtime explicit failure, never unchecked rank approximation.
- Runtime counters identify summary ranks and exact-shaded pixels for diagnostics, not acceptance tuning.

**Focused checks:** Run the focused tests, fixtures, dumps, and commands named by this task before the repository gate.

**Repository gate:**

```text
cargo xtask verify
cargo xtask diff-eval
```

**Stop conditions:** Stop if an acceptance condition cannot be met without changing a closed architectural decision, weakening a proof/failure boundary, or adding an external Cargo dependency.

**Commit**

```text
pixels P9.10: shade certified runs with verified summaries
```

## Task P9.11 — implement the display-error budget and refinement queue

**Requires:** P9.10.

**Produces:** Stop every approximation at one common output criterion and choose deterministic refinements.

**Files:**

```text
stdlib/core/render_transfer.wr # new at P-1 basis
stdlib/core/render_actor.wr # new at P-1 basis
crates/wrela-compiler/src/pixels/reference/scheduler.rs # new at P-1 basis
formal/pixels/Pixels/DisplayByte.lean # new at P-1 basis
```

**Contract/dump delta:** Only the contract and stable-dump changes explicitly named by this task are permitted.

**Work:**

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

**Tests:**

- Queue ordering deterministic across cores/hosts.
- No floating division in priority comparison.
- Every refinement strictly decreases a discrete measure `(remaining depths, interval widths)` or terminates, proving bounded progress.
- Exact small fixtures compare scheduler result to exhaustive refinement and produce same final bytes.
- Formal byte singleton theorem gates success.

**Focused checks:** Run the focused tests, fixtures, dumps, and commands named by this task before the repository gate.

**Repository gate:**

```text
cargo xtask verify
cargo xtask pixels-formal
```

**Stop conditions:** Stop if an acceptance condition cannot be met without changing a closed architectural decision, weakening a proof/failure boundary, or adding an external Cargo dependency.

**Commit**

```text
pixels P9.11: schedule refinements in display units
```

## Task P9.12 — replace debug output with final filmic BGRA output

**Requires:** P9.11.

**Produces:** Complete final opaque AAA framebuffer generation.

**Files:**

```text
stdlib/core/render_raster.wr # new at P-1 basis
stdlib/core/render_actor.wr # new at P-1 basis
stdlib/core/render_transfer.wr # new at P-1 basis
tests/golden/boot-pixels-*/ # new at P-1 basis
```

**Contract/dump delta:** Only the contract and stable-dump changes explicitly named by this task are permitted.

**Work:**

For every regular/event pixel:

1. evaluate candidate HDR color and complete interval;
2. run refinement queue until each encoded channel singleton;
3. write exact B,G,R codes and alpha 255;
4. preserve debug visibility mode behind compiler-internal conformance flag, not source option.

Background is an explicit environment material/light color with its own fixed interval. It is not an implicit zero after failed visibility.

Update frame digests/goldens from debug to final output while retaining separate debug conformance expected files.

**Tests:**

- All opaque permanent fixtures produce final filmic bytes.
- Every stored channel had a singleton proof or exact zero-width arithmetic path.
- No output candidate is quantized without endpoint agreement.
- Host framebuffer oracle lies within HDR intervals and final bytes agree.
- No unresolved frame.

**Focused checks:** Run the focused tests, fixtures, dumps, and commands named by this task before the repository gate.

**Repository gate:**

```text
cargo xtask verify
cargo xtask pixels-conformance
cargo xtask verify
```

**Stop conditions:** Stop if an acceptance condition cannot be met without changing a closed architectural decision, weakening a proof/failure boundary, or adding an external Cargo dependency.

**Commit**

```text
pixels P9.12: emit final byte-certified opaque frames
```

## Task P9.13 — add motion/lighting/material quality conformance

**Requires:** P9.12.

**Produces:** Lock visible AAA properties, not only static geometry correctness.

**Files:**

```text
crates/xtask/src/pixels_conformance.rs # new at P-1 basis
tests/golden/boot-pixels-quality/ # new at P-1 basis
tests/pixels_truth/quality/ # new at P-1 basis
```

**Contract/dump delta:** Only the contract and stable-dump changes explicitly named by this task are permitted.

**Work:**

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

**Tests:**

- All sequence digests stable.
- No visibility/identity/shadow classification failures.
- High-frequency detail does not alternate unpredictably under subpixel motion.
- Re-running identical sequence produces byte-identical frames.
- Single/four-core outputs identical.

**Focused checks:** Run the focused tests, fixtures, dumps, and commands named by this task before the repository gate.

**Repository gate:**

```text
cargo xtask pixels-conformance
cargo xtask verify
```

**Stop conditions:** Stop if an acceptance condition cannot be met without changing a closed architectural decision, weakening a proof/failure boundary, or adding an external Cargo dependency.

**Commit**

```text
pixels P9.13: lock opaque AAA quality sequences
```

### Milestone P9 close

Run `cargo xtask verify`. Opaque rendering is now feature-complete and byte-certified. Any later transparency/GI/temporal work composes into the same output verifier and cannot bypass it.

---

# Milestone P10 — ordered transparency and deterministic probe GI

Milestone result: the renderer handles bounded semitransparent surface stacks and deterministic diffuse global illumination. Both compose into the existing HDR interval and display-byte verifier. V1 transparency is absorptive/emissive alpha compositing with no refraction.

## Task P10.1 — classify opaque and transparent material identities

**Requires:** the preceding milestone close gate.

**Produces:** Make layer semantics and maximum stack capacity compile-time facts.

**Files:**

```text
crates/wrela-compiler/src/pixels/material.rs # new at P-1 basis
crates/wrela-compiler/src/pixels/capacities.rs # new at P-1 basis
crates/wrela-compiler/src/pixels/program.rs # new at P-1 basis
stdlib/core/render.wr # new at P-1 basis
```

**Contract/dump delta:** Only the contract and stable-dump changes explicitly named by this task are permitted.

**Work:**

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

**Tests:**

- Opaque fixture uses no transfer tree beyond one absorbing layer.
- Transparent-tail fixture has exact layer capacity.
- Zero-opacity nonemissive layer is safely skipped through material proof.
- Parameterized class changes are event-tracked or conservatively transparent.
- Capacity overflow is compile-time `P015`.

**Focused checks:** Run the focused tests, fixtures, dumps, and commands named by this task before the repository gate.

**Repository gate:**

```text
cargo xtask verify
```

**Stop conditions:** Stop if an acceptance condition cannot be met without changing a closed architectural decision, weakening a proof/failure boundary, or adding an external Cargo dependency.

**Commit**

```text
pixels P10.1: classify bounded transparent layers
```

## Task P10.2 — build ordered transfer layers from the CSG sweep

**Requires:** P10.1.

**Produces:** Convert the complete front-to-back composite boundary sequence into shading/compositing work.

**Files:**

```text
stdlib/core/render_sweep.wr # new at P-1 basis
stdlib/core/render_transfer.wr # new at P-1 basis
crates/wrela-compiler/src/pixels/reference/transfer.rs # new at P-1 basis
```

**Contract/dump delta:** Only the contract and stable-dump changes explicitly named by this task are permitted.

**Work:**

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

**Tests:**

- Ordered layer list agrees with host all-root/CSG oracle.
- Opaque first layer terminates deeper visibility work.
- Transparent layers retain exact q order/slack.
- Coincident transparent surfaces use event corridor/rebuild, not arbitrary ID order.
- Layer capacity is enforced before writes.

**Focused checks:** Run the focused tests, fixtures, dumps, and commands named by this task before the repository gate.

**Repository gate:**

```text
cargo xtask verify
cargo xtask pixels-conformance
```

**Stop conditions:** Stop if an acceptance condition cannot be met without changing a closed architectural decision, weakening a proof/failure boundary, or adding an external Cargo dependency.

**Commit**

```text
pixels P10.2: emit exact ordered surface layers
```

## Task P10.3 — implement balanced transfer trees

**Requires:** P10.2.

**Produces:** Compose transparent stacks associatively and prepare for local kinetic repairs.

**Files:**

```text
stdlib/core/render_transfer.wr # new at P-1 basis
crates/wrela-compiler/src/pixels/reference/transfer.rs # new at P-1 basis
formal/pixels/Pixels/Compositing.lean # new at P-1 basis
```

**Contract/dump delta:** Only the contract and stable-dump changes explicitly named by this task are permitted.

**Work:**

Each shaded layer yields premultiplied transfer:

```text
C = coverage * opacity * shaded_rgb
T = 1 - coverage * opacity
```

For regular interior pixels coverage is 1. Event coverage is interval-valued and enters both C/T bounds.

Store leaves in front-to-back order and build a balanced array tree sized to next power of two from sealed max layers. Identity leaves are `(0,1)`. Parent composition order is left/front then right/back.

For runs where all layer transfer summaries are low-degree/separable, tree nodes also store summary coefficients/residual. Otherwise compose per pixel. The verifier interval always follows the same order.

**Tests:**

- Balanced and linear front-to-back composition agree within arithmetic interval and exact candidate bits where operation order is intentionally matched.
- Tree storage has fixed maximum and deterministic leaf placement.
- Local leaf replacement updates only ancestors.
- Opaque prefix yields residual T exactly/interval containing zero and can absorb tail.
- Formal monoid/local-repair theorems build.

**Focused checks:** Run the focused tests, fixtures, dumps, and commands named by this task before the repository gate.

**Repository gate:**

```text
cargo xtask verify
cargo xtask pixels-formal
```

**Stop conditions:** Stop if an acceptance condition cannot be met without changing a closed architectural decision, weakening a proof/failure boundary, or adding an external Cargo dependency.

**Commit**

```text
pixels P10.3: compose transparent stacks as transfer trees
```

## Task P10.4 — implement certified transparency-tail termination

**Requires:** P10.3.

**Produces:** Avoid shading deep transparent layers that cannot affect stored output.

**Files:**

```text
stdlib/core/render_transfer.wr # new at P-1 basis
crates/wrela-compiler/src/pixels/material.rs # new at P-1 basis
formal/pixels/Pixels/TransparencyTail.lean # new at P-1 basis
```

**Contract/dump delta:** Only the contract and stable-dump changes explicitly named by this task are permitted.

**Work:**

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

**Tests:**

- Bright-tail control continues traversal despite low opacity.
- Once tail condition holds, adding more nonnegative-opacity layers cannot invalidate the bound without changed suffix radiance contract.
- Runtime never drops layers based only on layer count or transmittance scalar candidate.
- Formal tail theorems build.
- Host exact full-stack bytes equal early-out bytes.

**Focused checks:** Run the focused tests, fixtures, dumps, and commands named by this task before the repository gate.

**Repository gate:**

```text
cargo xtask verify
cargo xtask pixels-formal
```

**Stop conditions:** Stop if an acceptance condition cannot be met without changing a closed architectural decision, weakening a proof/failure boundary, or adding an external Cargo dependency.

**Commit**

```text
pixels P10.4: terminate invisible transparent tails
```

## Task P10.5 — define deterministic probe-GI semantics

**Requires:** P10.4.

**Produces:** Make diffuse GI a closed renderer model rather than an unbounded approximation claim.

**Files:**

```text
docs/language/07-pixels.md # new at P-1 basis
stdlib/core/render.wr # new at P-1 basis
stdlib/core/render_probe.wr # new at P-1 basis
crates/wrela-compiler/src/pixels/probe.rs # new at P-1 basis
```

**Contract/dump delta:** Only the contract and stable-dump changes explicitly named by this task are permitted.

**Work:**

V1 probe model is normative renderer semantics:

- three nested camera-centered clipmap levels;
- each level dimensions `16 × 8 × 16` probes;
- base spacing from sealed `ProbeConfig.base_spacing`; each next level spacing ×4;
- each probe stores 9 real spherical-harmonic coefficients per RGB channel for diffuse incident radiance, six axis distance moments derived from the same 32 rays, plus validity/version;
- coefficients stored f32 candidate plus verifier radius;
- 32 fixed unit directions per probe from a checked-in direction table with solid-angle weights summing to `4π` within stored interval;
- each direction traces one complete secondary segment to scene far/environment;
- hit contribution is outgoing diffuse/emissive approximation from a bounded one-bounce material/light query; miss contribution is environment;
- no random rotation or stochastic sequence;
- accumulation order is direction ID ascending, then channel/coefficient order;
- probe interpolation is trilinear within one level plus deterministic blend between two levels based on camera distance.

This is deterministic finite one-bounce probe GI. The renderer guarantees numeric/output correctness relative to this model, not equality to the full rendering equation.

Compiler emits direction/SH basis tables and capacities. Probe config may reduce levels/dims but cannot exceed v1 maxima; defaults above are flagship.

Admission includes the all-invalid frame: every configured probe times all 32 complete secondary rays, plus accumulation and presentation, must fit the declared initialization deadline and the applicable frame deadline. If it cannot, the declaration must reduce/disable probes or choose a static preinitialized probe mode. `AaaByteExact` never accepts a dynamic configuration on the assumption that only a typical subset invalidates.

**Tests:**

- Direction/weight/SH tables have fixed digests and are immutable numeric-contract data.
- Probe memory exact and reported.
- No RNG/time-dependent direction phase.
- GI semantics are fully stated in docs.
- Zero-GI configuration is explicit source config, not hidden fallback.

**Focused checks:** Run the focused tests, fixtures, dumps, and commands named by this task before the repository gate.

**Repository gate:**

```text
cargo xtask verify
```

**Stop conditions:** Stop if an acceptance condition cannot be met without changing a closed architectural decision, weakening a proof/failure boundary, or adding an external Cargo dependency.

**Commit**

```text
pixels P10.5: fix deterministic diffuse probe GI
```

## Task P10.6 — implement probe initialization

**Requires:** P10.5.

**Produces:** Ensure the first presented frame has fully defined GI state.

**Files:**

```text
stdlib/core/render_probe.wr # new at P-1 basis
stdlib/core/render_actor.wr # new at P-1 basis
crates/wrela-compiler/src/pixels/glue.rs # new at P-1 basis
```

**Contract/dump delta:** Only the contract and stable-dump changes explicitly named by this task are permitted.

**Work:**

Before the first frame using GI is presented:

1. place clipmaps at camera snapped to finest-level spacing;
2. update every probe in all levels in deterministic probe ID order, partitioned across workers by contiguous probe ranges;
3. trace 32 directions per probe;
4. combine worker results in probe ID order;
5. mark all probes valid for current scene/light/material dependency versions;
6. only then shade/present frame.

Initialization is finite and bounded by compiled capacities. It may span multiple actor turns/checkpoints internally but the public first render call does not return success until complete. Cancellation returns frame input and leaves probes invalid.

**Tests:**

- First GI frame is independent of uninitialized memory/previous runs.
- Single/four-core probe coefficients and frame bytes identical.
- All probe writes owned/disjoint.
- Initialization interruption cannot mark partial state valid.
- Zero-level/no-GI config skips initialization exactly.

**Focused checks:** Run the focused tests, fixtures, dumps, and commands named by this task before the repository gate.

**Repository gate:**

```text
cargo xtask verify
```

**Stop conditions:** Stop if an acceptance condition cannot be met without changing a closed architectural decision, weakening a proof/failure boundary, or adding an external Cargo dependency.

**Commit**

```text
pixels P10.6: initialize deterministic probe state
```

## Task P10.7 — implement probe update and invalidation

**Requires:** P10.6.

**Produces:** Keep GI state coherent with changing scene coefficients and clipmap movement.

**Files:**

```text
stdlib/core/render_probe.wr # new at P-1 basis
stdlib/core/render_actor.wr # new at P-1 basis
crates/wrela-compiler/src/pixels/probe.rs # new at P-1 basis
```

**Contract/dump delta:** Only the contract and stable-dump changes explicitly named by this task are permitted.

**Work:**

Compiler emits dependency/swept-bound records per object/light/material. At frame start:

- compare current/previous dependency slots;
- compute conservative swept AABB for changed geometry over the frame interval;
- invalidate probes whose support radius intersects swept AABB;
- invalidate all probes influenced by changed environment/exposure only where GI semantics depend on it; exposure/post do not invalidate radiance probes;
- invalidate affected probes for light/material/emissive changes using compiler influence bounds;
- when camera clipmap snaps, remap retained cells and mark newly exposed cells invalid;
- update every invalid probe before presenting the frame.

No fixed per-frame update budget may leave stale probes in `AaaByteExact`. If invalid count exceeds capacity (which should equal all probes), internal error. Work can be large but remains correct.

**Tests:**

- Static frame updates zero probes.
- Rigid moving object invalidates exactly a conservative neighborhood.
- Camera clipmap shift retains overlapping world-coordinate probes exactly.
- Changed direct-only post setting does not invalidate probes.
- Presented frame never reads invalid probe.

**Focused checks:** Run the focused tests, fixtures, dumps, and commands named by this task before the repository gate.

**Repository gate:**

```text
cargo xtask verify
```

**Stop conditions:** Stop if an acceptance condition cannot be met without changing a closed architectural decision, weakening a proof/failure boundary, or adding an external Cargo dependency.

**Commit**

```text
pixels P10.7: update all invalidated GI probes
```

## Task P10.8 — shade from probe SH with numeric bounds

**Requires:** P10.7.

**Produces:** Add smooth diffuse GI to the certified structure and output verifier.

**Files:**

```text
stdlib/core/render_probe.wr # new at P-1 basis
stdlib/core/render_light.wr # new at P-1 basis
stdlib/core/render_material.wr # new at P-1 basis
crates/wrela-compiler/src/pixels/reference/probe.rs # new at P-1 basis
```

**Contract/dump delta:** Only the contract and stable-dump changes explicitly named by this task are permitted.

**Work:**

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

- consume the six axis distance moments defined and populated by P10.5/P10.6;
- compare surface-to-probe vector/distance against directional mean/min-distance interval;
- downweight probes whose recorded occluder lies in front using a fixed smooth function;
- clamp weights and renormalize; if all zero, GI is zero for that sample.

This leak-reduction function is part of renderer semantics and documented.

**Tests:**

- Open diffuse environment produces expected smooth irradiance.
- Wall-separated control reduces leaks relative to unweighted interpolation and matches normative host model exactly.
- Probe candidate/interval contains host scalar evaluation.
- No invalid probe read.
- Summary/packet shading includes GI without changing byte proof rules.

**Focused checks:** Run the focused tests, fixtures, dumps, and commands named by this task before the repository gate.

**Repository gate:**

```text
cargo xtask verify
cargo xtask pixels-conformance
```

**Stop conditions:** Stop if an acceptance condition cannot be met without changing a closed architectural decision, weakening a proof/failure boundary, or adding an external Cargo dependency.

**Commit**

```text
pixels P10.8: shade deterministic bounded probe GI
```

## Task P10.9 — integrate transparency and GI into final frame path

**Requires:** P10.8.

**Produces:** Complete the full v1 lighting/compositing stack.

**Files:**

```text
stdlib/core/render_actor.wr # new at P-1 basis
stdlib/core/render_raster.wr # new at P-1 basis
stdlib/core/render_transfer.wr # new at P-1 basis
stdlib/core/render_probe.wr # new at P-1 basis
tests/golden/boot-pixels-transparent/ # new at P-1 basis
tests/golden/boot-pixels-gi/ # new at P-1 basis
```

**Contract/dump delta:** Only the contract and stable-dump changes explicitly named by this task are permitted.

**Work:**

- initialize/update probes before worker shading jobs;
- shade every visible/transparent layer with direct+AO+GI+emissive;
- compose transfer tree front-to-back over environment;
- apply coverage at event pixels correctly per layer/side;
- run tail termination/refinement;
- run filmic/transfer singleton proof;
- write final bytes/present.

Debug visibility path continues to bypass shading/GI/transparency for conformance.

**Tests:**

- Transparent stack and GI fixtures produce final exact digests.
- Full-stack host reference and guest bytes agree.
- No stale probe/unfinished transfer tree can be presented.
- Opaque fixtures remain byte-stable unless the intentionally added GI config changes expected output.
- Failure preserves prior front buffer.

**Focused checks:** Run the focused tests, fixtures, dumps, and commands named by this task before the repository gate.

**Repository gate:**

```text
cargo xtask verify
cargo xtask pixels-conformance
cargo xtask verify
```

**Stop conditions:** Stop if an acceptance condition cannot be met without changing a closed architectural decision, weakening a proof/failure boundary, or adding an external Cargo dependency.

**Commit**

```text
pixels P10.9: complete transparent GI frame composition
```

## Task P10.10 — add full lighting/transparency conformance sequences

**Requires:** P10.9.

**Produces:** Lock the remaining AAA output classes before temporal maintenance.

**Files:**

```text
crates/xtask/src/pixels_conformance.rs # new at P-1 basis
tests/pixels_truth/transparent/ # new at P-1 basis
tests/pixels_truth/gi/ # new at P-1 basis
tests/golden/boot-pixels-quality/ # new at P-1 basis
```

**Contract/dump delta:** Only the contract and stable-dump changes explicitly named by this task are permitted.

**Work:**

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

**Tests:**

- Zero normative model divergence.
- Tail early-out exact final bytes.
- Probe invalidation/remap exact.
- Repeated identical runs deterministic.
- Single/four-core outputs identical.

**Focused checks:** Run the focused tests, fixtures, dumps, and commands named by this task before the repository gate.

**Repository gate:**

```text
cargo xtask pixels-conformance
cargo xtask verify
```

**Stop conditions:** Stop if an acceptance condition cannot be met without changing a closed architectural decision, weakening a proof/failure boundary, or adding an external Cargo dependency.

**Commit**

```text
pixels P10.10: lock transparent and GI conformance
```

### Milestone P10 close

Run `cargo xtask verify`. The renderer is now visually and semantically complete from scratch. Temporal work begins only after this point and must prove equivalence to rebuilding the same normative frame state.

---

# Milestone P11 — kinetic proof maintenance, static reuse, and validated shading transport

Milestone result: the renderer reuses certified structure and shading between frames when explicit event/margin proofs remain valid. Camera cuts, out-of-rate changes, singular events, or uneconomic repair invoke bounded tile/full from-scratch sweep. A compile/test switch disabling all kinetic paths produces identical success/failure and displayed bytes.

## Task P11.1 — implement complete frame dependency digests

**Requires:** the preceding milestone close gate.

**Produces:** Make exact static-frame reuse and invalidation depend on every output-affecting input.

**Files:**

```text
stdlib/core/render_actor.wr # new at P-1 basis
stdlib/core/render_probe.wr # new at P-1 basis
crates/wrela-compiler/src/pixels/params.rs # new at P-1 basis
crates/wrela-compiler/src/pixels/glue.rs # new at P-1 basis
formal/pixels/Pixels/Kinetic.lean # new at P-1 basis
```

**Contract/dump delta:** Only the contract and stable-dump changes explicitly named by this task are permitted.

**Work:**

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

**Tests:**

- Changing each dependency class changes `all` and expected subdigest only.
- Changing unused P field changes none.
- Failed frame does not update previous-presented digest.
- Static repeated frames perform zero sweep/shading/probe writes and preserve exact visible digest.
- Formal dependency equality theorem is instantiated/documented.

**Focused checks:** Run the focused tests, fixtures, dumps, and commands named by this task before the repository gate.

**Repository gate:**

```text
cargo xtask verify
cargo xtask pixels-formal
```

**Stop conditions:** Stop if an acceptance condition cannot be met without changing a closed architectural decision, weakening a proof/failure boundary, or adding an external Cargo dependency.

**Commit**

```text
pixels P11.1: reuse exactly unchanged frames
```

## Task P11.2 — compile and evaluate temporal derivative programs

**Requires:** P11.1.

**Produces:** Transport roots/events/shading with conservative first/second-order bounds.

**Files:**

```text
crates/wrela-compiler/src/pixels/derivatives.rs # new at P-1 basis
crates/wrela-compiler/src/pixels/program.rs # new at P-1 basis
stdlib/core/render_sweep.wr # new at P-1 basis
stdlib/core/render_events.wr # new at P-1 basis
formal/pixels/Pixels/Kinetic.lean # new at P-1 basis
```

**Contract/dump delta:** Only the contract and stable-dump changes explicitly named by this task are permitted.

**Work:**

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

A condition/homotopy-style estimate may propose a temporal center, refinement
order, or candidate carry length for a regular sheet. It is candidate
arithmetic only. The accepted next-frame enclosure still uses the complete
dyadic first/second-order remainder, and every event, identity, adjacent
q-order, shading, and quantization certificate is checked independently. No
condition estimate may skip a predicate family or convert a failed structural
certificate into continued tracking.

Actual frame delta is normalized to one presentation interval. Skipped/late frames scale bounds with checked integer/rational dt; beyond compiler supported temporal box, invalidate and rebuild.

**Tests:**

- Zero deltas produce exact zero transport/remainder where expressions static.
- Derivative programs use only influencing slots.
- Transport intervals contain from-scratch next-frame root/event/shading truth on deterministic sequences.
- Condition/homotopy proposals never change the accepted interval or output;
  disabling them preserves success/failure and bytes.
- `G_q` containing zero invalidates transport.
- Formal implicit-flow/remainder theorems build.

**Focused checks:** Run the focused tests, fixtures, dumps, and commands named by this task before the repository gate.

**Repository gate:**

```text
cargo xtask verify
cargo xtask pixels-formal
```

**Stop conditions:** Stop if an acceptance condition cannot be met without changing a closed architectural decision, weakening a proof/failure boundary, or adding an external Cargo dependency.

**Commit**

```text
pixels P11.2: emit bounded temporal transport programs
```

## Task P11.3 — define persistent kinetic state

**Requires:** P11.2.

**Produces:** Store only the proof state needed to validate/repair the next frame.

**Files:**

```text
stdlib/core/render_sweep.wr # new at P-1 basis
stdlib/core/render_events.wr # new at P-1 basis
stdlib/core/render_actor.wr # new at P-1 basis
crates/wrela-compiler/src/pixels/capacities.rs # new at P-1 basis
```

**Contract/dump delta:** Only the contract and stable-dump changes explicitly named by this task are permitted.

**Work:**

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

**Tests:**

- State bytes derive/report exactly.
- Reset/invalidate cannot expose stale next-generation counts.
- Successful frame atomically commits framebuffer, probes, dependency digest, and kinetic state.
- Failed frame commits none.
- Static-frame path can use prior state without mutation.

**Focused checks:** Run the focused tests, fixtures, dumps, and commands named by this task before the repository gate.

**Repository gate:**

```text
cargo xtask verify
```

**Stop conditions:** Stop if an acceptance condition cannot be met without changing a closed architectural decision, weakening a proof/failure boundary, or adding an external Cargo dependency.

**Commit**

```text
pixels P11.3: persist certified frame structure safely
```

## Task P11.4 — implement compressed slack validation

**Requires:** P11.3.

**Produces:** Reject or retain previous proofs with a small integer common-path predicate.

**Files:**

```text
stdlib/core/render_events.wr # new at P-1 basis
stdlib/core/render_sweep.wr # new at P-1 basis
crates/wrela-compiler/src/pixels/reference/kinetic.rs # new at P-1 basis
formal/pixels/Pixels/Kinetic.lean # new at P-1 basis
```

**Contract/dump delta:** Only the contract and stable-dump changes explicitly named by this task are permitted.

**Work:**

For each persistent run/event/braid, retain component margins in debug/diagnostic builds and the minimum margin plus owner in production record. Compiler-generated transport program supplies conservative perturbation contributions:

```text
space + parameter + time + camera + light/material + quantization
```

A compressed record survives only when the upper perturbation is strictly less than the lower stored margin. Equality fails closed.

For a record whose minimum owner changes after transport, recompute all component margins before compressing next generation. Do not subtract perturbation repeatedly from one stale scalar across many frames; revalidate against current equations at least every generated maximum macroframe length, and immediately when any dependency changed.

Set v1 maximum kinetic carry length to 8 presented frames. On the 9th, revalidate/rebuild even if slack remains. This fixed bound limits accumulated arithmetic/remainder and avoids indefinite proof aging.

**Tests:**

- Compressed predicate matches full component check on all vectors.
- Equality/overflow invalidates.
- No record persists more than 8 frames without fresh verification.
- Static digest-equal frame reuse is separate and may persist indefinitely because inputs are equal.
- Formal margin theorem builds.

**Focused checks:** Run the focused tests, fixtures, dumps, and commands named by this task before the repository gate.

**Repository gate:**

```text
cargo xtask verify
cargo xtask pixels-formal
```

**Stop conditions:** Stop if an acceptance condition cannot be met without changing a closed architectural decision, weakening a proof/failure boundary, or adding an external Cargo dependency.

**Commit**

```text
pixels P11.4: validate compressed kinetic proof slack
```

## Task P11.5 — schedule possible event failures conservatively

**Requires:** P11.4.

**Produces:** Know which local predicates need re-evaluation without polling every event every frame.

**Files:**

```text
stdlib/core/render_events.wr # new at P-1 basis
crates/wrela-compiler/src/pixels/reference/kinetic.rs # new at P-1 basis
```

**Contract/dump delta:** Only the contract and stable-dump changes explicitly named by this task are permitted.

**Work:**

For each event predicate with current signed interval value `V`, first derivative interval `D`, and second-order remainder rate `R`, compute a conservative earliest possible zero time over `dt >= 0`. Use this fixed hierarchy:

1. if `V` contains zero: due now;
2. if derivative/remainder cannot move toward zero: infinity within macroframe;
3. linear lower bound `distance_to_zero / max_toward_speed` gives candidate;
4. second-order bound solves conservative quadratic inequality using outward dyadic arithmetic;
5. round time down to presentation-frame ticks;
6. cap at macroframe length 8.

Store events in a fixed binary min-heap keyed by due frame, tile ID, event ID. Heap capacity is exact emitted event count. Update changed event keys through deterministic rebuild of the tile heap slice rather than pointer mutation complexity.

At each frame, only due events plus records invalidated by dependency digests are fully re-evaluated. Nondue certificate remains valid by bound.

Classify each due/failing predicate with stable kinetic-data terminology:
scheduled expiry, isolated transverse event, simultaneous event, degenerate
event, dependency invalidation, or numeric invalidation. Record due-event
count and affected-domain density per tile. An `event storm` is a diagnostic
class for many due/overlapping events, never a heuristic correctness cutoff;
P11.9 uses sealed work bounds to choose the equivalent full sweep.

**Tests:**

- Predicted due time never exceeds actual first sign-zero in deterministic from-scratch comparisons.
- Zero-rate events schedule infinity/static.
- Heap order deterministic.
- No missed event when frame skips multiple ticks.
- Numeric failure schedules due now, never infinity.
- Event-storm classification is deterministic and every due predicate has one
  failure/expiry class.

**Focused checks:** Run the focused tests, fixtures, dumps, and commands named by this task before the repository gate.

**Repository gate:**

```text
cargo xtask verify
```

**Stop conditions:** Stop if an acceptance condition cannot be met without changing a closed architectural decision, weakening a proof/failure boundary, or adding an external Cargo dependency.

**Commit**

```text
pixels P11.5: schedule conservative event expiry
```

## Task P11.6 — transport and revalidate run/event geometry

**Requires:** P11.5.

**Produces:** Update a tile without reconstructing it when all local structure remains certified.

**Files:**

```text
stdlib/core/render_sweep.wr # new at P-1 basis
stdlib/core/render_events.wr # new at P-1 basis
stdlib/core/render_actor.wr # new at P-1 basis
```

**Contract/dump delta:** Only the contract and stable-dump changes explicitly named by this task are permitted.

**Work:**

For each tile whose dependencies changed but state may survive:

1. transport event curves/endpoints with derivative/remainder;
2. transport each sheet q/q derivatives;
3. evaluate compressed slack;
4. rerun cheap root/feature/order predicates at transported anchors;
5. rebuild fixed-q setup and coverage records for current coordinates;
6. refresh output/shading summaries or mark them for P11.10 transport;
7. write candidate state into next generation.

A transported run may shrink/expand only within neighboring event corridor bounds and tile domain. If event order changes, endpoints overlap, or any proof fails, mark affected tile/domain for repair. Transport cannot invent/delete runs.

**Tests:**

- Transported static/slow sequences produce same visibility runs/bytes as from-scratch mode.
- Run domains remain exact partition after transport.
- No event crossing occurs in a retained regular run.
- Transported records receive fresh current-frame margins before commit.
- Failure marks repair, not stale reuse.

**Focused checks:** Run the focused tests, fixtures, dumps, and commands named by this task before the repository gate.

**Repository gate:**

```text
cargo xtask verify
cargo xtask pixels-conformance
```

**Stop conditions:** Stop if an acceptance condition cannot be met without changing a closed architectural decision, weakening a proof/failure boundary, or adding an external Cargo dependency.

**Commit**

```text
pixels P11.6: transport certified frame structure
```

## Task P11.7 — maintain adjacent q-order braids

**Requires:** P11.6.

**Produces:** Preserve complete layer/root order with O(adjacent relations) certificates.

**Files:**

```text
stdlib/core/render_events.wr # new at P-1 basis
stdlib/core/render_transfer.wr # new at P-1 basis
crates/wrela-compiler/src/pixels/reference/kinetic.rs # new at P-1 basis
formal/pixels/Pixels/QOrder.lean # new at P-1 basis
```

**Contract/dump delta:** Only the contract and stable-dump changes explicitly named by this task are permitted.

**Work:**

For each regular run store front-to-back sheet/layer IDs and adjacent q-order slack. On transport:

- evaluate all adjacent relations in packets where possible;
- if all strict, complete order survives by theorem;
- if one relation fails and exactly one `DepthSwap` event is due in the same domain, isolate swap and use P11.8 handler;
- if multiple/nonadjacent relations fail, rebuild affected domain;
- transparent transfer-tree leaf order follows the braid.

Do not monitor all pairwise relations. Do not assume a failed relation means an actual swap; it means local proof expired.

**Tests:**

- Adjacent checks imply same total order as from-scratch root sorting.
- Close-depth/depth-swap sequence repairs or rebuilds without wrong frame.
- Transparent tree order always matches braid/current from-scratch order.
- Packet/scalar failure counts agree.
- Formal braid theorems build.

**Focused checks:** Run the focused tests, fixtures, dumps, and commands named by this task before the repository gate.

**Repository gate:**

```text
cargo xtask verify
cargo xtask pixels-formal
```

**Stop conditions:** Stop if an acceptance condition cannot be met without changing a closed architectural decision, weakening a proof/failure boundary, or adding an external Cargo dependency.

**Commit**

```text
pixels P11.7: maintain kinetic occlusion braids
```

## Task P11.8 — implement the limited local surgery set

**Requires:** P11.7.

**Produces:** Handle simple certified combinatorial events directly and rebuild everything else.

**Files:**

```text
stdlib/core/render_events.wr # new at P-1 basis
stdlib/core/render_sweep.wr # new at P-1 basis
crates/wrela-compiler/src/pixels/program.rs # new at P-1 basis
```

**Contract/dump delta:** Only the contract and stable-dump changes explicitly named by this task are permitted.

**Work:**

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

**Tests:**

- Each handler checks every precondition at runtime.
- Failing one precondition rebuilds; no partial surgery.
- Handler output equals from-scratch result on event sequences.
- Simultaneous-event fixture always rebuilds.
- Surgery counters are diagnostic only.

**Focused checks:** Run the focused tests, fixtures, dumps, and commands named by this task before the repository gate.

**Repository gate:**

```text
cargo xtask verify
cargo xtask pixels-conformance
```

**Stop conditions:** Stop if an acceptance condition cannot be met without changing a closed architectural decision, weakening a proof/failure boundary, or adding an external Cargo dependency.

**Commit**

```text
pixels P11.8: repair only certified simple events
```

## Task P11.9 — choose local repair versus full sweep by sealed cost bounds

**Requires:** P11.8.

**Produces:** Avoid heuristic storm thresholds and guarantee an upper-bounded recovery path.

**Files:**

```text
stdlib/core/render_actor.wr # new at P-1 basis
crates/wrela-compiler/src/pixels/cost_bounds.rs # new at P-1 basis
crates/wrela-compiler/src/pixels/program.rs # new at P-1 basis
```

**Contract/dump delta:** Only the contract and stable-dump changes explicitly named by this task are permitted.

**Work:**

Compiler emits conservative static work weights for:

- full tile sweep;
- each local rebuild domain class;
- transport/revalidation;
- surgery handler;
- full frame sweep.

Inputs also include exact due-event/affected-domain counts from the current
validated state. Telemetry histograms and historical averages are forbidden
as weight multipliers; the choice uses only current checked counts and sealed
per-class worst-case weights.

At frame start, after identifying invalid/expired tiles, compute checked sum of local worst-case weights. Choose local repair only if:

```text
transport_weight + local_repair_weight < full_sweep_weight
```

Equality chooses full sweep. A camera cut, output-mode change, out-of-range temporal delta, invalid previous state, or changed frame-program digest chooses full sweep immediately.

Weights are versioned structural operation counts, not claimed hardware cycles. They exist only to choose between two semantically equivalent paths deterministically.

**Tests:**

- Choice deterministic and input-derived.
- Full sweep path always available for valid input.
- A synthetic surgery storm chooses full sweep.
- A diagnostic event storm with cheap disjoint repairs may still choose local
  repair when the sealed inequality proves it cheaper; the label itself has no
  control authority.
- Changing weights changes build/numeric revision and report.
- Both paths produce identical output/error.

**Focused checks:** Run the focused tests, fixtures, dumps, and commands named by this task before the repository gate.

**Repository gate:**

```text
cargo xtask verify
```

**Stop conditions:** Stop if an acceptance condition cannot be met without changing a closed architectural decision, weakening a proof/failure boundary, or adding an external Cargo dependency.

**Commit**

```text
pixels P11.9: choose rebuilds from sealed work bounds
```

## Task P11.10 — transport shading between `shade_hz` frames

**Requires:** P11.9.

**Produces:** Allow 30 Hz expensive shading with 60 Hz presentation only when the exact displayed bytes remain certified.

**Files:**

```text
stdlib/core/render_material.wr # new at P-1 basis
stdlib/core/render_light.wr # new at P-1 basis
stdlib/core/render_actor.wr # new at P-1 basis
formal/pixels/Pixels/Kinetic.lean # new at P-1 basis
```

**Contract/dump delta:** Only the contract and stable-dump changes explicitly named by this task are permitted.

**Work:**

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

**Tests:**

- `shade_hz=refresh_hz` equals ordinary path.
- Intermediate transported frames equal from-scratch fully shaded bytes on conformance sequences.
- Disoccluded pixels never sample old background/foreground color.
- Failed byte proof triggers current shading, not approximate output.
- Formal transport/slack theorem applies.

**Focused checks:** Run the focused tests, fixtures, dumps, and commands named by this task before the repository gate.

**Repository gate:**

```text
cargo xtask verify
cargo xtask pixels-conformance
cargo xtask pixels-formal
```

**Stop conditions:** Stop if an acceptance condition cannot be met without changing a closed architectural decision, weakening a proof/failure boundary, or adding an external Cargo dependency.

**Commit**

```text
pixels P11.10: transport shading only under byte proof
```

## Task P11.11 — implement deterministic crisp temporal policy

**Requires:** P11.10.

**Produces:** Finish temporal presentation without TAA ghosting or stochastic jitter.

**Files:**

```text
stdlib/core/render.wr # new at P-1 basis
stdlib/core/render_actor.wr # new at P-1 basis
docs/language/07-pixels.md # new at P-1 basis
```

**Contract/dump delta:** Only the contract and stable-dump changes explicitly named by this task are permitted.

**Work:**

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

**Tests:**

- Source cannot enable hidden TAA/jitter in profile v1.
- Repeated static frames exact.
- Moving edge sequences show no history ghosts because no history blend exists.
- Documentation states the chosen temporal aesthetic.
- Replay captures exact current-time inputs/frame index.

**Focused checks:** Run the focused tests, fixtures, dumps, and commands named by this task before the repository gate.

**Repository gate:**

```text
cargo xtask verify
```

**Stop conditions:** Stop if an acceptance condition cannot be met without changing a closed architectural decision, weakening a proof/failure boundary, or adding an external Cargo dependency.

**Commit**

```text
pixels P11.11: seal the crisp temporal profile
```

## Task P11.12 — add kinetic-disable equivalence mode

**Requires:** P11.11.

**Produces:** Permanently prove temporal maintenance is an optimization only.

**Files:**

```text
stdlib/core/render_actor.wr # new at P-1 basis
crates/wrela-compiler/src/pixels/glue.rs # new at P-1 basis
crates/xtask/src/pixels_conformance.rs # new at P-1 basis
```

**Contract/dump delta:** Only the contract and stable-dump changes explicitly named by this task are permitted.

**Work:**

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

**Tests:**

- All sequences byte-identical enabled/disabled.
- Any mismatch prints first frame/tile/pixel/channel and both path diagnostics.
- Kinetic-disabled setting is not author-facing and does not ship as a runtime branch in release image; build specialization removes it.
- Error outcomes match exactly.

**Focused checks:** Run the focused tests, fixtures, dumps, and commands named by this task before the repository gate.

**Repository gate:**

```text
cargo xtask pixels-conformance --kinetic-diff
cargo xtask verify
```

**Stop conditions:** Stop if an acceptance condition cannot be met without changing a closed architectural decision, weakening a proof/failure boundary, or adding an external Cargo dependency.

**Commit**

```text
pixels P11.12: prove kinetic maintenance byte-equivalent
```

## Task P11.13 — lock temporal event and camera-cut sequences

**Requires:** P11.12.

**Produces:** Cover every maintenance/rebuild class permanently.

**Files:**

```text
crates/xtask/src/pixels_conformance.rs # new at P-1 basis
tests/pixels_truth/kinetic/ # new at P-1 basis
tests/golden/boot-pixels-kinetic/ # new at P-1 basis
```

**Contract/dump delta:** Only the contract and stable-dump changes explicitly named by this task are permitted.

**Work:**

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

**Tests:**

- Zero byte/error divergence.
- No stale-state use after cut/failure.
- Surgery used only in its three certified classes.
- Full sweep selected when sealed work bound says so.
- All successful frames have zero unresolved output.

**Focused checks:** Run the focused tests, fixtures, dumps, and commands named by this task before the repository gate.

**Repository gate:**

```text
cargo xtask pixels-conformance --all-temporal
cargo xtask verify
```

**Stop conditions:** Stop if an acceptance condition cannot be met without changing a closed architectural decision, weakening a proof/failure boundary, or adding an external Cargo dependency.

**Commit**

```text
pixels P11.13: lock kinetic and cut conformance
```

### Milestone P11 close

Run `cargo xtask verify`. The maintained-frame architecture is complete only when it is byte-equivalent to rebuilding every frame and all complex events safely route to local/full validated sweep.

---

# Milestone P12 — generated coefficient programs, SIMD/backend closure, and compiler cost admission

Milestone result: renderer hot paths execute through real Wrela/AArch64 code with no modeled packet fiction. The compiler evaluates coefficients once per frame, emits only the used primitive/material kernel palette, proves vector/scalar equivalence, reports actual assembly/register/slot traffic, and applies the existing A76 cost model to the sealed renderer workload.

## Task P12.1 — generate one per-frame coefficient evaluator

**Requires:** the preceding milestone close gate.

**Produces:** Move arbitrary parameter expression work out of root/event/pixel loops.

**Files:**

```text
crates/wrela-compiler/src/pixels/glue.rs # new at P-1 basis
crates/wrela-compiler/src/pixels/scalar.rs # new at P-1 basis
stdlib/core/render_actor.wr # new at P-1 basis
stdlib/core/render_program.wr # new at P-1 basis
```

**Contract/dump delta:** Only the contract and stable-dump changes explicitly named by this task are permitted.

**Work:**

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

**Tests:**

- Coefficient snapshot bytes/count match report.
- Candidate/interval values agree with Rust symbolic reference on parameter-corner/random vectors.
- Static coefficients generate no runtime instructions.
- Common subexpressions evaluated once.
- Nonfinite/domain failure aborts before worker jobs/framebuffer writes.

**Focused checks:** Run the focused tests, fixtures, dumps, and commands named by this task before the repository gate.

**Repository gate:**

```text
cargo xtask verify
cargo xtask diff-eval
```

**Stop conditions:** Stop if an acceptance condition cannot be met without changing a closed architectural decision, weakening a proof/failure boundary, or adding an external Cargo dependency.

**Commit**

```text
pixels P12.1: evaluate renderer coefficients once per frame
```

## Task P12.2 — generate the exact used kernel palette

**Requires:** P12.1.

**Produces:** Avoid a whole-scene scalar tape interpreter while retaining compact structural data.

**Files:**

```text
crates/wrela-compiler/src/pixels/glue.rs # new at P-1 basis
crates/wrela-compiler/src/pixels/program.rs # new at P-1 basis
stdlib/core/render_program.wr # new at P-1 basis
stdlib/core/render_material.wr # new at P-1 basis
stdlib/core/render_light.wr # new at P-1 basis
```

**Contract/dump delta:** Only the contract and stable-dump changes explicitly named by this task are permitted.

**Work:**

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

**Tests:**

- Unused primitive/material/light cases absent from generated typed/MachineWir dumps.
- Used tag always has one case; missing case internal build error.
- Specialized and op-program material evaluators agree.
- Dispatch count/bytes reported.
- No scene-wide field tape evaluation in regular run/raster/shading call graph.

**Focused checks:** Run the focused tests, fixtures, dumps, and commands named by this task before the repository gate.

**Repository gate:**

```text
cargo xtask verify
cargo xtask diff-eval
```

**Stop conditions:** Stop if an acceptance condition cannot be met without changing a closed architectural decision, weakening a proof/failure boundary, or adding an external Cargo dependency.

**Commit**

```text
pixels P12.2: generate the used renderer kernel palette
```

## Task P12.3 — complete SIMD and one-ISA idiom lowering required by Pixels

**Requires:** P12.2.

**Produces:** Implement the closed 128-bit vector semantics already promised by the language/library contract.

**Files:**

```text
crates/wrela-compiler/src/sema/types.rs
crates/wrela-compiler/src/sema/bodies.rs
crates/wrela-compiler/src/flowwir.rs
crates/wrela-compiler/src/mwir.rs
crates/wrela-compiler/src/lower.rs
crates/wrela-compiler/src/codegen.rs
crates/wrela-compiler/src/encode.rs
crates/wrela-machine/src/lib.rs
crates/wrela-vmm/src/hv.rs
crates/wrela-vmm/src/lib.rs
stdlib/core/simd.wr # new at P-1 basis
docs/language/05-library.md
docs/language/06-machine.md
```

**Contract/dump delta:** Only the contract and stable-dump changes explicitly named by this task are permitted.

**Work:**

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
- `i8x16`/`u8x16` dot-accumulate into `i32x4`, lowering exactly to `SDOT`/`UDOT`;
- widening multiply-accumulate, horizontal reductions, mask-select, fixed transpose/zip/unzip, and checked narrowing families required by §6.16.

Amend the normative machine baseline to name `FEAT_DotProd`; there is still one emitted image and no runtime feature test or fallback. Every operation has sema, FlowWir, MachineWir, A64 encoding, a specific cost rule, diff-eval, and emitted-word tests. Do not add general arbitrary shuffle if fixed named shuffles suffice. This task permits one independently gated commit per operation family (construction/load-store, arithmetic, compare/select, lane/shuffle, widen/convert, dot/reduction/idiom closure); each commit must leave the intrinsic and emitted-instruction censuses exact and pass `cargo xtask verify`.

**Tests:**

- SIMD scalar-lane semantics match scalar operations.
- Compiler refuses vector operations in ISR as existing float rule requires.
- No dev/release arithmetic divergence.
- Generated code uses NEON instructions, no scalar fallback loop.
- Intrinsic/cost/emitted instruction censuses updated and exact.
- `SDOT`/`UDOT` emitted-word tests prove the positive patterns and near-match tests prove they are not selected when signedness, accumulation width, overflow, or source order disagrees.
- VMM/machine feature validation requires the same `FEAT_DotProd` baseline named by the language chapter.

**Focused checks:** Run the focused tests, fixtures, dumps, and commands named by this task before the repository gate.

**Repository gate:**

```text
cargo xtask diff-eval
cargo xtask verify
```

**Stop conditions:** Stop if an acceptance condition cannot be met without changing a closed architectural decision, weakening a proof/failure boundary, or adding an external Cargo dependency.

**Commits**

```text
pixels P12.3a: lower SIMD construction and memory operations
pixels P12.3b: lower SIMD arithmetic operations
pixels P12.3c: lower SIMD comparison and selection
pixels P12.3d: lower SIMD lanes and fixed shuffles
pixels P12.3e: lower SIMD widening and conversion
pixels P12.3f: close machine-v1 dot and reduction idioms
```

## Task P12.4 — packetize proof predicates and shading/raster kernels

**Requires:** P12.3.

**Produces:** Use SIMD where lanes share one operation and keep divergent algorithms scalar/batched explicitly.

**Files:**

```text
stdlib/core/render_interval.wr # new at P-1 basis
stdlib/core/render_events.wr # new at P-1 basis
stdlib/core/render_raster.wr # new at P-1 basis
stdlib/core/render_material.wr # new at P-1 basis
stdlib/core/render_light.wr # new at P-1 basis
stdlib/core/render_transfer.wr # new at P-1 basis
```

**Contract/dump delta:** Only the contract and stable-dump changes explicitly named by this task are permitted.

**Work:**

Required packet kernels:

- four q recurrence pixels;
- four normal/position/shading pixels;
- four transfer compositions or four pixels through one layer;
- eight/suitable adjacent q certificate comparisons using i32 lanes where packed;
- four event predicate evaluations with same program;
- four interval endpoint affine/polynomial evaluations when exponents align;
- coefficient-face sign scans and generated composition arithmetic using
  widening integer lanes only where the exact outward-rounding/data-dependency
  contract permits;
- four texture taps/BRDF evaluations.

For each kernel, record its §6.16 ISA obligations. Byte-weight/filter and other exact packed 8-bit accumulations use `SDOT`/`UDOT`; f32 SH/BRDF/color work uses `FMLA` and retains the specified accumulation order; fixed-point accumulations use the appropriate widening MLA family. Layout conversion uses fixed zip/unzip/transpose or structure loads only when its proof obligations hold.

Do not force packetization across divergent root-isolation stacks. Batch independent same-kind candidate cells only when they already exist; scalar fixed-stack algorithm remains semantic implementation.

Do not lower proof-coefficient composition through f32 `FMLA`, `SDOT`, or
`UDOT`. Those families are legal only for operand semantics already named by
§6.16; Bernstein verifier schedules retain checked widening integer
arithmetic and outward rounding.

Use SoA for vectors/points/colors. Convert AoS records to SoA once at run setup, not inside pixel loop.

**Tests:**

- Every packet kernel has scalar differential test and manifest mapping.
- Every packet kernel has a complete `PixelsIsaObligation` row, including an explicit “not applicable” reason for instruction families whose operand semantics do not match.
- Packet use does not change subdivision/root/event decisions.
- No lane masking hides unresolved lane; collect any failure and process lane scalarly/rebuild.
- SoA conversion work counted/reported.
- Hot loops contain no dynamic allocation or generic trait dispatch.

**Focused checks:** Run the focused tests, fixtures, dumps, and commands named by this task before the repository gate.

**Repository gate:**

```text
cargo xtask verify
cargo xtask diff-eval
```

**Stop conditions:** Stop if an acceptance condition cannot be met without changing a closed architectural decision, weakening a proof/failure boundary, or adding an external Cargo dependency.

**Commit**

```text
pixels P12.4: packetize uniform renderer kernels
```

## Task P12.5 — add renderer-specific codegen conventions and hot-function assertions

**Requires:** P12.4.

**Produces:** Keep recurrence/packet state in registers and make spills/frames visible as build facts.

**Files:**

```text
crates/wrela-compiler/src/regalloc.rs
crates/wrela-compiler/src/frame_plan.rs
crates/wrela-compiler/src/frame_color.rs
crates/wrela-compiler/src/codegen.rs
crates/wrela-compiler/src/report.rs
crates/wrela-compiler/src/pixels/glue.rs # new at P-1 basis
```

**Contract/dump delta:** Only the contract and stable-dump changes explicitly named by this task are permitted.

**Work:**

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
- each `PixelsIsaObligation` is satisfied by decoded emitted words; generic `CostRule::Neon` is insufficient evidence.

If allocator cannot meet an assertion, fix live range/code shape; do not delete assertion or claim register residence.

**Tests:**

- Report names all generated hot functions and assembly facts.
- Fixed-q loop is frameless/call-free and q state register resident.
- Shading loop spill count is explicit; no false claim.
- Assertions inspect decoded MachineWir/emitted instructions robustly, not text substring alone where possible.
- Existing convention tests remain green.
- The missed-idiom audit fails on intentionally scalarized dot, horizontal-reduction, FMLA, narrowing, transpose, and paired-load fixtures.

**Focused checks:** Run the focused tests, fixtures, dumps, and commands named by this task before the repository gate.

**Repository gate:**

```text
cargo xtask verify
```

**Stop conditions:** Stop if an acceptance condition cannot be met without changing a closed architectural decision, weakening a proof/failure boundary, or adding an external Cargo dependency.

**Commit**

```text
pixels P12.5: lock renderer hot-loop code shape
```

## Task P12.6 — add Pixels cost dimensions and instruction weights

**Requires:** P12.5.

**Produces:** Score real emitted renderer code and memory traffic through the existing A76 model.

**Files:**

```text
crates/wrela-compiler/src/cost/rule.rs
crates/wrela-compiler/src/cost/oracles.rs
bench/a76-pi5.toml
bench/thresholds.toml
tests/census.toml
```

**Contract/dump delta:** Only the contract and stable-dump changes explicitly named by this task are permitted.

**Work:**

Add/split cost dimensions needed by actual emitted instructions:

- ASIMD f32 add/mul/FMA/min/max;
- ASIMD i32 add/compare/select;
- `SDOT`/`UDOT`, widening MLA, horizontal reductions, fixed permutes, narrowing/packing, and reciprocal-estimate/refinement families as separate cost rows;
- widening multiply;
- scalar/vector reciprocal/rsqrt Newton sequences as their actual words;
- square root if still emitted;
- table/texture loads;
- vector stores;
- branch/match dispatch;
- L1 load/store bytes and store-data/V-pipe contention dimensions already modeled by existing framework;
- cache-line/framebuffer write traffic;
- display descriptor/doorbell.

Source provisional weights from the existing A76/SOG inventory discipline. Where the current proxy lacks an exact renderer-relevant rule, add an explicit conservative range dimension and list the missing abstract-machine fact for P13.2; never defer closure to a hardware counter and never choose the optimistic endpoint for admission.

Split the generic `CostRule::Neon` catch-all until every renderer-emitted instruction family has a specific proxy row and port/resource identity. Update the dense row inventory and rule census. Every emitted renderer word must map to exactly the intended dimension set.

**Tests:**

- Cost dimension inventory remains dense and fully claimed.
- No renderer instruction has unknown/zero accidental cost.
- No renderer instruction is admitted under a generic NEON bucket.
- Vector and scalar paths score actual emitted words, not FLOP estimates.
- Memory refs classify stack/static/frameprog/pixelsdata/framebuffer/probe distinctly where model supports.
- Conservative endpoint used for build admission.

**Focused checks:** Run the focused tests, fixtures, dumps, and commands named by this task before the repository gate.

**Repository gate:**

```text
cargo xtask verify
```

**Stop conditions:** Stop if an acceptance condition cannot be met without changing a closed architectural decision, weakening a proof/failure boundary, or adding an external Cargo dependency.

**Commit**

```text
pixels P12.6: cost the emitted A76 renderer instructions
```

## Task P12.7 — attach sealed renderer workloads to generated functions

**Requires:** P12.6.

**Produces:** Turn frame-program capacities/structure into exact bounded loop counts for cost and deadlines.

**Files:**

```text
crates/wrela-compiler/src/pixels/workload.rs # new at P-1 basis
crates/wrela-compiler/src/cost/workload.rs
crates/wrela-compiler/src/report.rs
```

**Contract/dump delta:** Only the contract and stable-dump changes explicitly named by this task are permitted.

**Work:**

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

Join these exact structural counts to the versioned P7.9/P8 certificate
telemetry for diagnosis: run-length bins, proof methods/shapes, active
predicate counts, expiry owners, subdivision depths, event-corridor fractions,
and bounded-rebuild reasons. The join must account every observed unit of work
to one generated function. Histograms are never multiplied into admission
workloads and may not replace structural maxima.

For release admission, use the worst valid presented-frame path excluding first-frame probe initialization if the application can perform initialization before entering steady display deadline; report/init deadline separately. Camera cut/full sweep is included. The absolute pathological `CertificateExhausted` path may return error before completing a frame and is reported as failure bound, not admitted success.

Attach counts to exact generated function keys/loops. Do not multiply unrelated averages. Per-core work uses actual tile/probe partition maxima.

**Tests:**

- Workload report traces every count to frame-program table/capacity.
- Full sweep includes no kinetic discount.
- Single/four-core partition sums and max-core values exact.
- Generated hot function missing workload is build error.
- Report separates initialization, successful full sweep, kinetic valid, and failure/rebuild upper bounds.
- Every telemetry bin maps to a charged function/path or an explicitly
  zero-cost diagnostic event, with no unowned certificate work.

**Focused checks:** Run the focused tests, fixtures, dumps, and commands named by this task before the repository gate.

**Repository gate:**

```text
cargo xtask verify
```

**Stop conditions:** Stop if an acceptance condition cannot be met without changing a closed architectural decision, weakening a proof/failure boundary, or adding an external Cargo dependency.

**Commit**

```text
pixels P12.7: attach sealed frame workloads to cost
```

## Task P12.8 — enforce renderer deadline and memory admission

**Requires:** P12.7.

**Produces:** Make the declared 1080p60 profile a compiler proof obligation, not a runtime wish.

**Files:**

```text
crates/wrela-compiler/src/pixels/admission.rs # new at P-1 basis
crates/wrela-compiler/src/cost/mod.rs
crates/wrela-compiler/src/report.rs
bench/thresholds.toml
```

**Contract/dump delta:** Only the contract and stable-dump changes explicitly named by this task are permitted.

**Work:**

For `RenderProfile.AaaByteExact`:

- compute per-core conservative proxy cycles for successful full-sweep frame at declared mode/refresh;
- include scheduler/orchestration/display submission and memory traffic;
- compare against frame-period budget at configured/pinned flagship clock using conservative cost endpoint and existing core load/placement;
- reserve explicit headroom factor in `bench/thresholds.toml`; set v1 admission to at most 80% of modeled per-core frame budget, leaving 20% for model error/interrupt/display variance;
- check the sealed guest layout, including image/runtime/framebuffer/probe state and explicit stack/failure reserve, fits the machine-v1 1 GiB guest address-space profile;
- check first-frame initialization against separate renderer initialization deadline (default 2 seconds, source-configurable only downward in flagship profile);
- include the all-invalid dynamic-probe workload (`probe_count × 32` complete secondary rays plus accumulation) in initialization and frame admission; reject or require static/disabled probes when it cannot fit;
- refuse image with a detailed cost why-chain if any fails.

Until P13 closes every renderer-relevant range into the exact sealed cycle proxy, `AaaByteExact` remains buildable only under an explicit repository-internal `pixels_unlocked` feature for implementation fixtures. P13 removes that escape hatch before activation. The escape hatch is not source syntax and cannot ship release images.

**Tests:**

- Admission uses full from-scratch path, not typical kinetic frame.
- Over-budget fixture fails with per-core term breakdown.
- Memory/init-deadline fixtures fail correctly.
- Report prints budget, modeled range, conservative endpoint, headroom, and provenance.
- No threshold auto-adjust/update command in ordinary build.

**Focused checks:** Run the focused tests, fixtures, dumps, and commands named by this task before the repository gate.

**Repository gate:**

```text
cargo xtask verify
```

**Stop conditions:** Stop if an acceptance condition cannot be met without changing a closed architectural decision, weakening a proof/failure boundary, or adding an external Cargo dependency.

**Commit**

```text
pixels P12.8: admit renderer images against full-sweep budgets
```

## Task P12.9 — archive Cortex-A76 assembly and ISA-selection evidence

**Requires:** P12.8.

**Produces:** Make target code shape reviewable and pinned before exact cycle-proxy conformance.

**Files:**

```text
crates/xtask/src/pixels_asm.rs # new at P-1 basis
tests/golden/pixels-asm/ # new at P-1 basis
bench/pixels-a76.md # new at P-1 basis
```

**Contract/dump delta:** Only the contract and stable-dump changes explicitly named by this task are permitted.

**Work:**

Add `cargo xtask pixels-asm` and `cargo xtask pixels-isa-audit`. They build the locked acceptance renderer for the one machine-v1 target and emit normalized assembly/MachineWir summaries for:

- coefficient evaluator;
- interval/q-order predicates;
- fixed-q raster loop;
- normal/BRDF packet loop;
- texture filter loop;
- transfer composition;
- event/root isolation inner loops;
- probe SH evaluation;
- display descriptor path.

The ISA audit joins generated `PixelsIsaObligation` rows to decoded word ranges. It reports selected family, expected family, source semantic preconditions, and any forbidden scalarized sequence. It is not a text grep.

Normalize addresses, local labels, and build paths while preserving instruction sequence, registers, stack offsets, and branch structure. Check in compact summaries plus full assembly artifact under a generated ignored/artifact path if repository policy permits; golden only load-bearing loops.

**Tests:**

- AArch64 target code actually builds.
- Golden asserts no calls/stack ops in fixed-q hot loop.
- Instruction/cost report counts agree with assembly decoder.
- Every renderer workload obligation is satisfied, including `SDOT`/`UDOT` for qualifying byte-dot kernels and FMLA/widen/reduction/load-store idioms elsewhere.
- Negative selection fixtures retain the longer correct sequence.
- Assembly evidence is input to the cycle proxy but is not itself a physical timing claim.
- Changing a load/store/spill produces reviewed golden diff.

**Focused checks:** Run the focused tests, fixtures, dumps, and commands named by this task before the repository gate.

**Repository gate:**

```text
cargo xtask pixels-asm --check
cargo xtask pixels-isa-audit
cargo xtask verify
```

**Stop conditions:** Stop if an acceptance condition cannot be met without changing a closed architectural decision, weakening a proof/failure boundary, or adding an external Cargo dependency.

**Commit**

```text
pixels P12.9: pin Cortex-A76 renderer assembly shape
```

## Task P12.10 — close backend differential and cost reports

**Requires:** P12.9.

**Produces:** Prove optimized release code preserves all scalar/formal decisions and is fully accounted.

**Files:**

```text
crates/xtask/src/pixels_conformance.rs # new at P-1 basis
crates/wrela-compiler/src/report.rs
tests/golden/check-pixels-*/expected/report.txt # new at P-1 basis
```

**Contract/dump delta:** Only the contract and stable-dump changes explicitly named by this task are permitted.

**Work:**

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
  proxy_cycles_low/high  # provisional until P13 seals exact proxy_cycles
  isa_obligations=satisfied/total
  selected_sequence_ids
  bytes_read/written by memory class
  dispatches
  spills
  headroom

CertificateTelemetry
  schema_revision
  density_class
  run_length_bins
  proof_method_and_shape_counts
  active_predicate_bins
  expiry_and_margin_owner_counts
  subdivision_and_rebuild_counts
  regular_and_corridor_pixels
```

**Tests:**

- All build variants byte/error equivalent.
- Every renderer cost row has provenance and nonzero workload where used.
- Report labels cycle-proxy values as deterministic model output, never empirical measurement.
- Admission result computed from report data, not handwritten verdict.
- Versioned distributional non-regression bounds are checked for the locked P8
  adversarial scenes and report their failures separately. A passing
  distribution may never relax a failing exact per-frame workload/cycle
  admission result.
- All permanent reports pinned.

**Focused checks:** Run the focused tests, fixtures, dumps, and commands named by this task before the repository gate.

**Repository gate:**

```text
cargo xtask pixels-conformance --all-build-modes
cargo xtask verify
```

**Stop conditions:** Stop if an acceptance condition cannot be met without changing a closed architectural decision, weakening a proof/failure boundary, or adding an external Cargo dependency.

**Commit**

```text
pixels P12.10: lock renderer backend and cost equivalence
```

### Milestone P12 close

Run `cargo xtask verify`. The renderer must now execute as real emitted AArch64/NEON code, remain byte-equivalent across all optimized/scalar modes, and pass conservative compiler cost/memory admission under the temporary implementation unlock.

---

# Milestone P13 — exact cycle-proxy conformance, language activation, and release closure

Milestone result: the temporary Pixels implementation unlock is removed. Every renderer-relevant emitted word, dependency, pipeline/resource use, memory-class transition, branch path, and device charge is accounted by one exact versioned A76 cycle proxy. Every acceptance frame meets its deadline in that proxy with sealed headroom; ISA obligations prove the strongest correct machine-v1 instructions were selected; all formal and conformance gates are green; and `07-pixels.md` is active normative behavior. Physical hardware runs, counters, wall time, temperature, and board-specific observations are excluded from conformance.

## Task P13.1 — create the Wrela acceptance images

**Requires:** the preceding milestone close gate.

**Produces:** Exercise the complete production renderer with fixed authored scenes rather than fieldprobe.

**Files:**

```text
examples/pixels_colonnade/ # new at P-1 basis
examples/pixels_melee/ # new at P-1 basis
examples/pixels_quality/ # new at P-1 basis
examples/pixels_acceptance/ # new at P-1 basis
tests/golden/check-pixels-acceptance/ # new at P-1 basis
bench/pixels-acceptance.toml # new at P-1 basis
```

**Contract/dump delta:** Only the contract and stable-dump changes explicitly named by this task are permitted.

**Work:**

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
event_density_sequence: grazing, blend clutter, close q order, and mixed-scale
  near/far controls with kinetic disabled
```

All scripts use exact frame-indexed coefficients, no wall clock or input device. Expected frame digests are checked in for selected frames and rolling sequence digest.

`bench/pixels-acceptance.toml` is not a tunable benchmark file; it pins mode, frame count, scene path, expected digest, and hard conformance thresholds.

It also pins versioned certificate-telemetry non-regression ranges for each
density class. These ranges catch a collapse in run amortization or predicate
pruning on real scenes, but they never average away or override one
over-budget exact cycle-proxy frame.

**Tests:**

- Images compile with implementation unlock and pass host/VMM conformance.
- Scenes use ordinary public `@field`/`@material`/`Image.renderer` source.
- No fieldprobe crate/source imported.
- Frame scripts deterministic under replay.
- Acceptance report contains complete capacities, ISA obligations, and cycle-proxy inputs.
- Every density class emits complete telemetry with no unknown method, owner,
  expiry, subdivision, or rebuild ID.

**Focused checks:** Run the focused tests, fixtures, dumps, and commands named by this task before the repository gate.

**Repository gate:**

```text
cargo xtask verify
cargo xtask pixels-conformance --acceptance
```

**Stop conditions:** Stop if an acceptance condition cannot be met without changing a closed architectural decision, weakening a proof/failure boundary, or adding an external Cargo dependency.

**Commit**

```text
pixels P13.1: add production renderer acceptance images
```

## Task P13.2 — extend and seal the exact A76 renderer cycle proxy

**Requires:** P13.1.

**Produces:** Turn the current differential proxy into the exact normative cycle machine used for Pixels admission. This is model execution over emitted code and sealed workloads, never a physical-hardware measurement.

**Files:**

```text
crates/wrela-compiler/src/cost/mod.rs
crates/wrela-compiler/src/cost/oracles.rs
crates/wrela-compiler/src/cost/mem.rs
crates/wrela-compiler/src/cost/footprint.rs
crates/wrela-compiler/src/pixels/workload.rs # new at P-1 basis
crates/xtask/src/pixels_cycle_proxy.rs                 # new at P-1 basis
bench/a76-pi5.toml
bench/pixels-cycle-proxy-lock.toml                    # new at P-1 basis
docs/designs/pixels-a76-cycle-proxy.md                # new at P-1 basis
docs/language/04-compiler.md
docs/language/06-machine.md
```

**Contract/dump delta:** Only the contract and stable-dump changes explicitly named by this task are permitted.

**Work:**

Define a versioned deterministic proxy state machine over final emitted words and exact workload paths:

- decoder/dispatch width, dependency scoreboard, instruction latency, reciprocal throughput, execution-port occupancy, and store-data/vector-pipe contention;
- exact instruction families, including `SDOT`/`UDOT`, FMLA, widening MLA, horizontal reductions, permutes, narrowing, reciprocal steps, branches, barriers, and paired/structure memory operations;
- finite reorder window and structural branch path selected by the acceptance script/certificate outcome;
- per-core L1I/I-TLB and hot-text placement;
- data cache/TLB state with sealed line/page geometry, associativity, replacement rule, initial state, and exact addresses from image placement;
- explicit memory classes for stack, frame program, renderer state, framebuffer, textures, probes, queue/device pages, and immutable tables;
- actor handoff, cross-core wake, queue publication, doorbell, and display-submission charges as versioned machine-profile transitions;
- exact loop/traversal counts from `RendererWorkload`, never unrelated averages.

The acceptance runner interprets the final decoded AArch64 renderer/device path in lockstep with this proxy. Branches, addresses, dependencies, and cache/TLB transitions therefore come from the words being scored, not an instrumented substitute, source guess, or host execution. Compiler admission separately feeds the same proxy the sealed worst-success-path counts from `RendererWorkload`; those counts are conservative exact integers and name every block they multiply.

Every transition advances one integer `proxy_cycle`; the same emitted image, frame input, initial proxy state, and proxy revision must yield the same cycle total and trace digest on every host. Published-record provenance may define a transition constant; physical runs, counters, PGO, wall clocks, and fitted residuals may not. Amend the compiler and machine chapters so this exact renderer proxy is the normative admission ruler for `AaaByteExact`; retain clear wording that it models the sealed A76 target profile and is not observed elapsed time.

If the existing proxy cannot represent a renderer interaction precisely, extend its state/transition model before assigning a number. No generic NEON bucket, unresolved range, “typical cache” assumption, or manual per-scene correction survives this task.

If an exact transition constant or state rule cannot be justified from the repository’s allowed published-record provenance, stop profile activation at this task. Do not infer it from a physical run and do not choose a convenient point from a range.

Add:

```text
cargo xtask pixels-cycle-proxy --acceptance
```

It emits per-frame/per-core exact totals, critical dependency/resource path, memory transitions, ISA-obligation results, trace digest, proxy revision, image digest, and workload digest.

**Tests:**

- Every renderer-emitted opcode and machine/device transition has exactly one applicable proxy rule.
- Replaying a stored trace recomputes the same integer cycles and digest.
- Final-word interpretation and workload-bound mode agree on hand-constructed paths where the bound is exact; neither scores an instrumented binary.
- Tiny hand-scheduled dependency/port/cache examples have exact checked totals.
- Altering one emitted word, address class, loop count, ISA idiom, or proxy rule changes the appropriate trace digest/report.
- No code path reads host CPU identity, performance counters, temperature, frequency, or wall time.

**Focused checks:** Run the focused tests, fixtures, dumps, and commands named by this task before the repository gate.

**Repository gate:**

```text
cargo xtask pixels-cycle-proxy --self-test
cargo xtask verify
```

**Stop conditions:** Stop if an acceptance condition cannot be met without changing a closed architectural decision, weakening a proof/failure boundary, or adding an external Cargo dependency.

**Commit**

```text
pixels P13.2: seal the exact renderer cycle proxy
```

## Task P13.3 — close ISA selection and cycle-proxy coverage

**Requires:** P13.2.

**Produces:** Prove that every acceptance workload uses the correct instruction family for the one machine-v1 ISA and that every dynamic unit of work is charged exactly once.

**Files:**

```text
crates/wrela-compiler/src/pixels/isa.rs              # new at P-1 basis
crates/wrela-compiler/src/pixels/workload.rs # new at P-1 basis
crates/wrela-compiler/src/cost/oracles.rs
crates/wrela-compiler/src/lower.rs
crates/wrela-compiler/src/codegen.rs
crates/wrela-compiler/src/encode.rs
crates/xtask/src/pixels_asm.rs # new at P-1 basis
crates/xtask/src/pixels_cycle_proxy.rs # new at P-1 basis
bench/pixels-cycle-proxy-lock.toml # new at P-1 basis
tests/golden/pixels-asm/ # new at P-1 basis
tests/census.toml
```

**Contract/dump delta:** Only the contract and stable-dump changes explicitly named by this task are permitted.

**Work:**

Join four censuses by stable function/block/loop ID:

```text
semantic workload
  -> PixelsIsaObligation
  -> decoded emitted word range
  -> exact proxy transitions × exact workload frequency
```

Require equal complete coverage at every edge. Audit all §6.16 idioms, including `SDOT`/`UDOT` for qualifying packed byte dots, FMLA for f32 dot/matrix/SH rows, widening MLA for fixed accumulations, reductions, permutes, narrowing, reciprocal sequences, and proved paired/structure memory operations. Reject both missed idioms and illegal over-eager idioms.

Remove all remaining proxy ranges/residual boxes from renderer-reachable rows. Rows still uncertain in the generic compiler may remain for non-Pixels ranking, but `AaaByteExact` may reference only exact sealed rows.

**Tests:**

- ISA, emitted-word, proxy-rule, and workload-frequency coverage are each 100% and use the same denominators.
- No qualifying dot workload expands to scalar multiply/add or generic lane extraction.
- No nonqualifying f32/overflow-sensitive workload is forced through `SDOT`/`UDOT`.
- Renderer reports contain one exact `proxy_cycles` value per path/core, not low/high endpoints.
- Any missing workload/opcode/rule/idiom fails closed with its stable ID and source kernel.

**Focused checks:** Run the focused tests, fixtures, dumps, and commands named by this task before the repository gate.

**Repository gate:**

```text
cargo xtask pixels-isa-audit
cargo xtask pixels-cycle-proxy --coverage
cargo xtask verify
```

**Stop conditions:** Stop if an acceptance condition cannot be met without changing a closed architectural decision, weakening a proof/failure boundary, or adding an external Cargo dependency.

**Commit**

```text
pixels P13.3: close renderer ISA and proxy coverage
```

## Task P13.4 — pass the exact full-sweep 1080p60 proxy gate

**Requires:** P13.3.

**Produces:** Prove temporal maintenance is not hiding a full-sweep path that exceeds the flagship cycle budget after cuts or whips.

**Files:**

```text
bench/pixels-cycle-proxy-lock.toml # new at P-1 basis
docs/designs/pixels-a76-cycle-proxy.md # new at P-1 basis
tests/golden/check-pixels-acceptance/expected/report.txt # new at P-1 basis
```

**Contract/dump delta:** Only the contract and stable-dump changes explicitly named by this task are permitted.

**Work:**

Run the exact proxy with kinetic disabled and probes initialized:

```text
scene colonnade-flat: 3,600 frame inputs
scene colonnade:      3,600 frame inputs
scene melee:          3,600 frame inputs
```

Each script forces valid camera/animation changes every frame and includes cuts/whip segments that require full sweep. For every frame and core:

- `proxy_cycles <= 80%` of the cycle budget derived from the sealed target clock and 60 Hz frame period;
- zero `RenderError`;
- exact expected frame and rolling image digest;
- exact proxy trace digest;
- guest memory within the sealed 1 GiB guest profile;
- complete ISA/workload/proxy coverage.

Do not average frames, use p95, or subtract “idle” work. One over-budget frame fails. If this task fails, fix implementation, instruction selection, code shape, or exact workload bounds while preserving prior correctness and quality contracts. Do not enable kinetic mode, lower resolution/refresh, loosen output proof, or edit the script to hide work.

**Tests:**

- All frames in all three sequences meet every hard criterion.
- The lock records the maximum frame/core total and its full critical-path/transition breakdown.
- Compiler admission reproduces the xtask total and passes without unlock.
- Re-running on another host produces identical totals and trace digests.

**Focused checks:** Run the focused tests, fixtures, dumps, and commands named by this task before the repository gate.

**Repository gate:**

```text
cargo xtask pixels-cycle-proxy --full-sweep
cargo xtask verify
```

**Stop conditions:** Stop if an acceptance condition cannot be met without changing a closed architectural decision, weakening a proof/failure boundary, or adding an external Cargo dependency.

**Commit**

```text
pixels P13.4: lock exact full-sweep 1080p60 proxy conformance
```

## Task P13.5 — pass complete AAA sequence and bounded-state proxy conformance

**Requires:** P13.4.

**Produces:** Validate quality, transparency, GI, kinetic maintenance, initialization, and long-sequence state bounds without physical timing or thermal claims.

**Files:**

```text
bench/pixels-cycle-proxy-lock.toml # new at P-1 basis
docs/designs/pixels-a76-cycle-proxy.md # new at P-1 basis
```

**Contract/dump delta:** Only the contract and stable-dump changes explicitly named by this task are permitted.

**Work:**

Execute the deterministic 108,000-frame input sequence through the renderer conformance lane and exact cycle proxy, cycling:

- static reuse;
- kinetic melee motion;
- camera orbit/whip/cuts;
- textured glossy quality scene;
- area-light penumbrae;
- transparency;
- moving object/light probe invalidation, including the all-invalid probe frame;
- `shade_hz` transport;
- first-frame initialization as a separate deadline class.

Hard criteria:

- every presented frame and initialization path stays below its exact proxy budget with sealed headroom;
- zero render/display/probe/capacity errors;
- exact rolling/selected-frame image digests and per-class proxy trace digests;
- zero replay or scalar/SIMD divergence;
- no unbounded guest-state growth; every queue, generation counter, cache, and persistent kinetic/probe structure remains inside its sealed capacity;
- complete ISA/workload/proxy coverage on every distinct executed block.

Run the sequence three times from the same canonical initial state. Totals and digests must be identical; there is no cold-reboot, temperature, frequency, counter, RSS, or wall-clock input.

**Tests:**

- Three identical green model runs.
- No performance/quality fallback mode activated.
- Kinetic-disabled selected comparison sequence remains byte-identical.
- Lock includes the worst exact frame and initialization path, not average/p95.

**Focused checks:** Run the focused tests, fixtures, dumps, and commands named by this task before the repository gate.

**Repository gate:**

```text
cargo xtask pixels-cycle-proxy --complete-sequence --repeat 3
cargo xtask verify
```

**Stop conditions:** Stop if an acceptance condition cannot be met without changing a closed architectural decision, weakening a proof/failure boundary, or adding an external Cargo dependency.

**Commit**

```text
pixels P13.5: lock complete AAA cycle-proxy conformance
```

## Task P13.6 — remove the implementation unlock and activate profile admission

**Requires:** P13.5.

**Produces:** Prevent unaccounted, incorrectly selected, or over-budget flagship renderer images from shipping.

**Files:**

```text
crates/wrela-compiler/src/pixels/admission.rs # new at P-1 basis
crates/wrela-compiler/src/bin/wrela.rs
crates/wrela-compiler/Cargo.toml
bench/thresholds.toml
tests/golden/err-pixels-cost/ # new at P-1 basis
```

**Contract/dump delta:** Only the contract and stable-dump changes explicitly named by this task are permitted.

**Work:**

- Delete `pixels_unlocked` feature/env/internal bypass.
- `RenderProfile.AaaByteExact` always runs full cost/memory/formal-revision admission.
- Admission requires an exact cycle-proxy revision and 100% ISA/workload/proxy coverage; a range-valued or generic-NEON renderer row is rejection.
- Test fixtures that intentionally exceed budgets use rejected goldens, not bypass.
- Development can use smaller output/configurations that pass admission; there is no “ignore cost” source flag.
- Add explicit compiler error if ISA-obligation, cycle-proxy, workload, or lock revision is missing or mismatched.

**Tests:**

- Acceptance images build normally.
- Over-budget images fail.
- Grep/census finds no unlock/bypass.
- Report verdict and raw facts agree.
- Nonrenderer builds unaffected.

**Focused checks:** Run the focused tests, fixtures, dumps, and commands named by this task before the repository gate.

**Repository gate:**

```text
cargo xtask verify
```

**Stop conditions:** Stop if an acceptance condition cannot be met without changing a closed architectural decision, weakening a proof/failure boundary, or adding an external Cargo dependency.

**Commit**

```text
pixels P13.6: activate sealed renderer admission
```

## Task P13.7 — run complete formal, differential, and replay closure

**Requires:** P13.6.

**Produces:** Produce one final green trust-chain result after ISA and exact cycle-proxy closure.

**Files:**

```text
formal/pixels/EXPECTED_AXIOMS.txt # new at P-1 basis
formal/pixels/KERNELS.txt # new at P-1 basis
tests/golden/pixels-asm/ # new at P-1 basis
```

**Contract/dump delta:** Only the contract and stable-dump changes explicitly named by this task are permitted.

**Work:**

Run and pin:

```text
cargo xtask verify
cargo xtask pixels-formal
cargo xtask pixels-repro
cargo xtask pixels-conformance --all
cargo xtask pixels-conformance --all-build-modes
cargo xtask pixels-conformance --kinetic-diff
cargo xtask pixels-asm --check
cargo xtask pixels-isa-audit
cargo xtask pixels-cycle-proxy --complete-sequence
```

Fuzzing remains the repository’s separate discovery lane, invoked only as `cargo xtask fuzz all`; it is not a substitute for or constituent of this task gate. Promote every prior fuzz/differential finding to a permanent focused test before fixing. Update no golden blindly.

**Tests:**

- All commands green from clean checkout/toolchains.
- No admissions/unexpected axioms.
- No semantic/output divergence.
- No frame-program decoder crash.
- Assembly/cost/report/reproduction goldens stable.

**Focused checks:** Run the focused tests, fixtures, dumps, and commands named by this task before the repository gate.

**Repository gate:**

The command set above.

**Stop conditions:** Stop if an acceptance condition cannot be met without changing a closed architectural decision, weakening a proof/failure boundary, or adding an external Cargo dependency.

**Commit**

```text
pixels P13.7: close the renderer trust chain
```

## Task P13.8 — finalize normative specs and decision records

**Requires:** P13.7.

**Produces:** Make repository documentation describe the shipped renderer rather than the abandoned sample-first design.

**Files:**

```text
docs/language/07-pixels.md # new at P-1 basis
docs/language/04-compiler.md
docs/language/05-library.md
docs/language/06-machine.md
docs/designs/pixels.md
docs/designs/pixels-a76-cycle-proxy.md # new at P-1 basis
README.md                                      # new at P-1 basis
```

**Contract/dump delta:** Only the contract and stable-dump changes explicitly named by this task are permitted.

**Work:**

- Remove implementation-status wording from `07-pixels.md`; mark normative revision.
- State exact supported field/material/profile subset and build errors.
- Document `FrameProgram v1`, memory sections, actors, display format, replay, numeric/formal revisions.
- Document from-scratch sweep, kinetic equivalence, AAA model, and failure semantics.
- Preserve fieldprobe documents as historical evidence that rejected the old online baseline; add a clear supersession note pointing to production plan/results.
- Document the exact proxy as a normative deterministic admission machine and state plainly that it is not physical timing or empirical calibration.
- Document the one machine-v1 ISA and the workload-to-instruction obligations, including the exact scope of `SDOT`/`UDOT`.
- Do not claim universal rendering or full physical GI.
- Add source tutorial using public API and explain common diagnostics/cost report.

**Tests:**

- No active doc tells implementers to run a spike before FieldWir/Pixels work.
- No contradiction between source API/spec/compiler implementation.
- Cycle-proxy facts cite the locked revision, ISA census, emitted-image digest, workload digest, and trace digest.
- Unsupported features listed plainly.
- All doc links/goldens pass.

**Focused checks:** Run the focused tests, fixtures, dumps, and commands named by this task before the repository gate.

**Repository gate:**

```text
cargo xtask verify
```

**Stop conditions:** Stop if an acceptance condition cannot be met without changing a closed architectural decision, weakening a proof/failure boundary, or adding an external Cargo dependency.

**Commit**

```text
pixels P13.8: activate the production Pixels specification
```

## Task P13.9 — add the release conformance command

**Requires:** P13.8.

**Produces:** Give maintainers one command that states whether Pixels may ship.

**Files:**

```text
crates/xtask/src/main.rs
crates/xtask/src/pixels_release.rs # new at P-1 basis
bench/pixels-cycle-proxy-lock.toml # new at P-1 basis
```

**Contract/dump delta:** Only the contract and stable-dump changes explicitly named by this task are permitted.

**Work:**

Add:

```text
cargo xtask pixels-release-check
```

Host portion runs:

- verify-deep;
- formal;
- reproduction;
- complete conformance/differential;
- assembly check;
- ISA-obligation and missed-idiom audit;
- exact cycle-proxy self-test, coverage, full-sweep, and complete-sequence locks;
- report/admission check;
- validates checked-in cycle-proxy lock revisions/digests and commit compatibility.

The release verdict is host-independent and has no `--hardware` mode. Ordinary VMM boot may validate that the host implements machine-v1, but no host feature value, counter, frequency, temperature, or wall time enters the cycle-proxy lock or release verdict.

Output ends with one computed verdict:

```text
PixelsRelease revision=<...> PASS=true
```

No handwritten PASS line. Every constituent fact is printed before verdict.

**Tests:**

- Missing/stale formal/ISA/cycle-proxy lock produces PASS=false.
- Command never updates locks/goldens.
- Verdict computed from result structs.
- Repository release checklist invokes it.

**Focused checks:** Run the focused tests, fixtures, dumps, and commands named by this task before the repository gate.

**Repository gate:**

```text
cargo xtask pixels-release-check
cargo xtask verify
```

**Stop conditions:** Stop if an acceptance condition cannot be met without changing a closed architectural decision, weakening a proof/failure boundary, or adding an external Cargo dependency.

**Commit**

```text
pixels P13.9: add the renderer release gate
```

## Task P13.10 — final repository cleanup and ownership census

**Requires:** P13.9.

**Produces:** Remove transitional code and ensure every renderer surface is accounted for.

**Files:**

```text
crates/wrela-compiler/src/sema/intrinsics.rs
tests/census.toml
crates/xtask/src/census.rs                    # new at P-1 basis
Cargo.toml
AGENTS.md
```

**Contract/dump delta:** Only the contract and stable-dump changes explicitly named by this task are permitted.

**Work:**

- Remove placeholder debug-only branches no longer needed, retaining explicit conformance modes behind generated test builds.
- Census all Pixels intrinsics, dump stages, frame-program tags, event kinds, feature kinds, material/light kinds, cost rules, generated symbol families, formal kernel mappings, and report sections.
- Fail closed when implementation grows without updating the relevant written-down list.
- Confirm no new external Cargo dependencies were added; if a prior task accidentally added one, replace it with local direct code as required by repository policy.
- Add Pixels module ownership/map to `AGENTS.md` without changing repository behavioral rules.

**Tests:**

- Censuses equal producer/consumer sites.
- No TODO/placeholder/unimplemented path reachable in supported profile.
- No implementation unlock.
- No dependency growth.
- Full release check green.

**Focused checks:** Run the focused tests, fixtures, dumps, and commands named by this task before the repository gate.

**Repository gate:**

```text
cargo xtask pixels-release-check
cargo xtask verify
```

**Stop conditions:** Stop if an acceptance condition cannot be met without changing a closed architectural decision, weakening a proof/failure boundary, or adding an external Cargo dependency.

**Commit**

```text
pixels P13.10: close renderer ownership and cleanup
```

### Milestone P13 close

Run `cargo xtask verify`. The milestone closes only after that gate and the full release command are green, the exact cycle-proxy lock is valid for the current renderer/numeric/ISA/cost revisions, and the temporary implementation unlock is gone.

---

## 11. Permanent correctness and conformance matrix

The following matrix is normative. A fixture may gain assertions, but no row may be deleted or merged in a way that stops testing its named failure class.

| fixture / lane | protects | first milestone active | final assertions |
|---|---|---:|---|
| `check-pixels-plane` | projective cancellation; affine inverse depth; full-row regular run | P2 | one plane object/feature; exact q; no false event; final digest |
| `check-pixels-hard-csg` | union/intersection/subtraction occupancy ordering | P3 | complete roots; exact first composite transition; identity |
| `check-pixels-smooth-interior-root` | smooth root without a leaf-root seed | P7 | interior support-budget isolation finds `a=b=k/4`; no missed boundary |
| `check-pixels-smooth-csg` | support-shell completeness; active smooth cluster | P3 | leaf support; unique root runs; normal/material continuity |
| `check-pixels-repeat` | finite instances; wrap event; negative index ordering | P3 | no cross-wrap certificate; exact visible instance |
| `check-pixels-displace` | bounded deformation/Taylor remainder | P3 | no unsafe sphere step; root tube/normal/output containment |
| `check-pixels-close-depth` | overlapping q intervals/order swap | P4 | no ID tie-break; event corridor; exact side winners |
| `check-pixels-thin-feature` | subpixel structural discovery | P3 | feature retained; analytic coverage; no false background |
| `check-pixels-enclosed-feature` | sample-lattice information firewall | P3 | feature found from bound/support despite identical legacy samples |
| `check-pixels-material-edge` | nondepth discontinuity | P3 | material event, same geometry q, exact side bytes |
| `check-pixels-visibility-probe` | guest certificate evidence and oracle separation | P7 | guest hit/identity/q/normal/coverage are decoded and independently scored |
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
| `err-pixels-rate` | temporal proof domain | P1 | `P006` for invalid rate metadata |
| `err-pixels-topology-branch` | fixed topology | P2 | `P003`, both arm shapes shown |
| `err-pixels-repeat-unbounded` | finite instance count | P3 | `P012`, world/period contributors |
| `err-pixels-capacity` | no runtime allocation/overflow | P3 | `P015`, exact why-chain |
| `err-pixels-projective-zero` | positive denominator/q | P4 | `P016` |
| `err-pixels-fixed-q` | representable hot state | P6 | `P017` |
| `err-pixels-tone-table` | monotone byte proof | P9 | `P018` |
| `err-pixels-cost` | full-sweep deadline admission | P12 | modeled range/headroom why-chain |
| `err-pixels-reserved-name` | compiler-reserved surface fence | P7 | `name`, at the referencing token, for a user module spelling a generated intrinsic |
| `boot-pixels-numeric` | Lean/Rust/Wrela scalar correspondence | P6 | exact vector digest |
| `boot-pixels-plane` | full guest/VMM path | P8 | exact visible/raw tile/frame/replay digests |
| `boot-pixels-plane-one-core` | single-worker execution twin | P7 | exact debug frame digest and certificate telemetry equal the four-worker build |
| `boot-pixels-program-view` | checked sealed-table access | P7 | every out-of-range table/record/operand index fails closed |
| `boot-pixels-frame-input` | frame validation and snapshot determinism | P7 | exact validation errors and byte-identical digests across identical frames |
| `boot-pixels-quality` | complete AAA stack | P9 | selected frame and rolling sequence digests |
| `boot-pixels-transparent` | transfer tree/tail | P10 | normative host model and guest digest |
| `boot-pixels-gi` | probe init/update/interpolation | P10 | normative host model and guest digest |
| `boot-pixels-kinetic` | temporal maintenance | P11 | all mode/core/replay comparisons |
| `pixels-asm` | one-ISA A76 hot-loop shape | P12 | instructions/registers/stack/calls/cost counts plus complete ISA obligations (`SDOT`/`UDOT`, FMLA, widening/reduction/permute/narrow/load-store) |
| `pixels-cycle-proxy-lock` | exact target-profile cycle admission | P13 | per-frame/core cycles, ISA coverage, trace/image/workload digests, headroom |

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
  isa.rs                 semantic workload to machine-v1 instruction obligations
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
crates/wrela-machine/src/lib.rs
crates/wrela-vmm/src/display/*
crates/wrela-vmm/src/replay.rs
```

### 12.4 Formal

```text
formal/pixels/
```

Only generic mathematics and theorem-to-kernel manifests live here. Do not copy compiler source or large generated scene facts into Lean.

### 12.5 Tests, examples, and cycle-proxy locks

```text
tests/golden/check-pixels-*
tests/golden/err-pixels-*
tests/golden/boot-pixels-*
tests/golden/pixels-asm/
tests/pixels_truth/
examples/pixels_*/
bench/pixels-acceptance.toml
bench/pixels-cycle-proxy-lock.toml
docs/designs/pixels-a76-cycle-proxy.md
crates/xtask/src/pixels_cycle_proxy.rs
crates/xtask/src/pixels_asm.rs
```

---

## 13. Milestone dependency and invariant ladder

A later milestone may rely only on the invariant published by every earlier closed milestone.

| milestone | invariant after close |
|---|---|
| P-1 | repository paths/contracts are reconciled, the public ABI type-checks, smooth-object/deformation soundness is positive, and the plane-only vertical skeleton presents a locked digest |
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
| P13 | exact A76 cycle-proxy, one-ISA, and full trust-chain conformance activate the profile |

Do not parallelize tasks across an invariant boundary. Worktrees may parallelize independent tests/documentation inside one task only when one owner integrates and runs the exact task gate. If P6 kernel work is split internally, designate one `KERNELS.txt` integrator; no parallel branch edits that manifest independently.

---

## 14. Exact commit order

The expected linear history is the task order in this document. The compact list below is the execution queue. A commit must not combine adjacent tasks just because one is small; stable dump/test boundaries are deliberate.

```text
P-1.1 repository/task-schema reconciliation
P-1.2 public source ABI
P-1.3 display ABI/headless sink
P-1.4 canonical contract lint
P-1.5 smooth-object/deformation soundness
P-1.6 plane-only vertical walking skeleton

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
P5.9  orchestration/bootstrap reachability
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
P7.5  all smooth-object row-start roots
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
P8.8a portable display device model
P8.8b guest display driver
P8.8c headless display replay
P8.8d macOS/HVF presentation
P8.8e Linux/KVM presentation
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
P12.3a SIMD construction/memory
P12.3b SIMD arithmetic
P12.3c SIMD comparison/selection
P12.3d SIMD lanes/fixed shuffles
P12.3e SIMD widening/conversion
P12.3f machine-v1 dot/reduction idioms
P12.4 packet proof/shading kernels
P12.5 hot-loop code-shape assertions
P12.6 A76 cost dimensions
P12.7 renderer workloads
P12.8 deadline/memory admission
P12.9 A76 assembly/ISA artifacts
P12.10 backend/cost equivalence

P13.1 Wrela acceptance images
P13.2 exact A76 renderer cycle proxy
P13.3 ISA/proxy coverage closure
P13.4 full-sweep 1080p60 proxy gate
P13.5 complete AAA/state proxy gate
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
23. claim register residency, ISA optimality, SIMD speed, A76 proxy cycles, or target-profile frame admission without decoded emitted-word evidence, complete ISA obligations, and the exact cycle-proxy trace;
24. loosen tolerances, error budgets, subdivision depths, costs, or capacities merely to turn a failing fixture green;
25. add an external Cargo dependency;
26. skip a formal gate because Lean is unavailable in the milestone/release environment;
27. update a golden or cycle-proxy lock automatically;
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
- emitted one-ISA A76 assembly with audited DOTPROD/FMLA/widen/reduction/permute/narrow/load-store idioms and exact compiler cycle-proxy admission;
- locked exact A76/Pi 5 target-profile 1080p60/full-quality cycle-proxy conformance, with no physical-hardware gate;
- one release command computing the final verdict.

The single operational rule remains:

> The compiler describes every possible visible interaction admitted by the profile. The from-scratch sweep proves the current frame. The runtime may maintain that proof, but it may never replace a missing proof with a guess.
