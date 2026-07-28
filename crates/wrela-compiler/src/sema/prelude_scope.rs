//! Always-in-scope name categories — the one table docs and code cite.
//!
//! 02-language.md §2's fixed prelude is not the whole story the toolchain
//! keeps in scope without an import. This module documents every category
//! so resolution sites (`symbols::is_resolvable_without_import`) and the
//! language doc stay aligned (REVIEW-QUEUE observation M9-I).

use crate::sema::stdlib_enums::AUTO_VISIBLE;

/// Language fixed-prelude names (02-language.md §2): always in scope,
/// no import, no toolchain inject.
pub const FIXED_PRELUDE: &[&str] = &[
    "Option",
    "Some",
    "None",
    "Result",
    "Ok",
    "Err",
    "panic",
    "CallError",
    "Admission",
];

/// Time-prelude names auto-bound from `core.time` (decision 470).
/// Canonical definition — [`crate::loader::TIME_PRELUDE_NAMES`] re-exports
/// this slice. `now` is deliberately absent: it stays a sealed intrinsic
/// (05 §5).
pub const TIME_PRELUDE_NAMES: &[&str] = &[
    "Duration", "Instant", "ns", "us", "ms", "seconds", "minutes", "hours",
];

/// Stdlib enums that resolve without an import
/// ([`crate::sema::stdlib_enums::AUTO_VISIBLE`]).
pub const STDLIB_AUTO_VISIBLE: &[&str] = AUTO_VISIBLE;

/// True when `name` is one of the language fixed-prelude names.
pub fn is_fixed_prelude_name(name: &str) -> bool {
    FIXED_PRELUDE.contains(&name)
}
