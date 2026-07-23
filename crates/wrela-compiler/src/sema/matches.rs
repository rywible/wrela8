//! Match exhaustiveness (plans/M2.md item G): compositional usefulness
//! over closed sums, `bool`, tuples, and fixed arrays; integers and
//! everything unbounded require a wildcard; a wildcard (or any arm) that
//! covers nothing is an error; guarded arms never contribute; `|`
//! alternatives bind the same names at the same types (02-language.md
//! §7.2). Stubbed until item G lands.

use crate::sema::SemaError;
use crate::syntax::ast::Module;

/// Placeholder for item G's exhaustiveness pass; a no-op until then.
pub fn check(_module: &Module) -> Result<(), SemaError> {
    Ok(())
}
