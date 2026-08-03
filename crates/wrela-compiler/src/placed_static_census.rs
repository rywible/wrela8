use crate::census;
use crate::layout::PlacedStatic;
use crate::rtconfig::{INIT_SPAN_POOL_COUNT, MB_POOL_COUNT};

pub fn fixed_core_names() -> &'static [String] {
    &census::data().placed_static_fixed_core_names
}

pub fn fixed_set_len() -> usize {
    fixed_core_names().len() + MB_POOL_COUNT * 2
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Census {
    pub count: usize,
    pub fixed: Vec<String>,
    pub spans: usize,
}

impl Census {
    pub fn render_line(&self) -> String {
        format!(
            "PlacedStatics count={} fixed={} spans={}",
            self.count,
            self.fixed.join(","),
            self.spans
        )
    }

    pub fn within_ratchet(&self) -> bool {
        self.count <= fixed_set_len() + self.spans
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Class {
    Fixed,
    InitSpan(usize),
    Unexpected,
}

pub fn classify(name: &str) -> Class {
    if fixed_core_names().iter().any(|n| n == name) {
        return Class::Fixed;
    }
    if let Some(rest) = name.strip_prefix("MB") {
        if let Some((idx, suffix)) = rest.split_once('_') {
            if matches!(suffix, "DATA" | "CTL") {
                if let Ok(i) = idx.parse::<usize>() {
                    if i < MB_POOL_COUNT {
                        return Class::Fixed;
                    }
                }
            }
        }
    }
    if let Some(rest) = name.strip_prefix("INIT_SPAN") {
        if let Ok(i) = rest.parse::<usize>() {
            if i < INIT_SPAN_POOL_COUNT {
                return Class::InitSpan(i);
            }
        }
    }
    Class::Unexpected
}

pub fn summarize(placed: &[PlacedStatic], spans_live: usize) -> Census {
    let mut fixed = Vec::new();
    let mut count = 0usize;
    for s in placed {
        match classify(&s.name) {
            Class::Fixed => {
                fixed.push(s.name.clone());
                count += 1;
            }
            Class::InitSpan(i) => {
                if i < spans_live {
                    count += 1;
                }
            }
            Class::Unexpected => {
                count += 1;
            }
        }
    }
    fixed.sort();
    fixed.dedup();
    Census {
        count,
        fixed,
        spans: spans_live,
    }
}

pub fn is_forbidden_rtconfig_static_name(name: &str) -> bool {
    if name.starts_with("WAKE_PEND") || name.starts_with("INIT_SLOT") {
        return true;
    }
    if let Some(rest) = name.strip_prefix("RING") {
        let digits = rest.bytes().take_while(|b| b.is_ascii_digit()).count();
        if digits > 0 {
            return rest.as_bytes().get(digits) == Some(&b'_');
        }
    }
    false
}

pub fn forbidden_statics_in_rtconfig(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        let Some(rest) = line.strip_prefix("pub static ") else {
            continue;
        };
        let Some(name) = rest.split(':').next() else {
            continue;
        };
        let name = name.trim();
        if is_forbidden_rtconfig_static_name(name) {
            out.push(name.to_string());
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn ps(name: &str) -> PlacedStatic {
        PlacedStatic {
            name: name.to_string(),
            ty: "T".to_string(),
            addr: 0,
            size: 8,
        }
    }

    #[test]
    fn fixed_set_len_matches_core_plus_mb_pool() {
        assert_eq!(fixed_core_names().len(), 19);
        assert_eq!(MB_POOL_COUNT, 32);
        assert_eq!(fixed_set_len(), 83);
        for n in fixed_core_names() {
            assert_eq!(classify(n), Class::Fixed, "{n}");
        }
        for i in 0..MB_POOL_COUNT {
            assert_eq!(classify(&format!("MB{i}_DATA")), Class::Fixed);
            assert_eq!(classify(&format!("MB{i}_CTL")), Class::Fixed);
        }
        assert_eq!(classify("MB32_DATA"), Class::Unexpected);
        assert_eq!(classify("INIT_SPAN0"), Class::InitSpan(0));
        assert_eq!(classify("INIT_SPAN7"), Class::InitSpan(7));
        assert_eq!(classify("INIT_SPAN8"), Class::Unexpected);
        assert_eq!(classify("RING0_DATA"), Class::Unexpected);
        assert_eq!(classify("WAKE_PEND0"), Class::Unexpected);
        assert_eq!(classify("INIT_SLOT0"), Class::Unexpected);
    }

    #[test]
    fn census_excludes_init_span_placeholders_from_count() {
        let mut placed = Vec::new();
        for n in fixed_core_names() {
            placed.push(ps(n));
        }
        for i in 0..MB_POOL_COUNT {
            placed.push(ps(&format!("MB{i}_DATA")));
            placed.push(ps(&format!("MB{i}_CTL")));
        }
        for i in 0..INIT_SPAN_POOL_COUNT {
            placed.push(ps(&format!("INIT_SPAN{i}")));
        }
        let spans = 2;
        let c = summarize(&placed, spans);
        assert_eq!(c.fixed.len(), fixed_set_len());
        assert_eq!(c.spans, spans);
        assert_eq!(c.count, fixed_set_len() + spans);
        assert!(c.within_ratchet());
        assert_eq!(
            c.render_line(),
            format!(
                "PlacedStatics count={} fixed={} spans={spans}",
                fixed_set_len() + spans,
                {
                    let mut names: Vec<String> = fixed_core_names().iter().cloned().collect();
                    for i in 0..MB_POOL_COUNT {
                        names.push(format!("MB{i}_CTL"));
                        names.push(format!("MB{i}_DATA"));
                    }
                    names.sort();
                    names.join(",")
                }
            )
        );
    }

    #[test]
    fn unexpected_name_on_full_fixed_set_breaks_the_ratchet() {
        let mut full = Vec::new();
        for n in fixed_core_names() {
            full.push(ps(n));
        }
        for i in 0..MB_POOL_COUNT {
            full.push(ps(&format!("MB{i}_DATA")));
            full.push(ps(&format!("MB{i}_CTL")));
        }
        full.push(ps("RING0_DATA"));
        let c = summarize(&full, 0);
        assert_eq!(c.count, fixed_set_len() + 1);
        assert!(!c.within_ratchet());
    }

    #[test]
    fn forbidden_name_detector() {
        assert!(is_forbidden_rtconfig_static_name("RING0_DATA"));
        assert!(is_forbidden_rtconfig_static_name("RING12_CTL"));
        assert!(is_forbidden_rtconfig_static_name("WAKE_PEND"));
        assert!(is_forbidden_rtconfig_static_name("WAKE_PEND0"));
        assert!(is_forbidden_rtconfig_static_name("INIT_SLOT0"));
        assert!(!is_forbidden_rtconfig_static_name("RINGS_CTL"));
        assert!(!is_forbidden_rtconfig_static_name("RINGS_DATA"));
        assert!(!is_forbidden_rtconfig_static_name("RING_STRIDE_WORDS"));
        assert!(!is_forbidden_rtconfig_static_name("RING_POOL_COUNT"));
        assert!(!is_forbidden_rtconfig_static_name("INIT_SPAN0"));
        assert!(!is_forbidden_rtconfig_static_name("WAKE"));
    }

    fn golden_root() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tests/golden")
    }

    #[test]
    fn golden_rtconfigs_forbid_numbered_ring_wake_init_slot_statics() {
        let root = golden_root();
        let mut hits: Vec<String> = Vec::new();
        let entries = std::fs::read_dir(&root).unwrap_or_else(|e| {
            panic!("read {}: {e}", root.display());
        });
        for entry in entries {
            let entry = entry.expect("read_dir entry");
            let case = entry.path();
            if !case.is_dir() {
                continue;
            }
            let path = case.join("expected/rtconfig.txt");
            if !path.exists() {
                continue;
            }
            let text = std::fs::read_to_string(&path).unwrap_or_else(|e| {
                panic!("read {}: {e}", path.display());
            });
            for name in forbidden_statics_in_rtconfig(&text) {
                hits.push(format!(
                    "{}/expected/rtconfig.txt: pub static {name}",
                    case.file_name().unwrap().to_string_lossy()
                ));
            }
        }
        assert!(
            hits.is_empty(),
            "numbered RING{{i}}_ / WAKE_PEND / INIT_SLOT statics returned in \
             generated rtconfig (plans/M12.md item G / decisions 890–893):\n  {}",
            hits.join("\n  ")
        );
    }

    #[test]
    fn golden_reports_with_placed_statics_carry_census_within_ratchet() {
        let root = golden_root();
        let mut failures: Vec<String> = Vec::new();
        let entries = std::fs::read_dir(&root).unwrap_or_else(|e| {
            panic!("read {}: {e}", root.display());
        });
        for entry in entries {
            let entry = entry.expect("read_dir entry");
            let case = entry.path();
            if !case.is_dir() {
                continue;
            }
            let path = case.join("expected/report.txt");
            if !path.exists() {
                continue;
            }
            let text = std::fs::read_to_string(&path).unwrap_or_else(|e| {
                panic!("read {}: {e}", path.display());
            });
            let has_placed = text
                .lines()
                .any(|l| l.trim_start().starts_with("PlacedStatic "));
            if !has_placed {
                continue;
            }
            let census_line = text
                .lines()
                .map(str::trim_start)
                .find(|l| l.starts_with("PlacedStatics count="));
            let Some(line) = census_line else {
                failures.push(format!(
                    "{}: has PlacedStatic lines but no PlacedStatics census line",
                    case.file_name().unwrap().to_string_lossy()
                ));
                continue;
            };
            let mut count = None;
            let mut spans = None;
            for part in line.split_whitespace() {
                if let Some(v) = part.strip_prefix("count=") {
                    count = v.parse::<usize>().ok();
                }
                if let Some(v) = part.strip_prefix("spans=") {
                    spans = v.parse::<usize>().ok();
                }
            }
            let (Some(n), Some(k)) = (count, spans) else {
                failures.push(format!(
                    "{}: cannot parse census line: {line}",
                    case.file_name().unwrap().to_string_lossy()
                ));
                continue;
            };
            let limit = fixed_set_len();
            if n > limit + k {
                failures.push(format!(
                    "{}: census ratchet failed: N={n} > fixed_set_len ({limit}) + spans ({k})",
                    case.file_name().unwrap().to_string_lossy()
                ));
            }
            for l in text.lines() {
                let l = l.trim_start();
                let Some(rest) = l.strip_prefix("PlacedStatic name=") else {
                    continue;
                };
                let name = rest.split_whitespace().next().unwrap_or("");
                match classify(name) {
                    Class::Fixed | Class::InitSpan(_) => {}
                    Class::Unexpected => {
                        failures.push(format!(
                            "{}: PlacedStatic `{name}` is outside the fixed set / \
                             INIT_SPAN pool (plans/M12.md item G)",
                            case.file_name().unwrap().to_string_lossy()
                        ));
                    }
                }
            }
        }
        assert!(
            failures.is_empty(),
            "placed-static census golden lock failed \
             (plans/M12.md item G / decisions 890–893):\n  {}",
            failures.join("\n  ")
        );
    }
}
