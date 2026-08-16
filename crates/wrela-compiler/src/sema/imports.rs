use std::collections::{BTreeMap, BTreeSet};

use crate::sema::symbols::SymbolTable;
use crate::sema::{SemaError, unimplemented_at};
use crate::syntax::ast::{Item, Module};

pub struct ModuleExports {
    pub all: SymbolTable,
    pub public: BTreeSet<String>,
}

pub type Exports = BTreeMap<Vec<String>, ModuleExports>;

pub struct ImportBinding {
    pub target_module: Vec<String>,
    pub target_name: String,
}

pub type ImportBindings = BTreeMap<String, ImportBinding>;

/// Build every alias substitution table for one importer in a single pass.
///
/// Closure checking consults these tables for every imported declaration and
/// every transitively reached type. Re-scanning the import table at each such
/// site makes large prelude surfaces quadratic in their number of bindings.
pub(crate) fn alias_subs_by_exporter(
    bindings: &ImportBindings,
) -> BTreeMap<Vec<String>, BTreeMap<String, String>> {
    let mut by_exporter: BTreeMap<Vec<String>, BTreeMap<String, String>> = BTreeMap::new();
    for (local, binding) in bindings {
        if local != &binding.target_name {
            by_exporter
                .entry(binding.target_module.clone())
                .or_default()
                .insert(binding.target_name.clone(), local.clone());
        }
    }
    by_exporter
}

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

pub fn closure_type_shapes(
    modules: &[(Vec<String>, &Module)],
) -> BTreeMap<Vec<String>, BTreeMap<String, usize>> {
    modules
        .iter()
        .map(|(key, m)| (key.clone(), declared_type_shapes(m)))
        .collect()
}

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn alias_tables_are_grouped_by_exporter_without_identity_entries() {
        let alpha = vec!["alpha".to_string()];
        let beta = vec!["beta".to_string()];
        let bindings = ImportBindings::from([
            (
                "LocalA".to_string(),
                ImportBinding {
                    target_module: alpha.clone(),
                    target_name: "A".to_string(),
                },
            ),
            (
                "B".to_string(),
                ImportBinding {
                    target_module: alpha.clone(),
                    target_name: "B".to_string(),
                },
            ),
            (
                "LocalC".to_string(),
                ImportBinding {
                    target_module: beta.clone(),
                    target_name: "C".to_string(),
                },
            ),
        ]);

        assert_eq!(
            alias_subs_by_exporter(&bindings),
            BTreeMap::from([
                (
                    alpha,
                    BTreeMap::from([("A".to_string(), "LocalA".to_string())])
                ),
                (
                    beta,
                    BTreeMap::from([("C".to_string(), "LocalC".to_string())])
                ),
            ])
        );
    }
}
