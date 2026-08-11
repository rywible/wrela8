# Permanent Pixels fixture: boot-pixels-plane

Protects: the full guest/VMM display path and raw tile/frame/replay digests.
Deterministic geometry: the first boot scene is the exact 64×32 plane boundary case.
First active: P7.
P0 status: activated; implemented in task P7.15.

P7 exercises the production coordinator/worker path and deterministic debug
visibility here; P8 adds protected scanout and replay bytes.
