use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use crate::sema::SemaError;
use crate::syntax::ast::{Expr, Item, Member, Module, Span};
use crate::syntax::{lexer, parser};

pub enum LoadError {
    Lex(lexer::LexError),
    Parse(parser::ParseError),
    Build(SemaError),
}

pub struct LoadedModule {
    pub file: PathBuf,
    pub module: Module,
}

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

pub(crate) fn anchor_package_root(
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

fn ensure_under_package_root(
    file: &Path,
    expected_root: &Path,
    module_key: &[String],
    span: Span,
) -> Result<(), LoadError> {
    let root_canon = expected_root.canonicalize().map_err(|e| {
        build_error(
            format!(
                "cannot canonicalize package root `{}`: {e}",
                expected_root.display()
            ),
            span,
        )
    })?;
    let file_canon = file.canonicalize().map_err(|e| {
        build_error(
            format!(
                "cannot canonicalize module `{}` at `{}`: {e}",
                module_key.join("."),
                file.display()
            ),
            span,
        )
    })?;
    if file_canon.strip_prefix(&root_canon).is_err() {
        return Err(build_error(
            format!(
                "module `{}` resolves outside package root via symlink or path remap",
                module_key.join(".")
            ),
            span,
        ));
    }
    Ok(())
}

fn module_file_path(root: &Path, module_path: &[String]) -> PathBuf {
    let mut p = root.to_path_buf();
    for seg in module_path {
        p.push(seg);
    }
    p.set_extension("wr");
    p
}

pub fn toolchain_stdlib_core() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../stdlib/core")
}

pub fn stdlib_core_root(pkgroot: &Path, span: Span) -> Result<PathBuf, LoadError> {
    if let Some(parent) = pkgroot.parent() {
        let sibling_stdlib = parent.join("stdlib");
        if sibling_stdlib.is_dir() {
            let core = sibling_stdlib.join("core");
            if core.is_dir() {
                return Ok(core);
            }
            return Err(build_error(
                "stdlib not found: sibling `stdlib/` exists but has no `core/` directory"
                    .to_string(),
                span,
            ));
        }
    }
    let toolchain = toolchain_stdlib_core();
    if !toolchain.is_dir() {
        return Err(build_error(
            "stdlib not found: toolchain `stdlib/core/` is missing".to_string(),
            span,
        ));
    }
    Ok(toolchain)
}

fn core_root(pkgroot: &Path, span: Span) -> Result<PathBuf, LoadError> {
    stdlib_core_root(pkgroot, span)
}

pub fn toolchain_stdlib_drivers() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../stdlib/drivers")
}

pub fn stdlib_drivers_root(pkgroot: &Path, span: Span) -> Result<PathBuf, LoadError> {
    if let Some(parent) = pkgroot.parent() {
        let sibling_stdlib = parent.join("stdlib");
        if sibling_stdlib.is_dir() {
            let drivers = sibling_stdlib.join("drivers");
            if drivers.is_dir() {
                return Ok(drivers);
            }
            return Err(build_error(
                "stdlib not found: sibling `stdlib/` exists but has no `drivers/` directory"
                    .to_string(),
                span,
            ));
        }
    }
    let toolchain = toolchain_stdlib_drivers();
    if !toolchain.is_dir() {
        return Err(build_error(
            "stdlib not found: toolchain `stdlib/drivers/` is missing".to_string(),
            span,
        ));
    }
    Ok(toolchain)
}

fn drivers_root(pkgroot: &Path, span: Span) -> Result<PathBuf, LoadError> {
    stdlib_drivers_root(pkgroot, span)
}

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
    } else if import_path[0] == "drivers" {
        let root = drivers_root(pkgroot, span)?;
        let rest = &import_path[1..];
        let file = module_file_path(&root, rest);
        Ok((import_path.to_vec(), file, root))
    } else {
        let file = module_file_path(pkgroot, import_path);
        Ok((import_path.to_vec(), file, pkgroot.to_path_buf()))
    }
}

pub fn load_closure(root_file: &Path) -> Result<LoadedProgram, LoadError> {
    load_closure_with_discovery_order(root_file, false)
}

pub fn load_closure_with_discovery_order(
    root_file: &Path,
    reverse_imports: bool,
) -> Result<LoadedProgram, LoadError> {
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

    let mut queue: Vec<Vec<String>> = vec![root_key.clone()];
    let mut head = 0;
    while head < queue.len() {
        let current = queue[head].clone();
        head += 1;
        let mut imports = modules[&current].module.imports.clone();
        if reverse_imports {
            imports.reverse();
        }
        for import in &imports {
            if import.path.len() == 1 && (import.path[0] == "core" || import.path[0] == "drivers") {
                continue;
            }
            if is_image_runtime_import(&import.path) {
                continue;
            }
            let (key, file, expected_root) = import_target(&pkgroot, &import.path, import.span)?;
            if modules.contains_key(&key) {
                continue;
            }
            if !file.is_file() {
                return Err(build_error(
                    format!("module `{}` not found: no such file", key.join(".")),
                    import.span,
                ));
            }
            ensure_under_package_root(&file, &expected_root, &key, import.span)?;
            let module = parse_file(&file)?;
            check_agrees(&file, &module, &expected_root)?;
            queue.push(key.clone());
            modules.insert(key, LoadedModule { file, module });
        }
    }

    if closure_mentions_time(&modules) {
        ensure_time_module(&pkgroot, &mut modules)?;
    }

    if closure_is_runtime_bearing(&modules) {
        ensure_runtime_module(&pkgroot, &mut modules)?;
        ensure_image_runtime_stub(&mut modules)?;
    }

    Ok(LoadedProgram {
        root: root_key,
        modules,
    })
}

pub const TIME_MODULE_KEY: &[&str] = &["core", "time"];

pub fn closure_mentions_time(modules: &BTreeMap<Vec<String>, LoadedModule>) -> bool {
    modules.values().any(|m| module_mentions_time(&m.module))
}

pub fn module_mentions_time(module: &Module) -> bool {
    let text = crate::syntax::printer::pretty(module);
    text.split(|c: char| !c.is_ascii_alphanumeric() && c != '_')
        .any(|tok| tok == "now" || TIME_PRELUDE_NAMES.contains(&tok))
}

pub use crate::sema::prelude_scope::TIME_PRELUDE_NAMES;

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
    ensure_under_package_root(&file, &expected_root, &key, Span::default())?;
    let module = parse_file(&file)?;
    check_agrees(&file, &module, &expected_root)?;
    modules.insert(key, LoadedModule { file, module });
    Ok(())
}

pub fn load_time_module() -> Result<(Vec<String>, LoadedModule), LoadError> {
    let key: Vec<String> = TIME_MODULE_KEY.iter().map(|s| (*s).to_string()).collect();
    let toolchain = toolchain_stdlib_core();
    let file = toolchain.join("time.wr");
    if !file.is_file() {
        return Err(build_error(
            "stdlib not found: toolchain `stdlib/core/time.wr` is missing".to_string(),
            Span::default(),
        ));
    }
    ensure_under_package_root(&file, &toolchain, &key, Span::default())?;
    let module = parse_file(&file)?;
    check_agrees(&file, &module, &toolchain)?;
    Ok((key, LoadedModule { file, module }))
}

pub const RUNTIME_MODULE_KEY: &[&str] = &["core", "runtime"];

pub const IMAGE_RUNTIME_MODULE_KEY: &[&str] = &["core", "__image_runtime"];

pub const RUNTIME_INPUT_PATH: &str = "core/runtime.wr";

pub fn ensure_image_runtime_stub(
    modules: &mut BTreeMap<Vec<String>, LoadedModule>,
) -> Result<(), LoadError> {
    let key: Vec<String> = IMAGE_RUNTIME_MODULE_KEY
        .iter()
        .map(|s| (*s).to_string())
        .collect();
    if modules.contains_key(&key) {
        return Ok(());
    }
    let text = crate::rtconfig::stub_text();
    let tokens = crate::syntax::lexer::lex(&text)
        .map_err(|e| build_error(format!("rtconfig stub lex: {}", e.message), Span::default()))?;
    let module = crate::syntax::parser::parse(tokens).map_err(|e| {
        build_error(
            format!("rtconfig stub parse: {}", e.message),
            Span::default(),
        )
    })?;
    modules.insert(
        key,
        LoadedModule {
            file: PathBuf::from(crate::rtconfig::GENERATED_INPUT_PATH),
            module,
        },
    );
    Ok(())
}

pub fn is_image_runtime_import(path: &[String]) -> bool {
    path.len() == IMAGE_RUNTIME_MODULE_KEY.len()
        && path
            .iter()
            .zip(IMAGE_RUNTIME_MODULE_KEY.iter())
            .all(|(a, b)| a == *b)
}

pub fn closure_is_runtime_bearing(modules: &BTreeMap<Vec<String>, LoadedModule>) -> bool {
    modules
        .values()
        .any(|m| module_is_runtime_bearing(&m.module))
}

pub fn module_is_runtime_bearing(module: &Module) -> bool {
    items_are_runtime_bearing(&module.items)
}

fn items_are_runtime_bearing(items: &[Item]) -> bool {
    for item in items {
        match item {
            Item::Fn(f) => {
                if f.is_async || fn_is_runtime_test(f) {
                    return true;
                }
            }
            Item::Struct(s) => {
                if s.attrs
                    .iter()
                    .any(|a| a.name == "actor" || a.name == "driver")
                {
                    return true;
                }
                for member in &s.members {
                    if members_are_runtime_bearing(member) {
                        return true;
                    }
                }
            }
            Item::ComptimeIf(c) => {
                if items_are_runtime_bearing(&c.then_branch) {
                    return true;
                }
                if let Some(else_branch) = &c.else_branch {
                    if items_are_runtime_bearing(else_branch) {
                        return true;
                    }
                }
            }
            Item::Const(_) | Item::Static(_) | Item::Enum(_) | Item::Pool(_) => {}
        }
    }
    false
}

fn members_are_runtime_bearing(member: &Member) -> bool {
    match member {
        Member::Fn(f) => f.is_async || fn_is_runtime_test(f),
        Member::ComptimeIf(c) => {
            c.then_branch.iter().any(members_are_runtime_bearing)
                || c.else_branch
                    .as_ref()
                    .is_some_and(|b| b.iter().any(members_are_runtime_bearing))
        }
        Member::Field(_) | Member::Init(_) | Member::Pool(_) => false,
    }
}

fn fn_is_runtime_test(f: &crate::syntax::ast::FnItem) -> bool {
    f.attrs.iter().any(|a| {
        a.name == "test"
            && a.args.iter().any(|arg| {
                arg.label.is_none()
                    && matches!(&arg.value, Expr::Name(_, name) if name == "runtime")
            })
    })
}

pub fn ensure_runtime_module(
    pkgroot: &Path,
    modules: &mut BTreeMap<Vec<String>, LoadedModule>,
) -> Result<(), LoadError> {
    let key: Vec<String> = RUNTIME_MODULE_KEY
        .iter()
        .map(|s| (*s).to_string())
        .collect();
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
    ensure_under_package_root(&file, &expected_root, &key, Span::default())?;
    let module = parse_file(&file)?;
    check_agrees(&file, &module, &expected_root)?;
    modules.insert(key, LoadedModule { file, module });
    Ok(())
}

pub fn load_runtime_module() -> Result<(Vec<String>, LoadedModule), LoadError> {
    let key: Vec<String> = RUNTIME_MODULE_KEY
        .iter()
        .map(|s| (*s).to_string())
        .collect();
    let toolchain = toolchain_stdlib_core();
    let file = toolchain.join("runtime.wr");
    if !file.is_file() {
        return Err(build_error(
            "stdlib not found: toolchain `stdlib/core/runtime.wr` is missing".to_string(),
            Span::default(),
        ));
    }
    ensure_under_package_root(&file, &toolchain, &key, Span::default())?;
    let module = parse_file(&file)?;
    check_agrees(&file, &module, &toolchain)?;
    Ok((key, LoadedModule { file, module }))
}

pub fn load_runtime_module_with_image_runtime_import()
-> Result<(Vec<String>, LoadedModule), LoadError> {
    load_runtime_module()
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
mod runtime_module_tests {
    use super::*;

    #[test]
    fn load_runtime_module_parses() {
        let (key, loaded) = match load_runtime_module() {
            Ok(v) => v,
            Err(_) => panic!("runtime.wr exists"),
        };
        assert_eq!(key, vec!["core".to_string(), "runtime".to_string()]);
        assert_eq!(loaded.module.path, vec!["runtime".to_string()]);
    }

    #[test]
    fn runtime_test_fn_is_runtime_bearing() {
        let src = "module m\n\n@test(runtime)\npub fn t():\n    return\n";
        let tokens = lexer::lex(src).expect("lex");
        let module = parser::parse(tokens).expect("parse");
        assert!(module_is_runtime_bearing(&module));
    }

    #[test]
    fn plain_image_is_not_runtime_bearing() {
        let src = "module m\n\n@image\npub fn build() -> Image:\n    return Image(name=\"x\", target=Target.wrela_machine_v1).seal()\n";
        let tokens = lexer::lex(src).expect("lex");
        let module = parser::parse(tokens).expect("parse");
        assert!(!module_is_runtime_bearing(&module));
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
    fn drivers_root_prefers_a_sibling_stdlib_drivers_directory() {
        let tmp = std::env::temp_dir().join(format!(
            "wrela-loader-test-{}-{}",
            std::process::id(),
            "sibling-drivers"
        ));
        let pkgroot = tmp.join("proj/src");
        let sibling_drivers = tmp.join("proj/stdlib/drivers");
        std::fs::create_dir_all(&pkgroot).expect("create pkgroot");
        std::fs::create_dir_all(&sibling_drivers).expect("create sibling stdlib/drivers");
        let root = drivers_root(&pkgroot, Span::default())
            .ok()
            .expect("sibling stdlib/drivers exists");
        assert_eq!(root, sibling_drivers);
        std::fs::remove_dir_all(&tmp).ok();
    }

    #[test]
    fn drivers_root_falls_back_to_the_toolchain_stdlib_drivers() {
        let tmp = std::env::temp_dir().join(format!(
            "wrela-loader-test-{}-{}",
            std::process::id(),
            "fallback-drivers"
        ));
        let pkgroot = tmp.join("proj/src");
        std::fs::create_dir_all(&pkgroot).expect("create pkgroot");
        let root = drivers_root(&pkgroot, Span::default())
            .ok()
            .expect("toolchain stdlib/drivers exists");
        assert_eq!(
            root,
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../../stdlib/drivers")
        );
        std::fs::remove_dir_all(&tmp).ok();
    }

    #[test]
    fn drivers_root_rejects_a_sibling_stdlib_without_drivers() {
        let tmp = std::env::temp_dir().join(format!(
            "wrela-loader-test-{}-{}",
            std::process::id(),
            "missing-drivers"
        ));
        let pkgroot = tmp.join("proj/src");
        let sibling_stdlib = tmp.join("proj/stdlib");
        std::fs::create_dir_all(&pkgroot).expect("create pkgroot");
        std::fs::create_dir_all(&sibling_stdlib).expect("create empty sibling stdlib");
        let err = drivers_root(&pkgroot, Span::default())
            .err()
            .expect("missing drivers");
        match err {
            LoadError::Build(e) => {
                assert!(e.message.contains("stdlib not found"));
                assert!(e.message.contains("no `drivers/` directory"));
            }
            LoadError::Lex(_) | LoadError::Parse(_) => {
                panic!("expected Build error, got Lex/Parse")
            }
        }
        std::fs::remove_dir_all(&tmp).ok();
    }

    #[test]
    fn visited_set_admits_a_two_module_cycle() {
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

    #[test]
    fn refuses_a_symlink_that_escapes_the_package_root() {
        let tmp = std::env::temp_dir().join(format!(
            "wrela-loader-symlink-{}-{}",
            std::process::id(),
            "escape"
        ));
        let _ = std::fs::remove_dir_all(&tmp);
        let pkgroot = tmp.join("pkg");
        let outside = tmp.join("outside");
        std::fs::create_dir_all(&pkgroot).expect("pkgroot");
        std::fs::create_dir_all(&outside).expect("outside");
        let outside_file = outside.join("secret.wr");
        std::fs::write(&outside_file, "module secret\n").expect("write outside");
        let link = pkgroot.join("secret.wr");
        #[cfg(unix)]
        std::os::unix::fs::symlink(&outside_file, &link).expect("symlink");
        #[cfg(not(unix))]
        {
            let _ = (outside_file, link);
            return;
        }
        let err = ensure_under_package_root(
            &pkgroot.join("secret.wr"),
            &pkgroot,
            &[seg("secret")],
            Span::default(),
        )
        .expect_err("symlink escape");
        match err {
            LoadError::Build(e) => {
                assert!(
                    e.message.contains("outside package root"),
                    "got {}",
                    e.message
                );
            }
            LoadError::Lex(_) | LoadError::Parse(_) => panic!("expected Build"),
        }
        let _ = std::fs::remove_dir_all(&tmp);
    }
}
