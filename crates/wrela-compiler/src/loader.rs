//! The loader: root anchoring + transitive import closure (plans/M4.md
//! item A, decisions 1-3; 02-language.md §2, §2.1; 04-compiler.md §1
//! "Closure").
//!
//! A build (and every other tool that resolves imports — `wrela dump
//! --stage=check` included, once the target module has any) is pointed
//! at one file: the root. Everything else is derived from it.
//!
//! **Root anchoring** (`anchor_package_root`): a module's declared
//! `module a.b.c` path must agree with its file path — the file name is
//! the path's last segment (`c.wr`), each enclosing directory (innermost
//! first) is the next segment up (`b`, then `a`), and whatever directory
//! remains after consuming every segment is the package root. This rule
//! is applied to *every* file the closure reaches, not just the root
//! (02-language.md §2: "must match the file's path under the package
//! source root" is a per-file rule); the root file is simply the one
//! file whose walk *discovers* the root instead of merely confirming it
//! agrees with an already-known one. Disagreement anywhere is one
//! `error[build]` diagnostic naming the exact mismatch.
//!
//! **The import closure** (`load_closure`): starting from the root,
//! every `from path import Name` is resolved to `<pkgroot>/<path with
//! dots as slashes>.wr` and loaded if not already visited. The visited
//! set (this module's own `LoadedProgram::modules` keys) is exactly
//! what makes import cycles free (plans/M4.md decision 3, amended
//! 2026-07-23: 02-language.md §2 already settles this — "imports are
//! compile-time name bindings and run no code, so import cycles between
//! modules are legal"): a module already loaded is never re-read,
//! cycle or not, and the walk simply terminates. A missing file for an
//! imported module path is `error[build]`, naming the path.
//!
//! **The `core` alias**: `from core.X import ...` resolves inside the
//! toolchain's `stdlib/core/` tree, addressed throughout this compiler as
//! `["core", ...]` regardless of what `X`'s own file declares (`core`
//! is an import-side alias over that tree, not a real package segment —
//! `stdlib/README.md` calls `stdlib/core/` the `core` package root, so
//! a file at `stdlib/core/io_error.wr` declares plain `module io_error`,
//! agreeing with its path under `stdlib/core/` as *its* package root; the
//! `core` prefix is stripped before mapping the remaining segments onto
//! that root). Locating `stdlib/core/` itself uses the dumbest
//! deterministic rule that still lets a self-contained fixture (this
//! repo's own project-shaped golden cases included) ship its own, in
//! preference to the real toolchain tree: (1) a directory literally
//! named `stdlib/core` sitting next to the package root (a sibling of
//! `pkgroot`, i.e. `pkgroot.parent()/stdlib/core`) wins if it exists as
//! a directory; (2) otherwise, the real toolchain `stdlib/core/`, baked
//! in at compile time via this very crate's own `CARGO_MANIFEST_DIR`
//! (`crates/wrela-compiler/../../stdlib/core`) — wherever the compiled
//! `wrela` binary is actually run from. No environment variable, no
//! search path: exactly these two candidates, in this fixed order, every
//! time (recorded here per plans/M4.md item A's brief; plans/M9.md item
//! A2 decision 78 nested the package root under `core/`). If a sibling
//! `stdlib/` directory exists but has no `core/` subdirectory — or if
//! the chosen candidate is not a directory at all — the loader fails
//! closed with a named `error[build]: stdlib not found: ...` rather than
//! falling through to a missing-module diagnostic that would hide the
//! real cause (plans/M9.md item A2, golden/err-stdlib-missing). A bare
//! `from core import <name>` (no further segment) names a *submodule*,
//! not a declaration — the same "more than trivial" shape
//! `sema::imports` fails closed on (02-language.md §2's own
//! "`from core.bytes import Bytes` and `from core import time` are the
//! same construct" — but `core` alone has no declaration to look inside
//! at all, per that section, so this shape is unambiguous the moment
//! the path is just `["core"]`); the loader has nothing to load for it
//! (no file backs bare `core`), so such an import contributes nothing
//! to the closure and is left for `sema::imports::resolve_imports` to
//! reject once names are in scope.
//!
//! Every diagnostic this file itself raises (root disagreement, a
//! missing module file) uses the new `build` category (plans/M4.md
//! decision 1), rendered exactly like every other sema diagnostic
//! (`error[build]: <message> at <line>:<col>`) — see `sema::SemaError`.
//! A lex or parse failure in any file the closure reaches is *not* a
//! loader diagnostic; it is that file's own `error[lex]`/`error[parse]`,
//! unchanged, via `LoadError::Lex`/`LoadError::Parse`.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use crate::sema::SemaError;
use crate::syntax::ast::{Module, Span};
use crate::syntax::{lexer, parser};

/// One failure while loading the closure: a lex/parse error from some
/// file the closure reaches (rendered exactly like the single-file CLI
/// path's own diagnostics), or a `build`-category diagnostic for the
/// loader's own two obligations (root anchoring, closure resolution).
pub enum LoadError {
    Lex(lexer::LexError),
    Parse(parser::ParseError),
    Build(SemaError),
}

/// One loaded file: where it came from and what it parsed to.
pub struct LoadedModule {
    pub file: PathBuf,
    pub module: Module,
}

/// The whole build's module graph (plans/M4.md decisions 1-2): every
/// module the root's transitive import closure reaches, keyed by its
/// own *address* — a plain module's declared dotted path unchanged, or
/// `["core", ...]` for a module reached through the `core` alias.
/// `BTreeMap` gives the whole-program BTree-order-by-module-path walk
/// decision 2 asks for, free of any extra sorting step.
pub struct LoadedProgram {
    pub root: Vec<String>,
    pub modules: BTreeMap<Vec<String>, LoadedModule>,
}

fn build_error(message: String, span: Span) -> LoadError {
    LoadError::Build(SemaError::at("build", message, span))
}

fn parse_file(file: &Path) -> Result<Module, LoadError> {
    let source = std::fs::read_to_string(file).map_err(|e| {
        build_error(
            format!("cannot read `{}`: {e}", file.display()),
            Span::default(),
        )
    })?;
    let tokens = lexer::lex(&source).map_err(LoadError::Lex)?;
    parser::parse(tokens).map_err(LoadError::Parse)
}

/// Walks `module_path` upward against `file`'s own path, one segment per
/// directory level (innermost first), and returns whatever directory
/// remains once every segment is consumed — the package root this file
/// implies. See the module doc comment above for the exact rule.
fn anchor_package_root(
    file: &Path,
    module_path: &[String],
    span: Span,
) -> Result<PathBuf, LoadError> {
    let last = module_path
        .last()
        .expect("the parser guarantees a non-empty module path");
    let stem = file.file_stem().and_then(|s| s.to_str());
    if stem != Some(last.as_str()) {
        return Err(build_error(
            format!(
                "module `{}` disagrees with its file path: expected the file name `{last}.wr`, found `{}`",
                module_path.join("."),
                file.display()
            ),
            span,
        ));
    }
    let mut dir = file.parent().map(Path::to_path_buf).unwrap_or_default();
    for seg in module_path[..module_path.len() - 1].iter().rev() {
        let dir_name = dir.file_name().and_then(|s| s.to_str());
        if dir_name != Some(seg.as_str()) {
            return Err(build_error(
                format!(
                    "module `{}` disagrees with its file path: expected the directory `{seg}`, found `{}` in `{}`",
                    module_path.join("."),
                    dir_name.unwrap_or("<none>"),
                    file.display()
                ),
                span,
            ));
        }
        dir = dir.parent().map(Path::to_path_buf).unwrap_or_default();
    }
    Ok(dir)
}

/// Every file this loader reaches must agree with `expected_root` (the
/// package root already anchored from the build's own root file, or
/// `core_root` for a `core`-mapped file) — not just agree with *some*
/// root of its own (`anchor_package_root` alone only proves the latter).
fn check_agrees(file: &Path, module: &Module, expected_root: &Path) -> Result<(), LoadError> {
    let root = anchor_package_root(file, &module.path, module.span)?;
    if root != expected_root {
        return Err(build_error(
            format!(
                "module `{}`'s file `{}` does not anchor the expected package root `{}` (found `{}`)",
                module.path.join("."),
                file.display(),
                expected_root.display(),
                root.display()
            ),
            module.span,
        ));
    }
    Ok(())
}

/// `<root>/<path[0]>/<path[1]>/.../<path[n]>.wr` — every segment but the
/// last becomes a directory, the last becomes the file name.
fn module_file_path(root: &Path, module_path: &[String]) -> PathBuf {
    let mut p = root.to_path_buf();
    for seg in module_path {
        p.push(seg);
    }
    p.set_extension("wr");
    p
}

/// Locates the toolchain's `stdlib/core/` tree — see the module doc
/// comment above for the exact two-candidate priority this implements.
/// Returns a named `error[build]` when the chosen candidate is missing
/// (plans/M9.md item A2: golden/err-stdlib-missing).
fn core_root(pkgroot: &Path, span: Span) -> Result<PathBuf, LoadError> {
    if let Some(parent) = pkgroot.parent() {
        let sibling_stdlib = parent.join("stdlib");
        if sibling_stdlib.is_dir() {
            let core = sibling_stdlib.join("core");
            if core.is_dir() {
                return Ok(core);
            }
            // Sibling `stdlib/` shadows the toolchain tree, so a missing
            // `core/` here is not "fall through to CARGO_MANIFEST_DIR" —
            // it is the fail-closed "stdlib not found" case A2 pins.
            return Err(build_error(
                "stdlib not found: sibling `stdlib/` exists but has no `core/` directory"
                    .to_string(),
                span,
            ));
        }
    }
    let toolchain = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../stdlib/core");
    if !toolchain.is_dir() {
        return Err(build_error(
            "stdlib not found: toolchain `stdlib/core/` is missing".to_string(),
            span,
        ));
    }
    Ok(toolchain)
}

/// Resolves one `from path import ...` statement's `path` to the module
/// address it loads under, the file it lives at, and the package root
/// that file must itself agree with. Never called for a bare `["core"]`
/// path (the submodule-import shape) — callers filter that out first,
/// see `load_closure`.
fn import_target(
    pkgroot: &Path,
    import_path: &[String],
    span: Span,
) -> Result<(Vec<String>, PathBuf, PathBuf), LoadError> {
    if import_path[0] == "core" {
        let root = core_root(pkgroot, span)?;
        let rest = &import_path[1..];
        let file = module_file_path(&root, rest);
        Ok((import_path.to_vec(), file, root))
    } else {
        let file = module_file_path(pkgroot, import_path);
        Ok((import_path.to_vec(), file, pkgroot.to_path_buf()))
    }
}

/// Loads the root file and its whole transitive import closure
/// (plans/M4.md item A). See the module doc comment for the full rule.
pub fn load_closure(root_file: &Path) -> Result<LoadedProgram, LoadError> {
    let root_module = parse_file(root_file)?;
    let pkgroot = anchor_package_root(root_file, &root_module.path, root_module.span)?;

    let mut modules: BTreeMap<Vec<String>, LoadedModule> = BTreeMap::new();
    let root_key = root_module.path.clone();
    modules.insert(
        root_key.clone(),
        LoadedModule {
            file: root_file.to_path_buf(),
            module: root_module,
        },
    );

    // Visited-set walk (decision 3: import cycles between modules are
    // legal) — `modules`'s own keys *are* the visited set, so a module
    // already loaded is simply skipped, cycle or not. `queue` grows as
    // new modules are discovered; a plain index cursor avoids
    // recursion. Final iteration order is always by module path
    // (`modules` is a `BTreeMap`) regardless of discovery order, so the
    // order new modules are *found* in here has no effect on anything
    // downstream.
    let mut queue: Vec<Vec<String>> = vec![root_key.clone()];
    let mut head = 0;
    while head < queue.len() {
        let current = queue[head].clone();
        head += 1;
        let imports = modules[&current].module.imports.clone();
        for import in &imports {
            if import.path.len() == 1 && import.path[0] == "core" {
                // Bare `from core import <name>` — nothing to load; see
                // the module doc comment's own note on this shape.
                continue;
            }
            let (key, file, expected_root) = import_target(&pkgroot, &import.path, import.span)?;
            if modules.contains_key(&key) {
                continue;
            }
            if !file.is_file() {
                // Deliberately does not print `file`'s own resolved
                // path: for a `core`-mapped import, that path can
                // bottom out in `core_root`'s compile-time
                // `CARGO_MANIFEST_DIR` fallback — an absolute path
                // baked in at compiler build time, which would bake
                // *this checkout's own location* into a pinned golden
                // the moment it differs from another clone's. The
                // dotted module path is everything the diagnostic
                // needs and is stable everywhere.
                return Err(build_error(
                    format!("module `{}` not found: no such file", key.join(".")),
                    import.span,
                ));
            }
            let module = parse_file(&file)?;
            check_agrees(&file, &module, &expected_root)?;
            queue.push(key.clone());
            modules.insert(key, LoadedModule { file, module });
        }
    }

    // plans/M9.md item E: load `core.time` only when the closure mentions
    // a time-prelude name (or `now`). Always-loading would put
    // `Module path=time` in every project check dump; lazy keeps the
    // golden review surface honest.
    if closure_mentions_time(&modules) {
        ensure_time_module(&pkgroot, &mut modules)?;
    }

    Ok(LoadedProgram {
        root: root_key,
        modules,
    })
}

/// Loader key for `stdlib/core/time.wr` — the `core` alias prefix plus
/// the file's own `module time` address (same shape as `io_error`).
pub const TIME_MODULE_KEY: &[&str] = &["core", "time"];

/// True when any loaded module's source mentions a time-prelude name or
/// `now` (plans/M9.md item E: lazy load of `core.time`).
pub fn closure_mentions_time(modules: &BTreeMap<Vec<String>, LoadedModule>) -> bool {
    modules.values().any(|m| module_mentions_time(&m.module))
}

fn module_mentions_time(module: &Module) -> bool {
    // Pretty-print and scan tokens — cheaper and less brittle than
    // mirroring every Expr/Stmt variant, and false positives from a
    // comment spelling `seconds` only cost a harmless stdlib load.
    let text = crate::syntax::printer::pretty(module);
    text.split(|c: char| !c.is_ascii_alphanumeric() && c != '_')
        .any(|tok| tok == "now" || TIME_PRELUDE_NAMES.contains(&tok))
}

/// Names item E keeps prelude-visible while their implementations live
/// in `stdlib/core/time.wr`. `now` is deliberately absent: it stays a
/// sealed intrinsic (05 §5).
pub const TIME_PRELUDE_NAMES: &[&str] = &[
    "Duration", "Instant", "ns", "us", "ms", "seconds", "minutes", "hours",
];

/// Ensures `core.time` is present in `modules`. Idempotent when the
/// closure already imported it explicitly.
pub fn ensure_time_module(
    pkgroot: &Path,
    modules: &mut BTreeMap<Vec<String>, LoadedModule>,
) -> Result<(), LoadError> {
    let key: Vec<String> = TIME_MODULE_KEY.iter().map(|s| (*s).to_string()).collect();
    if modules.contains_key(&key) {
        return Ok(());
    }
    let (resolved_key, file, expected_root) = import_target(pkgroot, &key, Span::default())?;
    debug_assert_eq!(resolved_key, key);
    if !file.is_file() {
        return Err(build_error(
            format!("module `{}` not found: no such file", key.join(".")),
            Span::default(),
        ));
    }
    let module = parse_file(&file)?;
    check_agrees(&file, &module, &expected_root)?;
    modules.insert(key, LoadedModule { file, module });
    Ok(())
}

/// Parses toolchain `stdlib/core/time.wr` into a standalone loaded module
/// (plans/M9.md item E: the single-module `check_typed` entry still
/// needs the time surface without a user-facing import).
pub fn load_time_module() -> Result<(Vec<String>, LoadedModule), LoadError> {
    let key: Vec<String> = TIME_MODULE_KEY.iter().map(|s| (*s).to_string()).collect();
    let toolchain = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../stdlib/core");
    let file = toolchain.join("time.wr");
    if !file.is_file() {
        return Err(build_error(
            "stdlib not found: toolchain `stdlib/core/time.wr` is missing".to_string(),
            Span::default(),
        ));
    }
    let module = parse_file(&file)?;
    check_agrees(&file, &module, &toolchain)?;
    Ok((key, LoadedModule { file, module }))
}

#[cfg(test)]
mod time_prelude_tests {
    use super::*;

    #[test]
    fn load_time_module_parses() {
        let (key, loaded) = match load_time_module() {
            Ok(v) => v,
            Err(_) => panic!("time.wr exists"),
        };
        assert_eq!(key, vec!["core".to_string(), "time".to_string()]);
        assert_eq!(loaded.module.path, vec!["time".to_string()]);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn seg(s: &str) -> String {
        s.to_string()
    }

    #[test]
    fn anchors_single_segment_module() {
        let root = anchor_package_root(Path::new("main.wr"), &[seg("main")], Span::default())
            .ok()
            .expect("agrees");
        assert_eq!(root, PathBuf::from(""));
    }

    #[test]
    fn anchors_nested_module() {
        let root = anchor_package_root(
            Path::new("src/app/main.wr"),
            &[seg("app"), seg("main")],
            Span::default(),
        )
        .ok()
        .expect("agrees");
        assert_eq!(root, PathBuf::from("src"));
    }

    #[test]
    fn anchors_deeply_nested_module() {
        let root = anchor_package_root(
            Path::new("proj/src/a/b/c.wr"),
            &[seg("a"), seg("b"), seg("c")],
            Span::default(),
        )
        .ok()
        .expect("agrees");
        assert_eq!(root, PathBuf::from("proj/src"));
    }

    #[test]
    fn rejects_file_stem_disagreement() {
        let err = anchor_package_root(
            Path::new("src/app/other.wr"),
            &[seg("app"), seg("main")],
            Span::default(),
        );
        assert!(err.is_err());
    }

    #[test]
    fn rejects_directory_disagreement() {
        let err = anchor_package_root(
            Path::new("src/wrong/main.wr"),
            &[seg("app"), seg("main")],
            Span::default(),
        );
        assert!(err.is_err());
    }

    #[test]
    fn module_file_path_joins_dots_as_slashes() {
        let p = module_file_path(Path::new("src"), &[seg("a"), seg("b"), seg("c")]);
        assert_eq!(p, PathBuf::from("src/a/b/c.wr"));
    }

    #[test]
    fn core_root_prefers_a_sibling_stdlib_core_directory() {
        let tmp = std::env::temp_dir().join(format!(
            "wrela-loader-test-{}-{}",
            std::process::id(),
            "sibling"
        ));
        let pkgroot = tmp.join("proj/src");
        let sibling_core = tmp.join("proj/stdlib/core");
        std::fs::create_dir_all(&pkgroot).expect("create pkgroot");
        std::fs::create_dir_all(&sibling_core).expect("create sibling stdlib/core");
        let root = core_root(&pkgroot, Span::default())
            .ok()
            .expect("sibling stdlib/core exists");
        assert_eq!(root, sibling_core);
        std::fs::remove_dir_all(&tmp).ok();
    }

    #[test]
    fn core_root_falls_back_to_the_toolchain_stdlib_core() {
        let tmp = std::env::temp_dir().join(format!(
            "wrela-loader-test-{}-{}",
            std::process::id(),
            "fallback"
        ));
        let pkgroot = tmp.join("proj/src");
        std::fs::create_dir_all(&pkgroot).expect("create pkgroot");
        let root = core_root(&pkgroot, Span::default())
            .ok()
            .expect("toolchain stdlib/core exists");
        assert_eq!(
            root,
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../../stdlib/core")
        );
        std::fs::remove_dir_all(&tmp).ok();
    }

    #[test]
    fn core_root_rejects_a_sibling_stdlib_without_core() {
        let tmp = std::env::temp_dir().join(format!(
            "wrela-loader-test-{}-{}",
            std::process::id(),
            "missing-core"
        ));
        let pkgroot = tmp.join("proj/src");
        let sibling_stdlib = tmp.join("proj/stdlib");
        std::fs::create_dir_all(&pkgroot).expect("create pkgroot");
        std::fs::create_dir_all(&sibling_stdlib).expect("create empty sibling stdlib");
        let err = core_root(&pkgroot, Span::default())
            .err()
            .expect("missing core");
        match err {
            LoadError::Build(e) => {
                assert!(e.message.contains("stdlib not found"));
                assert!(e.message.contains("no `core/` directory"));
            }
            LoadError::Lex(_) | LoadError::Parse(_) => {
                panic!("expected Build error, got Lex/Parse")
            }
        }
        std::fs::remove_dir_all(&tmp).ok();
    }

    #[test]
    fn visited_set_admits_a_two_module_cycle() {
        // A pure exercise of the walk's own termination logic, without
        // touching the filesystem: two modules whose only import is
        // each other must not infinite-loop and must load exactly the
        // two of them. Mirrors `load_closure`'s own loop body.
        let mut modules: BTreeMap<Vec<String>, Vec<Vec<String>>> = BTreeMap::new();
        modules.insert(vec![seg("a")], vec![vec![seg("b")]]);
        modules.insert(vec![seg("b")], vec![vec![seg("a")]]);

        let root = vec![seg("a")];
        let mut visited: BTreeMap<Vec<String>, ()> = BTreeMap::new();
        visited.insert(root.clone(), ());
        let mut queue = vec![root];
        let mut head = 0;
        while head < queue.len() {
            let current = queue[head].clone();
            head += 1;
            for target in &modules[&current] {
                if visited.contains_key(target) {
                    continue;
                }
                visited.insert(target.clone(), ());
                queue.push(target.clone());
            }
        }
        assert_eq!(visited.len(), 2);
    }
}
