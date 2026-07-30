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
//! Overall compare (item K): given an `OverallSide` for baseline vs
//! candidate and pinned weights from `bench/workloads.toml`, **veto** if
//! any non-`flat` workload rises (ε=0), if measured coverage falls, or if
//! the static emitted word count grows; else **rank** by the weighted mean
//! of relative deltas `(cand−base)/base`.
//!
//! The coverage and word vetoes are soundness side conditions, not
//! rankings. The scoreboard prices neither "the candidate explains less of
//! the workload" nor "the candidate emits more code", and 04 §5 requires
//! that a proxy win never imply a real-machine loss — so both are refused
//! rather than absorbed into the mean.
//!
//! ## Freeze 1633: the barrier-removal refusal (plans/M20.md item G)
//!
//! A third side condition, and the only one that is a **correctness** rule
//! rather than a soundness-of-the-proxy rule: a candidate that emits fewer
//! ordering words (`DMB`, `LDAR`, `STLR`, system) than the baseline is
//! **refused**, whatever the cycle numbers say. Barriers are
//! correctness-load-bearing and `machine.cross-core.publish-acquire-barrier`
//! is a known-risk gap in plans/BLOCKED.md, so the gate may never credit
//! deleting one. The rule compares **counts of emitted words** —
//! [`cost::crosscore::ordering_removals`] — so there is no coefficient,
//! sweep dimension or table row whose value can satisfy it. Both gates
//! carry it: [`CorpusCompare::wins`] / [`assert_wins`] for the corpus gate
//! and [`VetoReason::OrderingWordsRemoved`] for the overall gate.
//!
//! **Note for item J:** the model side of this lives entirely in
//! `cost/crosscore.rs`; the only thing here is the plumbing — two
//! `CaseDelta` fields plus [`CaseDelta::ordering_removed`], one
//! `OverallSide` field plus [`OverallSide::with_ordering`], and one
//! `VetoReason` variant. Retiring the words veto (decision 1626) does not
//! touch any of it; the barrier refusal is independent of, and outlives,
//! the words condition.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use crate::cost::crosscore::{OrderingRemoval, ordering_removals, ordering_word_counts};
use crate::cost::score::CostReport;
use crate::cost::stage::report_cost_stage_path;
use crate::cost::workload::{self, FLAT_NAME, WorkloadSet};

use super::{CompileMode, OptId, RELEASE_OPTS, apply_mode, apply_opts};

/// One cost-* case scored under baseline vs candidate.
#[derive(Debug, Clone)]
pub struct CaseDelta {
    pub name: String,
    pub baseline: u64,
    pub candidate: u64,
    /// Static emitted word counts — the footprint side condition.
    pub baseline_words: u64,
    pub candidate_words: u64,
    /// Ordering-word counts per `[crosscore]`-priced rule — the freeze-1633
    /// refusal input (plans/M20.md item G).
    pub baseline_ordering: BTreeMap<&'static str, u64>,
    pub candidate_ordering: BTreeMap<&'static str, u64>,
}

impl CaseDelta {
    pub fn delta(&self) -> i64 {
        self.candidate as i64 - self.baseline as i64
    }

    pub fn words_delta(&self) -> i64 {
        self.candidate_words as i64 - self.baseline_words as i64
    }

    /// **Freeze 1633.** Ordering rules this case emits fewer words of under
    /// the candidate. Non-empty is a refusal, never a ranking input.
    pub fn ordering_removed(&self) -> Vec<OrderingRemoval> {
        ordering_removals(&self.baseline_ordering, &self.candidate_ordering)
    }

    /// Static-shape opts "delete or shorten the stream" (04 §5), so a
    /// rising word count contradicts the category by definition.
    pub fn words_grew(&self) -> bool {
        self.candidate_words > self.baseline_words
    }
}

/// Full corpus comparison result (per-case + sums).
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

    /// True when no case rises in cycles **or** words, no case deletes an
    /// ordering word, and at least one strictly falls in cycles.
    ///
    /// The word side condition exists because the scoreboard is in-order
    /// over an out-of-order core: reordering that the hardware already
    /// performs still shortens the modelled schedule, so a candidate could
    /// otherwise buy modelled cycles with real instructions. 04 §5 asks
    /// for "fewer/cheaper ops **and** shorter true data deps"; checking
    /// the composite alone does not enforce the conjunction.
    ///
    /// The ordering condition is freeze 1633 and is a different kind of
    /// rule: not "the proxy might be wrong" but "this word is
    /// correctness-load-bearing and its deletion is never a win".
    pub fn wins(&self) -> bool {
        let mut any_fall = false;
        for c in &self.cases {
            if c.candidate > c.baseline || c.words_grew() {
                return false;
            }
            if !c.ordering_removed().is_empty() {
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
    report_path_under_opts(path, opts).total_proxy_cycles
}

/// Same, returning the full report (cycles + static words).
pub fn report_path_under_opts(path: &Path, opts: &[OptId]) -> CostReport {
    apply_opts(opts);
    report_cost_stage_path(path).unwrap_or_else(|e| {
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
    let mut baseline_words = 0u64;
    let mut candidate_words = 0u64;

    for path in &corpus {
        let name = case_name(path);
        let b = report_path_under_opts(path, baseline);
        let c = report_path_under_opts(path, candidate);
        baseline_sum = baseline_sum.saturating_add(b.total_proxy_cycles);
        candidate_sum = candidate_sum.saturating_add(c.total_proxy_cycles);
        baseline_words = baseline_words.saturating_add(b.total_words);
        candidate_words = candidate_words.saturating_add(c.total_words);
        cases.push(CaseDelta {
            name,
            baseline: b.total_proxy_cycles,
            candidate: c.total_proxy_cycles,
            baseline_words: b.total_words,
            candidate_words: c.total_words,
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
        if c.words_grew() {
            grew.push(format!(
                "{}: {cand_label} {} words > {base_label} {} words",
                c.name, c.candidate_words, c.baseline_words
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
        grew.is_empty(),
        "{cand_label} grew static word count on {} case(s) — the proxy \
         cannot price I-cache footprint, so it must not certify growth \
         as a win:\n{}\n{table}",
        grew.len(),
        grew.join("\n"),
    );
    // Freeze 1633: barriers and the ordered accesses are
    // correctness-load-bearing, so their deletion is refused structurally
    // rather than priced. Checked before the "something fell" rule, since a
    // candidate whose only gain is a deleted barrier must read as refused,
    // not as "no case fell".
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

/// Stable text table for item L's evidence block.
pub fn format_delta_table(cmp: &CorpusCompare, base_label: &str, cand_label: &str) -> String {
    let mut out = String::new();
    out.push_str(&format!(
        "{:<22} {:>12} {:>12} {:>10} {:>10} {:>10} {:>8}\n",
        "case", base_label, cand_label, "Δ", "words_b", "words_c", "Δw"
    ));
    for c in &cmp.cases {
        out.push_str(&format!(
            "{:<22} {:>12} {:>12} {:>+10} {:>10} {:>10} {:>+8}\n",
            c.name,
            c.baseline,
            c.candidate,
            c.delta(),
            c.baseline_words,
            c.candidate_words,
            c.words_delta()
        ));
    }
    out.push_str(&format!(
        "{:<22} {:>12} {:>12} {:>+10} {:>10} {:>10} {:>+8}\n",
        "SUM",
        cmp.baseline_sum,
        cmp.candidate_sum,
        cmp.sum_delta(),
        cmp.baseline_words,
        cmp.candidate_words,
        cmp.words_delta()
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

/// Why an overall compare was vetoed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VetoReason {
    /// A non-`flat` measured workload rose (ε=0).
    WorkloadRose { name: String },
    /// Measured coverage fell: the candidate's `Σ f×s` explains less of
    /// the workload than the baseline's did. Without this, any transform
    /// that removes a hot method key from the scored set reads as a win —
    /// the gate would be rewarding *measuring less*, not running faster.
    CoverageFell {
        name: String,
        baseline: (u64, u64),
        candidate: (u64, u64),
    },
    /// Static emitted word count grew. The proxy has no I-cache/ITLB term,
    /// so it cannot certify a footprint increase as safe (04 §5: prefer
    /// over-cost when unsure).
    WordsGrew { baseline: u64, candidate: u64 },
    /// **Freeze 1633.** The candidate emits fewer words of a
    /// `[crosscore]`-priced ordering rule (`DMB`, `LDAR`, `STLR`, system).
    /// Those words are correctness-load-bearing —
    /// `machine.cross-core.publish-acquire-barrier` is a known-risk gap —
    /// so their deletion is refused structurally, not priced. Compares
    /// counts of emitted words, so no coefficient can satisfy it.
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
            VetoReason::WordsGrew {
                baseline,
                candidate,
            } => format!("words_grew:{baseline}->{candidate}"),
        }
    }
}

/// Outcome of overall compare over the pinned workload set.
#[derive(Debug, Clone)]
pub enum OverallOutcome {
    /// At least one veto condition fired.
    Veto { reasons: Vec<VetoReason> },
    /// No veto; `weighted_mean_rel` is Σ(w·rel)/Σ(w).
    Rank { weighted_mean_rel: f64 },
}

/// Full overall comparison (per-W rows + veto/rank outcome).
#[derive(Debug, Clone)]
pub struct OverallCompare {
    pub workloads_digest: String,
    pub workloads: Vec<WorkloadDelta>,
    pub baseline_coverage: BTreeMap<String, (u64, u64)>,
    pub candidate_coverage: BTreeMap<String, (u64, u64)>,
    pub baseline_words: u64,
    pub candidate_words: u64,
    pub outcome: OverallOutcome,
}

impl OverallCompare {
    pub fn vetoed(&self) -> bool {
        matches!(self.outcome, OverallOutcome::Veto { .. })
    }

    /// Veto reasons, empty when ranked.
    pub fn veto_reasons(&self) -> &[VetoReason] {
        match &self.outcome {
            OverallOutcome::Veto { reasons } => reasons,
            OverallOutcome::Rank { .. } => &[],
        }
    }

    /// Names of non-flat workloads that rose.
    pub fn risen(&self) -> Vec<String> {
        self.veto_reasons()
            .iter()
            .filter_map(|r| match r {
                VetoReason::WorkloadRose { name } => Some(name.clone()),
                _ => None,
            })
            .collect()
    }

    /// Weighted mean of relative deltas when not vetoed.
    pub fn weighted_mean_rel(&self) -> Option<f64> {
        match self.outcome {
            OverallOutcome::Rank { weighted_mean_rel } => Some(weighted_mean_rel),
            OverallOutcome::Veto { .. } => None,
        }
    }

    /// Win: not vetoed and weighted mean of relative deltas is strictly
    /// negative (overall proxy improvement under pinned weights).
    pub fn wins(&self) -> bool {
        match self.outcome {
            OverallOutcome::Veto { .. } => false,
            OverallOutcome::Rank { weighted_mean_rel } => weighted_mean_rel < 0.0,
        }
    }
}

/// One side of the overall gate: per-W proxy totals, per-W measured
/// coverage, and the static emitted word count.
#[derive(Debug, Clone, Default)]
pub struct OverallSide {
    pub totals: BTreeMap<String, u64>,
    /// Workload name → (matched_hits, total_hits).
    pub coverage: BTreeMap<String, (u64, u64)>,
    pub words: u64,
    /// Ordering-word counts per `[crosscore]`-priced rule — the freeze-1633
    /// refusal input (plans/M20.md item G). Empty on both sides leaves the
    /// refusal inert, which is what a plumbing test wants.
    pub ordering: BTreeMap<&'static str, u64>,
}

impl OverallSide {
    /// Read all four from a composed report (`cost::attach_workloads`
    /// must have run for measured rows to be present).
    pub fn from_report(report: &CostReport) -> Self {
        Self {
            totals: report.workload_totals.clone(),
            coverage: report.workload_coverage.clone(),
            words: report.total_words,
            ordering: ordering_word_counts(report),
        }
    }

    /// Totals only — no coverage rows, zero words, no ordering counts. For
    /// plumbing tests and flat-only callers; the coverage / word / ordering
    /// refusals stay inert.
    pub fn from_totals(totals: BTreeMap<String, u64>) -> Self {
        Self {
            totals,
            coverage: BTreeMap::new(),
            words: 0,
            ordering: BTreeMap::new(),
        }
    }

    pub fn with_ordering(mut self, ordering: BTreeMap<&'static str, u64>) -> Self {
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

/// Build a per-W totals map with only the `flat` row (pre-J shape).
pub fn flat_only_totals(flat_proxy_cycles: u64) -> BTreeMap<String, u64> {
    let mut m = BTreeMap::new();
    m.insert(FLAT_NAME.to_string(), flat_proxy_cycles);
    m
}

/// Stub every pinned workload with the same total (stand-in until item J
/// supplies measured `f×s` rows; useful for plumbing tests).
pub fn stub_all_workload_totals(proxy_cycles: u64, set: &WorkloadSet) -> BTreeMap<String, u64> {
    set.names().map(|n| (n.to_string(), proxy_cycles)).collect()
}

/// Load pinned weights from the committed `bench/workloads.toml`.
pub fn load_pinned_workloads() -> Result<WorkloadSet, String> {
    workload::load_default()
}

/// Compare candidate vs baseline under `weights`.
///
/// Fail closed if any pinned name is missing from either side. Extra keys
/// (not in the pinned set) are ignored. Veto — in this order, all reasons
/// collected — when any non-`flat` workload rises (ε=0), when measured
/// coverage falls, or when the static word count grows. Otherwise rank by
/// the weighted mean of relative deltas.
///
/// The two added vetoes close the ways a candidate could win the cycle
/// number while leaving real hardware the same or worse: explaining less
/// of the workload (coverage) and emitting more code (words).
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

    // Coverage: every workload the baseline measured must still be
    // explained at least as well. A missing candidate row is total loss.
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

    if candidate.words > baseline.words {
        reasons.push(VetoReason::WordsGrew {
            baseline: baseline.words,
            candidate: candidate.words,
        });
    }

    // Freeze 1633: a deleted ordering word is refused however the cycles
    // come out. Counts of emitted words, so nothing here is tunable.
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
        outcome,
    })
}

/// Stable per-W evidence table (printed under `--nocapture`).
pub fn format_overall_table(cmp: &OverallCompare, base_label: &str, cand_label: &str) -> String {
    let mut out = String::new();
    out.push_str(&format!("workloads_digest={}\n", cmp.workloads_digest));
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
        "words",
        "-",
        cmp.baseline_words,
        cmp.candidate_words,
        cmp.candidate_words as i64 - cmp.baseline_words as i64,
        "-"
    ));
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

/// Assert overall win (not vetoed, weighted mean rel < 0); panics with table.
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
        // flat improves; boot-actors rises → veto (ε=0).
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
        let baseline = OverallSide::from_totals(flat_only_totals(100));
        let candidate = OverallSide::from_totals(flat_only_totals(90));
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

    // -----------------------------------------------------------------------
    // Soundness side conditions: coverage + static footprint
    // -----------------------------------------------------------------------

    /// Losing measured coverage vetoes even when every cycle total falls.
    #[test]
    fn overall_vetoes_when_coverage_falls() {
        let set = pinned_set();
        let baseline = totals(&[("flat", 1000), ("boot-actors", 5000)]).with_coverage(cov(&[(
            "boot-actors",
            11,
            11,
        )]));
        // Every number improves — but the candidate explains 3 fewer hits.
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

    /// A candidate that drops the measured row entirely is total loss.
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

    /// Rising coverage is fine — explaining more is not a regression.
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

    /// Mismatched denominators mean the sides were measured against
    /// different frequency vectors — fail closed, don't rank.
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

    /// Growing the static word count vetoes even on a clean cycle win —
    /// the proxy has no I-cache term, so it must not certify growth.
    #[test]
    fn overall_vetoes_when_words_grow() {
        let set = pinned_set();
        let baseline = totals(&[("flat", 1000), ("boot-actors", 5000)]).with_words(4000);
        let candidate = totals(&[("flat", 900), ("boot-actors", 4000)]).with_words(4001);
        let cmp = compare_overall(&baseline, &candidate, &set).expect("compare");
        let table = format_overall_table(&cmp, "baseline", "candidate");
        eprintln!("overall words veto:\n{table}");
        assert!(cmp.vetoed(), "word growth must veto");
        assert!(!cmp.wins());
        assert_eq!(
            cmp.veto_reasons(),
            &[VetoReason::WordsGrew {
                baseline: 4000,
                candidate: 4001,
            }]
        );
        assert!(table.contains("words"), "{table}");
    }

    #[test]
    fn overall_equal_words_do_not_veto() {
        let set = pinned_set();
        let baseline = totals(&[("flat", 1000), ("boot-actors", 5000)]).with_words(4000);
        let candidate = totals(&[("flat", 900), ("boot-actors", 4000)]).with_words(4000);
        let cmp = compare_overall(&baseline, &candidate, &set).expect("compare");
        assert!(!cmp.vetoed());
        assert!(cmp.wins());
    }

    /// All three veto conditions are collected, not short-circuited — the
    /// evidence table should show every reason a candidate was refused.
    #[test]
    fn overall_collects_every_veto_reason() {
        let set = pinned_set();
        let baseline = totals(&[("flat", 1000), ("boot-actors", 5000)])
            .with_coverage(cov(&[("boot-actors", 11, 11)]))
            .with_words(4000);
        let candidate = totals(&[("flat", 900), ("boot-actors", 5001)])
            .with_coverage(cov(&[("boot-actors", 8, 11)]))
            .with_words(4100);
        let cmp = compare_overall(&baseline, &candidate, &set).expect("compare");
        assert_eq!(cmp.veto_reasons().len(), 3, "{:?}", cmp.veto_reasons());
    }

    // -----------------------------------------------------------------------
    // Freeze 1633: the barrier-removal refusal (plans/M20.md item G)
    // -----------------------------------------------------------------------

    fn ord(pairs: &[(&'static str, u64)]) -> BTreeMap<&'static str, u64> {
        pairs.iter().copied().collect()
    }

    /// **Freeze 1633 on the overall gate.** Every cycle number falls and
    /// coverage and words are clean — and the candidate is still refused,
    /// because it emits one fewer `DMB`. This is the `--omit-dmb` shape: the
    /// mutation arm of `boot-cross-core-publish-acquire` is exactly "delete
    /// the barrier and see if anything notices".
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

    /// The refusal covers the ordered halves too — they carry the same
    /// hazard by their own `removal_sensitive` profile rows — and it does
    /// **not** fire on keeping or adding an ordering word.
    #[test]
    fn overall_ordering_refusal_covers_every_crosscore_rule() {
        let set = pinned_set();
        let base_ord = ord(&[
            ("barrier", 6),
            ("load_acquire", 4),
            ("store_release", 6),
            ("system", 1),
        ]);
        let side = |ordering: BTreeMap<&'static str, u64>| {
            totals(&[("flat", 900), ("boot-actors", 4000)]).with_ordering(ordering)
        };
        let baseline =
            totals(&[("flat", 1000), ("boot-actors", 5000)]).with_ordering(base_ord.clone());
        // Identical counts: no refusal, ordinary win.
        let same = compare_overall(&baseline, &side(base_ord.clone()), &set).expect("cmp");
        assert!(!same.vetoed() && same.wins());
        // Adding is fine.
        let mut more = base_ord.clone();
        more.insert("barrier", 7);
        let added = compare_overall(&baseline, &side(more), &set).expect("cmp");
        assert!(!added.vetoed() && added.wins());
        // Dropping any one of the four is refused, and every dropped rule
        // is reported rather than only the first.
        for rule in ["barrier", "load_acquire", "store_release", "system"] {
            let mut fewer = base_ord.clone();
            *fewer.get_mut(rule).unwrap() -= 1;
            let cmp = compare_overall(&baseline, &side(fewer), &set).expect("cmp");
            assert!(cmp.vetoed(), "{rule} removal must be refused");
            assert_eq!(cmp.veto_reasons().len(), 1, "{rule}");
        }
        let cmp = compare_overall(&baseline, &side(ord(&[])), &set).expect("cmp");
        assert_eq!(cmp.veto_reasons().len(), 4, "{:?}", cmp.veto_reasons());
    }

    /// **Freeze 1633 on the corpus gate.** A case whose cycles fall while
    /// its barrier count drops is not a win, and `assert_wins` says why.
    #[test]
    fn corpus_gate_refuses_barrier_removal() {
        let cmp = CorpusCompare {
            cases: vec![CaseDelta {
                name: "cost-crosscore".to_string(),
                baseline: 1000,
                candidate: 900,
                baseline_words: 400,
                candidate_words: 399,
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
            msg.contains("freeze 1633") && msg.contains("ordering_words_removed:barrier:2->1"),
            "refusal must name freeze 1633 and the rule, got: {msg}"
        );
    }

    /// The live corpus gate is unaffected: the `cost-*` corpus emits no
    /// ordering word at all, so `release` vs `dev` carries 0 → 0 on every
    /// ordering rule and the refusal is inert there. Recorded because it is
    /// also the coverage gap item G reports — `cost-crosscore` (item M) is
    /// the golden that will exercise this.
    #[test]
    fn release_removes_no_ordering_words_and_the_corpus_has_none() {
        let cmp = compare_opt_lists(&[], RELEASE_OPTS);
        for c in &cmp.cases {
            assert!(
                c.ordering_removed().is_empty(),
                "{}: release removed an ordering word",
                c.name
            );
            assert!(
                c.baseline_ordering.values().all(|&n| n == 0),
                "{}: the cost-* corpus is expected to reach no ordering word; \
                 counts {:?} — if this fires, the coverage gap item G reported \
                 has closed and the report should say so",
                c.name,
                c.baseline_ordering
            );
            assert_eq!(
                c.baseline_ordering.len(),
                4,
                "every crosscore rule must have a slot, present at 0"
            );
        }
        apply_mode(CompileMode::Release);
    }

    // -----------------------------------------------------------------------
    // Null-opt oracle: the ruler must not reward a semantically neutral
    // change. These test the gate itself, not the code it gates.
    // -----------------------------------------------------------------------

    /// Renaming every scored fn key changes nothing a machine executes.
    /// Under method-grain compose the frequency vector stops matching, so
    /// this is exactly the fusion/rename shape — it must not win.
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
                total_words: words,
                owner_totals: BTreeMap::new(),
                fns,
                workloads_digest: None,
                workload_totals: BTreeMap::new(),
                workload_coverage: BTreeMap::new(),
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

    /// Comparing a program against itself must never be a win, on either
    /// gate. (Identity is the weakest possible null opt.)
    #[test]
    fn null_opt_identity_is_never_a_win() {
        let set = pinned_set();
        let side = totals(&[("flat", 1234), ("boot-actors", 5678)])
            .with_coverage(cov(&[("boot-actors", 11, 11)]))
            .with_words(999);
        let cmp = compare_overall(&side, &side, &set).expect("compare");
        assert!(!cmp.wins(), "identity must not win the overall gate");
        assert_eq!(cmp.weighted_mean_rel(), Some(0.0));

        // Corpus gate: release vs release.
        let corpus = compare_opt_lists(RELEASE_OPTS, RELEASE_OPTS);
        assert!(!corpus.wins(), "identity must not win the corpus gate");
        assert_eq!(corpus.sum_delta(), 0);
        assert_eq!(corpus.words_delta(), 0);
        apply_mode(CompileMode::Release);
    }

    /// The live release set must satisfy the footprint side condition, not
    /// just the cycle rule — release may not emit more words than dev.
    #[test]
    fn release_does_not_grow_words_vs_dev() {
        let cmp = compare_opt_lists(&[], RELEASE_OPTS);
        let table = format_delta_table(&cmp, "dev", "release");
        eprintln!("corpus words (dev → release):\n{table}");
        for c in &cmp.cases {
            assert!(
                !c.words_grew(),
                "{}: release {} words > dev {} words\n{table}",
                c.name,
                c.candidate_words,
                c.baseline_words
            );
        }
        assert!(
            cmp.words_delta() <= 0,
            "release must not grow total words\n{table}"
        );
    }
}
