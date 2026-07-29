//! Whole-program import resolution (plans/M4.md item A, decision 2;
//! 02-language.md §2). `crate::loader` has already loaded every module
//! in the closure and guaranteed every imported module *path* resolves
//! to a real file (a missing file is `error[build]`, raised there), so
//! this module only ever answers name-level questions against
//! already-loaded modules: is the imported name declared at all, is it
//! `pub`, and does binding it locally collide with anything.
//!
//! `from core import <name>` (bare `core`, no further segment) is the
//! one shape this deliberately does not resolve: 02-language.md §2
//! reads it as the same construct as `from core.bytes import Bytes`,
//! but the `core` alias itself has no declaration to look inside — the
//! name always denotes a *submodule*, and binding a name to a whole
//! module (rather than a declaration) is a different kind of binding
//! this item does not build (`ImportBinding` only ever names a
//! declaration, never a module). Bare `from drivers import <name>` is
//! the same carve-out for the `drivers` reserved alias (plans/M16.md
//! item B). Fails closed citing this plan rather than approximating
//! (plans/M4.md item A's own carve-out; see `crate::loader`'s module
//! doc for the loader-side half of this same note).
//!
//! A module's own public surface (`public_names`) is exactly its own
//! `pub const`/`fn`/`struct`/`enum` declarations — `pool` is
//! grammatically `pub`-less (02-language.md §4) and so never
//! importable, and `pub from ... import ...` re-export is *not*
//! implemented by this item: a module's exports are only ever what it
//! declares itself, never what it re-exports. This is a real, narrow
//! scope gap (re-exports would need a fixed-point pass across the whole
//! closure, since a re-export chain can be arbitrarily deep), recorded
//! here rather than silently mishandled — no required golden case
//! exercises `pub from`.

use std::collections::{BTreeMap, BTreeSet};

use crate::sema::symbols::SymbolTable;
use crate::sema::{SemaError, unimplemented_at};
use crate::syntax::ast::{Item, Module};

/// One module's export surface, built once per module in the closure
/// before any module's imports are resolved against it (decision 2's
/// "global symbol table: module path -> public names"). `all` keeps
/// every declared name (not just the `pub` ones) so "the name does not
/// exist" and "the name exists but is not `pub`" stay two different
/// diagnostics.
pub struct ModuleExports {
    pub all: SymbolTable,
    pub public: BTreeSet<String>,
}

/// The whole build's export table, keyed exactly like
/// `loader::LoadedProgram::modules` (a plain module's own dotted path,
/// or `["core", ...]` for a toolchain module).
pub type Exports = BTreeMap<Vec<String>, ModuleExports>;

/// One resolved import binding: `from binding.target_module import
/// binding.target_name [as <the map's own key>]`. Imports run no code
/// (02-language.md §2), so this is nothing but a name pointer;
/// *consuming* it — making the referenced declaration's already-checked
/// shape usable in the importing module's own body — is `sema::mod`'s
/// job (the splice step in `check_program`), not this file's.
pub struct ImportBinding {
    pub target_module: Vec<String>,
    pub target_name: String,
}

/// Local name introduced by an import -> what it refers to.
pub type ImportBindings = BTreeMap<String, ImportBinding>;

/// Exporter→local substitution for one exporting module, from an
/// importer's bindings. Only non-identity entries (aliased imports).
/// plans/M9.md item GG: applied in one simultaneous pass at the splice
/// — never chained — so an adversarial swap (`Src as Inner` + `Inner as
/// Src` from two modules) cannot transpose through order dependence.
pub(crate) fn alias_subs_for_exporter(
    bindings: &ImportBindings,
    target_module: &[String],
) -> BTreeMap<String, String> {
    let mut subs = BTreeMap::new();
    for (local, b) in bindings {
        if b.target_module.as_slice() == target_module && local != &b.target_name {
            subs.insert(b.target_name.clone(), local.clone());
        }
    }
    subs
}

/// Every name `module` exports for another module to import: its own
/// `pub const`/`fn`/`struct`/`enum`/`static` declarations, nothing
/// re-exported (see the module doc comment above). `pub static` is
/// required so `runtime.wr` can name generated `RT` / `GROUPS`
/// (plans/M11.md item E / decision 785).
pub fn public_names(module: &Module) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    for item in &module.items {
        let (name, is_pub) = match item {
            Item::Const(c) => (c.name.as_str(), c.is_pub),
            Item::Fn(f) => (f.name.as_str(), f.is_pub),
            Item::Struct(s) => (s.name.as_str(), s.is_pub),
            Item::Enum(e) => (e.name.as_str(), e.is_pub),
            Item::Static(s) => (s.name.as_str(), s.is_pub),
            Item::Pool(_) | Item::ComptimeIf(_) => continue,
        };
        if is_pub {
            out.insert(name.to_string());
        }
    }
    out
}

/// Every `struct`/`enum` a module declares, name -> its own
/// generic-parameter *count* — read straight off raw AST, so it needs
/// nothing from any module's `types::declare` (plans/M9.md item A1,
/// decision 8). That is exactly what keeps import cycles free: a
/// module's arity table is available to every other module before any
/// module's `declare` has run, so no module's declaration pass ever
/// waits on another's (`sema::check_program_typed`'s splice note is the
/// property being preserved).
///
/// Call it on an **already-specialized** module (decision 11). A
/// `struct`/`enum` declared inside a module-level `comptime if` exists
/// only after `specialize`, and every caller — `sema::check_program_typed`,
/// `sema::dump_program`, `layout::closure_imported_types` — specializes
/// first, so no two passes can disagree about which type names exist.
pub fn declared_type_shapes(module: &Module) -> BTreeMap<String, usize> {
    let mut out = BTreeMap::new();
    for item in &module.items {
        match item {
            Item::Struct(s) => {
                out.insert(s.name.clone(), s.generics.len());
            }
            Item::Enum(e) => {
                out.insert(e.name.clone(), e.generics.len());
            }
            _ => {}
        }
    }
    out
}

/// `declared_type_shapes` for every module in a build closure, keyed by
/// **module address** exactly like `Exports` above — the loader's own key,
/// not the module's declared `path`. The two differ for a toolchain
/// module: `stdlib/tiny.wr` says `module tiny` and is addressed
/// `["core", "tiny"]`, which is the spelling `import.path` carries.
pub fn closure_type_shapes(
    modules: &[(Vec<String>, &Module)],
) -> BTreeMap<Vec<String>, BTreeMap<String, usize>> {
    modules
        .iter()
        .map(|(key, m)| (key.clone(), declared_type_shapes(m)))
        .collect()
}

/// The type-name half of `module`'s own import list: local (possibly
/// aliased) name -> the exporting declaration's generic-parameter count
/// (plans/M9.md item A1). `types::declare` merges this straight into its
/// own module-local arity table, which is the whole of "an imported
/// `struct`/`enum` name is legal wherever a type is legal".
///
/// Read off raw AST on both sides — this module's `import` statements and
/// the exporting module's `struct`/`enum` headers — for the cycle reason
/// in `declared_type_shapes` above. An imported name that is not a type
/// (a `const`, a `fn`) simply does not appear here, so writing it in type
/// position keeps `error[type]: unknown type`, identical to the local
/// case. Every *validity* question about the import itself — the name
/// exists, it is `pub`, it does not collide, it does not alias a sealed
/// authority type — is `resolve_imports` below, which has already run and
/// failed fast by the time any caller of this reads the table.
pub fn imported_type_shapes(
    module: &Module,
    closure: &BTreeMap<Vec<String>, BTreeMap<String, usize>>,
) -> BTreeMap<String, usize> {
    let mut out = BTreeMap::new();
    for import in &module.imports {
        let Some(shapes) = closure.get(&import.path) else {
            continue;
        };
        for name in &import.names {
            let Some(arity) = shapes.get(&name.name) else {
                continue;
            };
            let local = name.alias.clone().unwrap_or_else(|| name.name.clone());
            out.insert(local, *arity);
        }
    }
    out
}

/// The same table as `imported_type_shapes`, but carrying *where* each
/// imported type is declared instead of its arity — the input
/// `types::classify_closure` needs to follow a local struct's field of
/// imported type into the exporting module's own declarations
/// (plans/M9.md item A1, decision 10).
pub fn imported_type_targets(
    module: &Module,
    closure: &BTreeMap<Vec<String>, BTreeMap<String, usize>>,
) -> BTreeMap<String, (Vec<String>, String)> {
    let mut out = BTreeMap::new();
    for import in &module.imports {
        let Some(shapes) = closure.get(&import.path) else {
            continue;
        };
        for name in &import.names {
            if !shapes.contains_key(&name.name) {
                continue;
            }
            let local = name.alias.clone().unwrap_or_else(|| name.name.clone());
            out.insert(local, (import.path.clone(), name.name.clone()));
        }
    }
    out
}

/// Builds `module`'s own import bindings against the whole-program
/// `exports` table (`local` is this same module's own `symbols::collect`
/// table, needed only for the local-collision check below) — source
/// order, fail-fast, exactly like every other sema pass (plans/M2.md
/// decision 1). A missing *module* can never surface here: the loader
/// already guaranteed every `import.path` this function sees resolved
/// to a real, loaded file before sema ever ran (see `crate::loader`).
pub fn resolve_imports(
    module: &Module,
    local: &SymbolTable,
    exports: &Exports,
) -> Result<ImportBindings, SemaError> {
    let mut bindings = ImportBindings::new();
    for import in &module.imports {
        if import.path.len() == 1 && import.path[0] == "core" {
            return Err(unimplemented_at(
                "a submodule import (`from core import <name>` names a module, not a declaration)",
                import.span,
            ));
        }
        if import.path.len() == 1 && import.path[0] == "drivers" {
            return Err(unimplemented_at(
                "a submodule import (`from drivers import <name>` names a module, not a declaration)",
                import.span,
            ));
        }
        let target = exports.get(&import.path).unwrap_or_else(|| {
            panic!(
                "the loader guarantees every imported module path is loaded: `{}`",
                import.path.join(".")
            )
        });
        for name in &import.names {
            if !target.all.contains_key(&name.name) {
                return Err(SemaError::at(
                    "name",
                    format!(
                        "module `{}` has no item `{}`",
                        import.path.join("."),
                        name.name
                    ),
                    name.span,
                ));
            }
            if !target.public.contains(&name.name) {
                return Err(SemaError::at(
                    "name",
                    format!(
                        "`{}` in module `{}` is not `pub`",
                        name.name,
                        import.path.join(".")
                    ),
                    name.span,
                ));
            }
            let local_name = name.alias.clone().unwrap_or_else(|| name.name.clone());
            // 03-hardware.md §1 (plans/M7.md item A): "no address,
            // **import**, or cast creates one." Checked on the *local*
            // name, which is what an alias controls — `from d import Thing
            // as Mmio` binds a real, constructible declaration under a
            // capability's name and is the exact shape the sentence
            // forbids. The exporting module could never have declared the
            // name in the first place (`symbols::collect` rejects it),
            // so this arm is specifically about the alias.
            if crate::sema::classes::name_holds_authority(&local_name) {
                let kind = crate::eval::image_checks::sealed_authority_kind(&local_name);
                return Err(SemaError::at(
                    "name",
                    format!(
                        "`{local_name}` is {kind} and cannot be bound by an import: no import \
                         creates one"
                    ),
                    name.span,
                ));
            }
            if local.contains_key(&local_name) {
                return Err(SemaError::at(
                    "name",
                    format!("import `{local_name}` collides with a local declaration"),
                    name.span,
                ));
            }
            if bindings.contains_key(&local_name) {
                return Err(SemaError::at(
                    "name",
                    format!("import `{local_name}` is bound more than once"),
                    name.span,
                ));
            }
            bindings.insert(
                local_name,
                ImportBinding {
                    target_module: import.path.clone(),
                    target_name: name.name.clone(),
                },
            );
        }
    }
    Ok(bindings)
}

// --- plans/M9.md item HH: import type-universe reachability --------------
//
// The importer's type universe is the explicitly imported names **plus**
// every type reachable through those declarations' signatures (and, at
// the ModuleCtx / TypedProgram splices, the finished decl bodies those
// signatures name). A binding is still required to *spell* a name
// (02 §2); reachability closes the universe for *using a value* whose
// type the importer never wrote. A reachable type that is not `pub` is
// still refused *by name* at `resolve_imports`
// (golden/err-import-type-not-pub) — privacy governs the import act, not
// inference over a value a pub API already handed across.

pub(crate) fn lookup_origin_type_name<'a>(
    tname: &str,
    origin: &[String],
    importer_bindings: &ImportBindings,
) -> String {
    for (local, b) in importer_bindings {
        if local.as_str() == tname && b.target_module.as_slice() == origin {
            return b.target_name.clone();
        }
    }
    tname.to_string()
}
