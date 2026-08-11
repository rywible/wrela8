# Permanent Pixels fixture: boot-pixels-frame-input

Protects: frame input validation returns exact errors before touching the
framebuffer, and the packed snapshot is deterministic.
Deterministic geometry: one ground plane offset by the `phase` parameter,
rendered at a fixed camera; the frame carries no kinetic state, so two
identical frames must produce byte-identical snapshot digests and every
out-of-range, non-finite, or NaN parameter must be rejected before the
framebuffer is touched.
First active: P7.2 (created retroactively at P7 close; the behavior was
previously covered by the renderer-unavailable error-contract fixture).
P0 status: activated; implemented in task P7.2.
