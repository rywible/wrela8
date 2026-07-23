//! Semantic checking: `syntax::ast::Module -> ()` (a diagnostic) or the
//! `check` stage's dump. Normative source: docs/language/, chiefly
//! §2-§8 (02-language.md). Shape frozen by plans/M2.md:
//!
//! - One file per pass (decision 10), in the frozen pass order (decision
//!   3): collect + resolve (symbols.rs) -> declare (types.rs) -> bodies
//!   (bodies.rs) -> access (access.rs) -> flow (flow.rs, storage paths
//!   shared with paths.rs) -> matches (matches.rs) -> generics
//!   (generics.rs). Every file exists from item A onward, stubbed until
//!   its own item lands, so later items land in their own file rather
//!   than growing this one.
//! - One diagnostic shape throughout (decision 1): `error[<category>]:
//!   <message> at <line>:<col>`, fail-fast — the first error in pass
//!   order, source order within a pass.
//! - A fail-closed helper (decision 7) any pass can reuse: a reachable
//!   construct sema does not check yet reports `error[unimplemented]:
//!   <what> is not checked yet at L:C` instead of silently accepting it.
//!
//! Item A lands collect + resolve and the builtin prelude (prelude.rs);
//! every later pass is a stub (a no-op) until its own item lands, so
//! `check` below only calls what item A actually implements.

pub mod access;
pub mod bodies;
pub mod flow;
pub mod generics;
pub mod matches;
pub mod paths;
pub mod prelude;
pub mod symbols;
pub mod types;

use crate::syntax::ast::{Item, Module, Span};

/// One sema diagnostic, printed by the CLI exactly like a lex/parse
/// error: `error[<category>]: <message> at <line>:<col>` (decision 1).
/// `category` is one of the fixed set the plan names — `name`, `type`,
/// `access`, `move`, `init`, `overlap`, `match`, `generic`,
/// `unimplemented` — so a `&'static str` is enough; no enum is needed
/// (decision 4: dumb, no seams for their own sake).
#[derive(Debug)]
pub struct SemaError {
    pub category: &'static str,
    pub message: String,
    pub line: u32,
    pub col: u32,
}

impl SemaError {
    fn at(category: &'static str, message: String, span: Span) -> SemaError {
        SemaError {
            category,
            message,
            line: span.line,
            col: span.col,
        }
    }
}

/// The fail-closed diagnostic (decision 7): `error[unimplemented]:
/// <subject> not checked yet at L:C`. `subject` is the whole clause
/// including its verb (e.g. `"imports are"`, `"await is"`) so the
/// message reads grammatically for both plural and singular
/// constructs — this helper only supplies " not checked yet" and the
/// category. Every pass that reaches a construct it does not check yet
/// returns this instead of silently accepting it. Item A's only user is
/// `symbols::resolve` (imports); later items reuse it verbatim for their
/// own fail-closed sets.
pub fn unimplemented_at(subject: &str, span: Span) -> SemaError {
    SemaError::at("unimplemented", format!("{subject} not checked yet"), span)
}

/// Runs the sema pipeline in frozen pass order (decision 3) and returns
/// the first diagnostic, if any. Only collect + resolve exist in item A;
/// `declare`/`bodies`/`access`/`flow`/`matches`/`generics` all land as
/// later items wire their stub into this pipeline.
pub fn check(module: &Module) -> Result<(), SemaError> {
    let symtab = symbols::collect(module)?;
    symbols::resolve(module, &symtab)?;
    Ok(())
}

/// The `check` stage's dump (decision 8): on success, one line per
/// module-level declaration, two-space indented, `<Kind> <name>` — no
/// spans, no resolved types yet (that's item B's job, which extends this
/// grammar with a ledger clause citing the extension). Only call this
/// after `check` returns `Ok`.
pub fn dump(module: &Module) -> String {
    let mut out = format!("Module path={}\n", module.path.join("."));
    for item in &module.items {
        if let Some((kind, name)) = item_dump(item) {
            out.push_str(&format!("  {kind} {name}\n"));
        }
    }
    out
}

/// `comptime if` items are not expanded yet (comptime evaluation is item
/// C's job — 02-language.md §12), so they contribute no dump line.
fn item_dump(item: &Item) -> Option<(&'static str, &str)> {
    match item {
        Item::Const(c) => Some(("Const", c.name.as_str())),
        Item::Fn(f) => Some(("Fn", f.name.as_str())),
        Item::Struct(s) => Some(("Struct", s.name.as_str())),
        Item::Enum(e) => Some(("Enum", e.name.as_str())),
        Item::Pool(p) => Some(("Pool", p.name.as_str())),
        Item::ComptimeIf(_) => None,
    }
}
