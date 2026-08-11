# Permanent Pixels fixture: boot-pixels-program-view

Protects: the sealed frame program is readable through checked accessors only;
every out-of-range table/record/operand index fails closed.
First active: P7.1 (created retroactively at P7 close; the behavior was
previously covered by repurposed walking-skeleton tests).
Deterministic geometry: one ground plane offset by the `phase` parameter;
the fixture reads the sealed frame program's tables rather than rendering,
so its expectation depends only on the sealed record layout.
P0 status: activated; implemented in task P7.1.
