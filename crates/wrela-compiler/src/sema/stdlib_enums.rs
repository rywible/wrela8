//! The five formerly-prelude enums, as real `stdlib/core/*.wr` source
//! (plans/M9.md item I, decisions 472–474; item QQ, decisions 500–505).
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
//!
//! ## Load failures are diagnostics, not panics (item QQ)
//!
//! A corrupt or missing enum file yields `error[build]:` naming the file
//! and the underlying lex/parse error — the same shape A2 pinned for a
//! missing stdlib tree (`golden/err-stdlib-missing`,
//! `golden/err-stdlib-enum-corrupt`). The table is located via
//! [`crate::loader::stdlib_core_root`], so a sibling `stdlib/` wins over
//! the toolchain tree exactly as ordinary `from core.X` imports do.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

use crate::sema::SemaError;
use crate::syntax::ast::{Item, Module, Span};
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

#[derive(Debug)]
pub(crate) struct Table {
    pub(crate) by_name: BTreeMap<String, Vec<String>>,
    strs: BTreeMap<String, Vec<&'static str>>,
}

/// Preferred `stdlib/core/` for this process, set by [`prepare`] before
/// the first table load. One `wrela` invocation has one package root;
/// unit tests that need a custom tree call [`load_table`] directly.
static CORE_ROOT: Mutex<Option<PathBuf>> = Mutex::new(None);

static TABLE: OnceLock<Result<Table, String>> = OnceLock::new();

/// Resolve `stdlib/core/` the same way the loader does for this package
/// root, then load the five enum files. Must run before any
/// [`variant_strs`] / [`variants`] call in a check that should see a
/// sibling stdlib (plans/M9.md item QQ).
pub fn prepare(pkgroot: &Path, span: Span) -> Result<(), SemaError> {
    let core = crate::loader::stdlib_core_root(pkgroot, span).map_err(load_error_to_sema)?;
    *CORE_ROOT.lock().expect("stdlib_enums CORE_ROOT lock") = Some(core);
    table_result(span).map(|_| ())
}

/// Load from the toolchain tree only (no package root). Used when a
/// caller has no file path — virtqueue's tag-order test, fuzz seeds with
/// a synthetic path, etc.
pub fn prepare_toolchain(span: Span) -> Result<(), SemaError> {
    *CORE_ROOT.lock().expect("stdlib_enums CORE_ROOT lock") =
        Some(crate::loader::toolchain_stdlib_core());
    table_result(span).map(|_| ())
}

fn table_result(span: Span) -> Result<&'static Table, SemaError> {
    let r = TABLE.get_or_init(|| {
        let core = CORE_ROOT
            .lock()
            .expect("stdlib_enums CORE_ROOT lock")
            .clone()
            .unwrap_or_else(crate::loader::toolchain_stdlib_core);
        load_table(&core).map_err(|e| e.message)
    });
    match r {
        Ok(t) => Ok(t),
        Err(msg) => Err(SemaError::at("build", msg.clone(), span)),
    }
}

fn ensure_table() -> Result<&'static Table, SemaError> {
    if TABLE.get().is_none()
        && CORE_ROOT
            .lock()
            .expect("stdlib_enums CORE_ROOT lock")
            .is_none()
    {
        // No prepare yet — toolchain fallback (matches the pre-QQ shape
        // for callers outside the check entry points).
        prepare_toolchain(Span::default())?;
    }
    table_result(Span::default())
}

/// Load the five enums from `core`. `pub(crate)` for unit tests that
/// corrupt a temp tree without going through the process-wide OnceLock.
pub(crate) fn load_table(core: &Path) -> Result<Table, SemaError> {
    let mut by_name = BTreeMap::new();
    for &(enum_name, stem) in ENUM_FILES {
        let path = core.join(format!("{stem}.wr"));
        let display = format!("stdlib/core/{stem}.wr");
        let src = std::fs::read_to_string(&path).map_err(|e| {
            SemaError::at(
                "build",
                format!("stdlib enum `{enum_name}` missing at {display}: {e}"),
                Span::default(),
            )
        })?;
        let tokens = lexer::lex(&src).map_err(|e| {
            SemaError::at(
                "build",
                format!(
                    "stdlib enum `{enum_name}` failed to lex ({display}): {}",
                    e.message
                ),
                Span::default(),
            )
        })?;
        let module = parser::parse(tokens).map_err(|e| {
            SemaError::at(
                "build",
                format!(
                    "stdlib enum `{enum_name}` failed to parse ({display}): {}",
                    e.message
                ),
                Span::default(),
            )
        })?;
        let variants = enum_variants_from_module(&module, enum_name).ok_or_else(|| {
            SemaError::at(
                "build",
                format!("stdlib file {display} does not declare `pub enum {enum_name}`"),
                Span::default(),
            )
        })?;
        by_name.insert(enum_name.to_string(), variants);
    }
    let mut strs = BTreeMap::new();
    for (k, vs) in &by_name {
        let leaked: Vec<&'static str> = vs
            .iter()
            .map(|s| Box::leak(s.clone().into_boxed_str()) as &'static str)
            .collect();
        strs.insert(k.clone(), leaked);
    }
    Ok(Table { by_name, strs })
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

fn load_error_to_sema(e: crate::loader::LoadError) -> SemaError {
    match e {
        crate::loader::LoadError::Build(e) => e,
        crate::loader::LoadError::Lex(e) => SemaError {
            category: "lex",
            message: e.message,
            line: e.line,
            col: e.col,
            extra_lines: vec![],
            omit_location: false,
            missing_method: None,
        },
        crate::loader::LoadError::Parse(e) => SemaError {
            category: "parse",
            message: e.message,
            line: e.line,
            col: e.col,
            extra_lines: vec![],
            omit_location: false,
            missing_method: None,
        },
    }
}

/// Is `name` one of the five auto-visible stdlib enums?
pub fn is_auto_visible(name: &str) -> bool {
    AUTO_VISIBLE.contains(&name)
}

/// Variant names in declaration order, or `None` if `name` is not one of
/// the five. Order is load-bearing for `lower::variant_index` and
/// `matches::shape_of`. Load failures are `error[build]` (item QQ).
pub fn variants(name: &str) -> Result<Option<&'static [String]>, SemaError> {
    let t = ensure_table()?;
    Ok(t.by_name.get(name).map(|v| v.as_slice()))
}

/// Same as [`variants`], but returning `&[&str]`-shaped slices for the
/// handful of call sites that previously took `builtin_enum_variants`'
/// `&'static [&'static str]`.
pub fn variant_strs(name: &str) -> Result<Option<&'static [&'static str]>, SemaError> {
    let t = ensure_table()?;
    Ok(t.strs.get(name).map(|v| v.as_slice()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn completion_outcome_order_matches_virtqueue_tags() {
        let vs = variant_strs("CompletionOutcome")
            .expect("load")
            .expect("loaded");
        assert_eq!(vs, &["Completed", "NotCompleted", "Unknown"]);
    }

    #[test]
    fn all_five_enums_load() {
        for name in AUTO_VISIBLE {
            assert!(
                variants(name).expect("load").is_some_and(|v| !v.is_empty()),
                "{name} must load from stdlib"
            );
        }
    }

    /// plans/M9.md item QQ: a corrupt stdlib enum file is `error[build]`,
    /// not a panic. Calls [`load_table`] on a temp tree so the process-wide
    /// OnceLock (already warm from other tests) is not involved — the
    /// golden `err-stdlib-enum-corrupt` covers the CLI path.
    #[test]
    fn corrupt_enum_file_is_build_error_not_panic() {
        let tmp = std::env::temp_dir().join(format!(
            "wrela-stdlib-enums-corrupt-{}-{}",
            std::process::id(),
            "qq"
        ));
        let core = tmp.join("stdlib/core");
        fs::create_dir_all(&core).expect("mkdir");
        // Valid copies of the four siblings so only Target fails.
        for stem in ["restart", "boot_error", "driver_mode", "completion_outcome"] {
            let src = crate::loader::toolchain_stdlib_core().join(format!("{stem}.wr"));
            fs::copy(&src, core.join(format!("{stem}.wr"))).expect("copy");
        }
        fs::write(
            core.join("target.wr"),
            "module target\n\npub enum Target:\n    @@@ not wrela @@@\n",
        )
        .expect("write corrupt");
        let err = load_table(&core).expect_err("corrupt Target must fail");
        assert_eq!(err.category, "build");
        assert!(
            err.message.contains("stdlib enum `Target` failed to parse"),
            "message={}",
            err.message
        );
        assert!(
            err.message.contains("stdlib/core/target.wr"),
            "message={}",
            err.message
        );
        fs::remove_dir_all(&tmp).ok();
    }

    /// plans/M9.md item QQ: `prepare` must prefer a sibling `stdlib/core`
    /// over the toolchain tree — same rule as `loader::stdlib_core_root`.
    #[test]
    fn load_table_reads_the_core_it_is_given() {
        let tmp = std::env::temp_dir().join(format!(
            "wrela-stdlib-enums-sibling-{}-{}",
            std::process::id(),
            "qq"
        ));
        let core = tmp.join("stdlib/core");
        fs::create_dir_all(&core).expect("mkdir");
        for stem in ["restart", "boot_error", "driver_mode", "completion_outcome"] {
            let src = crate::loader::toolchain_stdlib_core().join(format!("{stem}.wr"));
            fs::copy(&src, core.join(format!("{stem}.wr"))).expect("copy");
        }
        fs::write(
            core.join("target.wr"),
            "module target\n\npub enum Target:\n    SiblingOnly\n    AlsoSibling\n",
        )
        .expect("write sibling Target");
        let table = load_table(&core).expect("sibling tree loads");
        assert_eq!(
            table.by_name.get("Target").map(|v| v.as_slice()),
            Some(["SiblingOnly".to_string(), "AlsoSibling".to_string()].as_slice())
        );
        fs::remove_dir_all(&tmp).ok();
    }
}
