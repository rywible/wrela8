# Permanent Pixels fixture: check-pixels-texture-seam

Protects: UV wrap/filter events, bounded filtering, and stable seam ownership.
Deterministic geometry: the seam joins exact fixed UV values `255/256` and `0`.
First active: P3 for the immutable descriptor/filter/dimension contract; P9
adds byte-exact filtered sampling and seam ownership.
P0 status: activated; implemented in task P3.9.
P9 status: active with the sealed trilinear/four-tap anisotropic filter path.
