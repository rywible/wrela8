# Wrela Pixels P8R packet consumer matrix

This is the closed consumer-to-operation ledger required by P8R.5. The
substrate is compiler-internal; rows describe the first P9 consumer, not a
public language API. `load.aligned16` and `store.aligned16` are the mandatory
carrier traffic generated at every packet temporary boundary.

| need ID | P9 task | packet need | landed operation(s) | resolution |
|---|---|---|---|---|
| P9.4-MATERIAL-SOA | P9.4 material summaries | load/store four SoA coefficients and broadcast parameters | f32x4.load, f32x4.store, f32x4.splat, f32x4.add, f32x4.sub, f32x4.mul, f32x4.min, f32x4.max, f32x4.fma | landed |
| P9.4-CANDIDATE-SELECT | P9.4 material summaries | choose a verified candidate without exporting a mask | f32x4.select_ge, f32x4.select_gt | landed |
| P9.5-NORMAL-MOMENTS | P9.5 normal moments | accumulate first/second moments | f32x4.load, f32x4.store, f32x4.splat, f32x4.add, f32x4.sub, f32x4.mul, f32x4.min, f32x4.max, f32x4.fma | landed |
| P9.6-DIRECT-LANES | P9.6 direct lighting | lane arithmetic, bound clamps, and interval choices | f32x4.load, f32x4.store, f32x4.splat, f32x4.add, f32x4.sub, f32x4.mul, f32x4.min, f32x4.max, f32x4.select_ge, f32x4.select_gt, f32x4.fma | landed |
| P9.6-POINT-ATTENUATION | P9.6 direct lighting | point attenuation reciprocal and interval division | scalar | deliberately scalar; P9.6 canonically evaluates attenuation and its interval in the scalar light coefficient path before splatting bounded radiance into packet lanes |
| P9.7-SECONDARY-BOOKKEEPING | P9.7 secondary visibility | fixed-stack indices, signed comparisons, and lane compaction bookkeeping | i32x4.load, i32x4.store, i32x4.splat, i32x4.add, i32x4.sub, i32x4.shr_arith_imm, i32x4.and, i32x4.or, i32x4.select_gt | landed |
| P9.8-AREA-CHILDREN | P9.8 area-light integration | four dyadic child contributions and certified selection | f32x4.load, f32x4.store, f32x4.splat, f32x4.add, f32x4.sub, f32x4.mul, f32x4.min, f32x4.max, f32x4.select_ge, f32x4.select_gt, f32x4.fma | landed |
| P9.9-AO-CANDIDATES | P9.9 deterministic AO | five scalar taps; four-at-a-time candidate arithmetic only | f32x4.load, f32x4.store, f32x4.splat, f32x4.add, f32x4.sub, f32x4.mul, f32x4.min, f32x4.max, f32x4.select_ge, f32x4.select_gt, f32x4.fma | landed; the fifth tap remains scalar as P9.9 already specifies five taps |
| P9.10-CANDIDATE-EVAL | P9.10 packet shading | forward differences and SoA candidate evaluation | f32x4.to_i32x4, i32x4.to_f32x4, i32x4.add, i32x4.sub, i32x4.and, i32x4.or, i32x4.shr_arith_imm, i32x4.select_gt | landed |
| P9.10-BYTE-PACK | P9.10 packet shading | byte packing after singleton proof | f32x4.to_i32x4, i32x4.and, i32x4.or, i32x4.shr_arith_imm | landed |
| P9.11-QUEUE | P9.11 refinement queue | integer priority ordering and queue mutation | scalar | deliberately scalar; P9.11 requires integer cross multiplication and a single deterministic greatest-item choice, not four independent lane choices |

P9 closure is complete for this milestone. Packet reciprocal/rsqrt estimates,
lane extraction, shuffles, reductions, first-class masks, and packet register
allocation are not required by P9.4–P9.11 and remain P12.3 work. Point-light
attenuation is the canonical scalar coefficient path identified above. Scalar
carrier accessors used by the oracle fixtures are not packet operations and
do not create a mask or lane-extraction MWIR value.
