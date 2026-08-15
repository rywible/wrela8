# Permanent Pixels fixture: check-pixels-tile-boundary

Protects: half-open tile ownership without gaps or duplicate writes.
Deterministic geometry: a high-contrast analytic event crosses x `64`, the boundary between two real 64×32 machine-v1 scanout tiles in a 128×31 partial-tile mode; selected pixels on both sides have independently checked coverage byte 207.
First active: P8.
P0 status: activated; implemented in task P8.7.
