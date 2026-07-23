//! Statement/expression typing (plans/M2.md item C): assignment
//! introduction/reassignment, `if`/`while` condition typing, `for`
//! typing, operator desugar (02-language.md §7.4), call checking (arity,
//! labels), enum literals and leading-dot inference, pattern typing,
//! `is`, closures as structural `fn` types, `?`, `assert`, `defer`. Also
//! where the fail-closed set (decision 7) beyond imports lands:
//! `comptime if`/`comptime assert` evaluation, f-strings,
//! `await`/`send`/actor calls/`with` group/pool, `@image` bodies. Stubbed
//! until item C lands.

use crate::sema::SemaError;
use crate::syntax::ast::Module;

/// Placeholder for item C's body-checking pass; a no-op until then.
pub fn check(_module: &Module) -> Result<(), SemaError> {
    Ok(())
}
