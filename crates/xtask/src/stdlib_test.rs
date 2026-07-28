//! `cargo xtask stdlib-test` — plans/M16.md item F / decisions 1160–1166.
//!
//! In-process discovery of `stdlib/tests/**/*.wr`, then load + check +
//! `eval::run_tests` for every comptime / `@test(exhaustive)` fn. Fail
//! closed on a missing suite root, an empty suite (no `.wr` files), or
//! zero discovered comptime/exhaustive tests — `wrela test` alone can
//! exit 0 on `0 passed, 0 failed`, which is exactly the quiet pass this
//! lane exists to refuse.

use std::path::{Path, PathBuf};

use wrela_compiler::eval;
use wrela_compiler::loader;
use wrela_compiler::sema;
use wrela_compiler::sema::typed::TestKind;
use wrela_compiler::syntax::ast::Module;

use crate::fuzz::collect_wr_files;
use crate::root;

/// `cargo xtask stdlib-test` entry: suite root is the checkout's
/// `stdlib/tests/`.
pub(crate) fn stdlib_test() -> Result<(), String> {
    run_stdlib_tests(&root().join("stdlib/tests"))
}

/// Core lane body, parameterized on the suite root so the empty-root
/// unit test can point at a temp dir without touching the real tree.
pub(crate) fn run_stdlib_tests(tests_root: &Path) -> Result<(), String> {
    if !tests_root.exists() {
        return Err(format!(
            "stdlib-test: missing suite root {} (fail closed)",
            tests_root.display()
        ));
    }
    if !tests_root.is_dir() {
        return Err(format!(
            "stdlib-test: suite root {} is not a directory (fail closed)",
            tests_root.display()
        ));
    }

    let mut files: Vec<PathBuf> = Vec::new();
    collect_wr_files(tests_root, &mut files)?;
    files.sort();
    if files.is_empty() {
        return Err(format!(
            "stdlib-test: zero .wr files under {} — zero tests (fail closed)",
            tests_root.display()
        ));
    }

    let mut total_comptime = 0usize;
    let mut any_failed = false;

    for path in &files {
        let label = display_path(path);
        let (report, comptime_count, failed) = run_one_file(path)?;
        total_comptime += comptime_count;
        print!("--- {label} ---\n{report}");
        if failed {
            any_failed = true;
        }
    }

    if total_comptime == 0 {
        return Err(format!(
            "stdlib-test: zero comptime/exhaustive @test fns discovered under {} (fail closed)",
            tests_root.display()
        ));
    }
    if any_failed {
        return Err(format!(
            "stdlib-test: {total_comptime} comptime/exhaustive test(s) across {} file(s): FAILED",
            files.len()
        ));
    }

    println!(
        "stdlib-test: {total_comptime} comptime/exhaustive test(s) across {} file(s): ok",
        files.len()
    );
    Ok(())
}

fn display_path(path: &Path) -> String {
    path.strip_prefix(root())
        .map(|p| p.display().to_string())
        .unwrap_or_else(|_| path.display().to_string())
}

fn run_one_file(path: &Path) -> Result<(String, usize, bool), String> {
    let loaded = loader::load_closure(path).map_err(|e| {
        format!(
            "stdlib-test: load {}: {}",
            display_path(path),
            load_error_message(&e)
        )
    })?;

    let paths: std::collections::BTreeMap<Vec<String>, String> = loaded
        .modules
        .iter()
        .map(|(k, m)| (k.clone(), m.file.display().to_string()))
        .collect();
    let modules: std::collections::BTreeMap<Vec<String>, Module> = loaded
        .modules
        .into_iter()
        .map(|(k, m)| (k, m.module))
        .collect();

    let programs = sema::check_program_typed(&modules, &paths)
        .map_err(|e| format!("stdlib-test: check {}: {}", display_path(path), e.message))?;

    let program = programs.get(&loaded.root).ok_or_else(|| {
        format!(
            "stdlib-test: root module `{}` missing after check of {}",
            loaded.root.join("."),
            display_path(path)
        )
    })?;

    let comptime_count = program
        .tests
        .iter()
        .filter(|t| matches!(t.kind, TestKind::Comptime | TestKind::Exhaustive))
        .count();
    let (report, failed) = eval::run_tests(program);
    Ok((report, comptime_count, failed))
}

fn load_error_message(e: &loader::LoadError) -> String {
    match e {
        loader::LoadError::Lex(e) => e.message.clone(),
        loader::LoadError::Parse(e) => e.message.clone(),
        loader::LoadError::Build(e) => e.message.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_stdlib_tests_root_fails_closed() {
        let dir =
            std::env::temp_dir().join(format!("wrela-stdlib-test-empty-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("create empty suite root");
        let err = run_stdlib_tests(&dir).expect_err("empty root must fail closed");
        assert!(
            err.contains("zero") && err.contains("test"),
            "expected zero-tests fail-closed message, got: {err}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
