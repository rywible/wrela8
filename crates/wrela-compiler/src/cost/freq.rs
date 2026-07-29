//! Pinned Lane-1 method-grain frequency fixtures (integrity Item J).
//!
//! Dump cannot boot a guest. Boot-bearing cases commit a sidecar
//! `lane1-freq.txt` next to `input.wr`, generated from Lane 1
//! `lane1 hits=` transcript lines (flat index → method key resolved
//! offline). Score compose multiplies these counts by per-fn schedule
//! lengths (`Σ f × s_fn`).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// One measured workload's method-grain frequency vector.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MethodFreq {
    /// Workload name (must match a `[name]` in `workloads.toml`).
    pub workload: String,
    /// Method key (`Actor.method`) → call count.
    pub counts: BTreeMap<String, u64>,
}

/// Sidecar path: `<dir>/lane1-freq.txt` next to `input.wr`, if present.
pub fn sibling_freq_path(source: &Path) -> Option<PathBuf> {
    let parent = source.parent()?;
    let p = parent.join("lane1-freq.txt");
    p.is_file().then_some(p)
}

/// Load and parse a frequency fixture.
pub fn load_from_path(path: &Path) -> Result<MethodFreq, String> {
    let text =
        std::fs::read_to_string(path).map_err(|e| format!("freq: read {}: {e}", path.display()))?;
    parse(&text).map_err(|e| format!("freq {}: {e}", path.display()))
}

/// Parse fixture text.
///
/// Lines: `#` comments / blanks ignored; required `workload=<name>`;
/// then `<Method.key>=<u64>` rows (count ≥ 1). Unknown keys fail closed
/// only at compose time (missing scored fn → uncovered hit).
pub fn parse(text: &str) -> Result<MethodFreq, String> {
    let mut workload: Option<String> = None;
    let mut counts: BTreeMap<String, u64> = BTreeMap::new();
    for (lineno, raw) in text.lines().enumerate() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let (k, v) = line
            .split_once('=')
            .ok_or_else(|| format!("line {}: expected key=value, got `{line}`", lineno + 1))?;
        let key = k.trim();
        let val = v.trim();
        if key.is_empty() {
            return Err(format!("line {}: empty key", lineno + 1));
        }
        if key == "workload" {
            if val.is_empty() {
                return Err(format!("line {}: empty workload name", lineno + 1));
            }
            if workload.is_some() {
                return Err(format!("line {}: duplicate workload=", lineno + 1));
            }
            workload = Some(val.to_string());
            continue;
        }
        let n: u64 = val
            .parse()
            .map_err(|_| format!("line {}: count must be u64, got `{val}`", lineno + 1))?;
        if n < 1 {
            return Err(format!("line {}: count must be >= 1, got {n}", lineno + 1));
        }
        if counts.insert(key.to_string(), n).is_some() {
            return Err(format!("line {}: duplicate method `{key}`", lineno + 1));
        }
    }
    let workload = workload.ok_or_else(|| "missing workload=<name>".to_string())?;
    if counts.is_empty() {
        return Err("no method frequency rows".to_string());
    }
    Ok(MethodFreq { workload, counts })
}

#[cfg(test)]
mod tests {
    use super::*;

    const BOOT: &str = r#"
# lane1 hits=0:3,1:1,2:3,3:2,4:2
workload=boot-actors
Ledger.mark=3
Ledger.read_marks=1
Worker.slow=3
Worker.quick=2
Worker.report=2
"#;

    #[test]
    fn parse_boot_actors_fixture() {
        let f = parse(BOOT).expect("parse");
        assert_eq!(f.workload, "boot-actors");
        assert_eq!(f.counts["Ledger.mark"], 3);
        assert_eq!(f.counts["Worker.report"], 2);
        assert_eq!(f.counts.len(), 5);
    }

    #[test]
    fn reject_missing_workload() {
        let err = parse("Ledger.mark=1\n").expect_err("missing");
        assert!(err.contains("workload"), "got: {err}");
    }

    #[test]
    fn reject_zero_count() {
        let err = parse("workload=w\nFoo.bar=0\n").expect_err("zero");
        assert!(err.contains(">= 1"), "got: {err}");
    }

    #[test]
    fn load_committed_boot_actors_fixture() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .ancestors()
            .nth(2)
            .unwrap()
            .join("tests/golden/boot-actors/lane1-freq.txt");
        let f = load_from_path(&path).expect("committed fixture");
        assert_eq!(f.workload, "boot-actors");
        assert_eq!(f.counts.get("Ledger.mark"), Some(&3));
    }
}
