//! Structural generic instantiation (plans/M2.md item H): typing a
//! `Generic[Args]` use enqueues the concrete instantiation (keyed in a
//! `BTreeSet`, decision 3), which runs the full pass pipeline once; a
//! missing requirement reports the multi-line chain diagnostic (decision
//! 2, pinned verbatim in 02-language.md §7.3). Const arguments evaluate
//! only as literals, fieldless-enum variants, and direct `const`
//! references; instantiation recursion has a fixed depth cap. Stubbed
//! until item H lands.

use crate::sema::SemaError;
use crate::syntax::ast::Module;

/// Placeholder for item H's generic-instantiation pass; a no-op until
/// then.
pub fn check(_module: &Module) -> Result<(), SemaError> {
    Ok(())
}
