//! Deterministic generated renderer actor/configuration metadata.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::sync::atomic::{AtomicBool, Ordering};

static DEBUG_VISIBILITY: AtomicBool = AtomicBool::new(false);

/// Select the compiler-internal P8 visibility framebuffer for conformance.
/// Telemetry storage is independent: instrumented final-output tests must be
/// able to observe the same P9 bytes as production.
pub fn set_debug_visibility(enabled: bool) {
    DEBUG_VISIBILITY.store(enabled, Ordering::Relaxed);
}

use super::config::RendererConfig;
use super::program::VerifiedFrameProgram;
use super::projection_bounds::{TILE_HEIGHT_V1, TILE_WIDTH_V1};

pub(crate) const RENDERER_FRAME_BOUNDS_WORDS: usize = 47;
pub(crate) const RENDERER_PLACEMENT_WORDS: usize = 3 + super::config::P7_MAX_RENDER_WORKERS;
// Keep chunks below the ordinary runtime-layout ceiling and aligned to every
// P7 record stride (24, 32, and 64 bytes), so a sealed record never straddles
// two generated placed views.
pub(crate) const WORKSPACE_VIEW_CHUNK_BYTES: u64 = 384 * 1024;

// The normalized quartic discriminant for the sealed canonical torus
// R=2, r=1, eye=(0,0,-E), in X=u^2, Y=v^2, and E. The common factor 65536
// is omitted because it cannot change the zero set or side topology.
const STANDARD_TORUS_DISCRIMINANT_TERMS: [(u8, u8, u8, i32); 106] = [
    (0, 0, 0, 9),
    (0, 1, 0, -36),
    (0, 1, 2, -12),
    (0, 2, 0, -18),
    (0, 2, 2, -150),
    (0, 2, 4, -2),
    (0, 3, 0, 108),
    (0, 3, 2, -210),
    (0, 3, 4, -70),
    (0, 3, 6, 4),
    (0, 4, 0, 81),
    (0, 4, 2, -18),
    (0, 4, 4, -125),
    (0, 4, 6, -2),
    (0, 4, 8, 1),
    (0, 5, 2, 54),
    (0, 5, 4, -48),
    (0, 5, 6, -16),
    (0, 5, 8, 2),
    (0, 6, 4, 9),
    (0, 6, 6, -10),
    (0, 6, 8, 1),
    (1, 0, 0, 54),
    (1, 0, 2, -10),
    (1, 1, 0, -180),
    (1, 1, 2, 10),
    (1, 1, 4, 22),
    (1, 2, 0, -72),
    (1, 2, 2, -430),
    (1, 2, 4, 168),
    (1, 2, 6, -14),
    (1, 3, 0, 324),
    (1, 3, 2, -648),
    (1, 3, 4, 220),
    (1, 3, 6, -46),
    (1, 3, 8, 2),
    (1, 4, 0, 162),
    (1, 4, 2, -252),
    (1, 4, 4, 170),
    (1, 4, 6, -78),
    (1, 4, 8, 6),
    (1, 5, 2, -54),
    (1, 5, 4, 96),
    (1, 5, 6, -46),
    (1, 5, 8, 4),
    (2, 0, 0, 135),
    (2, 0, 2, -50),
    (2, 0, 4, 1),
    (2, 1, 0, -360),
    (2, 1, 2, 160),
    (2, 1, 4, 42),
    (2, 1, 6, -2),
    (2, 2, 0, -108),
    (2, 2, 2, -390),
    (2, 2, 4, 349),
    (2, 2, 6, -46),
    (2, 2, 8, 1),
    (2, 3, 0, 324),
    (2, 3, 2, -666),
    (2, 3, 4, 458),
    (2, 3, 6, -106),
    (2, 3, 8, 6),
    (2, 4, 0, 81),
    (2, 4, 2, -234),
    (2, 4, 4, 223),
    (2, 4, 6, -76),
    (2, 4, 8, 6),
    (3, 0, 0, 180),
    (3, 0, 2, -100),
    (3, 0, 4, 4),
    (3, 1, 0, -360),
    (3, 1, 2, 300),
    (3, 1, 4, -6),
    (3, 1, 6, -2),
    (3, 2, 0, -72),
    (3, 2, 2, -90),
    (3, 2, 4, 186),
    (3, 2, 6, -42),
    (3, 2, 8, 2),
    (3, 3, 0, 108),
    (3, 3, 2, -228),
    (3, 3, 4, 168),
    (3, 3, 6, -52),
    (3, 3, 8, 4),
    (4, 0, 0, 135),
    (4, 0, 2, -100),
    (4, 0, 4, 6),
    (4, 1, 0, -180),
    (4, 1, 2, 220),
    (4, 1, 4, -50),
    (4, 1, 6, 2),
    (4, 2, 0, -18),
    (4, 2, 2, 20),
    (4, 2, 4, 7),
    (4, 2, 6, -10),
    (4, 2, 8, 1),
    (5, 0, 0, 54),
    (5, 0, 2, -50),
    (5, 0, 4, 4),
    (5, 1, 0, -36),
    (5, 1, 2, 58),
    (5, 1, 4, -24),
    (5, 1, 6, 2),
    (6, 0, 0, 9),
    (6, 0, 2, -10),
    (6, 0, 4, 1),
];

// Point root-count classifier paired with the canonical torus discriminant
// integrator. This lives in the generated image module (rather than
// `core.render`) because injected renderer bodies are evaluated in that module
// and cannot reach private helpers from their source module.
const ALIGNED_DEFORMATION_DEPTH_MISS_SOURCE: &str = r#"
pub fn __wrela_pixels_p7_aligned_deformation_depth_miss(
    read cell: [f32; 4],
    t_lo: f32,
    t_hi: f32,
    phase: f32,
    read model: [f32; 26],
    read camera: [f32; 12],
) -> bool:
    u0 = cell[0]
    v0 = cell[1]
    u1 = cell[2]
    v1 = cell[3]
    if (
        model[21] != 1.0 or not model[25] > 0.0 or not model[18] > 0.0
        or camera[0] != model[22] or camera[1] != model[23]
        or camera[3] != 0.0 or camera[4] != 0.0 or camera[5] != 1.0
        or camera[6] != 1.0 or camera[7] != 0.0 or camera[8] != 0.0
        or camera[9] != 0.0 or camera[10] != 1.0 or camera[11] != 0.0
        or not t_hi > t_lo or t_lo < 0.0
    ):
        return false
    direct_z = model[24] - camera[2]
    if not direct_z > 0.0 or not __wrela_pixels_p5_finite(direct_z):
        return false
    if not __wrela_pixels_p5_finite(t_lo) or not __wrela_pixels_p5_finite(t_hi):
        return false
    window_pad = (
        __wrela_pixels_p7_abs(t_lo) + __wrela_pixels_p7_abs(t_hi)
        + direct_z + 1.0
    ) * 0.0000038147118175957395
    window_lo = t_lo - window_pad
    if window_lo < 0.0:
        window_lo = 0.0
    window_hi = t_hi + window_pad
    window_width = window_hi - window_lo
    if not window_width > 0.0 or not __wrela_pixels_p5_finite(window_width):
        return false
    u_abs_min = __wrela_pixels_p7_abs(u0)
    if __wrela_pixels_p7_abs(u1) < u_abs_min:
        u_abs_min = __wrela_pixels_p7_abs(u1)
    if u0 <= 0.0 and u1 >= 0.0:
        u_abs_min = 0.0
    v_abs_min = __wrela_pixels_p7_abs(v0)
    if __wrela_pixels_p7_abs(v1) < v_abs_min:
        v_abs_min = __wrela_pixels_p7_abs(v1)
    if v0 <= 0.0 and v1 >= 0.0:
        v_abs_min = 0.0
    u_abs_max = __wrela_pixels_p7_abs(u0)
    if __wrela_pixels_p7_abs(u1) > u_abs_max:
        u_abs_max = __wrela_pixels_p7_abs(u1)
    v_abs_max = __wrela_pixels_p7_abs(v0)
    if __wrela_pixels_p7_abs(v1) > v_abs_max:
        v_abs_max = __wrela_pixels_p7_abs(v1)
    s_lo = (u_abs_min * u_abs_min + v_abs_min * v_abs_min) * 0.9999990463247741274971
    s_hi = (u_abs_max * u_abs_max + v_abs_max * v_abs_max) * 1.0000009536752258725029
    if not s_lo >= 0.0 or not s_hi >= s_lo:
        return false
    a_lo = (1.0 + s_lo) * 0.9999997615813639327412
    a_hi = (1.0 + s_hi) * 1.0000002384186360672588
    z_squared_lo = direct_z * direct_z * 0.999999523162614423249
    radial_min_squared = (
        z_squared_lo * s_lo / a_hi
    ) * 0.9999990463247741274971
    closest_lo = direct_z / a_hi
    closest_hi = direct_z / a_lo
    closest_pad = (
        __wrela_pixels_p7_abs(closest_lo) + __wrela_pixels_p7_abs(closest_hi)
    ) * 0.000000476837385576751
    closest_lo = closest_lo - closest_pad
    closest_hi = closest_hi + closest_pad
    if not radial_min_squared >= 0.0 or not closest_hi >= closest_lo:
        return false
    segment: u8 = 0
    @budget(bound=16)
    while segment < 16:
        ta = window_lo + window_width * segment.to[f32]() * 0.0625
        tb = window_lo + window_width * (segment + 1).to[f32]() * 0.0625
        if segment == 15:
            tb = window_hi
        if not tb >= ta:
            return false
        x_lo = u0 * ta
        x_hi = x_lo
        product: u8 = 1
        @budget(bound=4)
        while product < 4:
            product_u = u0
            if product % 2 == 1:
                product_u = u1
            product_t = ta
            if product >= 2:
                product_t = tb
            candidate_x = product_u * product_t
            if candidate_x < x_lo:
                x_lo = candidate_x
            if candidate_x > x_hi:
                x_hi = candidate_x
            product = product + 1
        x_pad = (
            __wrela_pixels_p7_abs(x_lo) + __wrela_pixels_p7_abs(x_hi)
        ) * 0.000000476837385576751
        x_lo = x_lo - x_pad
        x_hi = x_hi + x_pad
        depth_delta: f32 = 0.0
        if tb < closest_lo:
            depth_delta = closest_lo - tb
        elif ta > closest_hi:
            depth_delta = ta - closest_hi
        distance_squared = radial_min_squared + a_lo * depth_delta * depth_delta
        distance_squared = distance_squared * 0.999999523162614423249
        distance_lo = sqrt_scalar(distance_squared) * 0.9999997615813639327412
        world_x_lo = model[22] + x_lo
        world_x_hi = model[22] + x_hi
        argument_lo = model[19] * world_x_lo + phase
        argument_hi = model[19] * world_x_hi + phase
        if argument_hi < argument_lo:
            argument_swap = argument_lo
            argument_lo = argument_hi
            argument_hi = argument_swap
        argument_pad = (
            __wrela_pixels_p7_abs(argument_lo)
            + __wrela_pixels_p7_abs(argument_hi)
            + __wrela_pixels_p7_abs(model[19] * world_x_lo)
            + __wrela_pixels_p7_abs(model[19] * world_x_hi)
            + __wrela_pixels_p7_abs(phase)
        ) * 0.0000009536752258725029
        sine = __wrela_pixels_p7_interval_sin(
            argument_lo - argument_pad, argument_hi + argument_pad,
        )
        displacement_lo = model[25] * sine[0]
        displacement_lo = displacement_lo - (
            __wrela_pixels_p7_abs(displacement_lo) + model[25]
        ) * 0.0000002384186360672588
        field_lo = distance_lo - model[18] + displacement_lo
        field_magnitude = (
            __wrela_pixels_p7_abs(distance_lo) + model[18]
            + __wrela_pixels_p7_abs(displacement_lo)
        )
        if not field_lo - field_magnitude * 0.000000476837385576751 > 0.0:
            return false
        segment = segment + 1
    return true
"#;

const ALIGNED_DEFORMATION_DEPTH_MISS_STUB: &str = r#"
pub fn __wrela_pixels_p7_aligned_deformation_depth_miss(
    read cell: [f32; 4],
    t_lo: f32,
    t_hi: f32,
    phase: f32,
    read model: [f32; 26],
    read camera: [f32; 12],
) -> bool:
    return false
"#;

const STANDARD_TORUS_ROOT_CLASSIFIER_SOURCE: &str = r#"
pub fn __wrela_pixels_p7_standard_torus_positive_hit(u: f32, v: f32, eye: f32) -> [i64; 2]:
    x = u * u
    y = v * v
    eye2 = eye * eye
    sum = x + y + 1.0
    a = sum * sum
    b = sum * eye * -4.0
    c = eye2 * 4.0 + sum * (eye2 + 3.0) * 2.0 - (x + 1.0) * 16.0
    d = eye * (5.0 - eye2) * 4.0
    e = (eye2 - 1.0) * (eye2 - 9.0)
    p0 = a * c * 8.0
    p1 = b * b * -3.0
    p_value = p0 + p1
    p_error = (
        __wrela_pixels_p7_abs(p0) + __wrela_pixels_p7_abs(p1)
    ) * 0.00004 + 1.0e-18
    d0 = a * a * a * e * 64.0
    d1 = a * a * c * c * -16.0
    d2 = a * b * b * c * 16.0
    d3 = a * a * b * d * -16.0
    d4 = b * b * b * b * -3.0
    d_value = d0 + d1 + d2 + d3 + d4
    d_error = (
        __wrela_pixels_p7_abs(d0)
        + __wrela_pixels_p7_abs(d1)
        + __wrela_pixels_p7_abs(d2)
        + __wrela_pixels_p7_abs(d3)
        + __wrela_pixels_p7_abs(d4)
    ) * 0.00004 + 1.0e-18
    if (
        p_value != p_value or p_error != p_error
        or d_value != d_value or d_error != d_error
    ):
        return [0; 2]
    if p_value < 0.0 - p_error and d_value < 0.0 - d_error:
        return [1, 1]
    if p_value > p_error or d_value > d_error:
        return [1, 0]
    return [0; 2]
"#;

// Box-wide version of the same P/Q root-count test. If P and Q are negative
// throughout a cell, every ray has four roots on D > 0 and two on D < 0, so
// the standalone torus occupies the whole cell. If either invariant is
// positive throughout, the positive-discriminant side is uniformly empty.
// All arithmetic is outward-rounded; an overlapping interval declines.
const STANDARD_TORUS_CELL_CLASSIFIER_SOURCE: &str = r#"
pub fn __wrela_pixels_p7_standard_interval_mul(read a: [f32; 2], read b: [f32; 2]) -> [f32; 2]:
    p0 = a[0] * b[0]
    p1 = a[0] * b[1]
    p2 = a[1] * b[0]
    p3 = a[1] * b[1]
    lo = p0
    hi = p0
    if p1 < lo:
        lo = p1
    if p2 < lo:
        lo = p2
    if p3 < lo:
        lo = p3
    if p1 > hi:
        hi = p1
    if p2 > hi:
        hi = p2
    if p3 > hi:
        hi = p3
    return [
        __wrela_pixels_p7_outward_low(lo),
        __wrela_pixels_p7_outward_high(hi),
    ]

pub fn __wrela_pixels_p7_standard_torus_cell_positive_hit(u: f32, v: f32, ru: f32, rv: f32, eye: f32) -> [i64; 2]:
    if ru < 0.0 or rv < 0.0 or not eye > 0.0:
        return [0; 2]
    u0 = u - ru
    u1 = u + ru
    v0 = v - rv
    v1 = v + rv
    u2_lo: f32 = 0.0
    if u0 > 0.0:
        u2_lo = __wrela_pixels_p7_outward_low(u0 * u0)
    elif u1 < 0.0:
        u2_lo = __wrela_pixels_p7_outward_low(u1 * u1)
    u2_hi = __wrela_pixels_p7_outward_high(u0 * u0)
    u2_other = __wrela_pixels_p7_outward_high(u1 * u1)
    if u2_other > u2_hi:
        u2_hi = u2_other
    v2_lo: f32 = 0.0
    if v0 > 0.0:
        v2_lo = __wrela_pixels_p7_outward_low(v0 * v0)
    elif v1 < 0.0:
        v2_lo = __wrela_pixels_p7_outward_low(v1 * v1)
    v2_hi = __wrela_pixels_p7_outward_high(v0 * v0)
    v2_other = __wrela_pixels_p7_outward_high(v1 * v1)
    if v2_other > v2_hi:
        v2_hi = v2_other
    x: [f32; 2] = [u2_lo, u2_hi]
    y: [f32; 2] = [v2_lo, v2_hi]
    eye2: [f32; 2] = [
        __wrela_pixels_p7_outward_low(eye * eye),
        __wrela_pixels_p7_outward_high(eye * eye),
    ]
    sum: [f32; 2] = [
        __wrela_pixels_p7_outward_low(x[0] + y[0] + 1.0),
        __wrela_pixels_p7_outward_high(x[1] + y[1] + 1.0),
    ]
    a = __wrela_pixels_p7_standard_interval_mul(sum, sum)
    b: [f32; 2] = [
        __wrela_pixels_p7_outward_low(sum[1] * eye * -4.0),
        __wrela_pixels_p7_outward_high(sum[0] * eye * -4.0),
    ]
    eye2_plus3: [f32; 2] = [
        __wrela_pixels_p7_outward_low(eye2[0] + 3.0),
        __wrela_pixels_p7_outward_high(eye2[1] + 3.0),
    ]
    c_middle = __wrela_pixels_p7_standard_interval_mul(sum, eye2_plus3)
    c: [f32; 2] = [
        __wrela_pixels_p7_outward_low(eye2[0] * 4.0 + c_middle[0] * 2.0 - (x[1] + 1.0) * 16.0),
        __wrela_pixels_p7_outward_high(eye2[1] * 4.0 + c_middle[1] * 2.0 - (x[0] + 1.0) * 16.0),
    ]
    five_minus_eye2: [f32; 2] = [
        __wrela_pixels_p7_outward_low(5.0 - eye2[1]),
        __wrela_pixels_p7_outward_high(5.0 - eye2[0]),
    ]
    d0 = __wrela_pixels_p7_standard_interval_mul([eye, eye], five_minus_eye2)
    d: [f32; 2] = [
        __wrela_pixels_p7_outward_low(d0[0] * 4.0),
        __wrela_pixels_p7_outward_high(d0[1] * 4.0),
    ]
    em1: [f32; 2] = [
        __wrela_pixels_p7_outward_low(eye2[0] - 1.0),
        __wrela_pixels_p7_outward_high(eye2[1] - 1.0),
    ]
    em9: [f32; 2] = [
        __wrela_pixels_p7_outward_low(eye2[0] - 9.0),
        __wrela_pixels_p7_outward_high(eye2[1] - 9.0),
    ]
    e = __wrela_pixels_p7_standard_interval_mul(em1, em9)
    ac = __wrela_pixels_p7_standard_interval_mul(a, c)
    bb = __wrela_pixels_p7_standard_interval_mul(b, b)
    p: [f32; 2] = [
        __wrela_pixels_p7_outward_low(ac[0] * 8.0 - bb[1] * 3.0),
        __wrela_pixels_p7_outward_high(ac[1] * 8.0 - bb[0] * 3.0),
    ]
    aa = __wrela_pixels_p7_standard_interval_mul(a, a)
    aaa = __wrela_pixels_p7_standard_interval_mul(aa, a)
    aaae = __wrela_pixels_p7_standard_interval_mul(aaa, e)
    cc = __wrela_pixels_p7_standard_interval_mul(c, c)
    aacc = __wrela_pixels_p7_standard_interval_mul(aa, cc)
    ab = __wrela_pixels_p7_standard_interval_mul(a, b)
    abc = __wrela_pixels_p7_standard_interval_mul(ab, c)
    abbc = __wrela_pixels_p7_standard_interval_mul(abc, b)
    aab = __wrela_pixels_p7_standard_interval_mul(aa, b)
    aabd = __wrela_pixels_p7_standard_interval_mul(aab, d)
    bbbb = __wrela_pixels_p7_standard_interval_mul(bb, bb)
    q: [f32; 2] = [
        __wrela_pixels_p7_outward_low(aaae[0] * 64.0 - aacc[1] * 16.0 + abbc[0] * 16.0 - aabd[1] * 16.0 - bbbb[1] * 3.0),
        __wrela_pixels_p7_outward_high(aaae[1] * 64.0 - aacc[0] * 16.0 + abbc[1] * 16.0 - aabd[0] * 16.0 - bbbb[0] * 3.0),
    ]
    if p[1] < 0.0 and q[1] < 0.0:
        return [1, 1]
    if p[0] > 0.0 or q[0] > 0.0:
        return [1, 0]
    return [0; 2]
"#;

// Error-free f32 transforms plus a conservative second-order remainder.
// Every returned triple is (hi, lo, absolute_error), enclosing the real
// result represented by the two-word expansion. The split constant is
// 2^12+1 for IEEE binary32's 24-bit significand. The 2e-12 factor is over
// 512*u^2 and therefore covers the rounded cross-term accumulation in the
// short (degree <= 4) coefficient Horner chains below.
const STANDARD_DOUBLE_DOUBLE_SOURCE: &str = r#"
pub fn __wrela_pixels_p7_standard_two_sum(a: f32, b: f32) -> [f32; 2]:
    sum = a + b
    virtual_b = sum - a
    error = (a - (sum - virtual_b)) + (b - virtual_b)
    return [sum, error]

pub fn __wrela_pixels_p7_standard_two_product(a: f32, b: f32) -> [f32; 2]:
    product = a * b
    split_a = a * 4097.0
    a_hi = split_a - (split_a - a)
    a_lo = a - a_hi
    split_b = b * 4097.0
    b_hi = split_b - (split_b - b)
    b_lo = b - b_hi
    error = ((a_hi * b_hi - product) + a_hi * b_lo + a_lo * b_hi) + a_lo * b_lo
    return [product, error]

pub fn __wrela_pixels_p7_standard_dd_mul(read a: [f32; 3], read b: [f32; 3]) -> [f32; 3]:
    product = __wrela_pixels_p7_standard_two_product(a[0], b[0])
    correction = product[1] + a[0] * b[1] + a[1] * b[0] + a[1] * b[1]
    normalized = __wrela_pixels_p7_standard_two_sum(product[0], correction)
    a_magnitude = __wrela_pixels_p7_abs(a[0]) + __wrela_pixels_p7_abs(a[1])
    b_magnitude = __wrela_pixels_p7_abs(b[0]) + __wrela_pixels_p7_abs(b[1])
    error = __wrela_pixels_p7_outward_high(
        a[2] * (b_magnitude + b[2])
        + b[2] * a_magnitude
        + (a_magnitude + a[2]) * (b_magnitude + b[2]) * 0.000000000002
    )
    return [normalized[0], normalized[1], error]

pub fn __wrela_pixels_p7_standard_dd_add_f32(read a: [f32; 3], b: f32) -> [f32; 3]:
    sum = __wrela_pixels_p7_standard_two_sum(a[0], b)
    normalized = __wrela_pixels_p7_standard_two_sum(sum[0], sum[1] + a[1])
    magnitude = __wrela_pixels_p7_abs(a[0]) + __wrela_pixels_p7_abs(a[1]) + a[2] + __wrela_pixels_p7_abs(b)
    error = __wrela_pixels_p7_outward_high(a[2] + magnitude * 0.000000000002)
    return [normalized[0], normalized[1], error]
"#;

// Keep non-torus images small. `core.render` imports the canonical-torus ABI
// for every renderer, so the names must exist even when sealing proved that no
// event can use them; zero-status stubs preserve that ABI and make every call
// fail closed without paying for the analytic kernel in unrelated images.
const STANDARD_TORUS_STUB_SOURCE: &str = r#"
pub fn __wrela_pixels_p7_standard_interval_mul(read a: [f32; 2], read b: [f32; 2]) -> [f32; 2]:
    return [0.0; 2]

pub fn __wrela_pixels_p7_standard_torus_positive_hit(u: f32, v: f32, eye: f32) -> [i64; 2]:
    return [0; 2]

pub fn __wrela_pixels_p7_standard_torus_cell_positive_hit(u: f32, v: f32, ru: f32, rv: f32, eye: f32) -> [i64; 2]:
    return [0; 2]

pub fn __wrela_pixels_p7_standard_torus_coefficients(renderer: usize, event: u32, read camera: [f32; 12]) -> [f32; 57]:
    return [0.0; 57]

pub fn __wrela_pixels_p7_standard_torus_value(read coefficients: [f32; 57], u: f32, v: f32) -> [f32; 3]:
    return [0.0; 3]

pub fn __wrela_pixels_p7_standard_torus_pixel_bounds(read coefficients: [f32; 57], u: f32, v: f32, ru: f32, rv: f32) -> [f32; 8]:
    return [0.0; 8]

pub fn __wrela_pixels_p7_standard_torus_discriminant(read coefficients: [f32; 57], read pixel_bounds: [f32; 8], u: f32, v: f32, ru: f32, rv: f32) -> [f32; 5]:
    return [0.0; 5]
"#;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GeneratedWorker {
    pub actor: String,
    pub core: usize,
    pub tiles_start: u32,
    pub tiles_end: u32,
}

#[derive(Clone, Debug, PartialEq)]
pub struct GeneratedRenderer {
    pub renderer_index: usize,
    pub coordinator: String,
    pub display_index: usize,
    pub workers: Vec<GeneratedWorker>,
    pub exposure_range: [f32; 2],
    pub environment_min: [f32; 3],
    pub environment_max: [f32; 3],
    pub camera_bounds: [[f32; 2]; 12],
    pub world_min: [f32; 3],
    pub world_max: [f32; 3],
    pub light_capacity: usize,
    pub light_kinds: [usize; 8],
    pub rooted_functions: Vec<String>,
    pub bootstrap_families: Vec<String>,
}

fn bootstrap_families(program: &VerifiedFrameProgram) -> BTreeSet<&'static str> {
    program
        .program()
        .tables
        .iter()
        .filter(|table| !table.records.is_empty())
        .map(|table| table.kind.stable_name())
        .collect()
}

fn write_renderer_constants(output: &mut String, index: usize, values: &[(&str, u64)]) {
    for (name, value) in values {
        writeln!(output, "const R{index}_{name}: usize = {value}")
            .expect("String writes cannot fail");
    }
}

fn outward_f32_interval(interval: super::reference::interval::F64Interval) -> [f32; 2] {
    let mut lo = interval.lo as f32;
    if f64::from(lo) > interval.lo {
        lo = super::reference::interval::next_down_f32(lo);
    }
    let mut hi = interval.hi as f32;
    if f64::from(hi) < interval.hi {
        hi = super::reference::interval::next_up_f32(hi);
    }
    [lo, hi]
}

fn cluster_requires_semantic_tube(
    renderer: &super::CompiledRenderer,
    cluster: &super::derivatives::DerivativeClusterTemplate,
) -> bool {
    let composed = renderer
        .structural
        .program()
        .objects
        .objects
        .iter()
        .find(|object| object.id == cluster.object)
        .is_some_and(|object| object.primitive_occurrences.len() > 1);
    composed
        || cluster.bundles.iter().any(|bundle| {
            renderer
                .projective
                .program()
                .equations
                .features
                .get(bundle.index())
                .is_some_and(|feature| feature.deformed_predictor)
        })
}

fn object_requires_semantic_scalar(
    renderer: &super::CompiledRenderer,
    object: &super::objects::SmoothObject,
) -> bool {
    let feature_count = renderer
        .structural
        .program()
        .features
        .iter()
        .filter(|feature| feature.object == object.id)
        .count();
    feature_count != 1
        || renderer
            .projective
            .program()
            .derivatives
            .clusters
            .iter()
            .any(|cluster| {
                cluster.object == object.id && cluster_requires_semantic_tube(renderer, cluster)
            })
}

fn light_kind_tag(kind: &str) -> Result<usize, String> {
    super::config::light_kind_tag(kind)
        .and_then(|tag| usize::try_from(tag).ok())
        .ok_or_else(|| format!("pixels::glue: sealed renderer has unknown light kind `{kind}`"))
}

fn state_region_constants(
    state_base: u64,
    base_name: &'static str,
    bytes_name: &'static str,
    region: super::state::StateRegion,
    bytes: Option<u64>,
) -> Result<[(&'static str, u64); 2], String> {
    let base = state_base
        .checked_add(region.offset)
        .ok_or_else(|| format!("P025: generated {base_name} state address overflow"))?;
    Ok([
        (base_name, base),
        (bytes_name, bytes.unwrap_or(region.bytes)),
    ])
}

fn canonical_wire_view_source() -> Result<String, String> {
    let (_, loaded) = crate::loader::load_render_program_module()
        .map_err(|_| "pixels::glue: stdlib/core/render_program.wr missing".to_string())?;
    let expected = BTreeSet::from([
        "FrameProgramHeaderV1",
        "FrameProgramTableV1",
        "FrameProgramRecordV1",
        "FrameProgramImmediateV1",
    ]);
    let mut module = loaded.module;
    module.path = vec!["__image_pixels".to_string()];
    module.imports.clear();
    module.items.retain(|item| {
        matches!(
            item,
            crate::syntax::ast::Item::Struct(item) if expected.contains(item.name.as_str())
        )
    });
    let found = module
        .items
        .iter()
        .filter_map(|item| match item {
            crate::syntax::ast::Item::Struct(item) => Some(item.name.as_str()),
            _ => None,
        })
        .collect::<BTreeSet<_>>();
    if found != expected {
        return Err("pixels::glue: canonical render-program wire views are incomplete".to_string());
    }
    let source = crate::syntax::printer::pretty(&module);
    let body = source
        .strip_prefix("module __image_pixels\n")
        .ok_or_else(|| {
            "pixels::glue: canonical render-program module has an unexpected address".to_string()
        })?;
    Ok(body.to_string())
}

fn table_view_name(kind: wrela_machine::pixels::FrameProgramTableKindV1) -> String {
    kind.stable_name()
        .split('-')
        .map(|part| {
            let mut chars = part.chars();
            chars
                .next()
                .map(|first| first.to_ascii_uppercase().to_string() + chars.as_str())
                .unwrap_or_default()
        })
        .collect()
}

fn wrela_f32_literal(value: f32) -> Result<String, String> {
    if !value.is_finite() {
        return Err("pixels::glue: generated P7 scalar is non-finite".to_string());
    }
    let mut literal = value.to_string();
    if !literal.contains('.') && !literal.contains('e') && !literal.contains('E') {
        literal.push_str(".0");
    }
    Ok(literal)
}

fn scalar_slot(id: super::ids::ScalarId) -> String {
    format!("__p7_scalar_{}", id.index())
}

fn transform_scalar_ids(
    transform: &super::graph::TransformProgram,
    scalars: &mut Vec<super::ids::ScalarId>,
) {
    use super::graph::TransformProgram;
    match transform {
        TransformProgram::Translate { by } => scalars.extend(by),
        TransformProgram::Rotate {
            row_x,
            row_y,
            row_z,
        } => {
            scalars.extend(row_x);
            scalars.extend(row_y);
            scalars.extend(row_z);
        }
        TransformProgram::Rigid {
            translation,
            row_x,
            row_y,
            row_z,
        } => {
            scalars.extend(translation);
            scalars.extend(row_x);
            scalars.extend(row_y);
            scalars.extend(row_z);
        }
        TransformProgram::UniformScale { .. } => {}
        TransformProgram::SourceRigidSequence { steps, .. }
        | TransformProgram::RigidSequence { steps, .. } => {
            for step in steps {
                transform_scalar_ids(step, scalars);
            }
        }
    }
}

fn write_p9_local_transform(
    output: &mut String,
    transform: &super::graph::TransformProgram,
    temporary: &mut u32,
) -> Result<(), String> {
    use super::graph::TransformProgram;
    match transform {
        TransformProgram::Translate { by } => {
            for (axis, name) in ["x", "y", "z"].into_iter().enumerate() {
                writeln!(
                    output,
                    "        local_p_{name} = local_p_{name} - {}",
                    scalar_slot(by[axis]),
                )
                .expect("String writes cannot fail");
            }
        }
        TransformProgram::Rotate {
            row_x,
            row_y,
            row_z,
        } => {
            let rows = [row_x, row_y, row_z];
            let id = *temporary;
            *temporary = temporary
                .checked_add(1)
                .ok_or_else(|| "pixels::glue: local-frame temporary overflow".to_string())?;
            for vector in ["p", "n", "dx", "dy"] {
                for name in ["x", "y", "z"] {
                    writeln!(
                        output,
                        "        uv_{id}_{vector}_{name} = local_{vector}_{name}"
                    )
                    .expect("String writes cannot fail");
                }
            }
            for (row, destination) in rows.into_iter().zip(["x", "y", "z"]) {
                for vector in ["p", "n", "dx", "dy"] {
                    writeln!(
                        output,
                        "        local_{vector}_{destination} = {} * uv_{id}_{vector}_x + {} * uv_{id}_{vector}_y + {} * uv_{id}_{vector}_z",
                        scalar_slot(row[0]),
                        scalar_slot(row[1]),
                        scalar_slot(row[2]),
                    )
                    .expect("String writes cannot fail");
                }
            }
        }
        TransformProgram::Rigid {
            translation,
            row_x,
            row_y,
            row_z,
        } => {
            write_p9_local_transform(
                output,
                &TransformProgram::Translate { by: *translation },
                temporary,
            )?;
            write_p9_local_transform(
                output,
                &TransformProgram::Rotate {
                    row_x: *row_x,
                    row_y: *row_y,
                    row_z: *row_z,
                },
                temporary,
            )?;
        }
        // UniformScale changes the field value, not its coordinates.
        TransformProgram::UniformScale { .. } => {}
        TransformProgram::SourceRigidSequence { steps, .. }
        | TransformProgram::RigidSequence { steps, .. } => {
            for step in steps {
                write_p9_local_transform(output, step, temporary)?;
            }
        }
    }
    Ok(())
}

fn scalar_dependency_closure(
    renderer: &super::CompiledRenderer,
    mut stack: Vec<super::ids::ScalarId>,
) -> Result<BTreeSet<usize>, String> {
    use super::scalar::ScalarOp;
    let mut scalars = BTreeSet::new();
    while let Some(id) = stack.pop() {
        if !scalars.insert(id.index()) {
            continue;
        }
        let node = renderer.symbolic.scalar.get(id)?;
        match &node.op {
            ScalarOp::Add(a, b)
            | ScalarOp::Sub(a, b)
            | ScalarOp::Mul(a, b)
            | ScalarOp::Div(a, b)
            | ScalarOp::Min(a, b)
            | ScalarOp::Max(a, b)
            | ScalarOp::Compare { a, b, .. } => stack.extend([*a, *b]),
            ScalarOp::Neg(value)
            | ScalarOp::Abs(value)
            | ScalarOp::Sqrt(value, _)
            | ScalarOp::Rsqrt(value, _)
            | ScalarOp::SinRestricted(value, _)
            | ScalarOp::CosRestricted(value, _)
            | ScalarOp::MaterialRoughness { value, .. } => stack.push(*value),
            ScalarOp::Clamp { value, lo, hi } => stack.extend([*value, *lo, *hi]),
            ScalarOp::Dot3(a, b) | ScalarOp::Cross3Component { a, b, .. } => {
                stack.extend(a);
                stack.extend(b);
            }
            ScalarOp::Length2(values) => stack.extend(values),
            ScalarOp::Length3(values) | ScalarOp::Normalize3Component { value: values, .. } => {
                stack.extend(values);
            }
            ScalarOp::Select { predicate, a, b } => stack.extend([*predicate, *a, *b]),
            ScalarOp::SelectIndex { index, options } => {
                stack.push(*index);
                stack.extend(options);
            }
            ScalarOp::SmoothMin { a, b, k, .. } => stack.extend([*a, *b, *k]),
            ScalarOp::FiniteOr {
                value, fallback, ..
            } => stack.extend([*value, *fallback]),
            ScalarOp::ConstF32(_)
            | ScalarOp::ConstF64(_)
            | ScalarOp::CoordX
            | ScalarOp::CoordY
            | ScalarOp::CoordZ
            | ScalarOp::SurfacePosition(_)
            | ScalarOp::SurfaceNormal(_)
            | ScalarOp::Param(_) => {}
        }
    }
    Ok(scalars)
}

pub(crate) fn is_standard_torus_event(
    renderer: &super::CompiledRenderer,
    event: &super::events::EventGenerator,
) -> Result<bool, String> {
    use super::graph::{FieldKind, Primitive};

    let Some(feature_id) = event
        .participants
        .iter()
        .find_map(|participant| match participant {
            super::events::Participant::Feature(feature) => Some(feature),
            _ => None,
        })
    else {
        return Ok(false);
    };
    let Some(feature) = renderer
        .structural
        .program()
        .features
        .get(feature_id.index())
    else {
        return Ok(false);
    };
    if feature.occurrence_path.len() != 1 {
        return Ok(false);
    }
    let node = renderer.symbolic.fields.get(feature.primitive)?;
    let FieldKind::Primitive(Primitive::Torus {
        center,
        axis,
        major,
        minor,
    }) = node.kind
    else {
        return Ok(false);
    };
    let constant = |id| {
        renderer
            .symbolic
            .scalar
            .get(id)
            .ok()
            .and_then(super::scalar::constant_bits)
            .map(f32::from_bits)
    };
    Ok(center.map(constant) == [Some(0.0), Some(0.0), Some(0.0)]
        && axis.map(constant) == [Some(0.0), Some(1.0), Some(0.0)]
        && constant(major) == Some(2.0)
        && constant(minor) == Some(1.0))
}

fn required_polynomial_values(
    renderer: &super::CompiledRenderer,
    polynomials: impl IntoIterator<Item = super::ids::PolyProgramId>,
) -> Result<(BTreeSet<usize>, BTreeSet<usize>), String> {
    use super::program::CoeffOp;

    let equations = &renderer.projective.program().equations;
    let mut coefficient_stack = polynomials
        .into_iter()
        .flat_map(|polynomial| {
            equations.polynomials[polynomial.index()]
                .terms
                .iter()
                .map(|term| term.coefficient)
        })
        .collect::<Vec<_>>();
    let mut coefficients = BTreeSet::new();
    let mut scalar_stack = Vec::new();
    while let Some(id) = coefficient_stack.pop() {
        if !coefficients.insert(id.index()) {
            continue;
        }
        let node = equations
            .coefficients
            .nodes
            .get(id.index())
            .ok_or_else(|| "pixels::glue: root polynomial coefficient is missing".to_string())?;
        match node.op {
            CoeffOp::Scalar(value) => scalar_stack.push(value),
            CoeffOp::Add(a, b) | CoeffOp::Mul(a, b) => {
                coefficient_stack.extend([a, b]);
            }
            CoeffOp::Neg(value) => coefficient_stack.push(value),
            CoeffOp::ConstF64(_)
            | CoeffOp::Camera(_)
            | CoeffOp::ScalarParamDerivative(_, _)
            | CoeffOp::ParamRate(_, _) => {}
        }
    }

    let scalars = scalar_dependency_closure(renderer, scalar_stack)?;
    Ok((coefficients, scalars))
}

fn write_scalar_evaluator(
    output: &mut String,
    renderer: &super::CompiledRenderer,
    required: &BTreeSet<usize>,
    with_coordinates: bool,
    with_surface_normal: bool,
) -> Result<(), String> {
    use super::scalar::ScalarOp;
    for (id, node) in renderer.symbolic.scalar.iter() {
        if !required.contains(&id.index()) {
            continue;
        }
        let destination = scalar_slot(id);
        let scalar = |id| scalar_slot(id);
        match &node.op {
            ScalarOp::ConstF32(bits) => writeln!(
                output,
                "    {destination} = {}",
                wrela_f32_literal(f32::from_bits(*bits))?
            ),
            ScalarOp::ConstF64(bits) => writeln!(
                output,
                "    {destination} = {}",
                wrela_f32_literal(f64::from_bits(*bits) as f32)?
            ),
            ScalarOp::Param(param) => {
                writeln!(output, "    {destination} = params[{}]", param.index())
            }
            ScalarOp::Add(a, b) => {
                writeln!(
                    output,
                    "    {destination} = {} + {}",
                    scalar(*a),
                    scalar(*b)
                )
            }
            ScalarOp::Sub(a, b) => {
                writeln!(
                    output,
                    "    {destination} = {} - {}",
                    scalar(*a),
                    scalar(*b)
                )
            }
            ScalarOp::Mul(a, b) => {
                writeln!(
                    output,
                    "    {destination} = {} * {}",
                    scalar(*a),
                    scalar(*b)
                )
            }
            ScalarOp::Div(a, b) => {
                writeln!(
                    output,
                    "    {destination} = {} / {}",
                    scalar(*a),
                    scalar(*b)
                )
            }
            ScalarOp::Neg(value) => {
                writeln!(output, "    {destination} = -{}", scalar(*value))
            }
            ScalarOp::Abs(value) => writeln!(
                output,
                "    {destination} = __wrela_pixels_p7_abs({})",
                scalar(*value)
            ),
            ScalarOp::Min(a, b) => writeln!(
                output,
                "    {destination} = __wrela_pixels_p7_min({}, {})",
                scalar(*a),
                scalar(*b)
            ),
            ScalarOp::Max(a, b) => writeln!(
                output,
                "    {destination} = __wrela_pixels_p7_max({}, {})",
                scalar(*a),
                scalar(*b)
            ),
            ScalarOp::Clamp { value, lo, hi } => writeln!(
                output,
                "    {destination} = __wrela_pixels_p7_clamp({}, {}, {})",
                scalar(*value),
                scalar(*lo),
                scalar(*hi)
            ),
            ScalarOp::Sqrt(value, _) => {
                writeln!(
                    output,
                    "    {destination} = sqrt_scalar({})",
                    scalar(*value)
                )
            }
            ScalarOp::Rsqrt(value, _) => {
                writeln!(
                    output,
                    "    {destination} = rsqrt_scalar({})",
                    scalar(*value)
                )
            }
            ScalarOp::SinRestricted(value, _) => {
                writeln!(output, "    {destination} = sin_scalar({})", scalar(*value))
            }
            ScalarOp::CosRestricted(value, _) => {
                writeln!(output, "    {destination} = cos_scalar({})", scalar(*value))
            }
            ScalarOp::Dot3(a, b) => writeln!(
                output,
                "    {destination} = {} * {} + {} * {} + {} * {}",
                scalar(a[0]),
                scalar(b[0]),
                scalar(a[1]),
                scalar(b[1]),
                scalar(a[2]),
                scalar(b[2])
            ),
            ScalarOp::Cross3Component { component, a, b } => {
                let (a0, b0, a1, b1) = match component {
                    0 => (a[1], b[2], a[2], b[1]),
                    1 => (a[2], b[0], a[0], b[2]),
                    2 => (a[0], b[1], a[1], b[0]),
                    _ => {
                        return Err("pixels::glue: generated cross-product component is invalid"
                            .to_string());
                    }
                };
                writeln!(
                    output,
                    "    {destination} = {} * {} - {} * {}",
                    scalar(a0),
                    scalar(b0),
                    scalar(a1),
                    scalar(b1)
                )
            }
            ScalarOp::Length2(value) => writeln!(
                output,
                "    {destination} = sqrt_scalar({0} * {0} + {1} * {1})",
                scalar(value[0]),
                scalar(value[1])
            ),
            ScalarOp::Length3(value) => writeln!(
                output,
                "    {destination} = sqrt_scalar({0} * {0} + {1} * {1} + {2} * {2})",
                scalar(value[0]),
                scalar(value[1]),
                scalar(value[2])
            ),
            ScalarOp::Normalize3Component {
                component, value, ..
            } => writeln!(
                output,
                "    {destination} = __wrela_pixels_p7_normalize_component({}, {}, {}, {})",
                scalar(value[0]),
                scalar(value[1]),
                scalar(value[2]),
                component
            ),
            ScalarOp::Compare { op, a, b } => {
                use super::scalar::CompareOp;
                let operator = match op {
                    CompareOp::Lt => "<",
                    CompareOp::Le => "<=",
                    CompareOp::Gt => ">",
                    CompareOp::Ge => ">=",
                    CompareOp::Eq => "==",
                    CompareOp::Ne => "!=",
                };
                writeln!(output, "    {destination} = 0.0").expect("String writes cannot fail");
                writeln!(output, "    if {} {operator} {}:", scalar(*a), scalar(*b))
                    .expect("String writes cannot fail");
                writeln!(output, "        {destination} = 1.0")
            }
            ScalarOp::Select { predicate, a, b } => {
                writeln!(output, "    {destination} = {}", scalar(*b))
                    .expect("String writes cannot fail");
                writeln!(output, "    if {} != 0.0:", scalar(*predicate))
                    .expect("String writes cannot fail");
                writeln!(output, "        {destination} = {}", scalar(*a))
            }
            ScalarOp::SelectIndex { index, options } => {
                writeln!(output, "    {destination} = 0.0").expect("String writes cannot fail");
                for (option, value) in options.iter().enumerate() {
                    writeln!(
                        output,
                        "    if {} == {}.0:\n        {destination} = {}",
                        scalar(*index),
                        option,
                        scalar(*value)
                    )
                    .expect("String writes cannot fail");
                }
                Ok(())
            }
            ScalarOp::SmoothMin { a, b, k, .. } => writeln!(
                output,
                "    {destination} = __wrela_pixels_p7_smooth_min({}, {}, {})",
                scalar(*a),
                scalar(*b),
                scalar(*k)
            ),
            ScalarOp::FiniteOr {
                value, fallback, ..
            } => writeln!(
                output,
                "    {destination} = __wrela_pixels_p7_finite_or({}, {})",
                scalar(*value),
                scalar(*fallback)
            ),
            ScalarOp::MaterialRoughness { value, .. } => writeln!(
                output,
                "    {destination} = __wrela_pixels_p7_clamp({}, 0.0, 1.0)",
                scalar(*value)
            ),
            ScalarOp::CoordX if with_coordinates => writeln!(output, "    {destination} = p_x"),
            ScalarOp::CoordY if with_coordinates => writeln!(output, "    {destination} = p_y"),
            ScalarOp::CoordZ if with_coordinates => writeln!(output, "    {destination} = p_z"),
            ScalarOp::SurfacePosition(0) if with_coordinates => {
                writeln!(output, "    {destination} = p_x")
            }
            ScalarOp::SurfacePosition(1) if with_coordinates => {
                writeln!(output, "    {destination} = p_y")
            }
            ScalarOp::SurfacePosition(2) if with_coordinates => {
                writeln!(output, "    {destination} = p_z")
            }
            ScalarOp::SurfaceNormal(0) if with_surface_normal => {
                writeln!(output, "    {destination} = n_x")
            }
            ScalarOp::SurfaceNormal(1) if with_surface_normal => {
                writeln!(output, "    {destination} = n_y")
            }
            ScalarOp::SurfaceNormal(2) if with_surface_normal => {
                writeln!(output, "    {destination} = n_z")
            }
            ScalarOp::CoordX
            | ScalarOp::CoordY
            | ScalarOp::CoordZ
            | ScalarOp::SurfacePosition(_)
            | ScalarOp::SurfaceNormal(_) => {
                writeln!(output, "    {destination} = 0.0")
            }
        }
        .map_err(|_| "pixels::glue: P7 scalar source formatting failed".to_string())?;
    }
    Ok(())
}

fn write_p9_material_return(
    output: &mut String,
    renderer: &super::CompiledRenderer,
    material: super::ids::MaterialId,
    identity: &super::graph::CanonicalIdentity,
    indent: &str,
) -> Result<(), String> {
    use super::material_graph::MaterialKind;
    let node = renderer.symbolic.materials.get(material)?;
    match &node.kind {
        MaterialKind::Sample(sample) => {
            let scalar = |id| scalar_slot(id);
            let texture = sample
                .pattern
                .as_ref()
                .map_or(-1.0_f32, |asset| asset.stable_id as f32);
            let (normal, slope_x, slope_y, normal_texture, normal_filter, normal_uv) =
                match &sample.normal {
                    super::material_graph::NormalModel::Geometric => (
                        0.0_f32,
                        "0.0".to_string(),
                        "0.0".to_string(),
                        -1.0_f32,
                        -1.0_f32,
                        8.0_f32,
                    ),
                    super::material_graph::NormalModel::AnalyticSlope { x, y } => {
                        (1.0_f32, scalar(*x), scalar(*y), -1.0_f32, -1.0_f32, 8.0_f32)
                    }
                    super::material_graph::NormalModel::TextureSlope { texture } => {
                        let filter = match texture.filter {
                            super::material_graph::TextureFilterV1::Nearest => 0.0,
                            super::material_graph::TextureFilterV1::Bilinear => 1.0,
                            super::material_graph::TextureFilterV1::Trilinear => 2.0,
                            super::material_graph::TextureFilterV1::Anisotropic4 => 3.0,
                        };
                        (
                            2.0_f32,
                            "0.0".to_string(),
                            "0.0".to_string(),
                            texture.stable_id as f32,
                            filter,
                            texture.uv_source.tag() as f32,
                        )
                    }
                };
            let pattern_filter =
                sample
                    .pattern
                    .as_ref()
                    .map_or(-1.0_f32, |asset| match asset.filter {
                        super::material_graph::TextureFilterV1::Nearest => 0.0,
                        super::material_graph::TextureFilterV1::Bilinear => 1.0,
                        super::material_graph::TextureFilterV1::Trilinear => 2.0,
                        super::material_graph::TextureFilterV1::Anisotropic4 => 3.0,
                    });
            writeln!(
                output,
                "{indent}return [1.0, {}, {}, {}, {}, {}, {}, {}, {}, {}, {}, {}, {}, {}, {}, {}, {}, {}, {}, {}, {}]",
                scalar(sample.base_color[0]),
                scalar(sample.base_color[1]),
                scalar(sample.base_color[2]),
                scalar(sample.metallic),
                scalar(sample.roughness),
                scalar(sample.specular_level),
                scalar(sample.emissive[0]),
                scalar(sample.emissive[1]),
                scalar(sample.emissive[2]),
                scalar(sample.opacity),
                wrela_f32_literal(texture)?,
                wrela_f32_literal(normal)?,
                scalar(sample.ior),
                slope_x,
                slope_y,
                wrela_f32_literal(normal_texture)?,
                wrela_f32_literal(pattern_filter)?,
                wrela_f32_literal(sample.pattern.as_ref().map_or(8.0, |asset| asset.uv_source.tag() as f32))?,
                wrela_f32_literal(normal_filter)?,
                wrela_f32_literal(normal_uv)?,
            )
            .map_err(|_| "pixels::glue: P9 material return formatting failed".to_string())?;
        }
        MaterialKind::Select { predicate, a, b } => {
            writeln!(output, "{indent}if {} != 0.0:", scalar_slot(*predicate))
                .map_err(|_| "pixels::glue: P9 material select formatting failed".to_string())?;
            write_p9_material_return(output, renderer, *a, identity, &format!("{indent}    "))?;
            write_p9_material_return(output, renderer, *b, identity, indent)?;
        }
        MaterialKind::IdentityTable { enum_key, cases } => {
            let selected = cases
                .iter()
                .find(|(candidate, _)| candidate == identity)
                .ok_or_else(|| {
                    format!(
                        "P024: material identity table `{enum_key}` has no case for {}::{}",
                        identity.enum_key, identity.variant,
                    )
                })?;
            write_p9_material_return(output, renderer, selected.1, identity, indent)?;
        }
    }
    Ok(())
}

fn write_p9_material_evaluator(
    output: &mut String,
    compiled: &[super::CompiledRenderer],
) -> Result<(), String> {
    for (renderer_index, renderer) in compiled.iter().enumerate() {
        use super::material_graph::MaterialKind;
        let mut roots = Vec::new();
        for (_, node) in renderer.symbolic.materials.iter() {
            if let MaterialKind::Sample(sample) = &node.kind {
                roots.extend(sample.base_color);
                roots.extend(sample.emissive);
                roots.extend([
                    sample.opacity,
                    sample.roughness,
                    sample.metallic,
                    sample.specular_level,
                    sample.ior,
                ]);
                if let super::material_graph::NormalModel::AnalyticSlope { x, y } = sample.normal {
                    roots.extend([x, y]);
                }
            } else if let MaterialKind::Select { predicate, .. } = node.kind {
                roots.push(predicate);
            }
        }
        let required = scalar_dependency_closure(renderer, roots)?;
        writeln!(
            output,
            "\npub fn __wrela_pixels_p9_material_r{renderer_index}(identity: u32, read surface: [f32; 6], read params: [f32; 16], read camera: [f32; 12]) -> [f32; 21]:\n    u = surface[0]\n    v = surface[1]\n    q = surface[2]\n    n_x = surface[3]\n    n_y = surface[4]\n    n_z = surface[5]\n    if not q > 0.0:\n        return [0.0; 21]\n    p_x = camera[0] + (camera[3] + u * camera[6] + v * camera[9]) / q\n    p_y = camera[1] + (camera[4] + u * camera[7] + v * camera[10]) / q\n    p_z = camera[2] + (camera[5] + u * camera[8] + v * camera[11]) / q"
        )
        .map_err(|_| "pixels::glue: P9 material function formatting failed".to_string())?;
        for scalar in &required {
            writeln!(output, "    __p7_scalar_{scalar}: f32 = 0.0")
                .expect("String writes cannot fail");
        }
        write_scalar_evaluator(output, renderer, &required, true, true)?;
        for identity_set in &renderer.structural.program().objects.identities {
            let Some(first) = identity_set.pairs.first() else {
                return Err("P024: empty material identity set".to_string());
            };
            if identity_set
                .pairs
                .iter()
                .any(|pair| pair.material != first.material)
            {
                // A regular run may not guess one member of an unresolved
                // material identity set. Event coverage supplies side runs.
                continue;
            }
            writeln!(output, "    if identity == {}:", identity_set.id)
                .expect("String writes cannot fail");
            write_p9_material_return(
                output,
                renderer,
                renderer.symbolic.material_root,
                &first.material,
                "        ",
            )?;
        }
        output.push_str("    return [0.0; 21]\n");
    }
    output.push_str("\npub fn __wrela_pixels_p9_material(renderer: usize, identity: u32, read surface: [f32; 6], read params: [f32; 16], read camera: [f32; 12]) -> [f32; 21]:\n");
    for renderer_index in 0..compiled.len() {
        writeln!(output, "    if renderer == {renderer_index}:\n        return __wrela_pixels_p9_material_r{renderer_index}(identity, surface, params, camera)")
            .expect("String writes cannot fail");
    }
    output.push_str("    return [0.0; 21]\n");

    // AO evaluates the active semantic field at each normal-distance tap. A
    // secondary segment answers visibility, not distance, and cannot stand in
    // for this program (a nearby parallel surface need not intersect the
    // normal segment). The returned interval includes a deterministic f32
    // evaluation allowance and is consumed with reversed AO endpoints.
    for (renderer_index, renderer) in compiled.iter().enumerate() {
        let root = renderer
            .symbolic
            .fields
            .get(renderer.symbolic.field_root)?
            .scalar_value;
        let required = scalar_dependency_closure(renderer, vec![root])?;
        // Bound the complete generated f32 dependency program, not merely the
        // final distance magnitude. Cancellation can make a small result from
        // large intermediates, so a result-relative allowance is unsound.
        // Each generated scalar instruction contributes a conservative 16-ulp
        // allowance at its compiler-proved magnitude; summing in f64 and then
        // rounding the final constant upward gives the guest one fixed,
        // auditable absolute error premise.
        let mut distance_error_f64 = 0.0_f64;
        for scalar in &required {
            let scalar = super::ids::ScalarId(u32::try_from(*scalar).map_err(|_| {
                "pixels::glue: P9 scene-distance scalar index exceeds u32".to_string()
            })?);
            let range = renderer.structural.program().values.get(scalar)?;
            let magnitude = range.lo.abs().max(range.hi.abs());
            if !magnitude.is_finite() {
                return Err(
                    "pixels::glue: non-finite P9 scene-distance dependency range".to_string(),
                );
            }
            distance_error_f64 += magnitude * f64::from(f32::EPSILON) * 16.0;
        }
        if !distance_error_f64.is_finite() || distance_error_f64 > f64::from(f32::MAX) {
            return Err("pixels::glue: P9 scene-distance error bound overflow".to_string());
        }
        let mut distance_error = distance_error_f64 as f32;
        if f64::from(distance_error) < distance_error_f64 {
            distance_error = f32::from_bits(distance_error.to_bits() + 1);
        }
        writeln!(
            output,
            "\npub fn __wrela_pixels_p9_scene_distance_r{renderer_index}(read point: [f32; 3], read params: [f32; 16]) -> [f32; 4]:\n    p_x = point[0]\n    p_y = point[1]\n    p_z = point[2]\n    u: f32 = 0.0\n    v: f32 = 0.0\n    q: f32 = 1.0\n    n_x: f32 = 0.0\n    n_y: f32 = 0.0\n    n_z: f32 = 1.0\n    camera: [f32; 12] = [0.0; 12]"
        )
        .map_err(|_| "pixels::glue: P9 scene-distance formatting failed".to_string())?;
        for scalar in &required {
            writeln!(output, "    __p7_scalar_{scalar}: f32 = 0.0")
                .expect("String writes cannot fail");
        }
        write_scalar_evaluator(output, renderer, &required, true, false)?;
        writeln!(
            output,
            "    distance = {}\n    if distance != distance or distance > 3.4028234663852886e38 or distance < -3.4028234663852886e38:\n        return [0.0; 4]\n    error: f32 = {}\n    return [1.0, distance - error, distance + error, distance]",
            scalar_slot(root),
            wrela_f32_literal(distance_error)?,
        )
        .map_err(|_| "pixels::glue: P9 scene-distance return formatting failed".to_string())?;
    }
    output.push_str("\npub fn __wrela_pixels_p9_scene_distance(renderer: usize, read point: [f32; 3], read params: [f32; 16]) -> [f32; 4]:\n");
    for renderer_index in 0..compiled.len() {
        writeln!(output, "    if renderer == {renderer_index}:\n        return __wrela_pixels_p9_scene_distance_r{renderer_index}(point, params)")
            .expect("String writes cannot fail");
    }
    output.push_str("    return [0.0; 4]\n");
    output.push_str(
        "\npub fn __wrela_pixels_p9_material_inputs(renderer: usize, identity: u32) -> [u64; 2]:\n",
    );
    for (renderer_index, renderer) in compiled.iter().enumerate() {
        let summaries = renderer
            .program
            .program()
            .tables
            .iter()
            .find(|table| {
                table.kind == wrela_machine::pixels::FrameProgramTableKindV1::ShadingSummary
            })
            .ok_or_else(|| "pixels::glue: missing P9 shading-summary table".to_string())?;
        let mut inputs_by_identity = std::collections::BTreeMap::<u64, u64>::new();
        for record in summaries.records.iter().filter(|record| record.tag == 1) {
            if record.operands.len() < 3 {
                return Err(format!(
                    "pixels::glue: P9 material summary {} has fewer than three operands",
                    record.stable_id
                ));
            }
            *inputs_by_identity.entry(record.operands[0]).or_default() |= record.operands[2];
        }
        for (identity, inputs) in inputs_by_identity {
            writeln!(
                output,
                "    if renderer == {renderer_index} and identity == {identity}:\n        return [1, {inputs}]"
            )
            .expect("String writes cannot fail");
        }
    }
    output.push_str("    return [0, 0]\n");
    output.push_str(
        "\npub fn __wrela_pixels_p9_light_range(renderer: usize, slot: usize) -> [f32; 12]:\n",
    );
    for (renderer_index, renderer) in compiled.iter().enumerate() {
        for (slot, range) in renderer.config.light_ranges.iter().enumerate() {
            writeln!(
                output,
                "    if renderer == {renderer_index} and slot == {slot}:\n        return [1.0, {}, {}, {}, {}, {}, {}, {}, {}, {}, {}, {}]",
                wrela_f32_literal(range.position_min.x)?,
                wrela_f32_literal(range.position_min.y)?,
                wrela_f32_literal(range.position_min.z)?,
                wrela_f32_literal(range.position_max.x)?,
                wrela_f32_literal(range.position_max.y)?,
                wrela_f32_literal(range.position_max.z)?,
                wrela_f32_literal(range.axis_component_max)?,
                wrela_f32_literal(range.radiance_max[0])?,
                wrela_f32_literal(range.radiance_max[1])?,
                wrela_f32_literal(range.radiance_max[2])?,
                wrela_f32_literal(range.max_delta)?,
            )
            .expect("String writes cannot fail");
        }
    }
    output.push_str("    return [0.0; 12]\n");
    output.push_str("\npub fn __wrela_pixels_p9_ao_config(renderer: usize) -> [f32; 3]:\n");
    for (renderer_index, renderer) in compiled.iter().enumerate() {
        writeln!(
            output,
            "    if renderer == {renderer_index}:\n        return [{}, {}, {}]",
            if renderer.config.ao_enabled {
                "1.0"
            } else {
                "0.0"
            },
            wrela_f32_literal(renderer.config.ao_radius)?,
            wrela_f32_literal(renderer.config.ao_strength)?,
        )
        .expect("String writes cannot fail");
    }
    output.push_str("    return [0.0; 3]\n");
    Ok(())
}

fn write_p9_transfer_tables(
    output: &mut String,
    compiled: &[super::CompiledRenderer],
) -> Result<(), String> {
    if compiled.is_empty() {
        output.push_str(
            "\npub fn __wrela_pixels_p9_round_ratio(numerator: u64, denominator: u64) -> u64:\n\
             \x20   if denominator == 0:\n\
             \x20       return 0\n\
             \x20   quotient = numerator / denominator\n\
             \x20   remainder = numerator % denominator\n\
             \x20   twice = remainder * 2\n\
             \x20   if twice > denominator or (twice == denominator and quotient % 2 == 1):\n\
             \x20       return quotient + 1\n\
             \x20   return quotient\n\
             \n\
             pub fn __wrela_pixels_p9_table_value(index: u64, filmic: bool) -> [u64; 2]:\n\
             \x20   return [0, 0]\n\
             \n\
             pub fn __wrela_pixels_p9_lut_interpolate(coordinate: u64, fraction_bits: u32, filmic: bool) -> [u64; 2]:\n\
             \x20   return [0, 0]\n\
             \n\
             pub fn __wrela_pixels_p9_filmic_tone(value: f32) -> [u64; 2]:\n\
             \x20   return [0, 0]\n\
             \n\
             pub fn __wrela_pixels_p9_encode_common(value: f32, filmic: bool) -> [u64; 2]:\n\
             \x20   return [0, 0]\n\
             \n\
             pub fn __wrela_pixels_p9_encode(renderer: usize, value: f32) -> [u64; 2]:\n\
             \x20   return [0, 0]\n",
        );
        return Ok(());
    }
    let shared = compiled
        .first()
        .ok_or_else(|| "pixels::glue: P9 transfer tables require a renderer".to_string())?
        .program
        .program()
        .tables
        .iter()
        .find(|table| table.kind == wrela_machine::pixels::FrameProgramTableKindV1::ShadingSummary)
        .ok_or_else(|| "pixels::glue: missing P9 shading-summary table".to_string())?;
    let record_id = |tag| {
        shared
            .records
            .iter()
            .find(|record| record.tag == tag)
            .map(|record| record.stable_id)
            .ok_or_else(|| format!("pixels::glue: missing P9 transfer-table tag {tag}"))
    };
    let filmic_record = record_id(4)?;
    let srgb_record = record_id(5)?;
    writeln!(
        output,
        "\nconst __WRELA_PIXELS_P9_FILMIC_RECORD: u32 = {filmic_record}\nconst __WRELA_PIXELS_P9_SRGB_RECORD: u32 = {srgb_record}"
    )
    .expect("String writes cannot fail");
    output.push_str(
        "\npub fn __wrela_pixels_p9_round_ratio(numerator: u64, denominator: u64) -> u64:\n\
         \x20   quotient = numerator / denominator\n\
         \x20   remainder = numerator % denominator\n\
         \x20   twice = remainder * 2\n\
         \x20   if twice > denominator or (twice == denominator and quotient % 2 == 1):\n\
         \x20       return quotient + 1\n\
         \x20   return quotient\n\
         \n\
         pub fn __wrela_pixels_p9_table_value(index: u64, filmic: bool) -> [u64; 2]:\n\
         \x20   if index > 4096:\n\
         \x20       return [0, 0]\n\
         \x20   record = __WRELA_PIXELS_P9_SRGB_RECORD\n\
         \x20   if filmic:\n\
         \x20       record = __WRELA_PIXELS_P9_FILMIC_RECORD\n\
         \x20   packed = __wrela_pixels_program_operand(0, 13, record, (5 + index / 4).to[u16]())\n\
         \x20   if packed[0] != 1:\n\
         \x20       return [0, 0]\n\
         \x20   shift = (index % 4) * 16\n\
         \x20   return [1, (packed[1] >> shift) & 65535]\n\
         \n\
         pub fn __wrela_pixels_p9_lut_interpolate(coordinate: u64, fraction_bits: u32, filmic: bool) -> [u64; 2]:\n\
         \x20   index = coordinate >> fraction_bits.to[u64]()\n\
         \x20   table_last: u64 = 4095\n\
         \x20   table_last = table_last + 1\n\
         \x20   if index >= table_last:\n\
         \x20       return __wrela_pixels_p9_table_value(table_last, filmic)\n\
         \x20   mask = (1.to[u64]() << fraction_bits.to[u64]()) - 1\n\
         \x20   fraction = coordinate & mask\n\
         \x20   scale = 1.to[u64]() << fraction_bits.to[u64]()\n\
         \x20   low = __wrela_pixels_p9_table_value(index, filmic)\n\
         \x20   high = __wrela_pixels_p9_table_value(index + 1, filmic)\n\
         \x20   if low[0] != 1 or high[0] != 1:\n\
         \x20       return [0, 0]\n\
         \x20   value = __wrela_pixels_p9_round_ratio(low[1] * scale + (high[1] - low[1]) * fraction, scale)\n\
         \x20   return [1, value]\n\
         \n\
         pub fn __wrela_pixels_p9_filmic_tone(value: f32) -> [u64; 2]:\n\
         \x20   bits = __wrela_pixels_f32_to_bits(value)\n\
         \x20   if bits & 2147483648 != 0 or bits & 2139095040 == 2139095040:\n\
         \x20       if bits & 2139095040 == 2139095040:\n\
         \x20           return [0, 0]\n\
         \x20       return [1, 0]\n\
         \x20   exponent_bits = (bits >> 23) & 255\n\
         \x20   mantissa = (bits & 8388607).to[u64]()\n\
         \x20   if exponent_bits == 0 and mantissa == 0:\n\
         \x20       return [1, 0]\n\
         \x20   exponent: i64 = exponent_bits.to[i64]() - 127\n\
         \x20   if exponent_bits == 0:\n\
         \x20       exponent = -126\n\
         \x20       @budget(bound=23)\n\
         \x20       while mantissa < 8388608:\n\
         \x20           mantissa = mantissa << 1\n\
         \x20           exponent = exponent - 1\n\
         \x20   else:\n\
         \x20       mantissa = mantissa | 8388608\n\
         \x20   fraction: u64 = 0\n\
         \x20   step: u32 = 0\n\
         \x20   @budget(bound=15)\n\
         \x20   while step < 15:\n\
         \x20       squared = (mantissa * mantissa) >> 23\n\
         \x20       if squared >= 16777216:\n\
         \x20           mantissa = squared >> 1\n\
         \x20           fraction = fraction | (1.to[u64]() << (14 - step).to[u64]())\n\
         \x20       else:\n\
         \x20           mantissa = squared\n\
         \x20       step = step + 1\n\
         \x20   log_q15 = exponent * 32768 + fraction.to[i64]()\n\
         \x20   coordinate_q8: u64 = 0\n\
         \x20   if log_q15 >= 524288:\n\
         \x20       coordinate_q8 = 1048576\n\
         \x20   elif log_q15 > -524288:\n\
         \x20       coordinate_q8 = (log_q15 + 524288).to[u64]()\n\
         \x20   return __wrela_pixels_p9_lut_interpolate(coordinate_q8, 8, true)\n\
         \n\
         pub fn __wrela_pixels_p9_encode_common(value: f32, filmic: bool) -> [u64; 2]:\n\
         \x20   tone: u16 = 0\n\
         \x20   if filmic:\n\
         \x20       result = __wrela_pixels_p9_filmic_tone(value)\n\
         \x20       if result[0] != 1:\n\
         \x20           return [0, 0]\n\
         \x20       tone = result[1].to[u16]()\n\
         \x20   else:\n\
         \x20       if value != value or value > 3.4028234663852886e38 or value < -3.4028234663852886e38:\n\
         \x20           return [0, 0]\n\
         \x20       clamped = value\n\
         \x20       if clamped < 0.0:\n\
         \x20           clamped = 0.0\n\
         \x20       if clamped > 1.0:\n\
         \x20           clamped = 1.0\n\
         \x20       tone = (clamped * 65535.0 + 0.5).to[u16]()\n\
         \x20   table_steps: u64 = 4095\n\
         \x20   table_steps = table_steps + 1\n\
         \x20   srgb_coordinate_q16 = __wrela_pixels_p9_round_ratio(tone.to[u64]() * table_steps * 65536, 65535)\n\
         \x20   encoded = __wrela_pixels_p9_lut_interpolate(srgb_coordinate_q16, 16, false)\n\
         \x20   if encoded[0] != 1:\n\
         \x20       return [0, 0]\n\
         \x20   byte = __wrela_pixels_p9_round_ratio(encoded[1] * 255, 65535)\n\
         \x20   return [1, byte]\n\
         \n\
         pub fn __wrela_pixels_p9_encode(renderer: usize, value: f32) -> [u64; 2]:\n",
    );
    for (renderer_index, renderer) in compiled.iter().enumerate() {
        writeln!(
            output,
            "    if renderer == {renderer_index}:\n        return __wrela_pixels_p9_encode_common(value, {})",
            renderer.config.tone_curve == "FilmicV1"
        )
        .expect("String writes cannot fail");
    }
    output.push_str("    return [0, 0]\n");
    Ok(())
}

fn coefficient_evaluator_bounds(
    compiled: &[super::CompiledRenderer],
) -> Result<(usize, u64, usize), String> {
    use super::program::CoeffOp;
    let mut maximum_depth = 1_usize;
    let mut maximum_visits = 1_u64;
    let mut maximum_count = 1_usize;
    for renderer in compiled {
        let nodes = &renderer.projective.program().equations.coefficients.nodes;
        maximum_count = maximum_count.max(nodes.len());
        let mut depths = Vec::<usize>::with_capacity(nodes.len());
        let mut visits = Vec::<u64>::with_capacity(nodes.len());
        for node in nodes {
            let (depth, visit_count) = match node.op {
                CoeffOp::ConstF64(_)
                | CoeffOp::Scalar(_)
                | CoeffOp::Camera(_)
                | CoeffOp::ScalarParamDerivative(_, _)
                | CoeffOp::ParamRate(_, _) => (1, 1),
                CoeffOp::Neg(value) => (
                    depths[value.index()].checked_add(1).ok_or_else(|| {
                        "P015: coefficient evaluator stack depth overflow".to_string()
                    })?,
                    visits[value.index()].checked_add(2).ok_or_else(|| {
                        "P015: coefficient evaluator visit bound overflow".to_string()
                    })?,
                ),
                CoeffOp::Add(a, b) | CoeffOp::Mul(a, b) => (
                    depths[a.index()]
                        .max(depths[b.index()])
                        .checked_add(1)
                        .ok_or_else(|| {
                            "P015: coefficient evaluator stack depth overflow".to_string()
                        })?,
                    visits[a.index()]
                        .checked_add(visits[b.index()])
                        .and_then(|value| value.checked_add(3))
                        .ok_or_else(|| {
                            "P015: coefficient evaluator visit bound overflow".to_string()
                        })?,
                ),
            };
            depths.push(depth);
            visits.push(visit_count);
            maximum_depth = maximum_depth.max(depth);
            maximum_visits = maximum_visits.max(visit_count);
        }
    }
    Ok((maximum_depth, maximum_visits, maximum_count))
}

fn write_sealed_root_polynomial_evaluator(
    output: &mut String,
    compiled: &[super::CompiledRenderer],
) -> Result<(), String> {
    use super::program::CoeffOp;
    use wrela_machine::pixels::FrameProgramTableKindV1;

    let (maximum_depth, maximum_visits, maximum_count) = coefficient_evaluator_bounds(compiled)?;
    let maximum_terms = compiled
        .iter()
        .flat_map(|renderer| {
            let equations = &renderer.projective.program().equations;
            equations.features.iter().flat_map(|feature| {
                std::iter::once(feature.root_equation)
                    .chain(
                        feature
                            .validity_predicates
                            .iter()
                            .map(|predicate| equations.predicates[predicate.index()].polynomial),
                    )
                    .map(|polynomial| equations.polynomials[polynomial.index()].terms.len())
            })
        })
        .max()
        .unwrap_or(0)
        .max(1);
    let maximum_exponent = compiled
        .iter()
        .flat_map(|renderer| {
            let equations = &renderer.projective.program().equations;
            equations.features.iter().flat_map(|feature| {
                std::iter::once(feature.root_equation)
                    .chain(
                        feature
                            .validity_predicates
                            .iter()
                            .map(|predicate| equations.predicates[predicate.index()].polynomial),
                    )
                    .flat_map(|polynomial| {
                        equations.polynomials[polynomial.index()]
                            .terms
                            .iter()
                            .flat_map(|term| {
                                std::iter::once(term.exponents.u)
                                    .chain(std::iter::once(term.exponents.v))
                                    .chain(
                                        term.exponents
                                            .param_terms
                                            .iter()
                                            .map(|parameter| parameter.exponent),
                                    )
                            })
                    })
            })
        })
        .max()
        .unwrap_or(0)
        .max(1);
    let maximum_predicates = compiled
        .iter()
        .flat_map(|renderer| {
            renderer
                .projective
                .program()
                .equations
                .features
                .iter()
                .map(|feature| feature.validity_predicates.len())
        })
        .max()
        .unwrap_or(0)
        .max(1);

    for (renderer_index, renderer) in compiled.iter().enumerate() {
        let equations = &renderer.projective.program().equations;
        let (required_coefficients, required_scalars) = required_polynomial_values(
            renderer,
            equations.features.iter().flat_map(|feature| {
                std::iter::once(feature.root_equation).chain(
                    feature
                        .validity_predicates
                        .iter()
                        .map(|predicate| equations.predicates[predicate.index()].polynomial),
                )
            }),
        )?;
        writeln!(
            output,
            "\npub fn __wrela_pixels_p7_coefficient_scalar_r{renderer_index}(target: u32, read params: [f32; 16]) -> [f32; 2]:"
        )
        .expect("String writes cannot fail");
        for scalar in &required_scalars {
            writeln!(output, "    __p7_scalar_{scalar}: f32 = 0.0")
                .expect("String writes cannot fail");
        }
        write_scalar_evaluator(output, renderer, &required_scalars, false, false)?;
        let scalar_roots = equations
            .coefficients
            .nodes
            .iter()
            .filter(|node| required_coefficients.contains(&node.id.index()))
            .filter_map(|node| match node.op {
                CoeffOp::Scalar(scalar) => Some(scalar),
                _ => None,
            })
            .collect::<BTreeSet<_>>();
        for scalar in scalar_roots {
            writeln!(
                output,
                "    if target == {}:\n        return [1.0, {}]",
                scalar.0,
                scalar_slot(scalar),
            )
            .expect("String writes cannot fail");
        }
        output.push_str("    return [0.0, 0.0]\n");
    }
    output.push_str(
        "\n\
         pub fn __wrela_pixels_p7_coefficient_scalar(renderer: usize, target: u32, read params: [f32; 16]) -> [f32; 2]:\n",
    );
    for renderer_index in 0..compiled.len() {
        writeln!(
            output,
            "    if renderer == {renderer_index}:\n        return __wrela_pixels_p7_coefficient_scalar_r{renderer_index}(target, params)"
        )
        .expect("String writes cannot fail");
    }
    output.push_str("    return [0.0, 0.0]\n");

    output.push_str("\npub fn __wrela_pixels_p7_root_coefficient_count(renderer: usize) -> u32:\n");
    for (renderer_index, renderer) in compiled.iter().enumerate() {
        writeln!(
            output,
            "    if renderer == {renderer_index}:\n        return {}",
            renderer
                .projective
                .program()
                .equations
                .coefficients
                .nodes
                .len()
        )
        .expect("String writes cannot fail");
    }
    output.push_str("    return 4294967295\n");

    for (name, tag) in [
        ("coefficient", 10_u16),
        ("polynomial", 20_u16),
        ("predicate", 22_u16),
    ] {
        writeln!(
            output,
            "\npub fn __wrela_pixels_p7_root_{name}_base(renderer: usize) -> u32:"
        )
        .expect("String writes cannot fail");
        for (renderer_index, renderer) in compiled.iter().enumerate() {
            let fixed = renderer
                .program
                .program()
                .tables
                .iter()
                .find(|table| table.kind == FrameProgramTableKindV1::FixedDomain)
                .ok_or_else(|| "pixels::glue: sealed fixed-domain table is missing".to_string())?;
            let base = fixed
                .records
                .iter()
                .position(|record| {
                    if name == "coefficient" {
                        (10..=17).contains(&record.tag)
                    } else {
                        record.tag == tag
                    }
                })
                .unwrap_or(fixed.records.len());
            writeln!(
                output,
                "    if renderer == {renderer_index}:\n        return {base}"
            )
            .expect("String writes cannot fail");
        }
        output.push_str("    return 4294967295\n");
    }

    output.push_str(
        "\n\
         pub fn __wrela_pixels_p7_root_camera_coefficient(renderer: usize, code: u64, read camera: [f32; 12]) -> [f32; 2]:\n\
         \x20   if code >= 256 and code < 259:\n\
         \x20       return [1.0, camera[(code - 256).to[usize]()]]\n\
         \x20   if code >= 512 and code < 515:\n\
         \x20       return [1.0, camera[(code - 509).to[usize]()]]\n\
         \x20   if code >= 768 and code < 771:\n\
         \x20       return [1.0, camera[(code - 762).to[usize]()]]\n\
         \x20   if code >= 1024 and code < 1027:\n\
         \x20       return [1.0, camera[(code - 1015).to[usize]()]]\n\
         \x20   if code == 2304:\n\
         \x20       return [1.0, 0.57735026]\n",
    );
    for (renderer_index, renderer) in compiled.iter().enumerate() {
        writeln!(
            output,
            "    if renderer == {renderer_index} and code == 2560:\n        return [1.0, {}]",
            wrela_f32_literal(renderer.config.width as f32 / renderer.config.height as f32)?
        )
        .expect("String writes cannot fail");
    }
    output.push_str("    return [0.0, 0.0]\n");

    writeln!(
        output,
        "\npub fn __wrela_pixels_p7_root_coefficient(renderer: usize, target: u32, read params: [f32; 16], read camera: [f32; 12]) -> [f32; 2]:\n\
         \x20   count = __wrela_pixels_p7_root_coefficient_count(renderer)\n\
         \x20   base = __wrela_pixels_p7_root_coefficient_base(renderer)\n\
         \x20   if count == 4294967295 or base == 4294967295 or count > {maximum_count} or target >= count:\n\
         \x20       return [0.0, 0.0]\n\
         \x20   nodes: [u32; {maximum_depth}] = [0; {maximum_depth}]\n\
         \x20   phases: [u8; {maximum_depth}] = [0; {maximum_depth}]\n\
         \x20   left_values: [f32; {maximum_depth}] = [0.0; {maximum_depth}]\n\
         \x20   nodes[0] = target\n\
         \x20   depth: usize = 0\n\
         \x20   last_value: f32 = 0.0\n\
         \x20   @budget(bound={maximum_visits})\n\
         \x20   while true:\n\
         \x20       node = nodes[depth]\n\
         \x20       record_id = base + node\n\
         \x20       if record_id < base:\n\
         \x20           return [0.0, 0.0]\n\
         \x20       record = __wrela_pixels_program_record(renderer, 9, record_id)\n\
         \x20       identity = __wrela_pixels_program_operand(renderer, 9, record_id, 0)\n\
         \x20       if record[0] != 1 or record[1] != record_id.to[u64]() or identity[0] != 1 or identity[1] != node.to[u64]():\n\
         \x20           return [0.0, 0.0]\n\
         \x20       tag = record[2]\n\
         \x20       value: f32 = 0.0\n\
         \x20       if tag == 10:\n\
         \x20           bits = __wrela_pixels_program_operand(renderer, 9, record_id, 1)\n\
         \x20           if bits[0] != 1:\n\
         \x20               return [0.0, 0.0]\n\
         \x20           value = __wrela_pixels_f64_bits_to_f32(bits[1])\n\
         \x20       elif tag == 11:\n\
         \x20           scalar = __wrela_pixels_program_operand(renderer, 9, record_id, 1)\n\
         \x20           if scalar[0] != 1 or scalar[1] > 4294967295:\n\
         \x20               return [0.0, 0.0]\n\
         \x20           evaluated = __wrela_pixels_p7_coefficient_scalar(renderer, scalar[1].to[u32](), params)\n\
         \x20           if evaluated[0] != 1.0:\n\
         \x20               return [0.0, 0.0]\n\
         \x20           value = evaluated[1]\n\
         \x20       elif tag == 12:\n\
         \x20           code = __wrela_pixels_program_operand(renderer, 9, record_id, 1)\n\
         \x20           if code[0] != 1:\n\
         \x20               return [0.0, 0.0]\n\
         \x20           evaluated = __wrela_pixels_p7_root_camera_coefficient(renderer, code[1], camera)\n\
         \x20           if evaluated[0] != 1.0:\n\
         \x20               return [0.0, 0.0]\n\
         \x20           value = evaluated[1]\n\
         \x20       elif tag == 15 or tag == 16:\n\
         \x20           a = __wrela_pixels_program_operand(renderer, 9, record_id, 1)\n\
         \x20           b = __wrela_pixels_program_operand(renderer, 9, record_id, 2)\n\
         \x20           if a[0] != 1 or b[0] != 1 or a[1] >= node.to[u64]() or b[1] >= node.to[u64]():\n\
         \x20               return [0.0, 0.0]\n\
         \x20           if phases[depth] == 0:\n\
         \x20               if depth + 1 >= {maximum_depth}:\n\
         \x20                   return [0.0, 0.0]\n\
         \x20               phases[depth] = 1\n\
         \x20               depth = depth + 1\n\
         \x20               nodes[depth] = a[1].to[u32]()\n\
         \x20               phases[depth] = 0\n\
         \x20               continue\n\
         \x20           if phases[depth] == 1:\n\
         \x20               if depth + 1 >= {maximum_depth}:\n\
         \x20                   return [0.0, 0.0]\n\
         \x20               left_values[depth] = last_value\n\
         \x20               phases[depth] = 2\n\
         \x20               depth = depth + 1\n\
         \x20               nodes[depth] = b[1].to[u32]()\n\
         \x20               phases[depth] = 0\n\
         \x20               continue\n\
         \x20           if phases[depth] != 2:\n\
         \x20               return [0.0, 0.0]\n\
         \x20           if tag == 15:\n\
         \x20               value = left_values[depth] + last_value\n\
         \x20           else:\n\
         \x20               value = left_values[depth] * last_value\n\
         \x20       elif tag == 17:\n\
         \x20           a = __wrela_pixels_program_operand(renderer, 9, record_id, 1)\n\
         \x20           if a[0] != 1 or a[1] >= node.to[u64]():\n\
         \x20               return [0.0, 0.0]\n\
         \x20           if phases[depth] == 0:\n\
         \x20               if depth + 1 >= {maximum_depth}:\n\
         \x20                   return [0.0, 0.0]\n\
         \x20               phases[depth] = 1\n\
         \x20               depth = depth + 1\n\
         \x20               nodes[depth] = a[1].to[u32]()\n\
         \x20               phases[depth] = 0\n\
         \x20               continue\n\
         \x20           if phases[depth] != 1:\n\
         \x20               return [0.0, 0.0]\n\
         \x20           value = -last_value\n\
         \x20       else:\n\
         \x20           return [0.0, 0.0]\n\
         \x20       if not __wrela_pixels_p5_finite(value):\n\
         \x20           return [0.0, 0.0]\n\
         \x20       last_value = value\n\
         \x20       if depth == 0:\n\
         \x20           return [1.0, last_value]\n\
         \x20       depth = depth - 1\n\
         \x20   return [0.0, 0.0]"
    )
    .expect("String writes cannot fail");

    writeln!(
        output,
        "\npub fn __wrela_pixels_p7_power(value: f32, exponent: u64) -> [f32; 2]:\n\
         \x20   if exponent > {maximum_exponent}:\n\
         \x20       return [0.0, 0.0]\n\
         \x20   result: f32 = 1.0\n\
         \x20   count: u64 = 0\n\
         \x20   @budget(bound={maximum_exponent})\n\
         \x20   while count < exponent:\n\
         \x20       result = result * value\n\
         \x20       if not __wrela_pixels_p5_finite(result):\n\
         \x20           return [0.0, 0.0]\n\
         \x20       count = count + 1\n\
         \x20   return [1.0, result]"
    )
    .expect("String writes cannot fail");

    writeln!(
        output,
        "\npub fn __wrela_pixels_p7_root_polynomial(renderer: usize, polynomial: u32, u: f32, v: f32, read params: [f32; 16], read camera: [f32; 12]) -> [f32; 11]:\n\
         \x20   base = __wrela_pixels_p7_root_polynomial_base(renderer)\n\
         \x20   if base == 4294967295:\n\
         \x20       return [0.0; 11]\n\
         \x20   record_id = base + polynomial\n\
         \x20   if record_id < base:\n\
         \x20       return [0.0; 11]\n\
         \x20   record = __wrela_pixels_program_record(renderer, 9, record_id)\n\
         \x20   identity = __wrela_pixels_program_operand(renderer, 9, record_id, 0)\n\
         \x20   term_count = __wrela_pixels_program_operand(renderer, 9, record_id, 1)\n\
         \x20   degree_q = __wrela_pixels_program_operand(renderer, 9, record_id, 4)\n\
         \x20   degree_x = __wrela_pixels_program_operand(renderer, 9, record_id, 5)\n\
         \x20   degree_t = __wrela_pixels_program_operand(renderer, 9, record_id, 6)\n\
         \x20   if record[0] != 1 or record[1] != record_id.to[u64]() or record[2] != 20 or identity[0] != 1 or identity[1] != polynomial.to[u64]() or term_count[0] != 1 or term_count[1] > {maximum_terms} or degree_q[0] != 1 or degree_q[1] > 8 or degree_x[0] != 1 or degree_x[1] != 0 or degree_t[0] != 1 or degree_t[1] != 0:\n\
         \x20       return [0.0; 11]\n\
         \x20   q_values: [f32; 9] = [0.0; 9]\n\
         \x20   ordinal: u64 = 8\n\
         \x20   term: u64 = 0\n\
         \x20   @budget(bound={maximum_terms})\n\
         \x20   while term < term_count[1]:\n\
         \x20       coefficient = __wrela_pixels_program_operand(renderer, 9, record_id, ordinal.to[u16]())\n\
         \x20       exponent_u = __wrela_pixels_program_operand(renderer, 9, record_id, (ordinal + 1).to[u16]())\n\
         \x20       exponent_v = __wrela_pixels_program_operand(renderer, 9, record_id, (ordinal + 2).to[u16]())\n\
         \x20       exponent_q = __wrela_pixels_program_operand(renderer, 9, record_id, (ordinal + 3).to[u16]())\n\
         \x20       exponent_x = __wrela_pixels_program_operand(renderer, 9, record_id, (ordinal + 4).to[u16]())\n\
         \x20       exponent_t = __wrela_pixels_program_operand(renderer, 9, record_id, (ordinal + 5).to[u16]())\n\
         \x20       parameter_count = __wrela_pixels_program_operand(renderer, 9, record_id, (ordinal + 6).to[u16]())\n\
         \x20       if coefficient[0] != 1 or coefficient[1] > 4294967295 or exponent_u[0] != 1 or exponent_v[0] != 1 or exponent_q[0] != 1 or exponent_q[1] > 8 or exponent_x[0] != 1 or exponent_x[1] != 0 or exponent_t[0] != 1 or exponent_t[1] != 0 or parameter_count[0] != 1 or parameter_count[1] > 16:\n\
         \x20           return [0.0; 11]\n\
         \x20       coefficient_value = __wrela_pixels_p7_root_coefficient(renderer, coefficient[1].to[u32](), params, camera)\n\
         \x20       u_power = __wrela_pixels_p7_power(u, exponent_u[1])\n\
         \x20       v_power = __wrela_pixels_p7_power(v, exponent_v[1])\n\
         \x20       if coefficient_value[0] != 1.0 or u_power[0] != 1.0 or v_power[0] != 1.0:\n\
         \x20           return [0.0; 11]\n\
         \x20       value = coefficient_value[1] * u_power[1] * v_power[1]\n\
         \x20       ordinal = ordinal + 7\n\
         \x20       parameter: u64 = 0\n\
         \x20       @budget(bound=16)\n\
         \x20       while parameter < parameter_count[1]:\n\
         \x20           parameter_id = __wrela_pixels_program_operand(renderer, 9, record_id, ordinal.to[u16]())\n\
         \x20           parameter_exponent = __wrela_pixels_program_operand(renderer, 9, record_id, (ordinal + 1).to[u16]())\n\
         \x20           if parameter_id[0] != 1 or parameter_id[1] >= 16 or parameter_exponent[0] != 1:\n\
         \x20               return [0.0; 11]\n\
         \x20           parameter_power = __wrela_pixels_p7_power(params[parameter_id[1].to[usize]()], parameter_exponent[1])\n\
         \x20           if parameter_power[0] != 1.0:\n\
         \x20               return [0.0; 11]\n\
         \x20           value = value * parameter_power[1]\n\
         \x20           ordinal = ordinal + 2\n\
         \x20           parameter = parameter + 1\n\
         \x20       if not __wrela_pixels_p5_finite(value):\n\
         \x20           return [0.0; 11]\n\
         \x20       q_values[exponent_q[1].to[usize]()] = q_values[exponent_q[1].to[usize]()] + value\n\
         \x20       if not __wrela_pixels_p5_finite(q_values[exponent_q[1].to[usize]()]):\n\
         \x20           return [0.0; 11]\n\
         \x20       term = term + 1\n\
         \x20   return [1.0, degree_q[1].to[f32](), q_values[0], q_values[1], q_values[2], q_values[3], q_values[4], q_values[5], q_values[6], q_values[7], q_values[8]]"
    )
    .expect("String writes cannot fail");
    writeln!(
        output,
        "\npub fn __wrela_pixels_p7_sealed_feature_valid(renderer: usize, feature: u32, u: f32, v: f32, q: f32, read params: [f32; 16], read camera: [f32; 12]) -> bool:\n\
         \x20   feature_record = __wrela_pixels_program_record(renderer, 4, feature)\n\
         \x20   predicate_count = __wrela_pixels_program_operand(renderer, 4, feature, 20)\n\
         \x20   predicate_base = __wrela_pixels_p7_root_predicate_base(renderer)\n\
         \x20   if feature_record[0] != 1 or feature_record[1] != feature.to[u64]() or predicate_count[0] != 1 or predicate_count[1] > {maximum_predicates} or predicate_base == 4294967295:\n\
         \x20       return false\n\
         \x20   ordinal: u64 = 0\n\
         \x20   @budget(bound={maximum_predicates})\n\
         \x20   while ordinal < predicate_count[1]:\n\
         \x20       predicate_id = __wrela_pixels_program_operand(renderer, 4, feature, (21 + ordinal).to[u16]())\n\
         \x20       if predicate_id[0] != 1 or predicate_id[1] > 4294967295:\n\
         \x20           return false\n\
         \x20       record_id = predicate_base + predicate_id[1].to[u32]()\n\
         \x20       if record_id < predicate_base:\n\
         \x20           return false\n\
         \x20       predicate_record = __wrela_pixels_program_record(renderer, 9, record_id)\n\
         \x20       identity = __wrela_pixels_program_operand(renderer, 9, record_id, 0)\n\
         \x20       polynomial_id = __wrela_pixels_program_operand(renderer, 9, record_id, 1)\n\
         \x20       sense = __wrela_pixels_program_operand(renderer, 9, record_id, 2)\n\
         \x20       if predicate_record[0] != 1 or predicate_record[1] != record_id.to[u64]() or predicate_record[2] != 22 or identity[0] != 1 or identity[1] != predicate_id[1] or polynomial_id[0] != 1 or polynomial_id[1] > 4294967295 or sense[0] != 1 or sense[1] < 1 or sense[1] > 5:\n\
         \x20           return false\n\
         \x20       polynomial = __wrela_pixels_p7_root_polynomial(renderer, polynomial_id[1].to[u32](), u, v, params, camera)\n\
         \x20       if polynomial[0] != 1.0 or polynomial[1] < 0.0 or polynomial[1] > 8.0:\n\
         \x20           return false\n\
         \x20       degree = polynomial[1].to[i32]()\n\
         \x20       value = polynomial[degree.to[usize]() + 2]\n\
         \x20       @budget(bound=8)\n\
         \x20       while degree > 0:\n\
         \x20           degree = degree - 1\n\
         \x20           value = value * q + polynomial[degree.to[usize]() + 2]\n\
         \x20       if not __wrela_pixels_p5_finite(value):\n\
         \x20           return false\n\
         \x20       if (sense[1] == 1 and value >= 0.0) or (sense[1] == 2 and value > 0.0) or (sense[1] == 3 and __wrela_pixels_p7_abs(value) > 0.000030517578125) or (sense[1] == 4 and value < 0.0) or (sense[1] == 5 and value <= 0.0):\n\
         \x20           return false\n\
         \x20       ordinal = ordinal + 1\n\
         \x20   return true"
    )
    .expect("String writes cannot fail");
    writeln!(
        output,
        "\npub fn __wrela_pixels_p7_sealed_feature_valid_filter(renderer: usize, feature: u32, read uv: [f32; 4], read q: [i32; 2], exponent: i32, read params: [f32; 16], read camera: [f32; 12]) -> [i64; 2]:\n\
         \x20   feature_record = __wrela_pixels_program_record(renderer, 4, feature)\n\
         \x20   predicate_count = __wrela_pixels_program_operand(renderer, 4, feature, 20)\n\
         \x20   predicate_base = __wrela_pixels_p7_root_predicate_base(renderer)\n\
         \x20   if uv[2] < 0.0 or uv[3] < 0.0 or q[0] > q[1] or feature_record[0] != 1 or feature_record[1] != feature.to[u64]() or predicate_count[0] != 1 or predicate_count[1] > {maximum_predicates} or predicate_base == 4294967295:\n\
         \x20       return [0, 0]\n\
         \x20   ambiguous = false\n\
         \x20   ordinal: u64 = 0\n\
         \x20   @budget(bound={maximum_predicates})\n\
         \x20   while ordinal < predicate_count[1]:\n\
         \x20       predicate_id = __wrela_pixels_program_operand(renderer, 4, feature, (21 + ordinal).to[u16]())\n\
         \x20       if predicate_id[0] != 1 or predicate_id[1] > 4294967295:\n\
         \x20           return [0, 0]\n\
         \x20       record_id = predicate_base + predicate_id[1].to[u32]()\n\
         \x20       if record_id < predicate_base:\n\
         \x20           return [0, 0]\n\
         \x20       predicate_record = __wrela_pixels_program_record(renderer, 9, record_id)\n\
         \x20       identity = __wrela_pixels_program_operand(renderer, 9, record_id, 0)\n\
         \x20       polynomial_id = __wrela_pixels_program_operand(renderer, 9, record_id, 1)\n\
         \x20       sense = __wrela_pixels_program_operand(renderer, 9, record_id, 2)\n\
         \x20       if predicate_record[0] != 1 or predicate_record[1] != record_id.to[u64]() or predicate_record[2] != 22 or identity[0] != 1 or identity[1] != predicate_id[1] or polynomial_id[0] != 1 or polynomial_id[1] > 4294967295 or sense[0] != 1 or sense[1] < 1 or sense[1] > 5:\n\
         \x20           return [0, 0]\n\
         \x20       polynomial = __wrela_pixels_p7_root_polynomial(renderer, polynomial_id[1].to[u32](), uv[0], uv[1], params, camera)\n\
         \x20       bounds = __wrela_pixels_p7_feature_predicate_uv_bounds(renderer, feature, ordinal.to[u32]())\n\
         \x20       if polynomial[0] != 1.0 or polynomial[1] < 0.0 or polynomial[1] > 8.0 or bounds[0] != 1.0 or bounds[1] != polynomial[1]:\n\
         \x20           return [0, 0]\n\
         \x20       degree = polynomial[1].to[u8]()\n\
         \x20       # Predicate sign resolution runs in f32, not the raw\n\
         \x20       # fixed-point grid: the coefficients arrive in f32, and\n\
         \x20       # the raw grid's quantization noise (tens of raw units\n\
         \x20       # after a degree-8 Horner walk) is the same order as the\n\
         \x20       # geometric margins of subpixel features, which turned a\n\
         \x20       # measure-zero validity boundary into a wide undecidable\n\
         \x20       # skin. Soundness terms: the uv box radius, a mean-value\n\
         \x20       # derivative bound over the q bracket, and a relative\n\
         \x20       # rounding allowance far above the true f32 error.\n\
         \x20       q_low = __wrela_pixels_p7_raw_to_f32(q[0], exponent)\n\
         \x20       q_high = __wrela_pixels_p7_raw_to_f32(q[1], exponent)\n\
         \x20       q_magnitude = q_low\n\
         \x20       if q_magnitude < 0.0:\n\
         \x20           q_magnitude = 0.0 - q_magnitude\n\
         \x20       q_other = q_high\n\
         \x20       if q_other < 0.0:\n\
         \x20           q_other = 0.0 - q_other\n\
         \x20       if q_other > q_magnitude:\n\
         \x20           q_magnitude = q_other\n\
         \x20       value_low: f32 = 0.0\n\
         \x20       value_high: f32 = 0.0\n\
         \x20       radius_eval: f32 = 0.0\n\
         \x20       magnitude: f32 = 0.0\n\
         \x20       derivative_bound: f32 = 0.0\n\
         \x20       term: usize = degree.to[usize]() + 1\n\
         \x20       @budget(bound=9)\n\
         \x20       while term > 0:\n\
         \x20           term = term - 1\n\
         \x20           coefficient_value = polynomial[term + 2]\n\
         \x20           coefficient_radius = bounds[2 + term * 2] * uv[2] + bounds[3 + term * 2] * uv[3]\n\
         \x20           if coefficient_radius < 0.0:\n\
         \x20               return [0, 0]\n\
         \x20           coefficient_magnitude = coefficient_value\n\
         \x20           if coefficient_magnitude < 0.0:\n\
         \x20               coefficient_magnitude = 0.0 - coefficient_magnitude\n\
         \x20           value_low = value_low * q_low + coefficient_value\n\
         \x20           value_high = value_high * q_high + coefficient_value\n\
         \x20           radius_eval = radius_eval * q_magnitude + coefficient_radius\n\
         \x20           derivative_bound = derivative_bound * q_magnitude + magnitude\n\
         \x20           magnitude = magnitude * q_magnitude + coefficient_magnitude + coefficient_radius\n\
         \x20       spread = derivative_bound * (q_high - q_low)\n\
         \x20       if spread < 0.0:\n\
         \x20           return [0, 0]\n\
         \x20       slack = radius_eval + spread + magnitude * 0.0000152587890625\n\
         \x20       value_minimum = value_low\n\
         \x20       if value_high < value_minimum:\n\
         \x20           value_minimum = value_high\n\
         \x20       value_maximum = value_low\n\
         \x20       if value_high > value_maximum:\n\
         \x20           value_maximum = value_high\n\
         \x20       lower_bound = value_minimum - slack\n\
         \x20       upper_bound = value_maximum + slack\n\
         \x20       if not __wrela_pixels_p5_finite(lower_bound) or not __wrela_pixels_p5_finite(upper_bound):\n\
         \x20           return [0, 0]\n\
         \x20       definitely_valid = false\n\
         \x20       definitely_invalid = false\n\
         \x20       if sense[1] == 1:\n\
         \x20           definitely_valid = upper_bound < 0.0\n\
         \x20           definitely_invalid = lower_bound >= 0.0\n\
         \x20       elif sense[1] == 2:\n\
         \x20           definitely_valid = upper_bound <= 0.0\n\
         \x20           definitely_invalid = lower_bound > 0.0\n\
         \x20       elif sense[1] == 3:\n\
         \x20           definitely_valid = lower_bound >= -0.000030517578125 and upper_bound <= 0.000030517578125\n\
         \x20           definitely_invalid = lower_bound > 0.000030517578125 or upper_bound < -0.000030517578125\n\
         \x20       elif sense[1] == 4:\n\
         \x20           definitely_valid = lower_bound >= 0.0\n\
         \x20           definitely_invalid = upper_bound < 0.0\n\
         \x20       else:\n\
         \x20           definitely_valid = lower_bound > 0.0\n\
         \x20           definitely_invalid = upper_bound <= 0.0\n\
         \x20       if definitely_invalid:\n\
         \x20           return [1, 0]\n\
         \x20       if not definitely_valid:\n\
         \x20           ambiguous = true\n\
         \x20       ordinal = ordinal + 1\n\
         \x20   if ambiguous:\n\
         \x20       return [2, 0]\n\
         \x20   return [1, 1]",
    )
    .expect("String writes cannot fail");
    writeln!(
        output,
        "\npub fn __wrela_pixels_p7_feature_filter_excludes_root(renderer: usize, feature: u32, read uv: [f32; 4], read q: [i32; 2], read params: [f32; 16], read camera: [f32; 12], exponent: i32) -> [i64; 2]:\n\
         \x20   if uv[2] < 0.0 or uv[3] < 0.0 or q[0] > q[1]:\n\
         \x20       return [0, 0]\n\
         \x20   polynomial = __wrela_pixels_p7_feature_polynomial(renderer, feature, uv[0], uv[1], params, camera)\n\
         \x20   bounds = __wrela_pixels_p7_feature_polynomial_uv_bounds(renderer, feature)\n\
         \x20   if polynomial[0] != 1.0 or polynomial[1] < 0.0 or polynomial[1] > 8.0 or bounds[0] != 1.0 or bounds[1] != polynomial[1]:\n\
         \x20       return [0, 0]\n\
         \x20   degree = polynomial[1].to[u8]()\n\
         \x20   power: [Iv32; 9] = [Iv32.point(0); 9]\n\
         \x20   coefficient: usize = 0\n\
         \x20   @budget(bound=9)\n\
         \x20   while coefficient <= degree.to[usize]():\n\
         \x20       radius = bounds[2 + coefficient * 2] * uv[2] + bounds[3 + coefficient * 2] * uv[3]\n\
         \x20       low = __wrela_pixels_p7_interval_from_f32(polynomial[coefficient + 2] - radius, exponent)\n\
         \x20       high = __wrela_pixels_p7_interval_from_f32(polynomial[coefficient + 2] + radius, exponent)\n\
         \x20       if low[0] != 1 or high[0] != 1:\n\
         \x20           return [0, 0]\n\
         \x20       power[coefficient] = Iv32.range(low[1].to[i32](), high[2].to[i32]())\n\
         \x20       coefficient = coefficient + 1\n\
         \x20   match polynomial_horner9(power, degree, Iv32.range(q[0], q[1]), FixedDomain.full(exponent)):\n\
         \x20       case .Value(value):\n\
         \x20           if value.upper() < 0 or value.lower() > 0:\n\
         \x20               return [1, 1]\n\
         \x20           return [1, 0]\n\
         \x20       case _:\n\
         \x20           # This is an optional one-sided exclusion. Arithmetic\n\
         \x20           # exhaustion cannot justify omission, so retain the\n\
         \x20           # feature for complete local root isolation.\n\
         \x20           return [1, 0]",
    )
    .expect("String writes cannot fail");
    Ok(())
}

/// Sealed constants for the conservative deformation-silhouette *miss* test.
///
/// This is deliberately **not** a representation of the silhouette curve. The
/// displaced silhouette has no closed form this tier can integrate; what the
/// model supports is the one-sided question "can this pixel's ray cell be
/// *proved* to contain no silhouette at all?".
pub(crate) struct DeformationMissModel {
    pub feature: super::ids::FeatureId,
    /// `B`: every point of the displaced surface satisfies `|H| <= B`, where
    /// `H(t) = |p(t) - c|^2 - r^2` is the *undisplaced* sphere's ray quadratic.
    pub band: f32,
    /// `B_T`: `B` plus the tangency slack `(r + A)^2 * G^2`.
    pub transversal_band: f32,
    /// Absolute f32 evaluation allowance for ray-quadratic coefficients 0..2.
    pub coefficient_error: [f32; 3],
    /// Domain magnitude bound for `b^2 + 4|a||c| + 4|a| B_T`, which the guest
    /// scales into a rounding slack for the two sign tests.
    pub magnitude: f32,
    /// `(|d2/du2|, |d2/dudv|, |d2/dv2|)` per coefficient, for the residual of
    /// the guest's three-corner bilinear model of each coefficient.
    pub curvature: [[f32; 3]; 3],
    /// `A`: the *global* displacement amplitude, kept separate from `band` so
    /// the guest can rebuild `B` from a locally evaluated amplitude.
    pub amplitude: f32,
    /// Sphere radius `r`, so the guest can re-form `B = A (2r + A)`.
    pub radius: f32,
    /// The displacement is `A sin(f x + phase)` in world `x`; `frequency` is
    /// `f`, emitted only when the sealed interval is a point. A frequency that
    /// varies with a parameter has no single value to localize against.
    pub frequency: f32,
    /// Parameter slot holding the phase at runtime, or `-1` when the phase is
    /// not a plain parameter read.
    pub phase_slot: i32,
    /// World-space sphere center for the direct local coefficient evaluator.
    /// Present only when the occurrence path contains no transform and every
    /// center component is a singleton exactly representable as f32.
    pub direct_center: Option<[f32; 3]>,
    /// Exact positive source amplitude for the direct local classifier. Zero
    /// means the residual shape/value was not a singleton supported form.
    pub direct_amplitude: f32,
}

/// Derive the miss model for a bounded-displacement silhouette over a sphere.
///
/// # What the representation means
///
/// `EventRepresentation::DeformationTaylorPredicate` is a *recompute* event:
/// every side is `EventSideMeaning::RecomputeRootSet`, and the `predictor`
/// polynomial is the **undisplaced** primitive's ray equation — its zero set is
/// not the displaced silhouette. What the sealed program does pin down is:
///
/// * the authored field is `f(p) = f_base(p) + d(p)` (`stdlib/core/field.wr`
///   `sinusoidal_displace`), with `f_base` the exact sphere distance
///   `|p - c| - r`;
/// * `|d| <= A` (`ProjectiveDeformationProgram::value_bound`) and
///   `|grad d| <= G` (`first_derivative_bound`), both certified in world space;
/// * `predictor(u, v, q) = q^2 * H(1/q)` with `H(t) = |p(t) - c|^2 - r^2` and
///   `p(t) = eye + t * (forward + u * right + v * up)`, so the predictor's
///   q-coefficients `(c0, c1, c2)` are exactly `H`'s `(t^2, t^1, t^0)`
///   coefficients `(|w|^2, 2 w.(eye - c), |eye - c|^2 - r^2)`.
///
/// # The two proofs
///
/// A point of the displaced surface has `|p - c| = r - d`, so
/// `|H| = |(r - d)^2 - r^2| <= A (2r + A) = B`. Hence:
///
/// * **Exterior.** If `H > B` for every `t`, no ray point lies on the surface
///   and (since `r^2 + B = (r + A)^2`) every ray point is strictly outside.
///   With `a = |w|^2 > 0`, `min_t H = -D / (4a)`, so this is `D + 4 a B < 0`.
///
///   The guest refines `B` per cell, and the sign convention above is what
///   makes that possible: `H = -d (2r - d)` is *decreasing* in `d`, so over a
///   window where the sine is bounded below by `d_lo` the exterior band is
///   `-d_lo (2r - d_lo)`, which goes negative wherever the wave runs positive
///   across the whole window and pulls the surface inside `r`. At the
///   symmetric extreme `d_lo = -A` it reproduces `B` exactly, so the refined
///   band is always at least as strong and never contradicts the sealed one.
///   Refining from the sine's *upper* end instead would both discard every
///   provable case and admit unsound ones.
/// * **Transversal.** A silhouette is a tangency: `dH/dt / (2 rho) + grad d . w
///   = 0` at a surface point, so `|dH/dt| <= 2 (r + A) G |w|` there. On
///   `|H| <= B` the quadratic obeys `(dH/dt)^2 >= D - 4 a B`. Therefore
///   `D - 4 a (B + (r + A)^2 G^2) > 0` proves no tangency exists on the whole
///   ray line — outer or self-occluding.
///
/// Both are sufficient conditions evaluated over all real `t`, which is
/// stronger than the near/far segment the renderer actually walks, and both
/// fail closed: anything else returns `None` and the guest keeps today's
/// `unclassified_boundary` behaviour.
pub(crate) fn deformation_sphere_miss_model(
    renderer: &super::CompiledRenderer,
    event: &super::events::EventGenerator,
) -> Result<Option<DeformationMissModel>, String> {
    use super::events::{EventRepresentation, Participant};
    use super::graph::{FieldKind, Primitive};
    use super::primitive::AnalyticPredicate;
    use super::reference::interval::{next_up, next_up_f32};

    let EventRepresentation::DeformationTaylorPredicate {
        predictor,
        phase_recurrence,
        ..
    } = &event.representation
    else {
        return Ok(None);
    };
    if event.kind != super::event_kinds::EventKind::Silhouette {
        return Ok(None);
    }
    let mut participants = event.participants.iter();
    let (Some(Participant::Feature(feature_id)), None) = (participants.next(), participants.next())
    else {
        return Ok(None);
    };

    let structural = renderer.structural.program();
    let Some(record) = structural
        .features
        .iter()
        .find(|candidate| candidate.id == feature_id)
    else {
        return Ok(None);
    };
    // Only the exact sphere distance gives `|H| = |(r - d)^2 - r^2|`. Any other
    // predicate has a different value-to-predictor relation, so it stays out.
    let [AnalyticPredicate::Sphere { center, radius }] = record.validity.constraints.as_slice()
    else {
        return Ok(None);
    };
    if !matches!(
        renderer.symbolic.fields.get(record.primitive)?.kind,
        FieldKind::Primitive(Primitive::Sphere { .. })
    ) {
        return Ok(None);
    }
    // The certified displacement bounds are world-space, and `H` is built in
    // the feature's local frame, so the two frames must be isometric. A uniform
    // scale is worse than non-isometric: it rescales the child's distance value
    // without moving coordinates, which changes the effective amplitude. A
    // smooth blend replaces the feature's own zero set outright.
    let mut displacements = 0_u32;
    let mut transforms = 0_u32;
    for step in record.occurrence_path.iter().skip(1) {
        match &renderer.symbolic.fields.get(step.field)?.kind {
            FieldKind::Transform { transform, .. } => {
                if !is_rigid_transform(transform) {
                    return Ok(None);
                }
                transforms += 1;
            }
            FieldKind::BoundedDisplace { .. } => displacements += 1,
            FieldKind::Mark { .. }
            | FieldKind::HardUnion { .. }
            | FieldKind::HardIntersection { .. }
            | FieldKind::HardSubtract { .. } => {}
            _ => return Ok(None),
        }
    }
    if displacements != 1 {
        return Ok(None);
    }

    let projective = renderer.projective.program();
    let Some(deformation) = projective
        .deformations
        .iter()
        .find(|program| program.feature == feature_id)
    else {
        return Ok(None);
    };
    if deformation.predictor != *predictor {
        return Ok(None);
    }
    let Some(lowered) = projective
        .equations
        .features
        .iter()
        .find(|candidate| candidate.feature == feature_id)
    else {
        return Ok(None);
    };
    // A validity predicate would cut the sphere into a partial surface whose
    // rim is a boundary this test says nothing about.
    if lowered.q_degree != 2 || !lowered.validity_predicates.is_empty() {
        return Ok(None);
    }
    if lowered.root_equation != *predictor {
        return Ok(None);
    }

    let radius = structural.values.get(*radius)?;
    if !(radius.lo > 0.0) {
        return Ok(None);
    }
    let radius_abs = radius.hi;
    let amplitude = deformation.value_bound;
    let gradient = deformation.first_derivative_bound;
    if !amplitude.is_finite() || !gradient.is_finite() || amplitude < 0.0 || gradient < 0.0 {
        return Ok(None);
    }
    // B = A (2r + A); rounding every step up only makes both tests stricter.
    let band = next_up(amplitude * next_up(next_up(2.0 * radius_abs) + amplitude));
    let outer = next_up(radius_abs + amplitude);
    let transversal_band =
        next_up(band + next_up(next_up(outer * outer) * next_up(gradient * gradient)));
    if !band.is_finite() || !transversal_band.is_finite() {
        return Ok(None);
    }

    let equations = &projective.equations;
    let polynomial = &equations.polynomials[predictor.index()];
    if polynomial.degree_q != 2 {
        return Ok(None);
    }
    let coefficient_intervals = super::projective::coefficient_intervals_for_roots(
        &equations.coefficients,
        &structural.values,
        equations.camera,
        polynomial.terms.iter().map(|term| term.coefficient),
    )?;
    let u_extent = equations.camera.aspect * equations.camera.tan_half_fov_y;
    let v_extent = equations.camera.tan_half_fov_y;
    let mut magnitudes = [0.0_f64; 3];
    let mut curvature = [[0.0_f64; 3]; 3];
    let mut terms = 0_usize;
    for term in &polynomial.terms {
        if term.exponents.x != 0 || term.exponents.t != 0 || usize::from(term.exponents.q) > 2 {
            return Ok(None);
        }
        let coefficient = coefficient_intervals
            .get(term.coefficient.index())
            .ok_or_else(|| {
                format!(
                    "pixels::glue: deformation miss coefficient {} lacks a verified interval",
                    term.coefficient
                )
            })?;
        let mut magnitude = coefficient.lo.abs().max(coefficient.hi.abs());
        for parameter in term.exponents.param_terms.iter() {
            let slot = structural
                .params
                .slots
                .get(parameter.param.index())
                .ok_or_else(|| {
                    format!(
                        "pixels::glue: deformation miss parameter {} lacks a sealed slot",
                        parameter.param
                    )
                })?;
            magnitude *= slot
                .range
                .min
                .abs()
                .max(slot.range.max.abs())
                .powi(i32::from(parameter.exponent));
        }
        let u_degree = f64::from(term.exponents.u);
        let v_degree = f64::from(term.exponents.v);
        let u_abs = u_extent.abs();
        let v_abs = v_extent.abs();
        let slot = &mut curvature[usize::from(term.exponents.q)];
        if term.exponents.u >= 2 {
            slot[0] = next_up(
                slot[0]
                    + next_up(
                        magnitude
                            * u_degree
                            * (u_degree - 1.0)
                            * u_abs.powi(i32::from(term.exponents.u) - 2)
                            * v_abs.powi(i32::from(term.exponents.v)),
                    ),
            );
        }
        if term.exponents.u >= 1 && term.exponents.v >= 1 {
            slot[1] = next_up(
                slot[1]
                    + next_up(
                        magnitude
                            * u_degree
                            * v_degree
                            * u_abs.powi(i32::from(term.exponents.u) - 1)
                            * v_abs.powi(i32::from(term.exponents.v) - 1),
                    ),
            );
        }
        if term.exponents.v >= 2 {
            slot[2] = next_up(
                slot[2]
                    + next_up(
                        magnitude
                            * v_degree
                            * (v_degree - 1.0)
                            * u_abs.powi(i32::from(term.exponents.u))
                            * v_abs.powi(i32::from(term.exponents.v) - 2),
                    ),
            );
        }
        magnitude *= u_abs.powi(i32::from(term.exponents.u));
        magnitude *= v_abs.powi(i32::from(term.exponents.v));
        magnitudes[usize::from(term.exponents.q)] =
            next_up(magnitudes[usize::from(term.exponents.q)] + next_up(magnitude));
        terms += 1;
    }
    // The generated evaluator rounds once per coefficient-program node on the
    // path to a term, a few times forming the monomial, and once per term in
    // the accumulation. `2^-14` relative is `1024` f32 ulps, so the sizes below
    // keep the allowance a strict over-estimate; anything larger fails closed.
    if terms > 256 || coefficient_intervals.len() > 700 {
        return Ok(None);
    }
    let mut coefficient_error = [0.0_f32; 3];
    for (slot, magnitude) in coefficient_error.iter_mut().zip(magnitudes) {
        *slot = next_up_f32((next_up(magnitude * f64::from(f32::EPSILON)) * 512.0) as f32);
        if !slot.is_finite() {
            return Ok(None);
        }
    }
    let magnitude = next_up(
        next_up(
            next_up(magnitudes[1] * magnitudes[1])
                + next_up(4.0 * next_up(magnitudes[0] * magnitudes[2])),
        ) + next_up(4.0 * next_up(magnitudes[0] * transversal_band)),
    );
    let magnitude = next_up_f32(magnitude as f32);
    let band = next_up_f32(band as f32);
    let transversal_band = next_up_f32(transversal_band as f32);
    if !magnitude.is_finite() || !band.is_finite() || !transversal_band.is_finite() {
        return Ok(None);
    }
    let mut curvature_f32 = [[0.0_f32; 3]; 3];
    for (row, source) in curvature_f32.iter_mut().zip(curvature) {
        for (slot, value) in row.iter_mut().zip(source) {
            *slot = next_up_f32(value as f32);
            if !slot.is_finite() {
                return Ok(None);
            }
        }
    }
    // Localization inputs. `band` above uses the amplitude anywhere on the
    // feature, which near a silhouette is far too pessimistic; the guest can
    // tighten it only when the frequency is one sealed number, the phase is a
    // parameter it can read, and the coordinate really is world `x`. Any other
    // shape emits `-1` and the guest keeps the global amplitude, which is
    // exactly today's answer.
    let frequency = if phase_recurrence.frequency.lo == phase_recurrence.frequency.hi {
        phase_recurrence.frequency.lo as f32
    } else {
        0.0
    };
    let coordinate_is_world_x = matches!(
        renderer.symbolic.scalar.get(phase_recurrence.coordinate_x),
        Ok(node) if node.op == super::scalar::ScalarOp::CoordX
    );
    let phase_slot = match renderer.symbolic.scalar.get(phase_recurrence.phase_scalar) {
        Ok(node) => match node.op {
            super::scalar::ScalarOp::Param(param) => structural
                .params
                .slots
                .iter()
                .position(|slot| slot.id == param)
                .and_then(|index| i32::try_from(index).ok())
                .unwrap_or(-1),
            _ => -1,
        },
        Err(_) => -1,
    };
    let localizable = frequency != 0.0 && frequency.is_finite() && coordinate_is_world_x;
    let direct_center = if transforms == 0 {
        let mut values = [0.0_f32; 3];
        let mut exact = true;
        for (slot, id) in values.iter_mut().zip(center) {
            let interval = structural.values.get(*id)?;
            let value = interval.lo as f32;
            if interval.lo != interval.hi || f64::from(value) != interval.lo {
                exact = false;
                break;
            }
            *slot = value;
        }
        exact.then_some(values)
    } else {
        None
    };
    let direct_amplitude = {
        let mut source = deformation.residual;
        let mut sign = 1.0_f32;
        while let super::scalar::ScalarOp::Neg(inner) = renderer.symbolic.scalar.get(source)?.op {
            source = inner;
            sign = -sign;
        }
        let amplitude = match renderer.symbolic.scalar.get(source)?.op {
            super::scalar::ScalarOp::Mul(amplitude, wave)
                if matches!(
                    renderer.symbolic.scalar.get(wave)?.op,
                    super::scalar::ScalarOp::SinRestricted(_, _)
                ) =>
            {
                Some(amplitude)
            }
            _ => None,
        };
        amplitude
            .and_then(|id| structural.values.get(id).ok())
            .filter(|value| value.lo == value.hi)
            .map(|value| value.lo as f32 * sign)
            .filter(|value| value.is_finite() && *value > 0.0)
            .unwrap_or(0.0)
    };
    Ok(Some(DeformationMissModel {
        feature: feature_id,
        band,
        transversal_band,
        coefficient_error,
        magnitude,
        curvature: curvature_f32,
        amplitude: amplitude as f32,
        radius: radius_abs as f32,
        frequency: if localizable { frequency } else { 0.0 },
        phase_slot: if localizable { phase_slot } else { -1 },
        direct_center,
        direct_amplitude,
    }))
}

fn is_rigid_transform(transform: &super::graph::TransformProgram) -> bool {
    use super::graph::TransformProgram;
    match transform {
        TransformProgram::Translate { .. }
        | TransformProgram::Rotate { .. }
        | TransformProgram::Rigid { .. } => true,
        TransformProgram::UniformScale { .. } => false,
        TransformProgram::SourceRigidSequence { steps, .. }
        | TransformProgram::RigidSequence { steps, .. } => steps.iter().all(is_rigid_transform),
    }
}

/// A near or far clip boundary is not a curve of its own: it is the level set
/// of the owning feature's ray polynomial at the sealed clip `q`. Resolving it
/// to `(root polynomial, q)` lets the coverage integrator treat a clip edge
/// with exactly the same machinery as a silhouette curve, so a pixel straddling
/// the far plane is integrated in closed form instead of being handed to the
/// subpixel walk.
fn clip_event_curve(
    renderer: &super::CompiledRenderer,
    event: &super::events::EventGenerator,
) -> Option<(super::ids::PolyProgramId, f64)> {
    let super::events::EventRepresentation::ClipQ { q } = event.representation else {
        return None;
    };
    let feature = event
        .participants
        .iter()
        .find_map(|participant| match participant {
            super::events::Participant::Feature(id) => Some(id),
            _ => None,
        })?;
    renderer
        .projective
        .program()
        .equations
        .features
        .iter()
        .find(|entry| entry.feature == feature)
        .map(|entry| (entry.root_equation, q))
}

/// A validity predicate of an affine-seeded feature, reduced to an exact curve
/// in `(u, v)` by eliminating `q`.
///
/// A feature whose ray polynomial is affine in `q` — `R = A_f q + S_f(u,v)` —
/// has the closed-form root `q* = -S_f / A_f` wherever `A_f` is nonzero.
/// Substituting that into a validity predicate
/// `P = A_p q + S_p(u,v)` clears the denominator:
///
/// ```text
///   P(q*) = (A_f S_p - A_p S_f) / A_f     so   sign P(q*) = sign(G) sign(A_f)
/// ```
///
/// with `G = A_f S_p - A_p S_f` a polynomial in `(u, v)` alone. Orienting `G`
/// by `sign(A_f)` and by the predicate's sense yields a curve `C` whose
/// non-negative side is exactly "this predicate is satisfied at the feature's
/// root", which is the same integrand shape the coverage tier already
/// integrates for a discriminant. `A_f` constant with `S_f`, `S_p` affine — the
/// box-face case — makes every second `uv` derivative of `G` identically zero,
/// so the affine model the integrator builds is exact.
///
/// A globally sealed sign is sufficient but not necessary. When `A_f` is
/// affine in `(u,v)`, the guest can prove one strict sign over each cell from
/// its four corners and orient both curves locally. A cell touching `A_f = 0`
/// declines, so the degenerate root equation remains owned by the subpixel
/// walk rather than being assigned area by this tier.
struct PredicateEliminant {
    root_polynomial: super::ids::PolyProgramId,
    predicate_polynomial: super::ids::PolyProgramId,
    /// The globally sealed strict sign of `A_f`, when one exists. `None`
    /// means `A_f` is affine in `(u,v)` and its sign must be sealed per cell.
    root_leading_sign: Option<f64>,
    /// Multiplies `sign(A_f) G` so that `C >= 0` is exactly "predicate
    /// satisfied".
    predicate_orientation: f64,
    /// Second `uv` derivative magnitude bounds of `G` and of `S_f`.
    curve_second: [f64; 3],
    witness_second: [f64; 3],
    /// Event ids of the feature's *other* validity predicates. A single
    /// predicate is not an occupancy boundary - the feature is present only
    /// where all of them hold at once - so the guest integrates the whole
    /// intersection and needs every sibling curve to do it.
    sibling_events: Vec<u32>,
}

/// `+1.0` for a sense whose satisfied side is `P >= 0`, `-1.0` for `P <= 0`.
/// Strict senses are refused: the closed-cell test the guest applies is
/// `lower_bound >= 0`, which is sound for a non-strict sense and unsound for a
/// strict one, and a box edge sitting exactly on a pixel boundary makes the
/// difference decide real pixels.
fn predicate_sense_orientation(sense: super::program::PredicateSense) -> Option<f64> {
    match sense {
        super::program::PredicateSense::NonNegative => Some(1.0),
        super::program::PredicateSense::NonPositive => Some(-1.0),
        super::program::PredicateSense::StrictPositive
        | super::program::PredicateSense::StrictNegative
        | super::program::PredicateSense::EqualZero => None,
    }
}

/// Split a polynomial's terms into its `q^1` and `q^0` groups. `None` when any
/// term carries a `q` degree above one or any `x`/`t` degree at all, neither of
/// which the affine elimination above covers.
fn split_affine_q_groups(
    polynomial: &super::polynomial::PolyProgram,
) -> Option<(
    Vec<super::polynomial::PolyTerm>,
    Vec<super::polynomial::PolyTerm>,
)> {
    let mut leading = Vec::new();
    let mut constant = Vec::new();
    for term in &polynomial.terms {
        if term.exponents.x != 0 || term.exponents.t != 0 {
            return None;
        }
        match term.exponents.q {
            0 => constant.push(*term),
            1 => leading.push(*term),
            _ => return None,
        }
    }
    Some((leading, constant))
}

/// Whether a `q` coefficient is affine in `(u,v)`. Parameters and camera
/// coefficients may still vary between frames; at a fixed frame they are
/// scalars, so four same-sign corner values prove a strict sign on the cell.
fn is_affine_uv(terms: &[super::polynomial::PolyTerm]) -> bool {
    terms
        .iter()
        .all(|term| u32::from(term.exponents.u) + u32::from(term.exponents.v) <= 1)
}

/// Magnitude bound of one polynomial term's coefficient, parameters included.
fn eliminant_term_magnitude(
    structural: &super::verify::StructuralProgram,
    coefficient_intervals: &[super::reference::interval::F64Interval],
    term: &super::polynomial::PolyTerm,
) -> Result<f64, String> {
    let coefficient = coefficient_intervals
        .get(term.coefficient.index())
        .ok_or_else(|| {
            format!(
                "pixels::glue: eliminant coefficient {} lacks a verified interval",
                term.coefficient
            )
        })?;
    let mut magnitude = coefficient.lo.abs().max(coefficient.hi.abs());
    for parameter in term.exponents.param_terms.iter() {
        let slot = structural
            .params
            .slots
            .get(parameter.param.index())
            .ok_or_else(|| {
                format!(
                    "pixels::glue: eliminant parameter {} lacks a sealed slot",
                    parameter.param
                )
            })?;
        magnitude *= slot
            .range
            .min
            .abs()
            .max(slot.range.max.abs())
            .powi(i32::from(parameter.exponent));
    }
    Ok(magnitude)
}

/// Second `uv` derivative magnitude bounds `[duu, duv, dvv]` of a monomial sum
/// given as `(magnitude, u exponent, v exponent)` triples.
fn monomial_second_derivative_bounds(
    monomials: &[(f64, u32, u32)],
    u_extent: f64,
    v_extent: f64,
) -> [f64; 3] {
    let mut bounds = [0.0_f64; 3];
    for (magnitude, eu, ev) in monomials.iter().copied() {
        if eu >= 2 {
            bounds[0] += magnitude
                * f64::from(eu)
                * f64::from(eu - 1)
                * u_extent.abs().powi(eu as i32 - 2)
                * v_extent.abs().powi(ev as i32);
        }
        if eu != 0 && ev != 0 {
            bounds[1] += magnitude
                * f64::from(eu)
                * f64::from(ev)
                * u_extent.abs().powi(eu as i32 - 1)
                * v_extent.abs().powi(ev as i32 - 1);
        }
        if ev >= 2 {
            bounds[2] += magnitude
                * f64::from(ev)
                * f64::from(ev - 1)
                * u_extent.abs().powi(eu as i32)
                * v_extent.abs().powi(ev as i32 - 2);
        }
    }
    bounds
}

/// Expand a product of two `uv` monomial lists: exponents add and magnitudes
/// multiply, so the product's second derivative bound is the ordinary term-wise
/// bound of the expanded list.
fn expand_monomial_product(
    structural: &super::verify::StructuralProgram,
    coefficient_intervals: &[super::reference::interval::F64Interval],
    left: &[super::polynomial::PolyTerm],
    right: &[super::polynomial::PolyTerm],
    into: &mut Vec<(f64, u32, u32)>,
) -> Result<(), String> {
    for a in left {
        let a_magnitude = eliminant_term_magnitude(structural, coefficient_intervals, a)?;
        for b in right {
            let magnitude =
                a_magnitude * eliminant_term_magnitude(structural, coefficient_intervals, b)?;
            into.push((
                magnitude,
                u32::from(a.exponents.u) + u32::from(b.exponents.u),
                u32::from(a.exponents.v) + u32::from(b.exponents.v),
            ));
        }
    }
    Ok(())
}

/// Expand a single `uv` monomial list into `(magnitude, eu, ev)` triples.
fn expand_monomials(
    structural: &super::verify::StructuralProgram,
    coefficient_intervals: &[super::reference::interval::F64Interval],
    terms: &[super::polynomial::PolyTerm],
    into: &mut Vec<(f64, u32, u32)>,
) -> Result<(), String> {
    for term in terms {
        into.push((
            eliminant_term_magnitude(structural, coefficient_intervals, term)?,
            u32::from(term.exponents.u),
            u32::from(term.exponents.v),
        ));
    }
    Ok(())
}

fn predicate_eliminant(
    renderer: &super::CompiledRenderer,
    event: &super::events::EventGenerator,
) -> Result<Option<PredicateEliminant>, String> {
    let super::events::EventRepresentation::SparsePredicate { predicate } = event.representation
    else {
        return Ok(None);
    };
    if event.kind != super::event_kinds::EventKind::FeatureBoundary {
        return Ok(None);
    }
    let structural = renderer.structural.program();
    let equations = &renderer.projective.program().equations;
    let generators = &renderer.projective.program().events.generators;
    let Some(feature_id) = event
        .participants
        .iter()
        .find_map(|participant| match participant {
            super::events::Participant::Feature(id) => Some(id),
            _ => None,
        })
    else {
        return Ok(None);
    };
    let Some(feature) = equations
        .features
        .iter()
        .find(|entry| entry.feature == feature_id)
    else {
        return Ok(None);
    };
    if !feature.validity_predicates.contains(&predicate) {
        return Ok(None);
    }
    // A deformed feature's root is not the ray polynomial's root, so the
    // elimination below would substitute the wrong `q`.
    if feature.deformed_predictor || feature.q_degree != 1 {
        return Ok(None);
    }
    let root_leading_sign = match feature.q_seed_kind {
        super::projective::SeedKind::Affine { denominator } => match denominator.sign {
            super::projective::StrictSign::Positive => Some(1.0),
            super::projective::StrictSign::Negative => Some(-1.0),
        },
        _ => None,
    };
    // A feature whose q domain is cut by a clip plane, or whose occupancy is
    // tiled by a repeat boundary, has boundaries inside a pixel that the
    // validity curve alone does not describe.
    if generators.iter().any(|other| {
        other.participants.iter().any(|participant| {
            matches!(participant, super::events::Participant::Feature(id) if id == feature_id)
        }) && matches!(
            other.representation,
            super::events::EventRepresentation::ClipQ { .. }
                | super::events::EventRepresentation::RepeatAffineBoundary { .. }
        )
    }) {
        return Ok(None);
    }
    // Every validity predicate of the feature must be present as an event and
    // reducible the same way: the guest proves the siblings satisfied over the
    // cell, and a predicate it cannot see is a predicate it cannot prove.
    if feature.validity_predicates.len() > 7 {
        return Ok(None);
    }
    let mut sibling_events = Vec::new();
    let mut predicate_orientation = None;
    for candidate in &feature.validity_predicates {
        let Some(program) = equations
            .predicates
            .iter()
            .find(|entry| entry.id == *candidate)
        else {
            return Ok(None);
        };
        let Some(orientation) = predicate_sense_orientation(program.sense) else {
            return Ok(None);
        };
        let Some(polynomial) = equations.polynomials.get(program.polynomial.index()) else {
            return Ok(None);
        };
        if split_affine_q_groups(polynomial).is_none() {
            return Ok(None);
        }
        let Some(generator) = generators.iter().find(|other| {
            matches!(
                other.representation,
                super::events::EventRepresentation::SparsePredicate { predicate: id }
                    if id == *candidate
            ) && other.participants.iter().any(|participant| {
                matches!(participant, super::events::Participant::Feature(id) if id == feature_id)
            })
        }) else {
            return Ok(None);
        };
        if *candidate == predicate {
            predicate_orientation = Some(orientation);
        } else {
            sibling_events.push(generator.id.0);
        }
    }
    let Some(predicate_orientation) = predicate_orientation else {
        return Ok(None);
    };
    let Some(root) = equations.polynomials.get(feature.root_equation.index()) else {
        return Ok(None);
    };
    let Some((root_leading, root_constant)) = split_affine_q_groups(root) else {
        return Ok(None);
    };
    if root_leading.is_empty() {
        return Ok(None);
    }
    // Without a global sign, the runtime proof below is exact only when the
    // leading coefficient is affine over the pixel cell. A nonlinear leading
    // coefficient could have equal corner signs and still cross zero inside.
    if root_leading_sign.is_none() && !is_affine_uv(&root_leading) {
        return Ok(None);
    }
    let predicate_program = equations
        .predicates
        .iter()
        .find(|entry| entry.id == predicate)
        .ok_or_else(|| format!("pixels::glue: predicate {predicate} is missing"))?;
    let predicate_polynomial = equations
        .polynomials
        .get(predicate_program.polynomial.index())
        .ok_or_else(|| {
            format!(
                "pixels::glue: predicate {predicate} names missing polynomial {}",
                predicate_program.polynomial
            )
        })?;
    let (predicate_leading, predicate_constant) = split_affine_q_groups(predicate_polynomial)
        .ok_or_else(|| format!("pixels::glue: predicate {predicate} lost its affine q split"))?;
    let coefficient_intervals = super::projective::coefficient_intervals_for_roots(
        &equations.coefficients,
        &structural.values,
        equations.camera,
        root.terms
            .iter()
            .chain(predicate_polynomial.terms.iter())
            .map(|term| term.coefficient),
    )?;
    let u_extent = equations.camera.aspect * equations.camera.tan_half_fov_y;
    let v_extent = equations.camera.tan_half_fov_y;
    let mut curve_monomials = Vec::new();
    expand_monomial_product(
        structural,
        &coefficient_intervals,
        &root_leading,
        &predicate_constant,
        &mut curve_monomials,
    )?;
    expand_monomial_product(
        structural,
        &coefficient_intervals,
        &predicate_leading,
        &root_constant,
        &mut curve_monomials,
    )?;
    let curve_second = monomial_second_derivative_bounds(&curve_monomials, u_extent, v_extent);
    // The forward-root witness is `S_f` itself, up to a sign.
    let mut witness_monomials = Vec::new();
    expand_monomials(
        structural,
        &coefficient_intervals,
        &root_constant,
        &mut witness_monomials,
    )?;
    let witness_second = monomial_second_derivative_bounds(&witness_monomials, u_extent, v_extent);
    Ok(Some(PredicateEliminant {
        root_polynomial: feature.root_equation,
        predicate_polynomial: predicate_program.polynomial,
        root_leading_sign,
        predicate_orientation,
        curve_second,
        witness_second,
        sibling_events,
    }))
}

fn write_visibility_polynomial_accessors(
    output: &mut String,
    compiled: &[super::CompiledRenderer],
) -> Result<(), String> {
    output.push_str(
        "\n\
         pub fn __wrela_pixels_p7_abs(value: f32) -> f32:\n\
         \x20   if value < 0.0:\n\
         \x20       return -value\n\
         \x20   return value\n\
         \n\
         # Exact rounded byte of `a*s + b*t + c >= 0` over the unit square.\n\
         # Evaluating the final rational directly avoids the one-byte\n\
         # enclosure introduced by an intermediate 512th-scale area.\n\
         pub fn __wrela_pixels_p7_half_plane_byte(a: i32, b: i32, c: i32) -> [i64; 2]:\n\
         \x20   if a < -1000000 or a > 1000000 or b < -1000000 or b > 1000000 or c < -1000000 or c > 1000000:\n\
         \x20       return [0, 0]\n\
         \x20   aa = a.to[i64]()\n\
         \x20   bb = b.to[i64]()\n\
         \x20   cc = c.to[i64]()\n\
         \x20   if aa < 0:\n\
         \x20       cc = cc + aa\n\
         \x20       aa = -aa\n\
         \x20   if bb < 0:\n\
         \x20       cc = cc + bb\n\
         \x20       bb = -bb\n\
         \x20   threshold = -cc\n\
         \x20   if threshold <= 0:\n\
         \x20       return [1, 255]\n\
         \x20   if threshold >= aa + bb:\n\
         \x20       return [1, 0]\n\
         \x20   numerator: i64 = 0\n\
         \x20   denominator: i64 = 0\n\
         \x20   if aa == 0:\n\
         \x20       numerator = (bb - threshold) * 255\n\
         \x20       denominator = bb\n\
         \x20   elif bb == 0:\n\
         \x20       numerator = (aa - threshold) * 255\n\
         \x20       denominator = aa\n\
         \x20   else:\n\
         \x20       h0 = threshold * threshold\n\
         \x20       h1: i64 = 0\n\
         \x20       h2: i64 = 0\n\
         \x20       if threshold > aa:\n\
         \x20           h1 = (threshold - aa) * (threshold - aa)\n\
         \x20       if threshold > bb:\n\
         \x20           h2 = (threshold - bb) * (threshold - bb)\n\
         \x20       denominator = 2 * aa * bb\n\
         \x20       foreground = denominator - (h0 - h1 - h2)\n\
         \x20       if foreground < 0:\n\
         \x20           foreground = 0\n\
         \x20       if foreground > denominator:\n\
         \x20           foreground = denominator\n\
         \x20       numerator = foreground * 255\n\
         \x20   if not denominator > 0:\n\
         \x20       return [0, 0]\n\
         \x20   return [1, (2 * numerator + denominator) / (2 * denominator)]\n\
         \n\
         pub fn __wrela_pixels_p7_round_i32(value: f32) -> i32:\n\
         \x20   if value < 0.0:\n\
         \x20       return -((-value + 0.5).to[i32]())\n\
         \x20   return (value + 0.5).to[i32]()\n\
         \n\
         pub fn __wrela_pixels_p7_min(a: f32, b: f32) -> f32:\n\
         \x20   if a < b:\n\
         \x20       return a\n\
         \x20   return b\n\
         \n\
         pub fn __wrela_pixels_p7_max(a: f32, b: f32) -> f32:\n\
         \x20   if a > b:\n\
         \x20       return a\n\
         \x20   return b\n\
         \n\
         pub fn __wrela_pixels_p7_clamp(value: f32, lo: f32, hi: f32) -> f32:\n\
         \x20   return __wrela_pixels_p7_min(__wrela_pixels_p7_max(value, lo), hi)\n\
         \n\
         pub fn __wrela_pixels_p7_normalize_component(x: f32, y: f32, z: f32, component: usize) -> f32:\n\
         \x20   length: f32 = sqrt_scalar(x * x + y * y + z * z)\n\
         \x20   if not length > 0.0:\n\
         \x20       return 0.0\n\
         \x20   if component == 0:\n\
         \x20       return x / length\n\
         \x20   if component == 1:\n\
         \x20       return y / length\n\
         \x20   return z / length\n\
         \n\
         pub fn __wrela_pixels_p7_smooth_min(a: f32, b: f32, k: f32) -> f32:\n\
         \x20   if not k > 0.0:\n\
         \x20       return __wrela_pixels_p7_min(a, b)\n\
         \x20   h: f32 = __wrela_pixels_p7_clamp(0.5 + 0.5 * (b - a) / k, 0.0, 1.0)\n\
         \x20   return b + (a - b) * h - k * h * (1.0 - h)\n\
         \n\
         pub fn __wrela_pixels_p7_finite_or(value: f32, fallback: f32) -> f32:\n\
         \x20   if value != value or value > 3.4028234663852886e38 or value < -3.4028234663852886e38:\n\
         \x20       return fallback\n\
         \x20   return value\n",
    );
    output.push_str(
        "\n\
         pub fn __wrela_pixels_p7_outward_low(value: f32) -> f32:\n\
         \x20   bits = __wrela_pixels_f32_to_bits(value)\n\
         \x20   if bits == 0:\n\
         \x20       return __wrela_pixels_f32_from_bits(2147483649)\n\
         \x20   if bits < 2147483648:\n\
         \x20       return __wrela_pixels_f32_from_bits(bits - 1)\n\
         \x20   return __wrela_pixels_f32_from_bits(bits + 1)\n\
         \n\
         pub fn __wrela_pixels_p7_outward_high(value: f32) -> f32:\n\
         \x20   bits = __wrela_pixels_f32_to_bits(value)\n\
         \x20   if bits == 2147483648:\n\
         \x20       return __wrela_pixels_f32_from_bits(1)\n\
         \x20   if bits < 2147483648:\n\
         \x20       return __wrela_pixels_f32_from_bits(bits + 1)\n\
         \x20   return __wrela_pixels_f32_from_bits(bits - 1)\n",
    );
    write_sealed_root_polynomial_evaluator(output, compiled)?;
    write_p9_transfer_tables(output, compiled)?;
    write_p9_material_evaluator(output, compiled)?;
    output.push_str(
        "\npub fn __wrela_pixels_p7_feature_polynomial(renderer: usize, feature: u32, u: f32, v: f32, read params: [f32; 16], read camera: [f32; 12]) -> [f32; 11]:\n\
             \x20   record = __wrela_pixels_program_record(renderer, 4, feature)\n\
             \x20   polynomial = __wrela_pixels_program_operand(renderer, 4, feature, 5)\n\
             \x20   if record[0] != 1 or record[1] != feature.to[u64]() or polynomial[0] != 1 or polynomial[1] > 4294967295:\n\
             \x20       return [0.0; 11]\n\
             \x20   return __wrela_pixels_p7_root_polynomial(renderer, polynomial[1].to[u32](), u, v, params, camera)\n",
    );
    output.push_str(
        "\npub fn __wrela_pixels_p7_feature_polynomial_uv_bounds(renderer: usize, feature: u32) -> [f32; 20]:\n",
    );
    for (renderer_index, renderer) in compiled.iter().enumerate() {
        let structural = renderer.structural.program();
        let equations = &renderer.projective.program().equations;
        let coefficient_intervals = super::projective::coefficient_intervals_for_roots(
            &equations.coefficients,
            &structural.values,
            equations.camera,
            equations.features.iter().flat_map(|feature| {
                equations.polynomials[feature.root_equation.index()]
                    .terms
                    .iter()
                    .map(|term| term.coefficient)
            }),
        )?;
        let u_extent = equations.camera.aspect * equations.camera.tan_half_fov_y;
        let v_extent = equations.camera.tan_half_fov_y;
        for feature in &equations.features {
            let polynomial = &equations.polynomials[feature.root_equation.index()];
            let mut du = [0.0_f64; 9];
            let mut dv = [0.0_f64; 9];
            for term in &polynomial.terms {
                let coefficient = coefficient_intervals
                    .get(term.coefficient.index())
                    .ok_or_else(|| {
                        format!(
                            "pixels::glue: coefficient {} lacks a verified interval",
                            term.coefficient
                        )
                    })?;
                let mut magnitude = coefficient.lo.abs().max(coefficient.hi.abs());
                for parameter in term.exponents.param_terms.iter() {
                    let slot = structural
                        .params
                        .slots
                        .get(parameter.param.index())
                        .ok_or_else(|| {
                            format!(
                                "pixels::glue: polynomial parameter {} lacks a sealed slot",
                                parameter.param
                            )
                        })?;
                    magnitude *= slot
                        .range
                        .min
                        .abs()
                        .max(slot.range.max.abs())
                        .powi(i32::from(parameter.exponent));
                }
                let u_power = u_extent
                    .abs()
                    .powi(i32::from(term.exponents.u.saturating_sub(1)));
                let v_power = v_extent
                    .abs()
                    .powi(i32::from(term.exponents.v.saturating_sub(1)));
                let q_index = usize::from(term.exponents.q);
                if term.exponents.u != 0 {
                    du[q_index] += magnitude
                        * f64::from(term.exponents.u)
                        * u_power
                        * v_extent.abs().powi(i32::from(term.exponents.v));
                }
                if term.exponents.v != 0 {
                    dv[q_index] += magnitude
                        * f64::from(term.exponents.v)
                        * u_extent.abs().powi(i32::from(term.exponents.u))
                        * v_power;
                }
            }
            let mut values = Vec::with_capacity(18);
            for index in 0..9 {
                let du = super::reference::interval::next_up_f32(du[index] as f32);
                let dv = super::reference::interval::next_up_f32(dv[index] as f32);
                if !du.is_finite() || !dv.is_finite() {
                    return Err("pixels::glue: non-finite polynomial derivative bound".to_string());
                }
                values.push(wrela_f32_literal(du)?);
                values.push(wrela_f32_literal(dv)?);
            }
            writeln!(
                output,
                "    if renderer == {renderer_index} and feature == {}:\n\
                 \x20       return [1.0, {}.0, {}]",
                feature.feature.0,
                polynomial.degree_q,
                values.join(", "),
            )
            .expect("String writes cannot fail");
        }
    }
    output.push_str("    return [0.0; 20]\n");
    output.push_str(
        "\npub fn __wrela_pixels_p7_feature_predicate_uv_bounds(renderer: usize, feature: u32, ordinal: u32) -> [f32; 20]:\n",
    );
    for (renderer_index, renderer) in compiled.iter().enumerate() {
        let structural = renderer.structural.program();
        let equations = &renderer.projective.program().equations;
        let coefficient_intervals = super::projective::coefficient_intervals_for_roots(
            &equations.coefficients,
            &structural.values,
            equations.camera,
            equations.features.iter().flat_map(|feature| {
                feature.validity_predicates.iter().flat_map(|predicate| {
                    equations.polynomials
                        [equations.predicates[predicate.index()].polynomial.index()]
                    .terms
                    .iter()
                    .map(|term| term.coefficient)
                })
            }),
        )?;
        let u_extent = equations.camera.aspect * equations.camera.tan_half_fov_y;
        let v_extent = equations.camera.tan_half_fov_y;
        for feature in &equations.features {
            for (ordinal, predicate) in feature.validity_predicates.iter().enumerate() {
                let polynomial = &equations.polynomials
                    [equations.predicates[predicate.index()].polynomial.index()];
                let mut du = [0.0_f64; 9];
                let mut dv = [0.0_f64; 9];
                for term in &polynomial.terms {
                    let coefficient = coefficient_intervals
                        .get(term.coefficient.index())
                        .ok_or_else(|| {
                            format!(
                                "pixels::glue: validity coefficient {} lacks a verified interval",
                                term.coefficient
                            )
                        })?;
                    let mut magnitude = coefficient.lo.abs().max(coefficient.hi.abs());
                    for parameter in term.exponents.param_terms.iter() {
                        let slot = structural
                            .params
                            .slots
                            .get(parameter.param.index())
                            .ok_or_else(|| {
                                format!(
                                    "pixels::glue: validity polynomial parameter {} lacks a sealed slot",
                                    parameter.param
                                )
                            })?;
                        magnitude *= slot
                            .range
                            .min
                            .abs()
                            .max(slot.range.max.abs())
                            .powi(i32::from(parameter.exponent));
                    }
                    let u_power = u_extent
                        .abs()
                        .powi(i32::from(term.exponents.u.saturating_sub(1)));
                    let v_power = v_extent
                        .abs()
                        .powi(i32::from(term.exponents.v.saturating_sub(1)));
                    let q_index = usize::from(term.exponents.q);
                    if term.exponents.u != 0 {
                        du[q_index] += magnitude
                            * f64::from(term.exponents.u)
                            * u_power
                            * v_extent.abs().powi(i32::from(term.exponents.v));
                    }
                    if term.exponents.v != 0 {
                        dv[q_index] += magnitude
                            * f64::from(term.exponents.v)
                            * u_extent.abs().powi(i32::from(term.exponents.u))
                            * v_power;
                    }
                }
                let mut values = Vec::with_capacity(18);
                for index in 0..9 {
                    let du = super::reference::interval::next_up_f32(du[index] as f32);
                    let dv = super::reference::interval::next_up_f32(dv[index] as f32);
                    if !du.is_finite() || !dv.is_finite() {
                        return Err(
                            "pixels::glue: non-finite validity polynomial derivative bound"
                                .to_string(),
                        );
                    }
                    values.push(wrela_f32_literal(du)?);
                    values.push(wrela_f32_literal(dv)?);
                }
                writeln!(
                    output,
                    "    if renderer == {renderer_index} and feature == {} and ordinal == {ordinal}:\n\
                     \x20       return [1.0, {}.0, {}]",
                    feature.feature.0,
                    polynomial.degree_q,
                    values.join(", "),
                )
                .expect("String writes cannot fail");
            }
        }
    }
    output.push_str("    return [0.0; 20]\n");
    output.push_str(
        "\npub fn __wrela_pixels_p7_event_polynomial(renderer: usize, event: u32, u: f32, v: f32, read params: [f32; 16], read camera: [f32; 12]) -> [f32; 11]:\n",
    );
    for (renderer_index, renderer) in compiled.iter().enumerate() {
        for event in &renderer.projective.program().events.generators {
            match event.representation {
                super::events::EventRepresentation::QuadraticDiscriminant {
                    discriminant, ..
                } => writeln!(
                    output,
                    "    if renderer == {renderer_index} and event == {}:\n\
                     \x20       return __wrela_pixels_p7_root_polynomial(renderer, {}, u, v, params, camera)",
                    event.id.0, discriminant.0,
                )
                .expect("String writes cannot fail"),
                // A linear-leading-coefficient silhouette is the same shape of
                // integrand as a quadratic discriminant: a pure uv polynomial
                // whose zero set is the event curve. Emitting it lets the
                // analytic coverage integrator apply to grazing geometry
                // (a ground plane's horizon) instead of handing every pixel on
                // the curve to the subpixel walk. Which side of the curve is
                // occupied is orientation dependent, so the guest resolves the
                // side from an occupancy sample rather than from the sign.
                super::events::EventRepresentation::LinearLeadingCoefficient {
                    coefficient,
                    ..
                } => writeln!(
                    output,
                    "    if renderer == {renderer_index} and event == {}:\n\
                     \x20       return __wrela_pixels_p7_root_polynomial(renderer, {}, u, v, params, camera)",
                    event.id.0, coefficient.0,
                )
                .expect("String writes cannot fail"),
                super::events::EventRepresentation::TorusLocalOracle { root, .. } => writeln!(
                    output,
                    "    if renderer == {renderer_index} and event == {}:\n\
                     \x20       return __wrela_pixels_p7_root_polynomial(renderer, {}, u, v, params, camera)",
                    event.id.0, root.0,
                )
                .expect("String writes cannot fail"),
                _ => {
                    if let Some((root, _)) = clip_event_curve(renderer, event) {
                        writeln!(
                            output,
                            "    if renderer == {renderer_index} and event == {}:\n\
                             \x20       return __wrela_pixels_p7_root_polynomial(renderer, {}, u, v, params, camera)",
                            event.id.0, root.0,
                        )
                        .expect("String writes cannot fail");
                    }
                }
            }
        }
    }
    output.push_str("    return [0.0; 11]\n");
    output.push_str(
        "\npub fn __wrela_pixels_p7_event_clip_q(renderer: usize, event: u32) -> [f32; 2]:\n",
    );
    for (renderer_index, renderer) in compiled.iter().enumerate() {
        for event in &renderer.projective.program().events.generators {
            if let Some((_, q)) = clip_event_curve(renderer, event) {
                writeln!(
                    output,
                    "    if renderer == {renderer_index} and event == {}:\n\
                     \x20       return [1.0, {}]",
                    event.id.0,
                    wrela_f32_literal(q as f32)?,
                )
                .expect("String writes cannot fail");
            }
        }
    }
    output.push_str("    return [0.0, 0.0]\n");
    output.push_str(
        "\npub fn __wrela_pixels_p7_deformation_miss_model(renderer: usize, event: u32) -> [f32; 26]:\n",
    );
    let mut has_aligned_deformation = false;
    for (renderer_index, renderer) in compiled.iter().enumerate() {
        for event in &renderer.projective.program().events.generators {
            let Some(model) = deformation_sphere_miss_model(renderer, event)? else {
                continue;
            };
            if model.direct_center.is_some() && model.direct_amplitude > 0.0 {
                has_aligned_deformation = true;
            }
            let mut values = vec![
                format!("{}.0", model.feature.0),
                wrela_f32_literal(model.band)?,
                wrela_f32_literal(model.transversal_band)?,
                wrela_f32_literal(model.coefficient_error[0])?,
                wrela_f32_literal(model.coefficient_error[1])?,
                wrela_f32_literal(model.coefficient_error[2])?,
                wrela_f32_literal(model.magnitude)?,
            ];
            for row in model.curvature {
                for value in row {
                    values.push(wrela_f32_literal(value)?);
                }
            }
            values.push(wrela_f32_literal(model.amplitude)?);
            values.push(wrela_f32_literal(model.radius)?);
            values.push(wrela_f32_literal(model.frequency)?);
            values.push(format!("{}.0", model.phase_slot));
            if let Some(center) = model.direct_center {
                values.push("1.0".to_string());
                for value in center {
                    values.push(wrela_f32_literal(value)?);
                }
            } else {
                values.extend([
                    "0.0".to_string(),
                    "0.0".to_string(),
                    "0.0".to_string(),
                    "0.0".to_string(),
                ]);
            }
            values.push(wrela_f32_literal(model.direct_amplitude)?);
            writeln!(
                output,
                "    if renderer == {renderer_index} and event == {}:\n\
                 \x20       return [1.0, {}]",
                event.id.0,
                values.join(", "),
            )
            .expect("String writes cannot fail");
        }
    }
    output.push_str("    return [0.0; 26]\n");
    if has_aligned_deformation {
        output.push_str(ALIGNED_DEFORMATION_DEPTH_MISS_SOURCE);
    } else {
        output.push_str(ALIGNED_DEFORMATION_DEPTH_MISS_STUB);
    }
    output.push_str(
        "\npub fn __wrela_pixels_p7_standard_torus_event(renderer: usize, event: u32) -> bool:\n",
    );
    let mut standard_torus_features = BTreeSet::new();
    let mut has_standard_torus = false;
    for (renderer_index, renderer) in compiled.iter().enumerate() {
        for event in &renderer.projective.program().events.generators {
            if is_standard_torus_event(renderer, event)? {
                has_standard_torus = true;
                writeln!(
                    output,
                    "    if renderer == {renderer_index} and event == {}:\n\
                     \x20       return true",
                    event.id.0,
                )
                .expect("String writes cannot fail");
                if let Some(feature) = event.participants.iter().find_map(|participant| {
                    if let super::events::Participant::Feature(feature) = participant {
                        Some(feature.0)
                    } else {
                        None
                    }
                }) {
                    standard_torus_features.insert((renderer_index, feature));
                }
            }
        }
    }
    output.push_str("    return false\n");
    output.push_str(
        "\npub fn __wrela_pixels_p7_standard_torus_feature(renderer: usize, feature: u32) -> bool:\n",
    );
    for (renderer, feature) in standard_torus_features {
        writeln!(
            output,
            "    if renderer == {renderer} and feature == {feature}:\n\
             \x20       return true"
        )
        .expect("String writes cannot fail");
    }
    output.push_str("    return false\n");
    if has_standard_torus {
        output.push_str(STANDARD_TORUS_ROOT_CLASSIFIER_SOURCE);
        output.push_str(STANDARD_TORUS_CELL_CLASSIFIER_SOURCE);
        output.push_str(STANDARD_DOUBLE_DOUBLE_SOURCE);
        let mut standard_terms = BTreeMap::<(u8, u8), Vec<(u8, i32)>>::new();
        for &(x, y, eye, coefficient) in &STANDARD_TORUS_DISCRIMINANT_TERMS {
            standard_terms
                .entry((x, y))
                .or_default()
                .push((eye, coefficient));
        }
        output.push_str(
        "\npub fn __wrela_pixels_p7_standard_torus_coefficients(renderer: usize, event: u32, read camera: [f32; 12]) -> [f32; 57]:\n\
         \x20   result: [f32; 57] = [0.0; 57]\n\
         \x20   if not __wrela_pixels_p7_standard_torus_event(renderer, event):\n\
         \x20       return result\n\
         \x20   if camera[0] != 0.0 or camera[1] != 0.0 or camera[2] >= 0.0:\n\
         \x20       return result\n\
         \x20   if camera[3] != 0.0 or camera[4] != 0.0 or camera[5] != 1.0:\n\
         \x20       return result\n\
         \x20   if camera[6] != 1.0 or camera[7] != 0.0 or camera[8] != 0.0:\n\
         \x20       return result\n\
         \x20   if camera[9] != 0.0 or camera[10] != 1.0 or camera[11] != 0.0:\n\
         \x20       return result\n\
         \x20   eye = 0.0 - camera[2]\n\
         \x20   if eye < 0.125 or eye > 64.0:\n\
         \x20       return result\n\
         \x20   eye2_product = __wrela_pixels_p7_standard_two_product(eye, eye)\n\
         \x20   eye2: [f32; 3] = [eye2_product[0], eye2_product[1], 0.0]\n\
         \x20   result[0] = 1.0\n",
    );
        for (index, terms) in standard_terms.values().enumerate() {
            let value_slot = 1 + index * 2;
            let upper_slot = value_slot + 1;
            let mut by_degree = [0_i32; 5];
            let mut max_degree = 0_usize;
            for &(eye_degree, coefficient) in terms {
                debug_assert_eq!(eye_degree % 2, 0);
                let degree = usize::from(eye_degree / 2);
                by_degree[degree] += coefficient;
                max_degree = max_degree.max(degree);
            }
            let leading = wrela_f32_literal(by_degree[max_degree] as f32)?;
            writeln!(
                output,
                "    coefficient_{index}: [f32; 3] = [{leading}, 0.0, 0.0]"
            )
            .expect("String writes cannot fail");
            for degree in (0..max_degree).rev() {
                let constant = wrela_f32_literal(by_degree[degree] as f32)?;
                writeln!(
                output,
                "    coefficient_{index} = __wrela_pixels_p7_standard_dd_mul(coefficient_{index}, eye2)\n\
                 \x20   coefficient_{index} = __wrela_pixels_p7_standard_dd_add_f32(coefficient_{index}, {constant})"
            )
            .expect("String writes cannot fail");
            }
            writeln!(
            output,
            "    coefficient_{index}_sum = __wrela_pixels_p7_standard_two_sum(coefficient_{index}[0], coefficient_{index}[1])\n\
             \x20   coefficient_{index}_error = __wrela_pixels_p7_outward_high(__wrela_pixels_p7_abs(coefficient_{index}_sum[1]) + coefficient_{index}[2])\n\
             \x20   result[{value_slot}] = __wrela_pixels_p7_outward_low(coefficient_{index}_sum[0] - coefficient_{index}_error)\n\
             \x20   result[{upper_slot}] = __wrela_pixels_p7_outward_high(coefficient_{index}_sum[0] + coefficient_{index}_error)"
        )
        .expect("String writes cannot fail");
        }
        output.push_str(
        "    return result\n\
         \n\
         pub fn __wrela_pixels_p7_standard_torus_value(read coefficients: [f32; 57], u: f32, v: f32) -> [f32; 3]:\n\
         \x20   if coefficients[0] != 1.0:\n\
         \x20       return [0.0; 3]\n\
         \x20   x_lo = __wrela_pixels_p7_outward_low(u * u)\n\
         \x20   x_hi = __wrela_pixels_p7_outward_high(u * u)\n\
         \x20   y_lo = __wrela_pixels_p7_outward_low(v * v)\n\
         \x20   y_hi = __wrela_pixels_p7_outward_high(v * v)\n\
         \x20   if x_lo < 0.0:\n\
         \x20       x_lo = 0.0\n\
         \x20   if y_lo < 0.0:\n\
         \x20       y_lo = 0.0\n\
         \x20   row_lo: [f32; 7] = [0.0; 7]\n\
         \x20   row_hi: [f32; 7] = [0.0; 7]\n\
         \x20   interval_product_lo: f32 = 0.0\n\
         \x20   interval_product_hi: f32 = 0.0\n\
         \x20   coefficient_index: usize = 1\n\
         \x20   x_degree: usize = 0\n\
         \x20   @budget(bound=7)\n\
         \x20   while x_degree < 7:\n\
         \x20       max_y = 6 - x_degree\n\
         \x20       coefficient_slot = coefficient_index + max_y * 2\n\
         \x20       accumulator_lo = coefficients[coefficient_slot]\n\
         \x20       accumulator_hi = coefficients[coefficient_slot + 1]\n\
         \x20       y_degree = max_y\n\
         \x20       @budget(bound=6)\n\
         \x20       while y_degree > 0:\n\
         \x20           if accumulator_lo >= 0.0:\n\
         \x20               interval_product_lo = accumulator_lo * y_lo\n\
         \x20               interval_product_hi = accumulator_hi * y_hi\n\
         \x20           elif accumulator_hi <= 0.0:\n\
         \x20               interval_product_lo = accumulator_lo * y_hi\n\
         \x20               interval_product_hi = accumulator_hi * y_lo\n\
         \x20           else:\n\
         \x20               interval_product_lo = accumulator_lo * y_hi\n\
         \x20               interval_product_hi = accumulator_hi * y_hi\n\
         \x20           interval_product_lo = __wrela_pixels_p7_outward_low(interval_product_lo)\n\
         \x20           interval_product_hi = __wrela_pixels_p7_outward_high(interval_product_hi)\n\
         \x20           y_degree = y_degree - 1\n\
         \x20           coefficient_slot = coefficient_slot - 2\n\
         \x20           accumulator_lo = __wrela_pixels_p7_outward_low(interval_product_lo + coefficients[coefficient_slot])\n\
         \x20           accumulator_hi = __wrela_pixels_p7_outward_high(interval_product_hi + coefficients[coefficient_slot + 1])\n\
         \x20       row_lo[x_degree] = accumulator_lo\n\
         \x20       row_hi[x_degree] = accumulator_hi\n\
         \x20       coefficient_index = coefficient_index + (max_y + 1) * 2\n\
         \x20       x_degree = x_degree + 1\n\
         \x20   value_lo = row_lo[6]\n\
         \x20   value_hi = row_hi[6]\n\
         \x20   x_degree = 6\n\
         \x20   @budget(bound=6)\n\
         \x20   while x_degree > 0:\n\
         \x20       if value_lo >= 0.0:\n\
         \x20           interval_product_lo = value_lo * x_lo\n\
         \x20           interval_product_hi = value_hi * x_hi\n\
         \x20       elif value_hi <= 0.0:\n\
         \x20           interval_product_lo = value_lo * x_hi\n\
         \x20           interval_product_hi = value_hi * x_lo\n\
         \x20       else:\n\
         \x20           interval_product_lo = value_lo * x_hi\n\
         \x20           interval_product_hi = value_hi * x_hi\n\
         \x20       interval_product_lo = __wrela_pixels_p7_outward_low(interval_product_lo)\n\
         \x20       interval_product_hi = __wrela_pixels_p7_outward_high(interval_product_hi)\n\
         \x20       x_degree = x_degree - 1\n\
         \x20       value_lo = __wrela_pixels_p7_outward_low(interval_product_lo + row_lo[x_degree])\n\
         \x20       value_hi = __wrela_pixels_p7_outward_high(interval_product_hi + row_hi[x_degree])\n\
         \x20   value = value_lo + (value_hi - value_lo) * 0.5\n\
         \x20   if value < value_lo:\n\
         \x20       value = value_lo\n\
         \x20   if value > value_hi:\n\
         \x20       value = value_hi\n\
         \x20   error = __wrela_pixels_p7_abs(value - value_lo)\n\
         \x20   other_error = __wrela_pixels_p7_abs(value_hi - value)\n\
         \x20   if other_error > error:\n\
         \x20       error = other_error\n\
         \x20   error = __wrela_pixels_p7_outward_high(error)\n\
         \x20   value = value * 65536.0\n\
         \x20   error = error * 65536.0\n\
         \x20   if value != value or error != error:\n\
         \x20       return [0.0; 3]\n\
         \x20   return [1.0, value, error]\n\
         \n\
         pub fn __wrela_pixels_p7_standard_torus_pixel_bounds(read coefficients: [f32; 57], u: f32, v: f32, ru: f32, rv: f32) -> [f32; 8]:\n\
         \x20   if coefficients[0] != 1.0 or ru < 0.0 or rv < 0.0:\n\
         \x20       return [0.0; 8]\n\
         \x20   ua = __wrela_pixels_p7_outward_high(__wrela_pixels_p7_abs(u) + ru)\n\
         \x20   va = __wrela_pixels_p7_outward_high(__wrela_pixels_p7_abs(v) + rv)\n\
         \x20   up: [f32; 13] = [1.0; 13]\n\
         \x20   vp: [f32; 13] = [1.0; 13]\n\
         \x20   power: usize = 1\n\
         \x20   @budget(bound=12)\n\
         \x20   while power < 13:\n\
         \x20       up[power] = __wrela_pixels_p7_outward_high(up[power - 1] * ua)\n\
         \x20       vp[power] = __wrela_pixels_p7_outward_high(vp[power - 1] * va)\n\
         \x20       power = power + 1\n\
         \x20   result: [f32; 8] = [1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0]\n",
    );
        for (index, &(x_degree, y_degree)) in standard_terms.keys().enumerate() {
            let value_slot = 1 + index * 2;
            let upper_slot = value_slot + 1;
            let p = 2 * i64::from(x_degree);
            let q = 2 * i64::from(y_degree);
            let contributions = [
                (1, p - 2, q, p * (p - 1)),
                (2, p - 1, q - 1, p * q),
                (3, p, q - 2, q * (q - 1)),
                (4, p - 3, q, p * (p - 1) * (p - 2)),
                (5, p - 2, q - 1, p * (p - 1) * q),
                (6, p - 1, q - 2, p * q * (q - 1)),
                (7, p, q - 3, q * (q - 1) * (q - 2)),
            ];
            if contributions.iter().any(|entry| entry.3 != 0) {
                writeln!(
                output,
                "    coefficient_magnitude_{index} = __wrela_pixels_p7_abs(coefficients[{value_slot}])\n\
                 \x20   coefficient_magnitude_{index}_hi = __wrela_pixels_p7_abs(coefficients[{upper_slot}])\n\
                 \x20   if coefficient_magnitude_{index}_hi > coefficient_magnitude_{index}:\n\
                 \x20       coefficient_magnitude_{index} = coefficient_magnitude_{index}_hi"
            )
            .expect("String writes cannot fail");
            }
            for (result_slot, u_degree, v_degree, factor) in contributions {
                if factor == 0 {
                    continue;
                }
                writeln!(
                output,
                "    result[{result_slot}] = __wrela_pixels_p7_outward_high(result[{result_slot}] + coefficient_magnitude_{index} * up[{u_degree}] * vp[{v_degree}] * {factor}.0)"
            )
            .expect("String writes cannot fail");
            }
        }
        output.push_str(
        "    if result[1] != result[1] or result[2] != result[2] or result[3] != result[3]:\n\
         \x20       return [0.0; 8]\n\
         \x20   return result\n\
         \n\
         pub fn __wrela_pixels_p7_standard_torus_discriminant(read coefficients: [f32; 57], read pixel_bounds: [f32; 8], u: f32, v: f32, ru: f32, rv: f32) -> [f32; 5]:\n\
         \x20   if coefficients[0] != 1.0 or pixel_bounds[0] != 1.0 or ru < 0.0 or rv < 0.0:\n\
         \x20       return [0.0; 5]\n\
         \x20   duu = __wrela_pixels_p7_outward_high(pixel_bounds[1] * 65536.0)\n\
         \x20   duv = __wrela_pixels_p7_outward_high(pixel_bounds[2] * 65536.0)\n\
         \x20   dvv = __wrela_pixels_p7_outward_high(pixel_bounds[3] * 65536.0)\n\
         \x20   return [1.0, 0.0, duu, duv, dvv]\n",
        );
    } else {
        output.push_str(STANDARD_TORUS_STUB_SOURCE);
    }
    output.push_str(
        "\npub fn __wrela_pixels_p7_abs_power_high(value: f32, exponent: u8) -> [f32; 2]:\n\
         \x20   if value < 0.0 or exponent > 16:\n\
         \x20       return [0.0, 0.0]\n\
         \x20   result: f32 = 1.0\n\
         \x20   count: u8 = 0\n\
         \x20   @budget(bound=16)\n\
         \x20   while count < exponent:\n\
         \x20       result = __wrela_pixels_p7_outward_high(result * value)\n\
         \x20       if result != result:\n\
         \x20           return [0.0, 0.0]\n\
         \x20       count = count + 1\n\
         \x20   return [1.0, result]\n\
         \n\
         pub fn __wrela_pixels_p7_monomial_uv_jet(magnitude: f32, u: f32, v: f32, eu: u8, ev: u8) -> [f32; 6]:\n\
         \x20   if magnitude < 0.0 or u < 0.0 or v < 0.0:\n\
         \x20       return [-1.0; 6]\n\
         \x20   up = __wrela_pixels_p7_abs_power_high(u, eu)\n\
         \x20   vp = __wrela_pixels_p7_abs_power_high(v, ev)\n\
         \x20   um1: [f32; 2] = [1.0, 0.0]\n\
         \x20   vm1: [f32; 2] = [1.0, 0.0]\n\
         \x20   um2: [f32; 2] = [1.0, 0.0]\n\
         \x20   vm2: [f32; 2] = [1.0, 0.0]\n\
         \x20   if eu > 0:\n\
         \x20       um1 = __wrela_pixels_p7_abs_power_high(u, eu - 1)\n\
         \x20   if ev > 0:\n\
         \x20       vm1 = __wrela_pixels_p7_abs_power_high(v, ev - 1)\n\
         \x20   if eu > 1:\n\
         \x20       um2 = __wrela_pixels_p7_abs_power_high(u, eu - 2)\n\
         \x20   if ev > 1:\n\
         \x20       vm2 = __wrela_pixels_p7_abs_power_high(v, ev - 2)\n\
         \x20   if up[0] != 1.0 or vp[0] != 1.0 or um1[0] != 1.0 or vm1[0] != 1.0 or um2[0] != 1.0 or vm2[0] != 1.0:\n\
         \x20       return [-1.0; 6]\n\
         \x20   result: [f32; 6] = [0.0; 6]\n\
         \x20   result[0] = __wrela_pixels_p7_outward_high(magnitude * up[1] * vp[1])\n\
         \x20   if eu > 0:\n\
         \x20       result[1] = __wrela_pixels_p7_outward_high(magnitude * eu.to[f32]() * um1[1] * vp[1])\n\
         \x20   if ev > 0:\n\
         \x20       result[2] = __wrela_pixels_p7_outward_high(magnitude * ev.to[f32]() * up[1] * vm1[1])\n\
         \x20   if eu > 1:\n\
         \x20       result[3] = __wrela_pixels_p7_outward_high(magnitude * eu.to[f32]() * (eu - 1).to[f32]() * um2[1] * vp[1])\n\
         \x20   if eu > 0 and ev > 0:\n\
         \x20       result[4] = __wrela_pixels_p7_outward_high(magnitude * eu.to[f32]() * ev.to[f32]() * um1[1] * vm1[1])\n\
         \x20   if ev > 1:\n\
         \x20       result[5] = __wrela_pixels_p7_outward_high(magnitude * ev.to[f32]() * (ev - 1).to[f32]() * up[1] * vm2[1])\n\
         \x20   return result\n\
         \n\
         pub fn __wrela_pixels_p7_uv_jet_mul(read a: [f32; 6], read b: [f32; 6]) -> [f32; 6]:\n\
         \x20   return [\n\
         \x20       __wrela_pixels_p7_outward_high(a[0] * b[0]),\n\
         \x20       __wrela_pixels_p7_outward_high(a[1] * b[0] + a[0] * b[1]),\n\
         \x20       __wrela_pixels_p7_outward_high(a[2] * b[0] + a[0] * b[2]),\n\
         \x20       __wrela_pixels_p7_outward_high(a[3] * b[0] + 2.0 * a[1] * b[1] + a[0] * b[3]),\n\
         \x20       __wrela_pixels_p7_outward_high(a[4] * b[0] + a[1] * b[2] + a[2] * b[1] + a[0] * b[4]),\n\
         \x20       __wrela_pixels_p7_outward_high(a[5] * b[0] + 2.0 * a[2] * b[2] + a[0] * b[5]),\n\
         \x20   ]\n\
         \n\
         pub fn __wrela_pixels_p7_uv_jet_pow(read value: [f32; 6], exponent: u8) -> [f32; 6]:\n\
         \x20   if exponent > 6:\n\
         \x20       return [-1.0; 6]\n\
         \x20   result: [f32; 6] = [1.0, 0.0, 0.0, 0.0, 0.0, 0.0]\n\
         \x20   count: u8 = 0\n\
         \x20   @budget(bound=6)\n\
         \x20   while count < exponent:\n\
         \x20       result = __wrela_pixels_p7_uv_jet_mul(result, value)\n\
         \x20       count = count + 1\n\
         \x20   return result\n",
    );
    for (renderer_index, renderer) in compiled.iter().enumerate() {
        let equations = &renderer.projective.program().equations;
        for event in &renderer.projective.program().events.generators {
            let super::events::EventRepresentation::TorusLocalOracle { root, .. } =
                event.representation
            else {
                continue;
            };
            let polynomial = &equations.polynomials[root.index()];
            writeln!(
                output,
                "\npub fn __wrela_pixels_p7_torus_event_magnitudes_r{renderer_index}_e{}(read params: [f32; 16], read camera: [f32; 12]) -> [f32; 65]:\n\
                 \x20   result: [f32; 65] = [0.0; 65]\n\
                 \x20   result[0] = 1.0",
                event.id.0,
            )
            .expect("String writes cannot fail");
            for (term_index, term) in polynomial.terms.iter().enumerate() {
                writeln!(
                    output,
                    "    coefficient_{term_index} = __wrela_pixels_p7_root_coefficient({renderer_index}, {}, params, camera)\n\
                     \x20   if coefficient_{term_index}[0] != 1.0:\n\
                     \x20       return [0.0; 65]\n\
                     \x20   magnitude_{term_index} = __wrela_pixels_p7_abs(coefficient_{term_index}[1])",
                    term.coefficient.0,
                )
                .expect("String writes cannot fail");
                for parameter in term.exponents.param_terms.iter() {
                    writeln!(
                        output,
                        "    parameter_{term_index}_{} = __wrela_pixels_p7_abs_power_high(__wrela_pixels_p7_abs(params[{}]), {})\n\
                         \x20   if parameter_{term_index}_{}[0] != 1.0:\n\
                         \x20       return [0.0; 65]\n\
                         \x20   magnitude_{term_index} = __wrela_pixels_p7_outward_high(magnitude_{term_index} * parameter_{term_index}_{}[1])",
                        parameter.param.0,
                        parameter.param.0,
                        parameter.exponent,
                        parameter.param.0,
                        parameter.param.0,
                    )
                    .expect("String writes cannot fail");
                }
                writeln!(
                    output,
                    "    result[{}] = magnitude_{term_index}",
                    term_index + 1
                )
                .expect("String writes cannot fail");
            }
            output.push_str("    return result\n");
            writeln!(
                output,
                "\npub fn __wrela_pixels_p7_torus_event_uv2_bounds_r{renderer_index}_e{}(u: f32, v: f32, ru: f32, rv: f32, read magnitudes: [f32; 65]) -> [f32; 4]:\n\
                 \x20   if ru < 0.0 or rv < 0.0:\n\
                 \x20       return [0.0; 4]\n\
                 \x20   if magnitudes[0] != 1.0:\n\
                 \x20       return [0.0; 4]\n\
                 \x20   u_abs = __wrela_pixels_p7_outward_high(__wrela_pixels_p7_abs(u) + ru)\n\
                 \x20   v_abs = __wrela_pixels_p7_outward_high(__wrela_pixels_p7_abs(v) + rv)\n\
                 \x20   coefficients: [f32; 30] = [0.0; 30]",
                event.id.0,
            )
            .expect("String writes cannot fail");
            for (term_index, term) in polynomial.terms.iter().enumerate() {
                writeln!(
                    output,
                    "    term_{term_index} = __wrela_pixels_p7_monomial_uv_jet(magnitudes[{}], u_abs, v_abs, {}, {})\n\
                     \x20   if term_{term_index}[0] < 0.0:\n\
                     \x20       return [0.0; 4]\n\
                     \x20   component_{term_index}: usize = 0\n\
                     \x20   @budget(bound=6)\n\
                     \x20   while component_{term_index} < 6:\n\
                     \x20       slot_{term_index} = {}.to[usize]() * 6 + component_{term_index}\n\
                     \x20       coefficients[slot_{term_index}] = __wrela_pixels_p7_outward_high(coefficients[slot_{term_index}] + term_{term_index}[component_{term_index}])\n\
                     \x20       component_{term_index} = component_{term_index} + 1",
                    term_index + 1,
                    term.exponents.u,
                    term.exponents.v,
                    term.exponents.q,
                )
                .expect("String writes cannot fail");
            }
            output.push_str(
                "    e: [f32; 6] = [coefficients[0], coefficients[1], coefficients[2], coefficients[3], coefficients[4], coefficients[5]]\n\
                 \x20   d: [f32; 6] = [coefficients[6], coefficients[7], coefficients[8], coefficients[9], coefficients[10], coefficients[11]]\n\
                 \x20   c: [f32; 6] = [coefficients[12], coefficients[13], coefficients[14], coefficients[15], coefficients[16], coefficients[17]]\n\
                 \x20   b: [f32; 6] = [coefficients[18], coefficients[19], coefficients[20], coefficients[21], coefficients[22], coefficients[23]]\n\
                 \x20   a: [f32; 6] = [coefficients[24], coefficients[25], coefficients[26], coefficients[27], coefficients[28], coefficients[29]]\n\
                 \x20   a2 = __wrela_pixels_p7_uv_jet_mul(a, a)\n\
                 \x20   a3 = __wrela_pixels_p7_uv_jet_mul(a2, a)\n\
                 \x20   b2 = __wrela_pixels_p7_uv_jet_mul(b, b)\n\
                 \x20   b3 = __wrela_pixels_p7_uv_jet_mul(b2, b)\n\
                 \x20   b4 = __wrela_pixels_p7_uv_jet_mul(b2, b2)\n\
                 \x20   c2 = __wrela_pixels_p7_uv_jet_mul(c, c)\n\
                 \x20   c3 = __wrela_pixels_p7_uv_jet_mul(c2, c)\n\
                 \x20   c4 = __wrela_pixels_p7_uv_jet_mul(c2, c2)\n\
                 \x20   d2 = __wrela_pixels_p7_uv_jet_mul(d, d)\n\
                 \x20   d3 = __wrela_pixels_p7_uv_jet_mul(d2, d)\n\
                 \x20   d4 = __wrela_pixels_p7_uv_jet_mul(d2, d2)\n\
                 \x20   e2 = __wrela_pixels_p7_uv_jet_mul(e, e)\n\
                 \x20   e3 = __wrela_pixels_p7_uv_jet_mul(e2, e)\n\
                 \x20   result: [f32; 6] = [0.0; 6]\n",
            );
            let factors: [(&str, &[&str], f32); 16] = [
                ("t0", &["a3", "e3"], 256.0),
                ("t1", &["a2", "b", "d", "e2"], 192.0),
                ("t2", &["a2", "c2", "e2"], 128.0),
                ("t3", &["a2", "c", "d2", "e"], 144.0),
                ("t4", &["a2", "d4"], 27.0),
                ("t5", &["a", "b2", "c", "e2"], 144.0),
                ("t6", &["a", "b2", "d2", "e"], 6.0),
                ("t7", &["a", "b", "c2", "d", "e"], 80.0),
                ("t8", &["a", "b", "c", "d3"], 18.0),
                ("t9", &["a", "c4", "e"], 16.0),
                ("t10", &["a", "c3", "d2"], 4.0),
                ("t11", &["b4", "e2"], 27.0),
                ("t12", &["b3", "c", "d", "e"], 18.0),
                ("t13", &["b3", "d3"], 4.0),
                ("t14", &["b2", "c3", "e"], 4.0),
                ("t15", &["b2", "c2", "d2"], 1.0),
            ];
            for (name, term_factors, scale) in factors {
                let scale = wrela_f32_literal(scale)?;
                writeln!(
                    output,
                    "    {name}: [f32; 6] = [1.0, 0.0, 0.0, 0.0, 0.0, 0.0]"
                )
                .expect("String writes cannot fail");
                for factor in term_factors {
                    writeln!(
                        output,
                        "    {name} = __wrela_pixels_p7_uv_jet_mul({name}, {factor})"
                    )
                    .expect("String writes cannot fail");
                }
                writeln!(
                    output,
                    "    component_{name}: usize = 0\n\
                     \x20   @budget(bound=6)\n\
                     \x20   while component_{name} < 6:\n\
                     \x20       result[component_{name}] = __wrela_pixels_p7_outward_high(result[component_{name}] + {scale} * {name}[component_{name}])\n\
                     \x20       component_{name} = component_{name} + 1"
                )
                .expect("String writes cannot fail");
            }
            output.push_str(
                "    if result[3] != result[3] or result[4] != result[4] or result[5] != result[5]:\n\
                 \x20       return [0.0; 4]\n\
                 \x20   return [1.0, result[3], result[4], result[5]]\n",
            );
        }
    }
    output.push_str(
        "\npub fn __wrela_pixels_p7_torus_event_magnitudes(renderer: usize, event: u32, read params: [f32; 16], read camera: [f32; 12]) -> [f32; 65]:\n",
    );
    for (renderer_index, renderer) in compiled.iter().enumerate() {
        for event in &renderer.projective.program().events.generators {
            if matches!(
                event.representation,
                super::events::EventRepresentation::TorusLocalOracle { .. }
            ) {
                writeln!(
                    output,
                    "    if renderer == {renderer_index} and event == {}:\n\
                     \x20       return __wrela_pixels_p7_torus_event_magnitudes_r{renderer_index}_e{}(params, camera)",
                    event.id.0, event.id.0,
                )
                .expect("String writes cannot fail");
            }
        }
    }
    output.push_str("    return [0.0; 65]\n");
    output.push_str(
        "\npub fn __wrela_pixels_p7_event_polynomial_uv2_bounds(renderer: usize, event: u32, u: f32, v: f32, ru: f32, rv: f32, read magnitudes: [f32; 65]) -> [f32; 4]:\n",
    );
    for (renderer_index, renderer) in compiled.iter().enumerate() {
        let structural = renderer.structural.program();
        let equations = &renderer.projective.program().equations;
        // A curve's second uv derivative bound is taken with `q` fixed at
        // whatever value the integrator will evaluate it at: the whole q range
        // for a curve that is already a pure uv polynomial, and the sealed clip
        // plane for a clip boundary. Using the full range for a clip edge would
        // inflate the residual by `q_near / q_clip` and stop the byte from
        // pinning.
        let q_extent = 1.0 / renderer.config.near;
        let discriminants = renderer
            .projective
            .program()
            .events
            .generators
            .iter()
            .filter_map(|event| match event.representation {
                super::events::EventRepresentation::QuadraticDiscriminant {
                    discriminant, ..
                } => Some((event.id.0, discriminant, false, q_extent)),
                // Same integrand shape as a discriminant curve; the second
                // derivative bounds drive the integrator's residual term.
                super::events::EventRepresentation::LinearLeadingCoefficient {
                    coefficient,
                    ..
                } => Some((event.id.0, coefficient, false, q_extent)),
                super::events::EventRepresentation::TorusLocalOracle { root, .. } => {
                    Some((event.id.0, root, true, q_extent))
                }
                _ => clip_event_curve(renderer, event)
                    .map(|(root, q)| (event.id.0, root, false, q.abs())),
            })
            .collect::<Vec<_>>();
        let coefficient_intervals = super::projective::coefficient_intervals_for_roots(
            &equations.coefficients,
            &structural.values,
            equations.camera,
            discriminants.iter().flat_map(|(_, polynomial, _, _)| {
                equations.polynomials[polynomial.index()]
                    .terms
                    .iter()
                    .map(|term| term.coefficient)
            }),
        )?;
        let u_extent = equations.camera.aspect * equations.camera.tan_half_fov_y;
        let v_extent = equations.camera.tan_half_fov_y;
        for (event_id, polynomial_id, torus, q_extent) in discriminants {
            if torus {
                writeln!(
                    output,
                    "    if renderer == {renderer_index} and event == {event_id}:\n\
                     \x20       return __wrela_pixels_p7_torus_event_uv2_bounds_r{renderer_index}_e{event_id}(u, v, ru, rv, magnitudes)"
                )
                .expect("String writes cannot fail");
                continue;
            }
            let polynomial = &equations.polynomials[polynomial_id.index()];
            let mut duu = 0.0_f64;
            let mut duv = 0.0_f64;
            let mut dvv = 0.0_f64;
            for term in &polynomial.terms {
                let coefficient = coefficient_intervals
                    .get(term.coefficient.index())
                    .ok_or_else(|| {
                        format!(
                            "pixels::glue: event coefficient {} lacks a verified interval",
                            term.coefficient
                        )
                    })?;
                let mut magnitude = coefficient.lo.abs().max(coefficient.hi.abs());
                for parameter in term.exponents.param_terms.iter() {
                    let slot = structural
                        .params
                        .slots
                        .get(parameter.param.index())
                        .ok_or_else(|| {
                            format!(
                                "pixels::glue: event polynomial parameter {} lacks a sealed slot",
                                parameter.param
                            )
                        })?;
                    magnitude *= slot
                        .range
                        .min
                        .abs()
                        .max(slot.range.max.abs())
                        .powi(i32::from(parameter.exponent));
                }
                let eu = term.exponents.u;
                let ev = term.exponents.v;
                magnitude *= q_extent.abs().powi(i32::from(term.exponents.q));
                if eu >= 2 {
                    duu += magnitude
                        * f64::from(eu)
                        * f64::from(eu - 1)
                        * u_extent.abs().powi(i32::from(eu - 2))
                        * v_extent.abs().powi(i32::from(ev));
                }
                if eu != 0 && ev != 0 {
                    duv += magnitude
                        * f64::from(eu)
                        * f64::from(ev)
                        * u_extent.abs().powi(i32::from(eu - 1))
                        * v_extent.abs().powi(i32::from(ev - 1));
                }
                if ev >= 2 {
                    dvv += magnitude
                        * f64::from(ev)
                        * f64::from(ev - 1)
                        * u_extent.abs().powi(i32::from(eu))
                        * v_extent.abs().powi(i32::from(ev - 2));
                }
            }
            let duu = super::reference::interval::next_up_f32(duu as f32);
            let duv = super::reference::interval::next_up_f32(duv as f32);
            let dvv = super::reference::interval::next_up_f32(dvv as f32);
            if !duu.is_finite() || !duv.is_finite() || !dvv.is_finite() {
                return Err("pixels::glue: non-finite event curvature bound".to_string());
            }
            writeln!(
                output,
                "    if renderer == {renderer_index} and event == {event_id}:\n\
                 \x20       return [1.0, {}, {}, {}]",
                wrela_f32_literal(duu)?,
                wrela_f32_literal(duv)?,
                wrela_f32_literal(dvv)?,
            )
            .expect("String writes cannot fail");
        }
    }
    output.push_str("    return [0.0; 4]\n");
    // The validity-predicate eliminant curve. See `PredicateEliminant`: the
    // feature's affine root is substituted into the predicate, which clears `q`
    // and leaves a curve in `(u, v)` alone, oriented so that a non-negative
    // value is exactly "this predicate holds at this feature's root".
    //
    // Slot layout:
    //   0        1.0 when this event carries a sealed eliminant
    //   1        C(u, v)  — predicate satisfied at the root iff C >= 0
    //   2        Q(u, v)  — the root is a forward ray root iff Q > 0
    //   3,4,5    second uv derivative bounds of C: duu, duv, dvv
    //   6,7,8    second uv derivative bounds of Q: duu, duv, dvv
    //   9        number of sibling validity predicates of the same feature
    //   10       q* = -S_f / A_f, the feature's own root at this (u, v), so a
    //            caller can check that this feature is what a visibility sample
    //            actually saw rather than a surface hidden behind it
    //   11..16   the sibling event ids
    //   17       the strict local sign of A_f (+1 or -1)
    output.push_str(
        "\npub fn __wrela_pixels_p7_event_predicate_curve(renderer: usize, event: u32, u: f32, v: f32, read params: [f32; 16], read camera: [f32; 12]) -> [f32; 20]:\n\
         \x20   descriptor: [f32; 20] = [0.0; 20]\n\
         \x20   root_polynomial: u32 = 0\n\
         \x20   predicate_polynomial: u32 = 0\n\
         \x20   sealed_sign: f32 = 0.0\n\
         \x20   predicate_orientation: f32 = 1.0\n",
    );
    for (renderer_index, renderer) in compiled.iter().enumerate() {
        for event in &renderer.projective.program().events.generators {
            let Some(eliminant) = predicate_eliminant(renderer, event)? else {
                continue;
            };
            let mut slots = vec!["1.0".to_string(), "0.0".to_string(), "0.0".to_string()];
            // 3..8 are filled by the curvature loop below, then slot 9 the
            // sibling count and slot 10 the root `q`.
            for bound in eliminant
                .curve_second
                .iter()
                .chain(eliminant.witness_second.iter())
            {
                let value = super::reference::interval::next_up_f32(*bound as f32);
                if !value.is_finite() {
                    return Err(
                        "pixels::glue: non-finite predicate eliminant curvature bound".to_string(),
                    );
                }
                slots.push(wrela_f32_literal(value)?);
            }
            slots.push(format!("{}.0", eliminant.sibling_events.len()));
            slots.push("0.0".to_string());
            for sibling in &eliminant.sibling_events {
                slots.push(format!("{sibling}.0"));
            }
            while slots.len() < 17 {
                slots.push("0.0".to_string());
            }
            slots.push("0.0".to_string());
            while slots.len() < 20 {
                slots.push("0.0".to_string());
            }
            let sealed_sign = eliminant.root_leading_sign.map_or_else(
                || "0.0".to_string(),
                |sign| if sign < 0.0 { "-1.0" } else { "1.0" }.to_string(),
            );
            writeln!(
                output,
                "    if renderer == {renderer_index} and event == {}:\n\
                 \x20       descriptor = [{}]\n\
                 \x20       root_polynomial = {}\n\
                 \x20       predicate_polynomial = {}\n\
                 \x20       sealed_sign = {}\n\
                 \x20       predicate_orientation = {}1.0",
                event.id.0,
                slots.join(", "),
                eliminant.root_polynomial.0,
                eliminant.predicate_polynomial.0,
                sealed_sign,
                if eliminant.predicate_orientation < 0.0 {
                    "-"
                } else {
                    ""
                },
            )
            .expect("String writes cannot fail");
        }
    }
    output.push_str(
        "    if descriptor[0] != 1.0:\n\
         \x20       return [0.0; 20]\n\
         \x20   root = __wrela_pixels_p7_root_polynomial(renderer, root_polynomial, u, v, params, camera)\n\
         \x20   predicate = __wrela_pixels_p7_root_polynomial(renderer, predicate_polynomial, u, v, params, camera)\n\
         \x20   if root[0] != 1.0 or predicate[0] != 1.0 or root[1] != 1.0 or predicate[1] > 1.0:\n\
         \x20       return [0.0; 20]\n\
         \x20   root_sign: f32 = 1.0\n\
         \x20   if root[3] < 0.0:\n\
         \x20       root_sign = -1.0\n\
         \x20   elif not root[3] > 0.0:\n\
         \x20       return [0.0; 20]\n\
         \x20   if sealed_sign != 0.0 and root_sign != sealed_sign:\n\
         \x20       return [0.0; 20]\n\
         \x20   eliminant = root[3] * predicate[2] - predicate[3] * root[2]\n\
         \x20   curve = predicate_orientation * root_sign * eliminant\n\
         \x20   witness = -root_sign * root[2]\n\
         \x20   root_q = -root[2] / root[3]\n\
         \x20   if not __wrela_pixels_p5_finite(curve) or not __wrela_pixels_p5_finite(witness) or not __wrela_pixels_p5_finite(root_q):\n\
         \x20       return [0.0; 20]\n\
         \x20   descriptor[1] = curve\n\
         \x20   descriptor[2] = witness\n\
         \x20   descriptor[10] = root_q\n\
         \x20   descriptor[17] = root_sign\n\
         \x20   return descriptor\n",
    );
    // The sealed camera pose, when the declaration pins one. Frame validation
    // rejects any frame whose camera differs, so a renderer that pins its pose
    // either always satisfies the analytic tiers' pose precondition or never
    // renders a frame at all — which is what makes that precondition a
    // compile-time fact rather than a per-frame cliff.
    output.push_str("\npub fn __wrela_pixels_p7_pinned_camera(renderer: usize) -> [f32; 13]:\n");
    for (renderer_index, renderer) in compiled.iter().enumerate() {
        let Some(pose) = renderer.config.camera_pose else {
            continue;
        };
        let mut slots = vec!["1.0".to_string()];
        for value in pose {
            slots.push(wrela_f32_literal(value)?);
        }
        writeln!(
            output,
            "    if renderer == {renderer_index}:\n\
             \x20       return [{}]",
            slots.join(", "),
        )
        .expect("String writes cannot fail");
    }
    output.push_str("    return [0.0; 13]\n");
    output.push_str("\npub fn __wrela_pixels_p7_projected_union_mode(renderer: usize) -> u64:\n");
    for (renderer_index, renderer) in compiled.iter().enumerate() {
        let csg = &renderer.structural.program().csg;
        let union_only = csg.constant.is_none()
            && !csg.instructions.is_empty()
            && csg.instructions.iter().all(|instruction| {
                matches!(
                    instruction,
                    super::csg::CsgInst::Push(_) | super::csg::CsgInst::Or
                )
            });
        if union_only {
            writeln!(
                output,
                "    if renderer == {renderer_index}:\n        return 1"
            )
            .expect("String writes cannot fail");
        }
    }
    output.push_str("    return 0\n");
    // A constant axis-aligned box under a pure-union object admits a compact
    // exact projected-rectangle tier for the canonical axis camera. Emit the
    // source constants, not widened world bounds; runtime code still performs
    // outward arithmetic and declines for every other camera/composition.
    output.push_str("\npub fn __wrela_pixels_p8_axis_box(renderer: usize) -> [f32; 9]:\n");
    let mut has_axis_box = false;
    for (renderer_index, renderer) in compiled.iter().enumerate() {
        use super::primitive::AnalyticPredicate;

        let features = &renderer.structural.program().features;
        if features.len() != 6 {
            continue;
        }
        let Some(AnalyticPredicate::BoxFace { center, half, .. }) =
            features[0].validity.constraints.first()
        else {
            continue;
        };
        let object = features[0].object;
        let identity = features[0].identity_set;
        let mut faces = [[false; 2]; 3];
        let mut exact_set = true;
        for feature in features {
            let Some(AnalyticPredicate::BoxFace {
                axis,
                sign,
                center: candidate_center,
                half: candidate_half,
            }) = feature.validity.constraints.first()
            else {
                exact_set = false;
                break;
            };
            if feature.object != object
                || feature.identity_set != identity
                || candidate_center != center
                || candidate_half != half
                || *axis >= 3
                || !matches!(*sign, -1 | 1)
            {
                exact_set = false;
                break;
            }
            faces[usize::from(*axis)][usize::from(*sign == 1)] = true;
        }
        let csg = &renderer.structural.program().csg;
        let union_only = csg.constant.is_none()
            && !csg.instructions.is_empty()
            && csg.instructions.iter().all(|instruction| {
                matches!(
                    instruction,
                    super::csg::CsgInst::Push(_) | super::csg::CsgInst::Or
                )
            });
        if !exact_set || !faces.into_iter().flatten().all(|present| present) || !union_only {
            continue;
        }
        let mut center_values = [0.0_f32; 3];
        let mut half_values = [0.0_f32; 3];
        let mut constants = true;
        for axis in 0..3 {
            let Some(center_value) =
                super::scalar::constant_value(&renderer.symbolic.scalar, center[axis])
            else {
                constants = false;
                break;
            };
            let Some(half_value) =
                super::scalar::constant_value(&renderer.symbolic.scalar, half[axis])
            else {
                constants = false;
                break;
            };
            if !center_value.is_finite() || !half_value.is_finite() || half_value <= 0.0 {
                constants = false;
                break;
            }
            center_values[axis] = center_value;
            half_values[axis] = half_value;
        }
        if !constants {
            continue;
        }
        has_axis_box = true;
        let mut slots = vec![
            "1.0".to_string(),
            wrela_f32_literal(object.0 as f32)?,
            wrela_f32_literal(identity as f32)?,
        ];
        for axis in 0..3 {
            slots.push(wrela_f32_literal(center_values[axis] - half_values[axis])?);
        }
        for axis in 0..3 {
            slots.push(wrela_f32_literal(center_values[axis] + half_values[axis])?);
        }
        writeln!(
            output,
            "    if renderer == {renderer_index}:\n        return [{}]",
            slots.join(", "),
        )
        .expect("String writes cannot fail");
    }
    output.push_str("    return [0.0; 9]\n");
    if has_axis_box {
        output.push_str(
            r#"
pub fn __wrela_pixels_p8_ratio_bounds(numerator_lo: f32, numerator_hi: f32, denominator_lo: f32, denominator_hi: f32) -> [f32; 2]:
    if not denominator_lo > 0.0 or numerator_lo > numerator_hi or denominator_lo > denominator_hi:
        return [1.0, -1.0]
    a = numerator_lo / denominator_lo
    b = numerator_lo / denominator_hi
    c = numerator_hi / denominator_lo
    d = numerator_hi / denominator_hi
    lo = __wrela_pixels_p7_min(__wrela_pixels_p7_min(a, b), __wrela_pixels_p7_min(c, d))
    hi = __wrela_pixels_p7_max(__wrela_pixels_p7_max(a, b), __wrela_pixels_p7_max(c, d))
    return [__wrela_pixels_p7_outward_low(lo), __wrela_pixels_p7_outward_high(hi)]

pub fn __wrela_pixels_p8_axis_box_coverage(renderer: usize, x: u32, y: u32, read camera: [f32; 12]) -> [i64; 4]:
    box = __wrela_pixels_p8_axis_box(renderer)
    if box[0] != 1.0 or camera[3] != 0.0 or camera[4] != 0.0 or camera[5] != 1.0 or camera[6] != 1.0 or camera[7] != 0.0 or camera[8] != 0.0 or camera[9] != 0.0 or camera[10] != 1.0 or camera[11] != 0.0 or not camera[2] < box[5]:
        return [0; 4]
    config = __wrela_pixels_p7_numeric_config(renderer)
    if config[0] != 1 or config[1] <= 0 or config[2] <= 0:
        return [0; 4]
    width = config[1].to[f32]()
    height = config[2].to[f32]()
    aspect = width / height
    pixel_u0 = ((x.to[f32]() / width) * 2.0 - 1.0) * aspect
    pixel_u1 = (((x + 1).to[f32]() / width) * 2.0 - 1.0) * aspect
    pixel_v1 = 1.0 - (y.to[f32]() / height) * 2.0
    pixel_v0 = 1.0 - ((y + 1).to[f32]() / height) * 2.0
    depth_lo = __wrela_pixels_p7_outward_low(box[5] - camera[2])
    depth_hi = __wrela_pixels_p7_outward_high(box[5] - camera[2])
    u_min = __wrela_pixels_p8_ratio_bounds(__wrela_pixels_p7_outward_low(box[3] - camera[0]), __wrela_pixels_p7_outward_high(box[3] - camera[0]), depth_lo, depth_hi)
    u_max = __wrela_pixels_p8_ratio_bounds(__wrela_pixels_p7_outward_low(box[6] - camera[0]), __wrela_pixels_p7_outward_high(box[6] - camera[0]), depth_lo, depth_hi)
    v_min = __wrela_pixels_p8_ratio_bounds(__wrela_pixels_p7_outward_low(box[4] - camera[1]), __wrela_pixels_p7_outward_high(box[4] - camera[1]), depth_lo, depth_hi)
    v_max = __wrela_pixels_p8_ratio_bounds(__wrela_pixels_p7_outward_low(box[7] - camera[1]), __wrela_pixels_p7_outward_high(box[7] - camera[1]), depth_lo, depth_hi)
    if u_min[0] > u_min[1] or u_max[0] > u_max[1] or v_min[0] > v_min[1] or v_max[0] > v_max[1]:
        return [0; 4]
    width_lo = __wrela_pixels_p7_min(pixel_u1, u_max[0]) - __wrela_pixels_p7_max(pixel_u0, u_min[1])
    width_hi = __wrela_pixels_p7_min(pixel_u1, u_max[1]) - __wrela_pixels_p7_max(pixel_u0, u_min[0])
    height_lo = __wrela_pixels_p7_min(pixel_v1, v_max[0]) - __wrela_pixels_p7_max(pixel_v0, v_min[1])
    height_hi = __wrela_pixels_p7_min(pixel_v1, v_max[1]) - __wrela_pixels_p7_max(pixel_v0, v_min[0])
    if width_lo < 0.0:
        width_lo = 0.0
    if width_hi < 0.0:
        width_hi = 0.0
    if height_lo < 0.0:
        height_lo = 0.0
    if height_hi < 0.0:
        height_hi = 0.0
    pixel_area = (pixel_u1 - pixel_u0) * (pixel_v1 - pixel_v0)
    if not pixel_area > 0.0:
        return [0; 4]
    area_lo = __wrela_pixels_p7_outward_low(width_lo * height_lo)
    area_hi = __wrela_pixels_p7_outward_high(width_hi * height_hi)
    coverage_lo = __wrela_pixels_p7_outward_low(area_lo / pixel_area * 255.0)
    coverage_hi = __wrela_pixels_p7_outward_high(area_hi / pixel_area * 255.0)
    if coverage_lo < 0.0:
        coverage_lo = 0.0
    if coverage_hi > 255.0:
        coverage_hi = 255.0
    byte_lo = (coverage_lo + 0.5).to[i64]()
    byte_hi = (coverage_hi + 0.5).to[i64]()
    if byte_lo != byte_hi:
        return [0; 4]
    return [1, 1, byte_lo, 8589934592 + box[1].to[i64]()]
"#,
        );
    } else {
        // Keep the injected surface total, but do not make a renderer that
        // has no eligible box carry the entire analytic rectangle tier in
        // branchable text. This is a compile-time scene fact, not a runtime
        // shortcut: both stubs fail closed.
        output.push_str(
            "\npub fn __wrela_pixels_p8_ratio_bounds(numerator_lo: f32, numerator_hi: f32, denominator_lo: f32, denominator_hi: f32) -> [f32; 2]:\n\
             \x20   return [1.0, -1.0]\n\
             \npub fn __wrela_pixels_p8_axis_box_coverage(renderer: usize, x: u32, y: u32, read camera: [f32; 12]) -> [i64; 4]:\n\
             \x20   return [0; 4]\n",
        );
    }
    output.push_str(
        "\npub fn __wrela_pixels_p7_feature_normal(renderer: usize, feature: u32, u: f32, v: f32, q: f32, read params: [f32; 16], read camera: [f32; 12]) -> [f32; 4]:\n\
         \x20   if not q > 0.0:\n\
         \x20       return [0.0; 4]\n",
    );
    for (renderer_index, renderer) in compiled.iter().enumerate() {
        use super::graph::{FieldKind, Primitive};

        for feature in &renderer.structural.program().features {
            if feature.occurrence_path.len() != 1 {
                continue;
            }
            let node = renderer.symbolic.fields.get(feature.primitive)?;
            match &node.kind {
                FieldKind::Primitive(Primitive::Plane { normal, .. }) => {
                    let values = normal
                        .iter()
                        .map(|scalar| {
                            renderer
                                .symbolic
                                .scalar
                                .get(*scalar)
                                .ok()
                                .and_then(super::scalar::constant_bits)
                                .map(f32::from_bits)
                        })
                        .collect::<Option<Vec<_>>>();
                    let Some(values) = values else {
                        continue;
                    };
                    let length =
                        (values[0] * values[0] + values[1] * values[1] + values[2] * values[2])
                            .sqrt();
                    if !length.is_finite() || length == 0.0 {
                        continue;
                    }
                    let nx = wrela_f32_literal(values[0] / length)?;
                    let ny = wrela_f32_literal(values[1] / length)?;
                    let nz = wrela_f32_literal(values[2] / length)?;
                    writeln!(
                        output,
                        "    if renderer == {renderer_index} and feature == {}:\n\
                         \x20       return [1.0, {nx}, {ny}, {nz}]",
                        feature.id.0,
                    )
                    .expect("String writes cannot fail");
                }
                FieldKind::Primitive(Primitive::Sphere { center, .. }) => {
                    let values = center
                        .iter()
                        .map(|scalar| {
                            renderer
                                .symbolic
                                .scalar
                                .get(*scalar)
                                .ok()
                                .and_then(super::scalar::constant_bits)
                                .map(f32::from_bits)
                        })
                        .collect::<Option<Vec<_>>>();
                    let Some(values) = values else {
                        continue;
                    };
                    let cx = wrela_f32_literal(values[0])?;
                    let cy = wrela_f32_literal(values[1])?;
                    let cz = wrela_f32_literal(values[2])?;
                    writeln!(
                        output,
                        "    if renderer == {renderer_index} and feature == {}:\n\
                         \x20       ray_x = camera[3] + u * camera[6] + v * camera[9]\n\
                         \x20       ray_y = camera[4] + u * camera[7] + v * camera[10]\n\
                         \x20       ray_z = camera[5] + u * camera[8] + v * camera[11]\n\
                         \x20       nx = camera[0] + ray_x / q - {cx}\n\
                         \x20       ny = camera[1] + ray_y / q - {cy}\n\
                         \x20       nz = camera[2] + ray_z / q - {cz}\n\
                         \x20       length = sqrt_scalar(nx * nx + ny * ny + nz * nz)\n\
                         \x20       if not length > 0.0 or not __wrela_pixels_p5_finite(length):\n\
                         \x20           return [0.0; 4]\n\
                         \x20       return [1.0, nx / length, ny / length, nz / length]",
                        feature.id.0,
                    )
                    .expect("String writes cannot fail");
                }
                _ => {}
            }
        }
    }
    output.push_str("    return [0.0; 4]\n");
    for (renderer_index, renderer) in compiled.iter().enumerate() {
        use super::events::EventRepresentation;

        let material_events = renderer
            .projective
            .program()
            .events
            .generators
            .iter()
            .filter_map(|event| match &event.representation {
                EventRepresentation::MaterialDifferenceTaylorPredicate { left, right, .. } => {
                    Some((event.id, *left, *right))
                }
                _ => None,
            })
            .collect::<Vec<_>>();
        if material_events.is_empty() {
            writeln!(
                output,
                "\npub fn __wrela_pixels_p7_event_scalar_difference_r{renderer_index}(event: u32, u: f32, v: f32, q: f32, read params: [f32; 16], read camera: [f32; 12]) -> [f32; 2]:\n\
                 \x20   return [0.0, 0.0]"
            )
            .expect("String writes cannot fail");
            continue;
        }
        let required_scalars = scalar_dependency_closure(
            renderer,
            material_events
                .iter()
                .flat_map(|(_, left, right)| [*left, *right])
                .collect(),
        )?;
        writeln!(
            output,
            "\npub fn __wrela_pixels_p7_event_scalar_difference_r{renderer_index}(event: u32, u: f32, v: f32, q: f32, read params: [f32; 16], read camera: [f32; 12]) -> [f32; 2]:\n\
             \x20   if q == 0.0:\n\
             \x20       return [0.0, 0.0]\n\
             \x20   ray_x = camera[3] + u * camera[6] + v * camera[9]\n\
             \x20   ray_y = camera[4] + u * camera[7] + v * camera[10]\n\
             \x20   ray_z = camera[5] + u * camera[8] + v * camera[11]\n\
             \x20   p_x = camera[0] + ray_x / q\n\
             \x20   p_y = camera[1] + ray_y / q\n\
             \x20   p_z = camera[2] + ray_z / q"
        )
        .expect("String writes cannot fail");
        for scalar in &required_scalars {
            writeln!(output, "    __p7_scalar_{scalar}: f32 = 0.0")
                .expect("String writes cannot fail");
        }
        write_scalar_evaluator(output, renderer, &required_scalars, true, false)?;
        let mut scalar_groups = Vec::new();
        for (event, left, right) in material_events {
            if let Some((_, event_end, group_left, group_right)) = scalar_groups.last_mut()
                && *event_end == event.0
                && *group_left == left
                && *group_right == right
            {
                *event_end = event.0 + 1;
                continue;
            }
            scalar_groups.push((event.0, event.0 + 1, left, right));
        }
        for (event_start, event_end, left, right) in scalar_groups {
            writeln!(
                output,
                "    if event >= {event_start} and event < {event_end}:\n\
                 \x20       return [1.0, {} - {}]",
                scalar_slot(left),
                scalar_slot(right),
            )
            .expect("String writes cannot fail");
        }
        output.push_str("    return [0.0, 0.0]\n");
    }
    for (renderer_index, renderer) in compiled.iter().enumerate() {
        writeln!(
            output,
            "\npub fn __wrela_pixels_p7_feature_support_q_span_r{renderer_index}(feature: u32, object: u32, read uv: [f32; 2], read q_domain: [i64; 3]) -> [i64; 3]:"
        )
        .expect("String writes cannot fail");
        let has_composed_roots = renderer
            .projective
            .program()
            .derivatives
            .clusters
            .iter()
            .any(|cluster| cluster_requires_semantic_tube(renderer, cluster));
        if has_composed_roots {
            writeln!(
                output,
                "    exponent = q_domain[0].to[i32]()\n\
                 \x20   q_lo = q_domain[1].to[i32]()\n\
                 \x20   q_hi = q_domain[2].to[i32]()\n\
                 \x20   if q_lo >= q_hi:\n\
                 \x20       return [2, 0, 0]\n\
                 \x20   params = __wrela_pixels_p7_frame_snapshot_params({renderer_index})\n\
                 \x20   camera = __wrela_pixels_p7_frame_snapshot_camera({renderer_index})\n\
                 \x20   support = __wrela_pixels_p7_object_support({renderer_index}, object)\n\
                 \x20   polynomial = __wrela_pixels_p7_feature_polynomial({renderer_index}, feature, uv[0], uv[1], params, camera)\n\
                 \x20   if support[0] != 1.0 or polynomial[0] != 1.0:\n\
                 \x20       return [0; 3]\n\
                 \x20   if polynomial[1] != 1.0:\n\
                 \x20       return [3, q_domain[1], q_domain[2]]\n\
                 \x20   # For positive q, leaf <= support is exactly\n\
                 \x20   # Phi(q) - support*q <= 0. The generated support is\n\
                 \x20   # outward-rounded upward, so uncertainty only retains q.\n\
                 \x20   polynomial[3] = polynomial[3] - support[1]\n\
                 \x20   power: [Iv32; 9] = [Iv32.point(0); 9]\n\
                 \x20   coefficient: usize = 0\n\
                 \x20   @budget(bound=2)\n\
                 \x20   while coefficient < 2:\n\
                 \x20       converted = __wrela_pixels_p7_interval_from_f32(polynomial[coefficient + 2], exponent)\n\
                 \x20       if converted[0] != 1:\n\
                 \x20           return [0; 3]\n\
                 \x20       power[coefficient] = Iv32.range(converted[1].to[i32](), converted[2].to[i32]())\n\
                 \x20       coefficient = coefficient + 1\n\
                 \x20   domain = FixedDomain.full(exponent)\n\
                 \x20   left: Iv32 = Iv32.point(0)\n\
                 \x20   right: Iv32 = Iv32.point(0)\n\
                 \x20   match polynomial_horner9(power, 1, Iv32.point(q_lo), domain):\n\
                 \x20       case .Value(value):\n\
                 \x20           left = value\n\
                 \x20       case _:\n\
                 \x20           return [1, q_domain[1], q_domain[2]]\n\
                 \x20   match polynomial_horner9(power, 1, Iv32.point(q_hi), domain):\n\
                 \x20       case .Value(value):\n\
                 \x20           right = value\n\
                 \x20       case _:\n\
                 \x20           return [1, q_domain[1], q_domain[2]]\n\
                 \x20   if left.lower() > 0 and right.lower() > 0:\n\
                 \x20       return [2, 0, 0]\n\
                 \x20   if left.upper() <= 0 and right.upper() <= 0:\n\
                 \x20       return [1, q_domain[1], q_domain[2]]\n\
                 \x20   isolated = __wrela_pixels_p7_analytic_front(polynomial, power, 1, exponent, q_lo, q_hi, domain)\n\
                 \x20   if isolated[0] != 1:\n\
                 \x20       return [1, q_domain[1], q_domain[2]]\n\
                 \x20   if left.upper() <= 0:\n\
                 \x20       return [1, q_domain[1], isolated[3]]\n\
                 \x20   if right.upper() <= 0:\n\
                 \x20       return [1, isolated[2], q_domain[2]]"
            )
            .expect("String writes cannot fail");
        }
        output.push_str("    return [3, q_domain[1], q_domain[2]]\n");
    }
    output.push_str(
        "\npub fn __wrela_pixels_p7_feature_support_q_span(renderer: usize, feature: u32, object: u32, read uv: [f32; 2], read q_domain: [i64; 3]) -> [i64; 3]:\n",
    );
    for renderer_index in 0..compiled.len() {
        writeln!(
            output,
            "    if renderer == {renderer_index}:\n\
             \x20       return __wrela_pixels_p7_feature_support_q_span_r{renderer_index}(feature, object, uv, q_domain)"
        )
        .expect("String writes cannot fail");
    }
    output.push_str("    return [0; 3]\n");
    for (renderer_index, renderer) in compiled.iter().enumerate() {
        use super::events::EventRepresentation;

        let material_events = renderer
            .projective
            .program()
            .events
            .generators
            .iter()
            .filter(|event| {
                matches!(
                    event.representation,
                    EventRepresentation::MaterialDifferenceTaylorPredicate { .. }
                )
            })
            .collect::<Vec<_>>();
        if material_events.is_empty() {
            writeln!(
                output,
                "\npub fn __wrela_pixels_p7_material_event_coverage_r{renderer_index}(x: u32, y: u32, q: f32, hit: bool, read params: [f32; 16], read camera: [f32; 12]) -> [i64; 3]:\n\
                 \x20   return [1, 0, 255]"
            )
            .expect("String writes cannot fail");
            continue;
        }
        writeln!(
            output,
            "\npub fn __wrela_pixels_p7_material_event_cell_r{renderer_index}(event: u32, x: u32, y: u32, q: f32, hit: bool, read params: [f32; 16], read camera: [f32; 12]) -> [i64; 3]:\n\
             \x20   width: f32 = {}.0\n\
             \x20   height: f32 = {}.0\n\
             \x20   aspect = width / height\n\
             \x20   u0 = ((x.to[f32]() / width) * 2.0 - 1.0) * aspect\n\
             \x20   u1 = (((x + 1).to[f32]() / width) * 2.0 - 1.0) * aspect\n\
             \x20   v0 = 1.0 - (y.to[f32]() / height) * 2.0\n\
             \x20   v1 = 1.0 - ((y + 1).to[f32]() / height) * 2.0\n\
             \x20   f00 = __wrela_pixels_p7_event_scalar_difference_r{renderer_index}(event, u0, v0, q, params, camera)\n\
             \x20   f10 = __wrela_pixels_p7_event_scalar_difference_r{renderer_index}(event, u1, v0, q, params, camera)\n\
             \x20   f01 = __wrela_pixels_p7_event_scalar_difference_r{renderer_index}(event, u0, v1, q, params, camera)\n\
             \x20   f11 = __wrela_pixels_p7_event_scalar_difference_r{renderer_index}(event, u1, v1, q, params, camera)\n\
             \x20   fc = __wrela_pixels_p7_event_scalar_difference_r{renderer_index}(event, (u0 + u1) * 0.5, (v0 + v1) * 0.5, q, params, camera)\n\
             \x20   if f00[0] != 1.0 or f10[0] != 1.0 or f01[0] != 1.0 or f11[0] != 1.0 or fc[0] != 1.0:\n\
             \x20       return [0; 3]\n\
             \x20   samples: [f32; 5] = [f00[1], f10[1], f01[1], f11[1], fc[1]]\n\
             \x20   all_negative = true\n\
             \x20   all_positive = true\n\
             \x20   magnitude: f32 = 0.0\n\
             \x20   sample: usize = 0\n\
             \x20   @budget(bound=5)\n\
             \x20   while sample < 5:\n\
             \x20       value = samples[sample]\n\
             \x20       if value >= 0.0:\n\
             \x20           all_negative = false\n\
             \x20       if value <= 0.0:\n\
             \x20           all_positive = false\n\
             \x20       absolute = __wrela_pixels_p7_abs(value)\n\
             \x20       if absolute > magnitude:\n\
             \x20           magnitude = absolute\n\
             \x20       sample = sample + 1\n\
             \x20   if all_negative or all_positive:\n\
             \x20       return [1, 0, 255]\n\
             \x20   if not magnitude > 0.0:\n\
             \x20       return [0; 3]\n\
             \x20   scale = 262144.0 / magnitude\n\
             \x20   c = __wrela_pixels_p7_round_i32(f00[1] * scale)\n\
             \x20   a = __wrela_pixels_p7_round_i32((f10[1] - f00[1]) * scale)\n\
             \x20   b = __wrela_pixels_p7_round_i32((f01[1] - f00[1]) * scale)\n\
             \x20   corner_residual = __wrela_pixels_p7_abs(f11[1] - f10[1] - f01[1] + f00[1])\n\
             \x20   center_residual = __wrela_pixels_p7_abs(fc[1] - (f00[1] + f10[1] + f01[1] + f11[1]) * 0.25)\n\
             \x20   if corner_residual * scale > 0.5 or center_residual * scale > 0.5:\n\
             \x20       return [0; 3]\n\
             \x20   # Nearest-integer coefficient error is at most 1.5 over\n\
             \x20   # the unit square; the accepted affine residual adds 0.5.\n\
             \x20   # Both extrema must therefore round to the same byte.\n\
             \x20   exact_lo = __wrela_pixels_p7_half_plane_byte(a, b, c - 2)\n\
             \x20   exact_hi = __wrela_pixels_p7_half_plane_byte(a, b, c + 2)\n\
             \x20   if exact_lo[0] != 1 or exact_hi[0] != 1 or exact_lo[1] != exact_hi[1]:\n\
             \x20       return [0; 3]\n\
             \x20   coverage = exact_lo[1]\n\
             \x20   center_positive = fc[1] >= 0.0\n\
             \x20   if hit != center_positive:\n\
             \x20       coverage = 255 - coverage\n\
             \x20   return [1, 1, coverage]",
            renderer.config.width,
            renderer.config.height,
        )
        .expect("String writes cannot fail");
        writeln!(
            output,
            "\npub fn __wrela_pixels_p7_material_event_coverage_r{renderer_index}(x: u32, y: u32, q: f32, hit: bool, read params: [f32; 16], read camera: [f32; 12]) -> [i64; 3]:\n\
             \x20   result: [i64; 3] = [1, 0, 255]"
        )
        .expect("String writes cannot fail");
        let mut material_groups = Vec::<(u32, u32, u32, u32)>::new();
        for event in material_events {
            if let Some(group) = material_groups.last_mut()
                && group.1 == event.id.0
                && group.2 == event.pixels.y.start
                && group.3 == event.pixels.y.end
            {
                group.1 = event.id.0 + 1;
            } else {
                material_groups.push((
                    event.id.0,
                    event.id.0 + 1,
                    event.pixels.y.start,
                    event.pixels.y.end,
                ));
            }
        }
        for (group_index, (event_start, event_end, y_start, y_end)) in
            material_groups.into_iter().enumerate()
        {
            let event_count = event_end - event_start;
            writeln!(
                output,
                "    if y >= {y_start} and y < {y_end}:\n\
                 \x20       material_event_{group_index}: u32 = {event_start}\n\
                 \x20       @budget(bound={event_count})\n\
                 \x20       while material_event_{group_index} < {event_end}:\n\
                 \x20           candidate = __wrela_pixels_p7_material_event_cell_r{renderer_index}(material_event_{group_index}, x, y, q, hit, params, camera)\n\
                 \x20           if candidate[0] != 1:\n\
                 \x20               return [0; 3]\n\
                 \x20           if candidate[1] == 1:\n\
                 \x20               if result[1] == 1 and result[2] != candidate[2]:\n\
                 \x20                   return [0; 3]\n\
                 \x20               result = candidate\n\
                 \x20           material_event_{group_index} = material_event_{group_index} + 1",
            )
            .expect("String writes cannot fail");
        }
        output.push_str("    return result\n");
    }
    output.push_str(
        "\npub fn __wrela_pixels_p7_material_event_coverage(renderer: usize, x: u32, y: u32, q: f32, hit: bool, read params: [f32; 16], read camera: [f32; 12]) -> [i64; 3]:\n",
    );
    for renderer_index in 0..compiled.len() {
        writeln!(
            output,
            "    if renderer == {renderer_index}:\n\
             \x20       return __wrela_pixels_p7_material_event_coverage_r{renderer_index}(x, y, q, hit, params, camera)"
        )
        .expect("String writes cannot fail");
    }
    output.push_str("    return [0; 3]\n");
    output.push_str(
        "\n\
         pub fn __wrela_pixels_p7_feature_valid(renderer: usize, feature: u32, u: f32, v: f32, q: f32, read params: [f32; 16], read camera: [f32; 12]) -> bool:\n\
         \x20   return __wrela_pixels_p7_sealed_feature_valid(renderer, feature, u, v, q, params, camera)\n\
         \n\
         pub fn __wrela_pixels_p7_feature_valid_filter(renderer: usize, feature: u32, read uv: [f32; 4], read q: [i32; 2], exponent: i32, read params: [f32; 16], read camera: [f32; 12]) -> [i64; 2]:\n\
         \x20   return __wrela_pixels_p7_sealed_feature_valid_filter(renderer, feature, uv, q, exponent, params, camera)\n",
    );
    output.push_str(
        "\npub fn __wrela_pixels_p7_object_support(renderer: usize, object: u32) -> [f32; 2]:\n\
             \x20   record = __wrela_pixels_program_record(renderer, 3, object)\n\
             \x20   support = __wrela_pixels_program_operand(renderer, 3, object, 7)\n\
             \x20   if record[0] != 1 or record[1] != object.to[u64]() or support[0] != 1:\n\
             \x20       return [0.0, 0.0]\n\
             \x20   value = __wrela_pixels_f64_bits_to_f32(support[1])\n\
             \x20   if not __wrela_pixels_p5_finite(value):\n\
             \x20       return [0.0, 0.0]\n\
             \x20   return [1.0, __wrela_pixels_p7_outward_high(value)]\n",
    );
    output.push_str(
        "\npub fn __wrela_pixels_p7_feature_q_span(renderer: usize, feature: u32) -> [f32; 3]:\n\
             \x20   record = __wrela_pixels_program_record(renderer, 4, feature)\n\
             \x20   lo = __wrela_pixels_program_operand(renderer, 4, feature, 16)\n\
             \x20   hi = __wrela_pixels_program_operand(renderer, 4, feature, 17)\n\
             \x20   if record[0] != 1 or record[1] != feature.to[u64]() or lo[0] != 1 or hi[0] != 1:\n\
             \x20       return [0.0; 3]\n\
             \x20   lo_value = __wrela_pixels_f64_bits_to_f32(lo[1])\n\
             \x20   hi_value = __wrela_pixels_f64_bits_to_f32(hi[1])\n\
             \x20   if not __wrela_pixels_p5_finite(lo_value) or not __wrela_pixels_p5_finite(hi_value):\n\
             \x20       return [0.0; 3]\n\
             \x20   return [1.0, __wrela_pixels_p7_outward_low(lo_value), __wrela_pixels_p7_outward_high(hi_value)]\n",
    );
    output.push_str(
        "\npub fn __wrela_pixels_p7_feature_world_bounds(renderer: usize, feature: u32) -> [f32; 7]:\n\
             \x20   record = __wrela_pixels_program_record(renderer, 4, feature)\n\
             \x20   if record[0] != 1 or record[1] != feature.to[u64]() or record[4] < 6 or record[4] > 65535:\n\
             \x20       return [0.0; 7]\n\
             \x20   base = record[4] - 6\n\
             \x20   result: [f32; 7] = [0.0; 7]\n\
             \x20   result[0] = 1.0\n\
             \x20   bound: usize = 0\n\
             \x20   @budget(bound=6)\n\
             \x20   while bound < 6:\n\
             \x20       encoded = __wrela_pixels_program_operand(renderer, 4, feature, (base + bound.to[u64]()).to[u16]())\n\
             \x20       if encoded[0] != 1:\n\
             \x20           return [0.0; 7]\n\
             \x20       value = __wrela_pixels_f64_bits_to_f32(encoded[1])\n\
             \x20       if not __wrela_pixels_p5_finite(value):\n\
             \x20           return [0.0; 7]\n\
             \x20       if bound < 3:\n\
             \x20           result[bound + 1] = __wrela_pixels_p7_outward_low(value)\n\
             \x20       else:\n\
             \x20           result[bound + 1] = __wrela_pixels_p7_outward_high(value)\n\
             \x20       bound = bound + 1\n\
             \x20   return result\n",
    );
    // Replay the authoritative source-order coordinate transforms for texture
    // evaluation. The returned point, normal, and footprint derivatives all
    // live in the primitive/object frame; world triplanar mapping deliberately
    // bypasses this helper in `core.render_light`.
    for (renderer_index, renderer) in compiled.iter().enumerate() {
        use super::graph::FieldKind;
        use super::material_graph::{MaterialKind, NormalModel, UvSourceV1};

        let structural = renderer.structural.program();
        let needs_local_frame = renderer.symbolic.materials.iter().any(|(_, material)| {
            let MaterialKind::Sample(sample) = &material.kind else {
                return false;
            };
            sample
                .pattern
                .as_ref()
                .is_some_and(|texture| texture.uv_source != UvSourceV1::WorldTriplanar)
                || matches!(
                    &sample.normal,
                    NormalModel::TextureSlope { texture }
                        if texture.uv_source != UvSourceV1::WorldTriplanar
                )
        });
        if !needs_local_frame {
            writeln!(
                output,
                "\npub fn __wrela_pixels_p9_feature_local_frame_r{renderer_index}(feature: u32, read point: [f32; 3], read normal: [f32; 3], read d_p_dx: [f32; 3], read d_p_dy: [f32; 3], read params: [f32; 16]) -> [f32; 13]:\n\
                 \x20   return [0.0; 13]"
            )
            .expect("String writes cannot fail");
            continue;
        }
        let mut roots = Vec::new();
        for feature in &structural.features {
            for step in feature.occurrence_path.iter().skip(1) {
                match &renderer.symbolic.fields.get(step.field)?.kind {
                    FieldKind::Transform { transform, .. } => {
                        transform_scalar_ids(transform, &mut roots);
                    }
                    FieldKind::FiniteRepeat { period, .. } => roots.push(*period),
                    _ => {}
                }
            }
        }
        let required_scalars = scalar_dependency_closure(renderer, roots)?;
        writeln!(
            output,
            "\npub fn __wrela_pixels_p9_feature_local_frame_r{renderer_index}(feature: u32, read point: [f32; 3], read normal: [f32; 3], read d_p_dx: [f32; 3], read d_p_dy: [f32; 3], read params: [f32; 16]) -> [f32; 13]:\n\
             \x20   p_x = point[0]\n\
             \x20   p_y = point[1]\n\
             \x20   p_z = point[2]"
        )
        .expect("String writes cannot fail");
        for scalar in &required_scalars {
            writeln!(output, "    __p7_scalar_{scalar}: f32 = 0.0")
                .expect("String writes cannot fail");
        }
        write_scalar_evaluator(output, renderer, &required_scalars, true, false)?;
        for feature in &structural.features {
            writeln!(
                output,
                "    if feature == {}:\n\
                 \x20       local_p_x = point[0]\n\
                 \x20       local_p_y = point[1]\n\
                 \x20       local_p_z = point[2]\n\
                 \x20       local_n_x = normal[0]\n\
                 \x20       local_n_y = normal[1]\n\
                 \x20       local_n_z = normal[2]\n\
                 \x20       local_dx_x = d_p_dx[0]\n\
                 \x20       local_dx_y = d_p_dx[1]\n\
                 \x20       local_dx_z = d_p_dx[2]\n\
                 \x20       local_dy_x = d_p_dy[0]\n\
                 \x20       local_dy_y = d_p_dy[1]\n\
                 \x20       local_dy_z = d_p_dy[2]",
                feature.id.0,
            )
            .expect("String writes cannot fail");
            let object = structural
                .objects
                .objects
                .iter()
                .find(|object| object.id == feature.object)
                .ok_or_else(|| {
                    format!(
                        "pixels::glue: feature {} refers to missing object {}",
                        feature.id, feature.object,
                    )
                })?;
            let mut temporary = 0_u32;
            for step in feature.occurrence_path.iter().skip(1) {
                match &renderer.symbolic.fields.get(step.field)?.kind {
                    FieldKind::Transform { transform, .. } => {
                        write_p9_local_transform(output, transform, &mut temporary)?;
                    }
                    FieldKind::FiniteRepeat {
                        axis,
                        first,
                        period,
                        ..
                    } => {
                        let instance = object
                            .repeat_instances
                            .iter()
                            .find(|instance| {
                                instance.repeat_field == step.field
                                    || instance.equivalent_fields.contains(&step.field)
                            })
                            .ok_or_else(|| {
                                format!(
                                    "pixels::glue: feature {} lacks repeat instance for {}",
                                    feature.id, step.field,
                                )
                            })?;
                        let ordinal = instance.index.checked_sub(*first).ok_or_else(|| {
                            "pixels::glue: feature repeat ordinal underflow".to_string()
                        })?;
                        let source_first = wrela_f32_literal(*first as f32)?;
                        let source_ordinal = wrela_f32_literal(ordinal as f32)?;
                        let component = match axis {
                            super::graph::Axis::X => "x",
                            super::graph::Axis::Y => "y",
                            super::graph::Axis::Z => "z",
                        };
                        writeln!(
                            output,
                            "        local_p_{component} = local_p_{component} - ({source_first} + {source_ordinal}) * {}",
                            scalar_slot(*period),
                        )
                        .expect("String writes cannot fail");
                    }
                    _ => {}
                }
            }
            output.push_str(
                "        return [\n\
                 \x20           1.0, local_p_x, local_p_y, local_p_z,\n\
                 \x20           local_n_x, local_n_y, local_n_z,\n\
                 \x20           local_dx_x, local_dx_y, local_dx_z,\n\
                 \x20           local_dy_x, local_dy_y, local_dy_z,\n\
                 \x20       ]\n",
            );
        }
        output.push_str("    return [0.0; 13]\n");
    }
    output.push_str(
        "\npub fn __wrela_pixels_p9_feature_local_frame(renderer: usize, feature: u32, read point: [f32; 3], read normal: [f32; 3], read d_p_dx: [f32; 3], read d_p_dy: [f32; 3], read params: [f32; 16]) -> [f32; 13]:\n",
    );
    for renderer_index in 0..compiled.len() {
        writeln!(
            output,
            "    if renderer == {renderer_index}:\n\
             \x20       return __wrela_pixels_p9_feature_local_frame_r{renderer_index}(feature, point, normal, d_p_dx, d_p_dy, params)"
        )
        .expect("String writes cannot fail");
    }
    output.push_str("    return [0.0; 13]\n");
    for (renderer_index, renderer) in compiled.iter().enumerate() {
        let required_scalars = scalar_dependency_closure(
            renderer,
            renderer
                .structural
                .program()
                .objects
                .objects
                .iter()
                .filter(|object| object_requires_semantic_scalar(renderer, object))
                .map(|object| object.scalar_root)
                .collect(),
        )?;
        writeln!(
            output,
            "\npub fn __wrela_pixels_p7_object_scalar_r{renderer_index}(object: u32, u: f32, v: f32, q: f32, read params: [f32; 16], read camera: [f32; 12]) -> [f32; 2]:\n\
             \x20   if q == 0.0:\n\
             \x20       return [0.0, 0.0]\n\
             \x20   ray_x = camera[3] + u * camera[6] + v * camera[9]\n\
             \x20   ray_y = camera[4] + u * camera[7] + v * camera[10]\n\
             \x20   ray_z = camera[5] + u * camera[8] + v * camera[11]\n\
             \x20   p_x = camera[0] + ray_x / q\n\
             \x20   p_y = camera[1] + ray_y / q\n\
             \x20   p_z = camera[2] + ray_z / q"
        )
        .expect("String writes cannot fail");
        for scalar in &required_scalars {
            writeln!(output, "    __p7_scalar_{scalar}: f32 = 0.0")
                .expect("String writes cannot fail");
        }
        write_scalar_evaluator(output, renderer, &required_scalars, true, false)?;
        for object in renderer
            .structural
            .program()
            .objects
            .objects
            .iter()
            .filter(|object| object_requires_semantic_scalar(renderer, object))
        {
            writeln!(
                output,
                "    if object == {}:\n        return [1.0, {}]",
                object.id.0,
                scalar_slot(object.scalar_root),
            )
            .expect("String writes cannot fail");
        }
        output.push_str("    return [0.0, 0.0]\n");
    }
    for (renderer_index, renderer) in compiled.iter().enumerate() {
        writeln!(
            output,
            "\npub fn __wrela_pixels_p7_object_composed_root_r{renderer_index}(object: u32) -> bool:"
        )
        .expect("String writes cannot fail");
        let mut object_masks = vec![
            0_u64;
            renderer
                .structural
                .program()
                .objects
                .objects
                .len()
                .div_ceil(64)
                .max(1)
        ];
        for cluster in renderer
            .projective
            .program()
            .derivatives
            .clusters
            .iter()
            .filter(|cluster| cluster_requires_semantic_tube(renderer, cluster))
        {
            object_masks[cluster.object.index() / 64] |= 1_u64 << (cluster.object.index() % 64);
        }
        for (chunk, mask) in object_masks.into_iter().enumerate() {
            writeln!(
                output,
                "    if object >= {} and object < {}:\n\
                 \x20       return ({mask} & (1.to[u64]() << (object - {}).to[u64]())) != 0",
                chunk * 64,
                (chunk + 1) * 64,
                chunk * 64,
            )
            .expect("String writes cannot fail");
        }
        output.push_str("    return false\n");
    }
    output.push_str(
        "\npub fn __wrela_pixels_p7_object_composed_root(renderer: usize, object: u32) -> bool:\n",
    );
    for renderer_index in 0..compiled.len() {
        writeln!(
            output,
            "    if renderer == {renderer_index}:\n\
             \x20       return __wrela_pixels_p7_object_composed_root_r{renderer_index}(object)"
        )
        .expect("String writes cannot fail");
    }
    output.push_str("    return false\n");
    for (renderer_index, renderer) in compiled.iter().enumerate() {
        let boundary_clusters = renderer
            .projective
            .program()
            .derivatives
            .clusters
            .iter()
            .filter(|cluster| cluster_requires_semantic_tube(renderer, cluster))
            .collect::<Vec<_>>();
        if boundary_clusters.is_empty() {
            writeln!(
                output,
                "\npub fn __wrela_pixels_p7_object_q_tube_r{renderer_index}(object: u32, read uv: [f32; 4], q_lo: f32, q_hi: f32, read params: [f32; 16], read camera: [f32; 12]) -> [f32; 7]:\n\
                 \x20   return [0.0; 7]"
            )
            .expect("String writes cannot fail");
            continue;
        }
        writeln!(
            output,
            "\npub fn __wrela_pixels_p7_object_q_tube_r{renderer_index}(object: u32, read uv: [f32; 4], q_lo: f32, q_hi: f32, read params: [f32; 16], read camera: [f32; 12]) -> [f32; 7]:\n\
             \x20   if q_lo <= 0.0 or q_lo >= q_hi:\n\
             \x20       return [0.0; 7]\n\
             \x20   lower = __wrela_pixels_p7_object_scalar_r{renderer_index}(object, uv[0], uv[1], q_lo, params, camera)\n\
             \x20   upper = __wrela_pixels_p7_object_scalar_r{renderer_index}(object, uv[0], uv[1], q_hi, params, camera)\n\
             \x20   if lower[0] != 1.0 or upper[0] != 1.0:\n\
             \x20       return [0.0; 7]\n\
             \x20   width = q_hi - q_lo\n\
             \x20   secant = (upper[1] - lower[1]) / width\n\
             \x20   ray_x = camera[3] + uv[0] * camera[6] + uv[1] * camera[9]\n\
             \x20   ray_y = camera[4] + uv[0] * camera[7] + uv[1] * camera[10]\n\
             \x20   ray_z = camera[5] + uv[0] * camera[8] + uv[1] * camera[11]\n\
             \x20   ray_delta_x = __wrela_pixels_p7_abs(camera[6]) * uv[2] + __wrela_pixels_p7_abs(camera[9]) * uv[3]\n\
             \x20   ray_delta_y = __wrela_pixels_p7_abs(camera[7]) * uv[2] + __wrela_pixels_p7_abs(camera[10]) * uv[3]\n\
             \x20   ray_delta_z = __wrela_pixels_p7_abs(camera[8]) * uv[2] + __wrela_pixels_p7_abs(camera[11]) * uv[3]\n\
             \x20   inv_q = 1.0 / q_lo\n\
             \x20   inv_q2 = inv_q * inv_q\n\
             \x20   inv_q3 = inv_q2 * inv_q\n\
             \x20   speed_x = __wrela_pixels_p7_abs(ray_x) * inv_q2\n\
             \x20   speed_y = __wrela_pixels_p7_abs(ray_y) * inv_q2\n\
             \x20   speed_z = __wrela_pixels_p7_abs(ray_z) * inv_q2\n\
             \x20   acceleration_x = 2.0 * __wrela_pixels_p7_abs(ray_x) * inv_q3\n\
             \x20   acceleration_y = 2.0 * __wrela_pixels_p7_abs(ray_y) * inv_q3\n\
             \x20   acceleration_z = 2.0 * __wrela_pixels_p7_abs(ray_z) * inv_q3"
        )
        .expect("String writes cannot fail");
        for cluster in boundary_clusters {
            let tube = &cluster.root_tube;
            let first = tube.first_world_abs;
            // First-order f32 rounding budget for this cluster's scalar
            // schedule: one relative epsilon per evaluated node, with a 2x
            // safety factor over the f32 unit roundoff (2^-24 -> 2^-23), and
            // never below the historical flat 2^-16 allowance. This is a
            // running-error style bound using the face/radius magnitudes as
            // the intermediate-magnitude proxy; a fully rigorous closure needs
            // interval evaluation of the schedule and is tracked in the plan.
            let schedule_object = renderer
                .structural
                .program()
                .objects
                .objects
                .iter()
                .find(|object| object.id == cluster.object)
                .ok_or_else(|| {
                    format!(
                        "pixels::glue: tube cluster object {} has no structural object",
                        cluster.object.0
                    )
                })?;
            let schedule_ops =
                scalar_dependency_closure(renderer, vec![schedule_object.scalar_root])?.len();
            let epsilon =
                ((schedule_ops as f64) * (2.0_f64).powi(-23)).max((2.0_f64).powi(-16)) as f32;
            writeln!(
                output,
                "    if object == {}:\n\
                 \x20       speed = speed_x + speed_y + speed_z\n\
                 \x20       second = {} * speed * speed + {} * acceleration_x + {} * acceleration_y + {} * acceleration_z\n\
                 \x20       ray_delta = ray_delta_x + ray_delta_y + ray_delta_z\n\
                 \x20       point_delta = ray_delta * inv_q\n\
                 \x20       # `lower[0]` is exactly 1.0 here (guarded above); the\n\
                 \x20       # multiplication is an f32 type ascription for the bare\n\
                 \x20       # gradient-bound literals, which would otherwise default\n\
                 \x20       # to f64 and fail the typed re-check.\n\
                 \x20       gradient = {} * lower[0] + {} * lower[0] + {} * lower[0]\n\
                 \x20       ray_magnitude = __wrela_pixels_p7_abs(ray_x) + __wrela_pixels_p7_abs(ray_y) + __wrela_pixels_p7_abs(ray_z) + ray_delta\n\
                 \x20       value_radius = gradient * point_delta\n\
                 \x20       derivative_radius = {} * point_delta * ray_magnitude * inv_q2 + gradient * ray_delta * inv_q2\n\
                 \x20       # Face-value rounding of the two schedule evaluations.\n\
                 \x20       eval_error = (__wrela_pixels_p7_abs(lower[1]) + __wrela_pixels_p7_abs(upper[1]) + value_radius + 1.0) * {}\n\
                 \x20       # The secant divides the face difference by the cell\n\
                 \x20       # width, so its evaluation error is amplified by 1/width\n\
                 \x20       # and must scale with it; a flat allowance under-covers\n\
                 \x20       # narrow cells and would fake derivative certificates.\n\
                 \x20       secant_error = (eval_error + eval_error) / width\n\
                 \x20       model_error = (__wrela_pixels_p7_abs(secant) + derivative_radius + 1.0) * {}\n\
                 \x20       radius = second * width + derivative_radius + secant_error + model_error\n\
                 \x20       face_radius = value_radius + eval_error\n\
                 \x20       return [1.0, lower[1] - face_radius, lower[1] + face_radius, upper[1] - face_radius, upper[1] + face_radius, secant - radius, secant + radius]",
                cluster.object.0,
                wrela_f32_literal(tube.second_world_abs as f32)?,
                wrela_f32_literal(first[0] as f32)?,
                wrela_f32_literal(first[1] as f32)?,
                wrela_f32_literal(first[2] as f32)?,
                wrela_f32_literal(first[0] as f32)?,
                wrela_f32_literal(first[1] as f32)?,
                wrela_f32_literal(first[2] as f32)?,
                wrela_f32_literal(tube.second_world_abs as f32)?,
                wrela_f32_literal(epsilon)?,
                wrela_f32_literal(epsilon)?,
            )
            .expect("String writes cannot fail");
        }
        output.push_str("    return [0.0; 7]\n");
    }
    output.push_str(
        "\n\
         pub fn __wrela_pixels_p7_object_q_tube(renderer: usize, object: u32, read uv: [f32; 4], q_lo: f32, q_hi: f32, read params: [f32; 16], read camera: [f32; 12]) -> [f32; 7]:\n",
    );
    for renderer_index in 0..compiled.len() {
        writeln!(
            output,
            "    if renderer == {renderer_index}:\n\
             \x20       return __wrela_pixels_p7_object_q_tube_r{renderer_index}(object, uv, q_lo, q_hi, params, camera)"
        )
        .expect("String writes cannot fail");
    }
    output.push_str("    return [0.0; 7]\n");
    output.push_str(
        "\n\
         pub fn __wrela_pixels_p7_polynomial_at_q(read polynomial: [f32; 11], q: f32) -> [f32; 2]:\n\
         \x20   if polynomial[0] != 1.0:\n\
         \x20       return [0.0, 0.0]\n\
         \x20   degree = polynomial[1].to[usize]()\n\
         \x20   if degree == 0 or degree > 8:\n\
         \x20       return [0.0, 0.0]\n\
         \x20   value = polynomial[degree + 2]\n\
         \x20   coefficient = degree\n\
         \x20   @budget(bound=8)\n\
         \x20   while coefficient > 0:\n\
         \x20       coefficient = coefficient - 1\n\
         \x20       value = value * q + polynomial[coefficient + 2]\n\
         \x20   if value != value or value == 0.0:\n\
         \x20       return [0.0, 0.0]\n\
         \x20   return [1.0, value]\n",
    );
    for (renderer_index, renderer) in compiled.iter().enumerate() {
        writeln!(
            output,
            "\npub fn __wrela_pixels_p7_initial_inside_r{renderer_index}(u: f32, v: f32, q_near: f32, read params: [f32; 16], read camera: [f32; 12]) -> [u64; 2]:\n\
             \x20   bits: u64 = 0"
        )
        .expect("String writes cannot fail");
        let objects = &renderer.structural.program().objects.objects;
        for object in objects {
            if object_requires_semantic_scalar(renderer, object) {
                writeln!(
                    output,
                    "    object_{} = __wrela_pixels_p7_object_scalar_r{renderer_index}({}, u, v, q_near, params, camera)\n\
                     \x20   if object_{}[0] != 1.0 or object_{}[1] != object_{}[1] or object_{}[1] == 0.0:\n\
                     \x20       return [0, 0]\n\
                     \x20   if object_{}[1] < 0.0:\n\
                     \x20       bits = bits | (1.to[u64]() << {}.to[u64]())",
                    object.id.0,
                    object.id.0,
                    object.id.0,
                    object.id.0,
                    object.id.0,
                    object.id.0,
                    object.id.0,
                    object.id.0,
                )
                .expect("String writes cannot fail");
            } else {
                let feature = renderer
                    .structural
                    .program()
                    .features
                    .iter()
                    .find(|feature| feature.object == object.id)
                    .ok_or_else(|| {
                        format!("pixels::glue: object {} has no boundary feature", object.id)
                    })?;
                writeln!(
                    output,
                    "    object_{}_poly = __wrela_pixels_p7_feature_polynomial({renderer_index}, {}, u, v, params, camera)\n\
                     \x20   object_{}_value = __wrela_pixels_p7_polynomial_at_q(object_{}_poly, q_near)\n\
                     \x20   if object_{}_value[0] != 1.0:\n\
                     \x20       return [0, 0]\n\
                     \x20   if object_{}_value[1] < 0.0:\n\
                     \x20       bits = bits | (1.to[u64]() << {}.to[u64]())",
                    object.id.0,
                    feature.id.0,
                    object.id.0,
                    object.id.0,
                    object.id.0,
                    object.id.0,
                    object.id.0,
                )
                .expect("String writes cannot fail");
            }
        }
        output.push_str("    return [1, bits]\n");
    }
    output.push_str(
        "\npub fn __wrela_pixels_p7_initial_inside(renderer: usize, u: f32, v: f32, q_near: f32, read params: [f32; 16], read camera: [f32; 12]) -> [u64; 2]:\n",
    );
    for renderer_index in 0..compiled.len() {
        writeln!(
            output,
            "    if renderer == {renderer_index}:\n\
             \x20       return __wrela_pixels_p7_initial_inside_r{renderer_index}(u, v, q_near, params, camera)"
        )
        .expect("String writes cannot fail");
    }
    output.push_str("    return [0, 0]\n");
    output.push_str(
        "\n\
         pub fn __wrela_pixels_p7_param_slot(renderer: usize, path_key: u64) -> [u64; 2]:\n",
    );
    for (renderer_index, renderer) in compiled.iter().enumerate() {
        let mut keys = BTreeSet::new();
        for (slot, parameter) in renderer.symbolic.params.iter().enumerate() {
            let key = super::params::parameter_path_key(&parameter.path, parameter.component)?;
            if !keys.insert(key) {
                return Err(format!(
                    "pixels::glue: renderer {renderer_index} parameter path-key collision"
                ));
            }
            writeln!(
                output,
                "    if renderer == {renderer_index} and path_key == {key}:\n\
                 \x20       return [1, {slot}]"
            )
            .expect("String writes cannot fail");
        }
    }
    output.push_str("    return [0; 2]\n");
    output.push_str(
        "\n\
         pub fn __wrela_pixels_p7_numeric_config(renderer: usize) -> [i64; 14]:\n",
    );
    for (renderer_index, renderer) in compiled.iter().enumerate() {
        let policy = renderer
            .program
            .program()
            .tables
            .iter()
            .find(|table| table.kind == wrela_machine::pixels::FrameProgramTableKindV1::FixedDomain)
            .and_then(|table| table.records.iter().find(|record| record.tag == 5))
            .ok_or_else(|| "pixels::glue: P7 fixed-q policy is missing".to_string())?;
        let exponent = policy.operands[0] as i64;
        let scale = 2_f64.powi(
            i32::try_from(-exponent)
                .map_err(|_| "pixels::glue: fixed-q exponent exceeds i32".to_string())?,
        );
        let q_lo = (1.0 / renderer.config.far * scale).floor() as i64;
        let q_hi = (1.0 / renderer.config.near * scale).ceil() as i64;
        let table_count = |kind| {
            renderer
                .program
                .program()
                .tables
                .iter()
                .find(|table| table.kind == kind)
                .map_or(0, |table| table.records.len())
        };
        let csg_count = table_count(wrela_machine::pixels::FrameProgramTableKindV1::Csg);
        let object_count = table_count(wrela_machine::pixels::FrameProgramTableKindV1::Object);
        let event_count = table_count(wrela_machine::pixels::FrameProgramTableKindV1::Event);
        let feature_count = table_count(wrela_machine::pixels::FrameProgramTableKindV1::Feature);
        let declared_smooth_depth = renderer
            .projective
            .program()
            .derivatives
            .clusters
            .iter()
            .map(|cluster| cluster.root_tube.subdivision_depth)
            .max()
            .unwrap_or(1);
        // A terminal smooth cell is at most one sealed fixed-q unit wide.
        // Derive the depth from the complete q domain rather than a fixed
        // root-count or subdivision guess.
        let mut fixed_cells = (q_hi - q_lo).unsigned_abs();
        let mut fixed_resolution_depth = 0_u8;
        while fixed_cells > 1 {
            fixed_cells = fixed_cells.div_ceil(2);
            fixed_resolution_depth = fixed_resolution_depth
                .checked_add(1)
                .ok_or_else(|| "pixels::glue: smooth root depth overflow".to_string())?;
        }
        let smooth_subdivision_depth = declared_smooth_depth.max(fixed_resolution_depth);
        writeln!(
            output,
            "    if renderer == {renderer_index}:\n\
             \x20       return [1, {}, {}, {exponent}, {q_lo}, {q_hi}, {}, {}, {csg_count}, {object_count}, {event_count}, {feature_count}, {}, {smooth_subdivision_depth}]",
            renderer.config.width,
            renderer.config.height,
            renderer.symbolic.params.len(),
            renderer.projective.program().capacities.row_start_roots,
            renderer.projective.program().capacities.root_stack_nodes,
        )
        .expect("String writes cannot fail");
    }
    output.push_str("    return [0; 14]\n");
    // Straight-line CSG occupancy compiled from the sealed stack program at
    // generation time. The runtime interpreter this replaces re-read every
    // table record per evaluation — inside the overlap-arrangement
    // enumeration that is 2^k evaluations per group. A malformed sealed
    // program skips generation for its renderer; the dispatcher's [0; 2]
    // fallback preserves the interpreter's fail-closed surface.
    for (renderer_index, renderer) in compiled.iter().enumerate() {
        let compiled_program = renderer
            .program
            .program()
            .tables
            .iter()
            .find(|table| table.kind == wrela_machine::pixels::FrameProgramTableKindV1::Csg)
            .and_then(|table| compile_csg_stack_program(&table.records));
        writeln!(
            output,
            "\npub fn __wrela_pixels_p7_csg_occupancy_r{renderer_index}(inside_bits: u64) -> [i64; 2]:"
        )
        .expect("String writes cannot fail");
        match compiled_program {
            Some(lines) => {
                for line in lines {
                    writeln!(output, "    {line}").expect("String writes cannot fail");
                }
            }
            None => output.push_str("    return [0; 2]\n"),
        }
    }
    output.push_str(
        "\npub fn __wrela_pixels_p7_csg_occupancy(renderer: usize, inside_bits: u64) -> [i64; 2]:\n",
    );
    for renderer_index in 0..compiled.len() {
        writeln!(
            output,
            "    if renderer == {renderer_index}:\n\
             \x20       return __wrela_pixels_p7_csg_occupancy_r{renderer_index}(inside_bits)"
        )
        .expect("String writes cannot fail");
    }
    output.push_str("    return [0; 2]\n");
    // Boolean influence (P7.6 step 2): the set of object bits the sealed
    // CSG program reads at all. A crossing of an object outside this mask
    // can never change composite occupancy, so the sweep toggles its bit
    // and skips the transition work. All-ones is the fail-closed default
    // (skip nothing) for a malformed program.
    output.push_str("\npub fn __wrela_pixels_p7_csg_influence(renderer: usize) -> u64:\n");
    for (renderer_index, renderer) in compiled.iter().enumerate() {
        let influence = renderer
            .program
            .program()
            .tables
            .iter()
            .find(|table| table.kind == wrela_machine::pixels::FrameProgramTableKindV1::Csg)
            .map_or(u64::MAX, |table| {
                let mut mask = 0_u64;
                for record in &table.records {
                    if record.tag == 1 {
                        match record.operands.first() {
                            Some(&object) if object < 64 => mask |= 1_u64 << object,
                            _ => return u64::MAX,
                        }
                    }
                }
                mask
            });
        writeln!(
            output,
            "    if renderer == {renderer_index}:\n\
             \x20       return {influence}"
        )
        .expect("String writes cannot fail");
    }
    output.push_str("    return 18446744073709551615\n");
    write_worker_error_class(output);
    Ok(())
}

/// Generate the guest-side worker error classifier from the single-source
/// table (`pixels::worker_errors`). The coordinator's `__worker_error`
/// dispatches on the returned class instead of restating the code list by
/// hand; a code missing from the table classifies as 0 (internal invariant),
/// which is the fail-closed default.
fn write_worker_error_class(output: &mut String) {
    output.push_str("\npub fn __wrela_pixels_p7_worker_error_class(error: u8) -> u8:\n");
    for spec in crate::pixels::worker_errors::WORKER_ERRORS {
        writeln!(
            output,
            "    if error == {}:\n\
             \x20       # {}: {}\n\
             \x20       return {}",
            spec.code, spec.name, spec.doc, spec.class as u8
        )
        .expect("String writes cannot fail");
    }
    output.push_str("    return 0\n");
    write_event_class(output);
}

/// Generate the guest-side event classifier from the single-source taxonomy
/// (`pixels::event_kinds`). The analytic coverage tiers used to restate the
/// sealed kind and representation numbers as inline literals, so a new
/// occupancy-bearing kind added in Rust would have left the guest unaware
/// that a boundary it must not integrate past even existed.
///
/// An unregistered pairing classifies as 0, which reads to every caller as
/// "not a curve, not a predicate, bounds no occupancy" — the pessimistic
/// answer everywhere except the occupancy bit, which is why that bit is
/// emitted from the kind alone and before any pairing is consulted.
fn write_event_class(output: &mut String) {
    use crate::pixels::event_kinds::{
        ALL_EVENT_KINDS, ALL_REPRESENTATION_TAGS, event_class, kind_bounds_occupancy,
        kind_wire_tag, representation_wire_tag,
    };
    output.push_str(
        "\npub fn __wrela_pixels_p7_event_class(representation: u64, kind: u64) -> u64:\n\
         \x20   class: u64 = 0\n",
    );
    let occupancy: Vec<String> = ALL_EVENT_KINDS
        .iter()
        .copied()
        .filter(|kind| kind_bounds_occupancy(*kind))
        .map(|kind| format!("kind == {}", kind_wire_tag(kind)))
        .collect();
    writeln!(
        output,
        "    # Kinds that can bound where a surface is visible.\n\
         \x20   if {}:\n\
         \x20       class = class | {}",
        occupancy.join(" or "),
        crate::pixels::event_kinds::event_class::OCCUPANCY,
    )
    .expect("String writes cannot fail");
    for representation in ALL_REPRESENTATION_TAGS.iter().copied() {
        // Group the kinds this representation classifies identically, so the
        // emitted dispatch stays one branch per (representation, class).
        let mut by_class: std::collections::BTreeMap<u64, Vec<u64>> =
            std::collections::BTreeMap::new();
        for kind in ALL_EVENT_KINDS.iter().copied() {
            let geometric = event_class(representation, kind)
                & !crate::pixels::event_kinds::event_class::OCCUPANCY;
            if geometric != 0 {
                by_class
                    .entry(geometric)
                    .or_default()
                    .push(kind_wire_tag(kind));
            }
        }
        for (class, kinds) in by_class {
            let guard = kinds
                .iter()
                .map(|kind| format!("kind == {kind}"))
                .collect::<Vec<_>>()
                .join(" or ");
            writeln!(
                output,
                "    if representation == {} and ({guard}):\n\
                 \x20       class = class | {class}",
                representation_wire_tag(representation),
            )
            .expect("String writes cannot fail");
        }
    }
    output.push_str("    return class\n");
}

/// Compile a sealed CSG stack program (frame-program table 8) into
/// straight-line Wrela statements over `inside_bits`. Tags mirror the sealed
/// contract: 1 = push an object's inside bit, 2 = NOT, 3 = AND, 4 = OR,
/// 5/6 = push constant true/false. Returns `None` for any program the runtime
/// interpreter would have rejected (bad tag, stack under/overflow, object
/// index ≥ 64, or a final stack depth other than one).
fn compile_csg_stack_program(
    records: &[wrela_machine::pixels::FrameRecordV1],
) -> Option<Vec<String>> {
    if records.is_empty() || records.len() > 64 {
        return None;
    }
    let mut lines = Vec::new();
    let mut stack: Vec<String> = Vec::new();
    let mut temp = 0_usize;
    for record in records {
        match record.tag {
            1 => {
                let object = *record.operands.first()?;
                if object >= 64 || stack.len() >= 64 {
                    return None;
                }
                lines.push(format!("v{temp} = inside_bits & {} != 0", 1_u64 << object));
                stack.push(format!("v{temp}"));
                temp += 1;
            }
            2 => {
                let value = stack.pop()?;
                lines.push(format!("v{temp} = not {value}"));
                stack.push(format!("v{temp}"));
                temp += 1;
            }
            3 | 4 => {
                let right = stack.pop()?;
                let left = stack.pop()?;
                let operator = if record.tag == 3 { "and" } else { "or" };
                lines.push(format!("v{temp} = {left} {operator} {right}"));
                stack.push(format!("v{temp}"));
                temp += 1;
            }
            5 | 6 => {
                if stack.len() >= 64 {
                    return None;
                }
                lines.push(format!(
                    "v{temp} = {}",
                    if record.tag == 5 { "true" } else { "false" }
                ));
                stack.push(format!("v{temp}"));
                temp += 1;
            }
            _ => return None,
        }
    }
    if stack.len() != 1 {
        return None;
    }
    let result = stack.pop()?;
    lines.push("occupied: i64 = 0".to_string());
    lines.push(format!("if {result}:"));
    lines.push("    occupied = 1".to_string());
    lines.push("return [1, occupied]".to_string());
    Some(lines)
}

fn write_program_accessors(
    output: &mut String,
    placements: &[crate::layout::RendererPlacement],
    compiled: &[super::CompiledRenderer],
) -> Result<(), String> {
    let mut verified_tables = Vec::with_capacity(compiled.len());
    for (placement, renderer) in placements.iter().zip(compiled) {
        let index = placement.index;
        let tables = super::binary_verify::verify_envelope(&renderer.encoded).map_err(|error| {
            format!("pixels::glue: accessor source envelope verification: {error}")
        })?;
        write!(output, "\nconst R{index}_EXPECTED_DIGEST: [u8; 32] = [")
            .expect("String writes cannot fail");
        for byte in &renderer.encoded[wrela_machine::pixels::FRAME_PROGRAM_DIGEST_OFFSET_V1
            ..wrela_machine::pixels::FRAME_PROGRAM_DIGEST_OFFSET_V1
                + wrela_machine::pixels::FRAME_PROGRAM_DIGEST_BYTES_V1]
        {
            write!(output, "{byte}, ").expect("String writes cannot fail");
        }
        output.push_str("]\n");
        writeln!(
            output,
            "const R{index}_EXPECTED_DIRECTORY: [[u64; 5]; {}] = [",
            tables.len()
        )
        .expect("String writes cannot fail");
        for table in &tables {
            writeln!(
                output,
                "    [{}, {}, {}, {}, {}],",
                table.kind.code(),
                table.record_bytes,
                table.count,
                table.offset,
                table.byte_len,
            )
            .expect("String writes cannot fail");
        }
        output.push_str("]\n");
        verified_tables.push(tables);
    }
    writeln!(
        output,
        "\npub fn __wrela_pixels_program_validate(renderer: usize) -> bool:"
    )
    .expect("String writes cannot fail");
    for ((placement, renderer), tables) in placements.iter().zip(compiled).zip(&verified_tables) {
        let index = placement.index;
        writeln!(output, "    if renderer == {index}:").expect("String writes cannot fail");
        write!(
            output,
            "        header_ok = R{index}_FRAME_PROGRAM_HEADER.magic[0] == {}",
            wrela_machine::pixels::FRAME_PROGRAM_MAGIC_V1[0]
        )
        .expect("String writes cannot fail");
        for (byte, expected) in wrela_machine::pixels::FRAME_PROGRAM_MAGIC_V1
            .iter()
            .copied()
            .enumerate()
            .skip(1)
        {
            write!(
                output,
                " and R{index}_FRAME_PROGRAM_HEADER.magic[{byte}] == {expected}"
            )
            .expect("String writes cannot fail");
        }
        for condition in [
            format!(
                "R{index}_FRAME_PROGRAM_HEADER.version == {}",
                wrela_machine::pixels::FRAME_PROGRAM_VERSION_V1
            ),
            format!(
                "R{index}_FRAME_PROGRAM_HEADER.header_bytes == {}",
                wrela_machine::pixels::FRAME_PROGRAM_HEADER_BYTES_V1
            ),
            format!(
                "R{index}_FRAME_PROGRAM_HEADER.flags == {}",
                renderer.program.program().flags
            ),
            format!(
                "R{index}_FRAME_PROGRAM_HEADER.total_bytes == {}",
                renderer.encoded.len()
            ),
            format!(
                "R{index}_FRAME_PROGRAM_HEADER.renderer_index == {}",
                renderer.program.program().renderer_index
            ),
            format!("R{index}_FRAME_PROGRAM_HEADER.reserved0 == 0"),
            format!(
                "R{index}_FRAME_PROGRAM_HEADER.numeric_revision == {}",
                renderer.program.program().numeric_revision
            ),
            format!(
                "R{index}_FRAME_PROGRAM_HEADER.formal_revision == {}",
                renderer.program.program().formal_revision
            ),
            format!(
                "R{index}_FRAME_PROGRAM_HEADER.table_count == {}",
                wrela_machine::pixels::FrameProgramTableKindV1::REQUIRED_COUNT
            ),
        ] {
            write!(output, " and {condition}").expect("String writes cannot fail");
        }
        writeln!(
            output,
            "\n\
             \x20       if not header_ok:\n\
             \x20           return false\n\
             \x20       reserved: usize = 0\n\
             \x20       @budget(bound=14)\n\
             \x20       while reserved < 14:\n\
             \x20           if R{index}_FRAME_PROGRAM_HEADER.reserved1[reserved] != 0:\n\
             \x20               return false\n\
             \x20           reserved = reserved + 1\n\
             \x20       byte: usize = 0\n\
             \x20       @budget(bound=32)\n\
             \x20       while byte < 32:\n\
             \x20           if R{index}_FRAME_PROGRAM_HEADER.digest[byte] != R{index}_EXPECTED_DIGEST[byte]:\n\
             \x20               return false\n\
             \x20           byte = byte + 1\n\
             \x20       directory: usize = 0\n\
             \x20       @budget(bound={})\n\
             \x20       while directory < {}:\n\
             \x20           expected = R{index}_EXPECTED_DIRECTORY[directory]\n\
             \x20           if R{index}_FRAME_PROGRAM_DIRECTORY.records[directory].kind.to[u64]() != expected[0] or R{index}_FRAME_PROGRAM_DIRECTORY.records[directory].record_bytes.to[u64]() != expected[1] or R{index}_FRAME_PROGRAM_DIRECTORY.records[directory].count.to[u64]() != expected[2] or R{index}_FRAME_PROGRAM_DIRECTORY.records[directory].offset.to[u64]() != expected[3] or R{index}_FRAME_PROGRAM_DIRECTORY.records[directory].byte_len.to[u64]() != expected[4]:\n\
             \x20               return false\n\
             \x20           directory = directory + 1\n\
             \x20       return true",
            tables.len().max(1),
            tables.len(),
        )
        .expect("String writes cannot fail");
    }
    output.push_str("    return false\n");

    writeln!(
        output,
        "\npub fn __wrela_pixels_program_header(renderer: usize) -> [u64; 6]:"
    )
    .expect("String writes cannot fail");
    for placement in placements {
        let index = placement.index;
        writeln!(
            output,
            "    if renderer == {index}:\n\
             \x20       return [\n\
             \x20           R{index}_FRAME_PROGRAM_HEADER.renderer_index.to[u64](),\n\
             \x20           R{index}_FRAME_PROGRAM_HEADER.total_bytes.to[u64](),\n\
             \x20           R{index}_FRAME_PROGRAM_HEADER.table_count.to[u64](),\n\
             \x20           R{index}_FRAME_PROGRAM_HEADER.numeric_revision.to[u64](),\n\
             \x20           R{index}_FRAME_PROGRAM_HEADER.formal_revision.to[u64](),\n\
             \x20           R{index}_FRAME_PROGRAM_HEADER.flags.to[u64](),\n\
             \x20       ]"
        )
        .expect("String writes cannot fail");
    }
    output.push_str("    return [0; 6]\n");

    writeln!(
        output,
        "\npub fn __wrela_pixels_program_digest_byte(renderer: usize, byte: usize) -> u8:"
    )
    .expect("String writes cannot fail");
    for (placement, renderer) in placements.iter().zip(compiled) {
        let index = placement.index;
        for (byte, expected) in renderer.encoded
            [wrela_machine::pixels::FRAME_PROGRAM_DIGEST_OFFSET_V1
                ..wrela_machine::pixels::FRAME_PROGRAM_DIGEST_OFFSET_V1
                    + wrela_machine::pixels::FRAME_PROGRAM_DIGEST_BYTES_V1]
            .iter()
            .copied()
            .enumerate()
        {
            writeln!(
                output,
                "    if renderer == {index} and byte == {byte}:\n\
                 \x20       return {expected}"
            )
            .expect("String writes cannot fail");
        }
    }
    output.push_str("    return 0\n");

    writeln!(
        output,
        "\npub fn __wrela_pixels_program_table_count(renderer: usize, table: u16) -> u32:"
    )
    .expect("String writes cannot fail");
    for (placement, renderer) in placements.iter().zip(compiled) {
        let index = placement.index;
        let tables = super::binary_verify::verify_envelope(&renderer.encoded).map_err(|error| {
            format!("pixels::glue: table-count accessor envelope verification: {error}")
        })?;
        for table in tables
            .iter()
            .filter(|table| table.kind != wrela_machine::pixels::FrameProgramTableKindV1::Immediate)
        {
            writeln!(
                output,
                "    if renderer == {index} and table == {}:\n\
                 \x20       return {}",
                table.kind.code(),
                table.count,
            )
            .expect("String writes cannot fail");
        }
    }
    output.push_str("    return 4294967295\n");

    writeln!(
        output,
        "\npub fn __wrela_pixels_program_record(renderer: usize, table: u16, id: u32) -> [u64; 5]:"
    )
    .expect("String writes cannot fail");
    for (placement, renderer) in placements.iter().zip(compiled) {
        let index = placement.index;
        let tables = super::binary_verify::verify_envelope(&renderer.encoded).map_err(|error| {
            format!("pixels::glue: record accessor envelope verification: {error}")
        })?;
        for table in tables.iter().filter(|table| {
            table.count != 0
                && table.kind != wrela_machine::pixels::FrameProgramTableKindV1::Immediate
        }) {
            let upper = table
                .kind
                .stable_name()
                .replace('-', "_")
                .to_ascii_uppercase();
            writeln!(
                output,
                "    if renderer == {index} and table == {} and id < R{index}_{upper}_COUNT.to[u32]():\n\
                 \x20       return [1, R{index}_{upper}_TABLE.records[id.to[usize]()].stable_id.to[u64](), R{index}_{upper}_TABLE.records[id.to[usize]()].tag.to[u64](), R{index}_{upper}_TABLE.records[id.to[usize]()].flags.to[u64](), R{index}_{upper}_TABLE.records[id.to[usize]()].operand_count.to[u64]()]",
                table.kind.code(),
            )
            .expect("String writes cannot fail");
        }
    }
    output.push_str("    return [0; 5]\n");

    writeln!(
        output,
        "\npub fn __wrela_pixels_program_operand(renderer: usize, table: u16, id: u32, ordinal: u16) -> [u64; 2]:"
    )
    .expect("String writes cannot fail");
    for (placement, renderer) in placements.iter().zip(compiled) {
        let index = placement.index;
        let tables = super::binary_verify::verify_envelope(&renderer.encoded).map_err(|error| {
            format!("pixels::glue: operand accessor envelope verification: {error}")
        })?;
        let immediate_count = tables
            .iter()
            .find(|table| table.kind == wrela_machine::pixels::FrameProgramTableKindV1::Immediate)
            .map_or(0, |table| table.count);
        if immediate_count == 0 {
            continue;
        }
        for table in tables.iter().filter(|table| {
            table.count != 0
                && table.kind != wrela_machine::pixels::FrameProgramTableKindV1::Immediate
        }) {
            let upper = table
                .kind
                .stable_name()
                .replace('-', "_")
                .to_ascii_uppercase();
            writeln!(
                output,
                "    if renderer == {index} and table == {} and id < R{index}_{upper}_COUNT.to[u32]():\n\
                 \x20       if ordinal >= R{index}_{upper}_TABLE.records[id.to[usize]()].operand_count:\n\
                 \x20           return [0; 2]\n\
                 \x20       immediate_index = R{index}_{upper}_TABLE.records[id.to[usize]()].operand_offset.to[usize]() + ordinal.to[usize]()\n\
                 \x20       if immediate_index >= R{index}_IMMEDIATE_COUNT:\n\
                 \x20           return [0; 2]\n\
                 \x20       if R{index}_IMMEDIATE_TABLE.records[immediate_index].owner_kind != table or R{index}_IMMEDIATE_TABLE.records[immediate_index].owner_id != id or R{index}_IMMEDIATE_TABLE.records[immediate_index].ordinal != ordinal.to[u32]() or R{index}_IMMEDIATE_TABLE.records[immediate_index].reserved0 != 0 or R{index}_IMMEDIATE_TABLE.records[immediate_index].reserved1 != 0:\n\
                 \x20           return [0; 2]\n\
                 \x20       return [1, R{index}_IMMEDIATE_TABLE.records[immediate_index].value]",
                table.kind.code(),
            )
            .expect("String writes cannot fail");
        }
    }
    output.push_str("    return [0; 2]\n");
    output.push_str(
        "\n\
         pub fn __wrela_pixels_local_index_header(renderer: usize, index_kind: u64) -> [u64; 4]:\n\
         \x20   count = __wrela_pixels_program_table_count(renderer, 9)\n\
         \x20   if count == 4294967295 or count > 65536:\n\
         \x20       return [0; 4]\n\
         \x20   id: u32 = 0\n\
         \x20   @budget(bound=65536)\n\
         \x20   while id < count:\n\
         \x20       record = __wrela_pixels_program_record(renderer, 9, id)\n\
         \x20       if record[0] != 1:\n\
         \x20           return [0; 4]\n\
         \x20       if record[2] == 30:\n\
         \x20           marker = __wrela_pixels_program_operand(renderer, 9, id, 0)\n\
         \x20           kind = __wrela_pixels_program_operand(renderer, 9, id, 1)\n\
         \x20           if marker[0] != 1 or kind[0] != 1:\n\
         \x20               return [0; 4]\n\
         \x20           if marker[1] == 0 and kind[1] == index_kind:\n\
         \x20               cells = __wrela_pixels_program_operand(renderer, 9, id, 2)\n\
         \x20               ids = __wrela_pixels_program_operand(renderer, 9, id, 3)\n\
         \x20               chunks = __wrela_pixels_program_operand(renderer, 9, id, 4)\n\
         \x20               if cells[0] != 1 or ids[0] != 1 or chunks[0] != 1:\n\
         \x20                   return [0; 4]\n\
         \x20               if cells[1] > 4294967295 or ids[1] > 4294967295 or chunks[1] > 4294967295:\n\
         \x20                   return [0; 4]\n\
         \x20               return [1, cells[1], ids[1], chunks[1]]\n\
         \x20       id = id + 1\n\
         \x20   return [0; 4]\n\
         \n\
         pub fn __wrela_pixels_local_index_word(renderer: usize, index_kind: u64, word: u64) -> [u64; 2]:\n\
         \x20   count = __wrela_pixels_program_table_count(renderer, 9)\n\
         \x20   if count == 4294967295 or count > 65536:\n\
         \x20       return [0; 2]\n\
         \x20   id: u32 = 0\n\
         \x20   @budget(bound=65536)\n\
         \x20   while id < count:\n\
         \x20       record = __wrela_pixels_program_record(renderer, 9, id)\n\
         \x20       if record[0] != 1:\n\
         \x20           return [0; 2]\n\
         \x20       if record[2] == 30:\n\
         \x20           marker = __wrela_pixels_program_operand(renderer, 9, id, 0)\n\
         \x20           kind = __wrela_pixels_program_operand(renderer, 9, id, 1)\n\
         \x20           if marker[0] != 1 or kind[0] != 1:\n\
         \x20               return [0; 2]\n\
         \x20           if marker[1] == 1 and kind[1] == index_kind:\n\
         \x20               offset = __wrela_pixels_program_operand(renderer, 9, id, 3)\n\
         \x20               length = __wrela_pixels_program_operand(renderer, 9, id, 4)\n\
         \x20               if offset[0] != 1 or length[0] != 1:\n\
         \x20                   return [0; 2]\n\
         \x20               end = offset[1] +% length[1]\n\
         \x20               if end < offset[1]:\n\
         \x20                   return [0; 2]\n\
         \x20               if word >= offset[1] and word < end:\n\
         \x20                   ordinal = word - offset[1] + 5\n\
         \x20                   if ordinal > 65535:\n\
         \x20                       return [0; 2]\n\
         \x20                   return __wrela_pixels_program_operand(renderer, 9, id, ordinal.to[u16]())\n\
         \x20       id = id + 1\n\
         \x20   return [0; 2]\n\
         \n\
         pub fn __wrela_pixels_tile_feature_count(renderer: usize, tile: u32) -> u32:\n\
         \x20   header = __wrela_pixels_local_index_header(renderer, 0)\n\
         \x20   if header[0] != 1 or tile.to[u64]() >= header[1]:\n\
         \x20       return 4294967295\n\
         \x20   count = __wrela_pixels_local_index_word(renderer, 0, tile.to[u64]() * 2 + 1)\n\
         \x20   if count[0] != 1 or count[1] > 4294967295:\n\
         \x20       return 4294967295\n\
         \x20   return count[1].to[u32]()\n\
         \n\
         pub fn __wrela_pixels_tile_feature(renderer: usize, tile: u32, ordinal: u32) -> [u64; 2]:\n\
         \x20   header = __wrela_pixels_local_index_header(renderer, 0)\n\
         \x20   if header[0] != 1 or tile.to[u64]() >= header[1]:\n\
         \x20       return [0; 2]\n\
         \x20   offset = __wrela_pixels_local_index_word(renderer, 0, tile.to[u64]() * 2)\n\
         \x20   count = __wrela_pixels_local_index_word(renderer, 0, tile.to[u64]() * 2 + 1)\n\
         \x20   if offset[0] != 1 or count[0] != 1 or ordinal.to[u64]() >= count[1]:\n\
         \x20       return [0; 2]\n\
         \x20   end = offset[1] +% count[1]\n\
         \x20   if end < offset[1] or end > header[2]:\n\
         \x20       return [0; 2]\n\
         \x20   cell_words = header[1] *% 2\n\
         \x20   if header[1] != 0 and cell_words / 2 != header[1]:\n\
         \x20       return [0; 2]\n\
         \x20   id_word = cell_words +% offset[1] +% ordinal.to[u64]()\n\
         \x20   if id_word < cell_words or id_word < offset[1]:\n\
         \x20       return [0; 2]\n\
         \x20   return __wrela_pixels_local_index_word(renderer, 0, id_word)\n",
    );
    output.push_str(
        "\n\
         pub fn __wrela_pixels_tile_event_count(renderer: usize, tile: u32) -> u32:\n\
         \x20   header = __wrela_pixels_local_index_header(renderer, 1)\n\
         \x20   if header[0] != 1 or tile.to[u64]() >= header[1]:\n\
         \x20       return 4294967295\n\
         \x20   count = __wrela_pixels_local_index_word(renderer, 1, tile.to[u64]() * 2 + 1)\n\
         \x20   if count[0] != 1 or count[1] > 4294967295:\n\
         \x20       return 4294967295\n\
         \x20   return count[1].to[u32]()\n\
         \n\
         pub fn __wrela_pixels_tile_event(renderer: usize, tile: u32, ordinal: u32) -> [u64; 2]:\n\
         \x20   header = __wrela_pixels_local_index_header(renderer, 1)\n\
         \x20   if header[0] != 1 or tile.to[u64]() >= header[1]:\n\
         \x20       return [0; 2]\n\
         \x20   offset = __wrela_pixels_local_index_word(renderer, 1, tile.to[u64]() * 2)\n\
         \x20   count = __wrela_pixels_local_index_word(renderer, 1, tile.to[u64]() * 2 + 1)\n\
         \x20   if offset[0] != 1 or count[0] != 1 or ordinal.to[u64]() >= count[1]:\n\
         \x20       return [0; 2]\n\
         \x20   end = offset[1] +% count[1]\n\
         \x20   if end < offset[1] or end > header[2]:\n\
         \x20       return [0; 2]\n\
         \x20   cell_words = header[1] *% 2\n\
         \x20   if header[1] != 0 and cell_words / 2 != header[1]:\n\
         \x20       return [0; 2]\n\
         \x20   id_word = cell_words +% offset[1] +% ordinal.to[u64]()\n\
         \x20   if id_word < cell_words or id_word < offset[1]:\n\
         \x20       return [0; 2]\n\
         \x20   return __wrela_pixels_local_index_word(renderer, 1, id_word)\n",
    );
    Ok(())
}

pub fn generate(
    renderer_index: usize,
    config: &RendererConfig,
    program: &VerifiedFrameProgram,
) -> Result<GeneratedRenderer, String> {
    let workers = usize::try_from(config.worker_count)
        .map_err(|_| "pixels::glue: worker count exceeds usize".to_string())?;
    if workers == 0 {
        return Err("pixels::glue: renderer has zero workers".to_string());
    }
    let tiles_x = config
        .width
        .div_ceil(u32::from(wrela_machine::pixels::TILE_WIDTH));
    let tiles_y = config
        .height
        .div_ceil(u32::from(wrela_machine::pixels::TILE_HEIGHT));
    let tile_count = tiles_x
        .checked_mul(tiles_y)
        .ok_or_else(|| "P015: renderer tile count overflow".to_string())?;
    let workers_u32 =
        u32::try_from(workers).map_err(|_| "pixels::glue: worker count exceeds u32".to_string())?;
    let generated_workers = (0..workers)
        .map(|worker| {
            let worker_u32 =
                u32::try_from(worker).map_err(|_| "pixels::glue: worker index overflow")?;
            // Ceil-partition complete 64x32 scanout tiles. With fewer tiles
            // than workers this assigns the first tile to worker zero and
            // leaves later workers empty; no scanout allocation is shared.
            let start = (u64::from(tile_count) * u64::from(worker_u32)
                + u64::from(workers_u32 - 1))
                / u64::from(workers_u32);
            let end = (u64::from(tile_count) * u64::from(worker_u32 + 1)
                + u64::from(workers_u32 - 1))
                / u64::from(workers_u32);
            Ok(GeneratedWorker {
                actor: format!("__wrela_renderer_{renderer_index}_worker_{worker}"),
                core: worker,
                tiles_start: u32::try_from(start)
                    .map_err(|_| "pixels::glue: tile start overflow")?,
                tiles_end: u32::try_from(end).map_err(|_| "pixels::glue: tile end overflow")?,
            })
        })
        .collect::<Result<Vec<_>, &str>>()
        .map_err(str::to_string)?;

    let families = bootstrap_families(program);
    let coordinator = format!("__wrela_renderer_{renderer_index}_coordinator");
    let renderer_key = format!(
        "struct:Renderer[{}]",
        crate::sema::types::render_type(&config.params_type)
    );
    let mut rooted_functions = vec![
        format!("{renderer_key}.init"),
        "__wrela_pixels_bootstrap_dispatch".to_string(),
        "__wrela_pixels_program_validate".to_string(),
        "__wrela_abort_val".to_string(),
    ];
    if workers != 0 {
        for worker in 0..super::config::P7_MAX_RENDER_WORKERS {
            rooted_functions.push(format!("RendererWorker{worker}.init"));
        }
    }
    for family in &families {
        rooted_functions.push(format!(
            "__wrela_pixels_bootstrap_{}",
            family.replace('-', "_")
        ));
    }
    rooted_functions.sort();
    rooted_functions.dedup();
    let camera = super::camera::CameraContract::derive(config)?;
    let camera_bounds = [
        camera.eye_component[0],
        camera.eye_component[1],
        camera.eye_component[2],
        camera.forward_component[0],
        camera.forward_component[1],
        camera.forward_component[2],
        camera.right_component[0],
        camera.right_component[1],
        camera.right_component[2],
        camera.up_component[0],
        camera.up_component[1],
        camera.up_component[2],
    ]
    .map(outward_f32_interval);
    let mut light_kinds = [0usize; 8];
    for (slot, kind) in config.light_kinds.iter().enumerate() {
        let Some(target) = light_kinds.get_mut(slot) else {
            return Err("pixels::glue: sealed light topology exceeds eight slots".to_string());
        };
        *target = light_kind_tag(kind)?;
    }
    Ok(GeneratedRenderer {
        renderer_index,
        coordinator,
        display_index: config.display_index,
        workers: generated_workers,
        exposure_range: [config.exposure.min, config.exposure.max],
        environment_min: config.environment.min,
        environment_max: config.environment.max,
        camera_bounds,
        world_min: [config.world_min.x, config.world_min.y, config.world_min.z],
        world_max: [config.world_max.x, config.world_max.y, config.world_max.z],
        light_capacity: usize::try_from(config.light_capacity)
            .map_err(|_| "pixels::glue: light capacity exceeds usize".to_string())?,
        light_kinds,
        rooted_functions,
        bootstrap_families: families.into_iter().map(str::to_string).collect(),
    })
}

fn write_p7_runtime_storage_accessors(
    output: &mut String,
    placements: &[crate::layout::RendererPlacement],
    compiled: &[super::CompiledRenderer],
    instrumented: bool,
) -> Result<(), String> {
    output.push_str(
        "\npub fn __wrela_pixels_p7_frame_snapshot_store(renderer: usize, read params: [f32; 16], param_count: u16, read camera: [f32; 12], read light_kinds: [u64; 8], read light_scalars: [f32; 120], read post: [f32; 4], frame_index: u64) -> bool:\n\
         \x20   if param_count > 16:\n\
         \x20       return false\n",
    );
    for placement in placements {
        let index = placement.index;
        writeln!(
            output,
            "    if renderer == {index}:\n\
             \x20       param: usize = 0\n\
             \x20       @budget(bound=16)\n\
             \x20       while param < 16:\n\
             \x20           R{index}_FRAME_SNAPSHOT.bits[param] = __wrela_pixels_f32_to_bits(params[param])\n\
             \x20           param = param + 1\n\
             \x20       component: usize = 0\n\
             \x20       @budget(bound=12)\n\
             \x20       while component < 12:\n\
             \x20           R{index}_FRAME_SNAPSHOT.bits[16 + component] = __wrela_pixels_f32_to_bits(camera[component])\n\
             \x20           component = component + 1\n\
             \x20       light_component: usize = 0\n\
             \x20       @budget(bound=120)\n\
             \x20       while light_component < 120:\n\
             \x20           R{index}_FRAME_SNAPSHOT.bits[28 + light_component] = __wrela_pixels_f32_to_bits(light_scalars[light_component])\n\
             \x20           light_component = light_component + 1\n\
             \x20       R{index}_FRAME_SNAPSHOT.bits[148] = __wrela_pixels_f32_to_bits(post[0])\n\
             \x20       R{index}_FRAME_SNAPSHOT.bits[149] = __wrela_pixels_f32_to_bits(post[1])\n\
             \x20       R{index}_FRAME_SNAPSHOT.bits[150] = __wrela_pixels_f32_to_bits(post[2])\n\
             \x20       R{index}_FRAME_SNAPSHOT.bits[151] = __wrela_pixels_f32_to_bits(post[3])\n\
             \x20       light: usize = 0\n\
             \x20       @budget(bound=8)\n\
             \x20       while light < 8:\n\
             \x20           R{index}_FRAME_SNAPSHOT.meta[2 + light] = light_kinds[light]\n\
             \x20           light = light + 1\n\
             \x20       R{index}_FRAME_SNAPSHOT.meta[0] = frame_index\n\
             \x20       R{index}_FRAME_SNAPSHOT.meta[1] = 1 | (param_count.to[u64]() << 16)\n\
             \x20       return true"
        )
        .expect("String writes cannot fail");
    }
    output.push_str("    return false\n");

    output.push_str(
        "\npub fn __wrela_pixels_p7_frame_snapshot_params(renderer: usize) -> [f32; 16]:\n",
    );
    for placement in placements {
        let index = placement.index;
        writeln!(
            output,
            "    if renderer == {index}:\n\
             \x20       values: [f32; 16] = [0.0; 16]\n\
             \x20       param: usize = 0\n\
             \x20       @budget(bound=16)\n\
             \x20       while param < 16:\n\
             \x20           values[param] = __wrela_pixels_f32_from_bits(R{index}_FRAME_SNAPSHOT.bits[param])\n\
             \x20           param = param + 1\n\
             \x20       return values"
        )
        .expect("String writes cannot fail");
    }
    output.push_str("    return [0.0; 16]\n");

    output.push_str(
        "\npub fn __wrela_pixels_p7_frame_snapshot_camera(renderer: usize) -> [f32; 12]:\n",
    );
    for placement in placements {
        let index = placement.index;
        writeln!(
            output,
            "    if renderer == {index}:\n\
             \x20       values: [f32; 12] = [0.0; 12]\n\
             \x20       component: usize = 0\n\
             \x20       @budget(bound=12)\n\
             \x20       while component < 12:\n\
             \x20           values[component] = __wrela_pixels_f32_from_bits(R{index}_FRAME_SNAPSHOT.bits[16 + component])\n\
             \x20           component = component + 1\n\
             \x20       return values"
        )
        .expect("String writes cannot fail");
    }
    output.push_str("    return [0.0; 12]\n");

    output.push_str(
        "\npub fn __wrela_pixels_p7_frame_snapshot_light_kinds(renderer: usize) -> [u64; 8]:\n",
    );
    for placement in placements {
        let index = placement.index;
        writeln!(
            output,
            "    if renderer == {index}:\n\
             \x20       values: [u64; 8] = [0; 8]\n\
             \x20       index: usize = 0\n\
             \x20       @budget(bound=8)\n\
             \x20       while index < 8:\n\
             \x20           values[index] = R{index}_FRAME_SNAPSHOT.meta[2 + index]\n\
             \x20           index = index + 1\n\
             \x20       return values"
        )
        .expect("String writes cannot fail");
    }
    output.push_str("    return [0; 8]\n");

    output.push_str(
        "\npub fn __wrela_pixels_p7_frame_snapshot_light_scalars(renderer: usize) -> [f32; 120]:\n",
    );
    for placement in placements {
        let index = placement.index;
        writeln!(
            output,
            "    if renderer == {index}:\n\
             \x20       values: [f32; 120] = [0.0; 120]\n\
             \x20       index: usize = 0\n\
             \x20       @budget(bound=120)\n\
             \x20       while index < 120:\n\
             \x20           values[index] = __wrela_pixels_f32_from_bits(R{index}_FRAME_SNAPSHOT.bits[28 + index])\n\
             \x20           index = index + 1\n\
             \x20       return values"
        )
        .expect("String writes cannot fail");
    }
    output.push_str("    return [0.0; 120]\n");

    output
        .push_str("\npub fn __wrela_pixels_p7_frame_snapshot_post(renderer: usize) -> [f32; 4]:\n");
    for placement in placements {
        let index = placement.index;
        writeln!(
            output,
            "    if renderer == {index}:\n\
             \x20       return [\n\
             \x20           __wrela_pixels_f32_from_bits(R{index}_FRAME_SNAPSHOT.bits[148]),\n\
             \x20           __wrela_pixels_f32_from_bits(R{index}_FRAME_SNAPSHOT.bits[149]),\n\
             \x20           __wrela_pixels_f32_from_bits(R{index}_FRAME_SNAPSHOT.bits[150]),\n\
             \x20           __wrela_pixels_f32_from_bits(R{index}_FRAME_SNAPSHOT.bits[151]),\n\
             \x20       ]"
        )
        .expect("String writes cannot fail");
    }
    output.push_str("    return [0.0; 4]\n");

    output
        .push_str("\npub fn __wrela_pixels_p7_frame_snapshot_meta(renderer: usize) -> [u64; 3]:\n");
    for placement in placements {
        let index = placement.index;
        writeln!(
            output,
            "    if renderer == {index} and (R{index}_FRAME_SNAPSHOT.meta[1] & 65535) == 1:\n\
             \x20       return [1, R{index}_FRAME_SNAPSHOT.meta[0], R{index}_FRAME_SNAPSHOT.meta[1] >> 16]"
        )
        .expect("String writes cannot fail");
    }
    output.push_str("    return [0; 3]\n");

    output.push_str(
        "\npub fn __wrela_pixels_p7_worker_assignment(renderer: usize, worker: u32) -> [u64; 7]:\n",
    );
    for placement in placements {
        let index = placement.index;
        for worker in &placement.per_core {
            let worker_index = worker.worker_index;
            writeln!(
                output,
                "    if renderer == {index} and worker == {worker_index}:\n\
                 \x20       return [1, {}, {}, {}, {}, {}, {worker_index}]",
                placement.frameprog_base,
                worker.workspace_base,
                worker.workspace_bytes,
                worker.tiles_start,
                worker.tiles_end,
            )
            .expect("String writes cannot fail");
        }
    }
    output.push_str("    return [0; 7]\n");

    output.push_str(
        "\npub fn __wrela_pixels_p7_framebuffer_store_byte(renderer: usize, byte: usize, value: u8) -> bool:\n\
         \x20   shift = (byte % 8).to[u64]() * 8\n\
         \x20   mask = 255.to[u64]() << shift\n",
    );
    for placement in placements {
        let index = placement.index;
        let mut offset = 0_u64;
        let mut chunk = 0_usize;
        while offset < placement.framebuffer_bytes {
            let chunk_bytes =
                (placement.framebuffer_bytes - offset).min(WORKSPACE_VIEW_CHUNK_BYTES);
            writeln!(
                output,
                "    if renderer == {index} and byte.to[u64]() >= {offset} and byte.to[u64]() < {}:\n\
                 \x20       word = (byte.to[u64]() - {offset}).to[usize]() / 8\n\
                 \x20       old = R{index}_DEBUG_FRAMEBUFFER_CHUNK_{chunk}.words[word]\n\
                 \x20       R{index}_DEBUG_FRAMEBUFFER_CHUNK_{chunk}.words[word] = (old & (mask ^ 18446744073709551615)) | (value.to[u64]() << shift)\n\
                 \x20       return true",
                offset + chunk_bytes,
            )
            .expect("String writes cannot fail");
            offset += chunk_bytes;
            chunk += 1;
        }
    }
    output.push_str("    return false\n");

    output.push_str(
        "\npub fn __wrela_pixels_p7_framebuffer_load_byte(renderer: usize, byte: usize) -> [u64; 2]:\n\
         \x20   shift = (byte % 8).to[u64]() * 8\n",
    );
    for placement in placements {
        let index = placement.index;
        let mut offset = 0_u64;
        let mut chunk = 0_usize;
        while offset < placement.framebuffer_bytes {
            let chunk_bytes =
                (placement.framebuffer_bytes - offset).min(WORKSPACE_VIEW_CHUNK_BYTES);
            writeln!(
                output,
                "    if renderer == {index} and byte.to[u64]() >= {offset} and byte.to[u64]() < {}:\n\
                 \x20       word = (byte.to[u64]() - {offset}).to[usize]() / 8\n\
                 \x20       return [1, (R{index}_DEBUG_FRAMEBUFFER_CHUNK_{chunk}.words[word] >> shift) & 255]",
                offset + chunk_bytes,
            )
            .expect("String writes cannot fail");
            offset += chunk_bytes;
            chunk += 1;
        }
    }
    output.push_str("    return [0; 2]\n");

    output.push_str("\npub fn __wrela_pixels_p7_framebuffer_reset(renderer: usize) -> bool:\n");
    for (placement, renderer) in placements.iter().zip(compiled) {
        let index = placement.index;
        let pixel_count = u64::from(renderer.config.width)
            .checked_mul(u64::from(renderer.config.height))
            .ok_or_else(|| "P015: P7 framebuffer pixel count overflow".to_string())?;
        let front_bytes = pixel_count
            .checked_mul(4)
            .ok_or_else(|| "P015: P7 framebuffer front byte count overflow".to_string())?;
        let half = placement.framebuffer_bytes / 2;
        if placement.framebuffer_bytes % 2 != 0
            || front_bytes > half
            || pixel_count > placement.framebuffer_bytes - half
        {
            return Err(format!(
                "pixels::glue: renderer {index} debug framebuffer cannot hold pixels and write markers"
            ));
        }
        writeln!(
            output,
            "    if renderer == {index}:\n\
             \x20       pass"
        )
        .expect("String writes cannot fail");
        let mut offset = 0_u64;
        let mut chunk = 0_usize;
        while offset < placement.framebuffer_bytes {
            let chunk_bytes =
                (placement.framebuffer_bytes - offset).min(WORKSPACE_VIEW_CHUNK_BYTES);
            let chunk_words = chunk_bytes / 8;
            writeln!(
                output,
                "        word_{chunk}: usize = 0\n\
                 \x20       @budget(bound={chunk_words})\n\
                 \x20       while word_{chunk} < {chunk_words}:\n\
                 \x20           R{index}_DEBUG_FRAMEBUFFER_CHUNK_{chunk}.words[word_{chunk}] = 0\n\
                 \x20           word_{chunk} = word_{chunk} + 1"
            )
            .expect("String writes cannot fail");
            offset += chunk_bytes;
            chunk += 1;
        }
        output.push_str("        return true\n");
    }
    output.push_str("    return false\n");

    output.push_str(
        "\npub fn __wrela_pixels_p7_framebuffer_write(renderer: usize, pixel: u32, r: u8, g: u8, b: u8, a: u8) -> bool:\n",
    );
    for (placement, renderer) in placements.iter().zip(compiled) {
        let index = placement.index;
        let pixel_count = u64::from(renderer.config.width) * u64::from(renderer.config.height);
        let half = placement.framebuffer_bytes / 2;
        writeln!(
            output,
            "    if renderer == {index}:\n\
             \x20       if pixel.to[u64]() >= {pixel_count}:\n\
             \x20           return false\n\
             \x20       marker = {half} + pixel.to[usize]()\n\
             \x20       marker_value = __wrela_pixels_p7_framebuffer_load_byte({index}, marker)\n\
             \x20       if marker_value[0] != 1 or marker_value[1] != 0:\n\
             \x20           return false\n\
             \x20       byte = pixel.to[usize]() * 4\n\
             \x20       if not __wrela_pixels_p7_framebuffer_store_byte({index}, byte, r) or not __wrela_pixels_p7_framebuffer_store_byte({index}, byte + 1, g) or not __wrela_pixels_p7_framebuffer_store_byte({index}, byte + 2, b) or not __wrela_pixels_p7_framebuffer_store_byte({index}, byte + 3, a) or not __wrela_pixels_p7_framebuffer_store_byte({index}, marker, 1):\n\
             \x20           return false\n\
             \x20       return true"
        )
        .expect("String writes cannot fail");
    }
    output.push_str("    return false\n");

    output.push_str(
        "\npub fn __wrela_pixels_p7_framebuffer_byte(renderer: usize, byte: usize) -> [u64; 2]:\n",
    );
    for (placement, renderer) in placements.iter().zip(compiled) {
        let index = placement.index;
        let front_bytes = u64::from(renderer.config.width) * u64::from(renderer.config.height) * 4;
        writeln!(
            output,
            "    if renderer == {index} and byte < {front_bytes}:\n\
             \x20       return __wrela_pixels_p7_framebuffer_load_byte({index}, byte)"
        )
        .expect("String writes cannot fail");
    }
    output.push_str("    return [0; 2]\n");

    output.push_str(
        "\npub fn __wrela_pixels_p7_framebuffer_pixel_written(renderer: usize, pixel: u32) -> bool:\n",
    );
    for (placement, renderer) in placements.iter().zip(compiled) {
        let index = placement.index;
        let pixel_count = u64::from(renderer.config.width) * u64::from(renderer.config.height);
        let half = placement.framebuffer_bytes / 2;
        writeln!(
            output,
            "    if renderer == {index} and pixel.to[u64]() < {pixel_count}:\n\
             \x20       value = __wrela_pixels_p7_framebuffer_load_byte({index}, {half} + pixel.to[usize]())\n\
             \x20       return value[0] == 1 and value[1] == 1"
        )
        .expect("String writes cannot fail");
    }
    output.push_str("    return false\n");

    for (placement, renderer) in placements.iter().zip(compiled) {
        let index = placement.index;
        let pixel_count = u64::from(renderer.config.width) * u64::from(renderer.config.height);
        let front_bytes = pixel_count * 4;
        let half = placement.framebuffer_bytes / 2;
        if half % 8 != 0 {
            return Err(format!(
                "pixels::glue: renderer {index} debug framebuffer marker half is not word aligned"
            ));
        }
        writeln!(
            output,
            "\npub fn __wrela_pixels_p7_framebuffer_word_r{index}(word: usize) -> [u64; 2]:"
        )
        .expect("String writes cannot fail");
        let mut offset = 0_u64;
        let mut chunk = 0_usize;
        while offset < placement.framebuffer_bytes {
            let chunk_bytes =
                (placement.framebuffer_bytes - offset).min(WORKSPACE_VIEW_CHUNK_BYTES);
            let first_word = offset / 8;
            let end_word = (offset + chunk_bytes) / 8;
            writeln!(
                output,
                "    if word.to[u64]() >= {first_word} and word.to[u64]() < {end_word}:\n\
                 \x20       return [1, R{index}_DEBUG_FRAMEBUFFER_CHUNK_{chunk}.words[word - {first_word}] ]"
            )
            .expect("String writes cannot fail");
            offset += chunk_bytes;
            chunk += 1;
        }
        output.push_str("    return [0; 2]\n");
        writeln!(
            output,
            "\npub fn __wrela_pixels_p7_framebuffer_digest_r{index}() -> [u64; 5]:\n\
             \x20   marker_word: usize = 0\n\
             \x20   @budget(bound={})\n\
             \x20   while marker_word < {}:\n\
             \x20       marker = __wrela_pixels_p7_framebuffer_word_r{index}({} + marker_word)\n\
             \x20       if marker[0] != 1 or marker[1] != 72340172838076673:\n\
             \x20           return [0; 5]\n\
             \x20       marker_word = marker_word + 1",
            pixel_count / 8,
            pixel_count / 8,
            half / 8,
        )
        .expect("String writes cannot fail");
        let marker_tail = pixel_count % 8;
        if marker_tail != 0 {
            let expected_tail =
                (0..marker_tail).fold(0_u64, |value, byte| value | (1_u64 << (byte * 8)));
            writeln!(
                output,
                "    marker_tail = __wrela_pixels_p7_framebuffer_word_r{index}({} + marker_word)\n\
                 \x20   if marker_tail[0] != 1 or marker_tail[1] != {expected_tail}:\n\
                 \x20       return [0; 5]",
                half / 8,
            )
            .expect("String writes cannot fail");
        }
        writeln!(
            output,
            "    h0: u64 = 1469598103934665603\n\
             \x20   h1: u64 = 1099511628211\n\
             \x20   h2: u64 = 7809847782465536322\n\
             \x20   h3: u64 = 1609587929392839161\n\
             \x20   byte: usize = 0\n\
             \x20   @budget(bound={front_bytes})\n\
             \x20   while byte < {front_bytes}:\n\
             \x20       packed = __wrela_pixels_p7_framebuffer_word_r{index}(byte / 8)\n\
             \x20       if packed[0] != 1:\n\
             \x20           return [0; 5]\n\
             \x20       octet: usize = 0\n\
             \x20       @budget(bound=8)\n\
             \x20       while octet < 8 and byte < {front_bytes}:\n\
             \x20           value = (packed[1] >> (octet * 8).to[u64]()) & 255\n\
             \x20           h0 = (h0 ^ value) *% 1099511628211\n\
             \x20           h1 = (h1 ^ (value +% byte.to[u64]())) *% 14029467366897019727\n\
             \x20           h2 = (h2 +% value) *% 11400714785074694791\n\
             \x20           h3 = (h3 ^ (value << (byte % 8).to[u64]())) *% 9650029242287828579\n\
             \x20           byte = byte + 1\n\
             \x20           octet = octet + 1\n\
             \x20   return [1, h0, h1, h2, h3]"
        )
        .expect("String writes cannot fail");
    }
    output
        .push_str("\npub fn __wrela_pixels_p7_framebuffer_digest(renderer: usize) -> [u64; 5]:\n");
    for placement in placements {
        writeln!(
            output,
            "    if renderer == {}:\n\
             \x20       return __wrela_pixels_p7_framebuffer_digest_r{}()",
            placement.index, placement.index,
        )
        .expect("String writes cannot fail");
    }
    output.push_str("    return [0; 5]\n");

    for (placement, renderer) in placements.iter().zip(compiled) {
        let index = placement.index;
        let sample_alpha_0 = (16_u64 * u64::from(renderer.config.width) + 31) * 4 + 3;
        let sample_alpha_1 = (16_u64 * u64::from(renderer.config.width) + 32) * 4 + 3;
        let sample_alpha_2 = (16_u64 * u64::from(renderer.config.width) + 40) * 4 + 3;
        writeln!(
            output,
            "\npub fn __wrela_pixels_p7_framebuffer_alpha_samples_r{index}() -> u64:\n\
             \x20   word_0 = __wrela_pixels_p7_framebuffer_word_r{index}({})\n\
             \x20   word_1 = __wrela_pixels_p7_framebuffer_word_r{index}({})\n\
             \x20   word_2 = __wrela_pixels_p7_framebuffer_word_r{index}({})\n\
             \x20   if word_0[0] != 1 or word_1[0] != 1 or word_2[0] != 1:\n\
             \x20       return 18446744073709551615\n\
             \x20   alpha_0 = (word_0[1] >> {}) & 255\n\
             \x20   alpha_1 = (word_1[1] >> {}) & 255\n\
             \x20   alpha_2 = (word_2[1] >> {}) & 255\n\
             \x20   return alpha_0 | (alpha_1 << 8) | (alpha_2 << 16)",
            sample_alpha_0 / 8,
            sample_alpha_1 / 8,
            sample_alpha_2 / 8,
            (sample_alpha_0 % 8) * 8,
            (sample_alpha_1 % 8) * 8,
            (sample_alpha_2 % 8) * 8,
        )
        .expect("String writes cannot fail");
    }
    output.push_str(
        "\npub fn __wrela_pixels_p7_framebuffer_alpha_samples(renderer: usize) -> u64:\n",
    );
    for placement in placements {
        writeln!(
            output,
            "    if renderer == {}:\n\
             \x20       return __wrela_pixels_p7_framebuffer_alpha_samples_r{}()",
            placement.index, placement.index,
        )
        .expect("String writes cannot fail");
    }
    output.push_str("    return 18446744073709551615\n");

    output.push_str(
        "\npub fn __wrela_pixels_p7_workspace_reset(renderer: usize, worker: u32, generation: u64) -> bool:\n",
    );
    for placement in placements {
        let index = placement.index;
        for worker in &placement.per_core {
            let worker_index = worker.worker_index;
            writeln!(
                output,
                "    if renderer == {index} and worker == {worker_index}:\n\
                 \x20       R{index}_WORKER_{worker_index}_WORKSPACE_HEADER.words[0] = generation\n\
                 \x20       slot: usize = 1\n\
                 \x20       @budget(bound=7)\n\
                 \x20       while slot < 8:\n\
                 \x20           R{index}_WORKER_{worker_index}_WORKSPACE_HEADER.words[slot] = 0\n\
                 \x20           slot = slot + 1\n\
                 \x20       return true"
            )
            .expect("String writes cannot fail");
        }
    }
    output.push_str("    return false\n");

    output.push_str(
        "\npub fn __wrela_pixels_p7_workspace_charge(renderer: usize, worker: u32, slot: usize, amount: u64) -> bool:\n\
         \x20   if slot >= 7:\n\
         \x20       return false\n",
    );
    for (placement, renderer) in placements.iter().zip(compiled) {
        let index = placement.index;
        let capacity = &renderer.projective.program().capacities;
        for worker in &placement.per_core {
            let worker_index = worker.worker_index;
            writeln!(
                output,
                "    if renderer == {index} and worker == {worker_index}:\n\
                 \x20       if (slot == 0 and amount > {}) or (slot == 1 and amount > {}) or (slot == 4 and amount > {}) or (slot == 5 and amount > {}) or (slot == 6 and amount > {}):\n\
                 \x20           return false\n\
                 \x20       index = slot + 1\n\
                 \x20       before = R{index}_WORKER_{worker_index}_WORKSPACE_HEADER.words[index]\n\
                 \x20       after = before +% amount\n\
                 \x20       if after < before:\n\
                 \x20           return false\n\
                 \x20       R{index}_WORKER_{worker_index}_WORKSPACE_HEADER.words[index] = after\n\
                 \x20       return true",
                capacity.candidate_features_per_tile,
                capacity.row_start_roots,
                capacity.runs_per_row,
                capacity.candidate_features_per_tile,
                capacity.corridors_per_row,
            )
            .expect("String writes cannot fail");
        }
    }
    output.push_str("    return false\n");

    output.push_str(
        "\npub fn __wrela_pixels_p7_workspace_counter(renderer: usize, worker: u32, slot: usize) -> [u64; 2]:\n\
         \x20   if slot >= 7:\n\
         \x20       return [0; 2]\n",
    );
    for placement in placements {
        let index = placement.index;
        for worker in &placement.per_core {
            let worker_index = worker.worker_index;
            writeln!(
                output,
                "    if renderer == {index} and worker == {worker_index}:\n\
                 \x20       return [1, R{index}_WORKER_{worker_index}_WORKSPACE_HEADER.words[slot + 1]]"
            )
            .expect("String writes cannot fail");
        }
    }
    output.push_str("    return [0; 2]\n");

    output.push_str(
        "\npub fn __wrela_pixels_p7_workspace_store_record_word(renderer: usize, worker: u32, corridor: bool, record: u32, word: usize, value: u64) -> bool:\n",
    );
    for (placement, renderer) in placements.iter().zip(compiled) {
        let renderer_index = placement.index;
        let capacities = &renderer.projective.program().capacities;
        for worker in &placement.per_core {
            let worker_index = worker.worker_index;
            for (corridor, name, record_bytes, capacity) in [
                (
                    false,
                    "RUNS",
                    super::capacities::RUN_RECORD_BYTES_V1,
                    capacities.runs_per_row,
                ),
                (
                    true,
                    "CORRIDORS",
                    super::capacities::CORRIDOR_RECORD_BYTES_V1,
                    capacities.corridors_per_row,
                ),
            ] {
                let words_per_record = record_bytes / 8;
                let total_bytes = u64::from(capacity) * record_bytes;
                let mut chunk_offset = 0_u64;
                let mut chunk = 0_usize;
                while chunk_offset < total_bytes {
                    let chunk_bytes = (total_bytes - chunk_offset).min(WORKSPACE_VIEW_CHUNK_BYTES);
                    let first_record = chunk_offset / record_bytes;
                    let record_count = chunk_bytes / record_bytes;
                    writeln!(
                        output,
                        "    if renderer == {renderer_index} and worker == {worker_index} and corridor == {corridor} and word < {words_per_record} and record.to[u64]() >= {first_record} and record.to[u64]() < {}:\n\
                         \x20       base = (record.to[usize]() - {first_record}.to[usize]()) * {words_per_record}\n\
                         \x20       R{renderer_index}_WORKER_{worker_index}_WORKSPACE_{name}_CHUNK_{chunk}.words[base + word] = value\n\
                         \x20       return true",
                        first_record + record_count,
                    )
                    .expect("String writes cannot fail");
                    chunk_offset = chunk_offset.checked_add(chunk_bytes).ok_or_else(|| {
                        "P025: P7 record workspace chunk offset overflow".to_string()
                    })?;
                    chunk = chunk.checked_add(1).ok_or_else(|| {
                        "P015: P7 record workspace chunk count overflow".to_string()
                    })?;
                }
            }
        }
    }
    output.push_str("    return false\n");

    output.push_str(
        "\npub fn __wrela_pixels_p7_workspace_load_certified_run_word(renderer: usize, worker: u32, record: u32, word: usize) -> [u64; 2]:\n",
    );
    if instrumented {
        for (placement, renderer) in placements.iter().zip(compiled) {
            let renderer_index = placement.index;
            let capacities = &renderer.projective.program().capacities;
            for worker in &placement.per_core {
                let worker_index = worker.worker_index;
                let record_bytes = super::capacities::RUN_RECORD_BYTES_V1;
                let words_per_record = record_bytes / 8;
                let total_bytes = u64::from(capacities.runs_per_row) * record_bytes;
                let mut chunk_offset = 0_u64;
                let mut chunk = 0_usize;
                while chunk_offset < total_bytes {
                    let chunk_bytes = (total_bytes - chunk_offset).min(WORKSPACE_VIEW_CHUNK_BYTES);
                    let first_record = chunk_offset / record_bytes;
                    let record_count = chunk_bytes / record_bytes;
                    writeln!(
                        output,
                        "    if renderer == {renderer_index} and worker == {worker_index} and word < {words_per_record} and record.to[u64]() >= {first_record} and record.to[u64]() < {}:\n\
                         \x20       base = (record.to[usize]() - {first_record}.to[usize]()) * {words_per_record}\n\
                         \x20       return [1, R{renderer_index}_WORKER_{worker_index}_WORKSPACE_RUNS_CHUNK_{chunk}.words[base + word]]",
                        first_record + record_count,
                    )
                    .expect("String writes cannot fail");
                    chunk_offset = chunk_offset.checked_add(chunk_bytes).ok_or_else(|| {
                        "P025: P7 record workspace chunk offset overflow".to_string()
                    })?;
                    chunk = chunk.checked_add(1).ok_or_else(|| {
                        "P015: P7 record workspace chunk count overflow".to_string()
                    })?;
                }
            }
        }
    }
    output.push_str("    return [0; 2]\n");

    output.push_str(
        "\npub fn __wrela_pixels_p7_workspace_store_coverage(renderer: usize, worker: u32, corridor: bool, record: u32, read values: [i64; 8]) -> bool:\n\
         \x20   if values[0] < 0 or values[0] >= values[1] or values[1] > 65535 or values[2] < 0 or values[2] > 4294967295 or values[3] < 0 or values[3] > 4294967295 or values[4] < -2147483648 or values[4] > 2147483647 or values[5] < -2147483648 or values[5] > 2147483647 or values[6] < 0 or values[6] > 4294967295 or values[7] < 0 or values[7] > 4294967295:\n\
         \x20       return false\n\
         \x20   word0 = values[0].to[u64]() | (values[1].to[u64]() << 16)\n\
         \x20   word1 = values[2].to[u64]() | (values[3].to[u64]() << 32)\n\
         \x20   word2 = (values[4] & 4294967295).to[u64]() | ((values[5] & 4294967295).to[u64]() << 32)\n\
         \x20   word3 = values[6].to[u64]() | (values[7].to[u64]() << 32)\n\
         \x20   return (\n\
         \x20       __wrela_pixels_p7_workspace_store_record_word(renderer, worker, corridor, record, 0, word0)\n\
         \x20       and __wrela_pixels_p7_workspace_store_record_word(renderer, worker, corridor, record, 1, word1)\n\
         \x20       and __wrela_pixels_p7_workspace_store_record_word(renderer, worker, corridor, record, 2, word2)\n\
         \x20       and __wrela_pixels_p7_workspace_store_record_word(renderer, worker, corridor, record, 3, word3)\n\
         \x20   )\n",
    );

    if !instrumented {
        output.push_str(
            "\npub fn __wrela_pixels_p7_workspace_store_certified_run(renderer: usize, worker: u32, record: u32, read values: [i64; 8], read model: [i64; 8], read normal: [i64; 6], read slacks: [i64; 2], sample_meta: i64) -> bool:\n\
             \x20   return values[0] >= 0 and values[0] < values[1] and values[1] <= 65535 and record < 4294967295\n",
        );
    } else {
        output.push_str(
        "\npub fn __wrela_pixels_p7_workspace_store_certified_run(renderer: usize, worker: u32, record: u32, read values: [i64; 8], read model: [i64; 8], read normal: [i64; 6], read slacks: [i64; 2], sample_meta: i64) -> bool:\n\
         \x20   corridor = ((sample_meta >> 56) & 1) == 1\n\
         \x20   root_count = (sample_meta & 4294967295).to[u32]()\n\
         \x20   storage_record = record\n\
         \x20   point_witness = ((sample_meta >> 57) & 1) == 1\n\
         \x20   if not corridor and root_count & 65535 > 0 and point_witness:\n\
         \x20       storage_record = 0\n\
         \x20   elif not corridor and record == 0:\n\
         \x20       previous = __wrela_pixels_p7_workspace_load_certified_run_word(renderer, worker, 0, 14)\n\
         \x20       if previous[0] == 1 and previous[1] & 8 != 0:\n\
         \x20           return true\n\
         \x20   if not __wrela_pixels_p7_workspace_store_coverage(renderer, worker, corridor, storage_record, values):\n\
         \x20       return false\n\
         \x20   if not corridor and (slacks[0] <= 0 or slacks[1] <= 0):\n\
         \x20       return false\n\
         \x20   selected_sheet = root_count >> 16\n\
         \x20   proof_method = ((sample_meta >> 32) & 255).to[u32]()\n\
         \x20   composition_shape = ((sample_meta >> 40) & 255).to[u32]()\n\
         \x20   coverage = ((sample_meta >> 48) & 255).to[u32]()\n\
         \x20   event_id: i64 = 65535\n\
         \x20   owner_tag: i64 = (sample_meta >> 54) & 8\n\
         \x20   if corridor:\n\
         \x20       event_id = record.to[i64]()\n\
         \x20       owner_tag = 7\n\
         \x20   packed_method = (proof_method << 8) | (composition_shape << 16) | (coverage << 24)\n\
         \x20   packed_method_signed = packed_method.to[i64]()\n\
         \x20   if packed_method_signed >= 2147483648:\n\
         \x20       packed_method_signed = packed_method_signed - 4294967296\n\
         \x20   packed_method_signed = packed_method_signed | owner_tag\n\
         \x20   packed_sheet_value = ((root_count & 65535) << 16) | selected_sheet\n\
         \x20   packed_sheet_signed = packed_sheet_value.to[i64]()\n\
         \x20   if packed_sheet_signed >= 2147483648:\n\
         \x20       packed_sheet_signed = packed_sheet_signed - 4294967296\n\
         \x20   component: usize = 0\n\
         \x20   @budget(bound=8)\n\
         \x20   while component < 8:\n\
         \x20       if model[component] < -2147483648 or model[component] > 2147483647:\n\
         \x20           return false\n\
         \x20       component = component + 1\n\
         \x20   component = 0\n\
         \x20   @budget(bound=6)\n\
         \x20   while component < 6:\n\
         \x20       if normal[component] < -32767 or normal[component] > 32767:\n\
         \x20           return false\n\
         \x20       component = component + 1\n\
         \x20   if normal[0] > normal[1] or normal[2] > normal[3] or normal[4] > normal[5]:\n\
         \x20       return false\n\
         \x20   packed: [u64; 12] = [0; 12]\n\
         \x20   packed[0] = (model[0] & 4294967295).to[u64]() | ((model[1] & 4294967295).to[u64]() << 32)\n\
         \x20   packed[1] = (model[2] & 4294967295).to[u64]() | ((model[3] & 4294967295).to[u64]() << 32)\n\
         \x20   packed[2] = (model[4] & 4294967295).to[u64]() | ((model[5] & 4294967295).to[u64]() << 32)\n\
         \x20   packed[3] = (model[6] & 4294967295).to[u64]() | ((model[7] & 4294967295).to[u64]() << 32)\n\
         \x20   packed[4] = (slacks[0] & 4294967295).to[u64]() | ((slacks[0] & 4294967295).to[u64]() << 32)\n\
         \x20   packed[5] = (slacks[1] & 4294967295).to[u64]() | ((slacks[1] & 4294967295).to[u64]() << 32)\n\
         \x20   packed[6] = (normal[0] & 4294967295).to[u64]() | ((normal[1] & 4294967295).to[u64]() << 32)\n\
         \x20   packed[7] = (normal[2] & 4294967295).to[u64]() | ((normal[3] & 4294967295).to[u64]() << 32)\n\
         \x20   packed[8] = (normal[4] & 4294967295).to[u64]() | ((normal[5] & 4294967295).to[u64]() << 32)\n\
         \x20   packed[9] = event_id.to[u64]() | (event_id.to[u64]() << 32)\n\
         \x20   packed[10] = (packed_method_signed & 4294967295).to[u64]() | ((packed_method_signed & 4294967295).to[u64]() << 32)\n\
         \x20   packed[11] = (packed_sheet_signed & 4294967295).to[u64]() | ((packed_sheet_signed & 4294967295).to[u64]() << 32)\n\
         \x20   if corridor:\n\
         \x20       secondary_present: i64 = 0\n\
         \x20       if proof_method & 128 != 0:\n\
         \x20           secondary_present = 1\n\
         \x20       packed[0] = secondary_present.to[u64]() | (secondary_present.to[u64]() << 32)\n\
         \x20       packed[1] = selected_sheet.to[u64]() | (selected_sheet.to[u64]() << 32)\n\
         \x20       coverage_lo = values[7].to[u64]() >> 8 & 255\n\
         \x20       coverage_hi = values[7].to[u64]() >> 16 & 255\n\
         \x20       packed[2] = coverage_lo | (coverage_hi << 32)\n\
         \x20       packed[3] = packed[10]\n\
         \x20   evidence_words: usize = 12\n\
         \x20   if corridor:\n\
         \x20       evidence_words = 4\n\
         \x20   index: usize = 0\n\
         \x20   @budget(bound=12)\n\
         \x20   while index < evidence_words:\n\
         \x20       if not __wrela_pixels_p7_workspace_store_record_word(renderer, worker, corridor, storage_record, index + 4, packed[index]):\n\
         \x20           return false\n\
         \x20       index = index + 1\n\
         \x20   return true\n",
        );
    }

    output.push_str(
        "\npub fn __wrela_pixels_p7_workspace_store_root(renderer: usize, worker: u32, record: u32, read values: [i64; 8]) -> bool:\n\
         \x20   if values[0] < -2147483648 or values[0] > 2147483647 or values[1] < -2147483648 or values[1] > 2147483647 or values[0] > values[1] or values[2] < 0 or values[2] > 4294967295 or values[3] < 0 or values[3] > 4294967295 or values[4] < 0 or values[4] > 4294967295 or values[5] < 0 or values[5] > 1 or values[6] < 0 or values[6] > 255 or values[7] < 0 or values[7] > 2147483647:\n\
         \x20       return false\n\
         \x20   word0 = (values[0] & 4294967295).to[u64]() | ((values[1] & 4294967295).to[u64]() << 32)\n\
         \x20   word1 = values[2].to[u64]() | (values[3].to[u64]() << 32)\n\
         \x20   word2 = values[4].to[u64]() | (values[5].to[u64]() << 32) | (values[6].to[u64]() << 40)\n\
         \x20   word3 = values[7].to[u64]()\n",
    );
    for (placement, renderer) in placements.iter().zip(compiled) {
        let renderer_index = placement.index;
        let capacity = renderer.projective.program().capacities.root_stack_nodes;
        let record_bytes = super::capacities::ROOT_RECORD_BYTES_V1;
        let words_per_record = record_bytes / 8;
        let total_bytes = u64::from(capacity) * record_bytes;
        for worker in &placement.per_core {
            let worker_index = worker.worker_index;
            let mut chunk_offset = 0_u64;
            let mut chunk = 0_usize;
            while chunk_offset < total_bytes {
                let chunk_bytes = (total_bytes - chunk_offset).min(WORKSPACE_VIEW_CHUNK_BYTES);
                let first_record = chunk_offset / record_bytes;
                let record_count = chunk_bytes / record_bytes;
                writeln!(
                    output,
                    "    if renderer == {renderer_index} and worker == {worker_index} and record.to[u64]() >= {first_record} and record.to[u64]() < {}:\n\
                     \x20       base = (record.to[usize]() - {first_record}.to[usize]()) * {words_per_record}\n\
                     \x20       R{renderer_index}_WORKER_{worker_index}_WORKSPACE_ROOTS_CHUNK_{chunk}.words[base] = word0\n\
                     \x20       R{renderer_index}_WORKER_{worker_index}_WORKSPACE_ROOTS_CHUNK_{chunk}.words[base + 1] = word1\n\
                     \x20       R{renderer_index}_WORKER_{worker_index}_WORKSPACE_ROOTS_CHUNK_{chunk}.words[base + 2] = word2\n\
                     \x20       R{renderer_index}_WORKER_{worker_index}_WORKSPACE_ROOTS_CHUNK_{chunk}.words[base + 3] = word3\n\
                     \x20       return true",
                    first_record + record_count,
                )
                .expect("String writes cannot fail");
                chunk_offset = chunk_offset
                    .checked_add(chunk_bytes)
                    .ok_or_else(|| "P025: P7 root workspace chunk offset overflow".to_string())?;
                chunk = chunk
                    .checked_add(1)
                    .ok_or_else(|| "P015: P7 root workspace chunk count overflow".to_string())?;
            }
        }
    }
    output.push_str("    return false\n");

    output.push_str(
        "\npub fn __wrela_pixels_p7_workspace_load_root(renderer: usize, worker: u32, record: u32) -> [i64; 9]:\n",
    );
    for (placement, renderer) in placements.iter().zip(compiled) {
        let renderer_index = placement.index;
        let capacity = renderer.projective.program().capacities.root_stack_nodes;
        let record_bytes = super::capacities::ROOT_RECORD_BYTES_V1;
        let words_per_record = record_bytes / 8;
        let total_bytes = u64::from(capacity) * record_bytes;
        for worker in &placement.per_core {
            let worker_index = worker.worker_index;
            let mut chunk_offset = 0_u64;
            let mut chunk = 0_usize;
            while chunk_offset < total_bytes {
                let chunk_bytes = (total_bytes - chunk_offset).min(WORKSPACE_VIEW_CHUNK_BYTES);
                let first_record = chunk_offset / record_bytes;
                let record_count = chunk_bytes / record_bytes;
                writeln!(
                    output,
                    "    if renderer == {renderer_index} and worker == {worker_index} and record.to[u64]() >= {first_record} and record.to[u64]() < {}:\n\
                     \x20       base = (record.to[usize]() - {first_record}.to[usize]()) * {words_per_record}\n\
                     \x20       word0 = R{renderer_index}_WORKER_{worker_index}_WORKSPACE_ROOTS_CHUNK_{chunk}.words[base]\n\
                     \x20       word1 = R{renderer_index}_WORKER_{worker_index}_WORKSPACE_ROOTS_CHUNK_{chunk}.words[base + 1]\n\
                     \x20       word2 = R{renderer_index}_WORKER_{worker_index}_WORKSPACE_ROOTS_CHUNK_{chunk}.words[base + 2]\n\
                     \x20       word3 = R{renderer_index}_WORKER_{worker_index}_WORKSPACE_ROOTS_CHUNK_{chunk}.words[base + 3]\n\
                     \x20       lo = (word0 & 4294967295).to[i64]()\n\
                     \x20       hi = (word0 >> 32).to[i64]()\n\
                     \x20       if lo >= 2147483648:\n\
                     \x20           lo = lo - 4294967296\n\
                     \x20       if hi >= 2147483648:\n\
                     \x20           hi = hi - 4294967296\n\
                     \x20       return [1, lo, hi, (word1 & 4294967295).to[i64](), (word1 >> 32).to[i64](), (word2 & 4294967295).to[i64](), ((word2 >> 32) & 1).to[i64](), ((word2 >> 40) & 255).to[i64](), word3.to[i64]()]",
                    first_record + record_count,
                )
                .expect("String writes cannot fail");
                chunk_offset = chunk_offset.checked_add(chunk_bytes).ok_or_else(|| {
                    "P025: P7 root workspace load chunk offset overflow".to_string()
                })?;
                chunk = chunk.checked_add(1).ok_or_else(|| {
                    "P015: P7 root workspace load chunk count overflow".to_string()
                })?;
            }
        }
    }
    output.push_str("    return [0; 9]\n");

    output.push_str(
        "\npub fn __wrela_pixels_p7_workspace_store_root_tmp(renderer: usize, worker: u32, record: u32, read values: [i64; 8]) -> bool:\n\
         \x20   if values[0] < -2147483648 or values[0] > 2147483647 or values[1] < -2147483648 or values[1] > 2147483647 or values[0] > values[1] or values[2] < 0 or values[2] > 4294967295 or values[3] < 0 or values[3] > 4294967295 or values[4] < 0 or values[4] > 4294967295 or values[5] < 0 or values[5] > 1 or values[6] < 0 or values[6] > 255 or values[7] < 0 or values[7] > 2147483647:\n\
         \x20       return false\n\
         \x20   word0 = (values[0] & 4294967295).to[u64]() | ((values[1] & 4294967295).to[u64]() << 32)\n\
         \x20   word1 = values[2].to[u64]() | (values[3].to[u64]() << 32)\n\
         \x20   word2 = values[4].to[u64]() | (values[5].to[u64]() << 32) | (values[6].to[u64]() << 40)\n\
         \x20   word3 = values[7].to[u64]()\n",
    );
    for (placement, renderer) in placements.iter().zip(compiled) {
        let renderer_index = placement.index;
        let capacity = renderer.projective.program().capacities.root_stack_nodes;
        let record_bytes = super::capacities::ROOT_RECORD_BYTES_V1;
        let words_per_record = record_bytes / 8;
        let total_bytes = u64::from(capacity) * record_bytes;
        for worker in &placement.per_core {
            let worker_index = worker.worker_index;
            let mut chunk_offset = 0_u64;
            let mut chunk = 0_usize;
            while chunk_offset < total_bytes {
                let chunk_bytes = (total_bytes - chunk_offset).min(WORKSPACE_VIEW_CHUNK_BYTES);
                let first_record = chunk_offset / record_bytes;
                let record_count = chunk_bytes / record_bytes;
                writeln!(
                    output,
                    "    if renderer == {renderer_index} and worker == {worker_index} and record.to[u64]() >= {first_record} and record.to[u64]() < {}:\n\
                     \x20       base = (record.to[usize]() - {first_record}.to[usize]()) * {words_per_record}\n\
                     \x20       R{renderer_index}_WORKER_{worker_index}_WORKSPACE_ROOTS_TMP_CHUNK_{chunk}.words[base] = word0\n\
                     \x20       R{renderer_index}_WORKER_{worker_index}_WORKSPACE_ROOTS_TMP_CHUNK_{chunk}.words[base + 1] = word1\n\
                     \x20       R{renderer_index}_WORKER_{worker_index}_WORKSPACE_ROOTS_TMP_CHUNK_{chunk}.words[base + 2] = word2\n\
                     \x20       R{renderer_index}_WORKER_{worker_index}_WORKSPACE_ROOTS_TMP_CHUNK_{chunk}.words[base + 3] = word3\n\
                     \x20       return true",
                    first_record + record_count,
                )
                .expect("String writes cannot fail");
                chunk_offset = chunk_offset.checked_add(chunk_bytes).ok_or_else(|| {
                    "P025: P7 temporary root workspace chunk offset overflow".to_string()
                })?;
                chunk = chunk.checked_add(1).ok_or_else(|| {
                    "P015: P7 temporary root workspace chunk count overflow".to_string()
                })?;
            }
        }
    }
    output.push_str("    return false\n");

    output.push_str(
        "\npub fn __wrela_pixels_p7_workspace_load_root_tmp(renderer: usize, worker: u32, record: u32) -> [i64; 9]:\n",
    );
    for (placement, renderer) in placements.iter().zip(compiled) {
        let renderer_index = placement.index;
        let capacity = renderer.projective.program().capacities.root_stack_nodes;
        let record_bytes = super::capacities::ROOT_RECORD_BYTES_V1;
        let words_per_record = record_bytes / 8;
        let total_bytes = u64::from(capacity) * record_bytes;
        for worker in &placement.per_core {
            let worker_index = worker.worker_index;
            let mut chunk_offset = 0_u64;
            let mut chunk = 0_usize;
            while chunk_offset < total_bytes {
                let chunk_bytes = (total_bytes - chunk_offset).min(WORKSPACE_VIEW_CHUNK_BYTES);
                let first_record = chunk_offset / record_bytes;
                let record_count = chunk_bytes / record_bytes;
                writeln!(
                    output,
                    "    if renderer == {renderer_index} and worker == {worker_index} and record.to[u64]() >= {first_record} and record.to[u64]() < {}:\n\
                     \x20       base = (record.to[usize]() - {first_record}.to[usize]()) * {words_per_record}\n\
                     \x20       word0 = R{renderer_index}_WORKER_{worker_index}_WORKSPACE_ROOTS_TMP_CHUNK_{chunk}.words[base]\n\
                     \x20       word1 = R{renderer_index}_WORKER_{worker_index}_WORKSPACE_ROOTS_TMP_CHUNK_{chunk}.words[base + 1]\n\
                     \x20       word2 = R{renderer_index}_WORKER_{worker_index}_WORKSPACE_ROOTS_TMP_CHUNK_{chunk}.words[base + 2]\n\
                     \x20       word3 = R{renderer_index}_WORKER_{worker_index}_WORKSPACE_ROOTS_TMP_CHUNK_{chunk}.words[base + 3]\n\
                     \x20       lo = (word0 & 4294967295).to[i64]()\n\
                     \x20       hi = (word0 >> 32).to[i64]()\n\
                     \x20       if lo >= 2147483648:\n\
                     \x20           lo = lo - 4294967296\n\
                     \x20       if hi >= 2147483648:\n\
                     \x20           hi = hi - 4294967296\n\
                     \x20       return [1, lo, hi, (word1 & 4294967295).to[i64](), (word1 >> 32).to[i64](), (word2 & 4294967295).to[i64](), ((word2 >> 32) & 1).to[i64](), ((word2 >> 40) & 255).to[i64](), word3.to[i64]()]",
                    first_record + record_count,
                )
                .expect("String writes cannot fail");
                chunk_offset = chunk_offset.checked_add(chunk_bytes).ok_or_else(|| {
                    "P025: P7 temporary root workspace load chunk offset overflow".to_string()
                })?;
                chunk = chunk.checked_add(1).ok_or_else(|| {
                    "P015: P7 temporary root workspace load chunk count overflow".to_string()
                })?;
            }
        }
    }
    output.push_str("    return [0; 9]\n");

    output.push_str(
        "\npub fn __wrela_pixels_p7_telemetry_reset(renderer: usize, worker: u32) -> bool:\n",
    );
    if instrumented {
        for placement in placements {
            let renderer_index = placement.index;
            for worker in &placement.per_core {
                let worker_index = worker.worker_index;
                writeln!(
                    output,
                    "    if renderer == {renderer_index} and worker == {worker_index}:\n\
                     \x20       counter: usize = 0\n\
                     \x20       @budget(bound={})\n\
                     \x20       while counter < {}:\n\
                     \x20           R{renderer_index}_WORKER_{worker_index}_TELEMETRY.counters[counter] = 0\n\
                     \x20           counter = counter + 1\n\
                     \x20       return true",
                    super::reference::telemetry::CERTIFICATE_TELEMETRY_COUNTERS_V2,
                    super::reference::telemetry::CERTIFICATE_TELEMETRY_COUNTERS_V2,
                )
                .expect("String writes cannot fail");
            }
        }
    }
    output.push_str("    return false\n");

    output.push_str(&format!(
        "\npub fn __wrela_pixels_p7_telemetry_charge(renderer: usize, worker: u32, counter: usize, amount: u64) -> bool:\n\
         \x20   if counter >= {}:\n\
         \x20       return false\n",
        super::reference::telemetry::CERTIFICATE_TELEMETRY_COUNTERS_V2,
    ));
    if instrumented {
        for placement in placements {
            let renderer_index = placement.index;
            for worker in &placement.per_core {
                let worker_index = worker.worker_index;
                writeln!(
                    output,
                    "    if renderer == {renderer_index} and worker == {worker_index}:\n\
                     \x20       before = R{renderer_index}_WORKER_{worker_index}_TELEMETRY.counters[counter]\n\
                     \x20       after = before +% amount\n\
                     \x20       if after < before:\n\
                     \x20           return false\n\
                     \x20       R{renderer_index}_WORKER_{worker_index}_TELEMETRY.counters[counter] = after\n\
                     \x20       return true"
                )
                .expect("String writes cannot fail");
            }
        }
    }
    output.push_str("    return false\n");

    output.push_str(&format!(
        "\npub fn __wrela_pixels_p7_telemetry_counter(renderer: usize, worker: u32, counter: usize) -> [u64; 2]:\n\
         \x20   if counter >= {}:\n\
         \x20       return [0; 2]\n",
        super::reference::telemetry::CERTIFICATE_TELEMETRY_COUNTERS_V2,
    ));
    if instrumented {
        for placement in placements {
            let renderer_index = placement.index;
            for worker in &placement.per_core {
                let worker_index = worker.worker_index;
                writeln!(
                    output,
                    "    if renderer == {renderer_index} and worker == {worker_index}:\n\
                     \x20       return [1, R{renderer_index}_WORKER_{worker_index}_TELEMETRY.counters[counter]]"
                )
                .expect("String writes cannot fail");
            }
        }
    }
    output.push_str("    return [0; 2]\n");

    output.push_str(
        "\npub fn __wrela_pixels_p8_raster_evidence_store(renderer: usize, pixel: usize, q: u64, derivatives: u64, bounds: u64, class: u8) -> bool:\n",
    );
    if instrumented {
        output.push_str(
            "    if class < 1 or class > 3 or bounds >> 62 != 0:\n\
             \x20       return false\n\
             \x20   word0 = q\n\
             \x20   word1 = derivatives\n\
             \x20   word2 = bounds | (class.to[u64]() << 62)\n",
        );
    }
    for (placement, renderer) in placements.iter().zip(compiled) {
        let renderer_index = placement.index;
        let pixel_count = u64::from(renderer.config.width) * u64::from(renderer.config.height);
        if instrumented {
            let records_per_chunk = WORKSPACE_VIEW_CHUNK_BYTES / 24;
            let mut first = 0_u64;
            let mut chunk = 0_usize;
            while first < pixel_count {
                let count = (pixel_count - first).min(records_per_chunk);
                writeln!(
                    output,
                    "    if renderer == {renderer_index} and pixel.to[u64]() >= {first} and pixel.to[u64]() < {}:\n\
                     \x20       base = (pixel - {first}.to[usize]()) * 3\n\
                     \x20       R{renderer_index}_RASTER_EVIDENCE_CHUNK_{chunk}.words[base] = word0\n\
                     \x20       R{renderer_index}_RASTER_EVIDENCE_CHUNK_{chunk}.words[base + 1] = word1\n\
                     \x20       R{renderer_index}_RASTER_EVIDENCE_CHUNK_{chunk}.words[base + 2] = word2\n\
                     \x20       return true",
                    first + count,
                )
                .expect("String writes cannot fail");
                first += count;
                chunk += 1;
            }
        } else {
            writeln!(
                output,
                "    if renderer == {renderer_index} and pixel.to[u64]() < {pixel_count}:\n\
                 \x20       return true"
            )
            .expect("String writes cannot fail");
        }
    }
    output.push_str("    return false\n");

    output.push_str(
        "\npub fn __wrela_pixels_p8_raster_evidence_word(renderer: usize, word: usize) -> [u64; 2]:\n",
    );
    if instrumented {
        for (placement, renderer) in placements.iter().zip(compiled) {
            let renderer_index = placement.index;
            let evidence_words =
                u64::from(renderer.config.width) * u64::from(renderer.config.height) * 3;
            let words_per_chunk = WORKSPACE_VIEW_CHUNK_BYTES / 8;
            let mut first = 0_u64;
            let mut chunk = 0_usize;
            while first < evidence_words {
                let count = (evidence_words - first).min(words_per_chunk);
                writeln!(
                    output,
                    "    if renderer == {renderer_index} and word.to[u64]() >= {first} and word.to[u64]() < {}:\n\
                     \x20       return [1, R{renderer_index}_RASTER_EVIDENCE_CHUNK_{chunk}.words[word - {first}.to[usize]()]]",
                    first + count,
                )
                .expect("String writes cannot fail");
                first += count;
                chunk += 1;
            }
        }
    }
    output.push_str("    return [0; 2]\n");

    Ok(())
}

fn write_p8_scanout_accessors(
    output: &mut String,
    placements: &[crate::layout::RendererPlacement],
    compiled: &[super::CompiledRenderer],
) -> Result<(), String> {
    const INITIALIZED: u64 = 0x5752_454c_4150_5838;
    for (placement, renderer) in placements.iter().zip(compiled) {
        let index = placement.index;
        let width = u64::from(renderer.config.width);
        let height = u64::from(renderer.config.height);
        let pixel_count = width
            .checked_mul(height)
            .ok_or_else(|| "P015: P8 visible pixel count overflow".to_string())?;
        let tile_columns = width.div_ceil(u64::from(wrela_machine::pixels::TILE_WIDTH));
        let tile_rows = height.div_ceil(u64::from(wrela_machine::pixels::TILE_HEIGHT));
        let tile_count = tile_columns
            .checked_mul(tile_rows)
            .ok_or_else(|| "P015: P8 scanout tile count overflow".to_string())?;
        let generation_bytes = tile_count
            .checked_mul(wrela_machine::pixels::TILE_ALLOCATION_BYTES as u64)
            .ok_or_else(|| "P015: P8 scanout generation bytes overflow".to_string())?;
        if placement.framebuffer_bytes != generation_bytes * 2 {
            return Err(format!(
                "pixels::glue: renderer {index} framebuffer does not contain two exact scanout generations"
            ));
        }
        let list = &renderer.mutable_layout.tile_descriptors;
        let control_bytes = wrela_machine::pixels::CONTROL_BYTES as u64;
        let per_generation_list_bytes = control_bytes
            .checked_add(
                tile_count
                    .checked_mul(24)
                    .ok_or_else(|| "P015: P8 descriptor-list bytes overflow".to_string())?,
            )
            .ok_or_else(|| "P015: P8 control/list bytes overflow".to_string())?;
        if list.bytes != per_generation_list_bytes * 2 {
            return Err(format!(
                "pixels::glue: renderer {index} display-list storage is not two exact generations"
            ));
        }
        let list_base = placement
            .state_base
            .checked_add(list.offset)
            .ok_or_else(|| "P025: P8 display-list base overflow".to_string())?;
        let full_single_tile = width == u64::from(wrela_machine::pixels::TILE_WIDTH)
            && height == u64::from(wrela_machine::pixels::TILE_HEIGHT);
        let mut descriptor_digests = [[0_u64; 4]; 2];
        for generation in 0..2_u64 {
            let mut bytes = Vec::with_capacity(tile_count as usize * 24);
            for tile in 0..tile_count {
                let x = (tile % tile_columns) * 64;
                let y = (tile / tile_columns) * 32;
                let guest_addr = placement
                    .framebuffer_base
                    .checked_add(generation * generation_bytes)
                    .and_then(|base| {
                        base.checked_add(tile * wrela_machine::pixels::TILE_ALLOCATION_BYTES as u64)
                    })
                    .ok_or_else(|| "P025: P8 descriptor tile address overflow".to_string())?;
                let descriptor = wrela_machine::pixels::DisplayTileDescV1 {
                    guest_addr,
                    x: u16::try_from(x)
                        .map_err(|_| "P015: P8 descriptor x exceeds u16".to_string())?,
                    y: u16::try_from(y)
                        .map_err(|_| "P015: P8 descriptor y exceeds u16".to_string())?,
                    width: u16::try_from((width - x).min(64))
                        .map_err(|_| "P015: P8 descriptor width exceeds u16".to_string())?,
                    height: u16::try_from((height - y).min(32))
                        .map_err(|_| "P015: P8 descriptor height exceeds u16".to_string())?,
                    stride_bytes: wrela_machine::pixels::TILE_STRIDE_BYTES as u16,
                    format: wrela_machine::pixels::FORMAT_BGRA8_SRGB,
                    reserved: [0; 5],
                };
                bytes.extend_from_slice(&descriptor.encode());
            }
            descriptor_digests[generation as usize] =
                wrela_machine::pixels::guest_bounded_digest(&bytes);
        }

        let mut offset = 0_u64;
        let mut chunk = 0_usize;
        writeln!(
            output,
            "\npub fn __wrela_pixels_p8_list_store_word_r{index}(byte: usize, value: u64) -> bool:\n\
             \x20   if byte % 8 != 0:\n\
             \x20       return false"
        )
        .expect("String writes cannot fail");
        while offset < list.bytes {
            let chunk_bytes = (list.bytes - offset).min(WORKSPACE_VIEW_CHUNK_BYTES);
            writeln!(
                output,
                "    if byte.to[u64]() >= {offset} and byte.to[u64]() + 8 <= {}:\n\
                 \x20       R{index}_DISPLAY_LIST_CHUNK_{chunk}.words[(byte.to[u64]() - {offset}).to[usize]() / 8] = value\n\
                 \x20       return true",
                offset + chunk_bytes,
            )
            .expect("String writes cannot fail");
            offset += chunk_bytes;
            chunk += 1;
        }
        output.push_str("    return false\n");

        writeln!(
            output,
            "\npub fn __wrela_pixels_p8_list_load_word_r{index}(byte: usize) -> [u64; 2]:\n\
             \x20   if byte % 8 != 0:\n\
             \x20       return [0; 2]"
        )
        .expect("String writes cannot fail");
        offset = 0;
        chunk = 0;
        while offset < list.bytes {
            let chunk_bytes = (list.bytes - offset).min(WORKSPACE_VIEW_CHUNK_BYTES);
            writeln!(
                output,
                "    if byte.to[u64]() >= {offset} and byte.to[u64]() + 8 <= {}:\n\
                 \x20       return [1, R{index}_DISPLAY_LIST_CHUNK_{chunk}.words[(byte.to[u64]() - {offset}).to[usize]() / 8]]",
                offset + chunk_bytes,
            )
            .expect("String writes cannot fail");
            offset += chunk_bytes;
            chunk += 1;
        }
        output.push_str("    return [0; 2]\n");

        writeln!(
            output,
            "\npub fn __wrela_pixels_p8_initialize_r{index}() -> bool:\n\
             \x20   if R{index}_SCANOUT_STATE.words[0] == {INITIALIZED}:\n\
             \x20       return true\n\
             \x20   if R{index}_SCANOUT_STATE.words[0] != 0:\n\
             \x20       return false"
        )
        .expect("String writes cannot fail");
        writeln!(
            output,
            "    # Both complete generations are zero-initialized image statics.\n\
             \x20   # Initialization seals that one-time boot clear; later frames overwrite\n\
             \x20   # visible pixels and never touch padding.\n\
             \x20   R{index}_SCANOUT_STATE.words[0] = {INITIALIZED}\n\
             \x20   R{index}_SCANOUT_STATE.words[1] = 0\n\
             \x20   R{index}_SCANOUT_STATE.words[2] = 0\n\
             \x20   R{index}_SCANOUT_STATE.words[3] = 0\n\
             \x20   return true"
        )
        .expect("String writes cannot fail");

        writeln!(
            output,
            "\npub fn __wrela_pixels_p8_begin_r{index}() -> [u64; 3]:\n\
             \x20   if not __wrela_pixels_p8_initialize_r{index}() or R{index}_SCANOUT_STATE.words[3] != 0:\n\
             \x20       return [0; 3]\n\
             \x20   generation: u64 = 0\n\
             \x20   if R{index}_SCANOUT_STATE.words[1] == 1:\n\
             \x20       generation = 1\n\
             \x20   elif R{index}_SCANOUT_STATE.words[1] > 2:\n\
             \x20       return [0; 3]\n\
             \x20   R{index}_SCANOUT_STATE.words[3] = generation + 1\n\
             \x20   return [1, generation, R{index}_SCANOUT_STATE.words[2]]"
        )
        .expect("String writes cannot fail");

        let visible_address = if full_single_tile {
            format!(
                "    generation = owner - 1\n\
                 \x20   offset = generation * {generation_bytes} + byte.to[u64]()"
            )
        } else {
            format!(
                "    pixel = byte.to[u64]() / 4\n\
                 \x20   x = pixel % {width}\n\
                 \x20   y = pixel / {width}\n\
                 \x20   tile = (y / 32) * {tile_columns} + x / 64\n\
                 \x20   local = (y % 32) * 256 + (x % 64) * 4 + byte.to[u64]() % 4\n\
                 \x20   generation = owner - 1\n\
                 \x20   offset = generation * {generation_bytes} + tile * 8192 + local"
            )
        };
        let visible_return = if full_single_tile {
            format!(
                "    word = offset.to[usize]() / 8\n\
                 \x20   shift = (offset % 8) * 8\n\
                 \x20   return [1, (R{index}_DEBUG_FRAMEBUFFER_CHUNK_0.words[word] >> shift) & 255]"
            )
        } else {
            format!(
                "    return __wrela_pixels_p7_framebuffer_load_byte({index}, offset.to[usize]())"
            )
        };
        writeln!(
            output,
            "\npub fn __wrela_pixels_p8_visible_byte_r{index}(byte: usize) -> [u64; 2]:\n\
             \x20   owner = R{index}_SCANOUT_STATE.words[3]\n\
             \x20   if owner == 0:\n\
             \x20       owner = R{index}_SCANOUT_STATE.words[1]\n\
             \x20   if byte.to[u64]() >= {} or owner == 0:\n\
             \x20       return [0; 2]\n\
             {visible_address}\n\
             {visible_return}",
            pixel_count * 4,
        )
        .expect("String writes cannot fail");

        if full_single_tile {
            writeln!(
                output,
                "\npub fn __wrela_pixels_p8_visible_word_r{index}(word: usize) -> [u64; 2]:\n\
                 \x20   owner = R{index}_SCANOUT_STATE.words[3]\n\
                 \x20   if owner == 0:\n\
                 \x20       owner = R{index}_SCANOUT_STATE.words[1]\n\
                 \x20   if owner == 0 or word >= 1024:\n\
                 \x20       return [0; 2]\n\
                 \x20   return [1, R{index}_DEBUG_FRAMEBUFFER_CHUNK_0.words[(owner - 1).to[usize]() * 1024 + word]]"
            )
            .expect("String writes cannot fail");
        } else {
            writeln!(
                output,
                "\npub fn __wrela_pixels_p8_visible_word_r{index}(word: usize) -> [u64; 2]:\n\
                 \x20   if word.to[u64]() * 8 >= {}:\n\
                 \x20       return [0; 2]\n\
                 \x20   value: u64 = 0\n\
                 \x20   octet: usize = 0\n\
                 \x20   @budget(bound=8)\n\
                 \x20   while octet < 8 and word.to[u64]() * 8 + octet.to[u64]() < {}:\n\
                 \x20       loaded = __wrela_pixels_p8_visible_byte_r{index}(word * 8 + octet)\n\
                 \x20       if loaded[0] != 1:\n\
                 \x20           return [0; 2]\n\
                 \x20       value = value | (loaded[1] << (octet * 8).to[u64]())\n\
                 \x20       octet = octet + 1\n\
                 \x20   return [1, value]",
                pixel_count * 4,
                pixel_count * 4,
            )
            .expect("String writes cannot fail");
        }

        writeln!(
            output,
            "\npub fn __wrela_pixels_p8_load_u32_r{index}(byte: usize) -> [u64; 2]:\n\
             \x20   if byte % 4 != 0:\n\
             \x20       return [0; 2]\n\
             \x20   shift = (byte % 8).to[u64]() * 8"
        )
        .expect("String writes cannot fail");
        let mut framebuffer_offset = 0;
        let mut framebuffer_chunk = 0;
        while framebuffer_offset < placement.framebuffer_bytes {
            let chunk_bytes =
                (placement.framebuffer_bytes - framebuffer_offset).min(WORKSPACE_VIEW_CHUNK_BYTES);
            writeln!(
                output,
                "    if byte.to[u64]() >= {framebuffer_offset} and byte.to[u64]() + 4 <= {}:\n\
                 \x20       word = (byte.to[u64]() - {framebuffer_offset}).to[usize]() / 8\n\
                 \x20       return [1, (R{index}_DEBUG_FRAMEBUFFER_CHUNK_{framebuffer_chunk}.words[word] >> shift) & 4294967295]",
                framebuffer_offset + chunk_bytes,
            )
            .expect("String writes cannot fail");
            framebuffer_offset += chunk_bytes;
            framebuffer_chunk += 1;
        }
        output.push_str("    return [0; 2]\n");

        writeln!(
            output,
            "\npub fn __wrela_pixels_p8_store_u32_r{index}(byte: usize, value: u64) -> bool:\n\
             \x20   if byte % 4 != 0:\n\
             \x20       return false\n\
             \x20   shift = (byte % 8).to[u64]() * 8\n\
             \x20   mask = 4294967295.to[u64]() << shift"
        )
        .expect("String writes cannot fail");
        framebuffer_offset = 0;
        framebuffer_chunk = 0;
        while framebuffer_offset < placement.framebuffer_bytes {
            let chunk_bytes =
                (placement.framebuffer_bytes - framebuffer_offset).min(WORKSPACE_VIEW_CHUNK_BYTES);
            writeln!(
                output,
                "    if byte.to[u64]() >= {framebuffer_offset} and byte.to[u64]() + 4 <= {}:\n\
                 \x20       word = (byte.to[u64]() - {framebuffer_offset}).to[usize]() / 8\n\
                 \x20       old = R{index}_DEBUG_FRAMEBUFFER_CHUNK_{framebuffer_chunk}.words[word]\n\
                 \x20       R{index}_DEBUG_FRAMEBUFFER_CHUNK_{framebuffer_chunk}.words[word] = (old & (mask ^ 18446744073709551615)) | ((value & 4294967295) << shift)\n\
                 \x20       return true",
                framebuffer_offset + chunk_bytes,
            )
            .expect("String writes cannot fail");
            framebuffer_offset += chunk_bytes;
            framebuffer_chunk += 1;
        }
        output.push_str("    return false\n");

        writeln!(
            output,
            "\npub fn __wrela_pixels_p8_cancel_r{index}() -> bool:\n\
             \x20   if R{index}_SCANOUT_STATE.words[3] == 0:\n\
             \x20       return false\n\
             \x20   R{index}_SCANOUT_STATE.words[3] = 0\n\
             \x20   return true"
        )
        .expect("String writes cannot fail");

        let owner_clauses = placement
            .per_core
            .iter()
            .map(|assignment| {
                format!(
                    "(worker == {} and owner_tile >= {} and owner_tile < {})",
                    assignment.worker_index, assignment.tiles_start, assignment.tiles_end,
                )
            })
            .collect::<Vec<_>>()
            .join(" or ");
        let ownership_check = format!(
            "    owner_x = pixel.to[u64]() % {width}\n\
             \x20   owner_y = pixel.to[u64]() / {width}\n\
             \x20   owner_tile = (owner_y / 32) * {tile_columns} + owner_x / 64\n\
             \x20   if not ({owner_clauses}):\n\
             \x20       return false"
        );
        let write_address = if full_single_tile {
            format!(
                "    generation = R{index}_SCANOUT_STATE.words[3] - 1\n\
                 \x20   byte = generation * {generation_bytes} + pixel.to[u64]() * 4"
            )
        } else {
            format!(
                "    x = pixel.to[u64]() % {width}\n\
                 \x20   y = pixel.to[u64]() / {width}\n\
                 \x20   tile = (y / 32) * {tile_columns} + x / 64\n\
                 \x20   local = (y % 32) * 256 + (x % 64) * 4\n\
                 \x20   generation = R{index}_SCANOUT_STATE.words[3] - 1\n\
                 \x20   byte = generation * {generation_bytes} + tile * 8192 + local"
            )
        };
        let write_store = if full_single_tile {
            format!(
                "    value = blue.to[u64]() | (green.to[u64]() << 8) | (red.to[u64]() << 16) | (255.to[u64]() << 24)\n\
                 \x20   word = (byte / 8).to[usize]()\n\
                 \x20   shift = (byte % 8) * 8\n\
                 \x20   mask = 4294967295.to[u64]() << shift\n\
                 \x20   old = R{index}_DEBUG_FRAMEBUFFER_CHUNK_0.words[word]\n\
                 \x20   R{index}_DEBUG_FRAMEBUFFER_CHUNK_0.words[word] = (old & (mask ^ 18446744073709551615)) | (value << shift)\n\
                 \x20   return true"
            )
        } else {
            format!(
                "    value = blue.to[u64]() | (green.to[u64]() << 8) | (red.to[u64]() << 16) | (255.to[u64]() << 24)\n\
                 \x20   return __wrela_pixels_p8_store_u32_r{index}(byte.to[usize](), value)"
            )
        };
        let write_function = if placements.len() == 1 {
            "__wrela_pixels_p8_write".to_string()
        } else {
            format!("__wrela_pixels_p8_write_r{index}")
        };
        writeln!(
            output,
            "\npub fn {write_function}(renderer: usize, worker: u32, pixel: u32, front: u32, coverage: u8, back: u32) -> bool:\n\
             \x20   if renderer != {index}:\n\
             \x20       return false\n\
             \x20   if pixel.to[u64]() >= {pixel_count} or R{index}_SCANOUT_STATE.words[3] == 0:\n\
             \x20       return false\n\
             {ownership_check}\n\
             {write_address}\n\
             \x20   front_weight = coverage.to[u32]()\n\
             \x20   back_weight = 255.to[u32]() - front_weight\n\
             \x20   red = ((front & 255) * front_weight + (back & 255) * back_weight + 127) / 255\n\
             \x20   green = (((front >> 8) & 255) * front_weight + ((back >> 8) & 255) * back_weight + 127) / 255\n\
             \x20   blue = (((front >> 16) & 255) * front_weight + ((back >> 16) & 255) * back_weight + 127) / 255\n\
             {write_store}"
        )
        .expect("String writes cannot fail");

        let write4_function = if placements.len() == 1 {
            "__wrela_pixels_p8_write4".to_string()
        } else {
            format!("__wrela_pixels_p8_write4_r{index}")
        };
        let write4_address = if full_single_tile {
            format!(
                "    generation = R{index}_SCANOUT_STATE.words[3] - 1\n\
                 \x20   byte = generation * {generation_bytes} + pixel.to[u64]() * 4"
            )
        } else {
            format!(
                "    x = pixel.to[u64]() % {width}\n\
                 \x20   y = pixel.to[u64]() / {width}\n\
                 \x20   tile = (y / 32) * {tile_columns} + x / 64\n\
                 \x20   local = (y % 32) * 256 + (x % 64) * 4\n\
                 \x20   generation = R{index}_SCANOUT_STATE.words[3] - 1\n\
                 \x20   byte = generation * {generation_bytes} + tile * 8192 + local"
            )
        };
        let write4_store = if full_single_tile {
            format!(
                "    word = (byte / 8).to[usize]()\n\
                 \x20   R{index}_DEBUG_FRAMEBUFFER_CHUNK_0.words[word] = packed\n\
                 \x20   R{index}_DEBUG_FRAMEBUFFER_CHUNK_0.words[word + 1] = packed\n\
                 \x20   return true"
            )
        } else {
            format!(
                "    return (\n\
                 \x20       __wrela_pixels_p8_store_u32_r{index}(byte.to[usize](), value)\n\
                 \x20       and __wrela_pixels_p8_store_u32_r{index}(byte.to[usize]() + 4, value)\n\
                 \x20       and __wrela_pixels_p8_store_u32_r{index}(byte.to[usize]() + 8, value)\n\
                 \x20       and __wrela_pixels_p8_store_u32_r{index}(byte.to[usize]() + 12, value)\n\
                 \x20   )"
            )
        };
        writeln!(
            output,
            "\npub fn {write4_function}(renderer: usize, worker: u32, pixel: u32, r: u8, g: u8, b: u8) -> bool:\n\
             \x20   if (\n\
             \x20       renderer != {index}\n\
             \x20       or R{index}_SCANOUT_STATE.words[3] == 0\n\
             \x20       or pixel.to[u64]() + 4 > {pixel_count}\n\
             \x20       or pixel.to[u64]() % {width} + 4 > {width}\n\
             \x20       or (pixel.to[u64]() % {width}) % 4 != 0\n\
             \x20   ):\n\
             \x20       return false\n\
             {ownership_check}\n\
             \x20   if owner_x % 64 + 4 > 64:\n\
             \x20       return false\n\
             {write4_address}\n\
             \x20   value = b.to[u64]() | (g.to[u64]() << 8) | (r.to[u64]() << 16) | (255.to[u64]() << 24)\n\
             \x20   packed = value | (value << 32)\n\
             {write4_store}"
        )
        .expect("String writes cannot fail");

        if full_single_tile {
            writeln!(
                output,
                "\npub fn __wrela_pixels_p8_digest_r{index}() -> [u64; 5]:\n\
                 \x20   owner = R{index}_SCANOUT_STATE.words[3]\n\
                 \x20   if owner == 0:\n\
                 \x20       owner = R{index}_SCANOUT_STATE.words[1]\n\
                 \x20   if owner == 0:\n\
                 \x20       return [0; 5]\n\
                 \x20   base_word = (owner - 1).to[usize]() * 1024\n\
                 \x20   h0: u64 = 1469598103934665603\n\
                 \x20   h1: u64 = 1099511628211\n\
                 \x20   h2: u64 = 7809847782465536322\n\
                 \x20   h3: u64 = 1609587929392839161\n\
                 \x20   word: usize = 0\n\
                 \x20   @budget(bound=1024)\n\
                 \x20   while word < 1024:\n\
                 \x20       packed = R{index}_DEBUG_FRAMEBUFFER_CHUNK_0.words[base_word + word]\n\
                 \x20       octet: usize = 0\n\
                 \x20       @budget(bound=8)\n\
                 \x20       while octet < 8:\n\
                 \x20           byte = word * 8 + octet\n\
                 \x20           value = packed & 255\n\
                 \x20           h0 = (h0 ^ value) *% 1099511628211\n\
                 \x20           h1 = (h1 ^ (value +% byte.to[u64]())) *% 14029467366897019727\n\
                 \x20           h2 = (h2 +% value) *% 11400714785074694791\n\
                 \x20           h3 = (h3 ^ (value << octet.to[u64]())) *% 9650029242287828579\n\
                 \x20           packed = packed >> 8\n\
                 \x20           octet = octet + 1\n\
                 \x20       word = word + 1\n\
                 \x20   return [1, h0, h1, h2, h3]"
            )
            .expect("String writes cannot fail");
        } else {
            writeln!(
                output,
                "\npub fn __wrela_pixels_p8_digest_r{index}() -> [u64; 5]:\n\
                 \x20   if R{index}_SCANOUT_STATE.words[3] == 0 and R{index}_SCANOUT_STATE.words[1] == 0:\n\
                 \x20       return [0; 5]\n\
                 \x20   h0: u64 = 1469598103934665603\n\
                 \x20   h1: u64 = 1099511628211\n\
                 \x20   h2: u64 = 7809847782465536322\n\
                 \x20   h3: u64 = 1609587929392839161\n\
                 \x20   pixel: usize = 0\n\
                 \x20   @budget(bound={})\n\
                 \x20   while pixel < {}:\n\
                 \x20       x = pixel.to[u64]() % {width}\n\
                 \x20       y = pixel.to[u64]() / {width}\n\
                 \x20       tile = (y / 32) * {tile_columns} + x / 64\n\
                 \x20       local = (y % 32) * 256 + (x % 64) * 4\n\
                 \x20       owner = R{index}_SCANOUT_STATE.words[3]\n\
                 \x20       if owner == 0:\n\
                 \x20           owner = R{index}_SCANOUT_STATE.words[1]\n\
                 \x20       if owner == 0:\n\
                 \x20           return [0; 5]\n\
                 \x20       offset = (owner - 1) * {generation_bytes} + tile * 8192 + local\n\
                 \x20       loaded = __wrela_pixels_p8_load_u32_r{index}(offset.to[usize]())\n\
                 \x20       if loaded[0] != 1:\n\
                 \x20           return [0; 5]\n\
                 \x20       packed = loaded[1]\n\
                 \x20       octet: usize = 0\n\
                 \x20       @budget(bound=4)\n\
                 \x20       while octet < 4:\n\
                 \x20           byte = pixel * 4 + octet\n\
                 \x20           value = packed & 255\n\
                 \x20           h0 = (h0 ^ value) *% 1099511628211\n\
                 \x20           h1 = (h1 ^ (value +% byte.to[u64]())) *% 14029467366897019727\n\
                 \x20           h2 = (h2 +% value) *% 11400714785074694791\n\
                 \x20           h3 = (h3 ^ (value << (byte % 8).to[u64]())) *% 9650029242287828579\n\
                 \x20           packed = packed >> 8\n\
                 \x20           octet = octet + 1\n\
                 \x20       pixel = pixel + 1\n\
                 \x20   return [1, h0, h1, h2, h3]",
                pixel_count,
                pixel_count,
            )
            .expect("String writes cannot fail");
        }

        if full_single_tile {
            writeln!(
                output,
                "\npub fn __wrela_pixels_p8_descriptor_digest_r{index}() -> [u64; 5]:\n\
                 \x20   active = R{index}_SCANOUT_STATE.words[3]\n\
                 \x20   if active == 1:\n\
                 \x20       return [1, {}, {}, {}, {}]\n\
                 \x20   if active == 2:\n\
                 \x20       return [1, {}, {}, {}, {}]\n\
                 \x20   return [0; 5]",
                descriptor_digests[0][0],
                descriptor_digests[0][1],
                descriptor_digests[0][2],
                descriptor_digests[0][3],
                descriptor_digests[1][0],
                descriptor_digests[1][1],
                descriptor_digests[1][2],
                descriptor_digests[1][3],
            )
            .expect("String writes cannot fail");
        } else {
            writeln!(
            output,
            "\npub fn __wrela_pixels_p8_guest_digest_r{index}(kind: u8) -> [u64; 5]:\n\
             \x20   owner = R{index}_SCANOUT_STATE.words[3]\n\
             \x20   if owner == 0 and kind == 0:\n\
             \x20       owner = R{index}_SCANOUT_STATE.words[1]\n\
             \x20   if owner == 0 or kind > 1:\n\
             \x20       return [0; 5]\n\
             \x20   length: usize = {generation_bytes}.to[usize]()\n\
             \x20   base = (owner - 1) * {generation_bytes}\n\
             \x20   if kind == 1:\n\
             \x20       length = {}\n\
             \x20       base = (owner - 1) * {per_generation_list_bytes} + {control_bytes}\n\
             \x20   h0: u64 = 1469598103934665603\n\
             \x20   h1: u64 = 1099511628211\n\
             \x20   h2: u64 = 7809847782465536322\n\
             \x20   h3: u64 = 1609587929392839161\n\
             \x20   byte: usize = 0\n\
             \x20   @budget(bound={})\n\
             \x20   while byte < length:\n\
             \x20       value: u64 = 0\n\
             \x20       if kind == 1:\n\
             \x20           listed = __wrela_pixels_p8_list_load_word_r{index}(base.to[usize]() + (byte / 8) * 8)\n\
             \x20           if listed[0] != 1:\n\
             \x20               return [0; 5]\n\
             \x20           value = (listed[1] >> ((byte % 8).to[u64]() * 8)) & 255\n\
             \x20       else:\n\
             \x20           loaded = __wrela_pixels_p7_framebuffer_load_byte({index}, (base + byte.to[u64]()).to[usize]())\n\
             \x20           if loaded[0] != 1:\n\
             \x20               return [0; 5]\n\
             \x20           value = loaded[1]\n\
             \x20       h0 = (h0 ^ value) *% 1099511628211\n\
             \x20       h1 = (h1 ^ (value +% byte.to[u64]())) *% 14029467366897019727\n\
             \x20       h2 = (h2 +% value) *% 11400714785074694791\n\
             \x20       h3 = (h3 ^ (value << (byte % 8).to[u64]())) *% 9650029242287828579\n\
             \x20       byte = byte + 1\n\
             \x20   offset: usize = 8\n\
             \x20   if kind == 1:\n\
             \x20       offset = 12\n\
             \x20   R{index}_SCANOUT_STATE.words[offset] = h0\n\
             \x20   R{index}_SCANOUT_STATE.words[offset + 1] = h1\n\
             \x20   R{index}_SCANOUT_STATE.words[offset + 2] = h2\n\
             \x20   R{index}_SCANOUT_STATE.words[offset + 3] = h3\n\
             \x20   return [1, h0, h1, h2, h3]\n\
             \n\
             pub fn __wrela_pixels_p8_raw_digest_r{index}() -> [u64; 5]:\n\
             \x20   return __wrela_pixels_p8_guest_digest_r{index}(0)\n\
             \n\
             pub fn __wrela_pixels_p8_descriptor_digest_r{index}() -> [u64; 5]:\n\
             \x20   return __wrela_pixels_p8_guest_digest_r{index}(1)",
            tile_count * 24,
            generation_bytes.max(tile_count * 24),
            )
            .expect("String writes cannot fail");
        }

        let raw_digest_function = if full_single_tile {
            "digest"
        } else {
            "raw_digest"
        };
        // Descriptor construction stays loop-shaped at every resolution.
        // Unrolling one store triplet per tile makes ordinary 1080p images
        // exceed the compiler's own frame limit and scales generated source
        // with display area despite the fixed tile algorithm.
        writeln!(
            output,
            "\npub fn __wrela_pixels_p8_present_r{index}(frame_index: u64) -> [u64; 3]:\n\
             \x20   active = R{index}_SCANOUT_STATE.words[3]\n\
             \x20   if active == 0:\n\
             \x20       return [0; 3]\n\
             \x20   generation = active - 1\n\
             \x20   sequence = R{index}_SCANOUT_STATE.words[2]\n\
             \x20   list_offset = generation.to[usize]() * {per_generation_list_bytes}\n\
             \x20   tiles_addr = {list_base} + generation * {per_generation_list_bytes} + {control_bytes}\n\
             \x20   if (\n\
             \x20       not __wrela_pixels_p8_list_store_word_r{index}(list_offset, {}.to[u64]() | (1.to[u64]() << 32) | (generation << 40) | ({index}.to[u64]() << 48))\n\
             \x20       or not __wrela_pixels_p8_list_store_word_r{index}(list_offset + 8, {width}.to[u64]() | ({height}.to[u64]() << 32))\n\
             \x20       or not __wrela_pixels_p8_list_store_word_r{index}(list_offset + 16, {}.to[u64]() | ({tile_count}.to[u64]() << 32))\n\
             \x20       or not __wrela_pixels_p8_list_store_word_r{index}(list_offset + 24, sequence)\n\
             \x20       or not __wrela_pixels_p8_list_store_word_r{index}(list_offset + 32, tiles_addr)\n\
             \x20       or not __wrela_pixels_p8_list_store_word_r{index}(list_offset + 40, frame_index)\n\
             \x20       or not __wrela_pixels_p8_list_store_word_r{index}(list_offset + 48, frame_index)\n\
             \x20   ):\n\
             \x20       R{index}_SCANOUT_STATE.words[3] = 0\n\
             \x20       return [0, 3, sequence]\n\
             \x20   tile: u64 = 0\n\
             \x20   @budget(bound={tile_count})\n\
             \x20   while tile < {tile_count}:\n\
             \x20       x = (tile % {tile_columns}) * 64\n\
             \x20       y = (tile / {tile_columns}) * 32\n\
             \x20       visible_width: u64 = 64\n\
             \x20       visible_height: u64 = 32\n\
             \x20       if x + visible_width > {width}:\n\
             \x20           visible_width = {width} - x\n\
             \x20       if y + visible_height > {height}:\n\
             \x20           visible_height = {height} - y\n\
             \x20       descriptor = list_offset + {control_bytes} + tile.to[usize]() * 24\n\
             \x20       pixels_addr = {} + generation * {generation_bytes} + tile * 8192\n\
             \x20       if (\n\
             \x20           not __wrela_pixels_p8_list_store_word_r{index}(descriptor, pixels_addr)\n\
             \x20           or not __wrela_pixels_p8_list_store_word_r{index}(descriptor + 8, x | (y << 16) | (visible_width << 32) | (visible_height << 48))\n\
             \x20           or not __wrela_pixels_p8_list_store_word_r{index}(descriptor + 16, 256.to[u64]() | (1.to[u64]() << 16))\n\
             \x20       ):\n\
             \x20           R{index}_SCANOUT_STATE.words[3] = 0\n\
             \x20           return [0, 3, sequence]\n\
             \x20       tile = tile + 1\n\
             \x20   visible = __wrela_pixels_p8_digest_r{index}()\n\
             \x20   raw = __wrela_pixels_p8_{raw_digest_function}_r{index}()\n\
             \x20   descriptors = __wrela_pixels_p8_descriptor_digest_r{index}()\n\
             \x20   if visible[0] != 1 or raw[0] != 1 or descriptors[0] != 1:\n\
             \x20       R{index}_SCANOUT_STATE.words[3] = 0\n\
             \x20       return [0, 3, sequence]\n\
             \x20   digest_word: usize = 0\n\
             \x20   @budget(bound=4)\n\
             \x20   while digest_word < 4:\n\
             \x20       if (\n\
             \x20           not __wrela_pixels_p8_list_store_word_r{index}(list_offset + 56 + digest_word * 8, visible[digest_word + 1])\n\
             \x20           or not __wrela_pixels_p8_list_store_word_r{index}(list_offset + 88 + digest_word * 8, raw[digest_word + 1])\n\
             \x20           or not __wrela_pixels_p8_list_store_word_r{index}(list_offset + 120 + digest_word * 8, descriptors[digest_word + 1])\n\
             \x20       ):\n\
             \x20           R{index}_SCANOUT_STATE.words[3] = 0\n\
             \x20           return [0, 3, sequence]\n\
             \x20       digest_word = digest_word + 1\n\
             \x20   if not __wrela_pixels_p8_list_store_word_r{index}(list_offset + 152, 0):\n\
             \x20       R{index}_SCANOUT_STATE.words[3] = 0\n\
             \x20       return [0, 3, sequence]\n\
             \x20   return [2, {list_base} + generation * {per_generation_list_bytes}, sequence]",
            wrela_machine::pixels::ABI_VERSION,
            renderer.config.refresh_hz,
            placement.framebuffer_base,
        )
        .expect("String writes cannot fail");

        let status_load = if list.bytes <= WORKSPACE_VIEW_CHUNK_BYTES {
            format!(
                "    status = R{index}_DISPLAY_LIST_CHUNK_0.words[generation.to[usize]() * {} + 19] & 4294967295",
                per_generation_list_bytes / 8,
            )
        } else {
            format!(
                "    loaded = __wrela_pixels_p8_list_load_word_r{index}(list_offset + 152)\n\
                 \x20   if loaded[0] != 1:\n\
                 \x20       R{index}_SCANOUT_STATE.words[3] = 0\n\
                 \x20       return [0, 3, sequence]\n\
                 \x20   status = loaded[1] & 4294967295"
            )
        };
        writeln!(
            output,
            "\npub fn __wrela_pixels_p8_complete_r{index}() -> [u64; 3]:\n\
             \x20   active = R{index}_SCANOUT_STATE.words[3]\n\
             \x20   if active == 0:\n\
             \x20       return [0; 3]\n\
             \x20   generation = active - 1\n\
             \x20   sequence = R{index}_SCANOUT_STATE.words[2]\n\
             \x20   list_offset = generation.to[usize]() * {per_generation_list_bytes}\n\
             {status_load}\n\
             \x20   if status != 1:\n\
             \x20       R{index}_SCANOUT_STATE.words[3] = 0\n\
             \x20       return [0, status, sequence]\n\
             \x20   R{index}_SCANOUT_STATE.words[1] = active\n\
             \x20   R{index}_SCANOUT_STATE.words[2] = sequence + 1\n\
             \x20   R{index}_SCANOUT_STATE.words[3] = 0\n\
             \x20   return [1, sequence, 0]"
        )
        .expect("String writes cannot fail");
    }

    output.push_str("\npub fn __wrela_pixels_p8_initialize(renderer: usize) -> bool:\n");
    for placement in placements {
        writeln!(
            output,
            "    if renderer == {}:\n\x20       return __wrela_pixels_p8_initialize_r{}()",
            placement.index, placement.index,
        )
        .expect("String writes cannot fail");
    }
    output.push_str("    return false\n");
    output.push_str("\npub fn __wrela_pixels_p8_begin(renderer: usize) -> [u64; 3]:\n");
    for placement in placements {
        writeln!(
            output,
            "    if renderer == {}:\n\x20       return __wrela_pixels_p8_begin_r{}()",
            placement.index, placement.index,
        )
        .expect("String writes cannot fail");
    }
    output.push_str("    return [0; 3]\n");
    output.push_str("\npub fn __wrela_pixels_p8_complete(renderer: usize) -> [u64; 3]:\n");
    for placement in placements {
        writeln!(
            output,
            "    if renderer == {}:\n\x20       return __wrela_pixels_p8_complete_r{}()",
            placement.index, placement.index,
        )
        .expect("String writes cannot fail");
    }
    output.push_str("    return [0; 3]\n");
    output.push_str("\npub fn __wrela_pixels_p8_cancel(renderer: usize) -> bool:\n");
    for placement in placements {
        writeln!(
            output,
            "    if renderer == {}:\n\x20       return __wrela_pixels_p8_cancel_r{}()",
            placement.index, placement.index,
        )
        .expect("String writes cannot fail");
    }
    output.push_str("    return false\n");
    if placements.len() != 1 {
        output.push_str("\npub fn __wrela_pixels_p8_write(renderer: usize, worker: u32, pixel: u32, front: u32, coverage: u8, back: u32) -> bool:\n");
        for placement in placements {
            writeln!(
                output,
                "    if renderer == {}:\n\x20       return __wrela_pixels_p8_write_r{}(renderer, worker, pixel, front, coverage, back)",
                placement.index, placement.index,
            )
            .expect("String writes cannot fail");
        }
        output.push_str("    return false\n");
        output.push_str("\npub fn __wrela_pixels_p8_write4(renderer: usize, worker: u32, pixel: u32, r: u8, g: u8, b: u8) -> bool:\n");
        for placement in placements {
            writeln!(
                output,
                "    if renderer == {}:\n\x20       return __wrela_pixels_p8_write4_r{}(renderer, worker, pixel, r, g, b)",
                placement.index, placement.index,
            )
            .expect("String writes cannot fail");
        }
        output.push_str("    return false\n");
    }
    output.push_str(
        "\npub fn __wrela_pixels_p8_visible_byte(renderer: usize, byte: usize) -> [u64; 2]:\n",
    );
    for placement in placements {
        writeln!(
            output,
            "    if renderer == {}:\n\x20       return __wrela_pixels_p8_visible_byte_r{}(byte)",
            placement.index, placement.index,
        )
        .expect("String writes cannot fail");
    }
    output.push_str("    return [0; 2]\n");
    output.push_str(
        "\npub fn __wrela_pixels_p8_visible_word(renderer: usize, word: usize) -> [u64; 2]:\n",
    );
    for placement in placements {
        writeln!(
            output,
            "    if renderer == {}:\n\x20       return __wrela_pixels_p8_visible_word_r{}(word)",
            placement.index, placement.index,
        )
        .expect("String writes cannot fail");
    }
    output.push_str("    return [0; 2]\n");
    output.push_str("\npub fn __wrela_pixels_p8_digest(renderer: usize) -> [u64; 5]:\n");
    for placement in placements {
        writeln!(
            output,
            "    if renderer == {}:\n\x20       return __wrela_pixels_p8_digest_r{}()",
            placement.index, placement.index,
        )
        .expect("String writes cannot fail");
    }
    output.push_str("    return [0; 5]\n");
    output.push_str("\npub fn __wrela_pixels_p8_raw_digest(renderer: usize) -> [u64; 5]:\n");
    for (placement, renderer) in placements.iter().zip(compiled) {
        let raw_function = if renderer.config.width == wrela_machine::pixels::TILE_WIDTH
            && renderer.config.height == wrela_machine::pixels::TILE_HEIGHT
        {
            "digest"
        } else {
            "raw_digest"
        };
        writeln!(
            output,
            "    if renderer == {}:\n\x20       return __wrela_pixels_p8_{raw_function}_r{}()",
            placement.index, placement.index,
        )
        .expect("String writes cannot fail");
    }
    output.push_str("    return [0; 5]\n");
    output.push_str("\npub fn __wrela_pixels_p8_descriptor_digest(renderer: usize) -> [u64; 5]:\n");
    for placement in placements {
        writeln!(
            output,
            "    if renderer == {}:\n\x20       return __wrela_pixels_p8_descriptor_digest_r{}()",
            placement.index, placement.index,
        )
        .expect("String writes cannot fail");
    }
    output.push_str("    return [0; 5]\n");
    output.push_str(
        "\npub fn __wrela_pixels_p8_present(renderer: usize, frame_index: u64) -> [u64; 3]:\n",
    );
    for placement in placements {
        writeln!(
            output,
            "    if renderer == {}:\n\x20       return __wrela_pixels_p8_present_r{}(frame_index)",
            placement.index, placement.index,
        )
        .expect("String writes cannot fail");
    }
    output.push_str("    return [0; 3]\n");
    output.push_str("\npub fn __wrela_pixels_p8_display_doorbell(renderer: usize) -> [u64; 2]:\n");
    for (placement, renderer) in placements.iter().zip(compiled) {
        writeln!(
            output,
            "    if renderer == {}:\n\x20       return [1, {}]",
            placement.index, renderer.config.display_doorbell_addr,
        )
        .expect("String writes cannot fail");
    }
    output.push_str(
        "    return [0; 2]\n\
         \n\
         pub fn __wrela_pixels_p8_submit(doorbell_addr: u64, control_addr: u64) -> unit:\n\
         \x20   return unit\n",
    );
    Ok(())
}

pub fn configuration_source(
    placements: &[crate::layout::RendererPlacement],
    compiled: &[super::CompiledRenderer],
    instrumented: bool,
) -> Result<String, String> {
    if placements.len() != compiled.len() {
        return Err("pixels::glue: placement/program count differs".to_string());
    }
    let mut output = String::from(
        "module __image_pixels\n\n\
         from core.field import cos_scalar, rsqrt_scalar, sin_scalar, sqrt_scalar\n\n\
         from core.render_interval import FixedDomain, Iv32, interval_add, interval_from_f32_bits, interval_mul\n\n\
         from core.render_arrangement import __wrela_pixels_p8r_clip_handler, __wrela_pixels_p8r_deformation_handler, __wrela_pixels_p8r_dispatch_handler, __wrela_pixels_p8r_polynomial_handler, __wrela_pixels_p8r_predicate_handler, __wrela_pixels_p8r_smooth_band_handler, __wrela_pixels_p8r_torus_handler\n\n\
         from core.render_raster import AffineRunSetup, EventId, EventPixel, I32x4, IdSlice, IdentitySetId, LightSummaryId, MaterialSummaryId, OutputProofCode, QRunScalar, RasterGeometryLane, RasterRun, RunId, pixels_i32x4_backend_add, raster_geometry_lane_valid, raster_run4\n\n\
         from core.render_raster import F32x4, __pixels_f32_exponent, __pixels_f32_mantissa, __pixels_u128_add, __pixels_u128_bit, __pixels_u128_compare, __pixels_u128_from_u64, __pixels_u128_is_zero, __pixels_u128_lower_bits_nonzero, __pixels_u128_round_shift_even, __pixels_u128_scale_to, __pixels_u128_shift_left, __pixels_u128_shift_right, __pixels_u128_shift_right_jam, __pixels_u128_sub, __pixels_u128_top_bit, __pixels_u128_zero, pixels_f32_fma_bits_fallback, pixels_f32_fma_scalar, pixels_f32_from_bits, pixels_f32_max_scalar, pixels_f32_min_scalar, pixels_f32_select_ge_scalar, pixels_f32_select_gt_scalar, pixels_f32_to_bits, pixels_f32_to_i32_scalar, pixels_f32x4_backend_add, pixels_f32x4_backend_fma, pixels_f32x4_backend_max, pixels_f32x4_backend_min, pixels_f32x4_backend_mul, pixels_f32x4_backend_select_ge, pixels_f32x4_backend_select_gt, pixels_f32x4_backend_splat, pixels_f32x4_backend_sub, pixels_f32x4_backend_to_i32x4, pixels_i32_select_gt_scalar, pixels_i32x4_backend_and, pixels_i32x4_backend_or, pixels_i32x4_backend_select_gt, pixels_i32x4_backend_shr_arith_imm, pixels_i32x4_backend_splat, pixels_i32x4_backend_sub, pixels_i32x4_backend_to_f32x4\n\n\
         from core.render_certificate import certify_quadratic_discriminant\n\n\
         from core.render_program import polynomial_horner9\n\n\
         # These declarations are emitted mechanically from core.render_program,\n\
         # the single canonical Wrela wire-view schema. Keeping them local lets\n\
         # ordinary @layout nesting type the exact generated placed roots.\n",
    );
    output.push_str(&canonical_wire_view_source()?);
    let debug_visibility = DEBUG_VISIBILITY.load(Ordering::Relaxed);
    writeln!(
        output,
        "\npub fn __wrela_pixels_p9_debug_visibility() -> bool:\n    return {debug_visibility}"
    )
    .expect("String writes cannot fail");
    output.push_str(
        "\n\
         # These generated-only helpers are lowered as representation-preserving\n\
         # copies. Their source bodies keep ordinary checking deterministic.\n\
         pub fn __wrela_pixels_f32_to_bits(value: f32) -> u32:\n\
         \x20   return value.to[u32]()\n\
         \n\
         pub fn __wrela_pixels_f32_from_bits(bits: u32) -> f32:\n\
         \x20   return bits.to[f32]()\n\
         \n\
         pub fn __wrela_pixels_f64_bits_to_f32(bits: u64) -> f32:\n\
         \x20   return bits.to[f32]()\n\
         \n\
         # Guest-executed P8R packet substrate regression. Every sealed packet\n\
         # operation is driven across all four lanes and compared by exact bits.\n\
         pub fn __wrela_pixels_p8r_packet_selftest() -> bool:\n\
         \x20   ia = I32x4.from_lanes([2147483647, -2147483648, -7, 12])\n\
         \x20   ib = I32x4.from_lanes([1, 1, 3, -4])\n\
         \x20   isplat = pixels_i32x4_backend_splat(-5)\n\
         \x20   if isplat.lane(0) != -5 or isplat.lane(1) != -5 or isplat.lane(2) != -5 or isplat.lane(3) != -5:\n\
         \x20       return false\n\
         \x20   iadd = pixels_i32x4_backend_add(ia, ib)\n\
         \x20   if iadd.lane(0) != -2147483648 or iadd.lane(1) != -2147483647 or iadd.lane(2) != -4 or iadd.lane(3) != 8:\n\
         \x20       return false\n\
         \x20   isub = pixels_i32x4_backend_sub(ia, ib)\n\
         \x20   if isub.lane(0) != 2147483646 or isub.lane(1) != 2147483647 or isub.lane(2) != -10 or isub.lane(3) != 16:\n\
         \x20       return false\n\
         \x20   ishr = pixels_i32x4_backend_shr_arith_imm(ia, 3)\n\
         \x20   if ishr.lane(0) != 268435455 or ishr.lane(1) != -268435456 or ishr.lane(2) != -1 or ishr.lane(3) != 1:\n\
         \x20       return false\n\
         \x20   iand = pixels_i32x4_backend_and(ia, I32x4.from_lanes([255; 4]))\n\
         \x20   if iand.lane(0) != 255 or iand.lane(1) != 0 or iand.lane(2) != 249 or iand.lane(3) != 12:\n\
         \x20       return false\n\
         \x20   ior = pixels_i32x4_backend_or(iand, I32x4.from_lanes([256; 4]))\n\
         \x20   if ior.lane(0) != 511 or ior.lane(1) != 256 or ior.lane(2) != 505 or ior.lane(3) != 268:\n\
         \x20       return false\n\
         \x20   isel = pixels_i32x4_backend_select_gt(ia, ib, pixels_i32x4_backend_splat(7), pixels_i32x4_backend_splat(9))\n\
         \x20   if isel.lane(0) != 7 or isel.lane(1) != 9 or isel.lane(2) != 9 or isel.lane(3) != 7:\n\
         \x20       return false\n\
         \x20   itof = pixels_i32x4_backend_to_f32x4(I32x4.from_lanes([0, -1, 16777217, 2147483647]))\n\
         \x20   if itof.lane_bits(0) != 0 or itof.lane_bits(1) != 3212836864 or itof.lane_bits(2) != 1266679808 or itof.lane_bits(3) != 1325400064:\n\
         \x20       return false\n\
         \x20   fa = F32x4.from_lanes([1.0, -2.0, 3.0, 4.0])\n\
         \x20   fb = F32x4.from_lanes([0.5, 4.0, -1.0, 4.0])\n\
         \x20   fsplat = pixels_f32x4_backend_splat(-3.5)\n\
         \x20   if fsplat.lane_bits(0) != 3227516928 or fsplat.lane_bits(1) != 3227516928 or fsplat.lane_bits(2) != 3227516928 or fsplat.lane_bits(3) != 3227516928:\n\
         \x20       return false\n\
         \x20   fadd = pixels_f32x4_backend_add(fa, fb)\n\
         \x20   if fadd.lane_bits(0) != 1069547520 or fadd.lane_bits(1) != 1073741824 or fadd.lane_bits(2) != 1073741824 or fadd.lane_bits(3) != 1090519040:\n\
         \x20       return false\n\
         \x20   fsub = pixels_f32x4_backend_sub(fa, fb)\n\
         \x20   if fsub.lane_bits(0) != 1056964608 or fsub.lane_bits(1) != 3233808384 or fsub.lane_bits(2) != 1082130432 or fsub.lane_bits(3) != 0:\n\
         \x20       return false\n\
         \x20   fmul = pixels_f32x4_backend_mul(fa, fb)\n\
         \x20   if fmul.lane_bits(0) != 1056964608 or fmul.lane_bits(1) != 3238002688 or fmul.lane_bits(2) != 3225419776 or fmul.lane_bits(3) != 1098907648:\n\
         \x20       return false\n\
         \x20   zeros = F32x4.from_bits([2147483648, 0, 2143289344, 1073741824])\n\
         \x20   zero_rhs = F32x4.from_bits([0, 2147483648, 1065353216, 2143289344])\n\
         \x20   fmin = pixels_f32x4_backend_min(zeros, zero_rhs)\n\
         \x20   if fmin.lane_bits(0) != 2147483648 or fmin.lane_bits(1) != 2147483648 or fmin.lane_bits(2) != 2143289344 or fmin.lane_bits(3) != 2143289344:\n\
         \x20       return false\n\
         \x20   fmax = pixels_f32x4_backend_max(zeros, zero_rhs)\n\
         \x20   if fmax.lane_bits(0) != 0 or fmax.lane_bits(1) != 0 or fmax.lane_bits(2) != 2143289344 or fmax.lane_bits(3) != 2143289344:\n\
         \x20       return false\n\
         \x20   cmp_lhs = F32x4.from_bits([1065353216, 1073741824, 2143289344, 1082130432])\n\
         \x20   cmp_rhs = F32x4.from_lanes([1.0, 3.0, 0.0, 3.0])\n\
         \x20   when_true = F32x4.from_lanes([10.0, 11.0, 12.0, 13.0])\n\
         \x20   when_false = F32x4.from_lanes([20.0, 21.0, 22.0, 23.0])\n\
         \x20   fge = pixels_f32x4_backend_select_ge(cmp_lhs, cmp_rhs, when_true, when_false)\n\
         \x20   if fge.lane_bits(0) != 1092616192 or fge.lane_bits(1) != 1101529088 or fge.lane_bits(2) != 1102053376 or fge.lane_bits(3) != 1095761920:\n\
         \x20       return false\n\
         \x20   fgt = pixels_f32x4_backend_select_gt(cmp_lhs, cmp_rhs, when_true, when_false)\n\
         \x20   if fgt.lane_bits(0) != 1101004800 or fgt.lane_bits(1) != 1101529088 or fgt.lane_bits(2) != 1102053376 or fgt.lane_bits(3) != 1095761920:\n\
         \x20       return false\n\
         \x20   fused = pixels_f32x4_backend_fma(F32x4.from_bits([1065353217, 8388608, 2139095039, 2147483648]), F32x4.from_bits([1065353215, 1056964608, 1065353217, 1065353216]), F32x4.from_bits([3212836864, 1, 4286578687, 2147483648]))\n\
         \x20   if fused.lane_bits(0) != 864026622 or fused.lane_bits(1) != 4194305 or fused.lane_bits(2) != 1946157055 or fused.lane_bits(3) != 2147483648:\n\
         \x20       return false\n\
         \x20   nan_fused = pixels_f32x4_backend_fma(F32x4.from_bits([2143363909, 2139095041, 1065353216, 1065353216]), F32x4.from_bits([1065353216, 1065353216, 2143363909, 1065353216]), F32x4.from_bits([0, 0, 0, 2139095041]))\n\
         \x20   if nan_fused.lane_bits(0) != 2143289344 or nan_fused.lane_bits(1) != 2143289344 or nan_fused.lane_bits(2) != 2143289344 or nan_fused.lane_bits(3) != 2143289344:\n\
         \x20       return false\n\
         \x20   ftoi = pixels_f32x4_backend_to_i32x4(F32x4.from_bits([2139095040, 4286578688, 2143289344, 1325400063]))\n\
         \x20   if ftoi.lane(0) != 2147483647 or ftoi.lane(1) != -2147483648 or ftoi.lane(2) != 0 or ftoi.lane(3) != 2147483520:\n\
         \x20       return false\n\
         \x20   return true\n",
    );
    writeln!(
        output,
        "\npub const N_RENDERERS: usize = {}",
        placements.len()
    )
    .expect("String writes cannot fail");
    let worker_count = placements
        .first()
        .map_or(0, |placement| placement.per_core.len());
    if placements
        .iter()
        .any(|placement| placement.per_core.len() != worker_count)
    {
        return Err("pixels::glue: renderer worker counts differ within one image".to_string());
    }
    writeln!(output, "pub const N_RENDER_WORKERS: usize = {worker_count}")
        .expect("String writes cannot fail");
    for (placement, renderer) in placements.iter().zip(compiled) {
        let index = placement.index;
        let capacities = &renderer.structural.program().capacities;
        let projective = &renderer.projective.program().capacities;
        output.push('\n');
        write_renderer_constants(
            &mut output,
            index,
            &[
                ("FRAMEPROG_BASE", placement.frameprog_base),
                ("FRAMEPROG_BYTES", placement.frameprog_size),
                ("STATE_BASE", placement.state_base),
                ("STATE_BYTES", placement.state_size),
                ("WIDTH", u64::from(renderer.config.width)),
                ("HEIGHT", u64::from(renderer.config.height)),
                (
                    "DISPLAY_INDEX",
                    u64::try_from(renderer.config.display_index)
                        .map_err(|_| "P025: display index exceeds u64".to_string())?,
                ),
                (
                    "DISPLAY_DOORBELL_ADDR",
                    renderer.config.display_doorbell_addr,
                ),
                ("REFRESH_HZ", u64::from(renderer.config.refresh_hz)),
                ("SHADE_HZ", u64::from(renderer.config.shade_hz)),
                ("LIGHT_CAPACITY", u64::from(renderer.config.light_capacity)),
                (
                    "PROBE_INITIALIZATION_WORST_CASE_MS",
                    u64::from(renderer.config.probe_initialization_worst_case_ms),
                ),
                (
                    "INITIALIZATION_DEADLINE_MS",
                    u64::from(renderer.config.initialization_deadline_ms),
                ),
                ("TILE_W", u64::from(TILE_WIDTH_V1)),
                ("TILE_H", u64::from(TILE_HEIGHT_V1)),
                ("WORKERS", u64::from(capacities.worker_count)),
                ("OBJECTS", u64::from(capacities.object_count)),
                (
                    "FEATURE_TEMPLATES",
                    u64::from(capacities.feature_template_count),
                ),
                ("FEATURES", u64::from(capacities.feature_count)),
                (
                    "REPEATED_INSTANCES",
                    u64::from(capacities.repeated_instance_count),
                ),
                (
                    "SCALAR_PROGRAM_SLOTS",
                    u64::from(capacities.scalar_program_slots),
                ),
                (
                    "DERIVATIVE_PROGRAM_SLOTS",
                    u64::from(capacities.derivative_program_slots),
                ),
                ("PARAMETER_SLOTS", u64::from(capacities.parameter_slots)),
                ("CSG_STACK", u64::from(capacities.max_csg_stack)),
                (
                    "MAX_PROJECTED_FEATURES_ROW",
                    u64::from(capacities.max_projected_features_per_row),
                ),
                (
                    "MAX_PROJECTED_FEATURES_TILE",
                    u64::from(capacities.max_projected_features_per_tile),
                ),
                (
                    "MAX_OBJECT_ROOTS_ROW_START",
                    u64::from(capacities.max_object_roots_per_row_start),
                ),
                (
                    "MAX_ACTIVE_SHEETS_ROW",
                    u64::from(capacities.max_active_sheet_records_per_row),
                ),
                (
                    "STRUCTURAL_EVENT_GENERATORS",
                    u64::from(capacities.event_generator_count),
                ),
                (
                    "MAX_EVENT_SUBDIVISIONS",
                    u64::from(capacities.max_event_subdivisions),
                ),
                ("MAX_EVENT_RECORDS", u64::from(capacities.max_event_records)),
                (
                    "MAX_RUN_RECORDS_TILE_ROW",
                    u64::from(capacities.max_run_records_per_tile_row),
                ),
                (
                    "MAX_CSG_EVENTS_ROW",
                    u64::from(capacities.max_csg_events_per_row),
                ),
                (
                    "MAX_TRANSPARENT_LAYERS",
                    u64::from(capacities.max_transparent_layers),
                ),
                (
                    "MAX_LOCAL_REBUILD_QUEUE",
                    u64::from(capacities.max_local_rebuild_queue),
                ),
                ("CANDIDATE_STORAGE_BYTES", capacities.candidate_bytes),
                ("ROOT_STORAGE_BYTES", capacities.root_bytes),
                ("SHEET_STORAGE_BYTES", capacities.sheet_bytes),
                ("EVENT_STORAGE_BYTES", capacities.event_bytes),
                ("RUN_STORAGE_BYTES", capacities.run_bytes),
                ("CORRIDOR_STORAGE_BYTES", capacities.corridor_bytes),
                ("FIXED_Q_STORAGE_BYTES", capacities.fixed_q_bytes),
                ("SHADING_STORAGE_BYTES", capacities.shading_bytes),
                ("TRANSPARENCY_STORAGE_BYTES", capacities.transparency_bytes),
                (
                    "STRUCTURAL_PER_WORKER_SCRATCH_BYTES",
                    capacities.per_worker_scratch_bytes,
                ),
                (
                    "STRUCTURAL_ALL_WORKER_SCRATCH_BYTES",
                    capacities.all_worker_scratch_bytes,
                ),
                (
                    "TELEMETRY_PRODUCTION_BYTES",
                    capacities.telemetry_bytes_production,
                ),
                (
                    "TELEMETRY_INSTRUMENTED_BYTES",
                    capacities.telemetry_bytes_instrumented,
                ),
                ("OUTPUT_TILE_BYTES", capacities.output_tile_bytes),
                (
                    "OUTPUT_DOUBLE_BUFFER_BYTES",
                    capacities.output_double_buffer_bytes,
                ),
                ("PROBE_STATE_BYTES", capacities.probe_bytes),
                (
                    "KINETIC_CERTIFICATE_BYTES",
                    capacities.kinetic_certificate_bytes,
                ),
                ("STATE_HEADER_CAPACITY_BYTES", capacities.state_header_bytes),
                (
                    "COEFFICIENT_SNAPSHOT_CAPACITY_BYTES",
                    capacities.coefficient_snapshot_bytes,
                ),
                (
                    "FRAME_SNAPSHOT_CAPACITY_BYTES",
                    capacities.frame_dependency_snapshot_bytes,
                ),
                (
                    "FRAME_COMPLEX_CAPACITY_BYTES",
                    capacities.frame_complex_double_buffer_bytes,
                ),
                (
                    "TILE_DESCRIPTOR_CAPACITY_BYTES",
                    capacities.tile_descriptor_bytes,
                ),
                (
                    "TILE_OWNERSHIP_CAPACITY_BYTES",
                    capacities.tile_ownership_bytes,
                ),
                (
                    "FAILURE_RECORD_CAPACITY_BYTES",
                    capacities.failure_record_bytes,
                ),
                (
                    "PRODUCTION_STATE_BYTES",
                    projective.total_renderer_state_bytes,
                ),
                (
                    "INSTRUMENTED_STATE_BYTES",
                    projective.total_renderer_state_bytes_instrumented,
                ),
                (
                    "CANDIDATE_FEATURES_TILE",
                    u64::from(projective.candidate_features_per_tile),
                ),
                ("ROW_START_ROOTS", u64::from(projective.row_start_roots)),
                (
                    "ACTIVE_SHEETS_ROW",
                    u64::from(projective.active_sheets_per_row),
                ),
                ("EVENT_GENERATORS", u64::from(projective.event_generators)),
                (
                    "COMPETITION_PAIRS_TILE",
                    u64::from(projective.competition_pairs_per_tile),
                ),
                (
                    "ROW_EVENT_INTERVALS",
                    u64::from(projective.row_event_intervals),
                ),
                ("ROOT_STACK_NODES", u64::from(projective.root_stack_nodes)),
                ("EVENT_STACK_NODES", u64::from(projective.event_stack_nodes)),
                ("RUNS_PER_ROW", u64::from(projective.runs_per_row)),
                ("CORRIDORS_PER_ROW", u64::from(projective.corridors_per_row)),
                ("MAX_INDEX_SLICE", u64::from(projective.max_index_slice)),
                (
                    "POLYNOMIAL_PROGRAMS",
                    u64::from(projective.polynomial_programs),
                ),
                ("RATIONAL_PROGRAMS", u64::from(projective.rational_programs)),
                (
                    "POLYNOMIAL_TERMS_PROGRAM",
                    u64::from(projective.polynomial_terms_per_program),
                ),
                ("COEFFICIENT_NODES", u64::from(projective.coefficient_nodes)),
                (
                    "DERIVATIVE_BUNDLES",
                    u64::from(projective.derivative_bundles),
                ),
                (
                    "DERIVATIVE_CLUSTERS",
                    u64::from(projective.derivative_clusters),
                ),
                ("INDEX_BYTES", projective.index_bytes),
                (
                    "PROJECTIVE_PER_WORKER_SCRATCH_BYTES",
                    projective.final_per_worker_scratch_bytes,
                ),
                (
                    "PROJECTIVE_ALL_WORKER_SCRATCH_BYTES",
                    projective.final_all_worker_scratch_bytes,
                ),
                (
                    "CERT_TELEMETRY_BYTES",
                    if instrumented {
                        capacities.telemetry_bytes_instrumented
                    } else {
                        0
                    },
                ),
            ],
        );
        for (name, offset, bytes) in projective.worker_workspace_regions()? {
            writeln!(
                output,
                "const R{index}_WORKSPACE_{name}_OFFSET: usize = {offset}\n\
                 const R{index}_WORKSPACE_{name}_BYTES: usize = {bytes}"
            )
            .expect("String writes cannot fail");
        }
        let state = &renderer.mutable_layout;
        for region in [
            state_region_constants(
                placement.state_base,
                "STATE_HEADER_BASE",
                "STATE_HEADER_BYTES",
                state.header,
                None,
            )?,
            state_region_constants(
                placement.state_base,
                "COEFFICIENT_SNAPSHOTS_BASE",
                "COEFFICIENT_SNAPSHOTS_BYTES",
                state.coefficient_snapshots,
                None,
            )?,
            state_region_constants(
                placement.state_base,
                "FRAME_SNAPSHOTS_BASE",
                "FRAME_SNAPSHOTS_BYTES",
                state.frame_snapshots,
                None,
            )?,
            state_region_constants(
                placement.state_base,
                "FRAME_COMPLEXES_BASE",
                "FRAME_COMPLEXES_BYTES",
                state.frame_complexes,
                None,
            )?,
            state_region_constants(
                placement.state_base,
                "WORKER_SCRATCH_BASE",
                "WORKER_SCRATCH_BYTES",
                state.worker_scratch,
                None,
            )?,
            state_region_constants(
                placement.state_base,
                "FRAMEBUFFERS_BASE",
                "FRAMEBUFFERS_BYTES",
                state.framebuffers,
                None,
            )?,
            state_region_constants(
                placement.state_base,
                "PROBES_BASE",
                "PROBES_BYTES",
                state.probes,
                None,
            )?,
            state_region_constants(
                placement.state_base,
                "KINETIC_STATE_BASE",
                "KINETIC_STATE_BYTES",
                state.kinetic,
                None,
            )?,
            state_region_constants(
                placement.state_base,
                "TILE_DESCRIPTORS_BASE",
                "TILE_DESCRIPTORS_BYTES",
                state.tile_descriptors,
                None,
            )?,
            state_region_constants(
                placement.state_base,
                "TILE_OWNERSHIP_BASE",
                "TILE_OWNERSHIP_BYTES",
                state.tile_ownership,
                None,
            )?,
            state_region_constants(
                placement.state_base,
                "FAILURE_BASE",
                "FAILURE_BYTES",
                state.failure,
                None,
            )?,
            state_region_constants(
                placement.state_base,
                "TELEMETRY_BASE",
                "TELEMETRY_BYTES",
                state.telemetry,
                Some(if instrumented {
                    state.telemetry.bytes
                } else {
                    0
                }),
            )?,
        ] {
            write_renderer_constants(&mut output, index, &region);
        }
        write_renderer_constants(
            &mut output,
            index,
            &[
                ("FRAMEBUFFER_BASE", placement.framebuffer_base),
                ("FRAMEBUFFER_BYTES", placement.framebuffer_bytes),
                ("PROBE_RESERVATION_BASE", placement.probe_base),
                ("PROBE_RESERVATION_BYTES", placement.probe_bytes),
            ],
        );
        if placement.framebuffer_bytes % 8 != 0 {
            return Err(format!(
                "pixels::glue: renderer {index} debug framebuffer is not word aligned"
            ));
        }
        let mut framebuffer_offset = 0_u64;
        let mut framebuffer_chunk = 0_usize;
        while framebuffer_offset < placement.framebuffer_bytes {
            let chunk_bytes =
                (placement.framebuffer_bytes - framebuffer_offset).min(WORKSPACE_VIEW_CHUNK_BYTES);
            writeln!(
                output,
                "@layout(runtime, endian=little)\n\
                 struct R{index}DebugFramebufferChunk{framebuffer_chunk}View:\n\
                 \x20   words: [u64; {}]\n\
                 @placed({:#x})\n\
                 static R{index}_DEBUG_FRAMEBUFFER_CHUNK_{framebuffer_chunk}: R{index}DebugFramebufferChunk{framebuffer_chunk}View",
                chunk_bytes / 8,
                placement
                    .framebuffer_base
                    .checked_add(framebuffer_offset)
                    .ok_or_else(|| "P025: framebuffer chunk address overflow".to_string())?,
            )
            .expect("String writes cannot fail");
            framebuffer_offset = framebuffer_offset
                .checked_add(chunk_bytes)
                .ok_or_else(|| "P025: framebuffer chunk offset overflow".to_string())?;
            framebuffer_chunk = framebuffer_chunk
                .checked_add(1)
                .ok_or_else(|| "P015: framebuffer chunk count overflow".to_string())?;
        }
        let state_header_base = placement
            .state_base
            .checked_add(state.header.offset)
            .ok_or_else(|| "P025: P8 state-header address overflow".to_string())?;
        if state.header.bytes != super::capacities::RENDERER_STATE_HEADER_BYTES_V1 {
            return Err(format!(
                "pixels::glue: renderer {index} has a noncanonical state header"
            ));
        }
        writeln!(
            output,
            "@layout(runtime, endian=little)\n\
             struct R{index}ScanoutStateView:\n\
             \x20   words: [u64; {}]\n\
             @placed({state_header_base:#x})\n\
             static R{index}_SCANOUT_STATE: R{index}ScanoutStateView",
            state.header.bytes / 8,
        )
        .expect("String writes cannot fail");
        if state.tile_descriptors.bytes % 8 != 0 {
            return Err(format!(
                "pixels::glue: renderer {index} display-list storage is not word aligned"
            ));
        }
        let display_list_base = placement
            .state_base
            .checked_add(state.tile_descriptors.offset)
            .ok_or_else(|| "P025: P8 display-list address overflow".to_string())?;
        let mut list_offset = 0_u64;
        let mut list_chunk = 0_usize;
        while list_offset < state.tile_descriptors.bytes {
            let chunk_bytes =
                (state.tile_descriptors.bytes - list_offset).min(WORKSPACE_VIEW_CHUNK_BYTES);
            writeln!(
                output,
                "@layout(runtime, endian=little)\n\
                 struct R{index}DisplayListChunk{list_chunk}View:\n\
                 \x20   words: [u64; {}]\n\
                 @placed({:#x})\n\
                 static R{index}_DISPLAY_LIST_CHUNK_{list_chunk}: R{index}DisplayListChunk{list_chunk}View",
                chunk_bytes / 8,
                display_list_base
                    .checked_add(list_offset)
                    .ok_or_else(|| "P025: P8 display-list chunk address overflow".to_string())?,
            )
            .expect("String writes cannot fail");
            list_offset = list_offset
                .checked_add(chunk_bytes)
                .ok_or_else(|| "P025: P8 display-list chunk offset overflow".to_string())?;
            list_chunk = list_chunk
                .checked_add(1)
                .ok_or_else(|| "P015: P8 display-list chunk count overflow".to_string())?;
        }
        if state.frame_snapshots.bytes < 688 {
            return Err(format!(
                "pixels::glue: renderer {index} frame snapshot region is smaller than the P7 visibility snapshot"
            ));
        }
        let snapshot_base = placement
            .state_base
            .checked_add(state.frame_snapshots.offset)
            .ok_or_else(|| "P025: P7 frame snapshot base overflow".to_string())?;
        writeln!(
            output,
            "@layout(runtime, endian=little)\n\
             struct R{index}FrameSnapshotView:\n\
             \x20   bits: [u32; 152]\n\
             \x20   meta: [u64; 10]\n\
             @placed({snapshot_base:#x})\n\
             static R{index}_FRAME_SNAPSHOT: R{index}FrameSnapshotView",
        )
        .expect("String writes cannot fail");
        for worker in &placement.per_core {
            let worker_index = worker.worker_index;
            writeln!(
                output,
                "const R{index}_WORKER_{worker_index}_CORE: usize = {}\n\
                 const R{index}_WORKER_{worker_index}_TILES_START: usize = {}\n\
                 const R{index}_WORKER_{worker_index}_TILES_END: usize = {}\n\
                 const R{index}_WORKER_{worker_index}_WORKSPACE_BASE: usize = {}\n\
                 const R{index}_WORKER_{worker_index}_WORKSPACE_BYTES: usize = {}",
                worker.core,
                worker.tiles_start,
                worker.tiles_end,
                worker.workspace_base,
                worker.workspace_bytes,
            )
            .expect("String writes cannot fail");
            writeln!(
                output,
                "@layout(runtime, endian=little)\n\
                 struct R{index}Worker{worker_index}WorkspaceHeaderView:\n\
                 \x20   words: [u64; 8]\n\
                 @placed({:#x})\n\
                 static R{index}_WORKER_{worker_index}_WORKSPACE_HEADER: R{index}Worker{worker_index}WorkspaceHeaderView",
                worker.workspace_base,
            )
            .expect("String writes cannot fail");
            // A legal renderer may need more than the compiler's 16 MiB
            // single-`@layout` declaration ceiling. Keep the exact region
            // boundaries visible without pretending one enormous byte array
            // is an ordinary runtime struct.
            for (name, offset, bytes) in projective.worker_workspace_regions()? {
                if name == "HEADER" {
                    continue;
                }
                let mut chunk_offset = 0_u64;
                let mut chunk = 0_usize;
                while chunk_offset < bytes {
                    let chunk_bytes = (bytes - chunk_offset).min(WORKSPACE_VIEW_CHUNK_BYTES);
                    writeln!(
                        output,
                        "@layout(runtime, endian=little)\n\
                         struct R{index}Worker{worker_index}Workspace{name}Chunk{chunk}View:"
                    )
                    .expect("String writes cannot fail");
                    let words = chunk_bytes / 8;
                    let tail = chunk_bytes % 8;
                    if words != 0 {
                        writeln!(output, "    words: [u64; {words}]")
                            .expect("String writes cannot fail");
                    }
                    if tail != 0 {
                        writeln!(output, "    tail: [u8; {tail}]")
                            .expect("String writes cannot fail");
                    }
                    let chunk_base = worker
                        .workspace_base
                        .checked_add(offset)
                        .and_then(|base| base.checked_add(chunk_offset))
                        .ok_or_else(|| {
                            "P025: worker workspace chunk address overflow".to_string()
                        })?;
                    writeln!(
                        output,
                        "@placed({chunk_base:#x})\n\
                         static R{index}_WORKER_{worker_index}_WORKSPACE_{name}_CHUNK_{chunk}: R{index}Worker{worker_index}Workspace{name}Chunk{chunk}View"
                    )
                    .expect("String writes cannot fail");
                    chunk_offset = chunk_offset.checked_add(chunk_bytes).ok_or_else(|| {
                        "P025: worker workspace chunk offset overflow".to_string()
                    })?;
                    chunk = chunk
                        .checked_add(1)
                        .ok_or_else(|| "P015: worker workspace chunk count overflow".to_string())?;
                }
            }
            if instrumented {
                let telemetry_base = placement
                    .state_base
                    .checked_add(state.telemetry.offset)
                    .and_then(|base| {
                        u64::try_from(worker_index)
                            .ok()
                            .and_then(|worker| worker.checked_mul(8 * super::reference::telemetry::CERTIFICATE_TELEMETRY_COUNTERS_V2))
                            .and_then(|offset| base.checked_add(offset))
                    })
                    .ok_or_else(|| "P025: worker telemetry address overflow".to_string())?;
                writeln!(
                    output,
                    "@layout(runtime, endian=little)\n\
                     struct R{index}Worker{worker_index}TelemetryView:\n\
                     \x20   counters: [u64; {}]\n\
                     @placed({telemetry_base:#x})\n\
                     static R{index}_WORKER_{worker_index}_TELEMETRY: R{index}Worker{worker_index}TelemetryView",
                    super::reference::telemetry::CERTIFICATE_TELEMETRY_COUNTERS_V2,
                )
                .expect("String writes cannot fail");
            }
        }
        if instrumented {
            let counter_bytes = u64::from(renderer.config.worker_count)
                .checked_mul(8 * super::reference::telemetry::CERTIFICATE_TELEMETRY_COUNTERS_V2)
                .ok_or_else(|| "P025: renderer telemetry counter size overflow".to_string())?;
            let evidence_base = placement
                .state_base
                .checked_add(state.telemetry.offset)
                .and_then(|base| base.checked_add(counter_bytes))
                .ok_or_else(|| "P025: raster evidence address overflow".to_string())?;
            let evidence_bytes = u64::from(renderer.config.width)
                .checked_mul(u64::from(renderer.config.height))
                .and_then(|pixels| pixels.checked_mul(24))
                .ok_or_else(|| "P025: raster evidence size overflow".to_string())?;
            let mut chunk_offset = 0_u64;
            let mut chunk = 0_usize;
            while chunk_offset < evidence_bytes {
                let chunk_bytes =
                    (evidence_bytes - chunk_offset).min(WORKSPACE_VIEW_CHUNK_BYTES / 24 * 24);
                let words = chunk_bytes / 8;
                let chunk_base = evidence_base
                    .checked_add(chunk_offset)
                    .ok_or_else(|| "P025: raster evidence chunk address overflow".to_string())?;
                writeln!(
                    output,
                    "@layout(runtime, endian=little)\n\
                     struct R{index}RasterEvidenceChunk{chunk}View:\n\
                     \x20   words: [u64; {words}]\n\
                     @placed({chunk_base:#x})\n\
                     static R{index}_RASTER_EVIDENCE_CHUNK_{chunk}: R{index}RasterEvidenceChunk{chunk}View"
                )
                .expect("String writes cannot fail");
                chunk_offset = chunk_offset
                    .checked_add(chunk_bytes)
                    .ok_or_else(|| "P025: raster evidence chunk offset overflow".to_string())?;
                chunk = chunk
                    .checked_add(1)
                    .ok_or_else(|| "P015: raster evidence chunk count overflow".to_string())?;
            }
        }
        let tables = super::binary_verify::verify_envelope(&renderer.encoded).map_err(|error| {
            format!("pixels::glue: encoded program failed verification: {error}")
        })?;
        for table in tables {
            let upper = table
                .kind
                .stable_name()
                .replace('-', "_")
                .to_ascii_uppercase();
            writeln!(
                output,
                "const R{index}_{upper}_BASE: usize = {:#x}\n\
                 const R{index}_{upper}_COUNT: usize = {}\n\
                 const R{index}_{upper}_BYTES: usize = {}",
                if table.count == 0 {
                    0
                } else {
                    placement
                        .frameprog_base
                        .checked_add(u64::from(table.offset))
                        .ok_or_else(|| {
                            "P025: generated frame-program table address overflow".to_string()
                        })?
                },
                table.count,
                table.byte_len,
            )
            .expect("String writes cannot fail");
            if table.count != 0 {
                let view_name = table_view_name(table.kind);
                let table_base = placement
                    .frameprog_base
                    .checked_add(u64::from(table.offset))
                    .ok_or_else(|| "P025: generated placed table address overflow".to_string())?;
                let record_type =
                    if table.kind == wrela_machine::pixels::FrameProgramTableKindV1::Immediate {
                        "FrameProgramImmediateV1"
                    } else {
                        "FrameProgramRecordV1"
                    };
                writeln!(
                    output,
                    "@layout(runtime, endian=little)\n\
                     struct R{index}{view_name}TableView:\n\
                     \x20   records: [{record_type}; R{index}_{upper}_COUNT]\n\
                     @placed({table_base:#x})\n\
                     static R{index}_{upper}_TABLE: R{index}{view_name}TableView",
                )
                .expect("String writes cannot fail");
            }
        }
        writeln!(
            output,
            "\n@placed({:#x})\n\
             static R{index}_FRAME_PROGRAM_HEADER: FrameProgramHeaderV1\n\
             @layout(runtime, endian=little)\n\
             struct R{index}FrameProgramDirectoryView:\n\
             \x20   records: [FrameProgramTableV1; {}]\n\
             @placed({:#x})\n\
             static R{index}_FRAME_PROGRAM_DIRECTORY: R{index}FrameProgramDirectoryView\n\
             const R{index}_DIRECTORY_COUNT: usize = {}",
            placement.frameprog_base,
            wrela_machine::pixels::FrameProgramTableKindV1::REQUIRED_COUNT,
            placement
                .frameprog_base
                .checked_add(u64::from(
                    wrela_machine::pixels::FRAME_PROGRAM_HEADER_BYTES_V1,
                ))
                .ok_or_else(|| "P025: frame-program directory address overflow".to_string())?,
            wrela_machine::pixels::FrameProgramTableKindV1::REQUIRED_COUNT,
        )
        .expect("String writes cannot fail");
    }
    write_program_accessors(&mut output, placements, compiled)?;
    write_visibility_polynomial_accessors(&mut output, compiled)?;
    write_p7_runtime_storage_accessors(&mut output, placements, compiled, instrumented)?;
    write_p8_scanout_accessors(&mut output, placements, compiled)?;
    Ok(output)
}

pub fn parse_configuration_source(source: &str) -> Result<crate::syntax::ast::Module, String> {
    let tokens = crate::syntax::lexer::lex(source)
        .map_err(|error| format!("pixels::glue: generated module lex: {}", error.message))?;
    crate::syntax::parser::parse(tokens)
        .map_err(|error| format!("pixels::glue: generated module parse: {}", error.message))
}

fn rewrite_renderer_refs(
    value: &mut crate::eval::value::Value,
    coordinators: &[usize],
) -> Result<(), String> {
    use crate::eval::image::ImageDeclRef;
    use crate::eval::value::Value;
    match value {
        Value::ImageDecl(ImageDeclRef::Renderer(index)) => {
            let actor = coordinators.get(*index).copied().ok_or_else(|| {
                format!("pixels::glue: renderer handle {index} has no coordinator")
            })?;
            *value = Value::ImageDecl(ImageDeclRef::Actor(actor));
        }
        Value::Tuple(values)
        | Value::Array(values)
        | Value::Struct(values)
        | Value::Enum(_, values) => {
            for value in values {
                rewrite_renderer_refs(value, coordinators)?;
            }
        }
        Value::Closure { env, .. } => {
            for scope in env {
                for value in scope.values_mut() {
                    rewrite_renderer_refs(value, coordinators)?;
                }
            }
        }
        _ => {}
    }
    Ok(())
}

fn integer_arg(label: &str, value: u64) -> crate::eval::image::DeclArg {
    crate::eval::image::DeclArg {
        label: label.to_string(),
        ty: crate::sema::types::Type::Usize,
        value: crate::eval::value::Value::Usize(value),
        span: crate::syntax::ast::Span::default(),
    }
}

fn u32_arg(label: &str, value: u32) -> crate::eval::image::DeclArg {
    crate::eval::image::DeclArg {
        label: label.to_string(),
        ty: crate::sema::types::Type::U32,
        value: crate::eval::value::Value::U32(value),
        span: crate::syntax::ast::Span::default(),
    }
}

fn handle_arg(
    label: &str,
    ty: crate::sema::types::Type,
    value: crate::eval::image::ImageDeclRef,
) -> crate::eval::image::DeclArg {
    crate::eval::image::DeclArg {
        label: label.to_string(),
        ty,
        value: crate::eval::value::Value::ImageDecl(value),
        span: crate::syntax::ast::Span::default(),
    }
}

fn renderer_worker_type(worker: usize) -> Result<crate::sema::types::Type, String> {
    if worker >= super::config::P7_MAX_RENDER_WORKERS {
        return Err(format!(
            "pixels::glue: worker index {worker} exceeds the P7 sealed worker types"
        ));
    }
    Ok(crate::sema::types::Type::Named(
        format!("RendererWorker{worker}"),
        Vec::new(),
    ))
}

fn worker_handles(
    first_worker: usize,
    worker_count: usize,
) -> Result<Vec<crate::eval::value::Value>, String> {
    if worker_count == 0 || worker_count > super::config::P7_MAX_RENDER_WORKERS {
        return Err("pixels::glue: sealed renderer worker count is invalid".to_string());
    }
    (0..super::config::P7_MAX_RENDER_WORKERS)
        .map(|worker| {
            first_worker
                .checked_add(worker)
                .map(crate::eval::image::ImageDeclRef::Actor)
                .map(crate::eval::value::Value::ImageDecl)
                .ok_or_else(|| "P015: generated worker edge index overflow".to_string())
        })
        .collect()
}

fn worker_job_value(
    renderer_index: usize,
    _frameprog_base: u64,
    worker_index: usize,
    worker: Option<&crate::layout::RendererCorePlacement>,
) -> Result<crate::eval::value::Value, String> {
    let tiles_start = worker.map_or(0, |worker| worker.tiles_start);
    let tiles_end = worker.map_or(0, |worker| worker.tiles_end);
    if renderer_index > 15
        || worker_index >= super::config::P7_MAX_RENDER_WORKERS
        || tiles_start > 0x00ff_ffff
        || tiles_end > 0x00ff_ffff
    {
        return Err("P025: renderer worker assignment exceeds its sealed token encoding".into());
    }
    let word = u64::from(tiles_start)
        | (u64::from(tiles_end) << 24)
        | (u64::try_from(renderer_index).map_err(|_| "P015: renderer index exceeds u64")? << 48)
        | (u64::try_from(worker_index).map_err(|_| "P015: worker index exceeds u64")? << 52);
    Ok(crate::eval::value::Value::Struct(vec![
        crate::eval::value::Value::U64(word),
    ]))
}

fn renderer_placement_value(
    renderer_index: usize,
    frameprog_base: u64,
    state_base: u64,
    state_bytes: u64,
    workers: &[crate::layout::RendererCorePlacement],
) -> Result<crate::eval::value::Value, String> {
    let mut fields = vec![
        crate::eval::value::Value::Usize(frameprog_base),
        crate::eval::value::Value::Usize(state_base),
        crate::eval::value::Value::Usize(state_bytes),
    ];
    for worker_index in 0..super::config::P7_MAX_RENDER_WORKERS {
        fields.push(worker_job_value(
            renderer_index,
            frameprog_base,
            worker_index,
            workers.get(worker_index),
        )?);
    }
    Ok(crate::eval::value::Value::Struct(fields))
}

pub fn synthesize_image_graph(
    source: &crate::eval::image::ImageGraph,
    renderers: &[GeneratedRenderer],
) -> Result<crate::eval::image::ImageGraph, String> {
    if source.renderers.len() != renderers.len() {
        return Err("pixels::glue: renderer graph/generated count differs".to_string());
    }
    let mut graph = source.clone();
    let original_actor_count = graph.actors.len();
    let coordinators = (0..renderers.len())
        .map(|index| {
            original_actor_count
                .checked_add(index)
                .ok_or_else(|| "P015: generated coordinator index overflow".to_string())
        })
        .collect::<Result<Vec<_>, _>>()?;
    let first_worker = original_actor_count
        .checked_add(renderers.len())
        .ok_or_else(|| "P015: generated worker index overflow".to_string())?;
    let generated_worker_count = renderers
        .len()
        .checked_mul(super::config::P7_MAX_RENDER_WORKERS)
        .ok_or_else(|| "P015: generated renderer worker count overflow".to_string())?;
    let next_actor = first_worker
        .checked_add(generated_worker_count)
        .ok_or_else(|| "P015: generated actor count overflow".to_string())?;
    if next_actor > crate::rtconfig::MB_POOL_COUNT {
        return Err(format!(
            "P015: renderer-generated actors need {next_actor} mailbox slots, ceiling {}",
            crate::rtconfig::MB_POOL_COUNT
        ));
    }
    for actor in &mut graph.actors {
        for argument in &mut actor.args {
            rewrite_renderer_refs(&mut argument.value, &coordinators)?;
        }
    }
    for (renderer_index, generated) in renderers.iter().enumerate() {
        let renderer_decl = source
            .renderers
            .get(renderer_index)
            .ok_or_else(|| "pixels::glue: renderer declaration missing".to_string())?;
        let mailbox = u64::try_from(generated.workers.len() + 2)
            .map_err(|_| "P015: coordinator mailbox capacity overflow".to_string())?;
        let renderer_first_worker = first_worker
            .checked_add(
                renderer_index
                    .checked_mul(super::config::P7_MAX_RENDER_WORKERS)
                    .ok_or_else(|| "P015: generated renderer worker offset overflow".to_string())?,
            )
            .ok_or_else(|| "P015: generated renderer worker edge overflow".to_string())?;
        let worker_handles = worker_handles(renderer_first_worker, generated.workers.len())?;
        let mut frame_bounds = generated
            .exposure_range
            .into_iter()
            .chain(generated.environment_min)
            .chain(generated.environment_max)
            .map(crate::eval::value::Value::F32)
            .collect::<Vec<_>>();
        for bounds in generated.camera_bounds {
            frame_bounds.extend(bounds.map(crate::eval::value::Value::F32));
        }
        frame_bounds.extend(
            generated
                .world_min
                .into_iter()
                .chain(generated.world_max)
                .map(crate::eval::value::Value::F32),
        );
        frame_bounds.push(crate::eval::value::Value::Usize(
            u64::try_from(generated.light_capacity)
                .map_err(|_| "P015: generated light capacity exceeds u64".to_string())?,
        ));
        for kind in generated.light_kinds {
            frame_bounds.push(crate::eval::value::Value::Usize(
                u64::try_from(kind)
                    .map_err(|_| "P015: generated light kind tag exceeds u64".to_string())?,
            ));
        }
        if frame_bounds.len() != RENDERER_FRAME_BOUNDS_WORDS {
            return Err(format!(
                "pixels::glue: generated frame bounds have {} values, expected {}",
                frame_bounds.len(),
                RENDERER_FRAME_BOUNDS_WORDS,
            ));
        }
        let coordinator_args = vec![
            integer_arg("core", 0),
            integer_arg("mailbox", mailbox),
            crate::eval::image::DeclArg {
                label: "workers".to_string(),
                ty: crate::sema::types::Type::Named("RendererWorkers".to_string(), Vec::new()),
                value: crate::eval::value::Value::Struct(worker_handles),
                span: crate::syntax::ast::Span::default(),
            },
            u32_arg(
                "worker_count",
                u32::try_from(generated.workers.len())
                    .map_err(|_| "P015: generated worker count exceeds u32".to_string())?,
            ),
            handle_arg(
                "display",
                crate::sema::types::Type::Usize,
                crate::eval::image::ImageDeclRef::Driver(generated.display_index),
            ),
            u32_arg(
                "renderer_index",
                u32::try_from(renderer_index)
                    .map_err(|_| "P015: renderer index exceeds u32".to_string())?,
            ),
            crate::eval::image::DeclArg {
                label: "placement".to_string(),
                ty: crate::sema::types::Type::Named(
                    "RendererPlacementState".to_string(),
                    Vec::new(),
                ),
                value: renderer_placement_value(renderer_index, 0, 0, 0, &[])?,
                span: crate::syntax::ast::Span::default(),
            },
            crate::eval::image::DeclArg {
                label: "bounds".to_string(),
                ty: crate::sema::types::Type::Named("RendererFrameBounds".to_string(), Vec::new()),
                value: crate::eval::value::Value::Struct(frame_bounds),
                span: crate::syntax::ast::Span::default(),
            },
        ];
        graph.actors.push(crate::eval::image::ActorDecl {
            actor_type: renderer_decl.actor_type.clone(),
            args: coordinator_args,
        });
    }
    for _renderer_index in 0..renderers.len() {
        for worker_index in 0..super::config::P7_MAX_RENDER_WORKERS {
            let core = worker_index.min(source.cores.saturating_sub(1));
            graph.actors.push(crate::eval::image::ActorDecl {
                actor_type: renderer_worker_type(worker_index)?,
                args: vec![
                    integer_arg(
                        "core",
                        u64::try_from(core).map_err(|_| "pixels::glue: worker core exceeds u64")?,
                    ),
                    integer_arg("mailbox", 1),
                ],
            });
        }
    }
    for actor in &graph.actors {
        for argument in &actor.args {
            fn has_renderer(value: &crate::eval::value::Value) -> bool {
                use crate::eval::image::ImageDeclRef;
                use crate::eval::value::Value;
                match value {
                    Value::ImageDecl(ImageDeclRef::Renderer(_)) => true,
                    Value::Tuple(values)
                    | Value::Array(values)
                    | Value::Struct(values)
                    | Value::Enum(_, values) => values.iter().any(has_renderer),
                    Value::Closure { env, .. } => {
                        env.iter().any(|scope| scope.values().any(has_renderer))
                    }
                    _ => false,
                }
            }
            if has_renderer(&argument.value) {
                return Err(
                    "pixels::glue: unresolved renderer declaration reference after synthesis"
                        .to_string(),
                );
            }
        }
    }
    Ok(graph)
}

fn set_generated_arg(
    actor: &mut crate::eval::image::ActorDecl,
    label: &str,
    value: crate::eval::value::Value,
) -> Result<(), String> {
    let argument = actor
        .args
        .iter_mut()
        .find(|argument| argument.label == label)
        .ok_or_else(|| format!("pixels::glue: generated actor has no `{label}` argument"))?;
    argument.value = value;
    Ok(())
}

pub fn bind_image_graph_placements(
    graph: &mut crate::eval::image::ImageGraph,
    renderers: &[GeneratedRenderer],
    placements: &[crate::layout::RendererPlacement],
) -> Result<(), String> {
    if renderers.len() != placements.len() {
        return Err("pixels::glue: renderer/placement count differs".to_string());
    }
    let generated_worker_count = renderers
        .len()
        .checked_mul(super::config::P7_MAX_RENDER_WORKERS)
        .ok_or_else(|| "P015: generated renderer worker count overflow".to_string())?;
    let generated_actor_count = renderers
        .len()
        .checked_add(generated_worker_count)
        .ok_or_else(|| "P015: generated actor count overflow".to_string())?;
    let mut actor_index = graph
        .actors
        .len()
        .checked_sub(generated_actor_count)
        .ok_or_else(|| "pixels::glue: generated actor suffix is truncated".to_string())?;
    for (renderer, placement) in renderers.iter().zip(placements) {
        if renderer.renderer_index != placement.index
            || renderer.workers.len() != placement.per_core.len()
        {
            return Err("pixels::glue: generated renderer placement identity differs".to_string());
        }
        let coordinator = graph
            .actors
            .get_mut(actor_index)
            .ok_or_else(|| "pixels::glue: coordinator actor is missing".to_string())?;
        set_generated_arg(
            coordinator,
            "placement",
            renderer_placement_value(
                renderer.renderer_index,
                placement.frameprog_base,
                placement.state_base,
                placement.state_size,
                &placement.per_core,
            )?,
        )?;
        actor_index += 1;
    }
    if actor_index
        .checked_add(generated_worker_count)
        .is_none_or(|end| end != graph.actors.len())
    {
        return Err("pixels::glue: generated actor suffix has trailing actors".to_string());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    #[test]
    fn canonical_wrela_wire_views_match_machine_sizes_and_offsets() {
        let (_, loaded) = crate::loader::load_render_program_module()
            .unwrap_or_else(|_| panic!("load render_program"));
        let module = crate::sema::specialize::specialize(&loaded.module).expect("specialize");
        let layouts = crate::sema::types::check_layouts(&module).expect("layout views");
        let fields = |name: &str| {
            let layout = layouts
                .iter()
                .find(|layout| layout.name == name)
                .unwrap_or_else(|| panic!("missing layout {name}"));
            let fields = layout
                .entries
                .iter()
                .filter_map(|entry| match entry {
                    crate::sema::types::LayoutEntry::Field(field) => {
                        Some((field.name.as_str(), field.offset, field.size))
                    }
                    crate::sema::types::LayoutEntry::Padding { .. } => None,
                })
                .collect::<Vec<_>>();
            (layout.size.expect("fixed layout size"), fields)
        };
        assert_eq!(
            fields("FrameProgramHeaderV1"),
            (
                u64::from(wrela_machine::pixels::FRAME_PROGRAM_HEADER_BYTES_V1),
                vec![
                    ("magic", 0, 8),
                    ("version", 8, 2),
                    ("header_bytes", 10, 2),
                    ("flags", 12, 4),
                    ("total_bytes", 16, 4),
                    ("renderer_index", 20, 2),
                    ("reserved0", 22, 2),
                    ("numeric_revision", 24, 4),
                    ("formal_revision", 28, 4),
                    ("table_count", 32, 2),
                    ("reserved1", 34, 14),
                    ("digest", 48, 32),
                ],
            )
        );
        assert_eq!(
            fields("FrameProgramTableV1").0,
            u64::from(wrela_machine::pixels::FRAME_PROGRAM_TABLE_BYTES_V1)
        );
        assert_eq!(
            fields("FrameProgramRecordV1").0,
            u64::from(wrela_machine::pixels::FRAME_PROGRAM_RECORD_BYTES_V1)
        );
        assert_eq!(
            fields("FrameProgramImmediateV1").0,
            u64::from(wrela_machine::pixels::FRAME_PROGRAM_IMMEDIATE_BYTES_V1)
        );
    }

    #[test]
    fn bootstrap_census_comes_from_populated_verified_record_tables() {
        let program = super::super::program::minimal_verified_frame_program();
        assert_eq!(
            super::bootstrap_families(&program)
                .into_iter()
                .collect::<Vec<_>>(),
            vec![
                "camera-light-post",
                "csg",
                "feature",
                "field",
                "fixed-domain",
                "object",
                "scalar",
                "shading-summary"
            ]
        );
    }

    use super::*;

    fn two_sum_f32(a: f32, b: f32) -> [f32; 2] {
        let sum = a + b;
        let virtual_b = sum - a;
        [sum, (a - (sum - virtual_b)) + (b - virtual_b)]
    }

    fn two_product_f32(a: f32, b: f32) -> [f32; 2] {
        let product = a * b;
        let split_a = a * 4097.0;
        let a_hi = split_a - (split_a - a);
        let a_lo = a - a_hi;
        let split_b = b * 4097.0;
        let b_hi = split_b - (split_b - b);
        let b_lo = b - b_hi;
        [
            product,
            ((a_hi * b_hi - product) + a_hi * b_lo + a_lo * b_hi) + a_lo * b_lo,
        ]
    }

    fn dd_mul_f32(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
        let product = two_product_f32(a[0], b[0]);
        let correction = product[1] + a[0] * b[1] + a[1] * b[0] + a[1] * b[1];
        let normalized = two_sum_f32(product[0], correction);
        let a_magnitude = a[0].abs() + a[1].abs();
        let b_magnitude = b[0].abs() + b[1].abs();
        let error = super::super::reference::interval::next_up_f32(
            a[2] * (b_magnitude + b[2])
                + b[2] * a_magnitude
                + (a_magnitude + a[2]) * (b_magnitude + b[2]) * 0.000_000_000_002,
        );
        [normalized[0], normalized[1], error]
    }

    fn dd_add_f32(a: [f32; 3], b: f32) -> [f32; 3] {
        let sum = two_sum_f32(a[0], b);
        let normalized = two_sum_f32(sum[0], sum[1] + a[1]);
        let magnitude = a[0].abs() + a[1].abs() + a[2] + b.abs();
        let error =
            super::super::reference::interval::next_up_f32(a[2] + magnitude * 0.000_000_000_002);
        [normalized[0], normalized[1], error]
    }

    fn standard_coefficient_intervals(eye: f32) -> Vec<[f32; 2]> {
        let eye2_product = two_product_f32(eye, eye);
        let eye2 = [eye2_product[0], eye2_product[1], 0.0];
        let mut terms = BTreeMap::<(u8, u8), [i32; 5]>::new();
        for &(x, y, eye_degree, coefficient) in &STANDARD_TORUS_DISCRIMINANT_TERMS {
            terms.entry((x, y)).or_default()[usize::from(eye_degree / 2)] += coefficient;
        }
        terms
            .values()
            .map(|by_degree| {
                let max_degree = by_degree
                    .iter()
                    .rposition(|coefficient| *coefficient != 0)
                    .unwrap_or(0);
                let mut value = [by_degree[max_degree] as f32, 0.0, 0.0];
                for degree in (0..max_degree).rev() {
                    value = dd_mul_f32(value, eye2);
                    value = dd_add_f32(value, by_degree[degree] as f32);
                }
                let sum = two_sum_f32(value[0], value[1]);
                let error = super::super::reference::interval::next_up_f32(sum[1].abs() + value[2]);
                [
                    super::super::reference::interval::next_down_f32(sum[0] - error),
                    super::super::reference::interval::next_up_f32(sum[0] + error),
                ]
            })
            .collect()
    }

    fn interval_mul_f32(a: [f32; 2], b: [f32; 2]) -> [f32; 2] {
        let products = [a[0] * b[0], a[0] * b[1], a[1] * b[0], a[1] * b[1]];
        let lo = products.iter().copied().fold(f32::INFINITY, f32::min);
        let hi = products.iter().copied().fold(f32::NEG_INFINITY, f32::max);
        [
            super::super::reference::interval::next_down_f32(lo),
            super::super::reference::interval::next_up_f32(hi),
        ]
    }

    fn standard_value_enclosure(eye: f32, u: f32, v: f32) -> [f32; 2] {
        let coefficients = standard_coefficient_intervals(eye);
        let x = [
            super::super::reference::interval::next_down_f32(u * u).max(0.0),
            super::super::reference::interval::next_up_f32(u * u),
        ];
        let y = [
            super::super::reference::interval::next_down_f32(v * v).max(0.0),
            super::super::reference::interval::next_up_f32(v * v),
        ];
        let mut rows = [[0.0_f32; 2]; 7];
        let mut coefficient_index = 0;
        for (x_degree, row) in rows.iter_mut().enumerate() {
            let max_y = 6 - x_degree;
            let mut accumulator = coefficients[coefficient_index + max_y];
            for y_degree in (0..max_y).rev() {
                let product = interval_mul_f32(accumulator, y);
                accumulator = [
                    super::super::reference::interval::next_down_f32(
                        product[0] + coefficients[coefficient_index + y_degree][0],
                    ),
                    super::super::reference::interval::next_up_f32(
                        product[1] + coefficients[coefficient_index + y_degree][1],
                    ),
                ];
            }
            *row = accumulator;
            coefficient_index += max_y + 1;
        }
        let mut value = rows[6];
        for x_degree in (0..6).rev() {
            let product = interval_mul_f32(value, x);
            value = [
                super::super::reference::interval::next_down_f32(product[0] + rows[x_degree][0]),
                super::super::reference::interval::next_up_f32(product[1] + rows[x_degree][1]),
            ];
        }
        let mut center = value[0] + (value[1] - value[0]) * 0.5;
        center = center.clamp(value[0], value[1]);
        let error = super::super::reference::interval::next_up_f32(
            (center - value[0]).abs().max((value[1] - center).abs()),
        );
        [center * 65536.0, error * 65536.0]
    }

    fn outward_low(value: f32) -> f32 {
        super::super::reference::interval::next_down_f32(value)
    }

    fn outward_high(value: f32) -> f32 {
        super::super::reference::interval::next_up_f32(value)
    }

    fn standard_cell_classification(u: f32, v: f32, ru: f32, rv: f32, eye: f32) -> Option<bool> {
        let (u0, u1) = (u - ru, u + ru);
        let (v0, v1) = (v - rv, v + rv);
        let u2_lo = if u0 > 0.0 {
            outward_low(u0 * u0)
        } else if u1 < 0.0 {
            outward_low(u1 * u1)
        } else {
            0.0
        };
        let u2_hi = outward_high(u0 * u0).max(outward_high(u1 * u1));
        let v2_lo = if v0 > 0.0 {
            outward_low(v0 * v0)
        } else if v1 < 0.0 {
            outward_low(v1 * v1)
        } else {
            0.0
        };
        let v2_hi = outward_high(v0 * v0).max(outward_high(v1 * v1));
        let x = [u2_lo, u2_hi];
        let y = [v2_lo, v2_hi];
        let eye2 = [outward_low(eye * eye), outward_high(eye * eye)];
        let sum = [
            outward_low(x[0] + y[0] + 1.0),
            outward_high(x[1] + y[1] + 1.0),
        ];
        let a = interval_mul_f32(sum, sum);
        let b = [
            outward_low(sum[1] * eye * -4.0),
            outward_high(sum[0] * eye * -4.0),
        ];
        let eye2_plus3 = [outward_low(eye2[0] + 3.0), outward_high(eye2[1] + 3.0)];
        let c_middle = interval_mul_f32(sum, eye2_plus3);
        let c = [
            outward_low(eye2[0] * 4.0 + c_middle[0] * 2.0 - (x[1] + 1.0) * 16.0),
            outward_high(eye2[1] * 4.0 + c_middle[1] * 2.0 - (x[0] + 1.0) * 16.0),
        ];
        let five_minus_eye2 = [outward_low(5.0 - eye2[1]), outward_high(5.0 - eye2[0])];
        let d0 = interval_mul_f32([eye, eye], five_minus_eye2);
        let d = [outward_low(d0[0] * 4.0), outward_high(d0[1] * 4.0)];
        let em1 = [outward_low(eye2[0] - 1.0), outward_high(eye2[1] - 1.0)];
        let em9 = [outward_low(eye2[0] - 9.0), outward_high(eye2[1] - 9.0)];
        let e = interval_mul_f32(em1, em9);
        let ac = interval_mul_f32(a, c);
        let bb = interval_mul_f32(b, b);
        let p = [
            outward_low(ac[0] * 8.0 - bb[1] * 3.0),
            outward_high(ac[1] * 8.0 - bb[0] * 3.0),
        ];
        let aa = interval_mul_f32(a, a);
        let aaa = interval_mul_f32(aa, a);
        let aaae = interval_mul_f32(aaa, e);
        let cc = interval_mul_f32(c, c);
        let aacc = interval_mul_f32(aa, cc);
        let ab = interval_mul_f32(a, b);
        let abc = interval_mul_f32(ab, c);
        let abbc = interval_mul_f32(abc, b);
        let aab = interval_mul_f32(aa, b);
        let aabd = interval_mul_f32(aab, d);
        let bbbb = interval_mul_f32(bb, bb);
        let q = [
            outward_low(
                aaae[0] * 64.0 - aacc[1] * 16.0 + abbc[0] * 16.0 - aabd[1] * 16.0 - bbbb[1] * 3.0,
            ),
            outward_high(
                aaae[1] * 64.0 - aacc[0] * 16.0 + abbc[1] * 16.0 - aabd[0] * 16.0 - bbbb[0] * 3.0,
            ),
        ];
        if p[1] < 0.0 && q[1] < 0.0 {
            Some(true)
        } else if p[0] > 0.0 || q[0] > 0.0 {
            Some(false)
        } else {
            None
        }
    }

    fn exact_torus_pq(u: f64, v: f64, eye: f64) -> [f64; 2] {
        let x = u * u;
        let y = v * v;
        let eye2 = eye * eye;
        let sum = x + y + 1.0;
        let a = sum * sum;
        let b = sum * eye * -4.0;
        let c = eye2 * 4.0 + sum * (eye2 + 3.0) * 2.0 - (x + 1.0) * 16.0;
        let d = eye * (5.0 - eye2) * 4.0;
        let e = (eye2 - 1.0) * (eye2 - 9.0);
        [
            a * c * 8.0 - b * b * 3.0,
            a * a * a * e * 64.0 - a * a * c * c * 16.0 + a * b * b * c * 16.0
                - a * a * b * d * 16.0
                - b * b * b * b * 3.0,
        ]
    }

    #[test]
    fn standard_torus_compensated_coefficients_enclose_f64_reference() {
        let mut terms = BTreeMap::<(u8, u8), [i32; 5]>::new();
        for &(x, y, eye_degree, coefficient) in &STANDARD_TORUS_DISCRIMINANT_TERMS {
            terms.entry((x, y)).or_default()[usize::from(eye_degree / 2)] += coefficient;
        }
        for step in 0..=1024 {
            let eye = (0.125_f32 + step as f32 * ((64.0_f32 - 0.125) / 1024.0)).min(64.0);
            let eye2 = f64::from(eye) * f64::from(eye);
            let intervals = standard_coefficient_intervals(eye);
            for ((powers, by_degree), interval) in terms.iter().zip(&intervals) {
                let max_degree = by_degree
                    .iter()
                    .rposition(|coefficient| *coefficient != 0)
                    .unwrap_or(0);
                let mut exact = f64::from(by_degree[max_degree]);
                for degree in (0..max_degree).rev() {
                    exact = exact * eye2 + f64::from(by_degree[degree]);
                }
                assert!(
                    f64::from(interval[0]) <= exact && exact <= f64::from(interval[1]),
                    "eye={eye:?} powers={powers:?} interval={interval:?} exact={exact:?}"
                );
            }
        }
    }

    #[test]
    fn standard_torus_value_interval_encloses_expanded_f64_discriminant() {
        let eyes = [0.125_f32, 0.5, 1.0, 3.0, 4.35, 8.0, 32.0, 64.0];
        for eye in eyes {
            for u_step in -8..=8 {
                let u = u_step as f32 * 0.25;
                for v_step in -8..=8 {
                    let v = v_step as f32 * 0.25;
                    let enclosure = standard_value_enclosure(eye, u, v);
                    let exact = STANDARD_TORUS_DISCRIMINANT_TERMS.iter().fold(
                        0.0_f64,
                        |sum, &(x, y, eye_degree, coefficient)| {
                            sum + f64::from(coefficient)
                                * f64::from(u).powi(2 * i32::from(x))
                                * f64::from(v).powi(2 * i32::from(y))
                                * f64::from(eye).powi(i32::from(eye_degree))
                        },
                    ) * 65536.0;
                    let lo = f64::from(enclosure[0] - enclosure[1]);
                    let hi = f64::from(enclosure[0] + enclosure[1]);
                    assert!(
                        lo <= exact && exact <= hi,
                        "eye={eye:?} u={u:?} v={v:?} enclosure={enclosure:?} exact={exact:?}"
                    );
                }
            }
        }
    }

    #[test]
    fn standard_torus_cell_classifier_never_claims_the_wrong_pq_region() {
        let eyes = [0.5_f32, 1.5, 3.0, 4.35, 8.0];
        let radii = [0.01_f32, 0.0625, 0.125, 0.25];
        let mut classified = 0;
        for eye in eyes {
            for u_step in -6..=6 {
                let u = u_step as f32 * 0.25;
                for v_step in -6..=6 {
                    let v = v_step as f32 * 0.25;
                    for radius in radii {
                        let Some(positive_hit) =
                            standard_cell_classification(u, v, radius, radius, eye)
                        else {
                            continue;
                        };
                        classified += 1;
                        for sample_u in 0..=4 {
                            let su = f64::from(u - radius)
                                + f64::from(radius * 2.0) * f64::from(sample_u) / 4.0;
                            for sample_v in 0..=4 {
                                let sv = f64::from(v - radius)
                                    + f64::from(radius * 2.0) * f64::from(sample_v) / 4.0;
                                let pq = exact_torus_pq(su, sv, f64::from(eye));
                                let agrees = if positive_hit {
                                    pq[0] < 0.0 && pq[1] < 0.0
                                } else {
                                    pq[0] > 0.0 || pq[1] > 0.0
                                };
                                assert!(
                                    agrees,
                                    "eye={eye:?} cell=({u:?},{v:?},{radius:?}) sample=({su:?},{sv:?}) pq={pq:?} class={positive_hit:?}"
                                );
                            }
                        }
                    }
                }
            }
        }
        assert!(
            classified > 100,
            "classifier test did not exercise enough cells"
        );
    }

    #[test]
    fn tile_partition_is_half_open_complete_and_disjoint() {
        let tile_count = 17_u32;
        let workers = 4_u32;
        let ranges = (0..workers)
            .map(|worker| {
                (
                    (u64::from(tile_count) * u64::from(worker) + u64::from(workers - 1))
                        / u64::from(workers),
                    (u64::from(tile_count) * u64::from(worker + 1) + u64::from(workers - 1))
                        / u64::from(workers),
                )
            })
            .collect::<Vec<_>>();
        assert_eq!(ranges.first().unwrap().0, 0);
        assert_eq!(ranges.last().unwrap().1, u64::from(tile_count));
        assert!(ranges.windows(2).all(|pair| pair[0].1 == pair[1].0));
        assert_eq!(
            (0..4)
                .map(|worker| { ((worker + 3) / 4, ((worker + 1) + 3) / 4,) })
                .collect::<Vec<_>>(),
            vec![(0, 1), (1, 1), (1, 1), (1, 1)],
        );
    }

    #[test]
    fn images_without_renderers_do_not_gain_renderer_workers() {
        let mut source = crate::eval::image::ImageGraph::default();
        source.actors.push(crate::eval::image::ActorDecl {
            actor_type: crate::sema::types::Type::Named("App".to_string(), Vec::new()),
            args: Vec::new(),
        });
        let synthesized = synthesize_image_graph(&source, &[]).unwrap();
        assert_eq!(synthesized.actors, source.actors);
    }

    #[test]
    fn worker_handle_slots_name_the_declared_sealed_worker_types() {
        let first = 17;
        let handles = worker_handles(first, 1).unwrap();
        assert_eq!(handles.len(), crate::pixels::config::P7_MAX_RENDER_WORKERS);
        for handle in &handles {
            let crate::eval::value::Value::ImageDecl(crate::eval::image::ImageDeclRef::Actor(
                actor,
            )) = handle
            else {
                panic!("worker handle is not an actor")
            };
            assert_eq!(
                *actor,
                first + handles.iter().position(|value| value == handle).unwrap()
            );
        }
        let handles = worker_handles(first, 4).unwrap();
        for (slot, handle) in handles.iter().enumerate() {
            let crate::eval::value::Value::ImageDecl(crate::eval::image::ImageDeclRef::Actor(
                actor,
            )) = handle
            else {
                panic!("worker handle is not an actor")
            };
            assert_eq!(*actor, first + slot);
        }
        assert_eq!(
            renderer_worker_type(3).unwrap(),
            crate::sema::types::Type::Named("RendererWorker3".to_string(), Vec::new())
        );
    }

    #[test]
    fn worker_assignment_token_round_trips_and_rejects_unrepresentable_ranges() {
        let placement = crate::layout::RendererCorePlacement {
            worker_index: 3,
            core: 3,
            actor: "worker".to_string(),
            tiles_start: 0x00ab_cdef,
            tiles_end: 0x00ff_ffff,
            workspace_base: 0x4066_0000,
            workspace_bytes: 4096,
        };
        let crate::eval::value::Value::Struct(words) =
            worker_job_value(5, 0x4063_0000, 3, Some(&placement)).unwrap()
        else {
            panic!("worker assignment must be a struct");
        };
        let [crate::eval::value::Value::U64(word)] = words.as_slice() else {
            panic!("worker assignment must contain one u64 token");
        };
        assert_eq!(word & 0x00ff_ffff, u64::from(placement.tiles_start));
        assert_eq!((word >> 24) & 0x00ff_ffff, u64::from(placement.tiles_end));
        assert_eq!((word >> 48) & 15, 5);
        assert_eq!((word >> 52) & 3, 3);
        assert_eq!(word >> 54, 0);
        assert!(worker_job_value(16, 0, 0, None).is_err());
        let mut too_many_tiles = placement;
        too_many_tiles.tiles_end = 0x0100_0000;
        assert!(worker_job_value(0, 0, 0, Some(&too_many_tiles)).is_err());
    }

    #[test]
    fn p9_local_texture_frame_replays_rigid_source_order_for_all_differentials() {
        use super::super::graph::TransformProgram;
        use super::super::ids::ScalarId;

        let transform = TransformProgram::Rigid {
            translation: [ScalarId(0), ScalarId(1), ScalarId(2)],
            row_x: [ScalarId(3), ScalarId(4), ScalarId(5)],
            row_y: [ScalarId(6), ScalarId(7), ScalarId(8)],
            row_z: [ScalarId(9), ScalarId(10), ScalarId(11)],
        };
        let mut source = String::new();
        let mut temporary = 0;
        write_p9_local_transform(&mut source, &transform, &mut temporary).unwrap();
        assert_eq!(temporary, 1);
        let translation = source
            .find("local_p_x = local_p_x - __p7_scalar_0")
            .expect("translation is emitted");
        let saved_point = source
            .find("uv_0_p_x = local_p_x")
            .expect("translated point is saved before rotation");
        assert!(translation < saved_point);
        for vector in ["p", "n", "dx", "dy"] {
            assert!(source.contains(&format!(
                "local_{vector}_x = __p7_scalar_3 * uv_0_{vector}_x + __p7_scalar_4 * uv_0_{vector}_y + __p7_scalar_5 * uv_0_{vector}_z"
            )));
        }
        assert!(!source.contains("local_n_x = local_n_x -"));
        assert!(!source.contains("local_dx_x = local_dx_x -"));
    }

    #[test]
    fn placement_binding_gives_each_generated_actor_exact_addresses() {
        let mut graph = crate::eval::image::ImageGraph::default();
        let actor = |ty: crate::sema::types::Type, labels: &[&str]| crate::eval::image::ActorDecl {
            actor_type: ty,
            args: labels.iter().map(|label| integer_arg(label, 0)).collect(),
        };
        graph.actors.push(actor(
            crate::sema::types::Type::Named("Renderer".to_string(), Vec::new()),
            &["placement"],
        ));
        for worker in 0..crate::pixels::config::P7_MAX_RENDER_WORKERS {
            graph
                .actors
                .push(actor(renderer_worker_type(worker).unwrap(), &[]));
        }
        let generated = GeneratedRenderer {
            renderer_index: 0,
            coordinator: "coordinator".to_string(),
            display_index: 0,
            workers: vec![GeneratedWorker {
                actor: "worker".to_string(),
                core: 0,
                tiles_start: 0,
                tiles_end: 8,
            }],
            exposure_range: [-1.0, 1.0],
            environment_min: [0.0; 3],
            environment_max: [1.0; 3],
            camera_bounds: [[-1.0, 1.0]; 12],
            world_min: [-1.0; 3],
            world_max: [1.0; 3],
            light_capacity: 0,
            light_kinds: [0; 8],
            rooted_functions: Vec::new(),
            bootstrap_families: Vec::new(),
        };
        let placement = crate::layout::RendererPlacement {
            index: 0,
            frameprog_base: 0x4055_0000,
            frameprog_size: 4096,
            state_base: 0x4056_0000,
            state_size: 8192,
            coordinator_actor: "coordinator".to_string(),
            coordinator_core: 0,
            per_core: vec![crate::layout::RendererCorePlacement {
                worker_index: 0,
                core: 0,
                actor: "worker".to_string(),
                tiles_start: 0,
                tiles_end: 8,
                workspace_base: 0x4056_1000,
                workspace_bytes: 1024,
            }],
            framebuffer_base: 0x4056_2000,
            framebuffer_bytes: 4096,
            probe_base: 0,
            probe_bytes: 0,
        };
        let placements = vec![placement];
        bind_image_graph_placements(&mut graph, &[generated], &placements).unwrap();
        let value = |actor: usize, label: &str| {
            graph.actors[actor]
                .args
                .iter()
                .find(|argument| argument.label == label)
                .map(|argument| argument.value.clone())
                .unwrap()
        };
        assert_eq!(
            value(0, "placement"),
            renderer_placement_value(0, 0x4055_0000, 0x4056_0000, 8192, &placements[0].per_core,)
                .unwrap()
        );
    }
}
