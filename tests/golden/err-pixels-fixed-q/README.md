# Permanent Pixels fixture: err-pixels-fixed-q

Protects: unrepresentable inverse-depth fixed-q setup rejection `P017` after
exhausting the complete v1 exponent range.
Deterministic geometry: the near plane requires 101 signed q bits at exponent
zero, more than the largest v1 exponent can represent.
First active: P6.
P0 status: activated; implemented in task P6.7.
