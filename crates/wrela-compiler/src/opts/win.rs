//! Corpus proxy win oracle (plans/M19.md item E / decisions 1450–1453)
//! plus multi-workload overall veto-then-rank (Phase 2 item K).
//!
//! Discover every `tests/golden/cost-*/input.wr` (sorted), score under
//! two opt-list configs by `total_proxy_cycles` only, and assert the
//! freeze-1403 win rule: candidate must not raise any case and must
//! strictly lower at least one.
//!
//! Scoring uses the same emit path as `wrela dump --stage=cost`
//! (`cost::stage`) so force-rooted `core.runtime` is in the totals when
//! a case is runtime-bearing — the surface opts are gated on.
//!
//! Overall compare (item K): given per-workload proxy totals for baseline
//! vs candidate and pinned weights from `bench/workloads.toml`, **veto**
//! if any non-`flat` workload rises (ε=0), else **rank** by the weighted
//! mean of relative deltas `(cand−base)/base`. Until CostReport multi-W
//! compose lands (item J), callers pass a `BTreeMap` (flat-only or
//! stubbed measured rows).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use crate::cost::stage::score_cost_stage_path;
use crate::cost::workload::{self, FLAT_NAME, WorkloadSet};

use super::{CompileMode, OptId, RELEASE_OPTS, apply_mode, apply_opts};

/// One cost-* case scored under baseline vs candidate.
#[derive(Debug, Clone)]
pub struct CaseDelta {
    pub name: String,
    pub baseline: u64,
    pub candidate: u64,
}

impl CaseDelta {
    pub fn delta(&self) -> i64 {
        self.candidate as i64 - self.baseline as i64
    }
}

/// Full corpus comparison result (per-case + sums).
#[derive(Debug, Clone)]
pub struct CorpusCompare {
    pub cases: Vec<CaseDelta>,
    pub baseline_sum: u64,
    pub candidate_sum: u64,
}

impl CorpusCompare {
    pub fn sum_delta(&self) -> i64 {
        self.candidate_sum as i64 - self.baseline_sum as i64
    }

    /// True when no case rises and at least one strictly falls.
    pub fn wins(&self) -> bool {
        let mut any_fall = false;
        for c in &self.cases {
            if c.candidate > c.baseline {
                return false;
            }
            if c.candidate < c.baseline {
                any_fall = true;
            }
        }
        any_fall
    }
}

/// Deterministic discovery: all `tests/golden/cost-*` dirs that contain
/// `input.wr`, sorted by path (decision 1450).
pub fn discover_cost_corpus() -> Vec<PathBuf> {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tests/golden");
    let entries = std::fs::read_dir(&root).unwrap_or_else(|e| {
        panic!("read {}: {e}", root.display());
    });
    let mut paths = Vec::new();
    for entry in entries {
        let entry = entry.expect("read_dir entry");
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if !name.starts_with("cost-") {
            continue;
        }
        let input = path.join("input.wr");
        if input.is_file() {
            paths.push(input);
        }
    }
    paths.sort();
    paths
}

/// Lower+codegen+score `path` under an explicit opt list, via the
/// dump `--stage=cost` pipeline (force-roots included when relevant).
pub fn score_path_under_opts(path: &Path, opts: &[OptId]) -> u64 {
    apply_opts(opts);
    score_cost_stage_path(path).unwrap_or_else(|e| {
        panic!("cost-stage score {}: {e}", path.display());
    })
}

/// Score every cost-* case under `baseline` vs `candidate` opt lists
/// (decision 1451–1452). Restores `CompileMode::Release` afterward.
pub fn compare_opt_lists(baseline: &[OptId], candidate: &[OptId]) -> CorpusCompare {
    let corpus = discover_cost_corpus();
    assert!(
        !corpus.is_empty(),
        "cost corpus empty: expected tests/golden/cost-*/input.wr"
    );

    let mut cases = Vec::with_capacity(corpus.len());
    let mut baseline_sum = 0u64;
    let mut candidate_sum = 0u64;

    for path in &corpus {
        let name = case_name(path);
        let b = score_path_under_opts(path, baseline);
        let c = score_path_under_opts(path, candidate);
        baseline_sum = baseline_sum.saturating_add(b);
        candidate_sum = candidate_sum.saturating_add(c);
        cases.push(CaseDelta {
            name,
            baseline: b,
            candidate: c,
        });
    }

    apply_mode(CompileMode::Release);
    CorpusCompare {
        cases,
        baseline_sum,
        candidate_sum,
    }
}

/// Freeze-1403 oracle: `RELEASE_OPTS` vs empty (`dev`). Panics with the
/// per-case table if any case rises or none falls.
pub fn assert_release_wins_cost_corpus() -> CorpusCompare {
    let cmp = compare_opt_lists(&[], RELEASE_OPTS);
    assert_wins(&cmp, "release", "dev");
    cmp
}

/// Candidate helper: `candidate` must win vs `baseline` the same way
/// (decision 1452).
pub fn assert_candidate_wins(baseline: &[OptId], candidate: &[OptId]) -> CorpusCompare {
    let cmp = compare_opt_lists(baseline, candidate);
    assert_wins(&cmp, "candidate", "baseline");
    cmp
}

fn assert_wins(cmp: &CorpusCompare, cand_label: &str, base_label: &str) {
    let table = format_delta_table(cmp, base_label, cand_label);
    let mut rose = Vec::new();
    let mut any_fall = false;
    for c in &cmp.cases {
        if c.candidate > c.baseline {
            rose.push(format!(
                "{}: {cand_label} {} > {base_label} {}",
                c.name, c.candidate, c.baseline
            ));
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
        any_fall,
        "{cand_label} must strictly lower at least one cost-* case vs {base_label}\n{table}"
    );
}

/// Stable text table for item L's evidence block.
pub fn format_delta_table(cmp: &CorpusCompare, base_label: &str, cand_label: &str) -> String {
    let mut out = String::new();
    out.push_str(&format!(
        "{:<22} {:>12} {:>12} {:>10}\n",
        "case", base_label, cand_label, "Δ"
    ));
    for c in &cmp.cases {
        out.push_str(&format!(
            "{:<22} {:>12} {:>12} {:>+10}\n",
            c.name,
            c.baseline,
            c.candidate,
            c.delta()
        ));
    }
    out.push_str(&format!(
        "{:<22} {:>12} {:>12} {:>+10}\n",
        "SUM",
        cmp.baseline_sum,
        cmp.candidate_sum,
        cmp.sum_delta()
    ));
    out
}

// ---------------------------------------------------------------------------
// Overall veto-then-rank (Phase 2 item K)
// ---------------------------------------------------------------------------

/// One pinned workload's baseline vs candidate proxy totals.
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

    /// Relative delta `(cand − base) / base`. Both zero → `0.0`; base zero
    /// and cand positive → `+∞` (a rise; non-flat already vetoed).
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

    /// ε=0 rise: candidate strictly greater than baseline.
    pub fn rises(&self) -> bool {
        self.candidate > self.baseline
    }
}

/// Outcome of overall compare over the pinned workload set.
#[derive(Debug, Clone)]
pub enum OverallOutcome {
    /// At least one non-`flat` workload rose (ε=0).
    Veto { risen: Vec<String> },
    /// No non-flat rise; `weighted_mean_rel` is Σ(w·rel)/Σ(w).
    Rank { weighted_mean_rel: f64 },
}

/// Full overall comparison (per-W rows + veto/rank outcome).
#[derive(Debug, Clone)]
pub struct OverallCompare {
    pub workloads_digest: String,
    pub workloads: Vec<WorkloadDelta>,
    pub outcome: OverallOutcome,
}

impl OverallCompare {
    pub fn vetoed(&self) -> bool {
        matches!(self.outcome, OverallOutcome::Veto { .. })
    }

    /// Weighted mean of relative deltas when not vetoed.
    pub fn weighted_mean_rel(&self) -> Option<f64> {
        match self.outcome {
            OverallOutcome::Rank {
                weighted_mean_rel,
            } => Some(weighted_mean_rel),
            OverallOutcome::Veto { .. } => None,
        }
    }

    /// Win: not vetoed and weighted mean of relative deltas is strictly
    /// negative (overall proxy improvement under pinned weights).
    pub fn wins(&self) -> bool {
        match self.outcome {
            OverallOutcome::Veto { .. } => false,
            OverallOutcome::Rank {
                weighted_mean_rel,
            } => weighted_mean_rel < 0.0,
        }
    }
}

/// Build a per-W totals map with only the `flat` row (pre-J shape).
pub fn flat_only_totals(flat_proxy_cycles: u64) -> BTreeMap<String, u64> {
    let mut m = BTreeMap::new();
    m.insert(FLAT_NAME.to_string(), flat_proxy_cycles);
    m
}

/// Stub every pinned workload with the same total (stand-in until item J
/// supplies measured `f×s` rows; useful for plumbing tests).
pub fn stub_all_workload_totals(
    proxy_cycles: u64,
    set: &WorkloadSet,
) -> BTreeMap<String, u64> {
    set.names()
        .map(|n| (n.to_string(), proxy_cycles))
        .collect()
}

/// Load pinned weights from the committed `bench/workloads.toml`.
pub fn load_pinned_workloads() -> Result<WorkloadSet, String> {
    workload::load_default()
}

/// Compare candidate vs baseline per-W totals under `weights`.
///
/// Fail closed if any pinned name is missing from either map. Extra keys
/// in the maps (not in the pinned set) are ignored. Veto when any
/// non-`flat` workload rises (ε=0); otherwise rank by weighted mean of
/// relative deltas.
pub fn compare_overall(
    baseline: &BTreeMap<String, u64>,
    candidate: &BTreeMap<String, u64>,
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
        let b = baseline.get(name).copied().ok_or_else(|| {
            format!("overall: baseline missing workload `{name}`")
        })?;
        let c = candidate.get(name).copied().ok_or_else(|| {
            format!("overall: candidate missing workload `{name}`")
        })?;
        rows.push(WorkloadDelta {
            name: name.to_string(),
            weight,
            baseline: b,
            candidate: c,
        });
    }

    let mut risen = Vec::new();
    for r in &rows {
        if !r.is_flat() && r.rises() {
            risen.push(r.name.clone());
        }
    }

    let outcome = if !risen.is_empty() {
        OverallOutcome::Veto { risen }
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
        outcome,
    })
}

/// Stable per-W evidence table (printed under `--nocapture`).
pub fn format_overall_table(cmp: &OverallCompare, base_label: &str, cand_label: &str) -> String {
    let mut out = String::new();
    out.push_str(&format!(
        "workloads_digest={}\n",
        cmp.workloads_digest
    ));
    out.push_str(&format!(
        "{:<16} {:>8} {:>12} {:>12} {:>10} {:>12}\n",
        "workload", "weight", base_label, cand_label, "Δ", "rel"
    ));
    for r in &cmp.workloads {
        let rel = r.relative_delta();
        let rel_s = if rel.is_infinite() {
            "inf".to_string()
        } else {
            format!("{rel:+.6}")
        };
        out.push_str(&format!(
            "{:<16} {:>8} {:>12} {:>12} {:>+10} {:>12}\n",
            r.name,
            r.weight,
            r.baseline,
            r.candidate,
            r.delta(),
            rel_s
        ));
    }
    match &cmp.outcome {
        OverallOutcome::Veto { risen } => {
            out.push_str(&format!(
                "outcome=veto risen={}\n",
                risen.join(",")
            ));
        }
        OverallOutcome::Rank {
            weighted_mean_rel,
        } => {
            out.push_str(&format!(
                "outcome=rank weighted_mean_rel={weighted_mean_rel:+.6} wins={}\n",
                cmp.wins()
            ));
        }
    }
    out
}

/// Assert overall win (not vetoed, weighted mean rel < 0); panics with table.
pub fn assert_overall_wins(cmp: &OverallCompare, cand_label: &str, base_label: &str) {
    let table = format_overall_table(cmp, base_label, cand_label);
    if let OverallOutcome::Veto { risen } = &cmp.outcome {
        panic!(
            "{cand_label} vetoed: non-flat workload(s) rose vs {base_label}: {}\n{table}",
            risen.join(", "),
        );
    }
    let mean = cmp.weighted_mean_rel().expect("rank outcome");
    assert!(
        mean < 0.0,
        "{cand_label} overall weighted_mean_rel={mean:+.6} must be < 0 vs {base_label}\n{table}"
    );
}

fn case_name(input: &Path) -> String {
    input
        .parent()
        .and_then(|p| p.file_name())
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| input.display().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::opts::OptId;

    #[test]
    fn discover_cost_corpus_is_sorted_cost_star() {
        let paths = discover_cost_corpus();
        assert!(
            paths.len() >= 4,
            "expected ≥4 cost-* goldens, got {}",
            paths.len()
        );
        let names: Vec<String> = paths.iter().map(|p| case_name(p)).collect();
        let mut sorted = names.clone();
        sorted.sort();
        assert_eq!(names, sorted, "corpus paths must be sorted");
        for n in &names {
            assert!(n.starts_with("cost-"), "unexpected case {n}");
        }
        assert!(names.iter().any(|n| n == "cost-bounds-elide"));
        assert!(names.iter().any(|n| n == "cost-calls"));
    }

    /// plans/M19.md item E / decisions 1450–1451: release vs dev on the
    /// full cost-* corpus (dump `--stage=cost` pipeline).
    #[test]
    fn assert_release_wins_cost_corpus_oracle() {
        let cmp = assert_release_wins_cost_corpus();
        let table = format_delta_table(&cmp, "dev", "release");
        eprintln!("corpus proxy win (dev → release):\n{table}");
        // Stable shape for item L: every case + SUM row.
        for c in &cmp.cases {
            assert!(table.contains(&c.name), "table missing case {}", c.name);
        }
        assert!(table.contains("SUM"));
        assert!(cmp.sum_delta() < 0, "corpus sum must fall under release");
        // Runtime-bearing case must see force-rooted runtime (not the
        // thin 6-cycle probe-only path).
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

    /// Decision 1453: BoundsElide alone wins on cost-bounds-elide.
    #[test]
    fn bounds_elide_alone_wins_cost_bounds_elide() {
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../tests/golden/cost-bounds-elide/input.wr");
        let dev = score_path_under_opts(&path, &[]);
        let alone = score_path_under_opts(&path, &[OptId::BoundsElide]);
        apply_mode(CompileMode::Release);
        assert!(
            alone < dev,
            "BoundsElide alone {} must beat dev {} on cost-bounds-elide",
            alone,
            dev
        );
    }

    /// Decision 1453: NarrowImm alone wins on at least one cost-* case.
    #[test]
    fn narrow_imm_alone_wins_some_cost_case() {
        let corpus = discover_cost_corpus();
        let mut wins = Vec::new();
        for path in &corpus {
            let name = case_name(path);
            let dev = score_path_under_opts(path, &[]);
            let alone = score_path_under_opts(path, &[OptId::NarrowImm]);
            if alone < dev {
                wins.push(format!("{name}: {alone} < {dev}"));
            }
        }
        apply_mode(CompileMode::Release);
        assert!(
            !wins.is_empty(),
            "NarrowImm alone must strictly lower ≥1 cost-* case; none fell"
        );
        eprintln!("NarrowImm alone wins:\n{}", wins.join("\n"));
    }

    /// Decision 1453: swapped opt-list order vs RELEASE_OPTS — document
    /// independence when totals match (lower vs codegen axes).
    #[test]
    fn swapped_order_scores_same_as_release_opts() {
        const SWAPPED: &[OptId] = &[OptId::NarrowImm, OptId::BoundsElide];
        let cmp = compare_opt_lists(RELEASE_OPTS, SWAPPED);
        let table = format_delta_table(&cmp, "RELEASE_OPTS", "swapped");
        eprintln!("order swap note:\n{table}");
        // Both axes are independent TLS knobs; enabling both before
        // lower+codegen yields identical scores regardless of slice order.
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

    /// Corrupting the candidate (no opts) fails the win rule.
    #[test]
    #[should_panic(expected = "must strictly lower at least one")]
    fn empty_candidate_fails_win_oracle() {
        let _ = assert_candidate_wins(&[], &[]);
    }

    /// Disabling a winning opt (drop BoundsElide) as candidate vs current
    /// release raises cost-bounds-elide — oracle fails.
    #[test]
    #[should_panic(expected = "raised proxy total")]
    fn disabling_bounds_elide_fails_candidate_oracle() {
        // baseline = full release; candidate = NarrowImm only → elide off
        // raises the bounds-elide case relative to release.
        let _ = assert_candidate_wins(RELEASE_OPTS, &[OptId::NarrowImm]);
    }

    // -----------------------------------------------------------------------
    // Overall veto-then-rank (Phase 2 item K)
    // -----------------------------------------------------------------------

    fn pinned_set() -> WorkloadSet {
        load_pinned_workloads().expect("load bench/workloads.toml")
    }

    fn totals(pairs: &[(&str, u64)]) -> BTreeMap<String, u64> {
        pairs
            .iter()
            .map(|(n, v)| ((*n).to_string(), *v))
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
        // flat improves; boot-actors rises → veto (ε=0).
        let baseline = totals(&[("flat", 1000), ("boot-actors", 5000)]);
        let candidate = totals(&[("flat", 800), ("boot-actors", 5001)]);
        let cmp = compare_overall(&baseline, &candidate, &set).expect("compare");
        let table = format_overall_table(&cmp, "baseline", "candidate");
        eprintln!("overall veto case:\n{table}");
        assert!(cmp.vetoed(), "must veto when boot-actors rises");
        assert!(!cmp.wins());
        match &cmp.outcome {
            OverallOutcome::Veto { risen } => {
                assert_eq!(risen, &vec!["boot-actors".to_string()]);
            }
            OverallOutcome::Rank { .. } => panic!("expected veto, got rank"),
        }
        assert!(table.contains("boot-actors"));
        assert!(table.contains("outcome=veto"));
    }

    #[test]
    fn overall_flat_rise_alone_does_not_veto() {
        let set = pinned_set();
        // flat rises, boot-actors falls enough that weighted mean < 0.
        // weights: flat=1, boot-actors=10
        // rel_flat = +0.10; rel_boot = -0.20
        // mean = (1*0.10 + 10*(-0.20)) / 11 = (-1.9)/11 < 0 → win
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
        // No non-flat rise, but overall mean ≥ 0.
        // rel_flat = -0.01; rel_boot = +0.0 (equal)
        // mean = (1*(-0.01) + 10*0) / 11 < 0 → actually a tiny win.
        // Use: flat falls a little, boot equal → win. Need loss:
        // flat rises a lot, boot equal → mean > 0, no veto.
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
        // ε=0: equal is allowed; flat falls → win.
        let baseline = totals(&[("flat", 1000), ("boot-actors", 5000)]);
        let candidate = totals(&[("flat", 900), ("boot-actors", 5000)]);
        let cmp = compare_overall(&baseline, &candidate, &set).expect("compare");
        assert!(!cmp.vetoed());
        assert!(cmp.wins());
    }

    #[test]
    fn overall_missing_workload_fails_closed() {
        let set = pinned_set();
        let baseline = flat_only_totals(100);
        let candidate = flat_only_totals(90);
        let err = compare_overall(&baseline, &candidate, &set).expect_err("missing");
        assert!(
            err.contains("boot-actors"),
            "error should name missing W, got: {err}"
        );
    }

    #[test]
    fn overall_stub_all_with_corpus_sums_ranks_like_flat() {
        // Until J supplies measured rows: stub every W with the corpus sum.
        // Relative deltas identical across W → weighted mean == flat rel.
        let set = pinned_set();
        let cmp_corpus = compare_opt_lists(&[], RELEASE_OPTS);
        let baseline = stub_all_workload_totals(cmp_corpus.baseline_sum, &set);
        let candidate = stub_all_workload_totals(cmp_corpus.candidate_sum, &set);
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
}
