use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MethodFreq {
    pub workload: String,
    pub counts: BTreeMap<String, u64>,
}

pub fn sibling_freq_path(source: &Path) -> Option<PathBuf> {
    let parent = source.parent()?;
    let p = parent.join("lane1-freq.txt");
    p.is_file().then_some(p)
}

pub fn sibling_block_freq_path(source: &Path) -> Option<PathBuf> {
    let parent = source.parent()?;
    let p = parent.join("lane2-freq.txt");
    p.is_file().then_some(p)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BlockFreq {
    pub workload: String,
    pub counts: BTreeMap<String, u64>,
}

pub fn load_block_from_path(path: &Path) -> Result<BlockFreq, String> {
    let text = std::fs::read_to_string(path)
        .map_err(|e| format!("block freq: read {}: {e}", path.display()))?;
    parse_block(&text).map_err(|e| format!("block freq {}: {e}", path.display()))
}

pub fn parse_block(text: &str) -> Result<BlockFreq, String> {
    let m = parse(text)?;
    for key in m.counts.keys() {
        super::bridge::split_key(key)?;
    }
    Ok(BlockFreq {
        workload: m.workload,
        counts: m.counts,
    })
}

pub fn load_from_path(path: &Path) -> Result<MethodFreq, String> {
    let text =
        std::fs::read_to_string(path).map_err(|e| format!("freq: read {}: {e}", path.display()))?;
    parse(&text).map_err(|e| format!("freq {}: {e}", path.display()))
}

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

    #[test]
    fn parse_block_requires_well_formed_block_keys() {
        let f = parse_block("workload=w\nFoo.bar#0=3\nFoo.bar#12=1\n").expect("parse");
        assert_eq!(f.workload, "w");
        assert_eq!(f.counts["Foo.bar#12"], 1);

        let err = parse_block("workload=w\nFoo.bar=3\n").expect_err("no #");
        assert!(err.contains("<fn_key>#<block_index>"), "got: {err}");
        let err = parse_block("workload=w\nFoo.bar#x=3\n").expect_err("bad index");
        assert!(err.contains("must be u32"), "got: {err}");
        let err = parse_block("workload=w\n#0=3\n").expect_err("comment, not a key");
        assert!(err.contains("no method frequency rows"), "got: {err}");
    }

    #[test]
    fn load_committed_boot_actors_block_fixture() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .ancestors()
            .nth(2)
            .unwrap()
            .join("tests/golden/boot-actors/lane2-freq.txt");
        let f = load_block_from_path(&path).expect("committed fixture");
        assert_eq!(f.workload, "boot-actors");
        assert_eq!(
            f.counts.len(),
            216,
            "the committed vector is the bounded production-window non-zero set"
        );
        assert_eq!(f.counts.values().sum::<u64>(), 1512);
        assert_eq!(f.counts.get("Ledger.mark#0"), Some(&3));
        assert!(f.counts.keys().all(|k| k.contains('#')));
    }

    #[test]
    fn sibling_block_freq_path_finds_the_committed_sidecar() {
        let input = Path::new(env!("CARGO_MANIFEST_DIR"))
            .ancestors()
            .nth(2)
            .unwrap()
            .join("tests/golden/boot-actors/input.wr");
        assert!(sibling_block_freq_path(&input).is_some());
        let none = Path::new(env!("CARGO_MANIFEST_DIR"))
            .ancestors()
            .nth(2)
            .unwrap()
            .join("tests/golden/cost-arith/input.wr");
        assert!(sibling_block_freq_path(&none).is_none());
    }
}
