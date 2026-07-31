//! Report formatting.
//!
//! Deterministic text, in the shape of the existing xtask lanes: every
//! number printed with its denominator so a ratio can never be quoted
//! without the population it came from. §16.3's "instrument it like a fuzz
//! lane — print measured reach, so a collapse to a flattering number on an
//! unrepresentative scene is visible rather than silent."

/// Percentage with its raw counts, so `100.0%` of four cells cannot be
/// mistaken for a result.
pub fn fmt_pct(num: f64, den: f64) -> String {
    if den <= 0.0 {
        return "n/a".to_string();
    }
    format!("{:>6.2}%", 100.0 * num / den)
}

/// Median of a pre-sorted slice.
pub fn median(sorted: &[u32]) -> u32 {
    if sorted.is_empty() {
        return 0;
    }
    sorted[sorted.len() / 2]
}

/// §1's machine, restated so the projection below cites one place.
///
/// 3 guest cores (one A76 is pinned to the host Linux, and that is a
/// machine-contract fact); ~2.4 of them are render after the ~20% game share;
/// 2 NEON pipes × FMLA `.4s` × 2/cycle = 16 fp32 FLOP/cycle/core at 2.4 GHz.
pub const RENDER_CORE_EQUIV: f64 = 2.4;
pub const FLOP_PER_CYCLE_PER_CORE: f64 = 16.0;
pub const CLOCK_HZ: f64 = 2.4e9;

pub fn peak_flops() -> f64 {
    RENDER_CORE_EQUIV * FLOP_PER_CYCLE_PER_CORE * CLOCK_HZ
}

/// Largest 16:9 frame whose pixel count fits `px`.
pub fn res_16x9(px: f64) -> String {
    if px <= 0.0 {
        return "none".to_string();
    }
    let w = (px * 16.0 / 9.0).sqrt();
    let h = w * 9.0 / 16.0;
    // Round to even, which is what a scanout path will want anyway.
    let w = ((w / 2.0).floor() * 2.0) as u32;
    let h = ((h / 2.0).floor() * 2.0) as u32;
    format!("{w}x{h}")
}

/// Name the nearest familiar mode at or below this pixel count, so the
/// number lands as a product decision rather than as arithmetic.
pub fn nearest_mode(px: f64) -> &'static str {
    const MODES: [(f64, &str); 8] = [
        (3840.0 * 2160.0, "2160p"),
        (2560.0 * 1440.0, "1440p"),
        (1920.0 * 1080.0, "1080p"),
        (1280.0 * 720.0, "720p"),
        (960.0 * 540.0, "540p"),
        (854.0 * 480.0, "480p"),
        (640.0 * 360.0, "360p"),
        (512.0 * 288.0, "288p (the §1 floor)"),
    ];
    for (p, n) in MODES {
        if px >= p {
            return n;
        }
    }
    "below the §1 floor"
}
