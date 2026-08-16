# P8R regression fixture: err-pixels-packet-internal

Protects: D-P8R-07 packet carriers and backend helpers are compiler-internal
and renderer-internal. Ordinary project source is rejected at the token fence
before import resolution can expose `F32x4`.
Deterministic geometry: the fixture contains one forbidden packet carrier use
at a fixed source location and expects one exact name diagnostic.
First active: P8.4.
P0 status: activated; implemented in task P8.4.
The renderer-internal boundary is tightened by P8R.5.
