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
use crate::sema::typed::TypedProgram;

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
