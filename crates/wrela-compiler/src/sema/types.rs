//! Declaration typing + classification (plans/M2.md item B): the `Type`
//! enum, resolving every signature/field/const type, and
//! data-vs-resource classification (`resource struct` by fiat, `own[P]
//! T`, and any composite containing a resource, transitively —
//! 02-language.md §3, §6, §7.1), plus `deriving` list validation. Stubbed
//! until item B lands.

use crate::sema::SemaError;
use crate::syntax::ast::Module;

/// Placeholder for item B's declare pass; a no-op until then.
pub fn declare(_module: &Module) -> Result<(), SemaError> {
    Ok(())
}
