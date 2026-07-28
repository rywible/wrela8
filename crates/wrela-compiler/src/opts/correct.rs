//! Dev/release semantic-correctness oracle (plans/M19.md item G /
//! decisions 1470–1473).
//!
//! Decision 1470: a dedicated dual-mode unit on a representative
//! `@test` slice — not doubling full `xtask diff-eval`. `diff-eval`
//! already runs under the product default (`release`); this lane is the
//! explicit `dev` ↔ `release` agreement check on the same semantic
//! oracle (`eval::run_tests`) for that slice, and proves both modes
//! still lower+codegen the `@test` bodies (`emit_comptime_tests`).
//!
//! Fail closed if the modes disagree, or if either drifts from the
//! pinned `expected/test.txt`.

use std::path::PathBuf;

use crate::codegen::codegen_program;
use crate::eval;
use crate::lower::{lower_program_with, LowerOpts};
use crate::mwir;
use crate::sema;
use crate::syntax::{lexer, parser};

use super::{apply_mode, CompileMode};

/// Representative comptime `@test` goldens (same arithmetic-heavy cases
/// `diff-eval` smoke already names for the evaluator↔backend agree
/// path — decision 1470's cheap slice, not the full corpus).
const DEV_CORRECT_SLICE: &[&str] = &["check-tests-arith", "check-tests-program"];

fn golden_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tests/golden")
}

/// Load + typecheck one golden `input.wr`.
fn load_typed(case: &str) -> (sema::typed::TypedProgram, mwir::LayoutCtx) {
    let dir = golden_root().join(case);
    let input = dir.join("input.wr");
    let src = std::fs::read_to_string(&input).unwrap_or_else(|e| {
        panic!("read {}: {e}", input.display());
    });
    let tokens = lexer::lex(&src).expect("lex");
    let module = parser::parse(tokens).expect("parse");
    let path = input.display().to_string();
    let typed = sema::check_typed(&module, &path).expect("check");
    let layout = mwir::build_layout_ctx(&module, &Default::default()).expect("layout");
    (typed, layout)
}

fn expected_test_report(case: &str) -> String {
    let path = golden_root().join(case).join("expected/test.txt");
    std::fs::read_to_string(&path).unwrap_or_else(|e| {
        panic!("read {}: {e}", path.display());
    })
}

/// Run the semantic `@test` oracle under `mode`, then lower+codegen the
/// same program with comptime tests emitted so named opts are live on
/// the backend path.
fn oracle_under_mode(
    typed: &sema::typed::TypedProgram,
    layout: &mwir::LayoutCtx,
    mode: CompileMode,
) -> String {
    apply_mode(mode);
    let (report, _) = eval::run_tests(typed);
    let lower_opts = LowerOpts {
        emit_comptime_tests: true,
        only: None,
    };
    let mwir = lower_program_with(typed, &lower_opts).unwrap_or_else(|e| {
        panic!("{mode:?}: lower failed: {}", e.message);
    });
    let _prog = codegen_program(&mwir, layout).unwrap_or_else(|e| {
        panic!("{mode:?}: codegen failed: {}", e.message);
    });
    report
}

/// Decision 1470–1472: every slice case's `eval::run_tests` report must
/// match under `dev` and `release`, and match the pinned golden
/// `expected/test.txt`. Both modes must lower+codegen the `@test`
/// bodies. Restores `CompileMode::Release` afterward.
pub fn assert_dev_release_agree_on_test_slice() {
    assert!(
        !DEV_CORRECT_SLICE.is_empty(),
        "dev-correct slice must name ≥1 golden"
    );
    for case in DEV_CORRECT_SLICE {
        let expected = expected_test_report(case);
        let (typed, layout) = load_typed(case);
        assert!(
            !typed.tests.is_empty(),
            "{case}: slice case must declare @test fns"
        );

        let dev = oracle_under_mode(&typed, &layout, CompileMode::Dev);
        let release = oracle_under_mode(&typed, &layout, CompileMode::Release);

        assert_eq!(
            dev, release,
            "dev-correct: case {case}: dev and release disagree on eval @test report:\n\
             --- dev ---\n{dev}--- release ---\n{release}"
        );
        assert_eq!(
            dev, expected,
            "dev-correct: case {case}: mode reports drifted from expected/test.txt:\n\
             --- got ---\n{dev}--- expected ---\n{expected}"
        );
    }
    apply_mode(CompileMode::Release);
}

#[cfg(test)]
mod tests {
    use super::*;

    /// plans/M19.md item G / decisions 1470–1472: dual-mode semantic
    /// oracle on the representative `@test` slice. Ledger
    /// `compiler.opts.dev-correct` cites this name at flip (item L).
    #[test]
    fn dev_and_release_agree_on_semantic_test_slice() {
        assert_dev_release_agree_on_test_slice();
    }

    #[test]
    fn slice_names_existing_goldens_with_expected_test() {
        let root = golden_root();
        for case in DEV_CORRECT_SLICE {
            let input = root.join(case).join("input.wr");
            let expected = root.join(case).join("expected/test.txt");
            assert!(
                input.is_file(),
                "missing {}",
                input.display()
            );
            assert!(
                expected.is_file(),
                "missing {}",
                expected.display()
            );
        }
    }
}
