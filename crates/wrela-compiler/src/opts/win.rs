//! Corpus proxy win oracle (plans/M19.md item E / decisions 1450–1453).
//!
//! Discover every `tests/golden/cost-*/input.wr` (sorted), score under
//! two opt-list configs by `total_proxy_cycles` only, and assert the
//! freeze-1403 win rule: candidate must not raise any case and must
//! strictly lower at least one.

use std::path::{Path, PathBuf};

use crate::codegen::codegen_program;
use crate::cost::score::score_program;
use crate::cost::table::load_default;
use crate::lower::lower_program;
use crate::mwir;
use crate::sema;
use crate::syntax::{lexer, parser};

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

/// Lower+codegen+score `src` under an explicit opt list.
pub fn score_src_under_opts(src: &str, opts: &[OptId]) -> u64 {
    let tokens = lexer::lex(src).expect("lex");
    let module = parser::parse(tokens).expect("parse");
    let typed = sema::check_typed(&module, "<win>").expect("check");
    let layout = mwir::build_layout_ctx(&module, &Default::default()).expect("layout");
    let table = load_default().expect("wrela-cost-v1");

    apply_opts(opts);
    let mwir = lower_program(&typed).expect("lower");
    let prog = codegen_program(&mwir, &layout).expect("codegen");
    let report = score_program(&prog, &table).expect("score");
    report.total_proxy_cycles
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
        let src = std::fs::read_to_string(path).unwrap_or_else(|e| {
            panic!("read {}: {e}", path.display());
        });
        let name = case_name(path);
        let b = score_src_under_opts(&src, baseline);
        let c = score_src_under_opts(&src, candidate);
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
    /// full cost-* corpus.
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
    }

    /// Decision 1453: BoundsElide alone wins on cost-bounds-elide.
    #[test]
    fn bounds_elide_alone_wins_cost_bounds_elide() {
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../tests/golden/cost-bounds-elide/input.wr");
        let src = std::fs::read_to_string(&path).expect("read cost-bounds-elide");
        let dev = score_src_under_opts(&src, &[]);
        let alone = score_src_under_opts(&src, &[OptId::BoundsElide]);
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
            let src = std::fs::read_to_string(path).expect("read");
            let name = case_name(path);
            let dev = score_src_under_opts(&src, &[]);
            let alone = score_src_under_opts(&src, &[OptId::NarrowImm]);
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
}
