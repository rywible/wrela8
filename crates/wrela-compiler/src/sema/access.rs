//! Access-mode checking (plans/M2.md item D): call-site `mut`/`take`
//! mirroring against signatures, the receiver exception, `pub`-method
//! receiver-effect spelling with private-method inference, and operator
//! operands being `read` (02-language.md §3, §5.1, §7.4). Stubbed until
//! item D lands.

use crate::sema::SemaError;
use crate::syntax::ast::Module;

/// Placeholder for item D's access pass; a no-op until then.
pub fn check(_module: &Module) -> Result<(), SemaError> {
    Ok(())
}
