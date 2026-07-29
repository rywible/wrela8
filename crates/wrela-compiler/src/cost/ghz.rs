//! Clock-rate helpers for proxy-cycle → wall-time conversion.
//!
//! Differential ranking only — GHz is a display scale, not a measurement
//! of host wall time or A76 SOG.

/// Default clock for Pi 5 flagship (1 GiB).
pub const DEFAULT_GHZ: f64 = 2.4;

/// Compact display for GHz / turns_per_sec / ms_per_turn_model in dumps.
/// Trims trailing zeros after a fixed precision (stable across hosts).
pub fn fmt_compact(v: f64) -> String {
    if !v.is_finite() {
        return "n/a".to_string();
    }
    // Enough digits for cycle→rate conversions; trim trailing zeros.
    let s = format!("{v:.10}");
    if let Some(dot) = s.find('.') {
        let int = &s[..dot];
        let frac = s[dot + 1..].trim_end_matches('0');
        if frac.is_empty() {
            int.to_string()
        } else {
            format!("{int}.{frac}")
        }
    } else {
        s
    }
}

/// Parse a GHz clock string. Fail closed if non-finite or `<= 0`.
pub fn parse_ghz(s: &str) -> Result<f64, String> {
    let v: f64 = s.parse().map_err(|e| format!("ghz: parse `{s}`: {e}"))?;
    if !v.is_finite() || v <= 0.0 {
        return Err(format!("ghz: must be finite and > 0, got {s}"));
    }
    Ok(v)
}

fn ghz_ok(ghz: f64) -> bool {
    ghz.is_finite() && ghz > 0.0
}

/// Turns per second at `ghz`: `ghz * 1e9 / proxy_cycles`.
/// `None` if `proxy_cycles == 0` or `ghz` is non-finite / `<= 0`.
pub fn turns_per_sec(proxy_cycles: u64, ghz: f64) -> Option<f64> {
    if proxy_cycles == 0 || !ghz_ok(ghz) {
        return None;
    }
    Some(ghz * 1e9 / (proxy_cycles as f64))
}

/// Milliseconds per turn at `ghz`: `proxy_cycles / (ghz * 1e6)`.
/// `None` if `proxy_cycles == 0` or `ghz` is non-finite / `<= 0`.
pub fn ms_per_turn(proxy_cycles: u64, ghz: f64) -> Option<f64> {
    if proxy_cycles == 0 || !ghz_ok(ghz) {
        return None;
    }
    Some((proxy_cycles as f64) / (ghz * 1e6))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_is_pi5_flagship() {
        assert_eq!(DEFAULT_GHZ, 2.4);
    }

    #[test]
    fn fmt_compact_trims_trailing_zeros() {
        assert_eq!(fmt_compact(2.4), "2.4");
        assert_eq!(fmt_compact(1.0), "1");
        assert_eq!(fmt_compact(0.5), "0.5");
        assert_eq!(fmt_compact(1_000_000.0), "1000000");
        assert_eq!(fmt_compact(0.001), "0.001");
        // 1e9 / 2400 at 1.0 GHz
        assert_eq!(fmt_compact(1e9 / 2400.0), "416666.6666666667");
    }

    #[test]
    fn parse_success() {
        assert_eq!(parse_ghz("2.4").unwrap(), 2.4);
        assert_eq!(parse_ghz("1").unwrap(), 1.0);
        assert_eq!(parse_ghz("0.5").unwrap(), 0.5);
    }

    #[test]
    fn parse_rejects_zero() {
        let err = parse_ghz("0").expect_err("zero");
        assert!(err.contains("ghz"), "got: {err}");
    }

    #[test]
    fn parse_rejects_negative() {
        let err = parse_ghz("-1").expect_err("negative");
        assert!(err.contains("ghz"), "got: {err}");
    }

    #[test]
    fn parse_rejects_nan() {
        let err = parse_ghz("nan").expect_err("nan");
        assert!(err.contains("ghz"), "got: {err}");
        let err = parse_ghz("NaN").expect_err("NaN");
        assert!(err.contains("ghz"), "got: {err}");
    }

    #[test]
    fn parse_rejects_non_numeric() {
        assert!(parse_ghz("abc").is_err());
        assert!(parse_ghz("").is_err());
    }

    #[test]
    fn turns_per_sec_known() {
        // 2.4 GHz / 2400 cycles = 1e6 turns/sec
        let t = turns_per_sec(2400, 2.4).expect("ok");
        assert!((t - 1_000_000.0).abs() < 1e-6, "got {t}");
    }

    #[test]
    fn ms_per_turn_known() {
        // 2400 cycles / (2.4 * 1e6) = 0.001 ms
        let t = ms_per_turn(2400, 2.4).expect("ok");
        assert!((t - 0.001).abs() < 1e-12, "got {t}");
    }

    #[test]
    fn turns_per_sec_none_on_zero_cycles() {
        assert_eq!(turns_per_sec(0, 2.4), None);
    }

    #[test]
    fn turns_per_sec_none_on_bad_ghz() {
        assert_eq!(turns_per_sec(100, 0.0), None);
        assert_eq!(turns_per_sec(100, -1.0), None);
        assert_eq!(turns_per_sec(100, f64::NAN), None);
    }

    #[test]
    fn ms_per_turn_none_on_zero_cycles() {
        assert_eq!(ms_per_turn(0, 2.4), None);
    }

    #[test]
    fn ms_per_turn_none_on_bad_ghz() {
        assert_eq!(ms_per_turn(100, 0.0), None);
        assert_eq!(ms_per_turn(100, -1.0), None);
        assert_eq!(ms_per_turn(100, f64::NAN), None);
    }
}
