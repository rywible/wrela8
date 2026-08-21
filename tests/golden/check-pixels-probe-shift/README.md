# Permanent Pixels fixture: check-pixels-probe-shift

Protects: clipmap remapping, invalidation, and world-coordinate retention.
Deterministic geometry: the clipmap shifts one integer cell while world cell `3` remains retained.
First active: P10.
P0 status: activated; implemented in task P10.7.
P10 status: active deterministic clipmap remap and invalidation coverage.
