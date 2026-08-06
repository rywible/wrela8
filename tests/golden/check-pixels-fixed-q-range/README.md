# Permanent Pixels fixture: check-pixels-fixed-q-range

Protects: fixed-q q/dq/ddq envelope selection, quantization error, and
recurrence reset with scalar agreement.
Deterministic geometry: the compiler seals a power-of-two reset no wider than
64 pixels; a `2^31` world bound must not be mistaken for a q-domain raw value.
First active: P6.
P0 status: activated; implemented in task P6.7.
