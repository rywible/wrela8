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
//! any core's text/TLB budget overflow **grows**; else **rank** by the
//! weighted mean of relative deltas `(cand−base)/base`.
//!
//! The coverage and budget vetoes are soundness side conditions, not
//! rankings. The scoreboard prices neither "the candidate explains less of
//! the workload" nor "the candidate no longer fits the core it runs on",
//! and 04 §5 requires that a proxy win never imply a real-machine loss —
//! so both are refused rather than absorbed into the mean.
//!
//! ## Item J: the words veto retired, the per-core budget installed
//!
//! Emitted word count is a **reported column** (04 §5 as item A rewrote it)
//! and no longer a condition of its own; the hard constraint in its place
//! is the per-core hot-text / I-TLB / L2-TLB budget of
//! [`cost::footprint`](crate::cost::footprint). Both landed in one commit,
//! never one ahead of the other (freeze 1626).
//!
//! The replacement is a **delta** rule — no core's over-budget quantity may
//! rise — and that is decision 1619. The reason is what the rule *means*,
//! not whether it would fire: an absolute
//! [`within_budget`](crate::cost::CoreBudget::within_budget) veto refuses a
//! candidate for a property of the **baseline** (a program already over its
//! L1I is refused however much better the candidate makes it), while the
//! veto being retired said "a candidate may not pay for schedule with more
//! footprint", which is a statement about the **change**. The delta keeps
//! that sentence true wherever the baseline sits relative to the ceiling.
//!
//! **plans/codegen-pareto-2.md decision 1954 made that reasoning load-
//! bearing rather than hypothetical.** This gate used to score the
//! cost-stage closure, which is comfortably inside every budget — the
//! flagship at 7 936 B of hot text against a 65 536 B L1I, `charge = 0` on
//! both sides. Item F's 91–92 KiB figure was the **image** program, a
//! different and much larger closure printing a line with the same name,
//! and item H measured the gap on the flagship at 11×. The gate now scores
//! the image each root would ship ([`crate::cost::stage::codegen_shipped_program`]),
//! so *every* image-bearing case is 89–391 KB of hot text and 367–5 092
//! lines over its L1I on both sides. An absolute veto would now refuse
//! every candidate including the identity, on every program the appliance
//! ships — pinned as
//! `unit:an_over_budget_identity_is_refused_absolutely_and_allowed_as_a_delta`.
//! The delta rule ranks them, and it is no longer silent: `release` takes
//! the flagship's `charge` from 6132 to 2569.
//!
//! **One** absolute assertion is kept alongside the delta: `within_budget()`
//! on every `cost-*` case in the corpus oracle, so the rule is live and
//! silent rather than inert.
//!
//! The absolute **I-TLB veto is retired** (plans/M20.md decision 1636). It
//! was kept on the premise that the I-side page span "is inside budget on
//! both surfaces", which item M falsified by building the `cost-itlb-span`
//! golden this plan asked for: core 0 at 57 text pages against 48, which
//! made the rule refuse **every** candidate at **every** box point, the
//! identity included. That is precisely the failure decision 1619 names —
//! an absolute rule refuses a candidate for a property of the **baseline**,
//! where the veto it replaced spoke about the **change**. The delta rule
//! already watches `over_itlb_pages`, so a candidate that *worsens* the
//! span is still refused and nothing was lost by retiring this.
//!
//! ## Item J: the ∀ sweep
//!
//! [`compare_opt_lists_over_box`] scores both sides at every point of the
//! residual-uncertainty box that can matter and refuses on any rank flip,
//! **naming the flipping point**. There is no per-point win predicate in
//! this module's public surface (freeze 1624) — `∃` is not a shape the API
//! can express, and `unit:no_public_per_point_win_predicate_exists` checks
//! that structurally rather than by comment.
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

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use crate::codegen::CodegenProgram;
use crate::cost::crosscore::{
    OrderingCounts, OrderingRemoval, ordering_removals, ordering_word_counts,
};
use crate::cost::footprint::CoreBudget;
use crate::cost::score::{CostReport, score_program_at};
use crate::cost::stage::{TextScope, codegen_shipped_program, report_cost_stage_path};
use crate::cost::sweep::{SweepPoint, endpoint_corners, record_reads};
use crate::cost::table::{CostTable, load_default};
use crate::cost::workload::{self, FLAT_NAME, WorkloadSet};
use crate::placement::PlacementTable;

use super::{CompileMode, OptId, RELEASE_OPTS, apply_mode, apply_opts};

/// One cost-* case scored under baseline vs candidate.
#[derive(Debug, Clone)]
pub struct CaseDelta {
    pub name: String,
    /// Which corpus tier this case belongs to (decision 1780). Reported in
    /// every table: decision 1717 forbids a reader having to guess which
    /// corpus a verdict came from.
    pub tier: CostTier,
    /// Which program was scored (decision 1954): the shipped image, or a
    /// closure for a root that declares no `@image`.
    pub scope: TextScope,
    pub baseline: u64,
    pub candidate: u64,
    /// Static emitted word counts — a **reported column** since item J
    /// retired the words veto (freeze 1626); no longer a condition.
    pub baseline_words: u64,
    pub candidate_words: u64,
    /// Per-core text/TLB budgets at the pinned point — the hard constraint
    /// that replaced the words veto, read as a **delta** (decision 1619).
    pub baseline_budgets: Vec<CoreBudget>,
    pub candidate_budgets: Vec<CoreBudget>,
    /// Ordering-word counts per `[crosscore]`-priced rule — the freeze-1633
    /// refusal input (plans/M20.md item G).
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

    /// **Freeze 1633.** Ordering rules this case emits fewer words of under
    /// the candidate. Non-empty is a refusal, never a ranking input.
    pub fn ordering_removed(&self) -> Vec<OrderingRemoval> {
        ordering_removals(&self.baseline_ordering, &self.candidate_ordering)
    }

    /// **Decision 1619.** Per-core budget overflow quantities that rose.
    /// Non-empty is a refusal; this is what stands in the retired words
    /// veto's place. `Err` when the two sides disagree about how many cores
    /// exist — that is not a rank, it is two different machines.
    pub fn budget_growth(&self) -> Result<Vec<BudgetGrowth>, String> {
        budget_overflow_growth(&self.baseline_budgets, &self.candidate_budgets)
    }
}

/// Total priced overflow charge across a side's cores — the reported
/// magnitude of the budget column.
fn total_charge(budgets: &[CoreBudget]) -> u64 {
    budgets.iter().fold(0u64, |a, b| a.saturating_add(b.charge))
}

/// One per-core budget quantity that rose from baseline to candidate.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BudgetGrowth {
    pub core: usize,
    /// The [`CoreBudget`] field name, as printed in the 04 §6 budget line.
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

/// The over-budget quantities the delta rule watches, in the order the
/// 04 §6 budget line prints them. Written as one list so a new overflow
/// field is added here or nowhere — the failure mode a hand-rolled
/// comparison per call site would have.
fn over_budget_quantities(b: &CoreBudget) -> [(&'static str, u64); 7] {
    [
        ("over_l1i_lines", b.over_l1i_lines),
        ("over_l2_lines", b.over_l2_lines),
        ("over_itlb_pages", b.over_itlb_pages),
        ("over_tlb_l2_pages", b.over_tlb_l2_pages),
        ("over_dtlb_pages", b.over_dtlb_pages),
        ("over_data_tlb_l2_pages", b.over_data_tlb_l2_pages),
        ("charge", b.charge),
    ]
}

/// **Decision 1619 — the rule that replaces the words veto.** No core's
/// over-budget quantity may increase.
///
/// The plan asked for the budget "as the hard constraint in its place",
/// which reads as an absolute [`CoreBudget::within_budget`] test. That
/// reading is implementable on the cost-stage closure — which is inside
/// every budget — but it is the wrong rule: it refuses a candidate for a
/// property of the **baseline**, while the veto it replaces was about the
/// **change**. It is also unsafe on the image program, whose every core is
/// already 409–413 lines over its 64 KiB L1I under `W_flat`, where an
/// absolute veto refuses every candidate including the identity. The delta
/// is what the words veto actually was, moved onto the right denominator.
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

/// The absolute half, kept **in addition** to the delta rule. Item F

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

    /// True when no case rises in cycles, no case grows a per-core budget
    /// overflow, no case deletes an ordering word, and at least one
    /// strictly falls in cycles.
    ///
    /// **Words are not checked here** (freeze 1626): with the I-side
    /// footprint priced, 04 §5 makes the emitted word count a reported
    /// column and the per-core hot-text / I-TLB / L2-TLB budget the hard
    /// constraint. The budget condition still exists for the reason the
    /// word one did — the scoreboard is in-order over an out-of-order core,
    /// so a candidate must not buy modelled cycles with real instructions —
    /// but it now asks the question against the denominator the machine has
    /// rather than against a whole-image word total.
    ///
    /// The ordering condition is freeze 1633 and is a different kind of
    /// rule: not "the proxy might be wrong" but "this word is
    /// correctness-load-bearing and its deletion is never a win".
    ///
    /// A core-count disagreement between the two sides is an error, and an
    /// error is not a win.
    pub fn wins(&self) -> bool {
        let mut any_fall = false;
        for c in &self.cases {
            if c.candidate > c.baseline {
                return false;
            }
            match c.budget_growth() {
                Ok(g) if g.is_empty() => {}
                _ => return false,
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

// ---------------------------------------------------------------------------
// The corpus and its two tiers (plans/codegen-pareto.md item H,
// decisions 1716/1717, 1780–1789)
// ---------------------------------------------------------------------------

/// Which tier of the cost corpus a case belongs to.
///
/// **Decision 1780.** The tier is read off the case's *shape*, not off a
/// list somebody maintains and not off its name:
///
/// - **[`Micro`](CostTier::Micro)** — the case owns its program. There is
///   `.wr` source inside the case directory (the flat `input.wr` shape, or
///   a `root` naming a package inside the case).
/// - **[`Product`](CostTier::Product)** — the case owns *no* source at
///   all. Its whole content is a one-line `root` naming a program that
///   already exists elsewhere in the tree for its own reasons.
///
/// The rule is the honesty rule. Decision 1716's first consequence is
/// **self-selection**: every item is told to add a `cost-*` case if none
/// exercises its opt, so each opt ends up graded on a program written to
/// show it off. A case that does not *contain* its program cannot have had
/// that program tuned for the gate — the appliance image and the boot
/// transcripts are what they are for reasons that predate this plan. So
/// "borrowed" and "product-scale" are the same predicate here, and the
/// classifier can be a total function of the directory rather than a
/// declaration a future case could get wrong.
///
/// Every other shape — no source and no `root`, both `input.wr` and a
/// `root`, a `root` that names nothing, a `root` pointing outside the case
/// while `.wr` files sit inside it — is an **error**, never a default
/// (decision 1793). M20's `MAX_SWEPT_DIMS` is the worked example of the
/// failure this avoids from the other side: a gate that silently does not
/// run is worse than one that admits it is off.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum CostTier {
    /// A microbenchmark written for this corpus.
    Micro,
    /// A program the appliance actually ships, borrowed whole.
    Product,
}

impl CostTier {
    pub fn as_str(self) -> &'static str {
        match self {
            CostTier::Micro => "micro",
            CostTier::Product => "product",
        }
    }

    /// Both tiers, in the order the tables print them. Written once so a
    /// third tier is added here or nowhere.
    pub const ALL: [CostTier; 2] = [CostTier::Micro, CostTier::Product];
}

impl std::fmt::Display for CostTier {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// One discovered `cost-*` case: its directory name, its tier, and the
/// program the cost stage is handed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CostCase {
    /// The case **directory** name (`cost-product-actors`), never the
    /// borrowed program's directory — the gate's rows must name the case a
    /// reader can find under `tests/golden/`.
    pub name: String,
    pub tier: CostTier,
    pub input: PathBuf,
}

fn golden_root() -> PathBuf {
    normalize_lexically(&PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tests/golden"))
}

/// Collapse `.` and `..` components without touching the filesystem.
///
/// A borrowed case's `root` is `../boot-actors/input.wr`, so the joined
/// path lexically *starts with* the case directory it is escaping — which
/// would make "does this case own its program?" answer yes for every
/// product case. Purely lexical because that is what the question is
/// about: which directory the case's `root` line points into. No symlink
/// resolution, no `canonicalize`, nothing that depends on the checkout.
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

/// Every `.wr` file at or under `dir`, recursively. Used only to decide
/// whether a case owns source; the answer is a bool, so it stops early.
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

/// **Decision 1793 — a case belongs to exactly one tier, or the corpus
/// refuses to be discovered.**
///
/// Returns `Err` with the case named and the shape described. There is no
/// "assume micro" branch: a case that falls through the classifier would be
/// scored by the gate while belonging to no tier's verdict, which is
/// precisely the lane-nobody-runs failure M20 spent an item on.
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
            // Read off the normalized target, not off the `root` line's
            // spelling: `./../x` and `../x` are the same escape.
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

/// Deterministic discovery of every `tests/golden/cost-*` case with its
/// tier, sorted by case name (decision 1450's ordering, widened by
/// decision 1780's tiering).
///
/// Fails closed: one unclassifiable directory refuses the **whole** corpus
/// rather than dropping that case. Sampling the corpus and ranking over
/// what is left is the failure this item exists to correct.
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

/// [`try_discover_cost_cases`], panicking with the offending case named.
/// Every gate entry point goes through this, so an untiered case stops the
/// gate instead of quietly leaving the corpus.
pub fn discover_cost_cases() -> Vec<CostCase> {
    try_discover_cost_cases().unwrap_or_else(|e| panic!("cost corpus: {e}"))
}

/// Cases in one tier only.
pub fn discover_cost_cases_in(tier: CostTier) -> Vec<CostCase> {
    discover_cost_cases()
        .into_iter()
        .filter(|c| c.tier == tier)
        .collect()
}

/// The scored programs of the whole corpus, both tiers, sorted by case
/// name (decision 1450). Kept path-only for the several structural census
/// tests outside this module that walk the corpus and do not care which
/// tier a program came from.
pub fn discover_cost_corpus() -> Vec<PathBuf> {
    discover_cost_cases().into_iter().map(|c| c.input).collect()
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

/// [`report_path_under_opts`] over the program that would **ship**
/// (decision 1954) — the image where the root declares one, the closure
/// where it does not — plus which of the two it was.
///
/// The corpus gate and the ∀ sweep both go through this, so they rank the
/// same program as each other and as `wrela build`.
fn shipped_report_under_opts(path: &Path, opts: &[OptId]) -> (CostReport, TextScope) {
    apply_opts(opts);
    let (program, placement, scope) = codegen_shipped_program(path)
        .unwrap_or_else(|e| panic!("shipped-program score {}: {e}", path.display()));
    let table = load_default().unwrap_or_else(|e| panic!("cost table: {e}"));
    let report = crate::cost::score::score_program(&program, &table, &placement)
        .unwrap_or_else(|e| panic!("score {}: {e}", path.display()));
    (report, scope)
}

/// Score every cost-* case under `baseline` vs `candidate` opt lists
/// (decision 1451–1452). Restores `CompileMode::Release` afterward.
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
        let (b, scope) = shipped_report_under_opts(path, baseline);
        let (c, cscope) = shipped_report_under_opts(path, candidate);
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
    // Decision 1619: the per-core budget replaces the words veto, as a
    // **delta**. Words stay in the table above as a reported column.
    assert!(
        grew.is_empty(),
        "{cand_label} grew a per-core text/TLB budget overflow on {} \
         case(s) — 04 §5 makes that budget the hard constraint the emitted \
         word count no longer is:\n{}\n{table}",
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
///
/// `words_b` / `words_c` / `Δw` stay — as a **reported column** (freeze
/// 1626), no longer a veto — and `chg_b` / `chg_c` / `Δchg` carry the
/// per-core budget charge that took the veto's place. `cores=0` in the
/// budget column means the case has no `@image`, so there is no per-core
/// denominator and the budget rule is inert on it.
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
    // Decision 1717: both tiers get printed, and a per-tier subtotal is
    // what makes "the two tiers disagree" a thing a reader can see rather
    // than derive. The product row governs.
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

// ---------------------------------------------------------------------------
// The ∀ sweep over the residual-uncertainty box (plans/M20.md item J,
// decision 1604, freeze 1624)
// ---------------------------------------------------------------------------

/// Fail-closed bound on how many dimensions one case may sweep.
///
/// The bound exists so a model change that makes many more dimensions live
/// **errors** rather than silently truncating the sweep — decision 1604
/// forbids dropping a dimension, so the only honest response to a box this
/// gate cannot enumerate is to refuse to rank. It is not a performance
/// target and never a knob to turn until a candidate passes.
///
/// **Set to 14 deliberately, on a measurement, 2026-07-30.** `cost-crosscore`
/// reads `dmb_cost`, `snoop_cost`, `load_acquire_cost` and
/// `store_release_cost` on top of the ten the rest of the corpus reaches,
/// so its probe reports 14 surviving dimensions. At the old bound of 12
/// the whole-corpus gate did not rank *anything*: it refused at
/// `cost-crosscore` before reaching any candidate, and since freeze 1714
/// routes every landing through `compare_opt_lists_over_box`, no opt could
/// be ranked over the box at all. A bound that refuses the entire corpus
/// is not a fail-closed bound, it is an outage.
///
/// The cost is measured, not guessed. The 1916 s (31 m 58 s) figure once
/// recorded here does not reproduce, and the run that produced it also
/// *failed*, on the since-retired absolute I-TLB veto.
///
/// The unit to plan by is **∀ points enumerated per side**, because that is
/// determined; wall clock on the machine that measured this was not, and
/// is quoted below only as a range with its `n`.
///
/// | when | corpus | release sweep pts/side | whole deep lane pts/side |
/// | --- | --- | --- | --- |
/// | 2026-07-30 (M20) | 15 micro | 26 112 | 52 224 |
/// | 2026-07-31 (item H) | 15 micro + 4 product | 36 352 | **93 184** |
///
/// Item H's four product cases raise the release sweep by 10 240
/// points/side (28% of it) and add a third deep test,
/// `each_release_opt_is_re_asked_alone_on_the_product_tier`, for **×1.78
/// the deep lane's ∀ work**.
///
/// **That ×1.78 did not show up as wall clock.** As `xtask::deep_lane`
/// invokes it, the widened lane ran in **309 / 362 / 397 s** (n=3) against
/// **411 s** (n=1) before — all three faster, on the same laptop under the
/// same concurrent-worktree load. Not because the product cases are cheap:
/// because the third `#[ignore]`d test gives `cargo test` a third harness
/// thread, and two long sweeps had been leaving cores idle. The same
/// widened corpus with only two tests took 597 s, which is what the extra
/// points cost without that parallelism. Minutes either way is a deep-lane
/// cost, not a `cargo test` cost, which is why these sweeps are
/// `#[ignore]`d and run by `xtask::deep_lane` — a lane that, until that
/// function landed, did not exist, so this gate was refusing into a void
/// nothing executed. `--fast` does not reach it (`xtask::check` returns
/// first), so the cost lands at milestone close and nowhere else. M20's
/// 243 s idle figure is not comparable to any of these; an idle
/// re-measurement belongs to the close.
///
/// No product case pushes the probe past this bound: the widest is
/// `cost-product-blk`/`cost-product-receipt` at `k=12`, under
/// `cost-crosscore`'s 14. That was the risk worth naming — a borrowed
/// program reaching `k=15` would have made the widened corpus refuse to
/// rank at all, which is this constant's own worked failure.
///
/// Raising it further needs the same treatment: a measured wall time for
/// the deep lane, in its own commit, with the reason the model now reads
/// more of the box.
pub const MAX_SWEPT_DIMS: usize = 14;

/// A dimension the sensitivity probe proved cannot matter for one case, so
/// it is held at its pinned value instead of being cornered over.
///
/// This is **not** dropping a dimension (decision 1604). `read=false` means
/// the model never asked for this dimension's value while scoring either
/// side — a term that is never read cannot change a score at any point of
/// the box, whatever the other dimensions do.
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

/// One point of one case's residual box, both sides scored.
///
/// Deliberately plain data with no verdict on it: a `wins`-shaped method
/// here would be the `∃` form freeze 1624 refuses, one `.iter().any()` away
/// from a search for a flattering assumption. The only verdict in this
/// module's public surface is [`SweepCompare::wins`], which is ∀.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PointRow {
    /// [`SweepPoint::label_over`] the case's surviving dimensions.
    pub point: String,
    pub baseline: u64,
    pub candidate: u64,
    /// Σ per-core budget charge, which moves with the point (its terms are
    /// `l2_latency`, `l3_latency` and `tlb_walk_cost`).
    pub baseline_charge: u64,
    pub candidate_charge: u64,
}

impl PointRow {
    pub fn delta(&self) -> i64 {
        self.candidate as i64 - self.baseline as i64
    }
}

/// One case swept over its residual box.
#[derive(Debug, Clone)]
pub struct CaseSweep {
    pub name: String,
    /// Which corpus tier this case belongs to (decision 1780).
    pub tier: CostTier,
    /// Dimensions the committed profile declares — the nominal box.
    pub box_dims: usize,
    /// `2^box_dims`: the endpoint-corner cardinality of the whole box.
    pub box_cardinality: u64,
    /// Dimensions this case is sensitive to; `k = swept.len()`.
    pub swept: Vec<String>,
    /// Dimensions held at pinned, with the bracket each was held across.
    pub held: Vec<HeldDim>,
    /// Read by the model but with no measured effect at any probe base.
    /// **Kept in `swept` anyway** — the probe excludes on "never read", and
    /// this list is the doubt it refused to resolve in its own favour.
    pub read_but_static: Vec<String>,
    /// `2^k` rows, in [`endpoint_corners`] order.
    pub points: Vec<PointRow>,
}

impl CaseSweep {
    pub fn corners(&self) -> usize {
        self.points.len()
    }
}

/// Why a swept comparison was refused. Every variant that can name a point
/// does (04 §5: a reason that fires at one point of the residual box names
/// that point).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SweepVeto {
    /// The candidate's total is higher than the baseline's at this point.
    CaseRose {
        case: String,
        point: String,
        baseline: u64,
        candidate: u64,
    },
    /// **Decision 1619.** A per-core budget overflow quantity rose at this
    /// point.
    BudgetGrew {
        case: String,
        point: String,
        growth: BudgetGrowth,
    },
    /// **Freeze 1633.** Point-independent: a count of emitted words.
    OrderingWordsRemoved {
        case: String,
        rule: &'static str,
        baseline: u64,
        candidate: u64,
    },
    /// No case **in this tier** falls at *every* point of its own box.
    /// 04 §5's "must strictly lower at least one" read under ∀: a case that
    /// falls only where the assumptions flatter it has not lowered
    /// anything.
    ///
    /// **Decision 1782 — the rule is per tier, not per corpus.** Decision
    /// 1717 says an opt may not gate on a case it authored alone, and a
    /// corpus-wide quantifier is exactly the loophole that permits it: with
    /// the tiers pooled, an opt that wins only on the microbenchmark it
    /// shipped with satisfies "some case fell everywhere" while the
    /// appliance never moved. Asking the question once per tier is what
    /// makes the product tier a gate rather than a printout.
    NoCaseFallsEverywhere { tier: CostTier },
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

    /// The tier this refusal is about, when it has one. `CaseRose` and
    /// friends name a case rather than a tier; the sweep looks the case's
    /// tier up, which is why this returns `None` for them and
    /// [`SweepCompare::reasons_for_tier`] does the join.
    pub fn tier(&self) -> Option<CostTier> {
        match self {
            SweepVeto::NoCaseFallsEverywhere { tier } => Some(*tier),
            _ => None,
        }
    }

    /// The case this refusal names, when it names one.
    pub fn case(&self) -> Option<&str> {
        match self {
            SweepVeto::CaseRose { case, .. }
            | SweepVeto::BudgetGrew { case, .. }
            | SweepVeto::OrderingWordsRemoved { case, .. } => Some(case.as_str()),
            SweepVeto::NoCaseFallsEverywhere { .. } => None,
        }
    }
}

/// The ∀ verdict plus the per-point table for the evidence block.
#[derive(Debug, Clone)]
pub struct SweepCompare {
    pub table_digest: String,
    pub cases: Vec<CaseSweep>,
    pub reasons: Vec<SweepVeto>,
}

impl SweepCompare {
    /// **The only verdict this module exposes**, and it is ∀: no case rose,
    /// no budget overflow grew and no ordering word vanished at **any**
    /// point, and some case fell at **every** point (freeze 1624).
    pub fn wins(&self) -> bool {
        self.reasons.is_empty()
    }

    /// Total points scored per side across the corpus.
    pub fn scored_points(&self) -> usize {
        self.cases.iter().map(CaseSweep::corners).sum()
    }

    /// The tiers this sweep actually covered, in [`CostTier::ALL`] order.
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

    /// Every refusal attributable to `tier` — the tier-tagged ones plus the
    /// case-named ones whose case sits in that tier (decision 1717: both
    /// tiers' verdicts are reported, so each must be separable).
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

    /// The ∀ verdict restricted to one tier. **Decision 1717: the
    /// `Product` answer governs**, and this is the accessor that makes the
    /// two answers separately statable. `wins()` is still the conjunction —
    /// a candidate is not landed on a tier split.
    pub fn wins_in_tier(&self, tier: CostTier) -> bool {
        self.reasons_for_tier(tier).is_empty()
    }

    /// The one line decision 1717 asks for: both tiers' verdicts, named.
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

/// One side of one case, compiled once and scored at many points. Codegen
/// dominates the cost of a comparison, so it happens `2 × cases` times, not
/// `2 × cases × points` times.
struct CompiledSide {
    program: CodegenProgram,
    placement: PlacementTable,
    /// Which program this is (decision 1954). Reported on every table row
    /// so nobody has to infer from a case name whether a verdict is about
    /// the shipped image or about a truncated closure.
    scope: TextScope,
}

/// Everything the gate reads from one side at one point.
#[derive(Debug, Clone, PartialEq, Eq)]
struct SideScore {
    cycles: u64,
    words: u64,
    budgets: Vec<CoreBudget>,
    ordering: OrderingCounts,
}

impl SideScore {
    fn charge(&self) -> u64 {
        total_charge(&self.budgets)
    }
}

/// **Decision 1954: the gate scores what would ship.**
///
/// This used to be [`codegen_cost_stage_with_placement`] — the
/// guest-reachable closure against a *stub* `core.__image_runtime`, which
/// on the flagship is 21 fns and 7 936 B of hot text with `charge = 0`.
/// The image `wrela build` emits for the same root is 325 fns and 89 024 B,
/// 367 lines over its 64 KiB L1I, charged 2 569. Both printed a line called
/// `Budget`, and the gate read the small one — so every landing decision on
/// this plan was taken against a program the appliance does not ship, and
/// round 1's item D could not be scored because its premise lived in the
/// other column (item H's decision 1788).
///
/// A case whose root declares no `@image` ships nothing; its closure *is*
/// its program, and [`TextScope::Closure`] says so on the row.
fn compile_side(path: &Path, opts: &[OptId]) -> Result<CompiledSide, String> {
    apply_opts(opts);
    let (program, placement, scope) = codegen_shipped_program(path)?;
    Ok(CompiledSide {
        program,
        placement,
        scope,
    })
}

fn score_side_at(
    side: &CompiledSide,
    table: &CostTable,
    point: &SweepPoint,
) -> Result<SideScore, String> {
    let r = score_program_at(&side.program, table, &side.placement, point)?;
    Ok(SideScore {
        cycles: r.total_proxy_cycles,
        words: r.total_words,
        budgets: r.footprint.clone(),
        ordering: ordering_word_counts(&r),
    })
}

/// `2^n` where `n` is the number of dimensions the committed profile
/// declares — the nominal endpoint-corner cardinality of the box.
pub fn box_cardinality(table: &CostTable) -> u64 {
    1u64 << table.sweep_dimensions().len().min(63)
}

/// What the sensitivity probe learned about one case.
#[derive(Debug)]
struct Probe {
    swept: Vec<String>,
    held: Vec<HeldDim>,
    read_but_static: Vec<String>,
}

/// Decide which dimensions this case can be swept over without dropping
/// one (decision 1604).
///
/// For each dimension `d` the probe scores **both sides** with `d` at `lo`
/// and at `hi`, over three bases — the pinned corner, the all-lo corner and
/// the all-hi corner — recording, at every one of those scorings, which
/// dimensions the model actually *read* through [`SweepPoint::get`].
///
/// A dimension is held only when it was **never read by either side at any
/// probe point**. That is a reason rather than an observation: a term no
/// scoring path asks for cannot move a total at any assignment of the other
/// dimensions. "Neither side's total moved" is kept as a cross-check — a
/// dimension that moved a total while never being read would mean the model
/// reads the box through some other door, so that combination is an error
/// (fail closed), not a silent exclusion. A dimension that *was* read but
/// moved nothing stays in the sweep: it is doubt, and doubt keeps it in.
fn probe_case(
    name: &str,
    base: &CompiledSide,
    cand: &CompiledSide,
    table: &CostTable,
) -> Result<Probe, String> {
    probe_case_bounded(name, base, cand, table, MAX_SWEPT_DIMS)
}

/// [`probe_case`] with the fail-closed bound supplied, so the refusal path
/// can be driven by a test at a bound the committed profile can exceed.
/// Every production caller goes through `probe_case` and gets
/// [`MAX_SWEPT_DIMS`].
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
    let mut score = |side: &CompiledSide, p: &SweepPoint| -> Option<SideScore> {
        let (out, r) = record_reads(|| score_side_at(side, table, p));
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

/// Every refusal that can fire at one point, for one case, appended to
/// `reasons` — and the evidence row for that point.
///
/// This is the whole per-point rule and it is **private and one-way**: it
/// pushes reasons and never answers "did the candidate win here". The ∀
/// quantifier lives in the loop that calls it, which is the only place a
/// verdict is formed (freeze 1624).
///
/// `check_ordering` exists because ordering-word counts are counts of
/// emitted words and therefore identical at every point of the box —
/// reporting the same refusal `2^k` times would bury every other reason.
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
        for r in ordering_removals(&b.ordering, &c.ordering) {
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

/// Sweep one already-compiled case over its residual box.
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

    let mut points = Vec::with_capacity(corners.len());
    let mut ordering_reported = false;
    for p in &corners {
        let b = score_side_at(base, table, p)?;
        let c = score_side_at(cand, table, p)?;
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

/// **The public ∀ entry.** Compare two opt lists over the whole cost-*
/// corpus at every point of the residual-uncertainty box that can matter,
/// returning the verdict together with the per-point table.
///
/// There is no per-point win predicate here or anywhere in this module's
/// public surface (freeze 1624). The caller gets rows and one ∀ verdict; it
/// cannot ask "did the candidate win *somewhere*".
pub fn compare_opt_lists_over_box(
    baseline: &[OptId],
    candidate: &[OptId],
) -> Result<SweepCompare, String> {
    sweep_corpus(baseline, candidate, CorpusSel::All)
}

/// The same ∀ sweep restricted to one tier. Exists so the deep lane can
/// say what the product tier costs and what it decides **on its own**,
/// without the micro tier's fifteen cases in the average — decision 1717's
/// "both numbers are reported" needs each to be obtainable alone.
pub fn compare_opt_lists_over_box_in_tier(
    baseline: &[OptId],
    candidate: &[OptId],
    tier: CostTier,
) -> Result<SweepCompare, String> {
    sweep_corpus(baseline, candidate, CorpusSel::Tier(tier))
}

/// The same ∀ sweep restricted to one named case — the **smoke lane**.
///
/// The whole-corpus sweep is minutes once item M's cases join it, so it is
/// `#[ignore]`d and run by `cargo xtask check`, exactly as every `fuzz_*`
/// lane splits a smoke budget from a deep one. This is what keeps ∀ coverage
/// in the default `cargo test` loop: it is the identical code path, the
/// identical probe and the identical refusals, over one case instead of
/// fifteen. A smoke lane that ran *different* code would be worthless.
pub fn compare_opt_lists_over_box_for_case(
    baseline: &[OptId],
    candidate: &[OptId],
    case: &str,
) -> Result<SweepCompare, String> {
    sweep_corpus(baseline, candidate, CorpusSel::Case(case))
}

/// Which slice of the corpus a sweep runs over. Deliberately a closed enum
/// rather than a predicate: "sweep whatever these cases are" is one step
/// from "sweep the cases that flatter the candidate".
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
        let b = compile_side(path, baseline)?;
        let c = compile_side(path, candidate)?;
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

    // 04 §5's "must strictly lower at least one", read under ∀: a case that
    // falls only at some points has not lowered anything the gate can rely
    // on. Checked after the refusals so a candidate whose only gain is a
    // deleted barrier reads as refused rather than as "nothing fell".
    //
    // **Decision 1782: once per tier.** Asked once over the pooled corpus,
    // the quantifier is satisfied by whichever tier is easiest, which for
    // every item on this plan is the microbenchmark it wrote itself. Only
    // the tiers actually swept are asked — the smoke lane sweeps one case
    // and must keep meaning what it meant.
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

/// Stable per-point evidence table (printed under `--nocapture`).
///
/// Prints both the nominal box cardinality and the surviving `k` per case,
/// so a reader sees what was enumerated *and* what it stands for, and lists
/// every held dimension with the bracket it was held across — a silent
/// reduction would be exactly the failure decision 1604 exists to prevent.
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
    // **Decision 1783 — every verdict is printed beside the tier it came
    // from.** Decision 1717 makes the product tier govern where the two
    // disagree, which is only usable if a reader can tell them apart
    // without going and looking up which corpus a case name belongs to.
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

/// Assert the ∀ verdict, panicking with the per-point table and every
/// reason that fired (04 §5: not just the first).
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

// ---------------------------------------------------------------------------
// Per-opt attribution (plans/M20.md item K)
// ---------------------------------------------------------------------------

/// One `cost-*` case scored under one named opt configuration.
///
/// Four columns because item K's question needs all four at once: `cycles`
/// is what the gate ranks on, `words` is the reported column freeze 1626
/// left behind, `charge` is the priced I-side term that replaced it, and
/// `hot_text_bytes` is the quantity `charge` is computed from — printed so
/// a footprint win that the budget prices at **zero** is visible as a
/// footprint win rather than as nothing at all.
#[derive(Debug, Clone)]
pub struct AttributionCell {
    pub config: String,
    pub proxy_cycles: u64,
    pub words: u64,
    pub charge: u64,
    pub hot_text_bytes: u64,
}

/// One `cost-*` case across every configuration, in the order given.
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

/// Score the whole cost-* corpus under each named opt configuration
/// (plans/M20.md item K). `compare_opt_lists` answers "does the candidate
/// beat the baseline"; this answers "which opt paid for it", which is a
/// different question and gets its own dumb loop rather than a flag on the
/// comparison.
///
/// This is **attribution, not a gate.** It returns no verdict and no win
/// predicate — freeze 1624's prohibition is on `∃`-shaped win predicates,
/// and the honest way to stay clear of one is to expose no predicate here
/// at all. Restores `CompileMode::Release` afterward, like its neighbours.
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
                hot_text_bytes: r.footprint.iter().map(|b| b.hot_text_bytes).sum(),
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

/// Stable text form of [`attribute_opts`] for item K's evidence block.
///
/// Every number here is a **flat** (`f ≡ 1`) total on the cost-stage
/// closure, not a measured one — decision 1617's coverage rider attaches to
/// the measured surface, and decision 1619 says the veto is read off the
/// flat row. The header says so, so no reader can lift a row out of this
/// table and call it a measurement.
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
                c.hot_text_bytes
            ));
        }
    }
    let configs: Vec<&str> = rows
        .first()
        .map(|r| r.cells.iter().map(|c| c.config.as_str()).collect())
        .unwrap_or_default();
    // Per-tier subtotals first, then the pooled one — decision 1783: a
    // reader must never have to guess which corpus a number came from, and
    // a pooled sum with fifteen micro rows and four product ones is exactly
    // that guess.
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
                    hot = hot.saturating_add(c.hot_text_bytes);
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
                hot = hot.saturating_add(c.hot_text_bytes);
            }
        }
        out.push_str(&format!(
            "{:<24} {:<8} {:<18} {:>10} {:>10} {:>8} {:>10}\n",
            "SUM", "both", label, cycles, words, charge, hot
        ));
    }
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
    /// **Decision 1619.** A per-core text/TLB budget overflow quantity
    /// rose. This is what replaced the retired word-count veto (freeze
    /// 1626): with the I-side term real, 04 §5 prices footprint growth and
    /// makes the **budget** the hard constraint. It is read as a delta
    /// because the veto it replaces was one — "a candidate may not pay for
    /// schedule with more footprint" is a claim about the change, not about
    /// where the baseline sits relative to the ceiling.
    BudgetOverflowGrew {
        core: usize,
        field: &'static str,
        baseline: u64,
        candidate: u64,
    },
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
            VetoReason::BudgetOverflowGrew {
                core,
                field,
                baseline,
                candidate,
            } => format!("budget_grew:core{core}:{field}:{baseline}->{candidate}"),
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
    /// Reported column, not a veto input (freeze 1626).
    pub baseline_words: u64,
    pub candidate_words: u64,
    /// The hard constraint that replaced it (decision 1619).
    pub baseline_budgets: Vec<CoreBudget>,
    pub candidate_budgets: Vec<CoreBudget>,
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
/// coverage, the reported word count, and the per-core budgets.
#[derive(Debug, Clone, Default)]
pub struct OverallSide {
    pub totals: BTreeMap<String, u64>,
    /// Workload name → (matched_hits, total_hits).
    pub coverage: BTreeMap<String, (u64, u64)>,
    /// **Reported**, never a veto input, since item J (freeze 1626).
    pub words: u64,
    /// Per-core text/TLB budgets — the hard constraint that replaced the
    /// word veto, read as a delta (decision 1619). Empty on both sides
    /// leaves the rule inert, which is what a plumbing test wants.
    pub budgets: Vec<CoreBudget>,
    /// Ordering-word counts per `[crosscore]`-priced rule — the freeze-1633
    /// refusal input (plans/M20.md item G). Empty on both sides leaves the
    /// refusal inert, which is what a plumbing test wants.
    pub ordering: OrderingCounts,
}

impl OverallSide {
    /// Read all four from a composed report (`cost::attach_workloads`
    /// must have run for measured rows to be present).
    pub fn from_report(report: &CostReport) -> Self {
        Self {
            totals: report.workload_totals.clone(),
            coverage: report.workload_coverage.clone(),
            words: report.total_words,
            budgets: report.footprint.clone(),
            ordering: ordering_word_counts(report),
        }
    }

    /// Totals only — no coverage rows, zero words, no budgets, no ordering
    /// counts. For plumbing tests and flat-only callers; the coverage /
    /// budget / ordering refusals stay inert.
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
/// coverage falls, when a per-core budget overflow grows, or when the
/// candidate's text overflows the L1 I-TLB. Otherwise rank by the weighted
/// mean of relative deltas.
///
/// The added vetoes close the ways a candidate could win the cycle number
/// while leaving real hardware the same or worse: explaining less of the
/// workload (coverage) and no longer fitting the core it runs on (budget).
/// The word count is a reported column and vetoes nothing (freeze 1626).
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

    // Decision 1619 / freeze 1626: the words veto is gone (words are the
    // reported column above) and the per-core budget stands in its place,
    // as a **delta** — under `W_flat` the absolute budget is already
    // breached on every core of every boot case, so an absolute reading
    // would refuse the identity.
    for g in budget_overflow_growth(&baseline.budgets, &candidate.budgets)? {
        reasons.push(VetoReason::BudgetOverflowGrew {
            core: g.core,
            field: g.field,
            baseline: g.baseline,
            candidate: g.candidate,
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
        baseline_budgets: baseline.budgets.clone(),
        candidate_budgets: candidate.budgets.clone(),
        outcome,
    })
}

/// Coverage fraction as `matched/total (pp.p%)`, or `-` when the row is
/// not a measured one. **Decision 1617:** block-grain coverage on
/// `boot-actors` is 893/6647 ≈ 13.4%, and with ~5 754 uncovered hits each
/// charged at the program maximum the measured total is dominated by that
/// term — so the fraction is printed *beside* the number it qualifies,
/// never on a line a reader can skip.
fn coverage_cell(cov: Option<(u64, u64)>) -> String {
    match cov {
        Some((m, t)) if t > 0 => {
            format!("{m}/{t} ({:.1}%)", 100.0 * (m as f64) / (t as f64))
        }
        Some((m, t)) => format!("{m}/{t}"),
        None => "-".to_string(),
    }
}

/// Stable per-W evidence table (printed under `--nocapture`).
///
/// Each measured row carries its coverage fraction **in the row**
/// (decision 1617): a 13.4%-covered total is not a measured result about
/// the program, and a reader must not be able to take it for one.
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
    // Reported column, not a veto (freeze 1626).
    out.push_str(&format!(
        "{:<16} {:>8} {:>12} {:>12} {:>+10} {:>12}\n",
        "words(reported)",
        "-",
        cmp.baseline_words,
        cmp.candidate_words,
        cmp.candidate_words as i64 - cmp.baseline_words as i64,
        "-"
    ));
    // The hard constraint that replaced it (decision 1619).
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
        // The path list stays the scored programs, in the same order.
        assert_eq!(
            discover_cost_corpus(),
            cases.iter().map(|c| c.input.clone()).collect::<Vec<_>>()
        );
    }

    // -----------------------------------------------------------------------
    // Item H: the tier split (decisions 1780–1783)
    // -----------------------------------------------------------------------

    /// **Decision 1793's oracle: every case is in exactly one tier, and
    /// both tiers are populated.**
    ///
    /// The second half is the one M20 paid for. `MAX_SWEPT_DIMS` at 12 made
    /// the ∀ gate refuse the whole corpus, and the whole-corpus sweep was
    /// `#[ignore]`d into a lane that did not exist — twice, the failure was
    /// a gate that silently scored nothing. An empty `product` tier would
    /// be the same failure in a new place: `compare_opt_lists_over_box`
    /// would keep returning `wins`, over the microbenchmarks only, and
    /// nothing would say so.
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

    /// **Decision 1793: an unclassifiable case refuses the corpus, it does
    /// not fall out of it.** Every shape that is neither tier is driven
    /// here, on real directories, because the failure mode being guarded is
    /// a case that scores in the gate while belonging to no tier's verdict.
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

        // The shape that would let a product case smuggle in a program of
        // its own — borrowed *and* self-authored.
        let hybrid = mk("cost-hybrid");
        std::fs::create_dir_all(hybrid.join("src")).unwrap();
        std::fs::write(hybrid.join("src/extra.wr"), "module x\n").unwrap();
        std::fs::write(hybrid.join("root"), "../cost-both/input.wr\n").unwrap();
        let e = classify_cost_case(&hybrid).expect_err("borrowed but self-authored");
        assert!(e.contains("owns no program of its own"), "{e}");

        // And the two shapes that *are* tiers, from the same classifier.
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

    /// **Decision 1783: the tier is on the row.** A verdict a reader has to
    /// attribute to a corpus by recognising case names is a verdict that
    /// gets attributed wrong.
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

    /// **Decision 1782 checked on a real refusal:** a candidate that falls
    /// everywhere in one tier and nowhere in the other is vetoed, and the
    /// veto names the tier. Built by hand rather than by finding an opt
    /// that does this, because the rule must hold before such an opt exists.
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

    /// A tier the corpus does not populate is an **error**, never a
    /// vacuous ∀ win. Same lesson as `MAX_SWEPT_DIMS`, from the other
    /// direction: a gate must not report success about nothing.
    #[test]
    fn sweeping_an_unpopulated_tier_is_an_error() {
        // Both tiers are populated on this tree, so drive the refusal
        // through the selector's own emptiness path with a name that
        // matches nothing.
        let e = compare_opt_lists_over_box_for_case(&[], RELEASE_OPTS, "cost-does-not-exist")
            .expect_err("must refuse");
        assert!(e.contains("no cost corpus case named"), "{e}");
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

    /// Decision 1453: NarrowImm alone wins on at least one cost-* case.
    #[test]
    fn narrow_imm_alone_wins_some_cost_case() {
        let corpus = discover_cost_cases();
        let mut wins = Vec::new();
        for case in &corpus {
            let dev = score_path_under_opts(&case.input, &[]);
            let alone = score_path_under_opts(&case.input, &[OptId::NarrowImm]);
            if alone < dev {
                wins.push(format!("{}[{}]: {alone} < {dev}", case.name, case.tier));
            }
        }
        apply_mode(CompileMode::Release);
        assert!(
            !wins.is_empty(),
            "NarrowImm alone must strictly lower ≥1 cost-* case; none fell"
        );
        eprintln!("NarrowImm alone wins:\n{}", wins.join("\n"));
    }

    /// **plans/M20.md item K — the NarrowImm finding, pinned.**
    ///
    /// The plan predicted that under the A76 ruler NarrowImm "may score near
    /// zero on cycles and win only on the footprint term", because
    /// `MOVZ`/`MOVK` are 1-cycle, throughput-3, port-I. Measured, the
    /// prediction is **inverted** and this oracle pins both halves of the
    /// inversion:
    ///
    /// 1. NarrowImm's win is **entirely on cycles** — it lowers the corpus
    ///    proxy total on its own, and (checked below) does so at every point
    ///    of the residual box in `unit:narrow_imm_alone_wins_at_every_box_point`.
    ///    It is not a latency win and never was: `load_imm` pushes each
    ///    `MOVK` with **no** source register, so the four-word materialization
    ///    is four *independent* 1-cycle uops, not a dependence chain. What
    ///    NarrowImm buys is dispatch and port-I **issue bandwidth**, bounded
    ///    above by one third of a cycle per deleted word (three I pipes).
    /// 2. NarrowImm's **footprint** win — which the plan expected to be the
    ///    whole story — is the half the gate cannot see. Hot text falls, and
    ///    the priced overflow `charge` is **0 on both sides of every case**,
    ///    because the cost-stage closure sits far inside its 64 KiB L1I. The
    ///    term that replaced the words veto (decision 1619) prices this
    ///    saving at exactly zero.
    ///
    /// So the retirement of the words veto did cost the gate a signal, and
    /// this oracle is where that is stated as a measurement: had NarrowImm
    /// been the footprint-only opt the plan expected, the gate would now be
    /// blind to it. It survives on cycles alone.
    #[test]
    fn narrow_imm_wins_on_cycles_while_its_footprint_win_is_priced_at_zero() {
        // **Derived from `RELEASE_OPTS`, not hand-listed** (item F). The
        // comment on `rankable` below already names the trap a hand-list
        // walks into — "silently becomes `release beats two of its six
        // opts`" — and the array underneath it was itself a hand-list, so
        // item F's three ids appeared in `rankable` and not in `configs`
        // and the lookup panicked. One source, both readers.
        let singles: Vec<(String, Vec<OptId>)> = RELEASE_OPTS
            .iter()
            .map(|id| (format!("{id:?}"), vec![*id]))
            .collect();
        let mut configs: Vec<(&str, &[OptId])> = vec![("dev", &[])];
        for (label, opts) in &singles {
            configs.push((label.as_str(), opts.as_slice()));
        }
        // The list as it stood before item F.
        let pre_f: Vec<OptId> = RELEASE_OPTS
            .iter()
            .copied()
            .take_while(|id| *id != OptId::InterprocRegs)
            .collect();
        configs.push(("release-minus-F", pre_f.as_slice()));
        // **The list every member of which is rankable alone** — the only
        // list the sum-of-singles bound below can honestly be asked over
        // (decision 1971). Kept in sync with `UNRANKABLE_ALONE`, asserted
        // there.
        let rankable_only: Vec<OptId> = RELEASE_OPTS
            .iter()
            .copied()
            .filter(|id| {
                !matches!(
                    id,
                    OptId::WideImmForms | OptId::InterprocRegs | OptId::Frameless
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

        // (1) Not "near zero on cycles".
        assert!(
            ni_cycles < dev_cycles,
            "NarrowImm alone must still lower the corpus proxy total: \
             dev {dev_cycles} -> NarrowImm {ni_cycles}\n{table}"
        );

        // (2) There is a real footprint win...
        let dev_hot = sum("dev", |c| c.hot_text_bytes);
        let ni_hot = sum("NarrowImm", |c| c.hot_text_bytes);
        assert!(
            ni_hot < dev_hot,
            "NarrowImm must still shrink hot text: {dev_hot} -> {ni_hot}\n{table}"
        );
        // ...and whether the **gate** can see it depends on the corpus, not
        // on the opt. Item K measured every case at `charge = 0` and
        // concluded the gate was blind to a footprint-only candidate. Item M
        // then added `cost-icache-cliff` and `cost-itlb-span`, which exist to
        // breach the budget, and the term became live: on those two cases
        // NarrowImm **lowers the priced charge** (5229 -> 2982 and
        // 24428 -> 18463). So the blindness was a property of the corpus and
        // the two witness cases fixed it (decision 1638).
        //
        // Both halves are asserted, because each is a real claim: the six
        // original cases still price the footprint win at zero, and at least
        // one case now prices it above zero and falls under NarrowImm.
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

        // (3) Which opt "carries the corpus" is a fact about the **corpus**,
        // not about the ruler. M20's evidence block credited `BoundsElide`
        // with 43.2% of release's cycle win across the fifteen micro
        // cases; item H then measured it byte-identical to `dev` on all
        // four programs the appliance ships, and item L deleted it
        // (decision 1970). That 43.2% was a fact about six fixtures, and
        // the disjointness claim that used to sit here — "NarrowImm is the
        // sole mover wherever BoundsElide is flat" — went with it.
        //
        // So the assertion is the one that is actually about the ruler:
        // every rankable opt contributes, none is inert, and their sum
        // bounds release's (they overlap rather than compose freely).
        //
        // The singles are derived from `RELEASE_OPTS` rather than written
        // out, because both item C and item E hit the same trap: a
        // hand-listed pair stops testing the claim the moment the list
        // grows, and silently becomes "release beats two of its six opts".
        //
        // `WideImmForms` is the one member that cannot be ranked against
        // `dev` at all (decision 1747): with `NarrowImm` off, `load_imm`
        // returns to `load_imm_naive` before C5's one-word forms are ever
        // reached, so `[] -> [WideImmForms]` is the identity comparison and
        // its "win" is zero for reasons that have nothing to do with C5. It
        // is excluded here by name and gated on its own baseline in
        // `ITEM_C_SMOKE`. The exclusion is asserted to be exactly that one
        // id, so a future opt cannot join it silently.
        //
        // **Item F adds two more, for the same structural reason.**
        // Neither is a transform of its own against `dev`:
        // `InterprocRegs` changes which register the allocator may pick
        // and `Frameless` is read off the allocation's own result, so
        // with `RegAlloc` off both are the identity. Each is gated on its
        // real baseline in `ITEM_F_SMOKE`, which is a chain rather than a
        // single baseline precisely because they compose in this order.
        const UNRANKABLE_ALONE: &[OptId] =
            &[OptId::WideImmForms, OptId::InterprocRegs, OptId::Frameless];
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
        // **The sum-of-singles bound, restated by item F (decision 1777)
        // and corrected by item L (decision 1971).**
        //
        // Through item E the claim was `rel_win <= Σ singles`: the opts
        // overlap rather than compose, so none of them creates cycles
        // that none creates alone. Item F breaks that, and the break is
        // the finding rather than a bug. Three of its four ids
        // (`InterprocRegs`, `Frameless`, `TailCalls`) are *unreachable*
        // against a `dev` baseline — none of them is a transform of its
        // own, each only fires once the one before it has — so their
        // singles are all exactly zero while their joint contribution is
        // not. Measuring `Σ singles` against `release` therefore compares
        // a sum that is missing three terms with a total that has them.
        //
        // **On the whole corpus the bound is now false, and that is the
        // finding (decision 1971).** Deleting `BoundsElide` did not create
        // the violation; it removed what was hiding it. `BoundsElide` was
        // byte-identical on all four product cases, so it contributed
        // exactly 0 to both sides of the product-tier arithmetic, while on
        // the micro tier its 4592-cycle single was strongly sub-additive
        // inside the list. That micro slack covered a product-tier excess
        // that was already there.
        //
        // Measured, over the list every member of which really is
        // rankable alone (`release-minus-F` is *not* such a list — it
        // carries `WideImmForms`, the id `UNRANKABLE_ALONE` names below):
        //
        // | tier | joint win | Σ singles | excess |
        // | --- | ---: | ---: | ---: |
        // | micro | 35 349 | 35 375 | **−26** |
        // | product | 2 445 | 2 397 | **+48** |
        //
        // and the +48 is `cost-product-blk` (+29) and
        // `cost-product-receipt` (+27), the two largest borrowed programs.
        // Localized to one **pair**: `NarrowImm` + `RegAlloc` beats the
        // sum of its own two singles by +33 and +31 on those two cases,
        // and every other pair of rankable ids is flat or sub-additive.
        // The mechanism is the obvious one — `NarrowImm` turns a four-word
        // `movk` chain into one word, and the allocator then keeps live
        // what the vanished chain's scratch pressure used to spill; each
        // opt alone leaves the other's saving on the table.
        //
        // So the bound is asserted on the tier where it was established
        // and still holds, and the tier where it fails is *localized* by a
        // positive claim rather than absorbed by a tolerance. Weakening
        // this to `Σ singles + fudge` would be exactly the tuning this
        // round forbids.
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
        // **And item I took the micro tier too** (decision 1906). Item L
        // could still assert the bound here, because the allocator it
        // measured was item E's: one that *relocated* spill traffic into
        // `mov`s. Coalescing and argument/return hinting delete that
        // traffic instead, and the interaction L localized below —
        // `NarrowImm` frees the scratch pressure the allocator then keeps
        // live — gets strictly stronger when keeping a value live costs no
        // instruction at all. So the sum of singles now undercounts on
        // *both* tiers.
        //
        // Not relaxed into a tolerance: the bound is gone, so it is
        // reported as the measurement it now is, and the *positive* claim
        // below — that the excess is one identified pair on a named case —
        // is what carries the weight. A bound nobody can state is worth
        // less than a mechanism somebody can check.
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

        // The localization, asserted rather than asserted-away: the one
        // super-additive pair is `NarrowImm` + `RegAlloc`, on the largest
        // borrowed program. If this interaction stops existing, the table
        // above stops describing anything and the comment must be redone.
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

    /// **plans/M20.md item K.** The cycle half of the finding above, under
    /// `∀` rather than at the pinned point: NarrowImm alone falls at **every**
    /// point of the residual box, on every case. That is the evidence that the
    /// win is a dispatch/issue-bandwidth effect and not a latency or memory
    /// one — the box varies every bracketed latency the model has, and on five
    /// of six cases NarrowImm's delta does not move across it at all.
    /// **Deep lane.** `#[ignore]`d by default and run by
    /// `xtask::deep_lane`, which `cargo xtask check` calls — matching how
    /// every `fuzz_*` lane already splits a smoke budget from a deep one
    /// (`crates/xtask/src/main.rs`). Measured 2026-07-31, after item H
    /// widened the corpus: **36 352 points per side** across 19 cases —
    /// 15 micro and 4 product — for **×1.78** the deep lane's ∀ work, which
    /// did not show up as wall clock: 309–397 s (n=3) against 411 s (n=1)
    /// before (see [`MAX_SWEPT_DIMS`] for why, and for why the work and not
    /// the clock is the stated number). That is not a cost the default `cargo test` loop
    /// should carry; CLAUDE.md separates the
    /// cheap per-item lane from the expensive close lane, and a
    /// whole-corpus ∀ gate belongs in the latter. Nothing about the
    /// oracle's strength changed — only which lane runs it.
    #[ignore = "deep lane: run via `cargo xtask check` (or --ignored)"]
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

    /// **plans/codegen-pareto.md item B1, the land gate — smoke form.**
    ///
    /// `AdrAddressing` alone, over one case, at every point of that case's
    /// residual box. `cost-arith` is the smoke case: it is small (147
    /// proxy-cycles under release) and it emits six rodata references from
    /// its checked-arithmetic abort stubs, so the substitution is the only
    /// thing separating the two sides. Freeze 1714 in one sentence — the
    /// oracle exercises the new path or it is not an oracle, and this one
    /// scores zero delta if `load_rodata_addr` stops substituting.
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
            case.points.iter().all(|p| p.candidate < p.baseline),
            "AdrAddressing must fall at every point of {}: {:?}",
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

    /// The same gate asked the question that actually decides the landing:
    /// not "is `AdrAddressing` a win against `dev`" but "does adding it to
    /// the already-shipping list still fall". A candidate that only wins
    /// from a `dev` baseline could be riding another opt's coattails; this
    /// one holds `NarrowImm` fixed on both sides.
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

    /// **plans/codegen-pareto.md item B1, the land gate — whole corpus.**
    /// `RELEASE_OPTS` minus `AdrAddressing` vs `RELEASE_OPTS`, ∀ over the
    /// residual box, over every `cost-*` case. **Deep lane**, same budget
    /// argument as its two neighbours above.
    #[ignore = "deep lane: run via `cargo xtask check` (or --ignored)"]
    #[test]
    fn adr_addressing_wins_at_every_point_of_the_residual_box() {
        const WITHOUT: &[OptId] = &[OptId::NarrowImm];
        let cmp = compare_opt_lists_over_box(WITHOUT, RELEASE_OPTS).expect("sweep");
        let table = format_sweep_table(&cmp, "release−AdrAddressing", "release");
        eprintln!("∀ sweep (release−AdrAddressing → release):\n{table}");
        assert_sweep_wins(&cmp, "release", "release−AdrAddressing");
        assert!(cmp.wins());
        eprintln!(
            "AdrAddressing ∀-sweep: {} points/side over {} cases",
            cmp.scored_points(),
            cmp.cases.len()
        );
    }

    /// **Deep lane. Decision 1784 — every `RELEASE_OPTS` member is
    /// re-asked, alone, on the product tier.**
    ///
    /// Decision 1717 says an opt may not gate on a case it authored alone.
    /// That sentence has no force unless somebody actually re-runs the
    /// landing question over the borrowed programs, so this is that run:
    /// for each opt in `RELEASE_OPTS`, `dev` vs `[opt]` over the product
    /// tier only, ∀ across each case's residual box.
    ///
    /// Product tier only, deliberately. The whole-corpus sweep above
    /// already covers both tiers for the list as a whole; repeating it per
    /// member would triple a lane that item H already measured at
    /// 411 s → 597 s. What is not covered anywhere else is the *member's*
    /// verdict on the programs it did not choose, and that is 4 cases
    /// rather than 19.
    #[ignore = "deep lane: run via `cargo xtask check` (or --ignored)"]
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
            // Each member is asked over **its own** baseline, not blindly
            // over `dev`. `WideImmForms` is the one member for which
            // `dev -> [it]` is the *identity* comparison (decision 1747):
            // with `NarrowImm` off, `load_imm` returns to `load_imm_naive`
            // before C5's one-word forms are ever reached, so a `dev`
            // baseline would record a veto that says nothing about the
            // product tier and everything about the question being
            // unanswerable. Asking the wrong question and pinning the
            // answer is worse than not asking.
            //
            // **Item F's two ids need the same treatment, and for the
            // same reason** (decision 1792). Neither is a transform of
            // its own: `InterprocRegs` changes which register the
            // allocator may hand out, and `Frameless` is read off the
            // allocation's own result, so with `RegAlloc` off both are
            // the identity and a `dev` baseline would pin a veto that
            // says nothing. Each is asked over its own link in item F's
            // chain, derived from `RELEASE_OPTS` rather than written out.
            //
            // `WideImmForms` was widened from `[NarrowImm]` to
            // `[NarrowImm, RegAlloc]` by decision 1791, and the reason is
            // the same mechanism one item earlier: C5 cannot be ranked
            // against `dev` at all (1747), and on the product tier it
            // cannot be ranked without `RegAlloc` either, because its
            // saving is *words* and words only become cycles once the
            // allocator has removed the schedule slack that absorbs them.
            // Both halves are measured in plans/codegen-pareto-C.md. That
            // baseline was widened *after* a red test, so it is not what
            // the membership claim rests on — leave-one-out is, and it has
            // no baseline freedom at all.
            let base: Vec<OptId> = match opt {
                OptId::WideImmForms => vec![OptId::NarrowImm, OptId::RegAlloc],
                OptId::InterprocRegs => item_f_baseline(),
                OptId::Frameless => {
                    let mut b = item_f_baseline();
                    b.push(OptId::InterprocRegs);
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
             - There is **no `BoundsElide` row**. Item H measured it \
             byte-identical to `dev` on all four product cases — same \
             cycles, same emitted words, same hot text — and \
             plans/codegen-pareto-2.md item L deleted the opt (decision \
             1970). A `veto` row that stays a `veto` forever is an opt \
             kept disabled, and losers are deleted.\n\
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

    /// **Decision 1785.** The measured per-tier verdict of each
    /// `RELEASE_OPTS` member, standing alone, over the product tier —
    /// pinned so a change in either direction is loud. See
    /// `unit:each_release_opt_is_re_asked_alone_on_the_product_tier` for
    /// what the two rows mean and why a `veto` row is a finding rather
    /// than a broken gate.
    ///
    /// **Every row here is `wins` (decision 1970).** The one `veto` row
    /// this set ever carried was `BoundsElide`'s, and item L deleted the
    /// opt rather than leave a permanent veto in the product's own list.
    /// A future `veto` row is therefore a finding to act on, not a shape
    /// this table is expected to have.
    const PINNED_PRODUCT_TIER_VERDICTS: &[(&str, &str)] = &[
        ("NarrowImm", "wins"),
        ("AdrAddressing", "wins"),
        ("BfxNarrow", "wins"),
        ("MaskCheck", "wins"),
        // **Item H's second finding, now resolved — decision 1791.** Over
        // `[NarrowImm]` this was `veto`: Δ = +0 at all 10 240 product-tier
        // points. Item H's hypothesis — that `MaskCheck` deletes the
        // constant materializations C5 would shorten — was **measured and
        // is true**: on the four shipped programs `MaskCheck` removes 4 of
        // C5's 7 customers, every bitmask-immediate one, leaving three
        // `MOVN`s on two programs.
        //
        // But that is not why the verdict was `veto`. The missing
        // ingredient was `RegAlloc`, not `MaskCheck`: C5's saving is
        // words, and words become cycles only once the allocator removes
        // the slack that absorbs them — the same crossover item C1 hit.
        // Asked over `[NarrowImm, RegAlloc]`, the configuration the
        // product actually ships, C5 falls at **every** one of the 10 240
        // points on `cost-product-blk` and `cost-product-receipt` and
        // rises nowhere. Leave-one-out against the whole shipped list
        // gives the identical verdict on the identical two cases.
        ("WideImmForms", "wins"),
        ("RegAlloc", "wins"),
        // **plans/codegen-pareto.md item F, decision 1780.** Both win on
        // all four borrowed programs, which is the question decision 1717
        // exists to ask: neither is carried by a microbenchmark it wrote
        // itself. At the pinned point each of the four product cases
        // falls by **-47** under `InterprocRegs` and by a further
        // **-108** under `Frameless` — the same number on every one of
        // them, because what both delete lives in the shared runtime
        // closure all four borrow rather than in any one program.
        //
        // F5 (tail calls) has no row because it has no id: the gate
        // scores it at exactly zero on all twenty cases of both tiers
        // (decision 1779, `unit:f5_has_no_opt_id` and the deep lane
        // beside it).
        ("InterprocRegs", "wins"),
        ("Frameless", "wins"),
        // **plans/codegen-pareto-2.md item L, decision 1976.** B4 is a
        // transform of its own against `dev` — nothing else has to fire
        // first for a trailing branch to be a trailing branch — so unlike
        // item F's two it is rankable alone, and it falls on all four
        // borrowed programs.
        ("BranchCleanup", "wins"),
    ];

    /// Decision 1453: swapped opt-list order vs RELEASE_OPTS — document
    /// independence when totals match (lower vs codegen axes).
    #[test]
    fn swapped_order_scores_same_as_release_opts() {
        // The **reverse** of the live list, not a hand-written pair: the
        // claim is that the order of the slice cannot change a score,
        // and a hardcoded pair stops testing that the moment the list
        // grows (plans/codegen-pareto.md item C added four ids, and the
        // stale pair silently turned this into a "release beats two of
        // its six opts" test instead).
        let swapped: Vec<OptId> = RELEASE_OPTS.iter().rev().copied().collect();
        assert_ne!(
            swapped.as_slice(),
            RELEASE_OPTS,
            "the reversal must actually reorder something"
        );
        let cmp = compare_opt_lists(RELEASE_OPTS, &swapped);
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

    /// Dropping every opt but `NarrowImm` from the shipped list raises
    /// cases the dropped opts carry — the candidate oracle must refuse it.
    #[test]
    #[should_panic(expected = "raised proxy total")]
    fn disabling_shipped_opts_fails_candidate_oracle() {
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

    // -----------------------------------------------------------------------
    // Decision 1619 / freeze 1626: the words veto retired, the per-core
    // budget delta installed in its place.
    // -----------------------------------------------------------------------

    /// A synthetic `CoreBudget`. `over_itlb_pages == 0` carries item F's
    /// measured 23 text pages against the 48-entry L1 I-TLB.
    fn budget(n: usize, over_l1i_lines: u64, over_itlb_pages: u64, charge: u64) -> CoreBudget {
        CoreBudget {
            n,
            hot_text_bytes: 91712,
            hot_code_bytes: 84284,
            packing_floor_lines: 1318,
            slack_lines: 115,
            l1i_bytes: 65536,
            over_l1i_lines,
            over_l2_lines: 0,
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

    /// **The replacement fires when an overflow rises.** Every cycle number
    /// falls; one core needs one more line than it did.
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

    /// Every watched quantity is watched, not just the first — including
    /// the priced `charge`, which is the only one that moves with the sweep
    /// point.
    #[test]
    fn overall_budget_veto_watches_every_over_quantity() {
        let set = pinned_set();
        let base_b = budget(0, 409, 0, 2863);
        let fields = [
            "over_l1i_lines",
            "over_l2_lines",
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

    /// **Decision 1619's counter-example.**
    ///
    /// These are the **image** program's budgets, not the cost-stage
    /// closure's — item F measured every core of every `boot-*` case
    /// already 409–413 lines over its 64 KiB L1I under `W_flat`. Nothing
    /// constrains this gate's inputs to be cost-stage closures, and handed
    /// these an absolute `CoreBudget::within_budget()` veto fires on the
    /// **identity**, refusing a program compared against itself and
    /// therefore every candidate there will ever be. The delta reading does
    /// not fire, which is why an absolute whole-budget veto is not the rule
    /// that replaces the words veto.
    #[test]
    fn an_over_budget_identity_is_refused_absolutely_and_allowed_as_a_delta() {
        let set = pinned_set();
        // The three cores item F reported on `boot-cores-3`.
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
        // The identity: same budgets on both sides.
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
        // And one line more on any core is still refused.
        let mut worse = boot.clone();
        worse[0].over_l1i_lines += 1;
        let cmp =
            compare_overall(&side, &better.clone().with_budgets(worse), &set).expect("compare");
        assert!(
            cmp.vetoed(),
            "growth from an already-over baseline still vetoes"
        );
    }

    /// **Decision 1636.** The absolute I-TLB veto is retired, so an
    /// over-span *baseline* no longer refuses everything — but a candidate
    /// that **worsens** the span is still refused, by the delta rule, which
    /// watches `over_itlb_pages` among its seven quantities. Both halves are
    /// asserted here, because retiring a rule is only safe if what replaces
    /// it still fires.
    #[test]
    fn an_over_itlb_baseline_is_allowed_but_worsening_the_span_is_refused() {
        let set = pinned_set();
        // Baseline already over the 48-entry I-TLB (the `cost-itlb-span`
        // shape). Candidate is no worse. Under the retired absolute rule
        // this was refused; under the delta it ranks.
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
        // Now worsen it by one page: the delta rule must fire.
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

    /// A core-count change is not a rank: the two sides were placed on
    /// different machines.
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

    /// **Freeze 1626, the retirement half.** Word growth alone is no longer
    /// a veto — it is a reported column — provided the budgets hold.
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
        // And no veto reason mentions words at all.
        for r in cmp.veto_reasons() {
            assert!(!r.label().contains("words_grew"), "{:?}", r);
        }
    }

    /// **Decision 1617.** The measured row prints its coverage fraction
    /// beside its own number, so a 13.4%-covered total cannot be read as a
    /// measured result about the program.
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
        // The flat row is not a measured row and claims no coverage.
        let flat = table
            .lines()
            .find(|l| l.starts_with("flat"))
            .expect("flat row");
        assert!(!flat.contains('%'), "flat is not a measured row: {flat}");
    }

    /// All firing conditions are collected, not short-circuited.
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
        // rise + coverage + two budget quantities (lines and charge).
        assert_eq!(cmp.veto_reasons().len(), 4, "{:?}", cmp.veto_reasons());
    }

    // -----------------------------------------------------------------------
    // Freeze 1633: the barrier-removal refusal (plans/M20.md item G)
    // -----------------------------------------------------------------------

    /// Ordering counts for a single-fn program. `OrderingCounts` is keyed
    /// per fn (freeze 1633 is about *where* an ordering word is), so these
    /// fixtures name one fn and vary the counts within it.
    fn ord(pairs: &[(&'static str, u64)]) -> OrderingCounts {
        pairs
            .iter()
            .map(|&(rule, n)| (("f".to_string(), rule), n))
            .collect()
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
        let side = |ordering: OrderingCounts| {
            totals(&[("flat", 900), ("boot-actors", 4000)]).with_ordering(ordering)
        };
        let baseline =
            totals(&[("flat", 1000), ("boot-actors", 5000)]).with_ordering(base_ord.clone());
        // Identical counts: no refusal, ordinary win.
        let same = compare_overall(&baseline, &side(base_ord.clone()), &set).expect("cmp");
        assert!(!same.vetoed() && same.wins());
        // Adding is fine.
        let mut more = base_ord.clone();
        more.insert(("f".to_string(), "barrier"), 7);
        let added = compare_overall(&baseline, &side(more), &set).expect("cmp");
        assert!(!added.vetoed() && added.wins());
        // Dropping any one of the four is refused, and every dropped rule
        // is reported rather than only the first.
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

    /// **Freeze 1633 on the corpus gate.** A case whose cycles fall while
    /// its barrier count drops is not a win, and `assert_wins` says why.
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

    /// **Item G's coverage gap is closed** (plans/M20.md item M). Item G
    /// reported that no `cost-*` case reached a `barrier` /
    /// `load_acquire` / `store_release` / `system` word at all, so freeze
    /// 1633's refusal was live but inert on the live corpus. The
    /// `cost-crosscore` golden reaches all four, so this test now asserts
    /// the opposite of what it used to: **some** case must carry ordering
    /// words, every case must still carry a slot for each of the four
    /// rules, and `release` must remove none of them anywhere.
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
            // Keyed per fn: every fn carries a slot for each of the four
            // rules, so the map is 4 x the case's fn count.
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

    /// **Decision 1954, stated as a set rather than as a claim.** Every
    /// case whose root declares an `@image` is scored as the image the
    /// appliance would ship; every other is scored as its closure and says
    /// so. The two lists are pinned so a case silently changing sides — a
    /// root losing its `@image`, or the image build starting to fail and
    /// falling back — is a failure rather than a quiet re-tiering.
    ///
    /// This is the K2 regression test: on the old behaviour every row here
    /// was a closure, including the flagship, and `hot_text_bytes` on the
    /// gate's side of the appliance read 7 936 B against the 89 024 B
    /// `--stage=report` printed for the same root.
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
                "cost-product-receipt",
            ],
            "the image-bearing cases changed"
        );
        assert_eq!(closure.len(), cmp.cases.len() - image.len());
        assert!(
            !closure.contains(&"cost-product-appliance"),
            "the flagship must never be ranked as a closure again"
        );
        // Every product-tier case ships an image: the tier exists to ask
        // the gate about programs the appliance runs, and a product case
        // that ranked as a closure would not be doing that.
        for c in cmp.cases.iter().filter(|c| c.tier == CostTier::Product) {
            assert_eq!(c.scope, TextScope::Image, "{}", c.name);
        }
        apply_mode(CompileMode::Release);
    }

    /// **Freeze 1626 on the corpus gate, both halves in one test.** Words
    /// are still a column and are recorded here; the condition the corpus
    /// gate now enforces on the same run is the per-core budget delta.
    ///
    /// The rule's coverage on this corpus is stated rather than assumed,
    /// and **plans/codegen-pareto-2.md decision 1954 changed what that
    /// statement is**. The gate used to score the cost-stage closure, which
    /// fits its L1I on every case but the two item M built to breach one:
    /// the constraint was live and almost entirely silent, and item H
    /// recorded that as "the budget rule is inert on real programs". It was
    /// inert on the *wrong* program. Scoring the image each root would
    /// actually ship, the claim inverts and becomes a clean one:
    ///
    /// * **every shipped image is over its L1I; no closure is.** 89–391 KB
    ///   of hot text against 64 KiB, on all eight image-bearing cases and
    ///   on both cores of the two-core ones. `within_budget()` is false on
    ///   exactly the `TextScope::Image` rows.
    /// * the **delta** rule still holds everywhere (no core's overflow
    ///   grows from `dev` to `release`) — that is the rule freeze 1626
    ///   installed and it is what this gate enforces. It is now doing real
    ///   work: `release` takes the flagship from `charge = 6132` to 2569.
    ///
    /// This is also the tree's live demonstration of decision 1619's
    /// central point, now on the whole product tier rather than on two
    /// fixtures — an *absolute* whole-budget veto would refuse the identity
    /// of every program the appliance ships, while the delta rule ranks
    /// them fine.
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
                // plans/M20.md item M's two-core image-bearing cases.
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
            for b in c.baseline_budgets.iter().chain(c.candidate_budgets.iter()) {
                if ships {
                    assert!(
                        !b.within_budget(),
                        "{}: every image this tree ships is over its 64 KiB L1I; if \
                         that stops being true it is the biggest result on this plan \
                         and must be reported, not absorbed: {}",
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

    /// **The retirement itself, structurally.** A candidate that grows the
    /// word count while lowering cycles wins the corpus gate now. Under the
    /// pre-J rule this exact shape was refused; the new refusal is on the
    /// budget, and this case's budgets are unchanged.
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
        // …and the same case with one more overflowing line is refused.
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

    // -----------------------------------------------------------------------
    // The ∀ sweep (plans/M20.md item J, decision 1604, freeze 1624)
    // -----------------------------------------------------------------------

    /// **Freeze 1624, checked structurally rather than by comment.**
    ///
    /// The `∃` form ("does this candidate win at *some* point") must not be
    /// expressible through this module's public surface. The check reads
    /// this file's own source and pins the set of public `bool`-returning
    /// functions: a new per-point predicate — `wins_at`, `wins_anywhere`, a
    /// `PointRow::wins` — cannot be added without failing here, which is
    /// the only kind of freeze that survives a rewrite.
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
                "wins_in_tier".to_string(),
            ],
            "the public predicate set changed. The three `wins` are the ∀ \
             verdicts on CorpusCompare / OverallCompare / SweepCompare; \
             `vetoed`, `rises` and `is_flat` are row facts. `wins_in_tier` \
             (item H, decision 1782) is a fourth ∀ verdict and not a fourth \
             kind of predicate: its argument is a CostTier — a slice of the \
             *corpus*, fixed on disk — and it still quantifies over every \
             point of every case in that slice. Anything else — in \
             particular anything taking a SweepPoint or a PointRow and \
             answering yes/no — is the ∃ form freeze 1624 refuses."
        );
        // No public signature may take a point and answer a verdict.
        for line in src.lines() {
            let t = line.trim();
            if t.starts_with("pub fn") && t.contains("SweepPoint") {
                assert!(
                    !t.contains("bool"),
                    "a public per-point predicate appeared: {t}"
                );
            }
        }
        // And PointRow carries data, not a verdict.
        assert!(
            !src.contains("impl PointRow {\n    pub fn wins"),
            "PointRow must not answer whether the candidate won here"
        );
    }

    /// The nominal box is 2^17 = 131072 endpoint corners, and the plan's
    /// "reduce by bracket endpoints only" is already that number — which is
    /// why item J needs the per-case sensitivity probe.
    #[test]
    fn the_residual_box_has_two_to_the_seventeen_endpoint_corners() {
        let table = load_default().expect("committed profile");
        assert_eq!(table.sweep_dimensions().len(), 17);
        assert_eq!(box_cardinality(&table), 131_072);
    }

    /// **Smoke lane: the ∀ sweep on one case, in the default `cargo test`.**
    ///
    /// The whole-corpus sweep below is `#[ignore]`d and run by
    /// `cargo xtask check` (decision 1637). This keeps the property under
    /// test on every ordinary run, through the *identical* code path — same
    /// probe, same corners, same refusals — over `cost-bounds-elide`, the
    /// case with the largest and most stable delta (1839 → 314 at the pinned
    /// point), so a real regression in the sweep machinery cannot hide until
    /// close.
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
        // The nominal box is still reported even though one case is swept:
        // a reader must see what the enumerated corners stand for.
        assert_eq!(case.box_cardinality, 131_072);
    }

    /// **The live ∀ sweep: `release` vs `dev`.** Records the per-point
    /// table, the nominal box cardinality and the surviving `k` per case.
    /// **Deep lane.** `#[ignore]`d by default and run by
    /// `xtask::deep_lane`, which `cargo xtask check` calls — matching how
    /// every `fuzz_*` lane already splits a smoke budget from a deep one
    /// (`crates/xtask/src/main.rs`). Measured 2026-07-31, after item H
    /// widened the corpus: **36 352 points per side** across 19 cases —
    /// 15 micro and 4 product — for **×1.78** the deep lane's ∀ work, which
    /// did not show up as wall clock: 309–397 s (n=3) against 411 s (n=1)
    /// before (see [`MAX_SWEPT_DIMS`] for why, and for why the work and not
    /// the clock is the stated number). That is not a cost the default `cargo test` loop
    /// should carry; CLAUDE.md separates the
    /// cheap per-item lane from the expensive close lane, and a
    /// whole-corpus ∀ gate belongs in the latter. Nothing about the
    /// oracle's strength changed — only which lane runs it.
    #[ignore = "deep lane: run via `cargo xtask check` (or --ignored)"]
    #[test]
    fn release_wins_at_every_point_of_the_residual_box() {
        let cmp = compare_opt_lists_over_box(&[], RELEASE_OPTS).expect("sweep");
        let table = format_sweep_table(&cmp, "dev", "release");
        eprintln!("∀ sweep (dev → release):\n{table}");
        for c in &cmp.cases {
            assert_eq!(c.box_dims, 17);
            assert_eq!(c.box_cardinality, 131_072);
            assert_eq!(
                c.points.len(),
                1usize << c.swept.len(),
                "{}: corners must be 2^k",
                c.name
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

    // ---------------------------------------------------------------
    // plans/codegen-pareto.md item C: one ∀ gate per arithmetic opt
    // (decision 1745). Each id is ranked **alone against `dev`**, so a
    // refusal names one transform rather than the bundle.
    // ---------------------------------------------------------------

    /// One row per item-C opt: `(baseline, opt, smoke case)`.
    ///
    /// The **case** is the one whose shapes the transform actually
    /// reaches; a smoke lane pointed at a case the opt does not touch is
    /// the green-unit-that-is-not-an-oracle freeze 1714 forbids, so the
    /// pairing is written down rather than defaulted.
    ///
    /// The **baseline** is `dev` except for `WideImmForms`, which is
    /// gated against `[NarrowImm]` (decision 1747). That is not a softer
    /// gate, it is the only gate that means anything for this opt:
    /// `load_imm` returns to `load_imm_naive` before it ever reaches
    /// C5's one-word forms when `NarrowImm` is off, so `[] → [WideImmForms]`
    /// is the identity comparison and would "pass" or "fail" for reasons
    /// that have nothing to do with C5. Item C5 is named in the plan as
    /// "the `NarrowImm` sequel"; this is what that composition means when
    /// the gate has to rank it.
    const ITEM_C_SMOKE: &[(&[OptId], OptId, &str)] = &[
        // Narrow checked `+`/`-`/`*` and the narrowing `.to[T]()`.
        (&[], OptId::MaskCheck, "cost-arith"),
        // `narrow_to_width` — every wrapping narrow op. `cost-arith`'s own
        // shapes move only in words, so the ranked case is the one whose
        // cycles move.
        (&[], OptId::BfxNarrow, "cost-runtime"),
        // The signed bounds constants and the `MIN`/`-1` divide guard.
        //
        // **Baseline widened to include `RegAlloc` at decision 1791**, and
        // this is the one row where the baseline is load-bearing, so the
        // reasoning is here rather than in the findings alone.
        //
        // Asked over `[NarrowImm]` this opt **fails** on the product
        // tier — `no_case_falls_everywhere`, which is what turned this
        // lane red on master once item H made the product tier part of the
        // box. That result is real and is not being papered over; it is
        // reproduced and explained in plans/codegen-pareto-C.md. What it
        // means is that `[NarrowImm]` is now the wrong baseline for the
        // same reason `dev` was wrong for it at decision 1747: it asks the
        // question in a configuration the product does not ship. C5's
        // saving is **words**, and words only become cycles when the
        // schedule has no slack left to absorb them. `RegAlloc` is what
        // removes that slack, and `RegAlloc` ships.
        //
        // The baseline was not chosen by hunting for a green one. The
        // *strictest* membership question available — leave-one-out
        // against the entire shipped list, where C5 has to beat every
        // other opt including the two that delete most of its customers —
        // gives the identical verdict on the identical two cases, and is
        // pinned separately in
        // `unit:item_c5_earns_its_place_by_leave_one_out_on_the_product_tier`
        // so this row's baseline cannot be what carries the claim.
        (
            &[OptId::NarrowImm, OptId::RegAlloc],
            OptId::WideImmForms,
            "cost-mpipe-block",
        ),
    ];

    /// **Decision 1791: C5's membership claim, asked the hardest way.**
    ///
    /// `ITEM_C_SMOKE` ranks each opt over a chosen baseline, and a chosen
    /// baseline is exactly the kind of freedom that can flatter an opt. So
    /// C5's place in `RELEASE_OPTS` is pinned here instead, by the one
    /// question that has no such freedom: **remove it from the shipped
    /// list and see whether the shipped list gets worse.**
    ///
    /// This is strictly harder than the alone-gate. C5 must beat the whole
    /// rest of `RELEASE_OPTS`, including `MaskCheck`, which deletes 4 of
    /// its 7 constant-materialization customers on the four programs the
    /// appliance ships — every bitmask-immediate one, leaving only three
    /// `MOVN`s across two of the four. It still falls at **every** point
    /// of the product box on both of those two, and rises nowhere.
    ///
    /// If this ever stops holding — most plausibly because `MaskCheck`'s
    /// coverage grows and eats the remaining `MOVN` customers — C5 has
    /// become dead weight and the doctrine is to delete it, not to look
    /// for a baseline where it still wins.
    #[ignore = "deep lane: run via `cargo xtask check` (or --ignored)"]
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

    /// Item C's attribution table, printed for
    /// `plans/codegen-pareto-C.md`: each opt alone against `dev`, at the
    /// pinned point, in cycles **and** in words.
    ///
    /// Not a gate — [`attribute_opts`]'s own doc says why it exposes no
    /// verdict. It exists because the gate ranks on cycles alone, and an
    /// opt whose whole effect is words needs a place where that effect is
    /// *visible* rather than a place where it reads as nothing.
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
        // The corpus must actually contain a customer for each opt, or the
        // table above is four columns of the identity (freeze 1714).
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

    /// **Why item C1 scores zero, measured rather than asserted**
    /// (plans/codegen-pareto-C.md, decision 1746).
    ///
    /// Decision 1740 predicted C1 would be unrankable because the table
    /// had no W-form row, and made adding one C1's job. The row is added,
    /// with T1 provenance; W-form multiplies *are* emitted and *are*
    /// priced at their own latency — and `WFormMul` still moves the total
    /// by exactly zero on every case in the corpus. So the missing row was
    /// not the reason, and this test finds the reason that is.
    ///
    /// It ablates the **X-form** multiply row upward, one cycle at a time,
    /// and reports the latency at which the substitution first becomes
    /// visible. Below that threshold the difference between an X-form
    /// multiply and a W-form one is absorbed by slack the block already
    /// has: under `compiler.codegen.naive-locked`'s spill-everything
    /// frame every operand arrives from a 4-cycle frame load and every
    /// result leaves through a store, so the two L pipes bound the block
    /// and the M pipe has cycles to spare. The committed X-form numbers
    /// (lat 4, hold `ceil(3/1) + 2 = 5`) fit inside that slack; the W-form
    /// numbers (lat 2, hold 1) fit inside it too, and two quantities that
    /// both fit under the same bound are the same number to a block
    /// schedule.
    ///
    /// The threshold is the useful output, not the zero: it says how much
    /// headroom the frame convention is currently donating, and therefore
    /// how much of item C1's win item E has to unlock before the row added
    /// here starts to pay. `cost-mpipe-block`'s golden recorded the same
    /// shape for the X-form *stall* in M20 ("the frame's own load/store
    /// traffic already spaces the multiplies more than five cycles
    /// apart"); this is that observation carried to the latency, on the
    /// case built to carry item C1.
    ///
    /// The test also refuses the *other* failure: if no inflation made any
    /// difference, the multiply term would be inert in the model and the
    /// zero would mean nothing at all.
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

        // One program, the one the compiler actually emits. The
        // substitution is priced by moving the **row**, not the opt:
        // C1 is unconditional (decision 1746), so there is no second
        // program to compare against, and pricing the emitted W-form
        // words at the X-form row is exactly the counterfactual anyway.
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
            score_side_at(&side, &table, &p).expect("score").cycles
        };

        // The committed W-form row, and the same words priced at the
        // X-form row this item replaced (SOG §3.6: lat 4, thru 1/3,
        // note 4's 2-cycle M-pipe stall).
        let w_form = score_with_mul_w_at(2, 1, 0);
        let x_form = score_with_mul_w_at(4, 3, 2);

        // **The crossover item C predicted, arriving on schedule.** On item
        // C's own tree these two were equal: the spill-everything frame
        // donated 6 cycles of M-pipe slack per multiply, against the 2 the
        // substitution saves, so C1 scored exactly zero and decision 1746
        // kept it out of `RELEASE_OPTS` as an unrankable form change. Item
        // E's allocator removed that slack, and C1 became visible in the
        // same merge that landed the allocator — which is precisely what
        // item C's findings said would happen ("C1's payoff is gated on
        // item E, not on the ruler").
        //
        // Promoting C1 to a ranked `OptId` was item C's follow-up, and the
        // answer is **no** — decision 1790. Not because it is unrankable
        // any more (it is: 3 cycles here), but because of *which* case
        // falls. `cost-arith-w` is the only case in either tier that moves
        // at all, item C wrote it, and C1 is worth exactly zero on all
        // four programs the appliance ships. Freeze 1717 forbids gating on
        // a case the opt authored alone, and that is the whole gate C1
        // would have had. See plans/codegen-pareto-C.md.
        //
        // What this pins is the *direction and size* of the crossover, so
        // it cannot quietly reverse.
        assert!(
            w_form < x_form,
            "item C1 has stopped being visible again: W-form {w_form} vs X-form \
             {x_form}. On the merged tree the allocator has removed the frame \
             slack that used to hide it, so W must now score strictly better."
        );
        assert_eq!(
            (w_form, x_form),
            (37, 70),
            "the measured size of C1's win once item E removed the frame slack \
             and item I coalesced the allocator's copies; re-measure this rather \
             than rescaling it"
        );

        // Walk the counterfactual X-form latency up until it does not, so
        // the zero above is a measured amount of slack rather than an
        // inert term. Throughput and stall held at the X-form values.
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

    /// Smoke lane for item C: every one of its opts falls at **every**
    /// point of its own case's residual box, alone, and is refused for
    /// nothing.
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
                sweep.points.iter().all(|p| p.candidate < p.baseline),
                "{id:?} must fall at every point of {case} over baseline {base:?}; got {:?}",
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

    /// **The land gate for item C.** Each opt, alone, over the whole
    /// `cost-*` corpus at every point of the residual box: no case may
    /// rise anywhere, and at least one must fall everywhere.
    ///
    /// **Deep lane**, for the same reason its neighbours are: this is four
    /// whole-corpus sweeps.
    #[ignore = "deep lane: run via `cargo xtask check` (or --ignored)"]
    #[test]
    fn each_item_c_opt_wins_over_the_whole_box_alone() {
        for &(base, id, _) in ITEM_C_SMOKE {
            let mut candidate = base.to_vec();
            candidate.push(id);
            let cmp = compare_opt_lists_over_box(base, &candidate).expect("sweep");
            eprintln!(
                "item C ∀ ({id:?} over {base:?}):\n{}",
                format_sweep_table(&cmp, &format!("{base:?}"), &format!("+{id:?}"))
            );
            assert_sweep_wins(&cmp, &format!("+{id:?}"), &format!("{base:?}"));
        }
    }

    /// Everything in `RELEASE_OPTS` **except** the allocator — the
    /// baseline item E's ∀ gate is measured against, so the verdict is
    /// about this item and not about the whole mode
    /// (plans/codegen-pareto.md decision 1764).
    const WITHOUT_REGALLOC: &[OptId] = &[OptId::NarrowImm];

    /// **Item E's smoke lane: the ∀ sweep of the allocator alone, on one
    /// case, in the default `cargo test`.** Same code path, same probe,
    /// same refusals as the whole-corpus sweep below — over
    /// `cost-branch-bias`, which the allocator alone takes from 562 to
    /// 437 at the pinned point, the largest *relative* fall in the corpus
    /// (22%) and so the one whose collapse would be most visible.
    ///
    /// `cost-bounds-elide` — the smoke case the `dev -> release` sweep
    /// uses — would be the wrong choice here: the allocator does not move
    /// it at all, because every temp in it is read exactly once and
    /// decision 1765 declines to promote those. A smoke lane over a case
    /// the candidate cannot change asserts nothing.
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

    /// **Item E's ∀ gate: the allocator alone, over the whole `cost-*`
    /// corpus, at every point of the residual box.** No case may rise at
    /// any point; at least one must fall at every point.
    ///
    /// The allocator's win is *structural* — it deletes a `str`/`ldr`
    /// pair, the store's V-pipe data uop and both accesses' AGU uops, and
    /// leaves one I-pipe `mov` — so no box coordinate can turn it into a
    /// loss. In particular it does **not** depend on the swept
    /// store-to-load-forwarding latency, which is what this sweep is for:
    /// the claim is checked at the corners, not argued.
    ///
    /// **Deep lane.** `#[ignore]`d and run by `cargo xtask check`, for the
    /// same reason the `dev -> release` sweep above is.
    #[ignore = "deep lane: run via `cargo xtask check` (or --ignored)"]
    #[test]
    fn regalloc_wins_at_every_point_of_the_residual_box() {
        let cmp = compare_opt_lists_over_box(WITHOUT_REGALLOC, RELEASE_OPTS).expect("sweep");
        let table = format_sweep_table(&cmp, "release-minus-RegAlloc", "release");
        eprintln!("∀ sweep (release-minus-RegAlloc → release):\n{table}");
        assert_sweep_wins(&cmp, "release", "release-minus-RegAlloc");
        assert!(cmp.wins());
    }

    // --- plans/codegen-pareto.md item F: the no-ABI gate ---------------
    //
    // Two ids, asked over a **chain** of baselines rather than over `dev`.
    // Neither is a transform of its own: `InterprocRegs` changes which
    // register the allocator may hand out and `Frameless` is read off the
    // allocation's own result, so `dev -> [it]` is the identity for both
    // and a verdict from it would be a verdict about nothing (decision
    // 1747's lesson, applied a second time).
    // ---------------------------------------------------------------

    /// Everything in `RELEASE_OPTS` before item F's first id — the list as
    /// it stood at the end of item E, and the baseline the first link of
    /// the chain is measured against.
    fn item_f_baseline() -> Vec<OptId> {
        RELEASE_OPTS
            .iter()
            .copied()
            .take_while(|o| *o != OptId::InterprocRegs)
            .collect()
    }

    /// `(opt, smoke case)` in `RELEASE_OPTS` order; each row's baseline is
    /// `item_f_baseline()` plus every row before it.
    ///
    /// The **case** is the one whose shapes the transform actually
    /// reaches, measured rather than guessed (freeze 1714):
    ///
    /// - `InterprocRegs` moves six of the twenty cases at the pinned
    ///   point. `cost-crosscore` moves by the most (4142 -> 4052, -90) but
    ///   carries **16 384** corners — a 53 s smoke lane on its own, which
    ///   is a lane changing kind, not a smoke test
    ///   (`bench/thresholds.toml`'s `[tests]` note). `cost-runtime` falls
    ///   by -47 over 1 024 corners and is the same transform.
    /// - `Frameless` moves eighteen of twenty; `cost-arith` is the
    ///   largest *relative* fall (136 -> 74, **-46 %**) and is four
    ///   functions, so it is also the cheapest of the eighteen to sweep.
    const ITEM_F_SMOKE: &[(OptId, &str)] = &[
        (OptId::InterprocRegs, "cost-runtime"),
        (OptId::Frameless, "cost-arith"),
    ];

    /// **Item F's smoke lane.** Each id falls at *every* point of its own
    /// case's residual box, over its own place in the chain, and is
    /// refused for nothing. Same code path, same probe and same refusals
    /// as the whole-corpus lane below.
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

    /// **The land gate for item F.** Each id, over the whole `cost-*`
    /// corpus at every point of the residual box, against its own link in
    /// the chain: no case may rise anywhere, and at least one must fall
    /// everywhere — asked once per tier (decision 1782), so the micro
    /// corpus cannot satisfy the quantifier on the product tier's behalf.
    ///
    /// **Deep lane**: two whole-corpus sweeps.
    #[ignore = "deep lane: run via `cargo xtask check` (or --ignored)"]
    #[test]
    fn each_item_f_opt_wins_over_the_whole_box_alone() {
        let mut base = item_f_baseline();
        for &(id, _) in ITEM_F_SMOKE {
            let mut candidate = base.clone();
            candidate.push(id);
            let cmp = compare_opt_lists_over_box(&base, &candidate).expect("sweep");
            eprintln!(
                "item F ∀ (+{id:?} over {} opts):\n{}",
                base.len(),
                format_sweep_table(&cmp, "base", &format!("+{id:?}"))
            );
            assert_sweep_wins(&cmp, &format!("+{id:?}"), "base");
            base = candidate;
        }
    }

    // ---------------------------------------------------------------
    // plans/codegen-pareto-2.md item L: B4's gate (decisions 1973–1976).
    //
    // `BranchCleanup` is asked over **its own baseline** — the shipped
    // list with it removed — so the verdict is about the transform and
    // not about the mode it rides in (decision 1717 / 1764), and asked
    // once per tier (decision 1782) so the fifteen micro cases cannot
    // satisfy the quantifier on the four borrowed programs' behalf.
    // ---------------------------------------------------------------

    /// `RELEASE_OPTS` with B4 removed: the baseline B4 is ranked against.
    fn without_branch_cleanup() -> Vec<OptId> {
        RELEASE_OPTS
            .iter()
            .copied()
            .filter(|o| *o != OptId::BranchCleanup)
            .collect()
    }

    /// **B4's smoke lane.** `cost-arith-w` is the largest *relative* fall
    /// in either tier (93 → 88 at the pinned point, −5.4 %) and is four
    /// small fns, so it is also among the cheapest to sweep. A smoke lane
    /// over a case the transform cannot reach would assert nothing
    /// (freeze 1714), and every fn ends in a `Return`, so it reaches this
    /// one everywhere.
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

    /// **The land gate for B4**, over the whole `cost-*` corpus at every
    /// point of the residual box, marginally over the shipped list.
    ///
    /// **Deep lane.** Item B measured B4 falling on all fifteen micro
    /// cases and reverted it for the bridge, not for the ruler; this is
    /// the same question asked of the boundary-preserving form that
    /// landed.
    #[ignore = "deep lane: run via `cargo xtask check` (or --ignored)"]
    #[test]
    fn branch_cleanup_wins_at_every_point_of_the_residual_box() {
        let base = without_branch_cleanup();
        let cmp = compare_opt_lists_over_box(&base, RELEASE_OPTS).expect("sweep");
        let table = format_sweep_table(&cmp, "release−BranchCleanup", "release");
        eprintln!("∀ sweep (release−BranchCleanup → release):\n{table}");
        assert_sweep_wins(&cmp, "release", "release−BranchCleanup");
        assert!(cmp.wins());
        eprintln!(
            "BranchCleanup ∀-sweep: {} points/side over {} cases",
            cmp.scored_points(),
            cmp.cases.len()
        );
    }

    /// The same question asked of **each tier alone** (decision 1782), so
    /// the product tier decides on its own four programs.
    ///
    /// **Deep lane.**
    #[ignore = "deep lane: run via `cargo xtask check` (or --ignored)"]
    #[test]
    fn branch_cleanup_wins_in_each_tier_on_its_own() {
        let base = without_branch_cleanup();
        for tier in CostTier::ALL {
            let cmp =
                compare_opt_lists_over_box_in_tier(&base, RELEASE_OPTS, tier).expect("tier sweep");
            eprintln!(
                "BranchCleanup ∀ [{tier}]:\n{}",
                format_sweep_table(&cmp, "release−BranchCleanup", "release")
            );
            assert!(
                cmp.wins_in_tier(tier),
                "BranchCleanup must win on the {tier} tier alone: {:?}",
                cmp.reasons_for_tier(tier)
                    .iter()
                    .map(|r| r.label())
                    .collect::<Vec<_>>()
            );
        }
    }

    /// The cheap half of decision 1779: F5 is not a `RELEASE_OPTS`
    /// member, so nothing can quietly re-add it without also having to
    /// explain the zero the deep lane below pins.
    #[test]
    fn f5_has_no_opt_id() {
        assert!(
            !format!("{RELEASE_OPTS:?}").contains("TailCalls"),
            "F5 has no id: the gate scores it at zero on both tiers \
             (decision 1779)"
        );
    }

    /// **F5's verdict, and why it is not an `OptId`** (decision 1779).
    ///
    /// The tail-call substitution scores **exactly zero on every case of
    /// both tiers**: the cost-stage closures the gate ranks contain no
    /// frameless tail-caller at all, so the transform never fires there.
    /// Freeze 1714 keeps an unrankable transform out of `RELEASE_OPTS`,
    /// so F5 lands unconditionally instead, exactly as item C1 did
    /// (decision 1746) — and this pins the zero, so if a later change
    /// makes it fire on the corpus, that is loud rather than silent and
    /// the id question is re-opened with evidence.
    /// **Deep lane**: this compiles the whole corpus once under
    /// `release`. The cheap half of the claim — that F5 has no id — is
    /// `unit:f5_has_no_opt_id`.
    #[ignore = "deep lane: run via `cargo xtask check` (or --ignored)"]
    #[test]
    fn tail_calls_are_not_rankable_because_the_gate_corpus_never_fires_them() {
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
            fired.is_empty(),
            "a cost-corpus case now fires a tail call: {fired:?}. That is not a \
             failure — it is the evidence F5 lacked. Re-run the ∀ gate over it \
             and, if it wins, give F5 an `OptId` and record the numbers in \
             plans/codegen-pareto-F.md."
        );
    }

    /// Sweep one named case only — the ∀ machinery on a single input, for
    /// oracles that would otherwise pay for the whole corpus.
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

    /// **A candidate that wins at one point and loses at another is vetoed,
    /// with the point named** (04 §5). The two scores are supplied, but the
    /// rule that reads them is the one `sweep_case` runs at every corner —
    /// and the point labels are real [`SweepPoint`] labels, so what is
    /// asserted is that the refusal carries the box coordinate a reader
    /// needs to reproduce it.
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
            ordering: BTreeMap::new(),
        };
        let mut reasons = Vec::new();
        // Wins here…
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
        // …and loses there.
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
        // The ∀ verdict on that pair is a loss, and there is no way to ask
        // for the other answer: `SweepCompare::wins` is the only verdict.
        let cmp = SweepCompare {
            table_digest: table.table_digest(),
            cases: Vec::new(),
            reasons,
        };
        assert!(!cmp.wins());
        assert!(format_sweep_table(&cmp, "base", "cand").contains("outcome=veto"));
    }

    /// The sensitivity probe holds a dimension only when the model never
    /// read it, reports what it held and what it kept, and the residual
    /// corner count is `2^k` over what survived.
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
            // Nothing is dropped: swept ∪ held is the whole declared box.
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
            // Every held dimension is named with the bracket it was held
            // across — silent reduction is the failure this guards.
            for h in &c.held {
                assert!(
                    table.contains(&h.label()),
                    "{}: held dimension {} is not reported",
                    c.name,
                    h.dim
                );
            }
            // A dimension kept only because it was read but never moved is
            // still swept: doubt keeps it in.
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

    /// Held dimensions really are inert for that case: moving one from lo
    /// to hi changes nothing, at the pinned corner and at both extreme
    /// corners. This is the probe's own claim, re-checked from outside it.
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
                    let lo = score_side_at(side, &table, &base.with(&h.dim, h.lo)).expect("lo");
                    let hi = score_side_at(side, &table, &base.with(&h.dim, h.hi)).expect("hi");
                    assert_eq!(lo, hi, "held dimension `{}` moved a score", h.dim);
                }
            }
        }
    }

    /// **Freeze 1633 survives the rewrite.** The barrier-removal refusal is
    /// still on the swept gate, still derived from `CostRule::is_crosscore`
    /// rather than a hand-list, and still independent of the retired words
    /// veto: this fixture grows words *and* deletes a `DMB`, and it is the
    /// barrier that refuses it.
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
        // The corpus gate's own refusal is untouched by item J.
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

    /// **The fail-closed bound is a refusal, not a truncation** — driven
    /// through the real probe, not asserted about a string this test wrote
    /// itself. `probe_case_bounded` takes the bound so a test can put it
    /// below what the committed profile reaches; every production caller
    /// goes through `probe_case` at `MAX_SWEPT_DIMS`.
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

        // At a bound of 0 every surviving dimension is over it, so the
        // probe must refuse rather than hand back a truncated `swept`.
        let err = probe_case_bounded("cost-bounds-elide", &b, &c, &table, 0)
            .expect_err("a bound the case exceeds must refuse");
        assert!(
            err.contains("survive the sensitivity probe")
                && err.contains("over the bound of 0")
                && err.contains("rather than truncating"),
            "the refusal must name the bound and say it is not a truncation: {err}"
        );

        // The same probe at the real bound succeeds and reports a `swept`
        // set — so the refusal above is the bound firing, not the case
        // being broken.
        let ok = probe_case_bounded("cost-bounds-elide", &b, &c, &table, MAX_SWEPT_DIMS)
            .expect("the smoke case must fit the committed bound");
        assert!(!ok.swept.is_empty() && ok.swept.len() <= MAX_SWEPT_DIMS);
        // And the bound is exactly what the refusal counted against.
        assert!(
            probe_case_bounded("cost-bounds-elide", &b, &c, &table, ok.swept.len() - 1).is_err(),
            "one dimension under the surviving count must still refuse"
        );
        apply_mode(CompileMode::Release);
    }
}
