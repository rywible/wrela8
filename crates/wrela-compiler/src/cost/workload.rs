//! Pinned workload set (`bench/workloads.toml`).
//!
//! Named workloads with integer weights for multi-W proxy ranking. `[flat]`
//! is required (`f≡1` policy row). Parse + digest here; score compose is
//! `cost::compose` (integrity Item J).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use super::{Fnv64, repo_root};

/// Only key allowed inside a workload table.
const WEIGHT_KEY: &str = "weight";

/// Required workload name: static ruler, `f≡1`.
pub const FLAT_NAME: &str = "flat";

/// Parsed pinned workload set (name → weight).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkloadSet {
    /// Sorted by name; always contains `flat`.
    weights: BTreeMap<String, u64>,
}

impl WorkloadSet {
    /// Weight for `name`, if present.
    pub fn weight(&self, name: &str) -> Option<u64> {
        self.weights.get(name).copied()
    }

    /// Weight of the required `flat` row.
    pub fn flat_weight(&self) -> u64 {
        *self
            .weights
            .get(FLAT_NAME)
            .expect("WorkloadSet always contains flat after parse")
    }

    /// All workload names in sorted order.
    pub fn names(&self) -> impl Iterator<Item = &str> {
        self.weights.keys().map(|s| s.as_str())
    }

    /// Number of named workloads (including `flat`).
    pub fn len(&self) -> usize {
        self.weights.len()
    }

    /// True when no workloads (unreachable after a successful parse).
    pub fn is_empty(&self) -> bool {
        self.weights.is_empty()
    }

    /// Stable hex digest of canonical content (sorted `name=weight` lines).
    /// FNV-1a 64-bit; not cryptographic. Same discipline as `CostTable::table_digest`.
    pub fn digest(&self) -> String {
        let mut h = Fnv64::new();
        for (i, (name, weight)) in self.weights.iter().enumerate() {
            if i > 0 {
                h.write(b"\n");
            }
            h.write(format!("{name}={weight}").as_bytes());
        }
        format!("{:016x}", h.finish())
    }
}

/// Workspace-relative path to the committed set, or `WRELA_WORKLOADS`
/// when set. Resolves repo root from `CARGO_MANIFEST_DIR`.
pub fn default_workloads_path() -> PathBuf {
    if let Ok(p) = std::env::var("WRELA_WORKLOADS") {
        return PathBuf::from(p);
    }
    repo_root().join("bench/workloads.toml")
}

/// Load and parse the default workloads path. Fail closed if missing/malformed.
pub fn load_default() -> Result<WorkloadSet, String> {
    load_from_path(&default_workloads_path())
}

/// Load and parse a workloads file from `path`.
pub fn load_from_path(path: &Path) -> Result<WorkloadSet, String> {
    let text = std::fs::read_to_string(path).map_err(|e| {
        format!(
            "workloads: read {}: {e} (expected workspace-relative \
             bench/workloads.toml)",
            path.display()
        )
    })?;
    parse(&text).map_err(|e| format!("workloads {}: {e}", path.display()))
}

/// Parse workloads TOML text.
///
/// Fail closed: missing `[flat]`, missing/invalid `weight`, unknown keys
/// inside a workload table, or non-table top-level keys.
pub fn parse(text: &str) -> Result<WorkloadSet, String> {
    let value: toml::Value = text.parse().map_err(|e| format!("parse failed: {e}"))?;
    let root = value
        .as_table()
        .ok_or_else(|| "root must be a table".to_string())?;

    let mut weights: BTreeMap<String, u64> = BTreeMap::new();
    for (name, val) in root {
        let tbl = val.as_table().ok_or_else(|| {
            format!("unknown key `{name}`: workload entries must be tables `[name]`")
        })?;
        for (key, _) in tbl {
            if key != WEIGHT_KEY {
                return Err(format!("unknown key `{key}` in [{name}]"));
            }
        }
        let weight_val = tbl
            .get(WEIGHT_KEY)
            .ok_or_else(|| format!("missing {name}.weight"))?;
        let n = weight_val
            .as_integer()
            .ok_or_else(|| format!("{name}.weight must be an integer"))?;
        if n < 1 {
            return Err(format!("{name}.weight must be >= 1, got {n}"));
        }
        weights.insert(name.clone(), n as u64);
    }

    if !weights.contains_key(FLAT_NAME) {
        return Err("missing required `[flat]` workload".to_string());
    }

    Ok(WorkloadSet { weights })
}

#[cfg(test)]
mod tests {
    use super::*;

    const MINIMAL: &str = r#"
[flat]
weight = 1
[boot-actors]
weight = 10
"#;

    #[test]
    fn parse_and_digest_stable() {
        let a = parse(MINIMAL).expect("parse");
        let b = parse(MINIMAL).expect("parse again");
        assert_eq!(a.flat_weight(), 1);
        assert_eq!(a.weight("boot-actors"), Some(10));
        assert_eq!(a.len(), 2);
        assert_eq!(a.digest(), b.digest());
        assert_eq!(a.digest().len(), 16);
        assert!(a.digest().chars().all(|c| c.is_ascii_hexdigit()));
        // Canonical order is name-sorted: boot-actors before flat.
        let names: Vec<_> = a.names().collect();
        assert_eq!(names, vec!["boot-actors", "flat"]);
    }

    #[test]
    fn digest_independent_of_source_order() {
        let flipped = r#"
[boot-actors]
weight = 10
[flat]
weight = 1
"#;
        let a = parse(MINIMAL).expect("minimal");
        let b = parse(flipped).expect("flipped");
        assert_eq!(a, b);
        assert_eq!(a.digest(), b.digest());
    }

    #[test]
    fn reject_missing_flat() {
        let text = r#"
[boot-actors]
weight = 10
"#;
        let err = parse(text).expect_err("missing flat");
        assert!(
            err.contains("flat"),
            "error should name missing flat, got: {err}"
        );
    }

    #[test]
    fn reject_unknown_key_in_table() {
        let text = r#"
[flat]
weight = 1
extra = 2
"#;
        let err = parse(text).expect_err("unknown key");
        assert!(
            err.contains("extra"),
            "error should name unknown key, got: {err}"
        );
    }

    #[test]
    fn reject_non_table_top_level() {
        let text = r#"
version = 1
[flat]
weight = 1
"#;
        let err = parse(text).expect_err("non-table top-level");
        assert!(
            err.contains("version"),
            "error should name unknown top-level key, got: {err}"
        );
    }

    #[test]
    fn reject_missing_weight() {
        let text = r#"
[flat]
"#;
        let err = parse(text).expect_err("missing weight");
        assert!(
            err.contains("weight"),
            "error should name missing weight, got: {err}"
        );
    }

    #[test]
    fn reject_zero_weight() {
        let text = r#"
[flat]
weight = 0
"#;
        let err = parse(text).expect_err("zero weight");
        assert!(
            err.contains("weight") && err.contains('0'),
            "error should cite weight floor, got: {err}"
        );
    }

    #[test]
    fn load_committed_workloads() {
        let w = load_default().expect("load bench/workloads.toml");
        assert_eq!(w.flat_weight(), 1);
        assert_eq!(w.weight("boot-actors"), Some(10));
        let again = load_default().expect("reload");
        assert_eq!(w.digest(), again.digest());
    }
}
