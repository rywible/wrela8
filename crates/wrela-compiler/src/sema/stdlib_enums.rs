use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use crate::sema::SemaError;
use crate::syntax::ast::{Item, Module, Span};
use crate::syntax::{lexer, parser};

pub const AUTO_VISIBLE: &[&str] = &[
    "Target",
    "Transport",
    "Failure",
    "BootError",
    "DriverMode",
    "CompletionOutcome",
    "Admission",
    "CapacityError",
];

const ENUM_FILES: &[(&str, &str)] = &[
    ("Target", "target"),
    ("Transport", "transport"),
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

static CORE_ROOT: Mutex<Option<PathBuf>> = Mutex::new(None);

static TABLES: Mutex<BTreeMap<PathBuf, &'static Table>> = Mutex::new(BTreeMap::new());

pub fn prepare(pkgroot: &Path, span: Span) -> Result<(), SemaError> {
    let core = crate::loader::stdlib_core_root(pkgroot, span).map_err(load_error_to_sema)?;
    *CORE_ROOT.lock().expect("stdlib_enums CORE_ROOT lock") = Some(core);
    table_result(span).map(|_| ())
}

pub fn prepare_toolchain(span: Span) -> Result<(), SemaError> {
    *CORE_ROOT.lock().expect("stdlib_enums CORE_ROOT lock") =
        Some(crate::loader::toolchain_stdlib_core());
    table_result(span).map(|_| ())
}

pub(crate) fn table_for(core: &Path, span: Span) -> Result<&'static Table, SemaError> {
    let mut tables = TABLES.lock().expect("stdlib_enums TABLES lock");
    if let Some(t) = tables.get(core) {
        return Ok(t);
    }
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
    if CORE_ROOT
        .lock()
        .expect("stdlib_enums CORE_ROOT lock")
        .is_none()
    {
        prepare_toolchain(Span::default())?;
    }
    table_result(Span::default())
}

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

pub fn is_auto_visible(name: &str) -> bool {
    AUTO_VISIBLE.contains(&name)
}

pub fn variants(name: &str) -> Result<Option<&'static [String]>, SemaError> {
    let t = ensure_table()?;
    Ok(t.by_name.get(name).map(|v| v.as_slice()))
}

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

    #[test]
    fn admission_live_variants_are_full_and_deadline_unmeetable() {
        let vs = variant_strs("Admission").expect("load").expect("loaded");
        assert_eq!(vs, &["Full", "DeadlineUnmeetable"]);
    }

    #[test]
    fn capacity_error_live_variant_is_exhausted() {
        let vs = variant_strs("CapacityError")
            .expect("load")
            .expect("loaded");
        assert_eq!(vs, &["Exhausted"]);
    }

    #[test]
    fn corrupt_enum_file_is_build_error_not_panic() {
        let tmp = std::env::temp_dir().join(format!(
            "wrela-stdlib-enums-corrupt-{}-{}",
            std::process::id(),
            "qq"
        ));
        let core = tmp.join("stdlib/core");
        fs::create_dir_all(&core).expect("mkdir");
        for stem in [
            "transport",
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
            "transport",
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

    #[test]
    fn the_cache_is_keyed_by_root() {
        let tmp =
            std::env::temp_dir().join(format!("wrela-stdlib-enums-rekey-{}", std::process::id()));
        let mut cores = Vec::new();
        for (dir, variant) in [("a", "OnlyInA"), ("b", "OnlyInB")] {
            let core = tmp.join(dir).join("stdlib/core");
            fs::create_dir_all(&core).expect("mkdir core");
            for stem in [
                "transport",
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

        let a_again = table_for(&cores[0], Span::default()).expect("tree a from cache");
        assert_eq!(target_variants(a_again), ["OnlyInA".to_string()]);
        assert!(
            std::ptr::eq(a, a_again),
            "a second lookup of the same root must hit the cache, not reload"
        );

        fs::remove_dir_all(&tmp).ok();
    }

    #[test]
    fn a_failed_load_is_not_cached() {
        let tmp =
            std::env::temp_dir().join(format!("wrela-stdlib-enums-nocache-{}", std::process::id()));
        let core = tmp.join("stdlib/core");
        fs::create_dir_all(&core).expect("mkdir core");
        for stem in [
            "transport",
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

        fs::write(&target, "module target\n\npub enum Target:\n    Repaired\n")
            .expect("write repaired");
        let ok = table_for(&core, Span::default()).expect("repaired tree loads");
        assert_eq!(target_variants(ok), ["Repaired".to_string()]);

        fs::remove_dir_all(&tmp).ok();
    }
}
