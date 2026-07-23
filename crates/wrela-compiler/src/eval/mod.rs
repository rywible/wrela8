//! The comptime evaluator (plans/M3.md item B): a tree-walking
//! interpreter over the typed tree (`sema::typed`), the reference
//! implementation of comptime semantics (ROADMAP.md: "the tree-walking
//! evaluator is the reference implementation of the semantics").
//!
//! - `value.rs` — `Value`, decision 5's one plain enum, plus exact
//!   scalar arithmetic (docs/language/02-language.md §6.1).
//! - `interp.rs` — the one obvious recursive walk (decision 4): statement/
//!   expression evaluation, calls, `defer`, `?`, quotas.
//! - `quota.rs` — decision 6's fixed step/memory/call-depth budgets.
//!
//! `legal.rs` (whole-graph comptime-legality inference, plans/M3.md item
//! C) lands separately, in parallel with this item — not part of item
//! B's own deliverable.
//!
//! Integration surface (the only user-visible one item B adds): const
//! initializers and const-generic arguments are evaluated with the real
//! evaluator here, replacing M2-H's literal-only subset
//! (`sema::generics::eval_const_expr`, now backed by `eval_standalone`
//! below instead of its own hand-rolled literal/const-name/fieldless-
//! variant match). This module does not gate on comptime legality
//! (plans/M3.md item C, parallel) — an illegal construct (`await`/
//! `send`/`with`/pool operations) cannot reach the typed tree in the
//! first place (sema already fails those closed before producing one),
//! so the evaluator's own fail-closed behavior on anything it does not
//! implement is the only guard item B needs.

pub mod interp;
pub mod legal;
pub mod quota;
pub mod value;

use crate::sema::SemaError;
use crate::sema::typed::{TestKind, TypedProgram};
use crate::syntax::ast::Span;

pub use interp::EvalError;
pub use value::Value;

/// Renders one `EvalError` as a `sema::SemaError` (category `comptime`):
/// typed nodes carry no spans, so — like `generics.rs`'s own multi-line
/// requirement-chain diagnostic — this reuses `omit_location`/
/// `extra_lines` rather than inventing a fake `L:C`. The primary line
/// names the operation that abandoned; each `extra_lines` entry is one
/// live call-stack frame, outermost (the const/context that kicked off
/// evaluation) first.
pub fn to_sema_error(e: EvalError) -> SemaError {
    let extra_lines = e
        .stack
        .iter()
        .map(|frame| format!("  while evaluating `{frame}`"))
        .collect();
    SemaError {
        category: "comptime",
        message: e.message,
        line: 0,
        col: 0,
        extra_lines,
        omit_location: true,
        missing_method: None,
    }
}

/// Evaluates every module-level `const`'s own initializer with the real
/// evaluator (the integration surface, plans/M3.md item B) — called
/// once, after the typed program is fully assembled (`sema::mod::check_typed`,
/// past `generics::check`, so a const initializer that calls into a
/// generic-fn instantiation can resolve it); fail-fast, `BTreeMap`
/// (name) order, matching every other pass's own diagnostic ordering
/// convention (CLAUDE.md: deterministic, first error wins).
pub fn check_consts(program: &TypedProgram) -> Result<(), SemaError> {
    for name in program.consts.keys() {
        interp::eval_const(program, name).map_err(to_sema_error)?;
    }
    Ok(())
}

/// `wrela test`'s own report (plans/M3.md item E, decision 9): every
/// `@test` fn's own verdict, one line apiece, in declaration order
/// (`program.tests`, `sema::typed::TypedProgram::tests`'s own doc
/// comment — source order, never `BTreeMap` order), followed by one
/// pinned summary line. Never fail-fasts across tests (decision 9: "the
/// report is still the complete stable dump" even when some test
/// failed) — every declared test always gets its own line and its own
/// fresh quota, regardless of how any other test came out. Returns the
/// full report text plus whether the caller's own exit code should be
/// nonzero (`true` iff at least one test's line is `FAILED`).
///
/// Line format, pinned by golden coverage (`comptime.tests.build-tier`),
/// chosen once and never varied:
///   `test <name>: ok`
///   `test <name>: FAILED <first line of the diagnostic>`
///   `<N> passed, <M> failed`
/// A file with no `@test` fns at all still prints the summary line alone
/// (`0 passed, 0 failed`) — the dumbest honest "ran zero tests" report,
/// not a special-cased error.
///
/// Fail-closed per decision 9/10: `@test(runtime)` is a *legal*
/// declaration (02-language.md §12.2 — it just names a different, not-
/// yet-built execution path, a generated image test on the wrela
/// machine runner, M5), so sema accepts its body like any other fn's;
/// this is the one place that refuses to *run* it, printing a FAILED
/// line naming M5 rather than silently skipping it or attempting
/// something M5 alone can build. A comptime-illegal `@test` closure
/// (decision 7's `legal::classify`) gets the identical fail-closed
/// treatment — today unreachable (no illegal operation is representable
/// in the typed tree yet, `eval::legal`'s own module doc), but wired
/// through so the day one becomes representable, its own `@test` fails
/// exactly this way rather than panicking or silently passing.
pub fn run_tests(program: &TypedProgram) -> (String, bool) {
    let legality = legal::classify(program);
    let mut out = String::new();
    let mut passed = 0usize;
    let mut failed = 0usize;
    for test in &program.tests {
        let line = match test.kind {
            TestKind::Runtime => {
                failed += 1;
                format!(
                    "test {}: FAILED `@test(runtime)` is not run yet (M5: generated image tests)",
                    test.name
                )
            }
            TestKind::Comptime => {
                match legal::require_legal(&legality, &test.name, "@test", Span::default()) {
                    Err(e) => {
                        failed += 1;
                        format!(
                            "test {}: FAILED {} (M5: illegal-closure tests run as image tests)",
                            test.name, e.message
                        )
                    }
                    Ok(()) => match interp::eval_test(program, &test.name) {
                        Ok(_) => {
                            passed += 1;
                            format!("test {}: ok", test.name)
                        }
                        Err(e) => {
                            failed += 1;
                            let first_line = e.message.lines().next().unwrap_or("");
                            format!("test {}: FAILED {first_line}", test.name)
                        }
                    },
                }
            }
        };
        out.push_str(&line);
        out.push('\n');
    }
    out.push_str(&format!("{passed} passed, {failed} failed\n"));
    (out, failed > 0)
}
