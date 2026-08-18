use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use crate::codegen::CodegenProgram;
use crate::cost::crosscore::{
    OrderingCounts, OrderingRemoval, ordering_removals, ordering_word_counts,
};
use crate::cost::footprint::CoreBudget;
use crate::cost::score::{CostReport, ScoreCtx, score_totals_at};
use crate::cost::stage::{
    ShippedFront, TextScope, codegen_shipped_from, codegen_shipped_program, load_shipped_front,
    report_cost_stage_path,
};
use crate::cost::sweep::{SweepPoint, endpoint_corners, record_reads};
use crate::cost::table::{CostTable, load_default};
use crate::cost::workload::{self, FLAT_NAME, WorkloadSet};
use crate::placement::PlacementTable;

use super::{CompileMode, OptId, RELEASE_OPTS, apply_mode, apply_opts};

#[derive(Debug, Clone)]
pub struct CaseDelta {
    pub name: String,
    pub tier: CostTier,
    pub scope: TextScope,
    pub baseline: u64,
    pub candidate: u64,
    pub baseline_words: u64,
    pub candidate_words: u64,
    pub baseline_budgets: Vec<CoreBudget>,
    pub candidate_budgets: Vec<CoreBudget>,
    pub baseline_ordering: OrderingCounts,
    pub candidate_ordering: OrderingCounts,
}

impl CaseDelta {
    pub fn delta(&self) -> i64 {
        self.candidate as i64 - self.baseline as i64
    }

    pub fn words_delta(&self) -> i64 {
        self.candidate_words as i64 - self.baseline_words as i64
    }

    pub fn ordering_removed(&self) -> Vec<OrderingRemoval> {
        ordering_removals(&self.baseline_ordering, &self.candidate_ordering)
    }

    pub fn budget_growth(&self) -> Result<Vec<BudgetGrowth>, String> {
        budget_overflow_growth(&self.baseline_budgets, &self.candidate_budgets)
    }
}

fn total_charge(budgets: &[CoreBudget]) -> u64 {
    budgets.iter().fold(0u64, |a, b| a.saturating_add(b.charge))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BudgetGrowth {
    pub core: usize,
    pub field: &'static str,
    pub baseline: u64,
    pub candidate: u64,
}

impl BudgetGrowth {
    pub fn label(&self) -> String {
        format!(
            "budget_grew:core{}:{}:{}->{}",
            self.core, self.field, self.baseline, self.candidate
        )
    }
}

fn over_budget_quantities(b: &CoreBudget) -> [(&'static str, u64); 8] {
    [
        ("over_l1i_lines", b.over_l1i_lines),
        ("over_l2_lines", b.over_l2_lines),
        ("over_l3_lines", b.over_l3_lines),
        ("over_itlb_pages", b.over_itlb_pages),
        ("over_tlb_l2_pages", b.over_tlb_l2_pages),
        ("over_dtlb_pages", b.over_dtlb_pages),
        ("over_data_tlb_l2_pages", b.over_data_tlb_l2_pages),
        ("charge", b.charge),
    ]
}

pub fn budget_overflow_growth(
    baseline: &[CoreBudget],
    candidate: &[CoreBudget],
) -> Result<Vec<BudgetGrowth>, String> {
    if baseline.len() != candidate.len() {
        return Err(format!(
            "budget: core count changed {}->{} — the two sides were placed \
             on different machines, which is an error rather than a rank",
            baseline.len(),
            candidate.len()
        ));
    }
    let mut out = Vec::new();
    for (b, c) in baseline.iter().zip(candidate.iter()) {
        if b.n != c.n {
            return Err(format!("budget: core index mismatch {} vs {}", b.n, c.n));
        }
        for ((field, bv), (_, cv)) in over_budget_quantities(b)
            .into_iter()
            .zip(over_budget_quantities(c))
        {
            if cv > bv {
                out.push(BudgetGrowth {
                    core: b.n,
                    field,
                    baseline: bv,
                    candidate: cv,
                });
            }
        }
    }
    Ok(out)
}

#[derive(Debug, Clone)]
pub struct CorpusCompare {
    pub cases: Vec<CaseDelta>,
    pub baseline_sum: u64,
    pub candidate_sum: u64,
    pub baseline_words: u64,
    pub candidate_words: u64,
}

impl CorpusCompare {
    pub fn sum_delta(&self) -> i64 {
        self.candidate_sum as i64 - self.baseline_sum as i64
    }

    pub fn words_delta(&self) -> i64 {
        self.candidate_words as i64 - self.baseline_words as i64
    }

    fn non_regressing(&self) -> bool {
        self.cases.iter().all(|c| {
            c.candidate <= c.baseline
                && c.budget_growth().is_ok_and(|growth| growth.is_empty())
                && c.ordering_removed().is_empty()
        })
    }

    pub fn wins(&self) -> bool {
        self.non_regressing() && self.cases.iter().any(|c| c.candidate < c.baseline)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum CostTier {
    Micro,
    Product,
}

impl CostTier {
    pub fn as_str(self) -> &'static str {
        match self {
            CostTier::Micro => "micro",
            CostTier::Product => "product",
        }
    }

    pub const ALL: [CostTier; 2] = [CostTier::Micro, CostTier::Product];
}

impl std::fmt::Display for CostTier {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CostCase {
    pub name: String,
    pub tier: CostTier,
    pub input: PathBuf,
}

fn golden_root() -> PathBuf {
    normalize_lexically(&PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tests/golden"))
}

fn normalize_lexically(p: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for c in p.components() {
        match c {
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => {
                if !out.pop() {
                    out.push("..");
                }
            }
            other => out.push(other),
        }
    }
    out
}

fn contains_wr_source(dir: &Path) -> bool {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return false;
    };
    for entry in entries.flatten() {
        let p = entry.path();
        if p.is_dir() {
            if p.file_name().is_some_and(|n| n == "expected") {
                continue;
            }
            if contains_wr_source(&p) {
                return true;
            }
        } else if p.extension().is_some_and(|e| e == "wr") {
            return true;
        }
    }
    false
}

pub fn classify_cost_case(dir: &Path) -> Result<CostCase, String> {
    let dir = &normalize_lexically(dir);
    let name = dir
        .file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .ok_or_else(|| format!("cost case {} has no directory name", dir.display()))?;
    let flat = dir.join("input.wr");
    let root_marker = dir.join("root");
    let has_flat = flat.is_file();
    let has_root = root_marker.is_file();

    match (has_flat, has_root) {
        (true, true) => Err(format!(
            "{name}: carries both `input.wr` and `root` — which program the \
             gate scores is ambiguous, and an ambiguous case cannot be tiered"
        )),
        (false, false) => Err(format!(
            "{name}: has neither `input.wr` nor `root`, so there is no program \
             to score. A `cost-*` directory is a corpus case; if it is not one, \
             it does not belong under that prefix"
        )),
        (true, false) => Ok(CostCase {
            name,
            tier: CostTier::Micro,
            input: normalize_lexically(&flat),
        }),
        (false, true) => {
            let rel = std::fs::read_to_string(&root_marker)
                .map_err(|e| format!("{name}: read {}: {e}", root_marker.display()))?;
            let rel = rel.trim().to_string();
            if rel.is_empty() {
                return Err(format!("{name}: `root` file is empty"));
            }
            let target = normalize_lexically(&dir.join(&rel));
            if !target.is_file() {
                return Err(format!(
                    "{name}: `root` names `{rel}`, which is not a file \
                     ({}) — a borrowed case whose program moved must fail the \
                     corpus, not vanish from it",
                    target.display()
                ));
            }
            let owns_source = contains_wr_source(dir);
            let borrowed = !target.starts_with(dir);
            match (borrowed, owns_source) {
                (true, false) => Ok(CostCase {
                    name,
                    tier: CostTier::Product,
                    input: target,
                }),
                (false, true) => Ok(CostCase {
                    name,
                    tier: CostTier::Micro,
                    input: target,
                }),
                (true, true) => Err(format!(
                    "{name}: `root` points outside the case (`{rel}`) but the \
                     case also contains `.wr` source. A product-scale case is \
                     one that owns no program of its own — that is the whole \
                     reason nobody can have tuned it for the gate"
                )),
                (false, false) => Err(format!(
                    "{name}: `root` names `{rel}` inside the case, but the case \
                     contains no `.wr` source"
                )),
            }
        }
    }
}

pub fn try_discover_cost_cases() -> Result<Vec<CostCase>, String> {
    let root = golden_root();
    let entries = std::fs::read_dir(&root).map_err(|e| format!("read {}: {e}", root.display()))?;
    let mut dirs = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|e| format!("read_dir {}: {e}", root.display()))?;
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        if !entry.file_name().to_string_lossy().starts_with("cost-") {
            continue;
        }
        dirs.push(path);
    }
    dirs.sort();
    let mut cases = Vec::with_capacity(dirs.len());
    for d in &dirs {
        cases.push(classify_cost_case(d)?);
    }
    Ok(cases)
}

pub fn discover_cost_cases() -> Vec<CostCase> {
    try_discover_cost_cases().unwrap_or_else(|e| panic!("cost corpus: {e}"))
}

pub fn discover_cost_cases_in(tier: CostTier) -> Vec<CostCase> {
    discover_cost_cases()
        .into_iter()
        .filter(|c| c.tier == tier)
        .collect()
}

pub fn discover_cost_corpus() -> Vec<PathBuf> {
    discover_cost_cases().into_iter().map(|c| c.input).collect()
}

pub fn score_path_under_opts(path: &Path, opts: &[OptId]) -> u64 {
    report_path_under_opts(path, opts).total_proxy_cycles
}

pub fn report_path_under_opts(path: &Path, opts: &[OptId]) -> CostReport {
    apply_opts(opts);
    report_cost_stage_path(path).unwrap_or_else(|e| {
        panic!("cost-stage score {}: {e}", path.display());
    })
}

fn shipped_report_under_opts(path: &Path, opts: &[OptId]) -> (CostReport, TextScope) {
    apply_opts(opts);
    let (program, placement, scope) = codegen_shipped_program(path)
        .unwrap_or_else(|e| panic!("shipped-program score {}: {e}", path.display()));
    let table = load_default().unwrap_or_else(|e| panic!("cost table: {e}"));
    let report = crate::cost::score::score_program(&program, &table, &placement)
        .unwrap_or_else(|e| panic!("score {}: {e}", path.display()));
    (report, scope)
}

fn linked_shipped_report_under_opts(path: &Path, opts: &[OptId]) -> (CostReport, TextScope) {
    apply_opts(opts);
    let (linked, placement, scope) = crate::cost::stage::linked_shipped_program(path)
        .unwrap_or_else(|e| panic!("linked-program score {}: {e}", path.display()));
    let table = load_default().unwrap_or_else(|e| panic!("cost table: {e}"));
    let report = match scope {
        TextScope::Image => crate::cost::score::score_linked_program(&linked, &table, &placement),
        TextScope::Closure => {
            let program = crate::cost::stage::codegen_shipped_program(path)
                .unwrap_or_else(|e| panic!("shipped-program score {}: {e}", path.display()))
                .0;
            crate::cost::score::score_program(&program, &table, &placement)
        }
    }
    .unwrap_or_else(|e| panic!("score {}: {e}", path.display()));
    (report, scope)
}

pub fn compare_opt_lists(baseline: &[OptId], candidate: &[OptId]) -> CorpusCompare {
    let corpus = discover_cost_cases();
    assert!(
        !corpus.is_empty(),
        "cost corpus empty: expected tests/golden/cost-*/input.wr"
    );

    let mut cases = Vec::with_capacity(corpus.len());
    let mut baseline_sum = 0u64;
    let mut candidate_sum = 0u64;
    let mut baseline_words = 0u64;
    let mut candidate_words = 0u64;

    for case in &corpus {
        let path = case.input.as_path();
        let name = case.name.clone();
        let scorer = if case.tier == CostTier::Product {
            linked_shipped_report_under_opts
        } else {
            shipped_report_under_opts
        };
        let (b, scope) = scorer(path, baseline);
        let (c, cscope) = scorer(path, candidate);
        assert_eq!(
            scope, cscope,
            "{name}: the two sides compiled different programs — one shipped an image \
             and the other did not, which is an error rather than a rank"
        );
        baseline_sum = baseline_sum.saturating_add(b.total_proxy_cycles);
        candidate_sum = candidate_sum.saturating_add(c.total_proxy_cycles);
        baseline_words = baseline_words.saturating_add(b.total_words);
        candidate_words = candidate_words.saturating_add(c.total_words);
        cases.push(CaseDelta {
            name,
            tier: case.tier,
            scope,
            baseline: b.total_proxy_cycles,
            candidate: c.total_proxy_cycles,
            baseline_words: b.total_words,
            candidate_words: c.total_words,
            baseline_budgets: b.footprint.clone(),
            candidate_budgets: c.footprint.clone(),
            baseline_ordering: ordering_word_counts(&b),
            candidate_ordering: ordering_word_counts(&c),
        });
    }

    apply_mode(CompileMode::Release);
    CorpusCompare {
        cases,
        baseline_sum,
        candidate_sum,
        baseline_words,
        candidate_words,
    }
}

pub fn assert_release_wins_cost_corpus() -> CorpusCompare {
    let cmp = compare_opt_lists(&[], RELEASE_OPTS);
    assert_wins(&cmp, "release", "dev");
    cmp
}

pub fn assert_candidate_wins(baseline: &[OptId], candidate: &[OptId]) -> CorpusCompare {
    let cmp = compare_opt_lists(baseline, candidate);
    assert_wins(&cmp, "candidate", "baseline");
    cmp
}

fn assert_wins(cmp: &CorpusCompare, cand_label: &str, base_label: &str) {
    let table = format_delta_table(cmp, base_label, cand_label);
    let mut rose = Vec::new();
    let mut grew = Vec::new();
    let mut unordered = Vec::new();
    let mut any_fall = false;
    for c in &cmp.cases {
        for r in c.ordering_removed() {
            unordered.push(format!("{}: {}", c.name, r.label()));
        }
        if c.candidate > c.baseline {
            rose.push(format!(
                "{}: {cand_label} {} > {base_label} {}",
                c.name, c.candidate, c.baseline
            ));
        }
        match c.budget_growth() {
            Ok(growth) => {
                for g in growth {
                    grew.push(format!("{}: {}", c.name, g.label()));
                }
            }
            Err(e) => grew.push(format!("{}: {e}", c.name)),
        }
        if c.candidate < c.baseline {
            any_fall = true;
        }
    }
    assert!(
        rose.is_empty(),
        "{cand_label} raised proxy total on {} case(s):\n{}\n{table}",
        rose.len(),
        rose.join("\n"),
    );
    assert!(
        grew.is_empty(),
        "{cand_label} grew a per-core text/TLB budget overflow on {} \
         case(s) — 04 §5 makes that budget the hard constraint the emitted \
         word count no longer is:\n{}\n{table}",
        grew.len(),
        grew.join("\n"),
    );
    assert!(
        unordered.is_empty(),
        "{cand_label} deleted correctness-load-bearing ordering words on {} case(s) — \
         freeze 1633 refuses barrier removal however the cycles come out:\n{}\n{table}",
        unordered.len(),
        unordered.join("\n"),
    );
    assert!(
        any_fall,
        "{cand_label} must strictly lower at least one cost-* case vs {base_label}\n{table}"
    );
}

pub fn format_delta_table(cmp: &CorpusCompare, base_label: &str, cand_label: &str) -> String {
    let mut out = String::new();
    out.push_str(&format!(
        "{:<24} {:<8} {:>12} {:>12} {:>10} {:>10} {:>10} {:>8} {:>8} {:>8} {:>7} {:>7}\n",
        "case",
        "tier",
        base_label,
        cand_label,
        "Δ",
        "words_b",
        "words_c",
        "Δw",
        "chg_b",
        "chg_c",
        "Δchg",
        "cores"
    ));
    for c in &cmp.cases {
        let cb = total_charge(&c.baseline_budgets);
        let cc = total_charge(&c.candidate_budgets);
        out.push_str(&format!(
            "{:<24} {:<8} {:>12} {:>12} {:>+10} {:>10} {:>10} {:>+8} {:>8} {:>8} {:>+7} {:>7}\n",
            c.name,
            c.tier.as_str(),
            c.baseline,
            c.candidate,
            c.delta(),
            c.baseline_words,
            c.candidate_words,
            c.words_delta(),
            cb,
            cc,
            cc as i64 - cb as i64,
            c.baseline_budgets.len(),
        ));
    }
    for tier in CostTier::ALL {
        let rows: Vec<&CaseDelta> = cmp.cases.iter().filter(|c| c.tier == tier).collect();
        if rows.is_empty() {
            continue;
        }
        let b: u64 = rows.iter().map(|c| c.baseline).sum();
        let d: u64 = rows.iter().map(|c| c.candidate).sum();
        let wb: u64 = rows.iter().map(|c| c.baseline_words).sum();
        let wc: u64 = rows.iter().map(|c| c.candidate_words).sum();
        let cb: u64 = rows.iter().map(|c| total_charge(&c.baseline_budgets)).sum();
        let cc: u64 = rows
            .iter()
            .map(|c| total_charge(&c.candidate_budgets))
            .sum();
        out.push_str(&format!(
            "{:<24} {:<8} {:>12} {:>12} {:>+10} {:>10} {:>10} {:>+8} {:>8} {:>8} {:>+7} {:>7}\n",
            format!("SUB[{}]", tier.as_str()),
            format!("n={}", rows.len()),
            b,
            d,
            d as i64 - b as i64,
            wb,
            wc,
            wc as i64 - wb as i64,
            cb,
            cc,
            cc as i64 - cb as i64,
            "-"
        ));
    }
    let sum_cb: u64 = cmp
        .cases
        .iter()
        .map(|c| total_charge(&c.baseline_budgets))
        .sum();
    let sum_cc: u64 = cmp
        .cases
        .iter()
        .map(|c| total_charge(&c.candidate_budgets))
        .sum();
    out.push_str(&format!(
        "{:<24} {:<8} {:>12} {:>12} {:>+10} {:>10} {:>10} {:>+8} {:>8} {:>8} {:>+7} {:>7}\n",
        "SUM",
        "both",
        cmp.baseline_sum,
        cmp.candidate_sum,
        cmp.sum_delta(),
        cmp.baseline_words,
        cmp.candidate_words,
        cmp.words_delta(),
        sum_cb,
        sum_cc,
        sum_cc as i64 - sum_cb as i64,
        "-"
    ));
    out
}

pub const MAX_SWEPT_DIMS: usize = 14;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HeldDim {
    pub dim: String,
    pub lo: u64,
    pub hi: u64,
    pub pinned: u64,
}

impl HeldDim {
    pub fn label(&self) -> String {
        format!("{}[{}..{}]@{}", self.dim, self.lo, self.hi, self.pinned)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PointRow {
    pub point: String,
    pub baseline: u64,
    pub candidate: u64,
    pub baseline_charge: u64,
    pub candidate_charge: u64,
}

impl PointRow {
    pub fn delta(&self) -> i64 {
        self.candidate as i64 - self.baseline as i64
    }
}

#[derive(Debug, Clone)]
pub struct CaseSweep {
    pub name: String,
    pub tier: CostTier,
    pub box_dims: usize,
    pub box_cardinality: u64,
    pub swept: Vec<String>,
    pub held: Vec<HeldDim>,
    pub read_but_static: Vec<String>,
    pub points: Vec<PointRow>,
}

impl CaseSweep {
    pub fn corners(&self) -> usize {
        self.points.len()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SweepVeto {
    CaseRose {
        case: String,
        point: String,
        baseline: u64,
        candidate: u64,
    },
    BudgetGrew {
        case: String,
        point: String,
        growth: BudgetGrowth,
    },
    OrderingWordsRemoved {
        case: String,
        rule: &'static str,
        baseline: u64,
        candidate: u64,
    },
    NoCaseFallsEverywhere {
        tier: CostTier,
    },
}

impl SweepVeto {
    pub fn label(&self) -> String {
        match self {
            SweepVeto::CaseRose {
                case,
                point,
                baseline,
                candidate,
            } => format!("case_rose:{case}:{baseline}->{candidate}@[{point}]"),
            SweepVeto::BudgetGrew {
                case,
                point,
                growth,
            } => {
                format!("{case}:{}@[{point}]", growth.label())
            }
            SweepVeto::OrderingWordsRemoved {
                case,
                rule,
                baseline,
                candidate,
            } => format!("ordering_words_removed:{case}:{rule}:{baseline}->{candidate}"),
            SweepVeto::NoCaseFallsEverywhere { tier } => {
                format!("no_case_falls_everywhere:tier={tier}")
            }
        }
    }

    pub fn tier(&self) -> Option<CostTier> {
        match self {
            SweepVeto::NoCaseFallsEverywhere { tier } => Some(*tier),
            _ => None,
        }
    }

    pub fn case(&self) -> Option<&str> {
        match self {
            SweepVeto::CaseRose { case, .. }
            | SweepVeto::BudgetGrew { case, .. }
            | SweepVeto::OrderingWordsRemoved { case, .. } => Some(case.as_str()),
            SweepVeto::NoCaseFallsEverywhere { .. } => None,
        }
    }
}

#[derive(Debug, Clone)]
pub struct SweepCompare {
    pub table_digest: String,
    pub cases: Vec<CaseSweep>,
    pub reasons: Vec<SweepVeto>,
}

impl SweepCompare {
    pub fn wins(&self) -> bool {
        self.reasons.is_empty()
    }

    pub fn scored_points(&self) -> usize {
        self.cases.iter().map(CaseSweep::corners).sum()
    }

    pub fn tiers(&self) -> Vec<CostTier> {
        CostTier::ALL
            .into_iter()
            .filter(|t| self.cases.iter().any(|c| c.tier == *t))
            .collect()
    }

    pub fn cases_in(&self, tier: CostTier) -> Vec<&CaseSweep> {
        self.cases.iter().filter(|c| c.tier == tier).collect()
    }

    pub fn scored_points_in(&self, tier: CostTier) -> usize {
        self.cases_in(tier).iter().map(|c| c.corners()).sum()
    }

    pub fn reasons_for_tier(&self, tier: CostTier) -> Vec<&SweepVeto> {
        self.reasons
            .iter()
            .filter(|r| match (r.tier(), r.case()) {
                (Some(t), _) => t == tier,
                (None, Some(case)) => self.cases.iter().any(|c| c.name == case && c.tier == tier),
                (None, None) => false,
            })
            .collect()
    }

    pub fn wins_in_tier(&self, tier: CostTier) -> bool {
        self.reasons_for_tier(tier).is_empty()
    }

    pub fn tier_verdicts(&self) -> String {
        self.tiers()
            .into_iter()
            .map(|t| {
                format!(
                    "{t}={} ({} case(s), {} points/side)",
                    if self.wins_in_tier(t) { "wins" } else { "veto" },
                    self.cases_in(t).len(),
                    self.scored_points_in(t)
                )
            })
            .collect::<Vec<_>>()
            .join(" ")
    }
}

struct CompiledSide {
    program: CodegenProgram,
    placement: PlacementTable,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct SideScore {
    cycles: u64,
    words: u64,
    budgets: Vec<CoreBudget>,
    ordering: Option<OrderingCounts>,
}

impl SideScore {
    fn charge(&self) -> u64 {
        total_charge(&self.budgets)
    }
}

#[cfg(test)]
fn compile_side(path: &Path, opts: &[OptId]) -> Result<CompiledSide, String> {
    compile_side_from(&load_shipped_front(path)?, opts)
}

fn compile_side_from(front: &ShippedFront, opts: &[OptId]) -> Result<CompiledSide, String> {
    apply_opts(opts);
    let (program, placement, _scope) = codegen_shipped_from(front)?;
    Ok(CompiledSide { program, placement })
}

fn score_side_at(
    side: &CompiledSide,
    table: &CostTable,
    ctx: &ScoreCtx,
    point: &SweepPoint,
    want_ordering: bool,
) -> Result<SideScore, String> {
    let r = score_totals_at(
        &side.program,
        table,
        &side.placement,
        point,
        ctx,
        want_ordering,
    )?;
    Ok(SideScore {
        cycles: r.total_proxy_cycles,
        words: r.total_words,
        budgets: r.footprint,
        ordering: r.ordering,
    })
}

pub fn box_cardinality(table: &CostTable) -> u64 {
    1u64 << table.sweep_dimensions().len().min(63)
}

#[derive(Debug)]
struct Probe {
    swept: Vec<String>,
    held: Vec<HeldDim>,
    read_but_static: Vec<String>,
}

fn probe_case(
    name: &str,
    base: &CompiledSide,
    cand: &CompiledSide,
    table: &CostTable,
) -> Result<Probe, String> {
    probe_case_bounded(name, base, cand, table, MAX_SWEPT_DIMS)
}

fn probe_case_bounded(
    name: &str,
    base: &CompiledSide,
    cand: &CompiledSide,
    table: &CostTable,
    max_swept_dims: usize,
) -> Result<Probe, String> {
    let dims: Vec<String> = table
        .sweep_dimensions()
        .into_iter()
        .map(str::to_string)
        .collect();
    let pinned = SweepPoint::pinned(table);
    let mut all_lo = pinned.clone();
    let mut all_hi = pinned.clone();
    for d in &dims {
        let row = table
            .sweep(d)
            .ok_or_else(|| format!("sweep dimension `{d}` vanished"))?;
        all_lo = all_lo.with(d, row.lo);
        all_hi = all_hi.with(d, row.hi);
    }
    let bases = [pinned, all_lo, all_hi];

    let mut read: BTreeSet<String> = BTreeSet::new();
    let mut moved: BTreeSet<String> = BTreeSet::new();
    let mut err: Option<String> = None;
    let ctx = ScoreCtx::new(table)?;
    let mut score = |side: &CompiledSide, p: &SweepPoint| -> Option<SideScore> {
        let (out, r) = record_reads(|| score_side_at(side, table, &ctx, p, true));
        read.extend(r);
        match out {
            Ok(s) => Some(s),
            Err(e) => {
                err.get_or_insert(e);
                None
            }
        }
    };

    for d in &dims {
        let row = table
            .sweep(d)
            .ok_or_else(|| format!("sweep dimension `{d}` vanished"))?;
        for b in &bases {
            let lo = b.with(d, row.lo);
            let hi = b.with(d, row.hi);
            for side in [base, cand] {
                let a = score(side, &lo);
                let z = score(side, &hi);
                if let (Some(a), Some(z)) = (a, z)
                    && a != z
                {
                    moved.insert(d.clone());
                }
            }
        }
    }
    if let Some(e) = err {
        return Err(format!("{name}: probe score failed: {e}"));
    }

    let mut swept = Vec::new();
    let mut held = Vec::new();
    let mut read_but_static = Vec::new();
    for d in &dims {
        let row = table
            .sweep(d)
            .ok_or_else(|| format!("sweep dimension `{d}` vanished"))?;
        let was_read = read.contains(d);
        let was_moved = moved.contains(d);
        if was_moved && !was_read {
            return Err(format!(
                "{name}: dimension `{d}` moved a total without ever being \
                 read through SweepPoint::get — the model reads the box \
                 through some other door and the probe cannot be trusted"
            ));
        }
        if was_read {
            if !was_moved {
                read_but_static.push(d.clone());
            }
            swept.push(d.clone());
        } else {
            held.push(HeldDim {
                dim: d.clone(),
                lo: row.lo,
                hi: row.hi,
                pinned: row.pinned,
            });
        }
    }
    if swept.len() > max_swept_dims {
        return Err(format!(
            "{name}: {} dimensions survive the sensitivity probe, over the \
             bound of {max_swept_dims} (2^{} corners). Decision 1604 forbids \
             dropping a dimension, so this errors rather than truncating the \
             sweep: raise MAX_SWEPT_DIMS deliberately, with the cost of the \
             sweep measured, or narrow what the model reads.",
            swept.len(),
            swept.len()
        ));
    }
    Ok(Probe {
        swept,
        held,
        read_but_static,
    })
}

fn refuse_at_point(
    case: &str,
    label: &str,
    b: &SideScore,
    c: &SideScore,
    check_ordering: bool,
    reasons: &mut Vec<SweepVeto>,
) -> Result<PointRow, String> {
    if c.cycles > b.cycles {
        reasons.push(SweepVeto::CaseRose {
            case: case.to_string(),
            point: label.to_string(),
            baseline: b.cycles,
            candidate: c.cycles,
        });
    }
    for g in budget_overflow_growth(&b.budgets, &c.budgets)? {
        reasons.push(SweepVeto::BudgetGrew {
            case: case.to_string(),
            point: label.to_string(),
            growth: g,
        });
    }
    if check_ordering {
        let (bo, co) = match (&b.ordering, &c.ordering) {
            (Some(bo), Some(co)) => (bo, co),
            _ => {
                return Err(format!(
                    "{case} at {label}: the ordering refusal was asked for at a point \
                     scored without the ordering census — freeze 1633's veto may not \
                     be answered from an absent count"
                ));
            }
        };
        for r in ordering_removals(bo, co) {
            reasons.push(SweepVeto::OrderingWordsRemoved {
                case: case.to_string(),
                rule: r.rule,
                baseline: r.baseline,
                candidate: r.candidate,
            });
        }
    }
    Ok(PointRow {
        point: label.to_string(),
        baseline: b.cycles,
        candidate: c.cycles,
        baseline_charge: b.charge(),
        candidate_charge: c.charge(),
    })
}

fn score_corners_in_parallel(
    corners: &[SweepPoint],
    base: &CompiledSide,
    cand: &CompiledSide,
    table: &CostTable,
    ctx: &ScoreCtx,
) -> Result<Vec<(SideScore, SideScore)>, String> {
    let n = corners.len();
    let workers = std::thread::available_parallelism()
        .map(|p| p.get())
        .unwrap_or(1)
        .min(n);
    if workers < 2 || n < 64 {
        return corners
            .iter()
            .enumerate()
            .map(|(i, p)| {
                Ok((
                    score_side_at(base, table, ctx, p, i == 0)?,
                    score_side_at(cand, table, ctx, p, i == 0)?,
                ))
            })
            .collect();
    }

    let chunk = n.div_ceil(workers);
    let mut out: Vec<Result<(SideScore, SideScore), String>> = Vec::with_capacity(n);
    out.resize_with(n, || Err(String::new()));

    std::thread::scope(|scope| {
        for (ci, (corner_chunk, out_chunk)) in
            corners.chunks(chunk).zip(out.chunks_mut(chunk)).enumerate()
        {
            let first = ci * chunk;
            scope.spawn(move || {
                for (j, (p, slot)) in corner_chunk.iter().zip(out_chunk.iter_mut()).enumerate() {
                    let want_ordering = first + j == 0;
                    *slot = score_side_at(base, table, ctx, p, want_ordering).and_then(|b| {
                        score_side_at(cand, table, ctx, p, want_ordering).map(|c| (b, c))
                    });
                }
            });
        }
    });

    out.into_iter().collect()
}

fn sweep_case(
    name: &str,
    tier: CostTier,
    base: &CompiledSide,
    cand: &CompiledSide,
    table: &CostTable,
    reasons: &mut Vec<SweepVeto>,
) -> Result<CaseSweep, String> {
    let probe = probe_case(name, base, cand, table)?;
    let swept_refs: Vec<&str> = probe.swept.iter().map(String::as_str).collect();
    let corners = endpoint_corners(table, &swept_refs);

    let ctx = ScoreCtx::new(table)?;
    let scored = score_corners_in_parallel(&corners, base, cand, table, &ctx)?;

    let mut points = Vec::with_capacity(corners.len());
    let mut ordering_reported = false;
    for (p, (b, c)) in corners.iter().zip(scored) {
        let label = p.label_over(&swept_refs);
        points.push(refuse_at_point(
            name,
            &label,
            &b,
            &c,
            !ordering_reported,
            reasons,
        )?);
        ordering_reported = true;
    }

    let box_dims = table.sweep_dimensions().len();
    Ok(CaseSweep {
        name: name.to_string(),
        tier,
        box_dims,
        box_cardinality: box_cardinality(table),
        swept: probe.swept,
        held: probe.held,
        read_but_static: probe.read_but_static,
        points,
    })
}

pub fn compare_opt_lists_over_box(
    baseline: &[OptId],
    candidate: &[OptId],
) -> Result<SweepCompare, String> {
    sweep_corpus(baseline, candidate, CorpusSel::All)
}

pub fn compare_opt_lists_over_box_in_tier(
    baseline: &[OptId],
    candidate: &[OptId],
    tier: CostTier,
) -> Result<SweepCompare, String> {
    sweep_corpus(baseline, candidate, CorpusSel::Tier(tier))
}

pub fn compare_opt_lists_over_box_for_case(
    baseline: &[OptId],
    candidate: &[OptId],
    case: &str,
) -> Result<SweepCompare, String> {
    sweep_corpus(baseline, candidate, CorpusSel::Case(case))
}

#[derive(Debug, Clone, Copy)]
enum CorpusSel<'a> {
    All,
    Tier(CostTier),
    Case(&'a str),
}

fn sweep_corpus(
    baseline: &[OptId],
    candidate: &[OptId],
    sel: CorpusSel<'_>,
) -> Result<SweepCompare, String> {
    let mut corpus = try_discover_cost_cases()?;
    match sel {
        CorpusSel::All => {}
        CorpusSel::Tier(t) => {
            corpus.retain(|c| c.tier == t);
            if corpus.is_empty() {
                return Err(format!(
                    "sweep: the `{t}` tier of the cost corpus is empty — a tier \
                     nothing populates is a lane nothing runs, and this refuses \
                     rather than reporting a vacuous ∀"
                ));
            }
        }
        CorpusSel::Case(want) => {
            corpus.retain(|c| c.name == want);
            if corpus.is_empty() {
                return Err(format!(
                    "sweep: no cost corpus case named `{want}` (smoke lane names a case that must exist)"
                ));
            }
        }
    }
    if corpus.is_empty() {
        return Err("cost corpus empty: expected tests/golden/cost-*/input.wr".to_string());
    }
    let table = load_default()?;
    let mut cases = Vec::with_capacity(corpus.len());
    let mut reasons = Vec::new();
    for case in &corpus {
        let path = case.input.as_path();
        let front = load_shipped_front(path)?;
        let b = compile_side_from(&front, baseline)?;
        let c = compile_side_from(&front, candidate)?;
        cases.push(sweep_case(
            &case.name,
            case.tier,
            &b,
            &c,
            &table,
            &mut reasons,
        )?);
    }
    apply_mode(CompileMode::Release);

    for tier in CostTier::ALL {
        let in_tier: Vec<&CaseSweep> = cases.iter().filter(|c| c.tier == tier).collect();
        if in_tier.is_empty() {
            continue;
        }
        let any_falls_everywhere = in_tier
            .iter()
            .any(|c| !c.points.is_empty() && c.points.iter().all(|p| p.candidate < p.baseline));
        if !any_falls_everywhere {
            reasons.push(SweepVeto::NoCaseFallsEverywhere { tier });
        }
    }

    Ok(SweepCompare {
        table_digest: table.table_digest(),
        cases,
        reasons,
    })
}

pub fn format_sweep_table(cmp: &SweepCompare, base_label: &str, cand_label: &str) -> String {
    let mut out = String::new();
    out.push_str(&format!("table_digest={}\n", cmp.table_digest));
    for c in &cmp.cases {
        out.push_str(&format!(
            "\ncase {} tier={} box_dims={} box_cardinality={} swept_k={} corners={}\n",
            c.name,
            c.tier,
            c.box_dims,
            c.box_cardinality,
            c.swept.len(),
            c.corners()
        ));
        out.push_str(&format!("  swept: {}\n", c.swept.join(" ")));
        out.push_str(&format!(
            "  held (never read by either side, so no corner over them can flip this case): {}\n",
            if c.held.is_empty() {
                "-".to_string()
            } else {
                c.held
                    .iter()
                    .map(HeldDim::label)
                    .collect::<Vec<_>>()
                    .join(" ")
            }
        ));
        out.push_str(&format!(
            "  read but static (kept in the sweep anyway): {}\n",
            if c.read_but_static.is_empty() {
                "-".to_string()
            } else {
                c.read_but_static.join(" ")
            }
        ));
        out.push_str(&format!(
            "  {:<44} {:>12} {:>12} {:>10} {:>8} {:>8}\n",
            "point", base_label, cand_label, "Δ", "chg_b", "chg_c"
        ));
        for p in &c.points {
            out.push_str(&format!(
                "  {:<44} {:>12} {:>12} {:>+10} {:>8} {:>8}\n",
                p.point,
                p.baseline,
                p.candidate,
                p.delta(),
                p.baseline_charge,
                p.candidate_charge
            ));
        }
    }
    for t in cmp.tiers() {
        let rows = cmp.cases_in(t);
        let refusals = cmp.reasons_for_tier(t);
        out.push_str(&format!(
            "\ntier {t} cases={} points_per_side={} outcome={}\n",
            rows.len(),
            cmp.scored_points_in(t),
            if refusals.is_empty() {
                "wins_at_every_point".to_string()
            } else {
                format!(
                    "veto reasons={}",
                    refusals
                        .iter()
                        .map(|r| r.label())
                        .collect::<Vec<_>>()
                        .join(" ")
                )
            }
        ));
    }
    if cmp.reasons.is_empty() {
        out.push_str(&format!(
            "\noutcome=wins_at_every_point points_per_side={} tiers[{}]\n",
            cmp.scored_points(),
            cmp.tier_verdicts()
        ));
    } else {
        let labels: Vec<String> = cmp.reasons.iter().map(SweepVeto::label).collect();
        out.push_str(&format!(
            "\noutcome=veto tiers[{}] reasons={}\n",
            cmp.tier_verdicts(),
            labels.join("\n                ")
        ));
    }
    out
}

pub fn assert_sweep_wins(cmp: &SweepCompare, cand_label: &str, base_label: &str) {
    if !cmp.reasons.is_empty() {
        let table = format_sweep_table(cmp, base_label, cand_label);
        let labels: Vec<String> = cmp.reasons.iter().map(SweepVeto::label).collect();
        panic!(
            "{cand_label} refused vs {base_label} at {} point(s)/reason(s):\n{}\n{table}",
            cmp.reasons.len(),
            labels.join("\n"),
        );
    }
}

#[derive(Debug, Clone)]
pub struct AttributionCell {
    pub config: String,
    pub proxy_cycles: u64,
    pub words: u64,
    pub charge: u64,
    pub fetched_text_bytes: u64,
}

#[derive(Debug, Clone)]
pub struct AttributionRow {
    pub name: String,
    pub tier: CostTier,
    pub cells: Vec<AttributionCell>,
}

impl AttributionRow {
    pub fn cell(&self, config: &str) -> Option<&AttributionCell> {
        self.cells.iter().find(|c| c.config == config)
    }
}

pub fn attribute_opts(configs: &[(&str, &[OptId])]) -> Vec<AttributionRow> {
    let corpus = discover_cost_cases();
    assert!(
        !corpus.is_empty(),
        "cost corpus empty: expected tests/golden/cost-*/input.wr"
    );
    let mut rows = Vec::with_capacity(corpus.len());
    for case in &corpus {
        let path = case.input.as_path();
        let mut cells = Vec::with_capacity(configs.len());
        for (label, opts) in configs {
            let r = report_path_under_opts(path, opts);
            cells.push(AttributionCell {
                config: (*label).to_string(),
                proxy_cycles: r.total_proxy_cycles,
                words: r.total_words,
                charge: total_charge(&r.footprint),
                fetched_text_bytes: r.footprint.iter().map(|b| b.fetched_text_bytes).sum(),
            });
        }
        rows.push(AttributionRow {
            name: case.name.clone(),
            tier: case.tier,
            cells,
        });
    }
    apply_mode(CompileMode::Release);
    rows
}

pub fn format_attribution_table(rows: &[AttributionRow]) -> String {
    let mut out = String::new();
    out.push_str("f=1 (flat); not a measured total\n");
    out.push_str(&format!(
        "{:<24} {:<8} {:<18} {:>10} {:>10} {:>8} {:>10}\n",
        "case", "tier", "config", "cycles", "words", "charge", "hot_text"
    ));
    for r in rows {
        for c in &r.cells {
            out.push_str(&format!(
                "{:<24} {:<8} {:<18} {:>10} {:>10} {:>8} {:>10}\n",
                r.name,
                r.tier.as_str(),
                c.config,
                c.proxy_cycles,
                c.words,
                c.charge,
                c.fetched_text_bytes
            ));
        }
    }
    let configs: Vec<&str> = rows
        .first()
        .map(|r| r.cells.iter().map(|c| c.config.as_str()).collect())
        .unwrap_or_default();
    for tier in CostTier::ALL {
        if !rows.iter().any(|r| r.tier == tier) {
            continue;
        }
        for label in &configs {
            let mut cycles = 0u64;
            let mut words = 0u64;
            let mut charge = 0u64;
            let mut hot = 0u64;
            for r in rows.iter().filter(|r| r.tier == tier) {
                if let Some(c) = r.cell(label) {
                    cycles = cycles.saturating_add(c.proxy_cycles);
                    words = words.saturating_add(c.words);
                    charge = charge.saturating_add(c.charge);
                    hot = hot.saturating_add(c.fetched_text_bytes);
                }
            }
            out.push_str(&format!(
                "{:<24} {:<8} {:<18} {:>10} {:>10} {:>8} {:>10}\n",
                format!("SUB[{}]", tier.as_str()),
                tier.as_str(),
                label,
                cycles,
                words,
                charge,
                hot
            ));
        }
    }
    for label in configs {
        let mut cycles = 0u64;
        let mut words = 0u64;
        let mut charge = 0u64;
        let mut hot = 0u64;
        for r in rows {
            if let Some(c) = r.cell(label) {
                cycles = cycles.saturating_add(c.proxy_cycles);
                words = words.saturating_add(c.words);
                charge = charge.saturating_add(c.charge);
                hot = hot.saturating_add(c.fetched_text_bytes);
            }
        }
        out.push_str(&format!(
            "{:<24} {:<8} {:<18} {:>10} {:>10} {:>8} {:>10}\n",
            "SUM", "both", label, cycles, words, charge, hot
        ));
    }
    out
}

#[derive(Debug, Clone)]
pub struct WorkloadDelta {
    pub name: String,
    pub weight: u64,
    pub baseline: u64,
    pub candidate: u64,
}

impl WorkloadDelta {
    pub fn delta(&self) -> i64 {
        self.candidate as i64 - self.baseline as i64
    }

    pub fn relative_delta(&self) -> f64 {
        if self.baseline == 0 {
            if self.candidate == 0 {
                0.0
            } else {
                f64::INFINITY
            }
        } else {
            self.delta() as f64 / self.baseline as f64
        }
    }

    pub fn is_flat(&self) -> bool {
        self.name == FLAT_NAME
    }

    pub fn rises(&self) -> bool {
        self.candidate > self.baseline
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VetoReason {
    WorkloadRose {
        name: String,
    },
    CoverageFell {
        name: String,
        baseline: (u64, u64),
        candidate: (u64, u64),
    },
    BudgetOverflowGrew {
        core: usize,
        field: &'static str,
        baseline: u64,
        candidate: u64,
    },
    OrderingWordsRemoved {
        rule: &'static str,
        baseline: u64,
        candidate: u64,
    },
}

impl VetoReason {
    pub fn label(&self) -> String {
        match self {
            VetoReason::OrderingWordsRemoved {
                rule,
                baseline,
                candidate,
            } => format!("ordering_words_removed:{rule}:{baseline}->{candidate}"),
            VetoReason::WorkloadRose { name } => format!("workload_rose:{name}"),
            VetoReason::CoverageFell {
                name,
                baseline,
                candidate,
            } => format!(
                "coverage_fell:{name}:{}/{}->{}/{}",
                baseline.0, baseline.1, candidate.0, candidate.1
            ),
            VetoReason::BudgetOverflowGrew {
                core,
                field,
                baseline,
                candidate,
            } => format!("budget_grew:core{core}:{field}:{baseline}->{candidate}"),
        }
    }
}

#[derive(Debug, Clone)]
pub enum OverallOutcome {
    Veto { reasons: Vec<VetoReason> },
    Rank { weighted_mean_rel: f64 },
}

#[derive(Debug, Clone)]
pub struct OverallCompare {
    pub workloads_digest: String,
    pub workloads: Vec<WorkloadDelta>,
    pub baseline_coverage: BTreeMap<String, (u64, u64)>,
    pub candidate_coverage: BTreeMap<String, (u64, u64)>,
    pub baseline_words: u64,
    pub candidate_words: u64,
    pub baseline_budgets: Vec<CoreBudget>,
    pub candidate_budgets: Vec<CoreBudget>,
    pub outcome: OverallOutcome,
}

impl OverallCompare {
    pub fn vetoed(&self) -> bool {
        matches!(self.outcome, OverallOutcome::Veto { .. })
    }

    pub fn veto_reasons(&self) -> &[VetoReason] {
        match &self.outcome {
            OverallOutcome::Veto { reasons } => reasons,
            OverallOutcome::Rank { .. } => &[],
        }
    }

    pub fn risen(&self) -> Vec<String> {
        self.veto_reasons()
            .iter()
            .filter_map(|r| match r {
                VetoReason::WorkloadRose { name } => Some(name.clone()),
                _ => None,
            })
            .collect()
    }

    pub fn weighted_mean_rel(&self) -> Option<f64> {
        match self.outcome {
            OverallOutcome::Rank { weighted_mean_rel } => Some(weighted_mean_rel),
            OverallOutcome::Veto { .. } => None,
        }
    }

    pub fn wins(&self) -> bool {
        match self.outcome {
            OverallOutcome::Veto { .. } => false,
            OverallOutcome::Rank { weighted_mean_rel } => weighted_mean_rel < 0.0,
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct OverallSide {
    pub totals: BTreeMap<String, u64>,
    pub coverage: BTreeMap<String, (u64, u64)>,
    pub words: u64,
    pub budgets: Vec<CoreBudget>,
    pub ordering: OrderingCounts,
}

impl OverallSide {
    pub fn from_report(report: &CostReport) -> Self {
        Self {
            totals: report.workload_totals.clone(),
            coverage: report.workload_coverage.clone(),
            words: report.total_words,
            budgets: report.footprint.clone(),
            ordering: ordering_word_counts(report),
        }
    }

    pub fn from_totals(totals: BTreeMap<String, u64>) -> Self {
        Self {
            totals,
            coverage: BTreeMap::new(),
            words: 0,
            budgets: Vec::new(),
            ordering: BTreeMap::new(),
        }
    }

    pub fn with_budgets(mut self, budgets: Vec<CoreBudget>) -> Self {
        self.budgets = budgets;
        self
    }

    pub fn with_ordering(mut self, ordering: OrderingCounts) -> Self {
        self.ordering = ordering;
        self
    }

    pub fn with_words(mut self, words: u64) -> Self {
        self.words = words;
        self
    }

    pub fn with_coverage(mut self, coverage: BTreeMap<String, (u64, u64)>) -> Self {
        self.coverage = coverage;
        self
    }
}

pub fn flat_only_totals(flat_proxy_cycles: u64) -> BTreeMap<String, u64> {
    let mut m = BTreeMap::new();
    m.insert(FLAT_NAME.to_string(), flat_proxy_cycles);
    m
}

pub fn stub_all_workload_totals(proxy_cycles: u64, set: &WorkloadSet) -> BTreeMap<String, u64> {
    set.names().map(|n| (n.to_string(), proxy_cycles)).collect()
}

pub fn load_pinned_workloads() -> Result<WorkloadSet, String> {
    workload::load_default()
}

pub fn compare_overall(
    baseline: &OverallSide,
    candidate: &OverallSide,
    weights: &WorkloadSet,
) -> Result<OverallCompare, String> {
    if weights.is_empty() {
        return Err("overall: empty workload set".to_string());
    }

    let mut rows = Vec::with_capacity(weights.len());
    for name in weights.names() {
        let weight = weights
            .weight(name)
            .ok_or_else(|| format!("overall: missing weight for `{name}`"))?;
        let b = baseline
            .totals
            .get(name)
            .copied()
            .ok_or_else(|| format!("overall: baseline missing workload `{name}`"))?;
        let c = candidate
            .totals
            .get(name)
            .copied()
            .ok_or_else(|| format!("overall: candidate missing workload `{name}`"))?;
        rows.push(WorkloadDelta {
            name: name.to_string(),
            weight,
            baseline: b,
            candidate: c,
        });
    }

    let mut reasons = Vec::new();
    for r in &rows {
        if !r.is_flat() && r.rises() {
            reasons.push(VetoReason::WorkloadRose {
                name: r.name.clone(),
            });
        }
    }

    for (name, &base_cov) in &baseline.coverage {
        let cand_cov = candidate
            .coverage
            .get(name)
            .copied()
            .unwrap_or((0, base_cov.1));
        if cand_cov.1 != base_cov.1 {
            return Err(format!(
                "overall: coverage denominator for `{name}` changed \
                 {}->{} — the two sides were measured against different \
                 frequency vectors",
                base_cov.1, cand_cov.1
            ));
        }
        if cand_cov.0 < base_cov.0 {
            reasons.push(VetoReason::CoverageFell {
                name: name.clone(),
                baseline: base_cov,
                candidate: cand_cov,
            });
        }
    }

    for g in budget_overflow_growth(&baseline.budgets, &candidate.budgets)? {
        reasons.push(VetoReason::BudgetOverflowGrew {
            core: g.core,
            field: g.field,
            baseline: g.baseline,
            candidate: g.candidate,
        });
    }

    for r in ordering_removals(&baseline.ordering, &candidate.ordering) {
        reasons.push(VetoReason::OrderingWordsRemoved {
            rule: r.rule,
            baseline: r.baseline,
            candidate: r.candidate,
        });
    }

    let outcome = if !reasons.is_empty() {
        OverallOutcome::Veto { reasons }
    } else {
        let mut w_sum = 0u64;
        let mut acc = 0.0f64;
        for r in &rows {
            w_sum = w_sum.saturating_add(r.weight);
            acc += (r.weight as f64) * r.relative_delta();
        }
        if w_sum == 0 {
            return Err("overall: total weight is 0".to_string());
        }
        OverallOutcome::Rank {
            weighted_mean_rel: acc / (w_sum as f64),
        }
    };

    Ok(OverallCompare {
        workloads_digest: weights.digest(),
        workloads: rows,
        baseline_coverage: baseline.coverage.clone(),
        candidate_coverage: candidate.coverage.clone(),
        baseline_words: baseline.words,
        candidate_words: candidate.words,
        baseline_budgets: baseline.budgets.clone(),
        candidate_budgets: candidate.budgets.clone(),
        outcome,
    })
}

#[derive(Debug)]
pub struct OptGateCompare {
    pub flat_corpus: CorpusCompare,
    pub overall: OverallCompare,
}

impl OptGateCompare {
    pub fn wins(&self) -> bool {
        self.flat_corpus.non_regressing() && self.overall.wins()
    }
}

/// Compile both optimization sets through the real linked-image workload path.
///
/// The flat row is the complete cost corpus. Each named row is scored from its
/// repository-mapped source with exact linked origin coverage. The current
/// workload file has one named source; fail closed rather than silently merging
/// incomparable per-core budget vectors if more are added.
pub fn compare_opt_lists_overall(
    baseline_opts: &[OptId],
    candidate_opts: &[OptId],
) -> Result<OptGateCompare, String> {
    let workloads = load_pinned_workloads()?;
    let flat_corpus = compare_opt_lists(baseline_opts, candidate_opts);
    let mut baseline = OverallSide::default();
    let mut candidate = OverallSide::default();
    baseline
        .totals
        .insert(FLAT_NAME.to_string(), flat_corpus.baseline_sum);
    candidate
        .totals
        .insert(FLAT_NAME.to_string(), flat_corpus.candidate_sum);
    baseline.words = flat_corpus.baseline_words;
    candidate.words = flat_corpus.candidate_words;

    let named: Vec<String> = workloads
        .names()
        .filter(|name| *name != FLAT_NAME)
        .map(str::to_string)
        .collect();
    if named.len() != 1 {
        return Err(format!(
            "overall opt gate requires exactly one named workload until budgets are keyed by workload, got {}",
            named.len()
        ));
    }
    let name = &named[0];
    let source = workloads
        .source_path(name)
        .ok_or_else(|| format!("overall opt gate: workload `{name}` has no source"))?;
    apply_opts(baseline_opts);
    let baseline_report = report_cost_stage_path(&source)?;
    apply_opts(candidate_opts);
    let candidate_report = report_cost_stage_path(&source)?;
    for (side, report) in [
        (&mut baseline, &baseline_report),
        (&mut candidate, &candidate_report),
    ] {
        side.totals.insert(
            name.clone(),
            *report
                .workload_totals
                .get(name)
                .ok_or_else(|| format!("overall opt gate: `{name}` was not scored"))?,
        );
        side.coverage.insert(
            name.clone(),
            *report
                .workload_coverage
                .get(name)
                .ok_or_else(|| format!("overall opt gate: `{name}` has no coverage row"))?,
        );
        side.budgets = report.footprint.clone();
        side.ordering = ordering_word_counts(report);
    }
    apply_mode(CompileMode::Release);
    let overall = compare_overall(&baseline, &candidate, &workloads)?;
    Ok(OptGateCompare {
        flat_corpus,
        overall,
    })
}

fn coverage_cell(cov: Option<(u64, u64)>) -> String {
    match cov {
        Some((m, t)) if t > 0 => {
            format!("{m}/{t} ({:.1}%)", 100.0 * (m as f64) / (t as f64))
        }
        Some((m, t)) => format!("{m}/{t}"),
        None => "-".to_string(),
    }
}

pub fn format_overall_table(cmp: &OverallCompare, base_label: &str, cand_label: &str) -> String {
    let mut out = String::new();
    out.push_str(&format!("workloads_digest={}\n", cmp.workloads_digest));
    out.push_str(&format!(
        "{:<16} {:>8} {:>12} {:>12} {:>10} {:>12}  {:<22} {:<22}\n",
        "workload", "weight", base_label, cand_label, "Δ", "rel", "coverage_b", "coverage_c"
    ));
    for r in &cmp.workloads {
        let rel = r.relative_delta();
        let rel_s = if rel.is_infinite() {
            "inf".to_string()
        } else {
            format!("{rel:+.6}")
        };
        let b_cov = cmp.baseline_coverage.get(&r.name).copied();
        let c_cov = cmp
            .candidate_coverage
            .get(&r.name)
            .copied()
            .or(b_cov.map(|(_, t)| (0, t)));
        out.push_str(&format!(
            "{:<16} {:>8} {:>12} {:>12} {:>+10} {:>12}  {:<22} {:<22}\n",
            r.name,
            r.weight,
            r.baseline,
            r.candidate,
            r.delta(),
            rel_s,
            coverage_cell(b_cov),
            coverage_cell(c_cov),
        ));
    }
    for (name, &(b_m, b_t)) in &cmp.baseline_coverage {
        let (c_m, c_t) = cmp
            .candidate_coverage
            .get(name)
            .copied()
            .unwrap_or((0, b_t));
        out.push_str(&format!("coverage {name} {b_m}/{b_t} -> {c_m}/{c_t}\n"));
    }
    out.push_str(&format!(
        "{:<16} {:>8} {:>12} {:>12} {:>+10} {:>12}\n",
        "words(reported)",
        "-",
        cmp.baseline_words,
        cmp.candidate_words,
        cmp.candidate_words as i64 - cmp.baseline_words as i64,
        "-"
    ));
    for (i, b) in cmp.baseline_budgets.iter().enumerate() {
        let c = cmp.candidate_budgets.get(i);
        out.push_str(&format!("budget_b {}\n", b.render()));
        if let Some(c) = c {
            out.push_str(&format!("budget_c {}\n", c.render()));
        }
    }
    match &cmp.outcome {
        OverallOutcome::Veto { reasons } => {
            let labels: Vec<String> = reasons.iter().map(|r| r.label()).collect();
            out.push_str(&format!("outcome=veto reasons={}\n", labels.join(",")));
        }
        OverallOutcome::Rank { weighted_mean_rel } => {
            out.push_str(&format!(
                "outcome=rank weighted_mean_rel={weighted_mean_rel:+.6} wins={}\n",
                cmp.wins()
            ));
        }
    }
    out
}

pub fn assert_overall_wins(cmp: &OverallCompare, cand_label: &str, base_label: &str) {
    let table = format_overall_table(cmp, base_label, cand_label);
    if let OverallOutcome::Veto { reasons } = &cmp.outcome {
        let labels: Vec<String> = reasons.iter().map(|r| r.label()).collect();
        panic!(
            "{cand_label} vetoed vs {base_label}: {}\n{table}",
            labels.join(", "),
        );
    }
    let mean = cmp.weighted_mean_rel().expect("rank outcome");
    assert!(
        mean < 0.0,
        "{cand_label} overall weighted_mean_rel={mean:+.6} must be < 0 vs {base_label}\n{table}"
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::opts::OptId;

    #[test]
    fn discover_cost_corpus_is_sorted_cost_star() {
        let cases = discover_cost_cases();
        assert!(
            cases.len() >= 4,
            "expected ≥4 cost-* goldens, got {}",
            cases.len()
        );
        let names: Vec<String> = cases.iter().map(|c| c.name.clone()).collect();
        let mut sorted = names.clone();
        sorted.sort();
        assert_eq!(names, sorted, "corpus cases must be sorted by case name");
        for n in &names {
            assert!(n.starts_with("cost-"), "unexpected case {n}");
        }
        assert!(names.iter().any(|n| n == "cost-bounds-elide"));
        assert!(names.iter().any(|n| n == "cost-calls"));
        assert_eq!(
            discover_cost_corpus(),
            cases.iter().map(|c| c.input.clone()).collect::<Vec<_>>()
        );
    }

    #[test]
    fn every_cost_case_belongs_to_exactly_one_tier_and_both_tiers_are_populated() {
        let cases = discover_cost_cases();
        let micro = discover_cost_cases_in(CostTier::Micro);
        let product = discover_cost_cases_in(CostTier::Product);
        assert_eq!(
            micro.len() + product.len(),
            cases.len(),
            "a case fell outside both tiers — the classifier must be total"
        );
        assert!(
            !micro.is_empty(),
            "the micro tier is empty: the smoke lane has nothing to sweep"
        );
        assert!(
            !product.is_empty(),
            "the product tier is empty — decision 1716 exists because the gate \
             ranked over microbenchmarks alone, and an unpopulated product tier \
             is that state with a name on it"
        );
        for c in &product {
            assert!(
                !c.input.starts_with(golden_root().join(&c.name)),
                "{}: a product case must borrow a program from outside itself, \
                 got {}",
                c.name,
                c.input.display()
            );
            assert!(c.input.is_file(), "{}: borrowed program missing", c.name);
        }
        for c in &micro {
            assert!(
                c.input.starts_with(golden_root().join(&c.name)),
                "{}: a micro case's program must live inside it, got {}",
                c.name,
                c.input.display()
            );
        }
        eprintln!(
            "cost corpus tiers: micro={} product={} ({})",
            micro.len(),
            product.len(),
            product
                .iter()
                .map(|c| c.name.as_str())
                .collect::<Vec<_>>()
                .join(" ")
        );
    }

    #[test]
    fn an_unclassifiable_cost_case_fails_closed_rather_than_being_dropped() {
        let tmp = std::env::temp_dir().join(format!(
            "wrela-cost-tier-{}-{}",
            std::process::id(),
            line!()
        ));
        let _ = std::fs::remove_dir_all(&tmp);
        let mk = |name: &str| -> PathBuf {
            let d = tmp.join(name);
            std::fs::create_dir_all(&d).expect("mkdir");
            d
        };

        let bare = mk("cost-bare");
        let e = classify_cost_case(&bare).expect_err("no program is not a tier");
        assert!(e.contains("neither `input.wr` nor `root`"), "{e}");

        let both = mk("cost-both");
        std::fs::write(both.join("input.wr"), "module x\n").unwrap();
        std::fs::write(both.join("root"), "../cost-bare/input.wr\n").unwrap();
        let e = classify_cost_case(&both).expect_err("ambiguous program");
        assert!(e.contains("both `input.wr` and `root`"), "{e}");

        let empty_root = mk("cost-empty-root");
        std::fs::write(empty_root.join("root"), "\n").unwrap();
        let e = classify_cost_case(&empty_root).expect_err("empty root");
        assert!(e.contains("`root` file is empty"), "{e}");

        let dangling = mk("cost-dangling");
        std::fs::write(dangling.join("root"), "../gone/input.wr\n").unwrap();
        let e = classify_cost_case(&dangling).expect_err("dangling root");
        assert!(e.contains("which is not a file"), "{e}");

        let hybrid = mk("cost-hybrid");
        std::fs::create_dir_all(hybrid.join("src")).unwrap();
        std::fs::write(hybrid.join("src/extra.wr"), "module x\n").unwrap();
        std::fs::write(hybrid.join("root"), "../cost-both/input.wr\n").unwrap();
        let e = classify_cost_case(&hybrid).expect_err("borrowed but self-authored");
        assert!(e.contains("owns no program of its own"), "{e}");

        let micro = mk("cost-micro");
        std::fs::write(micro.join("input.wr"), "module x\n").unwrap();
        assert_eq!(
            classify_cost_case(&micro).expect("micro").tier,
            CostTier::Micro
        );
        let product = mk("cost-prod");
        std::fs::write(product.join("root"), "../cost-micro/input.wr\n").unwrap();
        assert_eq!(
            classify_cost_case(&product).expect("product").tier,
            CostTier::Product
        );

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn every_reported_table_names_the_tier_of_every_row() {
        let cmp = compare_opt_lists_over_box_for_case(&[], RELEASE_OPTS, "cost-bounds-elide")
            .expect("smoke sweep");
        let table = format_sweep_table(&cmp, "dev", "release");
        assert!(
            table.contains("case cost-bounds-elide tier=micro"),
            "sweep table must tag each case with its tier:\n{table}"
        );
        assert!(
            table.contains("tier micro cases=1"),
            "sweep table must print a per-tier outcome:\n{table}"
        );
        assert!(
            table.contains("tiers[micro=wins"),
            "the overall line must carry both tiers' verdicts:\n{table}"
        );
        assert_eq!(cmp.tiers(), vec![CostTier::Micro]);
        assert!(cmp.wins_in_tier(CostTier::Micro));
    }

    #[test]
    fn a_win_confined_to_one_tier_is_vetoed_and_the_tier_is_named() {
        let flat_case = |name: &str, tier: CostTier, fell: bool| CaseSweep {
            name: name.to_string(),
            tier,
            box_dims: 17,
            box_cardinality: 131_072,
            swept: vec!["l2_latency".to_string()],
            held: Vec::new(),
            read_but_static: Vec::new(),
            points: vec![PointRow {
                point: "l2_latency=lo".to_string(),
                baseline: 100,
                candidate: if fell { 90 } else { 100 },
                baseline_charge: 0,
                candidate_charge: 0,
            }],
        };
        let cmp = SweepCompare {
            table_digest: "test".to_string(),
            cases: vec![
                flat_case("cost-fixture", CostTier::Micro, true),
                flat_case("cost-product-appliance", CostTier::Product, false),
            ],
            reasons: vec![SweepVeto::NoCaseFallsEverywhere {
                tier: CostTier::Product,
            }],
        };
        assert!(
            !cmp.wins(),
            "a product-tier refusal must refuse the landing"
        );
        assert!(cmp.wins_in_tier(CostTier::Micro));
        assert!(!cmp.wins_in_tier(CostTier::Product));
        assert_eq!(
            cmp.reasons[0].label(),
            "no_case_falls_everywhere:tier=product"
        );
        let table = format_sweep_table(&cmp, "dev", "cand");
        assert!(
            table.contains("tier product cases=1")
                && table.contains("no_case_falls_everywhere:tier=product"),
            "the tier's own outcome line must carry the refusal:\n{table}"
        );
    }

    #[test]
    fn sweeping_an_unpopulated_tier_is_an_error() {
        let e = compare_opt_lists_over_box_for_case(&[], RELEASE_OPTS, "cost-does-not-exist")
            .expect_err("must refuse");
        assert!(e.contains("no cost corpus case named"), "{e}");
    }

    #[ignore = "milestone lane: whole cost-corpus optimization oracle"]
    #[test]
    fn assert_release_wins_cost_corpus_oracle() {
        let cmp = assert_release_wins_cost_corpus();
        let table = format_delta_table(&cmp, "dev", "release");
        eprintln!("corpus proxy win (dev → release):\n{table}");
        for c in &cmp.cases {
            assert!(table.contains(&c.name), "table missing case {}", c.name);
        }
        assert!(table.contains("SUM"));
        assert!(cmp.sum_delta() < 0, "corpus sum must fall under release");
        let runtime = cmp
            .cases
            .iter()
            .find(|c| c.name == "cost-runtime")
            .expect("cost-runtime in corpus");
        assert!(
            runtime.baseline > 100 && runtime.candidate > 100,
            "cost-runtime must include force-rooted runtime totals, got \
             dev={} release={}",
            runtime.baseline,
            runtime.candidate
        );
        assert!(
            runtime.candidate < runtime.baseline,
            "cost-runtime must fall under release (NarrowImm on runtime \
             immediates), got {} → {}",
            runtime.baseline,
            runtime.candidate
        );
    }

    #[ignore = "milestone lane: whole product-tier BoundsElide attribution"]
    #[test]
    fn release_bounds_elide_transforms_without_regressing_the_product_tier() {
        assert!(RELEASE_OPTS.contains(&OptId::BoundsElide));
        let without: Vec<_> = RELEASE_OPTS
            .iter()
            .copied()
            .filter(|id| *id != OptId::BoundsElide)
            .collect();

        let micro = discover_cost_cases()
            .into_iter()
            .find(|c| c.name == "cost-bounds-elide")
            .expect("cost-bounds-elide must exist");
        let baseline = score_path_under_opts(&micro.input, &without);
        let release = score_path_under_opts(&micro.input, RELEASE_OPTS);
        assert!(
            release < baseline,
            "the shipped transform is inert on its own fixture: {release} vs {baseline}"
        );

        let mut strict_wins = 0usize;
        for case in discover_cost_cases_in(CostTier::Product) {
            let baseline = report_path_under_opts(&case.input, &without);
            let release = report_path_under_opts(&case.input, RELEASE_OPTS);
            assert!(
                release.total_proxy_cycles <= baseline.total_proxy_cycles
                    && release.total_words <= baseline.total_words,
                "BoundsElide regressed product case `{}`: cycles {} -> {}, words {} -> {}",
                case.name,
                baseline.total_proxy_cycles,
                release.total_proxy_cycles,
                baseline.total_words,
                release.total_words
            );
            if release.total_proxy_cycles < baseline.total_proxy_cycles
                || release.total_words < baseline.total_words
            {
                strict_wins += 1;
            }
        }

        assert!(
            strict_wins > 0,
            "the shipped proof transform must help at least one product"
        );

        apply_mode(CompileMode::Release);
    }

    #[ignore = "milestone lane: integrated product measurement table"]
    #[test]
    fn codegen_dataflow_plan_product_measurements_are_pinned() {
        let case = discover_cost_cases_in(CostTier::Product)
            .into_iter()
            .find(|case| case.name == "cost-product-actors")
            .expect("product actors");
        let base_opts = RELEASE_OPTS.to_vec();
        let base = report_path_under_opts(&case.input, &base_opts);
        let fetched = |report: &CostReport| {
            report
                .footprint
                .iter()
                .map(|budget| budget.fetched_text_bytes)
                .sum::<u64>()
        };
        assert_eq!(
            (
                base.rank_cycles,
                base.total_words,
                base.sync_frame_max_bytes,
                base.async_frame_total_bytes,
                fetched(&base),
            ),
            (10682, 14245, 1328, 944, 57024),
            "pinned product-actors baseline for the integrated plan table"
        );
        for opt in [OptId::Sroa, OptId::FrameColor] {
            let mut opts = base_opts.clone();
            opts.push(opt);
            let candidate = report_path_under_opts(&case.input, &opts);
            let measured = (
                candidate.rank_cycles,
                candidate.total_words,
                candidate.sync_frame_max_bytes,
                candidate.async_frame_total_bytes,
                fetched(&candidate),
            );
            match opt {
                OptId::Sroa => assert_eq!(measured, (10682, 14245, 1328, 944, 57024)),
                OptId::FrameColor => {
                    assert_eq!(measured, (10787, 14334, 1328, 272, 57472))
                }
                _ => unreachable!(),
            }
        }
        for opt in [
            OptId::FlowStateRegs,
            OptId::BoundsElide,
            OptId::NarrowImm,
            OptId::AdrAddressing,
        ] {
            let opts: Vec<_> = base_opts.iter().copied().filter(|id| *id != opt).collect();
            let without = report_path_under_opts(&case.input, &opts);
            let measured = (
                without.rank_cycles,
                without.total_words,
                without.sync_frame_max_bytes,
                without.async_frame_total_bytes,
                fetched(&without),
            );
            match opt {
                OptId::FlowStateRegs => {
                    assert_eq!(measured, (10844, 14396, 1328, 944, 57728))
                }
                OptId::BoundsElide => {
                    assert_eq!(measured, (10821, 14606, 1328, 944, 58560))
                }
                OptId::NarrowImm => assert_eq!(measured, (18588, 22429, 1328, 944, 89856)),
                OptId::AdrAddressing => {
                    assert_eq!(measured, (10682, 14245, 1328, 944, 57024))
                }
                _ => unreachable!(),
            }
        }
        apply_mode(CompileMode::Release);
    }

    #[ignore = "milestone lane: full corpus plus exact linked workload gate"]
    #[test]
    fn shipped_dataflow_opts_pass_the_real_linked_workload_gate() {
        for opt in [OptId::BoundsElide, OptId::FlowStateRegs] {
            let baseline: Vec<_> = RELEASE_OPTS
                .iter()
                .copied()
                .filter(|id| *id != opt)
                .collect();
            let gate = compare_opt_lists_overall(&baseline, RELEASE_OPTS).expect("overall gate");
            eprintln!(
                "{opt:?} flat corpus:\n{}\n{opt:?} overall gate:\n{}",
                format_delta_table(
                    &gate.flat_corpus,
                    &format!("release-minus-{opt:?}"),
                    "release"
                ),
                format_overall_table(&gate.overall, &format!("release-minus-{opt:?}"), "release")
            );
            assert!(gate.wins(), "{opt:?} must pass its applicable gate");
            assert!(gate.flat_corpus.non_regressing(), "{opt:?}");
            assert!(gate.overall.wins(), "{opt:?}");
            assert_eq!(
                gate.overall.baseline_coverage["boot-actors"],
                (18278, 18278)
            );
            assert_eq!(
                gate.overall.candidate_coverage["boot-actors"],
                (18278, 18278)
            );
        }
        let mut with_sroa = RELEASE_OPTS.to_vec();
        with_sroa.push(OptId::Sroa);
        let sroa = compare_opt_lists_overall(RELEASE_OPTS, &with_sroa).expect("SROA gate");
        assert!(
            !sroa.flat_corpus.non_regressing(),
            "SROA remains parked because at least one flat corpus veto survives"
        );
        assert!(!sroa.wins());
        apply_mode(CompileMode::Release);
    }

    #[test]
    fn narrow_imm_alone_wins_a_linkable_cost_case() {
        let case = discover_cost_cases()
            .into_iter()
            .find(|case| case.name == "cost-runtime")
            .expect("cost-runtime fixture");
        let dev = score_path_under_opts(&case.input, &[]);
        let alone = score_path_under_opts(&case.input, &[OptId::NarrowImm]);
        apply_mode(CompileMode::Release);
        assert!(
            alone < dev,
            "NarrowImm alone must lower cost-runtime: {alone} vs {dev}"
        );
    }

    #[ignore = "milestone lane: whole shipped-list NarrowImm attribution"]
    #[test]
    fn narrow_imm_wins_on_cycles_while_its_footprint_win_is_priced_at_zero() {
        let singles: Vec<(String, Vec<OptId>)> = RELEASE_OPTS
            .iter()
            .map(|id| (format!("{id:?}"), vec![*id]))
            .collect();
        let mut configs: Vec<(&str, &[OptId])> = vec![("dev", &[])];
        for (label, opts) in &singles {
            configs.push((label.as_str(), opts.as_slice()));
        }
        let pre_f: Vec<OptId> = RELEASE_OPTS
            .iter()
            .copied()
            .take_while(|id| *id != OptId::InterprocRegs)
            .collect();
        configs.push(("release-minus-F", pre_f.as_slice()));
        let rankable_only: Vec<OptId> = RELEASE_OPTS
            .iter()
            .copied()
            .filter(|id| {
                !matches!(
                    id,
                    OptId::WideImmForms
                        | OptId::InterprocRegs
                        | OptId::Frameless
                        | OptId::TailCalls
                )
            })
            .collect();
        configs.push(("rankable-only", rankable_only.as_slice()));
        configs.push(("release", RELEASE_OPTS));
        let rows = attribute_opts(&configs);
        let table = format_attribution_table(&rows);
        eprintln!("per-opt attribution (plans/M20.md item K):\n{table}");

        let sum = |label: &str, f: fn(&AttributionCell) -> u64| -> i64 {
            rows.iter()
                .map(|r| f(r.cell(label).expect("config scored")) as i64)
                .sum()
        };
        let dev_cycles = sum("dev", |c| c.proxy_cycles);
        let ni_cycles = sum("NarrowImm", |c| c.proxy_cycles);

        assert!(
            ni_cycles < dev_cycles,
            "NarrowImm alone must still lower the corpus proxy total: \
             dev {dev_cycles} -> NarrowImm {ni_cycles}\n{table}"
        );

        let dev_hot = sum("dev", |c| c.fetched_text_bytes);
        let ni_hot = sum("NarrowImm", |c| c.fetched_text_bytes);
        assert!(
            ni_hot < dev_hot,
            "NarrowImm must still shrink hot text: {dev_hot} -> {ni_hot}\n{table}"
        );
        const INSIDE_BUDGET: &[&str] = &[
            "cost-arith",
            "cost-bounds-elide",
            "cost-branchy",
            "cost-calls",
            "cost-mem-locality",
            "cost-runtime",
        ];
        for r in rows
            .iter()
            .filter(|r| INSIDE_BUDGET.contains(&r.name.as_str()))
        {
            for label in ["dev", "NarrowImm"] {
                let c = r.cell(label).expect("config scored");
                assert_eq!(
                    c.charge, 0,
                    "{}/{label}: this case is far inside its L1I, so the priced \
                     I-side term must be 0 and NarrowImm's footprint win is \
                     invisible to the gate here.\n{table}",
                    r.name
                );
            }
        }
        let priced: Vec<&str> = rows
            .iter()
            .filter(|r| {
                let d = r.cell("dev").expect("dev");
                let n = r.cell("NarrowImm").expect("ni");
                d.charge > 0 && n.charge < d.charge
            })
            .map(|r| r.name.as_str())
            .collect();
        assert!(
            !priced.is_empty(),
            "at least one case must price the I-side term above zero AND fall \
             under NarrowImm, or the gate is blind to every footprint-only \
             candidate again and decision 1638 needs rewriting.\n{table}"
        );

        const UNRANKABLE_ALONE: &[OptId] = &[
            OptId::WideImmForms,
            OptId::InterprocRegs,
            OptId::Frameless,
            OptId::TailCalls,
        ];
        let rankable: Vec<OptId> = RELEASE_OPTS
            .iter()
            .copied()
            .filter(|id| !UNRANKABLE_ALONE.contains(id))
            .collect();
        assert_eq!(
            rankable.len() + UNRANKABLE_ALONE.len(),
            RELEASE_OPTS.len(),
            "every RELEASE_OPTS member must be either ranked alone here or \
             named in UNRANKABLE_ALONE with a reason"
        );

        let rel_cycles = sum("release", |c| c.proxy_cycles);
        let rel_win = dev_cycles - rel_cycles;
        let wins: Vec<(String, i64)> = rankable
            .iter()
            .map(|id| {
                let label = format!("{id:?}");
                let w = dev_cycles - sum(&label, |c| c.proxy_cycles);
                (label, w)
            })
            .collect();

        assert!(
            wins.iter().all(|(_, w)| *w > 0),
            "every rankable opt must contribute on cycles: {wins:?}
{table}"
        );
        let tier_sum = |tier: CostTier, label: &str| -> i64 {
            rows.iter()
                .filter(|r| r.tier == tier)
                .map(|r| r.cell(label).expect("config scored").proxy_cycles as i64)
                .sum()
        };
        let tier_win = |tier: CostTier, label: &str| tier_sum(tier, "dev") - tier_sum(tier, label);
        let tier_singles =
            |tier: CostTier| -> i64 { wins.iter().map(|(label, _)| tier_win(tier, label)).sum() };
        let micro_win = tier_win(CostTier::Micro, "rankable-only");
        let micro_singles = tier_singles(CostTier::Micro);
        eprintln!(
            "micro tier: joint {micro_win} vs Σ singles {micro_singles} \
             (excess {})",
            micro_win - micro_singles
        );
        let product_win = tier_win(CostTier::Product, "rankable-only");
        let product_singles = tier_singles(CostTier::Product);
        eprintln!(
            "sum-of-singles by tier: micro joint={micro_win} Σ={micro_singles} \
             (excess {}); product joint={product_win} Σ={product_singles} \
             (excess {})",
            micro_win - micro_singles,
            product_win - product_singles
        );

        let e_win = dev_cycles - sum("release-minus-F", |c| c.proxy_cycles);

        let blk = discover_cost_cases_in(CostTier::Product)
            .into_iter()
            .find(|c| c.name == "cost-product-blk")
            .expect("cost-product-blk must exist");
        let blk_dev = score_path_under_opts(&blk.input, &[]) as i64;
        let solo_ni = blk_dev - score_path_under_opts(&blk.input, &[OptId::NarrowImm]) as i64;
        let solo_ra = blk_dev - score_path_under_opts(&blk.input, &[OptId::RegAlloc]) as i64;
        let joint_ni_ra = blk_dev
            - score_path_under_opts(&blk.input, &[OptId::NarrowImm, OptId::RegAlloc]) as i64;
        apply_mode(CompileMode::Release);
        assert!(
            joint_ni_ra > solo_ni + solo_ra,
            "the product tier's super-additivity is claimed to be \
             NarrowImm+RegAlloc on cost-product-blk: joint {joint_ni_ra} vs \
             {solo_ni} + {solo_ra}"
        );
        assert!(
            rel_win > e_win,
            "item F's three ids must add cycles on top of the whole list \
             before them: release {rel_win} vs release-minus-F {e_win}
{table}"
        );
        eprintln!(
            "item F block win (release-minus-F -> release): {} proxy cycles",
            rel_win - e_win
        );
        assert!(
            wins.iter().all(|(_, w)| rel_win > *w),
            "release must beat every single: release {rel_win} vs {wins:?}
{table}"
        );
    }

    #[ignore = "deep lane: run via `cargo xtask verify-deep` (or --ignored)"]
    #[test]
    fn narrow_imm_alone_wins_at_every_box_point() {
        let cmp = compare_opt_lists_over_box(&[], &[OptId::NarrowImm]).expect("sweep");
        assert_sweep_wins(&cmp, "NarrowImm", "dev");
        for c in &cmp.cases {
            assert!(
                !c.points.is_empty() && c.points.iter().all(|p| p.candidate < p.baseline),
                "{}: NarrowImm must fall at every box point",
                c.name
            );
        }
        eprintln!(
            "NarrowImm ∀-sweep: {} points/side over {} cases",
            cmp.scored_points(),
            cmp.cases.len()
        );
    }

    #[test]
    fn adr_addressing_wins_at_every_box_point_on_the_smoke_case() {
        let cmp = compare_opt_lists_over_box_for_case(&[], &[OptId::AdrAddressing], "cost-arith")
            .expect("smoke sweep");
        assert_eq!(cmp.cases.len(), 1, "the smoke lane sweeps exactly one case");
        let case = &cmp.cases[0];
        assert!(
            !case.points.is_empty(),
            "the smoke case must enumerate corners, not zero"
        );
        assert!(
            case.points.iter().all(|p| p.candidate <= p.baseline)
                && case.points.iter().any(|p| p.candidate < p.baseline),
            "AdrAddressing must not grow and must win at some point of {}: {:?}",
            case.name,
            case.points
                .iter()
                .map(|p| (p.baseline, p.candidate))
                .collect::<Vec<_>>()
        );
        assert!(
            cmp.reasons.iter().all(|r| {
                !r.label().starts_with("budget_growth")
                    && !r.label().starts_with("ordering_removed")
            }),
            "smoke sweep must have no growth/order veto: {:?}",
            cmp.reasons.iter().map(|r| r.label()).collect::<Vec<_>>()
        );
    }

    #[test]
    fn adr_addressing_is_a_marginal_win_over_the_previous_release_list() {
        const WITHOUT: &[OptId] = &[OptId::NarrowImm];
        let cmp = compare_opt_lists_over_box_for_case(WITHOUT, RELEASE_OPTS, "cost-arith")
            .expect("marginal smoke sweep");
        let case = &cmp.cases[0];
        assert!(
            case.points.iter().all(|p| p.candidate < p.baseline),
            "adding AdrAddressing must fall at every point of {}: {:?}",
            case.name,
            case.points
                .iter()
                .map(|p| (p.baseline, p.candidate))
                .collect::<Vec<_>>()
        );
        assert!(
            cmp.wins(),
            "marginal smoke sweep vetoed: {:?}",
            cmp.reasons.iter().map(|r| r.label()).collect::<Vec<_>>()
        );
    }

    #[ignore = "deep lane: run via `cargo xtask verify-deep` (or --ignored)"]
    #[test]
    fn residual_release_list_wins_at_every_point_of_the_box() {
        const RESIDUAL_BASE: &[OptId] = &[OptId::NarrowImm];
        let cmp = compare_opt_lists_over_box(RESIDUAL_BASE, RELEASE_OPTS).expect("sweep");
        let table = format_sweep_table(&cmp, "residual-base", "release");
        eprintln!("∀ sweep (residual-base → release):\n{table}");
        assert_sweep_wins(&cmp, "release", "residual-base");
        assert!(cmp.wins());
        eprintln!(
            "residual release-list ∀-sweep: {} points/side over {} cases",
            cmp.scored_points(),
            cmp.cases.len()
        );
    }

    #[ignore = "deep lane: run via `cargo xtask verify-deep` (or --ignored)"]
    #[test]
    fn each_release_opt_is_re_asked_alone_on_the_product_tier() {
        assert!(
            !RELEASE_OPTS.is_empty(),
            "an empty RELEASE_OPTS would make this lane vacuous"
        );
        let mut verdicts = Vec::new();
        let mut measured: Vec<(String, &'static str)> = Vec::new();
        for opt in RELEASE_OPTS {
            let label = format!("{opt:?}");
            let base: Vec<OptId> = match opt {
                OptId::WideImmForms => vec![OptId::NarrowImm, OptId::RegAlloc],
                OptId::Gvn => {
                    let mut b = item_j_baseline();
                    b.push(OptId::ConstProp);
                    b
                }
                OptId::InterprocRegs => item_f_baseline(),
                OptId::Frameless => {
                    let mut b = item_f_baseline();
                    b.push(OptId::InterprocRegs);
                    b
                }
                OptId::TailCalls => {
                    let mut b = item_f_baseline();
                    b.push(OptId::InterprocRegs);
                    b.push(OptId::Frameless);
                    b
                }
                _ => Vec::new(),
            };
            let base_label = if base.is_empty() {
                "dev".to_string()
            } else {
                format!("{base:?}")
            };
            let cmp = compare_opt_lists_over_box_in_tier(
                &base,
                &[&base[..], &[*opt][..]].concat(),
                CostTier::Product,
            )
            .unwrap_or_else(|e| panic!("{label}: product-tier sweep: {e}"));
            let table = format_sweep_table(&cmp, &base_label, &label);
            eprintln!("∀ sweep, product tier only ({base_label} → {label}):\n{table}");
            assert_eq!(cmp.tiers(), vec![CostTier::Product]);
            assert!(
                cmp.scored_points_in(CostTier::Product) > 0,
                "{label}: the product tier enumerated no points"
            );
            let reasons: Vec<String> = cmp.reasons.iter().map(SweepVeto::label).collect();
            let verdict = if cmp.wins_in_tier(CostTier::Product) {
                "wins"
            } else {
                "veto"
            };
            measured.push((label.clone(), verdict));
            verdicts.push(format!(
                "{label}: product={} ({} points/side over {} case(s)) reasons=[{}]",
                verdict,
                cmp.scored_points_in(CostTier::Product),
                cmp.cases_in(CostTier::Product).len(),
                reasons.join(" ")
            ));
        }
        eprintln!(
            "RELEASE_OPTS re-asked alone on the product tier:\n{}",
            verdicts.join("\n")
        );
        let measured: Vec<(&str, &str)> = measured.iter().map(|(l, v)| (l.as_str(), *v)).collect();
        assert_eq!(
            measured.as_slice(),
            PINNED_PRODUCT_TIER_VERDICTS,
            "**Decision 1785 — the pinned product-tier verdict set moved.**\n\
             \n\
             This is a *measurement*, pinned so that it cannot change \
             quietly in either direction, not a target. What it currently \
             records is item H's headline finding (plans/codegen-pareto-H.md):\n\
             \n\
             - `NarrowImm` alone falls at every point of every borrowed \
             program. It is justified by the appliance, not only by the \
             corpus.\n\
             - There is **no `BoundsElide` row**, because this loop asks \
             `RELEASE_OPTS` and `BoundsElide` is **parked** — in the tree, \
             out of the shipped list (`opts::PARKED_OPTS`, decisions \
             1970/1911). Item H measured it byte-identical to `dev` on all \
             four product cases: same cycles, same emitted words, same hot \
             text. Its `veto` row was the only one this set ever carried, \
             and a permanent `veto` does not belong in the product's own \
             list; the refusal, the mechanism and the condition for \
             re-asking it live on `PARKED_OPTS` instead.\n\
             \n\
             `RELEASE_OPTS` as a *list* still wins ∀ in both tiers \
             (`unit:release_wins_at_every_point_of_the_residual_box`), which \
             is what freeze 1714 gates, so nothing here un-lands anything. \
             Item H adds no opt and removes none. If this assertion fires, \
             re-derive the row it names and update the finding — do not \
             delete the row to get green.\n\
             \n{}",
            verdicts.join("\n")
        );
    }

    const PINNED_PRODUCT_TIER_VERDICTS: &[(&str, &str)] = &[
        ("ConstProp", "wins"),
        ("Gvn", "wins"),
        ("Dce", "wins"),
        ("NarrowImm", "wins"),
        ("AdrAddressing", "wins"),
        ("BfxNarrow", "wins"),
        ("MaskCheck", "wins"),
        ("WideImmForms", "wins"),
        ("RegAlloc", "wins"),
        ("InterprocRegs", "wins"),
        ("Frameless", "wins"),
        ("BranchCleanup", "wins"),
        ("TailCalls", "wins"),
    ];

    fn item_j_baseline() -> Vec<OptId> {
        RELEASE_OPTS
            .iter()
            .copied()
            .filter(|o| !matches!(o, OptId::ConstProp | OptId::Gvn | OptId::Dce))
            .collect()
    }

    const ITEM_J_CHAIN: &[(&str, &[OptId], &str)] = &[
        ("ConstProp", &[OptId::ConstProp], "cost-product-compositor"),
        ("Gvn+Dce", &[OptId::Gvn, OptId::Dce], "cost-icache-cliff"),
        ("Dce", &[OptId::Dce], "cost-arith-w"),
    ];

    fn item_j_link(n: usize) -> (Vec<OptId>, Vec<OptId>) {
        let mut acc = item_j_baseline();
        for &(_, ids, _) in &ITEM_J_CHAIN[..n] {
            for id in ids {
                if !acc.contains(id) {
                    acc.push(*id);
                }
            }
        }
        let mut cand = acc.clone();
        for id in ITEM_J_CHAIN[n].1 {
            if !cand.contains(id) {
                cand.push(*id);
            }
        }
        if ITEM_J_CHAIN[n].0 == "Dce" {
            let base: Vec<OptId> = cand.iter().copied().filter(|o| *o != OptId::Dce).collect();
            return (base, cand);
        }
        (acc, cand)
    }

    #[test]
    fn each_item_j_link_wins_at_the_default_point_on_its_smoke_case() {
        let cases = discover_cost_cases();
        for (n, &(label, _, case_name)) in ITEM_J_CHAIN.iter().enumerate() {
            let case = cases
                .iter()
                .find(|case| case.name == case_name)
                .unwrap_or_else(|| panic!("missing Item J smoke case `{case_name}`"));
            let (base, cand) = item_j_link(n);
            let (before, before_scope) = shipped_report_under_opts(&case.input, &base);
            let (after, after_scope) = shipped_report_under_opts(&case.input, &cand);
            assert_eq!(before_scope, after_scope, "{label} changed shipped scope");
            assert!(
                after.total_proxy_cycles < before.total_proxy_cycles,
                "{label} must win at the default point of {case_name}: {} -> {}",
                before.total_proxy_cycles,
                after.total_proxy_cycles
            );
        }
        apply_mode(CompileMode::Release);
    }

    #[ignore = "milestone lane: every Item J link over its whole sweep box"]
    #[test]
    fn each_item_j_link_wins_at_every_box_point_on_its_smoke_case() {
        for (n, &(label, _, case)) in ITEM_J_CHAIN.iter().enumerate() {
            let (base, cand) = item_j_link(n);
            let cmp = compare_opt_lists_over_box_for_case(&base, &cand, case)
                .unwrap_or_else(|e| panic!("{label} on {case}: {e}"));
            assert_eq!(cmp.cases.len(), 1, "the smoke lane sweeps exactly one case");
            let sweep = &cmp.cases[0];
            assert!(
                !sweep.points.is_empty(),
                "{label} on {case}: the probe enumerated no corners"
            );
            assert!(
                sweep.points.iter().all(|p| p.candidate < p.baseline),
                "{label} must fall at every point of {case}; got {:?}",
                sweep
                    .points
                    .iter()
                    .map(|p| (p.baseline, p.candidate))
                    .collect::<Vec<_>>()
            );
            assert!(
                cmp.wins(),
                "{label} refused on {case}: {:?}",
                cmp.reasons.iter().map(SweepVeto::label).collect::<Vec<_>>()
            );
            eprintln!(
                "item J smoke: {label} on {case} — {} corners, {} -> {} at the first",
                sweep.points.len(),
                sweep.points[0].baseline,
                sweep.points[0].candidate
            );
        }
    }

    #[ignore = "deep lane: run via `cargo xtask verify-deep` (or --ignored)"]
    #[test]
    fn each_item_j_link_wins_over_the_whole_box() {
        const MICRO_MARGINAL_ZERO: &[&str] = &["ConstProp"];
        for (n, &(label, _, _)) in ITEM_J_CHAIN.iter().enumerate() {
            let (base, cand) = item_j_link(n);
            let base_label = format!("{base:?}");
            let cmp =
                compare_opt_lists_over_box(&base, &cand).unwrap_or_else(|e| panic!("{label}: {e}"));
            let table = format_sweep_table(&cmp, &base_label, label);
            eprintln!("∀ sweep ({base_label} → +{label}):\n{table}");
            for tier in CostTier::ALL {
                let rose: Vec<String> = cmp
                    .reasons_for_tier(tier)
                    .iter()
                    .map(|r| r.label())
                    .filter(|l| l.starts_with("case_rose"))
                    .collect();
                assert!(
                    rose.is_empty(),
                    "{label} ({tier:?}) made a case worse, which is a veto rather \
                     than a marginal zero: {rose:?}\n{table}"
                );
                if tier == CostTier::Micro && MICRO_MARGINAL_ZERO.contains(&label) {
                    continue;
                }
                assert!(
                    cmp.wins_in_tier(tier),
                    "{label} must win in {tier}: {:?}\n{table}",
                    cmp.reasons_for_tier(tier)
                        .iter()
                        .map(|r| r.label())
                        .collect::<Vec<_>>()
                );
            }
        }
    }

    #[ignore = "milestone lane: whole-box GVN attribution"]
    #[test]
    fn gvn_collects_its_own_copies() {
        let (base, cand) = item_j_link(1);
        let mut gvn_only: Vec<OptId> = base.clone();
        gvn_only.push(OptId::Gvn);
        let alone = compare_opt_lists(&base, &gvn_only);
        let paired = compare_opt_lists(&base, &cand);
        let rose = |c: &CorpusCompare| -> Vec<String> {
            c.cases
                .iter()
                .filter(|r| r.candidate > r.baseline)
                .map(|r| format!("{}(+{})", r.name, r.candidate - r.baseline))
                .collect()
        };
        eprintln!(
            "item J: Gvn alone rose on {:?}; Gvn+Dce rose on {:?}",
            rose(&alone),
            rose(&paired)
        );
        assert!(
            alone.candidate_sum < alone.baseline_sum,
            "GVN alone must still be a large fall overall: {} -> {}",
            alone.baseline_sum,
            alone.candidate_sum
        );
        assert!(
            !rose(&alone).is_empty(),
            "if GVN alone stopped raising anything it would be rankable alone \
             and ITEM_J_CHAIN's pair should be split back into two links"
        );
        assert!(
            rose(&paired).is_empty(),
            "the pair must raise nothing: {:?}",
            rose(&paired)
        );
        apply_mode(CompileMode::Release);
    }

    #[ignore = "milestone lane: shipped-list Item J comparison"]
    #[test]
    fn item_j_as_a_block_over_the_shipped_list() {
        let base = item_j_baseline();
        let cmp = compare_opt_lists(&base, RELEASE_OPTS);
        let table = format_delta_table(&cmp, "release-minus-J", "release");
        eprintln!("item J as a block:\n{table}");
        assert_wins(&cmp, "release", "release-minus-J");
        assert_eq!(
            (cmp.baseline_sum, cmp.candidate_sum),
            (207_196, 185_636),
            "item J's measured size over the shipped list (-10.3%); re-measure \
             this rather than rescaling it"
        );
        apply_mode(CompileMode::Release);
    }

    #[ignore = "deep lane: six whole-corpus comparisons (--ignored)"]
    #[test]
    fn the_inliner_measured_in_both_pipeline_positions() {
        let mut framings: Vec<(&str, Vec<OptId>, Vec<OptId>, bool)> = Vec::new();
        for (label, base, rule_one_only) in [
            (
                "[ConstProp,Gvn,Dce]",
                vec![OptId::ConstProp, OptId::Gvn, OptId::Dce],
                false,
            ),
            ("release-minus-Inline", RELEASE_OPTS.to_vec(), false),
            (
                "release-minus-Inline, rule (i)",
                RELEASE_OPTS.to_vec(),
                true,
            ),
        ] {
            let mut cand = base.clone();
            cand.push(OptId::Inline);
            framings.push((label, base, cand, rule_one_only));
        }

        let mut summary = String::from(
            "\nitem P — the inliner in both pipeline positions\n\
             framing                              position                 Δcycles   Δwords\n",
        );
        let mut moved_somewhere = false;
        for (label, base, cand, rule_one_only) in &framings {
            crate::mwir_opt::set_inline_rule_one_only(*rule_one_only);
            for after in [false, true] {
                crate::mwir_opt::set_inline_after_redundancy(after);
                let position = if after {
                    "ConstProp/Gvn/Dce → inline"
                } else {
                    "inline → ConstProp/Gvn/Dce"
                };
                let cmp = compare_opt_lists(base, cand);
                eprintln!(
                    "item P: {label} → +Inline, {position}:\n{}",
                    format_delta_table(&cmp, label, "+Inline")
                );
                if cmp.sum_delta() != 0 || cmp.words_delta() != 0 {
                    moved_somewhere = true;
                }
                summary.push_str(&format!(
                    "{label:<36} {position:<26} {:>+8} {:>+8}\n",
                    cmp.sum_delta(),
                    cmp.words_delta()
                ));
            }
        }
        crate::mwir_opt::set_inline_rule_one_only(false);
        crate::mwir_opt::set_inline_after_redundancy(false);
        apply_mode(CompileMode::Release);
        eprintln!("{summary}");
        assert!(
            moved_somewhere,
            "the inliner reached nothing anywhere on this corpus — a refusal \
             measured over zero call sites is clean about nothing"
        );
        assert!(
            !RELEASE_OPTS.contains(&OptId::Inline),
            "item P reports the number and parks the opt; a human decides"
        );
    }

    #[ignore = "deep lane: six shipped-image compilations (--ignored)"]
    #[test]
    fn the_inliner_on_the_two_shipped_images_in_both_positions() {
        let appliance = golden_root().join("appliance/src/image.wr");
        let compositor = golden_root().join("boot-tile-compositor/input.wr");
        assert!(appliance.is_file(), "{}", appliance.display());
        assert!(compositor.is_file(), "{}", compositor.display());
        let mut with_inline: Vec<OptId> = RELEASE_OPTS.to_vec();
        with_inline.push(OptId::Inline);
        const MNEMONICS: [&str; 6] = ["str", "ldr", "mov", "bl", "movz", "movk"];

        let measure = |path: &Path, opts: &[OptId]| -> (u64, Vec<usize>) {
            let (report, _) = shipped_report_under_opts(path, opts);
            let (program, ..) = crate::cost::stage::codegen_shipped_program(path)
                .unwrap_or_else(|e| panic!("{}: {e}", path.display()));
            let asm = crate::codegen::dump(&program);
            let counts = MNEMONICS
                .iter()
                .map(|m| {
                    asm.lines()
                        .filter(|l| l.split_whitespace().nth(2) == Some(m))
                        .count()
                })
                .collect();
            (report.total_words, counts)
        };

        let mut table = String::from("\nitem P — shipped images, emitted words\n");
        table.push_str(&format!(
            "{:<12} {:<27} {:>8} {:>8} {:>6} {:>5} {:>5}",
            "image", "position", "release", "+Inline", "Δ", "sites", "moved"
        ));
        for m in MNEMONICS {
            table.push_str(&format!(" {m:>7}"));
        }
        table.push('\n');
        for (name, path) in [("appliance", &appliance), ("compositor", &compositor)] {
            crate::mwir_opt::set_inline_after_redundancy(false);
            let (base_words, base_counts) = measure(path, RELEASE_OPTS);
            let _ = crate::mwir_opt::take_inline_reach();
            for after in [false, true] {
                crate::mwir_opt::set_inline_after_redundancy(after);
                let (cand_words, cand_counts) = measure(path, &with_inline);
                let (sites, moved) = crate::mwir_opt::take_inline_reach();
                let position = if after {
                    "ConstProp/Gvn/Dce → inline"
                } else {
                    "inline → ConstProp/Gvn/Dce"
                };
                table.push_str(&format!(
                    "{name:<12} {position:<27} {base_words:>8} {cand_words:>8} {:>+6} {:>5} {:>5}",
                    cand_words as i64 - base_words as i64,
                    sites / 2,
                    moved / 2,
                ));
                for (b, c) in base_counts.iter().zip(cand_counts.iter()) {
                    table.push_str(&format!(" {:>+7}", *c as i64 - *b as i64));
                }
                table.push('\n');
            }
        }
        crate::mwir_opt::set_inline_after_redundancy(false);
        apply_mode(CompileMode::Release);
        eprintln!("{table}\n(the mnemonic columns are Δ against release)");
    }

    #[ignore = "milestone lane: whole-corpus release-order comparison"]
    #[test]
    fn swapped_order_scores_same_as_release_opts() {
        let swapped: Vec<OptId> = RELEASE_OPTS.iter().rev().copied().collect();
        assert_ne!(
            swapped.as_slice(),
            RELEASE_OPTS,
            "the reversal must actually reorder something"
        );
        let cmp = compare_opt_lists(RELEASE_OPTS, &swapped);
        let table = format_delta_table(&cmp, "RELEASE_OPTS", "swapped");
        eprintln!("order swap note:\n{table}");
        assert_eq!(
            cmp.baseline_sum,
            cmp.candidate_sum,
            "swapped order should be independent (equal totals); got Δ {}",
            cmp.sum_delta()
        );
        for c in &cmp.cases {
            assert_eq!(
                c.baseline, c.candidate,
                "{} differed under swapped order",
                c.name
            );
        }
    }

    #[ignore = "milestone lane: whole-corpus negative optimization oracle"]
    #[test]
    #[should_panic(expected = "must strictly lower at least one")]
    fn empty_candidate_fails_win_oracle() {
        let _ = assert_candidate_wins(&[], &[]);
    }

    #[ignore = "milestone lane: whole-corpus negative optimization oracle"]
    #[test]
    #[should_panic(expected = "raised proxy total")]
    fn disabling_shipped_opts_fails_candidate_oracle() {
        let _ = assert_candidate_wins(RELEASE_OPTS, &[OptId::NarrowImm]);
    }

    fn pinned_set() -> WorkloadSet {
        load_pinned_workloads().expect("load bench/workloads.toml")
    }

    fn totals(pairs: &[(&str, u64)]) -> OverallSide {
        OverallSide::from_totals(pairs.iter().map(|(n, v)| ((*n).to_string(), *v)).collect())
    }

    fn cov(pairs: &[(&str, u64, u64)]) -> BTreeMap<String, (u64, u64)> {
        pairs
            .iter()
            .map(|(n, m, t)| ((*n).to_string(), (*m, *t)))
            .collect()
    }

    #[test]
    fn load_pinned_workloads_has_flat_and_boot_actors() {
        let w = pinned_set();
        assert_eq!(w.flat_weight(), 1);
        assert_eq!(w.weight("boot-actors"), Some(10));
        assert!(!w.digest().is_empty());
    }

    #[test]
    fn overall_vetoes_when_non_flat_rises() {
        let set = pinned_set();
        let baseline = totals(&[("flat", 1000), ("boot-actors", 5000)]);
        let candidate = totals(&[("flat", 800), ("boot-actors", 5001)]);
        let cmp = compare_overall(&baseline, &candidate, &set).expect("compare");
        let table = format_overall_table(&cmp, "baseline", "candidate");
        eprintln!("overall veto case:\n{table}");
        assert!(cmp.vetoed(), "must veto when boot-actors rises");
        assert!(!cmp.wins());
        assert_eq!(cmp.risen(), vec!["boot-actors".to_string()]);
        assert_eq!(
            cmp.veto_reasons(),
            &[VetoReason::WorkloadRose {
                name: "boot-actors".to_string()
            }]
        );
        assert!(table.contains("boot-actors"));
        assert!(table.contains("outcome=veto"));
        assert!(table.contains("workload_rose:boot-actors"));
    }

    #[test]
    fn overall_flat_rise_alone_does_not_veto() {
        let set = pinned_set();
        let baseline = totals(&[("flat", 1000), ("boot-actors", 5000)]);
        let candidate = totals(&[("flat", 1100), ("boot-actors", 4000)]);
        let cmp = compare_overall(&baseline, &candidate, &set).expect("compare");
        let table = format_overall_table(&cmp, "baseline", "candidate");
        eprintln!("overall flat-rise rank win:\n{table}");
        assert!(!cmp.vetoed(), "flat rise must not veto");
        assert!(cmp.wins(), "weighted mean must be negative");
        let mean = cmp.weighted_mean_rel().expect("rank");
        let expected = (1.0 * 0.10 + 10.0 * (-0.20)) / 11.0;
        assert!(
            (mean - expected).abs() < 1e-9,
            "mean {mean} != expected {expected}"
        );
        assert!(table.contains("outcome=rank"));
        assert!(table.contains("wins=true"));
    }

    #[test]
    fn overall_rank_loss_when_weighted_mean_non_negative() {
        let set = pinned_set();
        let baseline = totals(&[("flat", 1000), ("boot-actors", 5000)]);
        let candidate = totals(&[("flat", 1200), ("boot-actors", 5000)]);
        let cmp = compare_overall(&baseline, &candidate, &set).expect("compare");
        let table = format_overall_table(&cmp, "baseline", "candidate");
        eprintln!("overall rank loss:\n{table}");
        assert!(!cmp.vetoed());
        assert!(!cmp.wins());
        let mean = cmp.weighted_mean_rel().expect("rank");
        assert!(mean > 0.0, "expected positive mean, got {mean}");
    }

    #[test]
    fn overall_equal_non_flat_is_not_veto() {
        let set = pinned_set();
        let baseline = totals(&[("flat", 1000), ("boot-actors", 5000)]);
        let candidate = totals(&[("flat", 900), ("boot-actors", 5000)]);
        let cmp = compare_overall(&baseline, &candidate, &set).expect("compare");
        assert!(!cmp.vetoed());
        assert!(cmp.wins());
    }

    #[test]
    fn overall_missing_workload_fails_closed() {
        let set = pinned_set();
        let baseline = OverallSide::from_totals(flat_only_totals(100));
        let candidate = OverallSide::from_totals(flat_only_totals(90));
        let err = compare_overall(&baseline, &candidate, &set).expect_err("missing");
        assert!(
            err.contains("boot-actors"),
            "error should name missing W, got: {err}"
        );
    }

    #[ignore = "milestone lane: whole-corpus aggregate ranking"]
    #[test]
    fn overall_stub_all_with_corpus_sums_ranks_like_flat() {
        let set = pinned_set();
        let cmp_corpus = compare_opt_lists(&[], RELEASE_OPTS);
        let baseline =
            OverallSide::from_totals(stub_all_workload_totals(cmp_corpus.baseline_sum, &set))
                .with_words(cmp_corpus.baseline_words);
        let candidate =
            OverallSide::from_totals(stub_all_workload_totals(cmp_corpus.candidate_sum, &set))
                .with_words(cmp_corpus.candidate_words);
        let overall = compare_overall(&baseline, &candidate, &set).expect("compare");
        let table = format_overall_table(&overall, "dev", "release");
        eprintln!("overall stubbed corpus sums:\n{table}");
        assert!(!overall.vetoed());
        assert!(
            overall.wins(),
            "release corpus sum drop must yield overall win under stubs"
        );
        let flat_rel = (cmp_corpus.sum_delta() as f64) / (cmp_corpus.baseline_sum as f64);
        let mean = overall.weighted_mean_rel().expect("rank");
        assert!(
            (mean - flat_rel).abs() < 1e-12,
            "stubbed mean {mean} should equal flat rel {flat_rel}"
        );
        for r in &overall.workloads {
            assert!(table.contains(&r.name), "table missing {}", r.name);
        }
        apply_mode(CompileMode::Release);
    }

    #[test]
    fn assert_overall_wins_panics_on_veto() {
        let set = pinned_set();
        let baseline = totals(&[("flat", 100), ("boot-actors", 100)]);
        let candidate = totals(&[("flat", 50), ("boot-actors", 101)]);
        let cmp = compare_overall(&baseline, &candidate, &set).expect("compare");
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            assert_overall_wins(&cmp, "candidate", "baseline");
        }));
        assert!(result.is_err(), "assert_overall_wins must panic on veto");
    }

    #[test]
    fn flat_only_totals_shape() {
        let m = flat_only_totals(42);
        assert_eq!(m.len(), 1);
        assert_eq!(m.get(FLAT_NAME), Some(&42));
    }

    #[test]
    fn overall_vetoes_when_coverage_falls() {
        let set = pinned_set();
        let baseline = totals(&[("flat", 1000), ("boot-actors", 5000)]).with_coverage(cov(&[(
            "boot-actors",
            11,
            11,
        )]));
        let candidate = totals(&[("flat", 900), ("boot-actors", 4000)]).with_coverage(cov(&[(
            "boot-actors",
            8,
            11,
        )]));
        let cmp = compare_overall(&baseline, &candidate, &set).expect("compare");
        let table = format_overall_table(&cmp, "baseline", "candidate");
        eprintln!("overall coverage veto:\n{table}");
        assert!(cmp.vetoed(), "coverage loss must veto");
        assert!(!cmp.wins());
        assert_eq!(
            cmp.veto_reasons(),
            &[VetoReason::CoverageFell {
                name: "boot-actors".to_string(),
                baseline: (11, 11),
                candidate: (8, 11),
            }]
        );
        assert!(
            table.contains("coverage boot-actors 11/11 -> 8/11"),
            "{table}"
        );
    }

    #[test]
    fn overall_vetoes_when_coverage_row_missing() {
        let set = pinned_set();
        let baseline = totals(&[("flat", 1000), ("boot-actors", 5000)]).with_coverage(cov(&[(
            "boot-actors",
            11,
            11,
        )]));
        let candidate = totals(&[("flat", 900), ("boot-actors", 10)]);
        let cmp = compare_overall(&baseline, &candidate, &set).expect("compare");
        assert!(cmp.vetoed());
        assert_eq!(
            cmp.veto_reasons(),
            &[VetoReason::CoverageFell {
                name: "boot-actors".to_string(),
                baseline: (11, 11),
                candidate: (0, 11),
            }]
        );
    }

    #[test]
    fn overall_rising_coverage_does_not_veto() {
        let set = pinned_set();
        let baseline = totals(&[("flat", 1000), ("boot-actors", 5000)]).with_coverage(cov(&[(
            "boot-actors",
            8,
            11,
        )]));
        let candidate = totals(&[("flat", 900), ("boot-actors", 4900)]).with_coverage(cov(&[(
            "boot-actors",
            11,
            11,
        )]));
        let cmp = compare_overall(&baseline, &candidate, &set).expect("compare");
        assert!(!cmp.vetoed());
        assert!(cmp.wins());
    }

    #[test]
    fn overall_coverage_denominator_change_fails_closed() {
        let set = pinned_set();
        let baseline = totals(&[("flat", 1000), ("boot-actors", 5000)]).with_coverage(cov(&[(
            "boot-actors",
            11,
            11,
        )]));
        let candidate = totals(&[("flat", 900), ("boot-actors", 4000)]).with_coverage(cov(&[(
            "boot-actors",
            11,
            20,
        )]));
        let err = compare_overall(&baseline, &candidate, &set).expect_err("denominator");
        assert!(err.contains("denominator"), "got: {err}");
    }

    fn budget(n: usize, over_l1i_lines: u64, over_itlb_pages: u64, charge: u64) -> CoreBudget {
        CoreBudget {
            n,
            fetched_text_bytes: 91712,
            executable_code_bytes: 84284,
            l1i_bytes: 65536,
            over_l1i_lines,
            over_l2_lines: 0,
            over_l3_lines: 0,
            text_pages: if over_itlb_pages == 0 {
                23
            } else {
                48 + over_itlb_pages
            },
            itlb_entries: 48,
            over_itlb_pages,
            tlb_l2_entries: 1280,
            over_tlb_l2_pages: 0,
            data_pages: 6,
            over_dtlb_pages: 0,
            over_data_tlb_l2_pages: 0,
            charge,
        }
    }

    #[test]
    fn overall_vetoes_when_a_core_budget_overflow_grows() {
        let set = pinned_set();
        let baseline = totals(&[("flat", 1000), ("boot-actors", 5000)])
            .with_budgets(vec![budget(0, 409, 0, 2863), budget(1, 413, 0, 2891)]);
        let candidate = totals(&[("flat", 900), ("boot-actors", 4000)])
            .with_budgets(vec![budget(0, 409, 0, 2863), budget(1, 414, 0, 2891)]);
        let cmp = compare_overall(&baseline, &candidate, &set).expect("compare");
        let table = format_overall_table(&cmp, "baseline", "candidate");
        eprintln!("overall budget veto:\n{table}");
        assert!(cmp.vetoed(), "a rising overflow must veto:\n{table}");
        assert!(!cmp.wins());
        assert_eq!(
            cmp.veto_reasons(),
            &[VetoReason::BudgetOverflowGrew {
                core: 1,
                field: "over_l1i_lines",
                baseline: 413,
                candidate: 414,
            }]
        );
        assert!(
            table.contains("budget_grew:core1:over_l1i_lines:413->414"),
            "the refusal must name itself (04 §5):\n{table}"
        );
    }

    #[test]
    fn overall_budget_veto_watches_every_over_quantity() {
        let set = pinned_set();
        let base_b = budget(0, 409, 0, 2863);
        let fields = [
            "over_l1i_lines",
            "over_l2_lines",
            "over_l3_lines",
            "over_itlb_pages",
            "over_tlb_l2_pages",
            "over_dtlb_pages",
            "over_data_tlb_l2_pages",
            "charge",
        ];
        for field in fields {
            let mut c = base_b.clone();
            match field {
                "over_l1i_lines" => c.over_l1i_lines += 1,
                "over_l2_lines" => c.over_l2_lines += 1,
                "over_l3_lines" => c.over_l3_lines += 1,
                "over_itlb_pages" => c.over_itlb_pages += 1,
                "over_tlb_l2_pages" => c.over_tlb_l2_pages += 1,
                "over_dtlb_pages" => c.over_dtlb_pages += 1,
                "over_data_tlb_l2_pages" => c.over_data_tlb_l2_pages += 1,
                "charge" => c.charge += 1,
                _ => unreachable!(),
            }
            let baseline =
                totals(&[("flat", 1000), ("boot-actors", 5000)]).with_budgets(vec![base_b.clone()]);
            let candidate = totals(&[("flat", 900), ("boot-actors", 4000)]).with_budgets(vec![c]);
            let cmp = compare_overall(&baseline, &candidate, &set).expect("compare");
            assert!(cmp.vetoed(), "{field} growth must veto");
            let labels: Vec<String> = cmp.veto_reasons().iter().map(|r| r.label()).collect();
            assert!(
                labels.iter().any(|l| l.contains(field)),
                "{field} must be named, got {labels:?}"
            );
        }
    }

    #[test]
    fn an_over_budget_identity_is_refused_absolutely_and_allowed_as_a_delta() {
        let set = pinned_set();
        let boot = vec![
            budget(0, 409, 0, 2863),
            budget(1, 413, 0, 2891),
            budget(2, 410, 0, 2870),
        ];
        for b in &boot {
            assert!(
                !b.within_budget(),
                "the premise: core {} is already over budget under W_flat",
                b.n
            );
        }
        let side = totals(&[("flat", 1000), ("boot-actors", 5000)]).with_budgets(boot.clone());
        let mut better = side.clone();
        better.totals.insert("flat".to_string(), 900);
        let cmp = compare_overall(&side, &better, &set).expect("compare");
        let table = format_overall_table(&cmp, "baseline", "identical-budgets");
        eprintln!("decision 1619 counter-example:\n{table}");
        assert!(
            !cmp.vetoed(),
            "the delta rule must not fire on unchanged budgets — the absolute \
             rule would have refused every one of these three cores:\n{table}"
        );
        assert!(cmp.wins());
        let mut worse = boot.clone();
        worse[0].over_l1i_lines += 1;
        let cmp =
            compare_overall(&side, &better.clone().with_budgets(worse), &set).expect("compare");
        assert!(
            cmp.vetoed(),
            "growth from an already-over baseline still vetoes"
        );
    }

    #[test]
    fn an_over_itlb_baseline_is_allowed_but_worsening_the_span_is_refused() {
        let set = pinned_set();
        let over = totals(&[("flat", 1000), ("boot-actors", 5000)])
            .with_budgets(vec![budget(0, 0, 2, 116)]);
        let same = totals(&[("flat", 900), ("boot-actors", 4000)])
            .with_budgets(vec![budget(0, 0, 2, 116)]);
        let cmp = compare_overall(&over, &same, &set).expect("compare");
        let labels: Vec<String> = cmp.veto_reasons().iter().map(|r| r.label()).collect();
        assert!(
            !cmp.vetoed(),
            "an unchanged over-span baseline must not veto: {labels:?}"
        );
        let worse = totals(&[("flat", 900), ("boot-actors", 4000)])
            .with_budgets(vec![budget(0, 0, 3, 116)]);
        let cmp = compare_overall(&over, &worse, &set).expect("compare");
        assert!(cmp.vetoed(), "worsening the I-TLB span must still veto");
        let labels: Vec<String> = cmp.veto_reasons().iter().map(|r| r.label()).collect();
        assert!(
            labels.iter().any(|l| l.contains("budget")),
            "the refusal must come from the budget delta, got {labels:?}"
        );
    }

    #[test]
    fn overall_budget_core_count_change_fails_closed() {
        let set = pinned_set();
        let baseline =
            totals(&[("flat", 1000), ("boot-actors", 5000)]).with_budgets(vec![budget(0, 0, 0, 0)]);
        let candidate = totals(&[("flat", 900), ("boot-actors", 4000)])
            .with_budgets(vec![budget(0, 0, 0, 0), budget(1, 0, 0, 0)]);
        let err = compare_overall(&baseline, &candidate, &set).expect_err("core count");
        assert!(err.contains("core count changed 1->2"), "got: {err}");
    }

    #[test]
    fn word_growth_no_longer_vetoes_but_is_still_reported() {
        let set = pinned_set();
        let baseline = totals(&[("flat", 1000), ("boot-actors", 5000)])
            .with_words(4000)
            .with_budgets(vec![budget(0, 409, 0, 2863)]);
        let candidate = totals(&[("flat", 900), ("boot-actors", 4000)])
            .with_words(4100)
            .with_budgets(vec![budget(0, 409, 0, 2863)]);
        let cmp = compare_overall(&baseline, &candidate, &set).expect("compare");
        let table = format_overall_table(&cmp, "baseline", "grew-100-words");
        eprintln!("words retired as a veto:\n{table}");
        assert!(
            !cmp.vetoed(),
            "+100 words inside the same budget is a priced trade, not a \
             refusal (04 §5 as item A rewrote it):\n{table}"
        );
        assert!(cmp.wins());
        assert_eq!(cmp.baseline_words, 4000);
        assert_eq!(cmp.candidate_words, 4100);
        assert!(
            table.contains("words(reported)") && table.contains("+100"),
            "words must still be reported:\n{table}"
        );
        for r in cmp.veto_reasons() {
            assert!(!r.label().contains("words_grew"), "{:?}", r);
        }
    }

    #[test]
    fn measured_rows_print_their_coverage_fraction_beside_the_number() {
        let set = pinned_set();
        let baseline = totals(&[("flat", 1000), ("boot-actors", 5000)]).with_coverage(cov(&[(
            "boot-actors",
            893,
            6647,
        )]));
        let candidate = totals(&[("flat", 900), ("boot-actors", 4000)]).with_coverage(cov(&[(
            "boot-actors",
            893,
            6647,
        )]));
        let cmp = compare_overall(&baseline, &candidate, &set).expect("compare");
        let table = format_overall_table(&cmp, "baseline", "candidate");
        eprintln!("coverage beside the number:\n{table}");
        let row = table
            .lines()
            .find(|l| l.starts_with("boot-actors"))
            .expect("boot-actors row");
        assert!(
            row.contains("893/6647 (13.4%)"),
            "the measured row must carry its coverage fraction: {row}"
        );
        let flat = table
            .lines()
            .find(|l| l.starts_with("flat"))
            .expect("flat row");
        assert!(!flat.contains('%'), "flat is not a measured row: {flat}");
    }

    #[test]
    fn overall_collects_every_veto_reason() {
        let set = pinned_set();
        let baseline = totals(&[("flat", 1000), ("boot-actors", 5000)])
            .with_coverage(cov(&[("boot-actors", 11, 11)]))
            .with_budgets(vec![budget(0, 409, 0, 2863)]);
        let candidate = totals(&[("flat", 900), ("boot-actors", 5001)])
            .with_coverage(cov(&[("boot-actors", 8, 11)]))
            .with_budgets(vec![budget(0, 410, 0, 2870)]);
        let cmp = compare_overall(&baseline, &candidate, &set).expect("compare");
        assert_eq!(cmp.veto_reasons().len(), 4, "{:?}", cmp.veto_reasons());
    }

    fn ord(pairs: &[(&'static str, u64)]) -> OrderingCounts {
        pairs
            .iter()
            .map(|&(rule, n)| (("f".to_string(), rule), n))
            .collect()
    }

    #[test]
    fn overall_refuses_a_candidate_whose_only_gain_is_deleting_a_dmb() {
        let set = pinned_set();
        let baseline = totals(&[("flat", 1000), ("boot-actors", 5000)])
            .with_words(4000)
            .with_ordering(ord(&[
                ("barrier", 6),
                ("load_acquire", 4),
                ("store_release", 6),
            ]));
        let candidate = totals(&[("flat", 900), ("boot-actors", 4000)])
            .with_words(3999)
            .with_ordering(ord(&[
                ("barrier", 5),
                ("load_acquire", 4),
                ("store_release", 6),
            ]));
        let cmp = compare_overall(&baseline, &candidate, &set).expect("compare");
        let table = format_overall_table(&cmp, "baseline", "omit-dmb");
        eprintln!("freeze 1633 refusal:\n{table}");
        assert!(cmp.vetoed(), "deleting a DMB must be refused:\n{table}");
        assert!(!cmp.wins());
        assert_eq!(
            cmp.veto_reasons(),
            &[VetoReason::OrderingWordsRemoved {
                rule: "barrier",
                baseline: 6,
                candidate: 5,
            }]
        );
        assert!(
            table.contains("ordering_words_removed:barrier:6->5"),
            "the refusal must name itself (04 §5):\n{table}"
        );
    }

    #[test]
    fn overall_ordering_refusal_covers_every_crosscore_rule() {
        let set = pinned_set();
        let base_ord = ord(&[
            ("barrier", 6),
            ("load_acquire", 4),
            ("store_release", 6),
            ("system", 1),
        ]);
        let side = |ordering: OrderingCounts| {
            totals(&[("flat", 900), ("boot-actors", 4000)]).with_ordering(ordering)
        };
        let baseline =
            totals(&[("flat", 1000), ("boot-actors", 5000)]).with_ordering(base_ord.clone());
        let same = compare_overall(&baseline, &side(base_ord.clone()), &set).expect("cmp");
        assert!(!same.vetoed() && same.wins());
        let mut more = base_ord.clone();
        more.insert(("f".to_string(), "barrier"), 7);
        let added = compare_overall(&baseline, &side(more), &set).expect("cmp");
        assert!(!added.vetoed() && added.wins());
        for rule in ["barrier", "load_acquire", "store_release", "system"] {
            let mut fewer = base_ord.clone();
            *fewer.get_mut(&("f".to_string(), rule)).unwrap() -= 1;
            let cmp = compare_overall(&baseline, &side(fewer), &set).expect("cmp");
            assert!(cmp.vetoed(), "{rule} removal must be refused");
            assert_eq!(cmp.veto_reasons().len(), 1, "{rule}");
        }
        let cmp = compare_overall(&baseline, &side(ord(&[])), &set).expect("cmp");
        assert_eq!(cmp.veto_reasons().len(), 4, "{:?}", cmp.veto_reasons());
    }

    #[test]
    fn corpus_gate_refuses_barrier_removal() {
        let cmp = CorpusCompare {
            cases: vec![CaseDelta {
                tier: CostTier::Micro,
                scope: TextScope::Closure,
                name: "cost-crosscore".to_string(),
                baseline: 1000,
                candidate: 900,
                baseline_words: 400,
                candidate_words: 399,
                baseline_budgets: Vec::new(),
                candidate_budgets: Vec::new(),
                baseline_ordering: ord(&[("barrier", 2)]),
                candidate_ordering: ord(&[("barrier", 1)]),
            }],
            baseline_sum: 1000,
            candidate_sum: 900,
            baseline_words: 400,
            candidate_words: 399,
        };
        assert!(
            !cmp.wins(),
            "a cycle fall bought by deleting a barrier is not a win"
        );
        assert_eq!(
            cmp.cases[0].ordering_removed(),
            vec![crate::cost::crosscore::OrderingRemoval {
                fn_key: "f".to_string(),
                rule: "barrier",
                baseline: 2,
                candidate: 1,
            }]
        );
        let err = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            assert_wins(&cmp, "candidate", "baseline");
        }))
        .expect_err("must refuse");
        let msg = err
            .downcast_ref::<String>()
            .map(String::as_str)
            .unwrap_or("<non-string panic>")
            .to_string();
        assert!(
            msg.contains("freeze 1633") && msg.contains("ordering_words_removed:f:barrier:2->1"),
            "refusal must name freeze 1633 and the rule, got: {msg}"
        );
    }

    #[ignore = "milestone lane: whole-corpus ordering census"]
    #[test]
    fn release_removes_no_ordering_words_and_the_corpus_reaches_them() {
        let cmp = compare_opt_lists(&[], RELEASE_OPTS);
        let mut reached: BTreeMap<String, u64> = BTreeMap::new();
        for c in &cmp.cases {
            assert!(
                c.ordering_removed().is_empty(),
                "{}: release removed an ordering word",
                c.name
            );
            assert!(
                c.baseline_ordering.len() % 4 == 0 && !c.baseline_ordering.is_empty(),
                "{}: every crosscore rule must have a slot on every fn, present at 0",
                c.name
            );
            for (rule, n) in &c.baseline_ordering {
                *reached.entry(rule.1.to_string()).or_insert(0) += *n;
            }
        }
        eprintln!("cost-* corpus ordering-word census: {reached:?}");
        for rule in ["barrier", "load_acquire", "store_release", "system"] {
            assert!(
                reached.get(rule).copied().unwrap_or(0) > 0,
                "the corpus must reach `{rule}`; item M's `cost-crosscore` is \
                 the case that does, and if this fires the coverage gap item G \
                 reported has re-opened: {reached:?}"
            );
        }
        apply_mode(CompileMode::Release);
    }

    #[test]
    fn null_opt_renaming_fn_keys_is_never_a_win() {
        use crate::cost::compose::{WorkloadAttach, attach_workloads};
        use crate::cost::freq::parse as parse_freq;
        use crate::cost::score::FnCost;

        let set = pinned_set();
        let freq =
            parse_freq("workload=boot-actors\nLedger.mark=3\nWorker.slow=3\nWorker.quick=2\n")
                .expect("freq");

        let base_fns = vec![
            ("Ledger.mark", 88u64),
            ("Worker.slow", 833),
            ("Worker.quick", 475),
        ];

        let build = |rename: bool| {
            let fns: Vec<FnCost> = base_fns
                .iter()
                .map(|(k, c)| FnCost {
                    key: if rename {
                        format!("{k}$fused")
                    } else {
                        (*k).to_string()
                    },
                    owner: "app".to_string(),
                    frame_bytes: 0,
                    proxy_cycles: *c,
                    words: *c,
                    terms: BTreeMap::new(),
                })
                .collect();
            let total: u64 = fns.iter().map(|f| f.proxy_cycles).sum();
            let words: u64 = fns.iter().map(|f| f.words).sum();
            let mut report = CostReport {
                version: 3,
                digest: "t".to_string(),
                provenance: "p".to_string(),
                provenance_summary: "T1=1 T2=0 T3=0 T4=0 T5=0 rows=1".to_string(),
                profile: "a76-pi5".to_string(),
                pipelines: 8,
                dispatch_mops: 4,
                dispatch_uops: 8,
                reorder_window: 128,
                total_proxy_cycles: total,
                schedule_cycles: total,
                footprint_cycles: 0,
                rank_cycles: total,
                total_words: words,
                sync_frame_max_bytes: 0,
                async_frame_total_bytes: 0,
                owner_totals: BTreeMap::new(),
                fns,
                workloads_digest: None,
                workload_totals: BTreeMap::new(),
                workload_coverage: BTreeMap::new(),
                workload_validation_bounds: BTreeMap::new(),
                footprint: Vec::new(),
            };
            attach_workloads(
                &mut report,
                &WorkloadAttach::from_parts(set.clone(), freq.clone()),
            )
            .expect("attach");
            OverallSide::from_report(&report)
        };

        let baseline = build(false);
        let candidate = build(true);
        let cmp = compare_overall(&baseline, &candidate, &set).expect("compare");
        let table = format_overall_table(&cmp, "baseline", "renamed");
        eprintln!("null-opt rename:\n{table}");
        assert!(
            !cmp.wins(),
            "a pure rename must never be a win — the gate would be \
             rewarding measuring less:\n{table}"
        );
        assert!(cmp.vetoed(), "expected a coverage veto:\n{table}");
    }

    #[ignore = "milestone lane: whole-corpus null optimization comparison"]
    #[test]
    fn null_opt_identity_is_never_a_win() {
        let set = pinned_set();
        let side = totals(&[("flat", 1234), ("boot-actors", 5678)])
            .with_coverage(cov(&[("boot-actors", 11, 11)]))
            .with_words(999);
        let cmp = compare_overall(&side, &side, &set).expect("compare");
        assert!(!cmp.wins(), "identity must not win the overall gate");
        assert_eq!(cmp.weighted_mean_rel(), Some(0.0));

        let corpus = compare_opt_lists(RELEASE_OPTS, RELEASE_OPTS);
        assert!(!corpus.wins(), "identity must not win the corpus gate");
        assert_eq!(corpus.sum_delta(), 0);
        assert_eq!(corpus.words_delta(), 0);
        apply_mode(CompileMode::Release);
    }

    #[ignore = "milestone lane: every shipped image root"]
    #[test]
    fn the_gate_scores_the_image_every_root_would_ship() {
        let cmp = compare_opt_lists(RELEASE_OPTS, RELEASE_OPTS);
        let mut image: Vec<&str> = Vec::new();
        let mut closure: Vec<&str> = Vec::new();
        for c in &cmp.cases {
            match c.scope {
                TextScope::Image => image.push(&c.name),
                TextScope::Closure => closure.push(&c.name),
            }
        }
        assert_eq!(
            image,
            vec![
                "cost-crosscore",
                "cost-icache-cliff",
                "cost-itlb-span",
                "cost-product-actors",
                "cost-product-appliance",
                "cost-product-blk",
                "cost-product-compositor",
                "cost-product-receipt",
            ],
            "the image-bearing cases changed"
        );
        assert_eq!(closure.len(), cmp.cases.len() - image.len());
        assert!(
            !closure.contains(&"cost-product-appliance"),
            "the flagship must never be ranked as a closure again"
        );
        for c in cmp.cases.iter().filter(|c| c.tier == CostTier::Product) {
            assert_eq!(c.scope, TextScope::Image, "{}", c.name);
        }
        apply_mode(CompileMode::Release);
    }

    #[ignore = "milestone lane: whole-corpus release budget"]
    #[test]
    fn release_words_are_reported_and_the_budget_is_the_live_condition() {
        let cmp = compare_opt_lists(&[], RELEASE_OPTS);
        let table = format_delta_table(&cmp, "dev", "release");
        eprintln!("corpus words + budget (dev → release):\n{table}");
        for c in &cmp.cases {
            for b in &c.baseline_budgets {
                eprintln!("{} dev  {}", c.name, b.render());
            }
            for b in &c.candidate_budgets {
                eprintln!("{} rel  {}", c.name, b.render());
            }
        }
        assert!(
            table.contains("words_b") && table.contains("chg_b"),
            "words stay a reported column beside the budget charge:\n{table}"
        );
        for c in &cmp.cases {
            assert!(
                c.budget_growth()
                    .expect("same placement both sides")
                    .is_empty(),
                "{}: release grew a per-core budget overflow\n{table}",
                c.name
            );
            let cores = match c.name.as_str() {
                "cost-crosscore" | "cost-itlb-span" => 2,
                _ => 1,
            };
            assert_eq!(
                c.baseline_budgets.len(),
                cores,
                "{}: expected {cores} core(s) of placement",
                c.name
            );
            let ships = c.scope == TextScope::Image;
            const FITS_IN_L1I: &[&str] = &["cost-product-compositor"];
            let expected_over = !FITS_IN_L1I.contains(&c.name.as_str());
            for b in c.baseline_budgets.iter().chain(c.candidate_budgets.iter()) {
                if ships {
                    assert_eq!(
                        !b.within_budget(),
                        expected_over,
                        "{}: this case's L1I verdict moved. The images this tree \
                         ships are over their 64 KiB L1I except those named in \
                         FITS_IN_L1I; a move in either direction is a result to \
                         report, not to absorb: {}",
                        c.name,
                        b.render()
                    );
                } else {
                    assert!(
                        b.within_budget(),
                        "{}: a closure that ships nothing fits its core; if that \
                         stops being true the gate's coverage claim changes: {}",
                        c.name,
                        b.render()
                    );
                }
            }
        }
    }

    #[test]
    fn corpus_gate_no_longer_refuses_word_growth() {
        let grew = CorpusCompare {
            cases: vec![CaseDelta {
                tier: CostTier::Micro,
                scope: TextScope::Closure,
                name: "cost-synthetic".to_string(),
                baseline: 1000,
                candidate: 900,
                baseline_words: 400,
                candidate_words: 500,
                baseline_budgets: vec![budget(0, 409, 0, 2863)],
                candidate_budgets: vec![budget(0, 409, 0, 2863)],
                baseline_ordering: ord(&[("barrier", 2)]),
                candidate_ordering: ord(&[("barrier", 2)]),
            }],
            baseline_sum: 1000,
            candidate_sum: 900,
            baseline_words: 400,
            candidate_words: 500,
        };
        assert_eq!(grew.words_delta(), 100, "the growth is real");
        assert!(
            grew.wins(),
            "+100 words at an unchanged budget is a priced trade now, not a \
             refusal (freeze 1626)"
        );
        let mut over = grew.clone();
        over.cases[0].candidate_budgets[0].over_l1i_lines += 1;
        assert!(
            !over.wins(),
            "the budget delta is what refuses footprint growth now"
        );
        let err = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            assert_wins(&over, "candidate", "baseline");
        }))
        .expect_err("must refuse");
        let msg = err
            .downcast_ref::<String>()
            .map(String::as_str)
            .unwrap_or("<non-string panic>")
            .to_string();
        assert!(
            msg.contains("budget_grew:core0:over_l1i_lines:409->410"),
            "the refusal must name itself, got: {msg}"
        );
    }

    #[test]
    fn no_public_per_point_win_predicate_exists() {
        let src = std::fs::read_to_string(
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src/opts/win.rs"),
        )
        .expect("read win.rs");
        let mut public_bools = Vec::new();
        for line in src.lines() {
            let t = line.trim();
            let Some(rest) = t.strip_prefix("pub fn ") else {
                continue;
            };
            if !t.ends_with("-> bool {") && !t.ends_with("-> bool") {
                continue;
            }
            let name = rest.split('(').next().unwrap_or("").to_string();
            public_bools.push(name);
        }
        public_bools.sort();
        assert_eq!(
            public_bools,
            vec![
                "is_flat".to_string(),
                "rises".to_string(),
                "vetoed".to_string(),
                "wins".to_string(),
                "wins".to_string(),
                "wins".to_string(),
                "wins".to_string(),
                "wins_in_tier".to_string(),
            ],
            "the public predicate set changed. The four `wins` are the ∀ \
             verdicts on CorpusCompare / OverallCompare / OptGateCompare / \
             SweepCompare; `vetoed`, `rises` and `is_flat` are row facts. \
             `wins_in_tier` (item H, decision 1782) is a fifth ∀ verdict and \
             not a fifth kind of predicate: its argument is a CostTier — a \
             slice of the *corpus*, fixed on disk — and it still quantifies over every \
             point of every case in that slice. Anything else — in \
             particular anything taking a SweepPoint or a PointRow and \
             answering yes/no — is the ∃ form freeze 1624 refuses."
        );
        for line in src.lines() {
            let t = line.trim();
            if t.starts_with("pub fn") && t.contains("SweepPoint") {
                assert!(
                    !t.contains("bool"),
                    "a public per-point predicate appeared: {t}"
                );
            }
        }
        assert!(
            !src.contains("impl PointRow {\n    pub fn wins"),
            "PointRow must not answer whether the candidate won here"
        );
    }

    #[test]
    fn the_residual_box_has_two_to_the_seventeen_endpoint_corners() {
        let table = load_default().expect("committed profile");
        assert_eq!(table.sweep_dimensions().len(), 17);
        assert_eq!(box_cardinality(&table), 131_072);
    }

    #[test]
    fn release_wins_at_every_box_point_on_the_smoke_case() {
        let cmp = compare_opt_lists_over_box_for_case(&[], RELEASE_OPTS, "cost-bounds-elide")
            .expect("smoke sweep");
        assert_eq!(cmp.cases.len(), 1, "the smoke lane sweeps exactly one case");
        let case = &cmp.cases[0];
        assert!(
            !case.points.is_empty(),
            "the smoke case must enumerate corners, not zero"
        );
        assert!(
            case.points.iter().all(|p| p.candidate < p.baseline),
            "release must fall at every point of {}: {:?}",
            case.name,
            case.points
                .iter()
                .map(|p| (p.baseline, p.candidate))
                .collect::<Vec<_>>()
        );
        assert!(
            cmp.wins(),
            "smoke sweep vetoed: {:?}",
            cmp.reasons.iter().map(|r| r.label()).collect::<Vec<_>>()
        );
        assert_eq!(case.box_cardinality, 131_072);
    }

    #[ignore = "deep lane: run via `cargo xtask verify-deep` (or --ignored)"]
    #[test]
    fn release_wins_at_every_point_of_the_residual_box() {
        let cmp = compare_opt_lists_over_box(&[], RELEASE_OPTS).expect("sweep");
        let table = format_sweep_table(&cmp, "dev", "release");
        eprintln!("∀ sweep (dev → release):\n{table}");
        for c in &cmp.cases {
            assert_eq!(c.box_dims, 17);
            assert_eq!(c.box_cardinality, 131_072);
            assert!(
                c.points.len() <= 1usize << c.swept.len() && !c.points.is_empty(),
                "{}: {} corners over k={}, which must be a non-empty subset of \
                 the 2^k product box",
                c.name,
                c.points.len(),
                c.swept.len()
            );
            assert!(
                table.contains(&format!("case {} box_cardinality=131072", c.name))
                    || table.contains(&c.name),
                "every case must appear in the evidence table"
            );
        }
        assert_sweep_wins(&cmp, "release", "dev");
        assert!(cmp.wins());
    }

    const ITEM_C_SMOKE: &[(&[OptId], OptId, &str)] = &[
        (&[], OptId::MaskCheck, "cost-arith"),
        (&[], OptId::BfxNarrow, "cost-runtime"),
        (
            &[OptId::NarrowImm, OptId::RegAlloc],
            OptId::WideImmForms,
            "cost-mpipe-block",
        ),
    ];

    #[ignore = "deep lane: run via `cargo xtask verify-deep` (or --ignored)"]
    #[test]
    fn item_c5_earns_its_place_by_leave_one_out_on_the_product_tier() {
        let without: Vec<OptId> = RELEASE_OPTS
            .iter()
            .copied()
            .filter(|o| *o != OptId::WideImmForms)
            .collect();
        assert_eq!(
            without.len() + 1,
            RELEASE_OPTS.len(),
            "WideImmForms must be in RELEASE_OPTS for leave-one-out to mean anything"
        );
        let cmp = compare_opt_lists_over_box_in_tier(&without, RELEASE_OPTS, CostTier::Product)
            .expect("product-tier leave-one-out sweep");
        let falls: Vec<&str> = cmp
            .cases
            .iter()
            .filter(|c| !c.points.is_empty() && c.points.iter().all(|p| p.candidate < p.baseline))
            .map(|c| c.name.as_str())
            .collect();
        eprintln!(
            "C5 leave-one-out (product tier): {} points/side, falls everywhere on {falls:?}",
            cmp.scored_points_in(CostTier::Product)
        );
        assert!(
            cmp.wins_in_tier(CostTier::Product),
            "C5 no longer earns its place in RELEASE_OPTS: {:?}. Delete it — do not \
             go looking for a baseline where it still wins.",
            cmp.reasons.iter().map(SweepVeto::label).collect::<Vec<_>>()
        );
        assert_eq!(
            falls,
            ["cost-product-blk", "cost-product-receipt"],
            "the two shipped programs C5 is justified by; re-derive rather than rescale"
        );
    }

    #[ignore = "milestone lane: whole-corpus Item C attribution"]
    #[test]
    fn item_c_attribution_over_the_corpus() {
        let rows = attribute_opts(&[
            ("dev", &[]),
            ("BfxNarrow", &[OptId::BfxNarrow]),
            ("MaskCheck", &[OptId::MaskCheck]),
            ("WideImmForms", &[OptId::WideImmForms]),
            ("+NarrowImm", &[OptId::NarrowImm]),
            ("+NI+WideImm", &[OptId::NarrowImm, OptId::WideImmForms]),
            ("release", RELEASE_OPTS),
            (
                "rel-noBfx",
                &[OptId::NarrowImm, OptId::MaskCheck, OptId::WideImmForms],
            ),
            (
                "rel-noMask",
                &[OptId::NarrowImm, OptId::BfxNarrow, OptId::WideImmForms],
            ),
            (
                "rel-noWideImm",
                &[OptId::NarrowImm, OptId::BfxNarrow, OptId::MaskCheck],
            ),
        ]);
        eprintln!("item C attribution:\n{}", format_attribution_table(&rows));
        for label in ["BfxNarrow", "MaskCheck", "+NI+WideImm"] {
            let moved = rows.iter().any(|r| {
                let dev = r.cell("dev").expect("dev cell");
                let c = r.cell(label).expect("opt cell");
                c.words != dev.words || c.proxy_cycles != dev.proxy_cycles
            });
            assert!(
                moved,
                "{label} changes nothing anywhere in the cost corpus — it has no \
                 customer, so no number in this table is about it"
            );
        }
    }

    #[test]
    fn item_c1_becomes_visible_once_the_allocator_removes_the_frame_slack() {
        let case = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../tests/golden/cost-arith-w/input.wr");
        let committed = std::fs::read_to_string(crate::cost::table::default_table_path())
            .expect("committed profile");
        const W_ROW: &str =
            "[latency.mul_w]\nlat = 2\nacc_lat = 1\nthru_num = 1\nthru_den = 1\nports = \"M\"\n";
        assert!(
            committed.contains(W_ROW),
            "the ablation must actually patch `[latency.mul_w]` — if the row's text \
             moved, fix the patch rather than letting this test pass vacuously"
        );

        let side = compile_side(&case, RELEASE_OPTS).expect("release side");
        apply_mode(CompileMode::Release);
        let rules: Vec<_> = side
            .program
            .fns
            .values()
            .flat_map(|f| f.code.iter().map(|w| w.rule))
            .collect();
        assert!(
            rules.contains(&crate::cost::rule::CostRule::MulW),
            "cost-arith-w must emit W-form multiplies or this measures nothing"
        );
        assert!(
            rules.contains(&crate::cost::rule::CostRule::Mul),
            "cost-arith-w's two X-form controls must survive"
        );

        let score_with_mul_w_at = |lat: u32, thru_den: u32, stall: u32| -> u64 {
            let mut row = format!(
                "[latency.mul_w]\nlat = {lat}\nacc_lat = 1\nthru_num = 1\nthru_den = {thru_den}\nports = \"M\"\n"
            );
            if stall > 0 {
                row.push_str(&format!("m_pipe_stall = {stall}\n"));
            }
            let patched = committed.replace(W_ROW, &row);
            let table = crate::cost::table::parse(&patched)
                .unwrap_or_else(|e| panic!("profile with mul_w lat={lat}: {e}"));
            assert_eq!(table.latency(crate::cost::rule::CostRule::MulW), lat as u64);
            let p = SweepPoint::pinned(&table);
            let ctx = ScoreCtx::new(&table).expect("ctx");
            score_side_at(&side, &table, &ctx, &p, true)
                .expect("score")
                .cycles
        };

        let w_form = score_with_mul_w_at(2, 1, 0);
        let x_form = score_with_mul_w_at(4, 3, 2);

        assert!(
            w_form < x_form,
            "item C1 has stopped being visible again: W-form {w_form} vs X-form \
             {x_form}. On the merged tree the allocator has removed the frame \
             slack that used to hide it, so W must now score strictly better."
        );
        assert_eq!(
            (w_form, x_form),
            (42, 48),
            "the measured size of C1's win once item E removed the frame slack, \
             item I coalesced the allocator's copies and item J's GVN removed \
             the redundancy under both forms; re-measure this rather than \
             rescaling it"
        );

        let mut threshold = None;
        for lat in 5..=40u32 {
            let inflated = score_with_mul_w_at(lat, 3, 2);
            if inflated != w_form {
                threshold = Some((lat, inflated));
                break;
            }
        }
        let (lat, inflated) = threshold.expect(
            "no multiply latency up to 40 changed the total — the multiply term is \
             inert in the model, which would make C1's zero meaningless rather than \
             informative",
        );
        assert!(inflated > w_form, "the slower row must score higher");
        eprintln!(
            "C1 ablation on cost-arith-w: the emitted W-form words score {w_form} at the \
             committed `[latency.mul_w]` against {x_form} when priced at the X-form row \
             they replaced — the substitution is worth {} cycles here. On item C's own \
             tree it was worth exactly 0, because the spill-everything frame donated 6 \
             cycles of M-pipe slack per multiply against the 2 the substitution saves; \
             item E's allocator removed that slack. Residual slack is now {} cycle(s): \
             the counterfactual X-form row first moves the total again at lat = {lat} \
             ({inflated} cycles).",
            x_form - w_form,
            lat - 4
        );
    }

    #[test]
    fn each_item_c_opt_wins_at_every_box_point_on_its_smoke_case() {
        for &(base, id, case) in ITEM_C_SMOKE {
            let mut candidate = base.to_vec();
            candidate.push(id);
            let (sweep, reasons) = sweep_one(case, base, &candidate);
            let labels: Vec<String> = reasons.iter().map(SweepVeto::label).collect();
            assert!(
                labels.is_empty(),
                "{id:?} refused on {case}: {}",
                labels.join(" ")
            );
            assert!(
                !sweep.points.is_empty(),
                "{id:?} on {case}: the probe enumerated no corners"
            );
            assert!(
                sweep.points.iter().all(|p| p.candidate <= p.baseline),
                "{id:?} must not grow over baseline {base:?} on {case}; got {:?}",
                sweep
                    .points
                    .iter()
                    .map(|p| (p.baseline, p.candidate))
                    .collect::<Vec<_>>()
            );
            eprintln!(
                "item C smoke: {id:?} over {base:?} on {case} — {} corners, {} → {} at the first",
                sweep.points.len(),
                sweep.points[0].baseline,
                sweep.points[0].candidate
            );
        }
    }

    #[ignore = "deep lane: run via `cargo xtask verify-deep` (or --ignored)"]
    #[test]
    fn each_item_c_opt_wins_over_the_micro_box_alone() {
        for &(base, id, _) in ITEM_C_SMOKE {
            let mut candidate = base.to_vec();
            candidate.push(id);
            let cmp = compare_opt_lists_over_box_in_tier(base, &candidate, CostTier::Micro)
                .expect("micro sweep");
            eprintln!(
                "item C ∀ micro ({id:?} over {base:?}):\n{}",
                format_sweep_table(&cmp, &format!("{base:?}"), &format!("+{id:?}"))
            );
            assert_sweep_wins(&cmp, &format!("+{id:?}"), &format!("{base:?}"));
        }
    }

    const WITHOUT_REGALLOC: &[OptId] = &[OptId::NarrowImm];

    #[test]
    fn regalloc_wins_at_every_box_point_on_the_smoke_case() {
        let cmp =
            compare_opt_lists_over_box_for_case(WITHOUT_REGALLOC, RELEASE_OPTS, "cost-branch-bias")
                .expect("smoke sweep");
        assert_eq!(cmp.cases.len(), 1, "the smoke lane sweeps exactly one case");
        let case = &cmp.cases[0];
        assert!(
            !case.points.is_empty(),
            "the smoke case must enumerate corners, not zero"
        );
        assert!(
            case.points.iter().all(|p| p.candidate < p.baseline),
            "RegAlloc must fall at every point of {}: {:?}",
            case.name,
            case.points
                .iter()
                .map(|p| (p.baseline, p.candidate))
                .collect::<Vec<_>>()
        );
        assert!(
            cmp.wins(),
            "smoke sweep vetoed: {:?}",
            cmp.reasons.iter().map(|r| r.label()).collect::<Vec<_>>()
        );
    }

    fn item_f_baseline() -> Vec<OptId> {
        RELEASE_OPTS
            .iter()
            .copied()
            .take_while(|o| *o != OptId::InterprocRegs)
            .collect()
    }

    const ITEM_F_SMOKE: &[(OptId, &str)] = &[
        (OptId::InterprocRegs, "cost-runtime"),
        (OptId::Frameless, "cost-arith"),
    ];

    #[test]
    fn each_item_f_opt_wins_at_every_box_point_on_its_smoke_case() {
        let mut base = item_f_baseline();
        for &(id, case) in ITEM_F_SMOKE {
            let mut candidate = base.clone();
            candidate.push(id);
            let (sweep, reasons) = sweep_one(case, &base, &candidate);
            let labels: Vec<String> = reasons.iter().map(SweepVeto::label).collect();
            assert!(
                labels.is_empty(),
                "{id:?} refused on {case}: {}",
                labels.join(" ")
            );
            assert!(
                !sweep.points.is_empty(),
                "{id:?} on {case}: the probe enumerated no corners"
            );
            assert!(
                sweep.points.iter().all(|p| p.candidate < p.baseline),
                "{id:?} must fall at every point of {case}; got {:?}",
                sweep
                    .points
                    .iter()
                    .map(|p| (p.baseline, p.candidate))
                    .collect::<Vec<_>>()
            );
            eprintln!(
                "item F smoke: {id:?} on {case} — {} corners, {} -> {} at the first",
                sweep.points.len(),
                sweep.points[0].baseline,
                sweep.points[0].candidate
            );
            base = candidate;
        }
    }

    #[ignore = "deep lane: run via `cargo xtask verify-deep` (or --ignored)"]
    #[test]
    fn each_item_f_opt_wins_over_the_micro_box_alone() {
        let mut base = item_f_baseline();
        for &(id, _) in ITEM_F_SMOKE {
            let mut candidate = base.clone();
            candidate.push(id);
            let cmp = compare_opt_lists_over_box_in_tier(&base, &candidate, CostTier::Micro)
                .expect("micro sweep");
            eprintln!(
                "item F ∀ micro (+{id:?} over {} opts):\n{}",
                base.len(),
                format_sweep_table(&cmp, "base", &format!("+{id:?}"))
            );
            assert_sweep_wins(&cmp, &format!("+{id:?}"), "base");
            base = candidate;
        }
    }

    fn without_branch_cleanup() -> Vec<OptId> {
        RELEASE_OPTS
            .iter()
            .copied()
            .filter(|o| *o != OptId::BranchCleanup)
            .collect()
    }

    #[test]
    fn branch_cleanup_wins_at_every_box_point_on_the_smoke_case() {
        let base = without_branch_cleanup();
        let cmp = compare_opt_lists_over_box_for_case(&base, RELEASE_OPTS, "cost-arith-w")
            .expect("smoke sweep");
        assert_eq!(cmp.cases.len(), 1, "the smoke lane sweeps exactly one case");
        let case = &cmp.cases[0];
        assert!(
            !case.points.is_empty(),
            "the smoke case must enumerate corners, not zero"
        );
        assert!(
            case.points.iter().all(|p| p.candidate < p.baseline),
            "BranchCleanup must fall at every point of {}: {:?}",
            case.name,
            case.points
                .iter()
                .map(|p| (p.baseline, p.candidate))
                .collect::<Vec<_>>()
        );
        assert!(
            cmp.wins(),
            "smoke sweep vetoed: {:?}",
            cmp.reasons.iter().map(|r| r.label()).collect::<Vec<_>>()
        );
    }

    #[ignore = "deep lane: run via `cargo xtask verify-deep` (or --ignored)"]
    #[test]
    fn branch_cleanup_wins_at_every_point_of_the_residual_box() {
        let base = without_branch_cleanup();
        let cmp = compare_opt_lists_over_box(&base, RELEASE_OPTS).expect("sweep");
        let table = format_sweep_table(&cmp, "release−BranchCleanup", "release");
        eprintln!("∀ sweep (release−BranchCleanup → release):\n{table}");
        assert_sweep_wins(&cmp, "release", "release−BranchCleanup");
        assert!(cmp.wins());
        for tier in CostTier::ALL {
            assert!(
                cmp.wins_in_tier(tier),
                "BranchCleanup must win on the {tier} tier alone: {:?}",
                cmp.reasons_for_tier(tier)
                    .iter()
                    .map(|r| r.label())
                    .collect::<Vec<_>>()
            );
        }
        eprintln!(
            "BranchCleanup ∀-sweep: {} points/side over {} cases",
            cmp.scored_points(),
            cmp.cases.len()
        );
    }

    #[test]
    fn f5_is_an_opt_id_now_that_the_corpus_fires_tail_calls() {
        assert!(
            RELEASE_OPTS.contains(&OptId::TailCalls),
            "F5 is rankable, so it is gated like everything else"
        );
        apply_opts(&[]);
        assert!(
            !crate::codegen::tail_calls(),
            "`dev` must keep BL+RET as the reference form"
        );
        apply_mode(CompileMode::Release);
    }

    #[ignore = "deep lane: run via `cargo xtask verify-deep` (or --ignored)"]
    #[test]
    fn the_corpus_still_fires_tail_calls() {
        let mut fired = Vec::new();
        for case in discover_cost_cases() {
            let side = compile_side(case.input.as_path(), RELEASE_OPTS).expect("release side");
            let tails = side
                .program
                .fns
                .values()
                .flat_map(|f| f.code.iter())
                .filter(|w| w.text.ends_with("; tail call"))
                .count();
            if tails > 0 {
                fired.push((case.name.clone(), tails));
            }
        }
        apply_mode(CompileMode::Release);
        assert!(
            !fired.is_empty(),
            "no cost-corpus case fires a tail call any more — F5's gate is \
             measuring nothing and the opt should be re-examined"
        );
        eprintln!("tail-call sites by case: {fired:?}");
    }

    fn sweep_one(
        case: &str,
        baseline: &[OptId],
        candidate: &[OptId],
    ) -> (CaseSweep, Vec<SweepVeto>) {
        let table = load_default().expect("committed profile");
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join(format!("../../tests/golden/{case}/input.wr"));
        let b = compile_side(&path, baseline).expect("baseline side");
        let c = compile_side(&path, candidate).expect("candidate side");
        apply_mode(CompileMode::Release);
        let mut reasons = Vec::new();
        let sweep = sweep_case(case, CostTier::Micro, &b, &c, &table, &mut reasons).expect("sweep");
        (sweep, reasons)
    }

    #[test]
    fn a_candidate_that_wins_at_one_point_and_loses_at_another_is_vetoed_with_the_point_named() {
        let table = load_default().expect("committed profile");
        let dims = ["l2_latency", "l3_latency"];
        let corners = endpoint_corners(&table, &dims);
        let flatter = corners[0].label_over(&dims);
        let harsher = corners[3].label_over(&dims);
        assert_ne!(flatter, harsher);

        let score = |cycles: u64| SideScore {
            cycles,
            words: 400,
            budgets: Vec::new(),
            ordering: Some(BTreeMap::new()),
        };
        let mut reasons = Vec::new();
        let row = refuse_at_point(
            "cost-flip",
            &flatter,
            &score(1000),
            &score(900),
            true,
            &mut reasons,
        )
        .expect("row");
        assert_eq!(row.delta(), -100);
        assert!(
            reasons.is_empty(),
            "a point the candidate wins at fires nothing: {reasons:?}"
        );
        let row = refuse_at_point(
            "cost-flip",
            &harsher,
            &score(1000),
            &score(1100),
            false,
            &mut reasons,
        )
        .expect("row");
        assert_eq!(row.delta(), 100);
        assert_eq!(
            reasons.iter().map(SweepVeto::label).collect::<Vec<_>>(),
            vec![format!("case_rose:cost-flip:1000->1100@[{harsher}]")],
            "the flip must be refused and must name the flipping point"
        );
        let cmp = SweepCompare {
            table_digest: table.table_digest(),
            cases: Vec::new(),
            reasons,
        };
        assert!(!cmp.wins());
        assert!(format_sweep_table(&cmp, "base", "cand").contains("outcome=veto"));
    }

    #[test]
    fn the_sensitivity_probe_holds_only_unread_dimensions_and_reports_them() {
        let (case, _) = sweep_one("cost-arith", &[], RELEASE_OPTS);
        let cmp = SweepCompare {
            table_digest: String::new(),
            cases: vec![case],
            reasons: Vec::new(),
        };
        let table = format_sweep_table(&cmp, "dev", "release");
        let all: Vec<String> = load_default()
            .expect("table")
            .sweep_dimensions()
            .into_iter()
            .map(str::to_string)
            .collect();
        for c in &cmp.cases {
            let mut seen: Vec<String> = c.swept.clone();
            seen.extend(c.held.iter().map(|h| h.dim.clone()));
            seen.sort();
            assert_eq!(seen, all, "{}: a dimension went missing", c.name);
            assert!(
                c.swept.len() <= MAX_SWEPT_DIMS,
                "{}: {} dims survive, over the fail-closed bound",
                c.name,
                c.swept.len()
            );
            for h in &c.held {
                assert!(
                    table.contains(&h.label()),
                    "{}: held dimension {} is not reported",
                    c.name,
                    h.dim
                );
            }
            for d in &c.read_but_static {
                assert!(
                    c.swept.contains(d),
                    "{}: `{d}` was read but not swept",
                    c.name
                );
            }
        }
        eprintln!("probe report:\n{table}");
    }

    #[test]
    fn a_held_dimension_moves_nothing_at_any_extreme_corner() {
        let table = load_default().expect("table");
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../tests/golden/cost-arith/input.wr");
        let dev = compile_side(&path, &[]).expect("dev side");
        let rel = compile_side(&path, RELEASE_OPTS).expect("release side");
        apply_mode(CompileMode::Release);
        let probe = probe_case("cost-arith", &dev, &rel, &table).expect("probe");
        assert!(!probe.held.is_empty(), "expected some inert dimension");
        let dims: Vec<String> = table
            .sweep_dimensions()
            .into_iter()
            .map(str::to_string)
            .collect();
        let pinned = SweepPoint::pinned(&table);
        let mut all_lo = pinned.clone();
        let mut all_hi = pinned.clone();
        for d in &dims {
            let r = table.sweep(d).expect("row");
            all_lo = all_lo.with(d, r.lo);
            all_hi = all_hi.with(d, r.hi);
        }
        for h in &probe.held {
            for base in [&pinned, &all_lo, &all_hi] {
                for side in [&dev, &rel] {
                    let ctx = ScoreCtx::new(&table).expect("ctx");
                    let lo = score_side_at(side, &table, &ctx, &base.with(&h.dim, h.lo), true)
                        .expect("lo");
                    let hi = score_side_at(side, &table, &ctx, &base.with(&h.dim, h.hi), true)
                        .expect("hi");
                    assert_eq!(lo, hi, "held dimension `{}` moved a score", h.dim);
                }
            }
        }
    }

    #[test]
    fn the_sweep_still_refuses_barrier_removal() {
        let mut reasons = Vec::new();
        let base = ord(&[
            ("barrier", 6),
            ("load_acquire", 4),
            ("store_release", 6),
            ("system", 1),
        ]);
        let mut fewer = base.clone();
        *fewer.get_mut(&("f".to_string(), "barrier")).unwrap() -= 1;
        for r in ordering_removals(&base, &fewer) {
            reasons.push(SweepVeto::OrderingWordsRemoved {
                case: "cost-crosscore".to_string(),
                rule: r.rule,
                baseline: r.baseline,
                candidate: r.candidate,
            });
        }
        let cmp = SweepCompare {
            table_digest: "t".to_string(),
            cases: Vec::new(),
            reasons,
        };
        assert!(
            !cmp.wins(),
            "a deleted DMB is refused on the swept gate too"
        );
        assert_eq!(
            cmp.reasons.iter().map(SweepVeto::label).collect::<Vec<_>>(),
            vec!["ordering_words_removed:cost-crosscore:barrier:6->5".to_string()]
        );
        let removed = CaseDelta {
            scope: TextScope::Closure,
            tier: CostTier::Micro,
            name: "cost-crosscore".to_string(),
            baseline: 1000,
            candidate: 900,
            baseline_words: 400,
            candidate_words: 500,
            baseline_budgets: Vec::new(),
            candidate_budgets: Vec::new(),
            baseline_ordering: base,
            candidate_ordering: fewer,
        };
        assert_eq!(removed.ordering_removed().len(), 1);
        let cmp = CorpusCompare {
            cases: vec![removed],
            baseline_sum: 1000,
            candidate_sum: 900,
            baseline_words: 400,
            candidate_words: 500,
        };
        assert!(
            !cmp.wins(),
            "freeze 1633 must outlive the words veto it never depended on"
        );
    }

    #[test]
    fn too_many_surviving_dimensions_is_an_error_not_a_truncation() {
        assert!(
            MAX_SWEPT_DIMS < 17,
            "the bound must actually bound the declared box"
        );
        let table = load_default().expect("profile");
        let path = discover_cost_cases()
            .into_iter()
            .find(|c| c.name == "cost-bounds-elide")
            .expect("cost-bounds-elide must exist")
            .input;
        let b = compile_side(&path, &[]).expect("baseline");
        let c = compile_side(&path, RELEASE_OPTS).expect("candidate");

        let err = probe_case_bounded("cost-bounds-elide", &b, &c, &table, 0)
            .expect_err("a bound the case exceeds must refuse");
        assert!(
            err.contains("survive the sensitivity probe")
                && err.contains("over the bound of 0")
                && err.contains("rather than truncating"),
            "the refusal must name the bound and say it is not a truncation: {err}"
        );

        let ok = probe_case_bounded("cost-bounds-elide", &b, &c, &table, MAX_SWEPT_DIMS)
            .expect("the smoke case must fit the committed bound");
        assert!(!ok.swept.is_empty() && ok.swept.len() <= MAX_SWEPT_DIMS);
        assert!(
            probe_case_bounded("cost-bounds-elide", &b, &c, &table, ok.swept.len() - 1).is_err(),
            "one dimension under the surviving count must still refuse"
        );
        apply_mode(CompileMode::Release);
    }
}

#[cfg(test)]
mod frameless_disposition {
    use super::*;
    use crate::opts::{OptId, RELEASE_OPTS};

    #[test]
    fn frameless_on_the_compute_workload() {
        let base: Vec<OptId> = RELEASE_OPTS
            .iter()
            .copied()
            .filter(|o| *o != OptId::Frameless)
            .collect();
        let cand: Vec<OptId> = RELEASE_OPTS.to_vec();
        assert_eq!(
            base.len() + 1,
            cand.len(),
            "Frameless must be in RELEASE_OPTS for this to be the marginal question"
        );

        let cmp = compare_opt_lists_over_box_for_case(&base, &cand, "cost-product-compositor")
            .expect("compositor sweep");
        let case = &cmp.cases[0];
        let rose: Vec<(u64, u64)> = case
            .points
            .iter()
            .filter(|p| p.candidate > p.baseline)
            .map(|p| (p.baseline, p.candidate))
            .collect();
        let fell = case
            .points
            .iter()
            .filter(|p| p.candidate < p.baseline)
            .count();
        eprintln!(
            "Frameless on cost-product-compositor: {} point(s), {fell} fell, {} rose {rose:?}",
            case.points.len(),
            rose.len()
        );
        assert!(
            rose.is_empty(),
            "Frameless rises again on the compute workload — this is what \
             parked it once already (decision 1918); park it again rather \
             than shipping a case that rises: {rose:?}"
        );
        assert_eq!(
            fell,
            case.points.len(),
            "it must fall at *every* point, not merely never rise"
        );
    }
}
