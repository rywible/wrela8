//! `wrela-cost-v1` ISA latency table (`bench/wrela-cost-v1.toml`).
//!
//! Differential ranking only — not host wall time, not A76 SOG ports
//! (plans/M18.md item B).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use super::CostRule;

/// Expected `version` field in `wrela-cost-v1.toml`.
pub const EXPECTED_VERSION: u64 = 1;

/// Default when `issue_width` is omitted from the table file.
pub const DEFAULT_ISSUE_WIDTH: u64 = 4;

/// Parsed ISA latency table for proxy-cycle ranking.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CostTable {
    pub version: u64,
    pub issue_width: u64,
    /// Latency in proxy-cycles for every `CostRule` in `ALL`.
    latencies: BTreeMap<&'static str, u64>,
}

impl CostTable {
    /// Latency for `rule`. Panics only if the table was built without that
    /// key — `parse` / `load_*` always require every `CostRule::ALL` key.
    pub fn latency(&self, rule: CostRule) -> u64 {
        *self
            .latencies
            .get(rule.as_str())
            .unwrap_or_else(|| panic!("cost table missing rule {}", rule.as_str()))
    }

    /// Stable hex digest of canonical table content (version, issue_width,
    /// sorted `key=value` latency lines). FNV-1a 64-bit; not cryptographic.
    pub fn table_digest(&self) -> String {
        let mut lines: Vec<String> = Vec::with_capacity(2 + self.latencies.len());
        lines.push(format!("version={}", self.version));
        lines.push(format!("issue_width={}", self.issue_width));
        for (k, v) in &self.latencies {
            lines.push(format!("{k}={v}"));
        }
        // latencies are already BTreeMap-ordered; version/issue_width first.
        let mut h = Fnv64::new();
        for (i, line) in lines.iter().enumerate() {
            if i > 0 {
                h.write(b"\n");
            }
            h.write(line.as_bytes());
        }
        format!("{:016x}", h.finish())
    }
}

/// Workspace-relative path to the committed v1 table, or `WRELA_COST_TABLE`
/// when set. Resolves repo root from `CARGO_MANIFEST_DIR` (this crate lives
/// at `crates/wrela-compiler`).
pub fn default_table_path() -> PathBuf {
    if let Ok(p) = std::env::var("WRELA_COST_TABLE") {
        return PathBuf::from(p);
    }
    repo_root().join("bench/wrela-cost-v1.toml")
}

/// Load and parse the default table path. Fail closed if missing/malformed.
pub fn load_default() -> Result<CostTable, String> {
    load_from_path(&default_table_path())
}

/// Load and parse a cost table from `path`.
pub fn load_from_path(path: &Path) -> Result<CostTable, String> {
    let text = std::fs::read_to_string(path).map_err(|e| {
        format!(
            "cost table: read {}: {e} (expected workspace-relative \
             bench/wrela-cost-v1.toml)",
            path.display()
        )
    })?;
    parse(&text).map_err(|e| format!("cost table {}: {e}", path.display()))
}

/// Parse table TOML text.
pub fn parse(text: &str) -> Result<CostTable, String> {
    let value: toml::Value = text.parse().map_err(|e| format!("parse failed: {e}"))?;

    let version = value
        .get("version")
        .and_then(|v| v.as_integer())
        .ok_or_else(|| "missing integer `version`".to_string())?;
    if version < 0 {
        return Err(format!("bad version {version}: must be >= 0"));
    }
    let version = version as u64;
    if version != EXPECTED_VERSION {
        return Err(format!(
            "bad version {version}: expected {EXPECTED_VERSION}"
        ));
    }

    let issue_width = match value.get("issue_width") {
        None => DEFAULT_ISSUE_WIDTH,
        Some(v) => {
            let n = v
                .as_integer()
                .ok_or_else(|| "`issue_width` must be an integer".to_string())?;
            if n <= 0 {
                return Err(format!("`issue_width` must be > 0, got {n}"));
            }
            n as u64
        }
    };

    let latency_tbl = value
        .get("latency")
        .and_then(|v| v.as_table())
        .ok_or_else(|| "missing `[latency]` table".to_string())?;

    let mut latencies: BTreeMap<&'static str, u64> = BTreeMap::new();
    for (key, val) in latency_tbl {
        let rule = CostRule::from_str(key).ok_or_else(|| format!("unknown latency key `{key}`"))?;
        let n = val
            .as_integer()
            .ok_or_else(|| format!("latency.{key} must be an integer"))?;
        if n < 0 {
            return Err(format!("latency.{key} must be >= 0, got {n}"));
        }
        latencies.insert(rule.as_str(), n as u64);
    }

    for &rule in CostRule::ALL {
        if !latencies.contains_key(rule.as_str()) {
            return Err(format!(
                "missing latency for CostRule key `{}`",
                rule.as_str()
            ));
        }
    }

    Ok(CostTable {
        version,
        issue_width,
        latencies,
    })
}

fn repo_root() -> PathBuf {
    // crates/wrela-compiler → repo root (same shape as xtask `root()`).
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("repo root from crates/wrela-compiler")
        .to_path_buf()
}

/// FNV-1a 64-bit — deterministic, std-only, fine for table digests.
struct Fnv64(u64);

impl Fnv64 {
    fn new() -> Self {
        Fnv64(0xcbf29ce484222325)
    }

    fn write(&mut self, bytes: &[u8]) {
        let mut h = self.0;
        for &b in bytes {
            h ^= u64::from(b);
            h = h.wrapping_mul(0x100000001b3);
        }
        self.0 = h;
    }

    fn finish(self) -> u64 {
        self.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MINIMAL: &str = r#"
version = 1
issue_width = 4
[latency]
alu = 1
load = 4
store = 1
branch = 1
call = 1
abort = 1
abort_val = 1
mov_wide = 1
mul = 3
sdiv = 12
udiv = 12
adrp = 1
barrier = 1
system = 1
neon = 1
"#;

    #[test]
    fn parse_and_digest_stable() {
        let a = parse(MINIMAL).expect("parse");
        let b = parse(MINIMAL).expect("parse again");
        assert_eq!(a.version, 1);
        assert_eq!(a.issue_width, 4);
        assert_eq!(a.latency(CostRule::Load), 4);
        assert_eq!(a.latency(CostRule::Sdiv), 12);
        assert_eq!(a.table_digest(), b.table_digest());
        assert_eq!(a.table_digest().len(), 16);
        assert!(a.table_digest().chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn issue_width_defaults_when_omitted() {
        let text = MINIMAL.replace("issue_width = 4\n", "");
        let t = parse(&text).expect("parse without issue_width");
        assert_eq!(t.issue_width, DEFAULT_ISSUE_WIDTH);
    }

    #[test]
    fn reject_missing_key() {
        let text = MINIMAL.replace("neon = 1\n", "");
        let err = parse(&text).expect_err("missing neon");
        assert!(
            err.contains("neon"),
            "error should name missing key, got: {err}"
        );
    }

    #[test]
    fn reject_bad_version() {
        let text = MINIMAL.replace("version = 1", "version = 99");
        let err = parse(&text).expect_err("bad version");
        assert!(
            err.contains("version") && err.contains("99"),
            "error should cite bad version, got: {err}"
        );
    }

    #[test]
    fn load_committed_table() {
        let t = load_default().expect("load bench/wrela-cost-v1.toml");
        assert_eq!(t.version, 1);
        assert_eq!(t.issue_width, 4);
        assert_eq!(t.latency(CostRule::Alu), 1);
        assert_eq!(t.latency(CostRule::Mul), 3);
        // Digest must be stable across loads of the same file.
        let again = load_default().expect("reload");
        assert_eq!(t.table_digest(), again.table_digest());
    }
}
