# Permanent Pixels fixture: check-pixels-camera-inside

Protects: initial inside occupancy, the first exit boundary, and normal orientation.
Deterministic geometry: the sealed canonical camera lies inside a radius-2
sphere centered at `(0, 0, -4)`.
First active: P7.
P0 status: activated; implemented in task P7.5.
P7 status: the production renderer accepts a from-scratch camera-inside frame;
the reference/conformance path pins first-exit occupancy and orientation.
