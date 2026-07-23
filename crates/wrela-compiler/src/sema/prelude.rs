//! The M2 builtin prelude (plans/M2.md decision 5): scalar types, the
//! fixed `Option`/`Result` vocabulary, `panic`, and the handful of
//! standard names the literal rules require (`Static`, `Str`, `Bytes`).
//! This is **not** the stdlib — the real one replaces this hardcoded
//! surface at its own milestone, which is also why the doc corpus (which
//! names stdlib types) is not sema-checked in M2 (plans/M2.md decision 5).
//!
//! Three small fixed arrays, one per source clause, rather than one
//! flat list — each documents *why* its names are in scope with no
//! import (02-language.md §2: "A fixed prelude is always in scope").
//! `is_builtin` is a linear scan across all three, which is dumb and
//! plenty fast for this table's size (decision 4: no premature
//! cleverness).

/// Scalar type names (02-language.md §6.1).
const SCALARS: &[&str] = &[
    "bool", "u8", "u16", "u32", "u64", "usize", "i8", "i16", "i32", "i64", "isize", "f32", "f64",
    "char", "unit", "never",
];

/// The `Option`/`Result` vocabulary (02-language.md §2).
const OPTION_RESULT: &[&str] = &["Option", "Some", "None", "Result", "Ok", "Err", "panic"];

/// The minimum standard surface the literal rules require
/// (02-language.md §1.1, plans/M2.md decision 5).
const LITERAL_SURFACE: &[&str] = &["Static", "Str", "Bytes"];

/// Is `name` one of the fixed prelude names above?
pub fn is_builtin(name: &str) -> bool {
    SCALARS.contains(&name) || OPTION_RESULT.contains(&name) || LITERAL_SURFACE.contains(&name)
}
