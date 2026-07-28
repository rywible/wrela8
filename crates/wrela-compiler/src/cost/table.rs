//! `wrela-cost-v1` ISA latency table (`bench/wrela-cost-v1.toml`).
//!
//! Differential ranking only — not host wall time, not A76 SOG ports.
//! Hard-cut schema `version = 2`: dual ports, max issue, branch penalty,
//! mem reuse / working-set knobs (no `issue_width`).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use super::CostRule;

/// Expected `version` field in `wrela-cost-v1.toml`.
pub const EXPECTED_VERSION: u64 = 2;

/// Required integer keys under `[ports]`.
const PORT_KEYS: &[&str] = &[
    "alu",
    "mem",
    "max_issue_per_cycle",
    "branch_penalty",
    "mem_reuse_window",
    "mem_working_set_cap",
];

/// Required integer keys under `[mem]`.
const MEM_KEYS: &[&str] = &[
    "load_stack_hit",
    "load_stack_miss",
    "load_cold_hit",
    "load_cold_miss",
    "store_stack",
    "store_cold",
    "working_set_surcharge",
];

/// Hit/miss / store / surcharge latencies for the emit-time mem model.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MemCosts {
    pub load_stack_hit: u64,
    pub load_stack_miss: u64,
    pub load_cold_hit: u64,
    pub load_cold_miss: u64,
    pub store_stack: u64,
    pub store_cold: u64,
    pub working_set_surcharge: u64,
}

/// Parsed ISA cost table for proxy-cycle ranking (schema version 2).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CostTable {
    pub version: u64,
    /// ALU issue ports per cycle.
    pub alu_ports: u64,
    /// Mem issue ports per cycle.
    pub mem_ports: u64,
    /// Global issue cap (ports ∩ this).
    pub max_issue_per_cycle: u64,
    /// Added to branch finish latency.
    pub branch_penalty: u64,
    /// Recent Load/Store op count window (not cache KB).
    pub mem_reuse_window: u64,
    /// Distinct keys tolerated in the window before surcharge.
    pub mem_working_set_cap: u64,
    pub mem: MemCosts,
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

    /// Stable hex digest of canonical table content (version, all ports /
    /// mem knobs, sorted `key=value` latency lines). FNV-1a 64-bit; not
    /// cryptographic.
    pub fn table_digest(&self) -> String {
        let mut lines: Vec<String> =
            Vec::with_capacity(1 + PORT_KEYS.len() + MEM_KEYS.len() + self.latencies.len());
        lines.push(format!("version={}", self.version));
        lines.push(format!("ports.alu={}", self.alu_ports));
        lines.push(format!("ports.mem={}", self.mem_ports));
        lines.push(format!(
            "ports.max_issue_per_cycle={}",
            self.max_issue_per_cycle
        ));
        lines.push(format!("ports.branch_penalty={}", self.branch_penalty));
        lines.push(format!("ports.mem_reuse_window={}", self.mem_reuse_window));
        lines.push(format!(
            "ports.mem_working_set_cap={}",
            self.mem_working_set_cap
        ));
        lines.push(format!("mem.load_stack_hit={}", self.mem.load_stack_hit));
        lines.push(format!("mem.load_stack_miss={}", self.mem.load_stack_miss));
        lines.push(format!("mem.load_cold_hit={}", self.mem.load_cold_hit));
        lines.push(format!("mem.load_cold_miss={}", self.mem.load_cold_miss));
        lines.push(format!("mem.store_stack={}", self.mem.store_stack));
        lines.push(format!("mem.store_cold={}", self.mem.store_cold));
        lines.push(format!(
            "mem.working_set_surcharge={}",
            self.mem.working_set_surcharge
        ));
        for (k, v) in &self.latencies {
            lines.push(format!("{k}={v}"));
        }
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

/// Workspace-relative path to the committed table, or `WRELA_COST_TABLE`
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

/// Parse table TOML text (schema version 2).
pub fn parse(text: &str) -> Result<CostTable, String> {
    let value: toml::Value = text.parse().map_err(|e| format!("parse failed: {e}"))?;

    if value.get("issue_width").is_some() {
        return Err(
            "`issue_width` is rejected (hard-cut v2; use [ports] max_issue_per_cycle)".to_string(),
        );
    }

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

    let ports_tbl = value
        .get("ports")
        .and_then(|v| v.as_table())
        .ok_or_else(|| "missing `[ports]` table".to_string())?;
    for (key, _) in ports_tbl {
        if !PORT_KEYS.contains(&key.as_str()) {
            return Err(format!("unknown ports key `{key}`"));
        }
    }
    let alu_ports = req_u64_ports(ports_tbl, "alu")?;
    let mem_ports = req_u64_ports(ports_tbl, "mem")?;
    let max_issue_per_cycle = req_u64_ports(ports_tbl, "max_issue_per_cycle")?;
    let branch_penalty = req_u64_nonneg(ports_tbl, "branch_penalty", "ports")?;
    let mem_reuse_window = req_u64_ports(ports_tbl, "mem_reuse_window")?;
    let mem_working_set_cap = req_u64_nonneg(ports_tbl, "mem_working_set_cap", "ports")?;
    if alu_ports == 0 || mem_ports == 0 || max_issue_per_cycle == 0 || mem_reuse_window == 0 {
        return Err(
            "[ports] alu, mem, max_issue_per_cycle, mem_reuse_window must be > 0".to_string(),
        );
    }

    let mem_tbl = value
        .get("mem")
        .and_then(|v| v.as_table())
        .ok_or_else(|| "missing `[mem]` table".to_string())?;
    for (key, _) in mem_tbl {
        if !MEM_KEYS.contains(&key.as_str()) {
            return Err(format!("unknown mem key `{key}`"));
        }
    }
    let mem = MemCosts {
        load_stack_hit: req_u64_nonneg(mem_tbl, "load_stack_hit", "mem")?,
        load_stack_miss: req_u64_nonneg(mem_tbl, "load_stack_miss", "mem")?,
        load_cold_hit: req_u64_nonneg(mem_tbl, "load_cold_hit", "mem")?,
        load_cold_miss: req_u64_nonneg(mem_tbl, "load_cold_miss", "mem")?,
        store_stack: req_u64_nonneg(mem_tbl, "store_stack", "mem")?,
        store_cold: req_u64_nonneg(mem_tbl, "store_cold", "mem")?,
        working_set_surcharge: req_u64_nonneg(mem_tbl, "working_set_surcharge", "mem")?,
    };
    if mem.load_stack_hit == 0 || mem.load_cold_hit == 0 {
        return Err("[mem] hit latencies must be >= 1 (no free reloads)".to_string());
    }
    if mem.load_stack_miss >= mem.load_cold_miss {
        return Err("[mem] load_stack_miss must be < load_cold_miss".to_string());
    }

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
        alu_ports,
        mem_ports,
        max_issue_per_cycle,
        branch_penalty,
        mem_reuse_window,
        mem_working_set_cap,
        mem,
        latencies,
    })
}

fn req_u64_ports(tbl: &toml::map::Map<String, toml::Value>, key: &str) -> Result<u64, String> {
    req_u64_nonneg(tbl, key, "ports")
}

fn req_u64_nonneg(
    tbl: &toml::map::Map<String, toml::Value>,
    key: &str,
    section: &str,
) -> Result<u64, String> {
    let val = tbl
        .get(key)
        .ok_or_else(|| format!("missing {section}.{key}"))?;
    let n = val
        .as_integer()
        .ok_or_else(|| format!("{section}.{key} must be an integer"))?;
    if n < 0 {
        return Err(format!("{section}.{key} must be >= 0, got {n}"));
    }
    Ok(n as u64)
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
version = 2
[ports]
alu = 2
mem = 2
max_issue_per_cycle = 2
branch_penalty = 3
mem_reuse_window = 8
mem_working_set_cap = 4
[latency]
alu = 1
load = 12
store = 2
branch = 1
call = 4
abort = 1
abort_val = 3
mov_wide = 1
mul = 3
sdiv = 12
udiv = 12
adrp = 1
barrier = 1
system = 1
neon = 1
[mem]
load_stack_hit = 1
load_stack_miss = 4
load_cold_hit = 4
load_cold_miss = 12
store_stack = 1
store_cold = 2
working_set_surcharge = 2
"#;

    #[test]
    fn parse_and_digest_stable() {
        let a = parse(MINIMAL).expect("parse");
        let b = parse(MINIMAL).expect("parse again");
        assert_eq!(a.version, 2);
        assert_eq!(a.alu_ports, 2);
        assert_eq!(a.mem_ports, 2);
        assert_eq!(a.max_issue_per_cycle, 2);
        assert_eq!(a.branch_penalty, 3);
        assert_eq!(a.mem_reuse_window, 8);
        assert_eq!(a.mem_working_set_cap, 4);
        assert_eq!(a.latency(CostRule::Load), 12);
        assert_eq!(a.latency(CostRule::Call), 4);
        assert_eq!(a.latency(CostRule::AbortVal), 3);
        assert_eq!(a.mem.load_cold_miss, 12);
        assert_eq!(a.mem.working_set_surcharge, 2);
        assert_eq!(a.table_digest(), b.table_digest());
        assert_eq!(a.table_digest().len(), 16);
        assert!(a.table_digest().chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn reject_issue_width() {
        let text = format!("issue_width = 4\n{MINIMAL}");
        let err = parse(&text).expect_err("issue_width rejected");
        assert!(
            err.contains("issue_width"),
            "error should name issue_width, got: {err}"
        );
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
    fn reject_missing_ports_key() {
        let text = MINIMAL.replace("branch_penalty = 3\n", "");
        let err = parse(&text).expect_err("missing branch_penalty");
        assert!(
            err.contains("branch_penalty"),
            "error should name missing ports key, got: {err}"
        );
    }

    #[test]
    fn reject_missing_mem_key() {
        let text = MINIMAL.replace("working_set_surcharge = 2\n", "");
        let err = parse(&text).expect_err("missing surcharge");
        assert!(
            err.contains("working_set_surcharge"),
            "error should name missing mem key, got: {err}"
        );
    }

    #[test]
    fn reject_bad_version() {
        let text = MINIMAL.replace("version = 2", "version = 1");
        let err = parse(&text).expect_err("bad version");
        assert!(
            err.contains("version") && err.contains('1'),
            "error should cite bad version, got: {err}"
        );
    }

    #[test]
    fn reject_zero_hit_latency() {
        let text = MINIMAL.replace("load_stack_hit = 1", "load_stack_hit = 0");
        let err = parse(&text).expect_err("zero hit");
        assert!(
            err.contains("hit"),
            "error should cite hit floor, got: {err}"
        );
    }

    #[test]
    fn load_committed_table() {
        let t = load_default().expect("load bench/wrela-cost-v1.toml");
        assert_eq!(t.version, 2);
        assert_eq!(t.max_issue_per_cycle, 2);
        assert_eq!(t.latency(CostRule::Alu), 1);
        assert_eq!(t.latency(CostRule::Call), 4);
        assert_eq!(t.latency(CostRule::AbortVal), 3);
        assert_eq!(t.latency(CostRule::Load), 12);
        assert_eq!(t.mem.load_stack_hit, 1);
        // Digest must be stable across loads of the same file.
        let again = load_default().expect("reload");
        assert_eq!(t.table_digest(), again.table_digest());
    }
}
