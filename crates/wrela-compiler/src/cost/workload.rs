use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use super::{Fnv64, repo_root};

const WEIGHT_KEY: &str = "weight";
const SOURCE_KEY: &str = "source";

pub const FLAT_NAME: &str = "flat";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkloadSet {
    weights: BTreeMap<String, u64>,
    sources: BTreeMap<String, PathBuf>,
}

impl WorkloadSet {
    pub fn weight(&self, name: &str) -> Option<u64> {
        self.weights.get(name).copied()
    }

    pub fn source(&self, name: &str) -> Option<&Path> {
        self.sources.get(name).map(PathBuf::as_path)
    }

    pub fn source_path(&self, name: &str) -> Option<PathBuf> {
        self.source(name).map(|path| repo_root().join(path))
    }

    pub fn flat_weight(&self) -> u64 {
        *self
            .weights
            .get(FLAT_NAME)
            .expect("WorkloadSet always contains flat after parse")
    }

    pub fn names(&self) -> impl Iterator<Item = &str> {
        self.weights.keys().map(|s| s.as_str())
    }

    pub fn len(&self) -> usize {
        self.weights.len()
    }

    pub fn is_empty(&self) -> bool {
        self.weights.is_empty()
    }

    pub fn digest(&self) -> String {
        let mut h = Fnv64::new();
        for (i, (name, weight)) in self.weights.iter().enumerate() {
            if i > 0 {
                h.write(b"\n");
            }
            let source = self
                .sources
                .get(name)
                .map(|path| path.to_string_lossy())
                .unwrap_or_default();
            h.write(format!("{name}={weight}@{source}").as_bytes());
        }
        format!("{:016x}", h.finish())
    }
}

pub fn default_workloads_path() -> PathBuf {
    if let Ok(p) = std::env::var("WRELA_WORKLOADS") {
        return PathBuf::from(p);
    }
    repo_root().join("bench/workloads.toml")
}

pub fn load_default() -> Result<WorkloadSet, String> {
    load_from_path(&default_workloads_path())
}

pub fn load_from_path(path: &Path) -> Result<WorkloadSet, String> {
    let text = std::fs::read_to_string(path).map_err(|e| {
        format!(
            "workloads: read {}: {e} (expected workspace-relative \
             bench/workloads.toml)",
            path.display()
        )
    })?;
    let set = parse(&text).map_err(|e| format!("workloads {}: {e}", path.display()))?;
    for name in set.names().filter(|name| *name != FLAT_NAME) {
        let source = set
            .source_path(name)
            .ok_or_else(|| format!("workloads {}: missing {name}.source", path.display()))?;
        if !source.is_file() {
            return Err(format!(
                "workloads {}: {name}.source does not name a file: {}",
                path.display(),
                source.display()
            ));
        }
    }
    Ok(set)
}

pub fn parse(text: &str) -> Result<WorkloadSet, String> {
    let value: toml::Value = text.parse().map_err(|e| format!("parse failed: {e}"))?;
    let root = value
        .as_table()
        .ok_or_else(|| "root must be a table".to_string())?;

    let mut weights: BTreeMap<String, u64> = BTreeMap::new();
    let mut sources: BTreeMap<String, PathBuf> = BTreeMap::new();
    for (name, val) in root {
        let tbl = val.as_table().ok_or_else(|| {
            format!("unknown key `{name}`: workload entries must be tables `[name]`")
        })?;
        for (key, _) in tbl {
            if key != WEIGHT_KEY && key != SOURCE_KEY {
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
        match tbl.get(SOURCE_KEY) {
            Some(value) => {
                let source = value
                    .as_str()
                    .ok_or_else(|| format!("{name}.source must be a string"))?;
                let path = PathBuf::from(source);
                if path.is_absolute()
                    || path
                        .components()
                        .any(|component| matches!(component, std::path::Component::ParentDir))
                {
                    return Err(format!(
                        "{name}.source must be a repository-relative path without `..`"
                    ));
                }
                sources.insert(name.clone(), path);
            }
            None => {}
        }
    }

    if !weights.contains_key(FLAT_NAME) {
        return Err("missing required `[flat]` workload".to_string());
    }
    if sources.contains_key(FLAT_NAME) {
        return Err("[flat] must not declare a source; it is the static ruler".to_string());
    }

    Ok(WorkloadSet { weights, sources })
}

#[cfg(test)]
mod tests {
    use super::*;

    const MINIMAL: &str = r#"
[flat]
weight = 1
[boot-actors]
weight = 10
source = "tests/golden/boot-actors/input.wr"
"#;

    #[test]
    fn parse_and_digest_stable() {
        let a = parse(MINIMAL).expect("parse");
        let b = parse(MINIMAL).expect("parse again");
        assert_eq!(a.flat_weight(), 1);
        assert_eq!(a.weight("boot-actors"), Some(10));
        assert_eq!(
            a.source("boot-actors"),
            Some(Path::new("tests/golden/boot-actors/input.wr"))
        );
        assert_eq!(a.len(), 2);
        assert_eq!(a.digest(), b.digest());
        assert_eq!(a.digest().len(), 16);
        assert!(a.digest().chars().all(|c| c.is_ascii_hexdigit()));
        let names: Vec<_> = a.names().collect();
        assert_eq!(names, vec!["boot-actors", "flat"]);
    }

    #[test]
    fn digest_independent_of_source_order() {
        let flipped = r#"
[boot-actors]
weight = 10
source = "tests/golden/boot-actors/input.wr"
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
source = "tests/golden/boot-actors/input.wr"
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
        assert!(w.source_path("boot-actors").unwrap().is_file());
        let again = load_default().expect("reload");
        assert_eq!(w.digest(), again.digest());
    }
}
