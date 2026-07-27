//! The five formerly-prelude enums, as real `stdlib/core/*.wr` source
//! (plans/M9.md item I, decisions 472–474; item QQ, decisions 500–505).
//!
//! `Target`, `Failure`, `BootError`, `DriverMode`, and `CompletionOutcome`
//! used to live only in `sema/prelude::builtin_enum_variants`. Their
//! declarations now sit in the toolchain stdlib; this module loads those
//! files once per `stdlib/core/` tree and exposes their variant order so
//! every consumer that used to hardcode the table reads the same source
//! of truth.
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
use std::sync::Mutex;

use crate::sema::SemaError;
use crate::syntax::ast::{Item, Module, Span};
use crate::syntax::{lexer, parser};

/// Auto-visible enum names whose definitions live in `stdlib/core/`.
/// plans/M13.md item I / decision 6: `Admission` joins the set (fieldless;
/// live variants `Full | DeadlineUnmeetable`). plans/M13.md item M:
/// `CapacityError` joins (fieldless; live variant `Exhausted`).
pub const AUTO_VISIBLE: &[&str] = &[
    "Target",
    "Failure",
    "BootError",
    "DriverMode",
    "CompletionOutcome",
    "Admission",
    "CapacityError",
];

/// `(enum name, file stem under stdlib/core/)`.
const ENUM_FILES: &[(&str, &str)] = &[
    ("Target", "target"),
    ("Failure", "failure"),
    ("BootError", "boot_error"),
    ("DriverMode", "driver_mode"),
    ("CompletionOutcome", "completion_outcome"),
    ("Admission", "admission"),
    ("CapacityError", "capacity_error"),
];

#[derive(Debug)]
pub(crate) struct Table {
    pub(crate) by_name: BTreeMap<String, Vec<String>>,
    strs: BTreeMap<String, Vec<&'static str>>,
}

/// Preferred `stdlib/core/` for the check currently running, set by
/// [`prepare`] before each table load. Unit tests that need a custom tree
/// call [`load_table`] directly.
static CORE_ROOT: Mutex<Option<PathBuf>> = Mutex::new(None);

/// Loaded tables, **keyed by the `stdlib/core/` they came from**
/// (plans/M9.md item RR).
///
/// This was a bare `OnceLock<Result<Table, String>>`, which made the
/// first `prepare` in a process the only one that could ever choose a
/// tree: later calls set `CORE_ROOT` and then got the *first* root's
/// table back from `get_or_init`, still reporting `Ok`. That silently
/// withdrew item A2's rule — a sibling `stdlib/` wins over the toolchain
/// tree — for every file after the first in any process that checks more
/// than one, and `xtask` is exactly that process: `corpus --sema` walks
/// every doc block and example, and the sema/eval/lower/async/imports
/// fuzz lanes each run `check_typed` thousands of times. A wrong table is
/// not a wrong message, it is wrong `lower::variant_index` tags and wrong
/// `matches::shape_of` exhaustiveness, with no diagnostic anywhere.
///
/// Keying by root fixes both halves at once: a different root loads its
/// own table, and **failures are not cached**, so a corrupt tree no
/// longer poisons every later check in the process (the old `Err` arm was
/// permanent). Entries are leaked to keep the `&'static` returns
/// [`variants`]/[`variant_strs`] already promise — the same `Box::leak`
/// discipline [`load_table`] uses for the variant strings, and bounded by
/// the number of distinct package roots one process ever sees.
static TABLES: Mutex<BTreeMap<PathBuf, &'static Table>> = Mutex::new(BTreeMap::new());

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

/// The cache lookup, against an **explicit** root — the whole of the
/// keying rule, with no ambient state read anywhere inside it.
/// `table_result` is the one place `CORE_ROOT` turns into one of these
/// calls, which is also what lets the unit tests exercise the keying
/// without touching a global other tests in this crate mutate.
pub(crate) fn table_for(core: &Path, span: Span) -> Result<&'static Table, SemaError> {
    let mut tables = TABLES.lock().expect("stdlib_enums TABLES lock");
    if let Some(t) = tables.get(core) {
        return Ok(t);
    }
    // Errors deliberately do not enter the cache: the next `prepare` for
    // this same root should re-read the tree and re-diagnose, not inherit
    // a verdict from a run that may have raced a half-written file.
    let table = load_table(core).map_err(|e| SemaError::at("build", e.message, span))?;
    let leaked: &'static Table = Box::leak(Box::new(table));
    tables.insert(core.to_path_buf(), leaked);
    Ok(leaked)
}

fn table_result(span: Span) -> Result<&'static Table, SemaError> {
    let core = CORE_ROOT
        .lock()
        .expect("stdlib_enums CORE_ROOT lock")
        .clone()
        .unwrap_or_else(crate::loader::toolchain_stdlib_core);
    table_for(&core, span)
}

fn ensure_table() -> Result<&'static Table, SemaError> {
    // No `prepare` yet — toolchain fallback (matches the pre-QQ shape for
    // callers outside the check entry points). `table_result` applies the
    // same fallback for an unset root, so this only has to cover the
    // *stateful* half: leaving `CORE_ROOT` set so a later call agrees.
    if CORE_ROOT
        .lock()
        .expect("stdlib_enums CORE_ROOT lock")
        .is_none()
    {
        prepare_toolchain(Span::default())?;
    }
    table_result(Span::default())
}

/// Load the auto-visible enums from `core`. `pub(crate)` for unit tests that
/// corrupt a temp tree without going through the process-wide cache.
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

/// Is `name` one of the auto-visible stdlib enums?
pub fn is_auto_visible(name: &str) -> bool {
    AUTO_VISIBLE.contains(&name)
}

/// Variant names in declaration order, or `None` if `name` is not
/// auto-visible. Order is load-bearing for `lower::variant_index` and
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
    fn all_auto_visible_enums_load() {
        for name in AUTO_VISIBLE {
            assert!(
                variants(name).expect("load").is_some_and(|v| !v.is_empty()),
                "{name} must load from stdlib"
            );
        }
    }

    /// plans/M13.md item I / decision 6: post-F live set only.
    #[test]
    fn admission_live_variants_are_full_and_deadline_unmeetable() {
        let vs = variant_strs("Admission").expect("load").expect("loaded");
        assert_eq!(vs, &["Full", "DeadlineUnmeetable"]);
    }

    /// plans/M13.md item M: `CapacityError` is fieldless with one live
    /// variant (`Exhausted`).
    #[test]
    fn capacity_error_live_variant_is_exhausted() {
        let vs = variant_strs("CapacityError")
            .expect("load")
            .expect("loaded");
        assert_eq!(vs, &["Exhausted"]);
    }

    /// plans/M9.md item QQ: a corrupt stdlib enum file is `error[build]`,
    /// not a panic. Calls [`load_table`] on a temp tree so the process-wide
    /// cache (already warm from other tests) is not involved — the golden
    /// `err-stdlib-enum-corrupt` covers the CLI path.
    #[test]
    fn corrupt_enum_file_is_build_error_not_panic() {
        let tmp = std::env::temp_dir().join(format!(
            "wrela-stdlib-enums-corrupt-{}-{}",
            std::process::id(),
            "qq"
        ));
        let core = tmp.join("stdlib/core");
        fs::create_dir_all(&core).expect("mkdir");
        // Valid copies of the siblings so only Target fails.
        for stem in [
            "failure",
            "boot_error",
            "driver_mode",
            "completion_outcome",
            "admission",
            "capacity_error",
        ] {
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
        for stem in [
            "failure",
            "boot_error",
            "driver_mode",
            "completion_outcome",
            "admission",
            "capacity_error",
        ] {
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

    fn target_variants(t: &Table) -> &[String] {
        t.by_name.get("Target").expect("Target loaded")
    }

    /// plans/M9.md item RR: the table cache is keyed by `stdlib/core/`, so
    /// a second root in the same process gets its own table instead of
    /// silently reusing the first one's variant order.
    ///
    /// This is the regression lock for the `OnceLock` shape this module
    /// used to have: under it the second lookup still returned `Ok` while
    /// answering from tree A, which is not a wrong message but wrong
    /// `lower::variant_index` tags and wrong `matches::shape_of`
    /// exhaustiveness, with no diagnostic anywhere.
    ///
    /// Goes through [`table_for`] rather than [`prepare`] deliberately:
    /// `prepare` writes the process-global `CORE_ROOT`, which tests
    /// elsewhere in this crate also write, and `cargo test` runs them
    /// concurrently in one process. `table_for` *is* the keying rule —
    /// `table_result` adds only the global read — so this locks the thing
    /// that regressed without racing anything.
    #[test]
    fn the_cache_is_keyed_by_root() {
        let tmp =
            std::env::temp_dir().join(format!("wrela-stdlib-enums-rekey-{}", std::process::id()));
        let mut cores = Vec::new();
        for (dir, variant) in [("a", "OnlyInA"), ("b", "OnlyInB")] {
            let core = tmp.join(dir).join("stdlib/core");
            fs::create_dir_all(&core).expect("mkdir core");
            for stem in [
                "failure",
                "boot_error",
                "driver_mode",
                "completion_outcome",
                "admission",
                "capacity_error",
            ] {
                let src = crate::loader::toolchain_stdlib_core().join(format!("{stem}.wr"));
                fs::copy(&src, core.join(format!("{stem}.wr"))).expect("copy");
            }
            fs::write(
                core.join("target.wr"),
                format!("module target\n\npub enum Target:\n    {variant}\n"),
            )
            .expect("write Target");
            cores.push(core);
        }

        let a = table_for(&cores[0], Span::default()).expect("tree a loads");
        assert_eq!(target_variants(a), ["OnlyInA".to_string()]);

        let b = table_for(&cores[1], Span::default()).expect("tree b loads");
        assert_eq!(
            target_variants(b),
            ["OnlyInB".to_string()],
            "the second root must win — a cached first root is the bug this locks"
        );

        // Back to the first: served from the cache, same answer as before.
        let a_again = table_for(&cores[0], Span::default()).expect("tree a from cache");
        assert_eq!(target_variants(a_again), ["OnlyInA".to_string()]);
        assert!(
            std::ptr::eq(a, a_again),
            "a second lookup of the same root must hit the cache, not reload"
        );

        fs::remove_dir_all(&tmp).ok();
    }

    /// plans/M9.md item RR: a failed load must not be cached. The old
    /// `OnceLock<Result<..>>` stored the `Err`, so one corrupt tree
    /// poisoned every later lookup in the process.
    #[test]
    fn a_failed_load_is_not_cached() {
        let tmp =
            std::env::temp_dir().join(format!("wrela-stdlib-enums-nocache-{}", std::process::id()));
        let core = tmp.join("stdlib/core");
        fs::create_dir_all(&core).expect("mkdir core");
        for stem in [
            "failure",
            "boot_error",
            "driver_mode",
            "completion_outcome",
            "admission",
            "capacity_error",
        ] {
            let src = crate::loader::toolchain_stdlib_core().join(format!("{stem}.wr"));
            fs::copy(&src, core.join(format!("{stem}.wr"))).expect("copy");
        }
        let target = core.join("target.wr");
        fs::write(
            &target,
            "module target\n\npub enum Target:\n    @@@ nope @@@\n",
        )
        .expect("write corrupt");
        let err = table_for(&core, Span::default()).expect_err("corrupt tree must fail");
        assert_eq!(err.category, "build");

        // Repair the same tree and look it up again: the fix must take.
        fs::write(&target, "module target\n\npub enum Target:\n    Repaired\n")
            .expect("write repaired");
        let ok = table_for(&core, Span::default()).expect("repaired tree loads");
        assert_eq!(target_variants(ok), ["Repaired".to_string()]);

        fs::remove_dir_all(&tmp).ok();
    }
}
