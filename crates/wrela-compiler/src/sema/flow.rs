//! One per-body CFG: definite initialization, moves, and exclusivity
//! (plans/M2.md items E/F, one CFG/one pass file per decision 3).
//! Definite init tracks initialization state per storage path (paths.rs)
//! on every control-flow edge (02-language.md §3.2); moves track `take`
//! deinitialization and use-after-take (§3, §3.1); exclusivity forbids
//! overlapping `mut`/read while a `mut` is active (§3). Flips
//! `values.data.copies-implicitly`, `values.resource.move-spells-take`,
//! `values.exclusivity.no-overlap`. Stubbed until item E/F lands.

use crate::sema::SemaError;
use crate::syntax::ast::Module;

/// Placeholder for item E/F's flow pass; a no-op until then.
pub fn check(_module: &Module) -> Result<(), SemaError> {
    Ok(())
}
