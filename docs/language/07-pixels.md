# Pixels language and machine contract

Pixels is the compiler-owned renderer declaration for machine v1. The
implementation plan in
`docs/designs/WRELA_PIXELS_COMPILER_IMPLEMENTATION_PLAN.md` is the canonical
definition during the P-series implementation program; this chapter records
the source and ABI boundary without copying wire definitions that could drift.

## Source boundary

`@field` marks a top-level synchronous function with one of these signatures:

```wrela
@field
fn world(p: Vec3) -> Field

@field
fn world(p: Vec3, read params: P) -> Field
```

`@material` marks a top-level synchronous function with the matching parameter
type and the one nominal material enum reached by `mark`:

```wrela
@material
fn shade(surface: SurfaceContext[M], read params: P) -> MaterialSample
```

An image binds those roots through `img.renderer[P](...)`. Every v1 label in
the plan's §2.5 is required. The result is
`ImageDecl[Renderer[P]]`; `handle()` returns `Actor[Renderer[P]]`. This is a
sealed exception for the standard `Renderer` actor only. Other generic actor
handles remain unsupported.

`RenderFrame[P]` transfers ownership of `P` to `Renderer[P].render`.
`RenderedFrame[P]` returns it after a successful presentation. The public
runtime failure variants are defined once by `RenderError` in plan §2.6 and
implemented by `stdlib/core/render.wr`.

The field operation set and scalar source semantics are defined by plan §2.2
and `stdlib/core/field.wr`. `Field` is opaque. The compiler recognizes the
canonical operation keys, and authors cannot construct an arbitrary field
value or assert deformation bounds.

## Compiler boundary

The only Pixels dump stages are:

```text
field-graph
frame-program
render-layout
```

`FieldKind` is canonical in plan §4.4. `Iv32` is canonical in plan §5.1.
`FrameProgramHeaderV1` is the exactly 80-byte, little-endian directory header
in plan §4.14; serialization is explicit and never uses Rust host layout.
Formal theorem and kernel audits use the exact filenames
`EXPECTED_AXIOMS.txt` and `KERNELS.txt`.

P-1 accepts exactly one directly marked plane at 64×32. Any sphere, second
plane, or other field composition receives a `pixels` diagnostic. This is a
walking skeleton, not a general renderer or a correctness claim for the later
scanline algorithm.

## Machine-v1 display boundary

The guest is the sole pixel producer. It writes a complete BGRA8 framebuffer
and an ordered tile list, then rings the display doorbell. The VMM validates:

- ABI version, format, 64×32 extent, and stride;
- exact-next sequence number, starting at zero;
- dense zero-based tile IDs in exact descriptor order within the fixed queue capacity;
- in-bounds, non-overlapping, complete coverage;
- exact pixel byte lengths.

On any error, the sequence and guest ownership are unchanged. On success, the
headless sink hashes the complete assembled framebuffer bytes in row-major
BGRA order and returns ownership synchronously. Record/replay includes the
frame sequence and digest. The shared constants and records live in
`crates/wrela-machine/src/pixels.rs`; the guest and host implementations may
not reinterpret them independently.
