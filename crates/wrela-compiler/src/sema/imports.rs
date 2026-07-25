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
//! declaration, never a module). Fails closed citing this plan rather
//! than approximating (plans/M4.md item A's own carve-out; see
//! `crate::loader`'s module doc for the loader-side half of this same
//! note).
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

/// Every name `module` exports for another module to import: its own
/// `pub const`/`fn`/`struct`/`enum` declarations, nothing re-exported
/// (see the module doc comment above).
pub fn public_names(module: &Module) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    for item in &module.items {
        let (name, is_pub) = match item {
            Item::Const(c) => (c.name.as_str(), c.is_pub),
            Item::Fn(f) => (f.name.as_str(), f.is_pub),
            Item::Struct(s) => (s.name.as_str(), s.is_pub),
            Item::Enum(e) => (e.name.as_str(), e.is_pub),
            Item::Pool(_) | Item::ComptimeIf(_) => continue,
        };
        if is_pub {
            out.insert(name.to_string());
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
            if crate::eval::image_checks::is_capability_type_name(&local_name) {
                return Err(SemaError::at(
                    "name",
                    format!(
                        "`{local_name}` is a capability type (03-hardware.md §1) and cannot be \
                         bound by an import: no import creates a capability"
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
