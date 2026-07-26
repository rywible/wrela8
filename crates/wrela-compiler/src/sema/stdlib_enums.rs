//! The five formerly-prelude enums, as real `stdlib/core/*.wr` source
//! (plans/M9.md item I, decisions 472–474).
//!
//! `Target`, `Restart`, `BootError`, `DriverMode`, and `CompletionOutcome`
//! used to live only in `sema/prelude::builtin_enum_variants`. Their
//! declarations now sit in the toolchain stdlib; this module loads those
//! files once and exposes their variant order so every consumer that used
//! to hardcode the table reads the same source of truth.
//!
//! ## Why they are not always in the build closure
//!
//! Putting them in every closure (or every closure that mentions a name)
//! would add `Input path=core/<enum>.wr` lines to multi-module report
//! goldens and `Module path=` blocks to dumps — exactly the review-surface
//! trap item E recorded for always-loading `core.time`. Auto-visibility
//! without closure membership keeps ~231 goldens byte-identical while the
//! `.wr` files remain the definition. Explicit `from core.<mod> import
//! <Enum>` still loads them through the ordinary pipeline (and is what
//! `golden/check-stdlib-prelude-enums` pins).

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::OnceLock;

use crate::syntax::ast::{Item, Module};
use crate::syntax::{lexer, parser};

/// Auto-visible enum names whose definitions live in `stdlib/core/`.
pub const AUTO_VISIBLE: &[&str] = &[
    "Target",
    "Restart",
    "BootError",
    "DriverMode",
    "CompletionOutcome",
];

/// `(enum name, file stem under stdlib/core/)`.
const ENUM_FILES: &[(&str, &str)] = &[
    ("Target", "target"),
    ("Restart", "restart"),
    ("BootError", "boot_error"),
    ("DriverMode", "driver_mode"),
    ("CompletionOutcome", "completion_outcome"),
];

fn table() -> &'static BTreeMap<String, Vec<String>> {
    static TABLE: OnceLock<BTreeMap<String, Vec<String>>> = OnceLock::new();
    TABLE.get_or_init(load_table)
}

fn load_table() -> BTreeMap<String, Vec<String>> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../stdlib/core");
    let mut out = BTreeMap::new();
    for &(enum_name, stem) in ENUM_FILES {
        let path = root.join(format!("{stem}.wr"));
        let src = std::fs::read_to_string(&path).unwrap_or_else(|e| {
            panic!(
                "toolchain stdlib enum `{}` missing at {}: {e}",
                enum_name,
                path.display()
            )
        });
        let tokens = lexer::lex(&src).unwrap_or_else(|e| {
            panic!(
                "toolchain stdlib enum `{}` failed to lex ({}): {e:?}",
                enum_name,
                path.display()
            )
        });
        let module = parser::parse(tokens).unwrap_or_else(|e| {
            panic!(
                "toolchain stdlib enum `{}` failed to parse ({}): {e:?}",
                enum_name,
                path.display()
            )
        });
        let variants = enum_variants_from_module(&module, enum_name).unwrap_or_else(|| {
            panic!(
                "toolchain stdlib file {} does not declare `pub enum {}`",
                path.display(),
                enum_name
            )
        });
        out.insert(enum_name.to_string(), variants);
    }
    out
}

fn enum_variants_from_module(module: &Module, enum_name: &str) -> Option<Vec<String>> {
    for item in &module.items {
        if let Item::Enum(e) = item {
            if e.name == enum_name {
                return Some(e.variants.iter().map(|v| v.name.clone()).collect());
            }
        }
    }
    None
}

/// Is `name` one of the five auto-visible stdlib enums?
pub fn is_auto_visible(name: &str) -> bool {
    AUTO_VISIBLE.contains(&name)
}

/// Variant names in declaration order, or `None` if `name` is not one of
/// the five. Order is load-bearing for `lower::variant_index` and
/// `matches::shape_of`.
pub fn variants(name: &str) -> Option<&'static [String]> {
    table().get(name).map(|v| v.as_slice())
}

/// Same as [`variants`], but returning `&[&str]`-shaped slices for the
/// handful of call sites that previously took `builtin_enum_variants`'
/// `&'static [&'static str]`. Allocates once into a parallel static.
pub fn variant_strs(name: &str) -> Option<&'static [&'static str]> {
    static STRS: OnceLock<BTreeMap<String, Vec<&'static str>>> = OnceLock::new();
    let map = STRS.get_or_init(|| {
        let mut m = BTreeMap::new();
        for (k, vs) in table() {
            let leaked: Vec<&'static str> = vs
                .iter()
                .map(|s| Box::leak(s.clone().into_boxed_str()) as &'static str)
                .collect();
            m.insert(k.clone(), leaked);
        }
        m
    });
    map.get(name).map(|v| v.as_slice())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn completion_outcome_order_matches_virtqueue_tags() {
        let vs = variant_strs("CompletionOutcome").expect("loaded");
        assert_eq!(vs, &["Completed", "NotCompleted", "Unknown"]);
    }

    #[test]
    fn all_five_enums_load() {
        for name in AUTO_VISIBLE {
            assert!(
                variants(name).is_some_and(|v| !v.is_empty()),
                "{name} must load from stdlib"
            );
        }
    }
}
