use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use super::{CostRule, Fnv64, repo_root};

pub const EXPECTED_VERSION: u64 = 3;

pub const PROFILE_NAME: &str = "a76-pi5";

const V3_SECTIONS: &[&str] = &[
    "profile",
    "pipelines",
    "latency",
    "geometry",
    "branch",
    "align",
    "crosscore",
    "sweep",
];

const RETIRED_SECTIONS: &[(&str, &str)] = &[
    (
        "ports",
        "its rows moved to [pipelines] (dual issue is a port map now)",
    ),
    ("mem", "its rows moved to [geometry]"),
    (
        "transitional",
        "plans/M20.md item F deleted the quarantine and its three unsourced \
         holdovers (mem_reuse_window / mem_working_set_cap / \
         working_set_surcharge) along with the reuse-window model that read \
         them; reuse distance over real capacities and associativity replaces \
         it, and an unsourced knob may not come back through this door",
    ),
];

const TOP_KEYS: &[&str] = &[
    "version",
    "profile",
    "pipelines",
    "latency",
    "geometry",
    "branch",
    "align",
    "crosscore",
    "sweep",
];

const PROFILE_ROWS: &[&str] = &["name", "ghz"];

const PIPELINE_ROWS: &[&str] = &[
    "count",
    "dispatch_mops",
    "dispatch_uops",
    "cap_b",
    "cap_s",
    "cap_m",
    "cap_v_each",
    "cap_l_each",
    "reorder_window",
    "port_i",
    "port_m",
    "port_l",
    "port_d",
    "port_b",
    "port_v0",
    "port_v1",
];

const GEOMETRY_ROWS: &[&str] = &[
    "l1i_bytes",
    "l1i_line_bytes",
    "l1i_ways",
    "l1d_bytes",
    "l1d_line_bytes",
    "l1d_ways",
    "l2_bytes",
    "l2_line_bytes",
    "l2_ways",
    "l2_inclusive_of_l1d",
    "l3_bytes",
    "l3_line_bytes",
    "l3_ways",
    "itlb_l1_entries",
    "dtlb_l1_entries",
    "tlb_l2_entries",
    "tlb_l2_ways",
    "lat_l1d_hit",
    "lat_l2",
    "lat_l3",
    "lat_dram",
];

const BRANCH_ROWS: &[&str] = &[
    "mispredict_penalty",
    "max_branches_per_32b",
    "region_bytes",
    "target_align_bytes",
    "entry_align_bytes",
    "loop_fit_bytes",
];

const ALIGN_ROWS: &[&str] = &["load_line_bytes", "store_boundary_bytes"];

const VALUE_ROW_KEYS: &[&str] = &[
    "value",
    "tier",
    "source",
    "bracket_lo",
    "bracket_hi",
    "sweep",
    "note",
];

const LATENCY_ROW_KEYS: &[&str] = &[
    "lat",
    "acc_lat",
    "thru_num",
    "thru_den",
    "ports",
    "m_pipe_stall",
    "m_pipe_block",
    "tier",
    "source",
    "bracket_lo",
    "bracket_hi",
    "sweep",
    "sweep_add",
    "note",
];

const CROSSCORE_ROW_KEYS: &[&str] = &[
    "rule",
    "base",
    "sweep",
    "tier",
    "mechanism",
    "source",
    "note",
];

const SWEEP_ROW_KEYS: &[&str] = &[
    "lo",
    "hi",
    "pinned",
    "pessimistic",
    "removal_sensitive",
    "ambiguity",
    "tier",
    "source",
    "note",
    "le",
    "le_physics",
];

const CROSSCORE_FORBIDDEN_KEYS: &[&str] = &[
    "value", "lat", "acc_lat", "cost", "cycles", "penalty", "pinned", "lo", "hi",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Tier {
    T1,
    T2,
    T3,
    T4,
    T5,
}

impl Tier {
    pub fn as_str(self) -> &'static str {
        match self {
            Tier::T1 => "T1",
            Tier::T2 => "T2",
            Tier::T3 => "T3",
            Tier::T4 => "T4",
            Tier::T5 => "T5",
        }
    }

    pub fn from_str(s: &str) -> Option<Tier> {
        Some(match s {
            "T1" => Tier::T1,
            "T2" => Tier::T2,
            "T3" => Tier::T3,
            "T4" => Tier::T4,
            "T5" => Tier::T5,
            _ => return None,
        })
    }

    pub const ALL: &'static [Tier] = &[Tier::T1, Tier::T2, Tier::T3, Tier::T4, Tier::T5];
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum End {
    Lo,
    Hi,
}

impl End {
    fn as_str(self) -> &'static str {
        match self {
            End::Lo => "lo",
            End::Hi => "hi",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Row {
    pub value: u64,
    pub text: Option<String>,
    pub tier: Tier,
    pub source: String,
    pub bracket: Option<(u64, u64)>,
    pub sweep: Option<String>,
    pub note: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LatRow {
    pub lat: u64,
    pub acc_lat: Option<u64>,
    pub thru_num: u64,
    pub thru_den: u64,
    pub ports: String,
    pub m_pipe_stall: u64,
    pub m_pipe_block: bool,
    pub tier: Tier,
    pub source: String,
    pub bracket: Option<(u64, u64)>,
    pub sweep: Option<String>,
    pub sweep_add: Option<String>,
    pub note: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CrossRow {
    pub rule: Option<String>,
    pub base: Option<String>,
    pub sweep: String,
    pub tier: Tier,
    pub mechanism: String,
    pub source: String,
    pub note: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SweepBound {
    pub minuend: String,
    pub subtrahend: Option<String>,
    pub physics: String,
}

impl SweepBound {
    pub fn expr(&self) -> String {
        match &self.subtrahend {
            Some(s) => format!("{} - {s}", self.minuend),
            None => self.minuend.clone(),
        }
    }

    pub fn bound_at(&self, minuend: u64, subtrahend: u64) -> u64 {
        minuend.saturating_sub(subtrahend)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SweepRow {
    pub lo: u64,
    pub hi: u64,
    pub pinned: u64,
    pub pessimistic: End,
    pub removal_sensitive: bool,
    pub ambiguity: Option<String>,
    pub le: Option<SweepBound>,
    pub tier: Tier,
    pub source: String,
    pub note: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct CostTable {
    pub version: u64,
    profile_name: String,
    ghz_text: String,
    ghz: f64,
    profile_tier_name: Tier,
    profile_tier_ghz: Tier,
    pipelines: BTreeMap<String, Row>,
    latency: BTreeMap<String, LatRow>,
    geometry: BTreeMap<String, Row>,
    branch: BTreeMap<String, Row>,
    align: BTreeMap<String, Row>,
    crosscore: BTreeMap<String, CrossRow>,
    sweep: BTreeMap<String, SweepRow>,
    latencies: BTreeMap<&'static str, u64>,
    crosscore_extra: BTreeMap<&'static str, u64>,
    crosscore_extra_dim: BTreeMap<&'static str, String>,
}

impl CostTable {
    pub fn latency(&self, rule: CostRule) -> u64 {
        *self
            .latencies
            .get(rule.as_str())
            .unwrap_or_else(|| panic!("cost table missing rule {}", rule.as_str()))
    }

    pub fn crosscore_extra(&self, rule: CostRule) -> u64 {
        self.crosscore_extra
            .get(rule.as_str())
            .copied()
            .unwrap_or(0)
    }

    pub fn crosscore_extra_dim(&self, rule: CostRule) -> Option<&str> {
        self.crosscore_extra_dim
            .get(rule.as_str())
            .map(String::as_str)
    }

    pub fn profile_name(&self) -> &str {
        &self.profile_name
    }

    pub fn ghz(&self) -> f64 {
        self.ghz
    }

    pub fn pipelines(&self) -> u64 {
        self.pipelines["count"].value
    }

    pub fn dispatch_mops(&self) -> u64 {
        self.pipelines["dispatch_mops"].value
    }

    pub fn dispatch_uops(&self) -> u64 {
        self.pipelines["dispatch_uops"].value
    }

    pub fn reorder_window(&self) -> u64 {
        self.pipelines["reorder_window"].value
    }

    pub fn pipeline_row(&self, key: &str) -> Option<&Row> {
        self.pipelines.get(key)
    }

    pub fn alu_ports(&self) -> u64 {
        self.pipelines["port_i"].value
    }

    pub fn mem_ports(&self) -> u64 {
        self.pipelines["port_l"].value
    }

    pub fn max_issue_per_cycle(&self) -> u64 {
        self.dispatch_uops()
    }

    pub fn geometry(&self, key: &str) -> Option<&Row> {
        self.geometry.get(key)
    }

    pub fn branch_row(&self, key: &str) -> Option<&Row> {
        self.branch.get(key)
    }

    pub fn align(&self, key: &str) -> Option<&Row> {
        self.align.get(key)
    }

    pub fn latency_row(&self, key: &str) -> Option<&LatRow> {
        self.latency.get(key)
    }

    pub fn crosscore(&self, key: &str) -> Option<&CrossRow> {
        self.crosscore.get(key)
    }

    pub fn sweep(&self, key: &str) -> Option<&SweepRow> {
        self.sweep.get(key)
    }

    pub fn sweep_dimensions(&self) -> Vec<&str> {
        self.sweep.keys().map(String::as_str).collect()
    }

    pub fn sweep_cardinality(&self) -> u128 {
        1u128 << self.sweep.len().min(127)
    }

    pub fn table_digest(&self) -> String {
        digest_lines(&self.content_lines())
    }

    pub fn provenance_digest(&self) -> String {
        digest_lines(&self.tier_lines())
    }

    fn tiers(&self) -> Vec<Tier> {
        let mut t = vec![self.profile_tier_name, self.profile_tier_ghz];
        for rows in [&self.pipelines, &self.geometry, &self.branch, &self.align] {
            t.extend(rows.values().map(|r| r.tier));
        }
        t.extend(self.latency.values().map(|r| r.tier));
        t.extend(self.crosscore.values().map(|r| r.tier));
        t.extend(self.sweep.values().map(|r| r.tier));
        t
    }

    pub fn provenance_summary(&self) -> String {
        let tiers = self.tiers();
        let mut counts: BTreeMap<&'static str, usize> = BTreeMap::new();
        for t in Tier::ALL {
            counts.insert(t.as_str(), 0);
        }
        for t in &tiers {
            if let Some(c) = counts.get_mut(t.as_str()) {
                *c += 1;
            }
        }
        let mut out = String::new();
        for t in Tier::ALL {
            out.push_str(&format!("{}={} ", t.as_str(), counts[t.as_str()]));
        }
        out.push_str(&format!("rows={}", tiers.len()));
        out
    }

    fn content_lines(&self) -> Vec<String> {
        let mut l: Vec<String> = Vec::new();
        l.push(format!("version={}", self.version));
        l.push(format!("profile.name={}", self.profile_name));
        l.push(format!("profile.ghz={}", self.ghz_text));
        for (section, rows) in [
            ("pipelines", &self.pipelines),
            ("geometry", &self.geometry),
            ("branch", &self.branch),
            ("align", &self.align),
        ] {
            for (name, row) in rows {
                match &row.text {
                    Some(t) => l.push(format!("{section}.{name}={t}({})", row.value)),
                    None => l.push(format!("{section}.{name}={}", row.value)),
                }
                if let Some((lo, hi)) = row.bracket {
                    l.push(format!("{section}.{name}.bracket={lo}..{hi}"));
                }
                if let Some(s) = &row.sweep {
                    l.push(format!("{section}.{name}.sweep={s}"));
                }
            }
        }
        for (name, r) in &self.latency {
            l.push(format!(
                "latency.{name}=lat:{} acc:{} thru:{}/{} ports:{} mstall:{} mblock:{}",
                r.lat,
                r.acc_lat.map(|v| v.to_string()).unwrap_or_default(),
                r.thru_num,
                r.thru_den,
                r.ports,
                r.m_pipe_stall,
                u8::from(r.m_pipe_block),
            ));
            if let Some((lo, hi)) = r.bracket {
                l.push(format!("latency.{name}.bracket={lo}..{hi}"));
            }
            if let Some(s) = &r.sweep {
                l.push(format!("latency.{name}.sweep={s}"));
            }
            if let Some(s) = &r.sweep_add {
                l.push(format!("latency.{name}.sweep_add={s}"));
            }
        }
        for (name, c) in &self.crosscore {
            l.push(format!(
                "crosscore.{name}=rule:{} base:{} sweep:{}",
                c.rule.clone().unwrap_or_default(),
                c.base.clone().unwrap_or_default(),
                c.sweep
            ));
        }
        for (name, s) in &self.sweep {
            l.push(format!(
                "sweep.{name}={}..{} pinned:{} pessimistic:{} removal_sensitive:{}",
                s.lo,
                s.hi,
                s.pinned,
                s.pessimistic.as_str(),
                u8::from(s.removal_sensitive),
            ));
            if let Some(b) = &s.le {
                l.push(format!("sweep.{name}.le={}", b.expr()));
            }
        }
        l.sort();
        l
    }

    fn tier_lines(&self) -> Vec<String> {
        let mut l: Vec<String> = Vec::new();
        l.push(format!("profile.name={}", self.profile_tier_name.as_str()));
        l.push(format!("profile.ghz={}", self.profile_tier_ghz.as_str()));
        let opt = |s: &Option<String>| s.clone().unwrap_or_default();
        for (section, rows) in [
            ("pipelines", &self.pipelines),
            ("geometry", &self.geometry),
            ("branch", &self.branch),
            ("align", &self.align),
        ] {
            for (name, row) in rows {
                l.push(format!(
                    "{section}.{name}={}\u{1}{}\u{1}{}",
                    row.tier.as_str(),
                    row.source,
                    opt(&row.note),
                ));
            }
        }
        for (name, r) in &self.latency {
            l.push(format!(
                "latency.{name}={}\u{1}{}\u{1}{}",
                r.tier.as_str(),
                r.source,
                opt(&r.note),
            ));
        }
        for (name, c) in &self.crosscore {
            l.push(format!(
                "crosscore.{name}={}\u{1}{}\u{1}{}\u{1}{}",
                c.tier.as_str(),
                c.source,
                c.mechanism,
                opt(&c.note),
            ));
        }
        for (name, s) in &self.sweep {
            l.push(format!(
                "sweep.{name}={}\u{1}{}\u{1}{}\u{1}{}\u{1}{}",
                s.tier.as_str(),
                s.source,
                opt(&s.ambiguity),
                opt(&s.note),
                s.le.as_ref().map(|b| b.physics.clone()).unwrap_or_default(),
            ));
        }
        l.sort();
        l
    }
}

fn digest_lines(lines: &[String]) -> String {
    let mut h = Fnv64::new();
    for (i, line) in lines.iter().enumerate() {
        if i > 0 {
            h.write(b"\n");
        }
        h.write(line.as_bytes());
    }
    format!("{:016x}", h.finish())
}

pub fn default_table_path() -> PathBuf {
    if let Ok(p) = std::env::var("WRELA_COST_TABLE") {
        return PathBuf::from(p);
    }
    repo_root().join("bench/a76-pi5.toml")
}

pub fn load_default() -> Result<CostTable, String> {
    load_from_path(&default_table_path())
}

pub fn profile_ghz() -> f64 {
    match load_default() {
        Ok(t) => t.ghz(),
        Err(_) => super::ghz::DEFAULT_GHZ,
    }
}

pub fn load_from_path(path: &Path) -> Result<CostTable, String> {
    let text = std::fs::read_to_string(path).map_err(|e| {
        format!(
            "cost table: read {}: {e} (expected workspace-relative \
             bench/a76-pi5.toml)",
            path.display()
        )
    })?;
    parse(&text).map_err(|e| format!("cost table {}: {e}", path.display()))
}

pub fn parse(text: &str) -> Result<CostTable, String> {
    let value: toml::Value = text.parse().map_err(|e| format!("parse failed: {e}"))?;

    if value.get("issue_width").is_some() {
        return Err(
            "`issue_width` is rejected (hard-cut; use [pipelines] dispatch_uops)".to_string(),
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
        let present: Vec<&str> = V3_SECTIONS
            .iter()
            .copied()
            .filter(|s| value.get(*s).is_some())
            .collect();
        if !present.is_empty() {
            return Err(format!(
                "bad version {version}: v3 section(s) [{}] present — a half-migrated \
                 table cannot score (expected version {EXPECTED_VERSION})",
                present.join(", ")
            ));
        }
        return Err(format!(
            "bad version {version}: expected {EXPECTED_VERSION}"
        ));
    }

    let root = value
        .as_table()
        .ok_or_else(|| "table root must be a TOML table".to_string())?;
    for (stale, why) in RETIRED_SECTIONS {
        if root.contains_key(*stale) {
            return Err(format!(
                "retired section `[{stale}]` is not part of version {EXPECTED_VERSION}: {why}"
            ));
        }
    }
    for key in root.keys() {
        if !TOP_KEYS.contains(&key.as_str()) {
            return Err(format!("unknown top-level key `{key}`"));
        }
    }
    for section in V3_SECTIONS {
        if !root.contains_key(*section) {
            return Err(format!(
                "missing `[{section}]` section (version {EXPECTED_VERSION} requires all of: {})",
                V3_SECTIONS.join(", ")
            ));
        }
    }

    let sweep = parse_sweeps(section(root, "sweep")?)?;

    let profile_tbl = section(root, "profile")?;
    require_rows("profile", profile_tbl, PROFILE_ROWS)?;
    let name_row = value_row(
        "profile",
        "name",
        sub(profile_tbl, "profile", "name")?,
        &sweep,
    )?;
    reject_pinned_t5("profile", "name", name_row.tier)?;
    let profile_name = name_row
        .text
        .clone()
        .ok_or_else(|| "profile.name must be a string".to_string())?;
    if profile_name != PROFILE_NAME {
        return Err(format!(
            "profile.name `{profile_name}`: `{PROFILE_NAME}` is the only profile (freeze 1621)"
        ));
    }
    let ghz_row = value_row(
        "profile",
        "ghz",
        sub(profile_tbl, "profile", "ghz")?,
        &sweep,
    )?;
    reject_pinned_t5("profile", "ghz", ghz_row.tier)?;
    let ghz_text = ghz_row.text.clone().ok_or_else(|| {
        "profile.ghz must be a string (parsed through ghz::parse_ghz)".to_string()
    })?;
    let ghz = super::ghz::parse_ghz(&ghz_text).map_err(|e| format!("profile.ghz: {e}"))?;

    let pipelines = parse_value_section(root, "pipelines", PIPELINE_ROWS, &sweep)?;
    let geometry = parse_value_section(root, "geometry", GEOMETRY_ROWS, &sweep)?;
    let branch = parse_value_section(root, "branch", BRANCH_ROWS, &sweep)?;
    let align = parse_value_section(root, "align", ALIGN_ROWS, &sweep)?;

    let pipe_count = pipelines["count"].value;
    for key in [
        "port_i", "port_m", "port_l", "port_d", "port_b", "port_v0", "port_v1",
    ] {
        let row = &pipelines[key];
        let spec = row
            .text
            .as_deref()
            .ok_or_else(|| format!("pipelines.{key} must be a pipe-range string like \"2-4\""))?;
        if row.value == 0 {
            return Err(format!("pipelines.{key} `{spec}`: names no pipeline"));
        }
        let hi = pipe_range(spec).map(|(_, hi)| hi).unwrap_or(0);
        if hi > pipe_count {
            return Err(format!(
                "pipelines.{key} `{spec}`: pipeline {hi} exceeds pipelines.count {pipe_count}"
            ));
        }
    }
    for key in [
        "count",
        "dispatch_mops",
        "dispatch_uops",
        "cap_b",
        "cap_s",
        "cap_m",
        "cap_v_each",
        "cap_l_each",
        "reorder_window",
    ] {
        if pipelines[key].value == 0 {
            return Err(format!("pipelines.{key} must be > 0"));
        }
        if pipelines[key].text.is_some() {
            return Err(format!("pipelines.{key} must be an integer, not a string"));
        }
    }
    if pipelines["dispatch_uops"].value < pipelines["dispatch_mops"].value {
        return Err(
            "pipelines.dispatch_uops must be >= dispatch_mops (a Mop may split into two uops)"
                .to_string(),
        );
    }

    for key in GEOMETRY_ROWS {
        if *key == "l2_inclusive_of_l1d" {
            continue;
        }
        if geometry[*key].value == 0 {
            return Err(format!("geometry.{key} must be > 0"));
        }
    }
    if geometry["l2_inclusive_of_l1d"].value != 1 {
        return Err(
            "geometry.l2_inclusive_of_l1d must be 1: A76's L2 is strictly inclusive of L1D, \
             so there is no victim path to model"
                .to_string(),
        );
    }
    if geometry["lat_l2"].value <= geometry["lat_l1d_hit"].value
        || geometry["lat_l3"].value <= geometry["lat_l2"].value
        || geometry["lat_dram"].value <= geometry["lat_l3"].value
    {
        return Err(
            "geometry leaf latencies must strictly increase: lat_l1d_hit < lat_l2 < lat_l3 \
             < lat_dram"
                .to_string(),
        );
    }
    for key in ["l1i_line_bytes", "l2_line_bytes", "l3_line_bytes"] {
        if geometry[key].value != geometry["l1d_line_bytes"].value {
            return Err(format!(
                "geometry.{key} ({}) disagrees with geometry.l1d_line_bytes ({}): the memory \
                 model resolves one line identity for every level (plans/M20.md item F)",
                geometry[key].value, geometry["l1d_line_bytes"].value
            ));
        }
    }
    for key in ["l1i_bytes", "l1d_bytes", "l2_bytes", "l3_bytes"] {
        let line = geometry[match key {
            "l1i_bytes" => "l1i_line_bytes",
            "l1d_bytes" => "l1d_line_bytes",
            "l2_bytes" => "l2_line_bytes",
            _ => "l3_line_bytes",
        }]
        .value;
        if geometry[key].value % line != 0 {
            return Err(format!(
                "geometry.{key} must be a multiple of its line size"
            ));
        }
    }

    let latency = parse_latency(section(root, "latency")?, &sweep)?;
    let crosscore = parse_crosscore(section(root, "crosscore")?, &latency, &sweep)?;

    let mut latencies: BTreeMap<&'static str, u64> = BTreeMap::new();
    let mut crosscore_extra: BTreeMap<&'static str, u64> = BTreeMap::new();
    let mut crosscore_extra_dim: BTreeMap<&'static str, String> = BTreeMap::new();
    for (key, row) in &latency {
        let rule = CostRule::from_str(key)
            .ok_or_else(|| format!("unknown latency group `{key}` (not a CostRule key)"))?;
        let mut lat = row.lat;
        if let Some(add) = &row.sweep_add {
            lat = lat.saturating_add(sweep[add].pinned);
        }
        latencies.insert(rule.as_str(), lat);
    }
    for (term, c) in &crosscore {
        let Some(rule_key) = &c.rule else {
            continue;
        };
        let rule = CostRule::from_str(rule_key)
            .ok_or_else(|| format!("crosscore.{term}.rule `{rule_key}` is not a CostRule key"))?;
        let base = match &c.base {
            Some(b) => latency[b].lat,
            None => 0,
        };
        if c.base.is_some() {
            crosscore_extra.insert(rule.as_str(), sweep[&c.sweep].pinned);
            crosscore_extra_dim.insert(rule.as_str(), c.sweep.clone());
        }
        if latencies
            .insert(rule.as_str(), base.saturating_add(sweep[&c.sweep].pinned))
            .is_some()
        {
            return Err(format!(
                "CostRule `{rule_key}` is priced twice: by [latency.{rule_key}] and by \
                 [crosscore.{term}]"
            ));
        }
    }
    for &rule in CostRule::ALL {
        if !latencies.contains_key(rule.as_str()) {
            return Err(format!(
                "CostRule key `{}` is priced by neither [latency] nor [crosscore] \
                 (freeze 1632: every cost dimension is accounted for)",
                rule.as_str()
            ));
        }
    }

    let profile_tier_name = name_row.tier;
    let profile_tier_ghz = ghz_row.tier;

    Ok(CostTable {
        version,
        profile_name,
        ghz_text,
        ghz,
        profile_tier_name,
        profile_tier_ghz,
        pipelines,
        latency,
        geometry,
        branch,
        align,
        crosscore,
        sweep,
        latencies,
        crosscore_extra,
        crosscore_extra_dim,
    })
}

type Tbl = toml::map::Map<String, toml::Value>;

fn section<'a>(root: &'a Tbl, name: &str) -> Result<&'a Tbl, String> {
    root.get(name)
        .and_then(|v| v.as_table())
        .ok_or_else(|| format!("`[{name}]` must be a table of rows"))
}

fn sub<'a>(tbl: &'a Tbl, section: &str, name: &str) -> Result<&'a Tbl, String> {
    tbl.get(name)
        .and_then(|v| v.as_table())
        .ok_or_else(|| format!("[{section}.{name}] must be a sub-table (row) with tier + source"))
}

fn require_rows(section: &str, tbl: &Tbl, rows: &[&str]) -> Result<(), String> {
    for r in rows {
        if !tbl.contains_key(*r) {
            return Err(format!("missing row [{section}.{r}]"));
        }
    }
    for k in tbl.keys() {
        if !rows.contains(&k.as_str()) {
            return Err(format!("unknown row [{section}.{k}]"));
        }
    }
    Ok(())
}

fn check_keys(path: &str, tbl: &Tbl, allowed: &[&str]) -> Result<(), String> {
    for k in tbl.keys() {
        if !allowed.contains(&k.as_str()) {
            return Err(format!("unknown key `{k}` in [{path}]"));
        }
    }
    Ok(())
}

fn req_u64(path: &str, tbl: &Tbl, key: &str) -> Result<u64, String> {
    let v = tbl
        .get(key)
        .ok_or_else(|| format!("missing {path}.{key}"))?
        .as_integer()
        .ok_or_else(|| format!("{path}.{key} must be an integer"))?;
    if v < 0 {
        return Err(format!("{path}.{key} must be >= 0, got {v}"));
    }
    Ok(v as u64)
}

fn opt_u64(path: &str, tbl: &Tbl, key: &str) -> Result<Option<u64>, String> {
    if !tbl.contains_key(key) {
        return Ok(None);
    }
    Ok(Some(req_u64(path, tbl, key)?))
}

fn req_str(path: &str, tbl: &Tbl, key: &str) -> Result<String, String> {
    let s = tbl
        .get(key)
        .ok_or_else(|| format!("missing {path}.{key}"))?
        .as_str()
        .ok_or_else(|| format!("{path}.{key} must be a string"))?;
    if s.trim().is_empty() {
        return Err(format!("{path}.{key} must not be empty"));
    }
    Ok(s.to_string())
}

fn opt_str(path: &str, tbl: &Tbl, key: &str) -> Result<Option<String>, String> {
    if !tbl.contains_key(key) {
        return Ok(None);
    }
    Ok(Some(req_str(path, tbl, key)?))
}

fn req_tier(path: &str, tbl: &Tbl) -> Result<Tier, String> {
    let s = req_str(path, tbl, "tier")?;
    Tier::from_str(&s).ok_or_else(|| format!("{path}.tier `{s}`: expected one of T1..T5"))
}

fn reject_pinned_t5(section: &str, name: &str, tier: Tier) -> Result<(), String> {
    if tier == Tier::T5 {
        Err(format!(
            "[{section}.{name}] is tier T5 and pinned: a T5 value cannot be pinned \
             (freeze 1629) — move it to a [sweep.<dimension>] row"
        ))
    } else {
        Ok(())
    }
}

pub(crate) fn pipe_range(spec: &str) -> Option<(u64, u64)> {
    let (lo, hi) = match spec.split_once('-') {
        Some((a, b)) => (a.trim().parse::<u64>().ok()?, b.trim().parse::<u64>().ok()?),
        None => {
            let n = spec.trim().parse::<u64>().ok()?;
            (n, n)
        }
    };
    if lo == 0 || hi < lo {
        return None;
    }
    Some((lo, hi))
}

fn value_row(
    section: &str,
    name: &str,
    tbl: &Tbl,
    sweep: &BTreeMap<String, SweepRow>,
) -> Result<Row, String> {
    let path = format!("{section}.{name}");
    check_keys(&path, tbl, VALUE_ROW_KEYS)?;
    let raw = tbl
        .get("value")
        .ok_or_else(|| format!("missing {path}.value"))?;
    let (value, text) = match raw {
        toml::Value::Integer(i) => {
            if *i < 0 {
                return Err(format!("{path}.value must be >= 0, got {i}"));
            }
            (*i as u64, None)
        }
        toml::Value::String(s) => {
            let count = match pipe_range(s) {
                Some((lo, hi)) => hi - lo + 1,
                None => 0,
            };
            (count, Some(s.clone()))
        }
        other => {
            return Err(format!(
                "{path}.value must be an integer or a string, got {}",
                other.type_str()
            ));
        }
    };
    let tier = req_tier(&path, tbl)?;
    let source = req_str(&path, tbl, "source")?;
    let bracket = bracket_of(&path, tbl)?;
    let sweep_name = opt_str(&path, tbl, "sweep")?;
    check_bracket(&path, value, bracket, sweep_name.as_deref(), sweep)?;
    Ok(Row {
        value,
        text,
        tier,
        source,
        bracket,
        sweep: sweep_name,
        note: opt_str(&path, tbl, "note")?,
    })
}

fn bracket_of(path: &str, tbl: &Tbl) -> Result<Option<(u64, u64)>, String> {
    let lo = opt_u64(path, tbl, "bracket_lo")?;
    let hi = opt_u64(path, tbl, "bracket_hi")?;
    match (lo, hi) {
        (Some(lo), Some(hi)) => {
            if lo > hi {
                return Err(format!("{path}: bracket_lo {lo} > bracket_hi {hi}"));
            }
            Ok(Some((lo, hi)))
        }
        (None, None) => Ok(None),
        _ => Err(format!(
            "{path}: bracket_lo and bracket_hi must be given together"
        )),
    }
}

fn check_bracket(
    path: &str,
    value: u64,
    bracket: Option<(u64, u64)>,
    sweep_name: Option<&str>,
    sweep: &BTreeMap<String, SweepRow>,
) -> Result<(), String> {
    if let Some(name) = sweep_name {
        if !sweep.contains_key(name) {
            return Err(format!("{path}.sweep `{name}`: no such [sweep.{name}] row"));
        }
    }
    let Some((lo, hi)) = bracket else {
        return Ok(());
    };
    let Some(name) = sweep_name else {
        return Err(format!(
            "{path} carries a bracket but names no sweep dimension: a bracket that is \
             not swept is a chosen assumption (decision 1604)"
        ));
    };
    let dim = &sweep[name];
    if (dim.lo, dim.hi) != (lo, hi) {
        return Err(format!(
            "{path} bracket {lo}..{hi} disagrees with [sweep.{name}] {}..{}",
            dim.lo, dim.hi
        ));
    }
    if value != dim.pinned {
        return Err(format!(
            "{path} value {value} is not [sweep.{name}]'s pinned point {} \
             (decision 1609: pin the bracket's pessimistic end)",
            dim.pinned
        ));
    }
    Ok(())
}

fn parse_value_section(
    root: &Tbl,
    name: &str,
    rows: &[&str],
    sweep: &BTreeMap<String, SweepRow>,
) -> Result<BTreeMap<String, Row>, String> {
    let tbl = section(root, name)?;
    require_rows(name, tbl, rows)?;
    let mut out = BTreeMap::new();
    for r in rows {
        let row = value_row(name, r, sub(tbl, name, r)?, sweep)?;
        reject_pinned_t5(name, r, row.tier)?;
        out.insert((*r).to_string(), row);
    }
    Ok(out)
}

fn parse_latency(
    tbl: &Tbl,
    sweep: &BTreeMap<String, SweepRow>,
) -> Result<BTreeMap<String, LatRow>, String> {
    if tbl.is_empty() {
        return Err("[latency] must carry at least one instruction group".to_string());
    }
    let mut out = BTreeMap::new();
    for (name, raw) in tbl {
        let path = format!("latency.{name}");
        let row = raw
            .as_table()
            .ok_or_else(|| format!("[{path}] must be a sub-table (row)"))?;
        check_keys(&path, row, LATENCY_ROW_KEYS)?;
        if CostRule::from_str(name).is_none() {
            return Err(format!(
                "[{path}]: `{name}` is not a CostRule key — model only what wrela emits \
                 (freeze 1630)"
            ));
        }
        let tier = req_tier(&path, row)?;
        reject_pinned_t5("latency", name, tier)?;
        let lat = req_u64(&path, row, "lat")?;
        let thru_num = req_u64(&path, row, "thru_num")?;
        let thru_den = req_u64(&path, row, "thru_den")?;
        if thru_num == 0 || thru_den == 0 {
            return Err(format!(
                "{path}: thru_num / thru_den must both be > 0 (throughput is the rational \
                 thru_num/thru_den — 3-per-cycle is 3/1, one-per-three-cycles is 1/3)"
            ));
        }
        let ports = req_str(&path, row, "ports")?;
        let bracket = bracket_of(&path, row)?;
        let sweep_name = opt_str(&path, row, "sweep")?;
        check_bracket(&path, lat, bracket, sweep_name.as_deref(), sweep)?;
        let sweep_add = opt_str(&path, row, "sweep_add")?;
        if let Some(add) = &sweep_add {
            if !sweep.contains_key(add) {
                return Err(format!(
                    "{path}.sweep_add `{add}`: no such [sweep.{add}] row"
                ));
            }
        }
        let m_pipe_block = match row.get("m_pipe_block") {
            None => false,
            Some(v) => match v.as_integer() {
                Some(0) => false,
                Some(1) => true,
                _ => return Err(format!("{path}.m_pipe_block must be 0 or 1")),
            },
        };
        out.insert(
            name.clone(),
            LatRow {
                lat,
                acc_lat: opt_u64(&path, row, "acc_lat")?,
                thru_num,
                thru_den,
                ports,
                m_pipe_stall: opt_u64(&path, row, "m_pipe_stall")?.unwrap_or(0),
                m_pipe_block,
                tier,
                source: req_str(&path, row, "source")?,
                bracket,
                sweep: sweep_name,
                sweep_add,
                note: opt_str(&path, row, "note")?,
            },
        );
    }
    Ok(out)
}

fn parse_crosscore(
    tbl: &Tbl,
    latency: &BTreeMap<String, LatRow>,
    sweep: &BTreeMap<String, SweepRow>,
) -> Result<BTreeMap<String, CrossRow>, String> {
    if tbl.is_empty() {
        return Err(
            "[crosscore] must declare the barrier / snoop / ordered-access terms".to_string(),
        );
    }
    let mut out = BTreeMap::new();
    for (name, raw) in tbl {
        let path = format!("crosscore.{name}");
        let row = raw
            .as_table()
            .ok_or_else(|| format!("[{path}] must be a sub-table (term)"))?;
        for forbidden in CROSSCORE_FORBIDDEN_KEYS {
            if row.contains_key(*forbidden) {
                return Err(format!(
                    "[{path}] carries `{forbidden}`: a cross-core term is T5 by absence and \
                     may not be pinned at all (decision 1602) — its magnitude belongs in \
                     [sweep.<dimension>]"
                ));
            }
        }
        check_keys(&path, row, CROSSCORE_ROW_KEYS)?;
        let tier = req_tier(&path, row)?;
        if tier != Tier::T5 {
            return Err(format!(
                "[{path}].tier is {}: every cross-core term is T5 by absence (the SOG has no \
                 barrier, exclusive, system-register or prefetch entry anywhere)",
                tier.as_str()
            ));
        }
        let sweep_name = req_str(&path, row, "sweep")?;
        if !sweep.contains_key(&sweep_name) {
            return Err(format!(
                "{path}.sweep `{sweep_name}`: no such [sweep.{sweep_name}] row"
            ));
        }
        let base = opt_str(&path, row, "base")?;
        if let Some(b) = &base {
            if !latency.contains_key(b) {
                return Err(format!("{path}.base `{b}`: no such [latency.{b}] row"));
            }
        }
        out.insert(
            name.clone(),
            CrossRow {
                rule: opt_str(&path, row, "rule")?,
                base,
                sweep: sweep_name,
                tier,
                mechanism: req_str(&path, row, "mechanism")?,
                source: req_str(&path, row, "source")?,
                note: opt_str(&path, row, "note")?,
            },
        );
    }
    Ok(out)
}

fn parse_sweeps(tbl: &Tbl) -> Result<BTreeMap<String, SweepRow>, String> {
    if tbl.is_empty() {
        return Err(
            "[sweep] must carry the residual-uncertainty box (decision 1604) — an empty box \
             would mean the model claims certainty it does not have"
                .to_string(),
        );
    }
    let mut out = BTreeMap::new();
    for (name, raw) in tbl {
        let path = format!("sweep.{name}");
        let row = raw
            .as_table()
            .ok_or_else(|| format!("[{path}] must be a sub-table (dimension)"))?;
        check_keys(&path, row, SWEEP_ROW_KEYS)?;
        let lo = req_u64(&path, row, "lo")?;
        let hi = req_u64(&path, row, "hi")?;
        let pinned = req_u64(&path, row, "pinned")?;
        if lo >= hi {
            return Err(format!(
                "{path}: lo {lo} must be < hi {hi} (a one-point dimension is a pinned value, \
                 not a residual uncertainty)"
            ));
        }
        if pinned < lo || pinned > hi {
            return Err(format!("{path}: pinned {pinned} is outside {lo}..{hi}"));
        }
        let pess = req_str(&path, row, "pessimistic")?;
        let pessimistic = match pess.as_str() {
            "lo" => End::Lo,
            "hi" => End::Hi,
            other => {
                return Err(format!(
                    "{path}.pessimistic `{other}`: expected `lo` or `hi` (which end over-costs)"
                ));
            }
        };
        let want = match pessimistic {
            End::Lo => lo,
            End::Hi => hi,
        };
        if pinned != want {
            return Err(format!(
                "{path}: pinned {pinned} is not the pessimistic end ({} = {want}) \
                 (decision 1609 / freeze 1623)",
                pessimistic.as_str()
            ));
        }
        let removal_sensitive = match row.get("removal_sensitive") {
            None => false,
            Some(toml::Value::Boolean(b)) => *b,
            Some(_) => return Err(format!("{path}.removal_sensitive must be a boolean")),
        };
        let ambiguity = opt_str(&path, row, "ambiguity")?;
        if removal_sensitive {
            if pessimistic != End::Lo {
                return Err(format!(
                    "{path}: removal_sensitive terms must pin the low end — a larger charge \
                     over-credits deletion, and freeze 1633 forbids making removal profitable"
                ));
            }
            if ambiguity.is_none() {
                return Err(format!(
                    "{path}: removal_sensitive requires an `ambiguity` note — decision 1609 \
                     says record the conflict, never resolve it silently"
                ));
            }
        }
        let le = parse_sweep_bound(&path, row)?;
        out.insert(
            name.clone(),
            SweepRow {
                lo,
                hi,
                pinned,
                pessimistic,
                removal_sensitive,
                ambiguity,
                le,
                tier: req_tier(&path, row)?,
                source: req_str(&path, row, "source")?,
                note: opt_str(&path, row, "note")?,
            },
        );
    }
    check_sweep_bounds(&out)?;
    Ok(out)
}

fn parse_sweep_bound(path: &str, row: &Tbl) -> Result<Option<SweepBound>, String> {
    let Some(expr) = opt_str(path, row, "le")? else {
        if opt_str(path, row, "le_physics")?.is_some() {
            return Err(format!(
                "{path}.le_physics without `le`: a justification for a constraint that \
                 does not exist is a comment pretending to be data"
            ));
        }
        return Ok(None);
    };
    let physics = opt_str(path, row, "le_physics")?.ok_or_else(|| {
        format!(
            "{path}.le requires `le_physics`: a constraint removes corners from the box \
             and can only ever make a candidate easier to land, so why the inequality \
             holds on the machine must be written down (decision 1950)"
        )
    })?;
    let (minuend, subtrahend) = match expr.split_once('-') {
        Some((a, b)) => (a.trim().to_string(), Some(b.trim().to_string())),
        None => (expr.trim().to_string(), None),
    };
    if minuend.is_empty() || subtrahend.as_deref() == Some("") {
        return Err(format!(
            "{path}.le `{expr}`: expected `<dim>` or `<dim> - <dim>`"
        ));
    }
    Ok(Some(SweepBound {
        minuend,
        subtrahend,
        physics,
    }))
}

fn check_sweep_bounds(sweep: &BTreeMap<String, SweepRow>) -> Result<(), String> {
    for (name, row) in sweep {
        let Some(b) = &row.le else { continue };
        let path = format!("sweep.{name}");
        let mut ends: Vec<(u64, u64)> = Vec::new();
        for dim in [Some(&b.minuend), b.subtrahend.as_ref()]
            .into_iter()
            .flatten()
        {
            if dim == name {
                return Err(format!(
                    "{path}.le `{}`: a dimension cannot bound itself",
                    b.expr()
                ));
            }
            let other = sweep
                .get(dim.as_str())
                .ok_or_else(|| format!("{path}.le `{}`: no such [sweep.{dim}] row", b.expr()))?;
            if other.le.is_some() {
                return Err(format!(
                    "{path}.le `{}`: `{dim}` is itself constrained — chained constraints \
                     are refused rather than solved, so the box stays a thing a reader \
                     can enumerate by hand",
                    b.expr()
                ));
            }
            ends.push((other.lo, other.hi));
        }
        let (m_lo, m_hi) = ends[0];
        let (s_lo, s_hi) = ends.get(1).copied().unwrap_or((0, 0));
        if row.lo > b.bound_at(m_hi, s_lo) {
            return Err(format!(
                "{path}.le `{}`: unsatisfiable — {name} >= {} but the bound is at most {} \
                 anywhere in the box",
                b.expr(),
                row.lo,
                b.bound_at(m_hi, s_lo)
            ));
        }
        let m_pin = sweep[&b.minuend].pinned;
        let s_pin = b.subtrahend.as_ref().map(|d| sweep[d].pinned).unwrap_or(0);
        if row.pinned > b.bound_at(m_pin, s_pin) {
            return Err(format!(
                "{path}.le `{}`: the pinned point violates it ({} > {}) — the committed \
                 model would be pinned at a point the machine cannot be at",
                b.expr(),
                row.pinned,
                b.bound_at(m_pin, s_pin)
            ));
        }
        if row.hi <= b.bound_at(m_lo, s_hi) {
            return Err(format!(
                "{path}.le `{}`: vacuous — no corner of the box violates it, so it is a \
                 comment rather than a constraint (decision 1950)",
                b.expr()
            ));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn committed_text() -> String {
        std::fs::read_to_string(repo_root().join("bench/a76-pi5.toml"))
            .expect("read bench/a76-pi5.toml")
    }

    fn without_section(text: &str, section: &str) -> String {
        let head = format!("[{section}.");
        let mut out = String::new();
        let mut dropping = false;
        for line in text.lines() {
            let t = line.trim_start();
            if t.starts_with('[') {
                dropping = t.starts_with(&head);
            }
            if !dropping {
                out.push_str(line);
                out.push('\n');
            }
        }
        out
    }

    fn without_block(text: &str, header: &str) -> String {
        let mut out = String::new();
        let mut dropping = false;
        let mut hit = false;
        for line in text.lines() {
            let t = line.trim_start();
            if t.starts_with('[') {
                dropping = t.starts_with(header);
                hit |= dropping;
            }
            if !dropping {
                out.push_str(line);
                out.push('\n');
            }
        }
        assert!(hit, "without_block: {header} not found");
        out
    }

    fn edit_block(text: &str, header: &str, from: &str, to: &str) -> String {
        let mut out = String::new();
        let mut inside = false;
        let mut done = false;
        for line in text.lines() {
            let t = line.trim_start();
            if t.starts_with('[') {
                inside = t.starts_with(header);
            }
            if inside && !done && line.trim() == from {
                out.push_str(to);
                out.push('\n');
                done = true;
                continue;
            }
            out.push_str(line);
            out.push('\n');
        }
        assert!(done, "edit_block: `{from}` not found in {header}");
        out
    }

    fn add_to_block(text: &str, header: &str, line: &str) -> String {
        let mut out = String::new();
        let mut done = false;
        for l in text.lines() {
            out.push_str(l);
            out.push('\n');
            if !done && l.trim().starts_with(header) {
                out.push_str(line);
                out.push('\n');
                done = true;
            }
        }
        assert!(done, "add_to_block: {header} not found");
        out
    }

    fn err_of(text: String) -> String {
        match parse(&text) {
            Ok(_) => panic!("expected the parser to fail closed"),
            Err(e) => e,
        }
    }

    fn assert_err_names(text: String, needle: &str) {
        let e = err_of(text);
        assert!(e.contains(needle), "error should name `{needle}`, got: {e}");
    }

    #[test]
    fn parse_and_digest_stable() {
        let text = committed_text();
        let a = parse(&text).expect("parse committed profile");
        let b = parse(&text).expect("parse again");

        assert_eq!(a.version, 3);
        assert_eq!(a.profile_name(), PROFILE_NAME);
        assert_eq!(a.ghz(), 2.4);

        assert_eq!(a.pipelines(), 8);
        assert_eq!(a.dispatch_mops(), 4);
        assert_eq!(a.dispatch_uops(), 8);
        assert_eq!(a.reorder_window(), 128);
        assert_eq!(a.alu_ports(), 3, "port_i = pipes 2-4");
        assert_eq!(a.mem_ports(), 2, "port_l = pipes 5-6");
        assert_eq!(a.max_issue_per_cycle(), 8);
        assert_eq!(a.branch_row("mispredict_penalty").expect("row").value, 14);
        assert_eq!(a.sweep("mispredict_penalty").expect("row").lo, 11);

        assert_eq!(a.latency(CostRule::Alu), 1);
        assert_eq!(a.latency(CostRule::Load), 4, "L1D-hit path only");
        assert_eq!(a.latency(CostRule::Store), 1, "to the store buffer");
        assert_eq!(a.latency(CostRule::Branch), 1);
        assert_eq!(a.latency(CostRule::Call), 1, "BL + swept call_overhead(0)");
        assert_eq!(a.latency(CostRule::Abort), 1);
        assert_eq!(a.latency(CostRule::AbortVal), 1);
        assert_eq!(a.latency(CostRule::MovWide), 1);
        assert_eq!(a.latency(CostRule::Mul), 4, "X-form MAC");
        assert_eq!(a.latency(CostRule::MulHigh), 5, "SMULH / UMULH");
        assert_eq!(a.latency(CostRule::Sdiv), 20, "X-form divide, pessimistic");
        assert_eq!(a.latency(CostRule::Udiv), 20);
        assert_eq!(a.latency(CostRule::Adrp), 1);

        assert_eq!(a.latency(CostRule::Barrier), 1);
        assert_eq!(a.latency(CostRule::System), 1);
        assert_eq!(a.latency(CostRule::LoadAcquire), 4, "load base + 0");
        assert_eq!(a.latency(CostRule::StoreRelease), 1, "store base + 0");
        assert!(
            a.latency_row("barrier").is_none(),
            "barrier has no pinned row"
        );
        assert!(
            a.latency_row("system").is_none(),
            "system has no pinned row"
        );

        assert_eq!(a.latency_row("mul").unwrap().m_pipe_stall, 2);
        assert_eq!(a.latency_row("mul_high").unwrap().m_pipe_stall, 3);
        assert!(a.latency_row("sdiv").unwrap().m_pipe_block);
        assert!(!a.latency_row("alu").unwrap().m_pipe_block);
        let alu = a.latency_row("alu").unwrap();
        assert_eq!((alu.thru_num, alu.thru_den), (3, 1));
        let mul = a.latency_row("mul").unwrap();
        assert_eq!((mul.thru_num, mul.thru_den), (1, 3));
        assert_eq!(a.latency_row("store").unwrap().ports, "L,D");

        assert_eq!(a.geometry("l1d_bytes").unwrap().value, 65536);
        assert_eq!(a.geometry("l1d_ways").unwrap().value, 4);
        assert_eq!(a.geometry("l2_bytes").unwrap().value, 524288);
        assert_eq!(a.geometry("l2_ways").unwrap().value, 8);
        assert_eq!(a.geometry("l3_bytes").unwrap().value, 2097152);
        assert_eq!(a.geometry("l3_ways").unwrap().value, 16);
        assert_eq!(a.geometry("itlb_l1_entries").unwrap().value, 48);
        assert_eq!(a.geometry("tlb_l2_entries").unwrap().value, 1280);
        assert_eq!(a.geometry("lat_l1d_hit").unwrap().value, 4);
        assert_eq!(a.geometry("lat_l2").unwrap().value, 11);
        assert_eq!(a.geometry("lat_l3").unwrap().value, 35);
        assert_eq!(a.geometry("lat_dram").unwrap().value, 347);
        assert_eq!(a.latency_row("store").unwrap().lat, 1, "to the buffer");

        assert_eq!(a.branch_row("mispredict_penalty").unwrap().value, 14);
        assert_eq!(a.branch_row("max_branches_per_32b").unwrap().value, 4);
        assert_eq!(a.align("load_line_bytes").unwrap().value, 64);
        assert_eq!(a.align("store_boundary_bytes").unwrap().value, 16);

        assert_eq!(a.table_digest(), b.table_digest());
        assert_eq!(a.provenance_digest(), b.provenance_digest());
        assert_eq!(a.table_digest().len(), 16);
        assert_eq!(a.provenance_digest().len(), 16);
        assert!(a.table_digest().chars().all(|c| c.is_ascii_hexdigit()));
        assert!(a.provenance_digest().chars().all(|c| c.is_ascii_hexdigit()));
        assert_ne!(a.table_digest(), a.provenance_digest());
        let summary = a.provenance_summary();
        for t in Tier::ALL {
            assert!(
                summary.contains(t.as_str()),
                "summary lost {}: {summary}",
                t.as_str()
            );
        }
        assert!(summary.contains("rows="), "got: {summary}");
    }

    #[test]
    fn every_committed_row_carries_a_tier_and_a_source() {
        let t = parse(&committed_text()).expect("parse");
        let mut rows = 0usize;
        for section in [&t.pipelines, &t.geometry, &t.branch, &t.align] {
            for (name, row) in section {
                assert!(!row.source.trim().is_empty(), "{name}: empty source");
                rows += 1;
            }
        }
        for (name, r) in &t.latency {
            assert!(!r.source.trim().is_empty(), "latency.{name}: empty source");
            rows += 1;
        }
        for (name, c) in &t.crosscore {
            assert!(
                !c.source.trim().is_empty(),
                "crosscore.{name}: empty source"
            );
            assert!(
                !c.mechanism.trim().is_empty(),
                "crosscore.{name}: empty mechanism"
            );
            rows += 1;
        }
        for (name, s) in &t.sweep {
            assert!(!s.source.trim().is_empty(), "sweep.{name}: empty source");
            rows += 1;
        }
        assert!(rows > 60, "expected a full profile, counted {rows} rows");
    }

    #[test]
    fn sweep_box_cardinality_is_endpoints_per_dimension() {
        let t = parse(&committed_text()).expect("parse");
        let dims = t.sweep_dimensions();
        assert_eq!(dims.len(), 17, "sweep box dimensions: {dims:?}");
        assert_eq!(t.sweep_cardinality(), 131_072);
        for named in [
            "l2_latency",
            "l3_latency",
            "dram_latency",
            "mispredict_penalty",
            "divide_x_latency",
            "range_cross_penalty",
            "store_to_load_forwarding",
            "tlb_walk_cost",
            "dmb_cost",
            "snoop_cost",
            "sysreg_flush_cost",
            "effective_l3_bytes",
            "load_acquire_cost",
            "store_release_cost",
            "call_overhead",
        ] {
            assert!(t.sweep(named).is_some(), "decision 1604 names {named}");
        }
    }

    #[test]
    fn provenance_digest_moves_when_only_a_tier_moves() {
        let base = parse(&committed_text()).expect("parse");
        let retiered = edit_block(
            &committed_text(),
            "[geometry.l3_ways]",
            "tier = \"T2\"",
            "tier = \"T4\"",
        );
        let after = parse(&retiered).expect("parse retiered");
        assert_ne!(base.provenance_digest(), after.provenance_digest());
        assert_eq!(
            base.table_digest(),
            after.table_digest(),
            "no value moved, so the content digest must not move"
        );
    }

    #[test]
    fn provenance_digest_moves_when_only_a_justification_moves() {
        let text = committed_text();
        let base = parse(&text).expect("parse committed");
        for (needle, replacement) in [
            (
                "SOG §3.3 branch immed / branch register / compare and branch",
                "SOG §3.3 — reworded, same number",
            ),
            ("DSU TRM 100453: a load", "DSU TRM 100453 (rev B): a load"),
            ("deleting a call is inlining and", "inlining, and"),
        ] {
            assert!(text.contains(needle), "fixture text drifted: {needle}");
            let edited = parse(&text.replace(needle, replacement)).expect("parse edited");
            assert_ne!(
                base.provenance_digest(),
                edited.provenance_digest(),
                "rewriting `{needle}` must move the provenance digest"
            );
            assert_eq!(
                base.table_digest(),
                edited.table_digest(),
                "no value moved, so the content digest must not move: `{needle}`"
            );
        }
    }

    #[test]
    fn profile_ghz_reads_the_committed_profile() {
        assert_eq!(profile_ghz(), parse(&committed_text()).unwrap().ghz());
    }

    #[test]
    fn default_path_is_the_a76_profile() {
        assert!(
            default_table_path().ends_with("bench/a76-pi5.toml"),
            "got {}",
            default_table_path().display()
        );
    }

    #[test]
    fn pipe_range_counts_pipes() {
        assert_eq!(pipe_range("4"), Some((4, 4)));
        assert_eq!(pipe_range("2-4"), Some((2, 4)));
        assert_eq!(pipe_range("0"), None);
        assert_eq!(pipe_range("4-2"), None);
        assert_eq!(pipe_range("a76-pi5"), None);
        assert_eq!(pipe_range("2.4"), None);
    }

    #[test]
    fn reject_issue_width() {
        assert_err_names(
            format!("issue_width = 4\n{}", committed_text()),
            "issue_width",
        );
    }

    #[test]
    fn reject_plain_version_2() {
        assert_err_names("version = 2\n".to_string(), "expected 3");
    }

    #[test]
    fn reject_version_2_carrying_v3_sections() {
        let text = committed_text().replace("version = 3", "version = 2");
        let e = err_of(text);
        assert!(e.contains("half-migrated"), "got: {e}");
        assert!(
            e.contains("pipelines"),
            "should name a v3 section, got: {e}"
        );
    }

    #[test]
    fn reject_v2_sections_at_v3() {
        for stale in ["[ports]\nalu = 2\n", "[mem]\nstore_stack = 1\n"] {
            let e = err_of(format!("{}\n{stale}", committed_text()));
            assert!(e.contains("retired section"), "got: {e}");
        }
    }

    #[test]
    fn reject_missing_section() {
        for s in V3_SECTIONS {
            let e = err_of(without_section(&committed_text(), s));
            assert!(
                e.contains(&format!("[{s}]")),
                "removing [{s}.*] should name the missing section, got: {e}"
            );
        }
    }

    #[test]
    fn reject_unknown_top_level_key() {
        assert_err_names(
            format!("bogus_knob = 7\n{}", committed_text()),
            "unknown top-level key",
        );
    }

    #[test]
    fn reject_unknown_row() {
        assert_err_names(
            format!(
                "{}\n[geometry.l4_bytes]\nvalue = 1\ntier = \"T1\"\nsource = \"x\"\n",
                committed_text()
            ),
            "unknown row [geometry.l4_bytes]",
        );
    }

    #[test]
    fn reject_unknown_key_in_row() {
        assert_err_names(
            add_to_block(&committed_text(), "[geometry.l3_ways]", "guess = 1"),
            "unknown key `guess`",
        );
    }

    #[test]
    fn reject_missing_required_row() {
        assert_err_names(
            without_block(&committed_text(), "[geometry.l3_ways]"),
            "missing row [geometry.l3_ways]",
        );
        assert_err_names(
            without_block(&committed_text(), "[branch.mispredict_penalty]"),
            "missing row [branch.mispredict_penalty]",
        );
    }

    #[test]
    fn reject_missing_key_within_a_row() {
        assert_err_names(
            without_line(&committed_text(), "[geometry.l3_ways]", "source ="),
            "missing geometry.l3_ways.source",
        );
        assert_err_names(
            without_line(&committed_text(), "[latency.mul]", "thru_den ="),
            "missing latency.mul.thru_den",
        );
    }

    fn without_line(text: &str, header: &str, prefix: &str) -> String {
        let mut out = String::new();
        let mut inside = false;
        let mut done = false;
        for line in text.lines() {
            let t = line.trim_start();
            if t.starts_with('[') {
                inside = t.starts_with(header);
            }
            if inside && !done && t.starts_with(prefix) {
                done = true;
                continue;
            }
            out.push_str(line);
            out.push('\n');
        }
        assert!(done, "without_line: `{prefix}` not found in {header}");
        out
    }

    #[test]
    fn reject_negative_value() {
        assert_err_names(
            edit_block(
                &committed_text(),
                "[geometry.tlb_l2_entries]",
                "value = 1280",
                "value = -1",
            ),
            "must be >= 0",
        );
    }

    #[test]
    fn reject_zero_where_illegal() {
        assert_err_names(
            edit_block(
                &committed_text(),
                "[geometry.l1d_ways]",
                "value = 4",
                "value = 0",
            ),
            "geometry.l1d_ways must be > 0",
        );
        assert_err_names(
            edit_block(
                &committed_text(),
                "[pipelines.dispatch_uops]",
                "value = 8",
                "value = 0",
            ),
            "pipelines.dispatch_uops must be > 0",
        );
        assert_err_names(
            edit_block(
                &committed_text(),
                "[latency.alu]",
                "thru_num = 3",
                "thru_num = 0",
            ),
            "thru_num / thru_den must both be > 0",
        );
    }

    #[test]
    fn reject_non_monotone_leaf_latencies() {
        assert_err_names(
            edit_block(
                &committed_text(),
                "[geometry.lat_l1d_hit]",
                "value = 4",
                "value = 12",
            ),
            "must strictly increase",
        );
    }

    #[test]
    fn reject_l2_not_inclusive_of_l1d() {
        assert_err_names(
            edit_block(
                &committed_text(),
                "[geometry.l2_inclusive_of_l1d]",
                "value = 1",
                "value = 0",
            ),
            "strictly inclusive",
        );
    }

    #[test]
    fn reject_port_beyond_the_pipeline_count() {
        assert_err_names(
            edit_block(
                &committed_text(),
                "[pipelines.port_v1]",
                "value = \"8\"",
                "value = \"9\"",
            ),
            "exceeds pipelines.count",
        );
    }

    #[test]
    fn reject_profile_name_other_than_the_one_profile() {
        assert_err_names(
            edit_block(
                &committed_text(),
                "[profile.name]",
                "value = \"a76-pi5\"",
                "value = \"rk3588\"",
            ),
            "only profile",
        );
    }

    #[test]
    fn reject_bad_ghz() {
        for bad in ["0", "-1", "nan", "abc"] {
            let e = err_of(edit_block(
                &committed_text(),
                "[profile.ghz]",
                "value = \"2.4\"",
                &format!("value = \"{bad}\""),
            ));
            assert!(
                e.contains("ghz"),
                "ghz `{bad}` should fail closed, got: {e}"
            );
        }
    }

    #[test]
    fn reject_pinned_t5_row() {
        let cases = [
            ("[latency.load]", "tier = \"T1\""),
            ("[latency.mul_high]", "tier = \"T1\""),
            ("[geometry.lat_l3]", "tier = \"T4\""),
            ("[geometry.l1d_ways]", "tier = \"T2\""),
            ("[branch.mispredict_penalty]", "tier = \"T4\""),
            ("[align.load_line_bytes]", "tier = \"T1\""),
            ("[pipelines.reorder_window]", "tier = \"T4\""),
            ("[profile.ghz]", "tier = \"T2\""),
        ];
        for (header, from) in cases {
            let e = err_of(edit_block(&committed_text(), header, from, "tier = \"T5\""));
            assert!(
                e.contains("T5") && e.contains("cannot be pinned"),
                "{header} pinned at T5 must be refused, got: {e}"
            );
        }
    }

    #[test]
    fn reject_bad_tier() {
        assert_err_names(
            edit_block(
                &committed_text(),
                "[geometry.l3_ways]",
                "tier = \"T2\"",
                "tier = \"T6\"",
            ),
            "expected one of T1..T5",
        );
    }

    #[test]
    fn reject_pinned_value_that_is_not_the_pessimistic_end() {
        let e = err_of(edit_block(
            &committed_text(),
            "[sweep.l3_latency]",
            "pinned = 35",
            "pinned = 26",
        ));
        assert!(e.contains("pessimistic end"), "got: {e}");
        let e = err_of(edit_block(
            &committed_text(),
            "[sweep.effective_l3_bytes]",
            "pinned = 1048576",
            "pinned = 2097152",
        ));
        assert!(e.contains("pessimistic end"), "got: {e}");
    }

    #[test]
    fn reject_bad_pessimistic_end() {
        assert_err_names(
            edit_block(
                &committed_text(),
                "[sweep.l2_latency]",
                "pessimistic = \"hi\"",
                "pessimistic = \"middle\"",
            ),
            "expected `lo` or `hi`",
        );
    }

    #[test]
    fn reject_sweep_pinned_outside_its_bracket() {
        assert_err_names(
            edit_block(
                &committed_text(),
                "[sweep.l2_latency]",
                "pinned = 11",
                "pinned = 99",
            ),
            "outside 9..11",
        );
    }

    #[test]
    fn the_committed_snoop_constraint_is_parsed_and_names_its_physics() {
        let t = load_default().expect("committed profile");
        let b = t
            .sweep("snoop_cost")
            .expect("[sweep.snoop_cost]")
            .le
            .as_ref()
            .expect("decision 1951's constraint");
        assert_eq!(b.expr(), "dram_latency - l3_latency");
        assert_eq!(b.minuend, "dram_latency");
        assert_eq!(b.subtrahend.as_deref(), Some("l3_latency"));
        assert!(
            b.physics.contains("DRAM path"),
            "the constraint must say why it holds on the machine: {}",
            b.physics
        );
        let constrained: Vec<&str> = t
            .sweep_dimensions()
            .into_iter()
            .filter(|d| t.sweep(d).expect("row").le.is_some())
            .collect();
        assert_eq!(constrained, vec!["snoop_cost"]);
    }

    fn edit_line_starting(text: &str, header: &str, prefix: &str, to: &str) -> String {
        let mut out = String::new();
        let mut inside = false;
        let mut done = false;
        for line in text.lines() {
            let t = line.trim_start();
            if t.starts_with('[') {
                inside = t.starts_with(header);
            }
            if inside && !done && t.starts_with(prefix) {
                out.push_str(to);
                out.push('\n');
                done = true;
                continue;
            }
            out.push_str(line);
            out.push('\n');
        }
        assert!(done, "edit_line_starting: `{prefix}` not found in {header}");
        out
    }

    const SNOOP: &str = "[sweep.snoop_cost]";

    #[test]
    fn reject_a_constraint_with_no_physics() {
        assert_err_names(
            edit_line_starting(&committed_text(), SNOOP, "le_physics", "# no physics"),
            "requires `le_physics`",
        );
    }

    #[test]
    fn reject_a_constraint_naming_no_such_dimension() {
        assert_err_names(
            edit_line_starting(
                &committed_text(),
                SNOOP,
                "le = ",
                "le = \"dram_latency - not_a_dimension\"",
            ),
            "no such [sweep.not_a_dimension] row",
        );
    }

    #[test]
    fn reject_a_self_referential_constraint() {
        assert_err_names(
            edit_line_starting(&committed_text(), SNOOP, "le = ", "le = \"snoop_cost\""),
            "cannot bound itself",
        );
    }

    #[test]
    fn reject_a_vacuous_constraint() {
        assert_err_names(
            edit_line_starting(
                &committed_text(),
                SNOOP,
                "le = ",
                "le = \"effective_l3_bytes\"",
            ),
            "vacuous",
        );
    }

    #[test]
    fn reject_a_constraint_the_pinned_point_violates() {
        let text = edit_block(
            &committed_text(),
            "[sweep.dram_latency]",
            "hi = 347",
            "hi = 340",
        );
        let e = err_of(edit_block(
            &text,
            "[sweep.dram_latency]",
            "pinned = 347",
            "pinned = 340",
        ));
        assert!(
            e.contains("pinned point violates it"),
            "lowering dram's high end below snoop + l3 must be refused: {e}"
        );
    }

    #[test]
    fn a_constraint_moves_the_value_digest_and_its_physics_the_provenance_one() {
        let base = load_default().expect("committed profile");
        let dropped = err_of(edit_line_starting(
            &committed_text(),
            SNOOP,
            "le = ",
            "# le removed",
        ));
        assert!(
            dropped.contains("le_physics without `le`"),
            "got: {dropped}"
        );

        let reworded = parse(&edit_line_starting(
            &committed_text(),
            SNOOP,
            "le_physics",
            "le_physics = \"reworded\"",
        ))
        .expect("reworded physics still parses");
        assert_eq!(
            base.table_digest(),
            reworded.table_digest(),
            "rewording the physics moves no value"
        );
        assert_ne!(
            base.provenance_digest(),
            reworded.provenance_digest(),
            "rewording the physics must move the provenance digest"
        );

        let unconstrained = parse(&edit_line_starting(
            &edit_line_starting(&committed_text(), SNOOP, "le = ", "# none"),
            SNOOP,
            "le_physics",
            "# none",
        ))
        .expect("dropping both fields leaves an independent bracket");
        assert_ne!(
            base.table_digest(),
            unconstrained.table_digest(),
            "the constraint is box shape and must move the value digest"
        );
    }

    #[test]
    fn reject_sweep_lo_not_below_hi() {
        assert_err_names(
            edit_block(&committed_text(), "[sweep.l2_latency]", "lo = 9", "lo = 11"),
            "must be < hi",
        );
    }

    #[test]
    fn reject_removal_sensitive_pinning_the_high_end() {
        let text = edit_block(
            &committed_text(),
            "[sweep.dmb_cost]",
            "pessimistic = \"lo\"",
            "pessimistic = \"hi\"",
        );
        let e = err_of(edit_block(
            &text,
            "[sweep.dmb_cost]",
            "pinned = 1",
            "pinned = 64",
        ));
        assert!(e.contains("over-credits deletion"), "got: {e}");
    }

    #[test]
    fn reject_removal_sensitive_without_an_ambiguity_note() {
        assert_err_names(
            without_line(&committed_text(), "[sweep.dmb_cost]", "ambiguity ="),
            "requires an `ambiguity` note",
        );
    }

    #[test]
    fn reject_bracket_disagreeing_with_its_sweep_dimension() {
        assert_err_names(
            edit_block(
                &committed_text(),
                "[geometry.lat_l2]",
                "bracket_lo = 9",
                "bracket_lo = 8",
            ),
            "disagrees with [sweep.l2_latency]",
        );
    }

    #[test]
    fn reject_bracket_without_a_sweep_dimension() {
        assert_err_names(
            without_line(&committed_text(), "[geometry.lat_l3]", "sweep ="),
            "names no sweep dimension",
        );
    }

    #[test]
    fn reject_unknown_sweep_reference() {
        assert_err_names(
            edit_block(
                &committed_text(),
                "[geometry.lat_l3]",
                "sweep = \"l3_latency\"",
                "sweep = \"nope\"",
            ),
            "no such [sweep.nope] row",
        );
        assert_err_names(
            edit_block(
                &committed_text(),
                "[latency.call]",
                "sweep_add = \"call_overhead\"",
                "sweep_add = \"nope\"",
            ),
            "no such [sweep.nope] row",
        );
    }

    #[test]
    fn reject_crosscore_term_carrying_a_magnitude() {
        for pinned in [
            "value = 1",
            "lat = 1",
            "cost = 40",
            "cycles = 40",
            "pinned = 40",
        ] {
            let e = err_of(add_to_block(&committed_text(), "[crosscore.dmb]", pinned));
            assert!(
                e.contains("may not be pinned at all"),
                "`{pinned}` in [crosscore.dmb] must be refused, got: {e}"
            );
        }
    }

    #[test]
    fn reject_crosscore_term_that_is_not_t5() {
        assert_err_names(
            edit_block(
                &committed_text(),
                "[crosscore.dmb]",
                "tier = \"T5\"",
                "tier = \"T1\"",
            ),
            "T5 by absence",
        );
    }

    #[test]
    fn reject_crosscore_unknown_base_or_sweep() {
        assert_err_names(
            edit_block(
                &committed_text(),
                "[crosscore.load_acquire]",
                "base = \"load\"",
                "base = \"nope\"",
            ),
            "no such [latency.nope] row",
        );
        assert_err_names(
            edit_block(
                &committed_text(),
                "[crosscore.dmb]",
                "sweep = \"dmb_cost\"",
                "sweep = \"nope\"",
            ),
            "no such [sweep.nope] row",
        );
    }

    #[test]
    fn reject_latency_group_that_is_not_a_costrule() {
        assert_err_names(
            format!(
                "{}\n[latency.ldp]\nlat = 4\nthru_num = 2\nthru_den = 1\nports = \"L\"\n\
                 tier = \"T1\"\nsource = \"SOG §3.9\"\n",
                committed_text()
            ),
            "not a CostRule key",
        );
    }

    #[test]
    fn reject_costrule_priced_twice() {
        assert_err_names(
            add_to_block(&committed_text(), "[crosscore.snoop]", "rule = \"load\""),
            "priced twice",
        );
    }

    #[test]
    fn reject_costrule_priced_by_neither_section() {
        assert_err_names(
            without_block(&committed_text(), "[crosscore.dmb]"),
            "priced by neither",
        );
        assert_err_names(
            without_block(&committed_text(), "[latency.mul_high]"),
            "priced by neither",
        );
    }

    #[test]
    fn reject_a_table_still_carrying_the_transitional_quarantine() {
        let revived = format!(
            "{}\n[transitional.mem_reuse_window]\nvalue = 8\ntier = \"T5\"\n\
             source = \"none — v2 scorer holdover\"\ndeleted_by = \"item F\"\n",
            committed_text()
        );
        let err = parse(&revived).expect_err("the quarantine must not load");
        assert!(
            err.contains("retired section `[transitional]`"),
            "got: {err}"
        );
        assert!(err.contains("item F"), "got: {err}");
        assert!(err.contains("reuse distance"), "got: {err}");
        for stale in ["ports", "mem"] {
            let err = parse(&format!(
                "{}\n[{stale}.alu]\nvalue = 2\ntier = \"T1\"\nsource = \"x\"\n",
                committed_text()
            ))
            .expect_err("v2 section must not load");
            assert!(
                err.contains(&format!("retired section `[{stale}]`")),
                "got: {err}"
            );
        }
    }

    #[test]
    fn the_deleted_knobs_cannot_reappear_in_a_sourced_section() {
        for knob in [
            "mem_reuse_window",
            "mem_working_set_cap",
            "working_set_surcharge",
        ] {
            let text = format!(
                "{}\n[geometry.{knob}]\nvalue = 8\ntier = \"T2\"\n\
                 source = \"Cortex-A76 Core TRM 100798\"\n",
                committed_text()
            );
            let err = parse(&text).expect_err("relocated knob must not load");
            assert!(err.contains("unknown row"), "{knob}: got {err}");
        }
    }

    #[test]
    fn reject_levels_that_disagree_on_the_line_size() {
        assert_err_names(
            edit_block(
                &committed_text(),
                "[geometry.l2_line_bytes]",
                "value = 64",
                "value = 128",
            ),
            "one line identity",
        );
    }

    #[test]
    fn load_committed_table() {
        let t = load_default().expect("load bench/a76-pi5.toml");
        assert_eq!(t.version, EXPECTED_VERSION);
        assert_eq!(t.profile_name(), "a76-pi5");
        assert_eq!(t.max_issue_per_cycle(), 8);
        assert_eq!(t.alu_ports(), 3);
        assert_eq!(t.latency(CostRule::Alu), 1);
        assert_eq!(t.latency(CostRule::Load), 4);
        assert_eq!(t.geometry("lat_l1d_hit").expect("row").value, 4);
        assert!(t.geometry("mem_reuse_window").is_none());
        assert!(t.geometry("mem_working_set_cap").is_none());
        assert!(t.geometry("working_set_surcharge").is_none());
        assert!(
            !committed_text().contains("\n[transitional."),
            "the committed profile must not carry the quarantine"
        );
        let again = load_default().expect("reload");
        assert_eq!(t.table_digest(), again.table_digest());
        assert_eq!(t.provenance_digest(), again.provenance_digest());
    }
}
