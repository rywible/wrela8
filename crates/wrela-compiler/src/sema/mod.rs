pub mod access;
pub mod actor;
pub mod attrs;
pub mod bodies;
pub mod classes;
pub mod flow;
pub mod fstring;
pub mod generics;
pub mod handoff;
pub mod imports;
pub mod intrinsics;
pub mod layout_types;
pub mod matches;
pub mod paths;
pub mod prelude_scope;
pub mod reserve_proof;
pub mod send_proof;
pub mod specialize;
pub mod stdlib_enums;
pub mod sum;
pub mod symbols;
pub mod transport;
pub mod typed;
pub mod types;

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use crate::syntax::ast::{Module, Span};

#[derive(Debug)]
pub struct SemaError {
    pub category: &'static str,
    pub message: String,
    pub line: u32,
    pub col: u32,
    pub extra_lines: Vec<String>,
    pub omit_location: bool,
    pub missing_method: Option<(String, String)>,
}

impl SemaError {
    pub(crate) fn at(category: &'static str, message: String, span: Span) -> SemaError {
        SemaError {
            category,
            message,
            line: span.line,
            col: span.col,
            extra_lines: Vec::new(),
            omit_location: false,
            missing_method: None,
        }
    }

    pub(crate) fn nowhere(category: &'static str, message: String) -> SemaError {
        SemaError {
            category,
            message,
            line: 0,
            col: 0,
            extra_lines: Vec::new(),
            omit_location: true,
            missing_method: None,
        }
    }
}

pub fn unimplemented_at(subject: &str, span: Span) -> SemaError {
    SemaError::at("unimplemented", format!("{subject} not checked yet"), span)
}

pub fn check(module: &Module, path: &str) -> Result<(), SemaError> {
    check_typed(module, path).map(|_| ())
}

pub fn check_typed(module: &Module, path: &str) -> Result<typed::TypedProgram, SemaError> {
    if let Some(import) = module.imports.first() {
        return Err(unimplemented_at(
            "imports through the single-module entry (`--stage=typed`, `wrela test`) are",
            import.span,
        ));
    }
    let text = crate::syntax::printer::pretty(module);
    let needs_time = text
        .split(|c: char| !c.is_ascii_alphanumeric() && c != '_')
        .any(|tok| tok == "now" || crate::loader::TIME_PRELUDE_NAMES.contains(&tok));
    if needs_time {
        return check_typed_with_time_prelude(module, path);
    }
    check_typed_single(module, path)
}

fn check_typed_with_time_prelude(
    module: &Module,
    path: &str,
) -> Result<typed::TypedProgram, SemaError> {
    let (time_key, time_loaded) = load_time_module_as_sema()?;
    let root_key = module.path.clone();
    let time_path = time_loaded.file.display().to_string();
    let mut modules = BTreeMap::new();
    modules.insert(root_key.clone(), module.clone());
    modules.insert(time_key.clone(), time_loaded.module);
    let mut paths = BTreeMap::new();
    paths.insert(root_key.clone(), path.to_string());
    paths.insert(time_key, time_path);
    let mut progs = check_program_typed(&modules, &paths)?;
    progs.remove(&root_key).ok_or_else(|| {
        SemaError::at(
            "internal",
            "internal error: time-prelude check lost the root module".to_string(),
            Span::default(),
        )
    })
}

fn check_typed_single(module: &Module, path: &str) -> Result<typed::TypedProgram, SemaError> {
    check_typed_single_with_decls(module, path).map(|(p, _)| p)
}

fn check_typed_single_dump(module: &Module, path: &str) -> Result<String, SemaError> {
    let (program, decl_items) = check_typed_single_with_decls(module, path)?;
    dump_with_imports(
        module,
        &types::ImportedTypes::new(),
        Some(&decl_items),
        Some(&program.effects),
    )
}

fn check_typed_single_with_decls(
    module: &Module,
    path: &str,
) -> Result<(typed::TypedProgram, Vec<types::DeclItem>), SemaError> {
    check_reserved_source_names(module, path)?;
    prepare_stdlib_enums_for_file(path, module)?;
    let specialized = specialize::specialize(module)?;
    let layouts = types::check_layouts(&specialized)?;
    let symtab = symbols::collect(&specialized)?;
    symbols::resolve(&specialized, &symtab, &imports::ImportBindings::new())?;
    let mut decl_items = types::declare(&specialized)?;
    types::validate_placed_statics(&decl_items, &layouts)?;
    types::check_mmio_claims(&specialized, &decl_items, &layouts)?;
    let mctx = bodies::build_module_ctx(&specialized, &decl_items, &types::ImportedTypes::new());
    let mut program = bodies::check(&specialized, &decl_items, &mctx)?;
    sync_inferred_error_sets(&mut decl_items, &mctx.inferred_rets.borrow());
    program.layouts = layouts;
    access::check(&mut program, &mctx)?;
    flow::check(&program, &mctx)?;
    handoff::check(&specialized, &decl_items, &mctx)?;
    matches::check(&program, &mctx)?;
    program.instantiations = generics::check(&specialized, &decl_items, &mctx, path)?;
    crate::eval::check_comptime(&program)?;
    let mut layouts = std::mem::take(&mut program.layouts);
    types::complete_layouts(&specialized, &program, &mut layouts)?;
    program.layouts = layouts;
    crate::eval::legal::check_provenance(
        &program,
        &types::capability_authority(&specialized, &decl_items),
    )?;
    crate::eval::legal::check_isr_effects(&program)?;
    crate::eval::legal::check_wake_sites(&program)?;
    crate::eval::legal::check_bottom_half(&program)?;
    let one = BTreeMap::from([(specialized.path.join("."), &program)]);
    send_proof::check(&one)?;
    reserve_proof::check(&one)?;
    {
        let observes = crate::eval::observes::classify(&program);
        crate::eval::observes::check_loop_discharge(
            &program,
            &observes,
            &program.unbounded_sync_loops,
        )?;
    }
    Ok((program, decl_items))
}

fn sync_inferred_error_sets(
    decl_items: &mut [types::DeclItem],
    inferred: &BTreeMap<String, types::Type>,
) {
    for item in decl_items.iter_mut() {
        match item {
            types::DeclItem::Fn(f) => {
                if let Some(ret) = inferred.get(&f.name) {
                    f.ret = ret.clone();
                }
            }
            types::DeclItem::Struct(s) => {
                for m in &mut s.members {
                    match m {
                        types::DeclMember::Fn(f) | types::DeclMember::Init(f) => {
                            let key = format!("{}.{}", s.name, f.name);
                            if let Some(ret) = inferred.get(&key) {
                                f.ret = ret.clone();
                            }
                        }
                        _ => {}
                    }
                }
            }
            types::DeclItem::Enum(e) => {
                for m in &mut e.members {
                    if let types::DeclMember::Fn(f) = m {
                        let key = format!("{}.{}", e.name, f.name);
                        if let Some(ret) = inferred.get(&key) {
                            f.ret = ret.clone();
                        }
                    }
                }
            }
            _ => {}
        }
    }
}

pub fn dump_typed(program: &typed::TypedProgram) -> String {
    let mut out = typed::dump(program);
    let observes = crate::eval::observes::classify(program);
    out.push_str(&crate::eval::observes::dump(&observes));
    out
}

pub fn dump(module: &Module) -> Result<String, SemaError> {
    let text = crate::syntax::printer::pretty(module);
    let needs_time = text
        .split(|c: char| !c.is_ascii_alphanumeric() && c != '_')
        .any(|tok| tok == "now" || crate::loader::TIME_PRELUDE_NAMES.contains(&tok));
    if needs_time {
        let (time_key, time_loaded) = load_time_module_as_sema()?;
        let mut modules = BTreeMap::new();
        modules.insert(module.path.clone(), module.clone());
        modules.insert(time_key, time_loaded.module);
        return dump_program(&modules);
    }
    dump_with_imports(module, &types::ImportedTypes::new(), None, None)
}

pub fn check_dump(module: &Module, path: &str) -> Result<String, SemaError> {
    if let Some(import) = module.imports.first() {
        return Err(unimplemented_at(
            "imports through the single-module entry (`--stage=typed`, `wrela test`) are",
            import.span,
        ));
    }
    let text = crate::syntax::printer::pretty(module);
    let needs_time = text
        .split(|c: char| !c.is_ascii_alphanumeric() && c != '_')
        .any(|tok| tok == "now" || crate::loader::TIME_PRELUDE_NAMES.contains(&tok));
    if needs_time {
        let (time_key, time_loaded) = load_time_module_as_sema()?;
        let root_key = module.path.clone();
        let time_path = time_loaded.file.display().to_string();
        let mut modules = BTreeMap::new();
        modules.insert(root_key.clone(), module.clone());
        modules.insert(time_key.clone(), time_loaded.module);
        let mut paths = BTreeMap::new();
        paths.insert(root_key, path.to_string());
        paths.insert(time_key, time_path);
        return check_program_dump(&modules, &paths);
    }
    check_typed_single_dump(module, path)
}

pub fn check_program_dump(
    modules: &BTreeMap<Vec<String>, Module>,
    paths: &BTreeMap<Vec<String>, String>,
) -> Result<String, SemaError> {
    let (programs, tables) = check_program_typed_tables(modules, paths)?;
    render_check_dump(modules, &programs, &tables)
}

fn dump_with_imports(
    module: &Module,
    imported: &types::ImportedTypes,
    classification: Option<&[types::DeclItem]>,
    effects: Option<&access::EffectMap>,
) -> Result<String, SemaError> {
    let specialized = specialize::specialize(module)?;
    let decl_items = match classification {
        Some(items) => items.to_vec(),
        None => types::declare_with_imports(&specialized, imported)?,
    };
    let owned_effects: Option<access::EffectMap> = if effects.is_none() {
        Some(access::infer_effects(&specialized, &decl_items, imported))
    } else {
        None
    };
    let effects = effects.or(owned_effects.as_ref()).expect("effects present");
    let mut out = format!("Module path={}\n", specialized.path.join("."));
    types::render_items(&decl_items, effects, &mut out);
    Ok(out)
}

struct CheckDumpTables {
    decl_items_map: BTreeMap<Vec<String>, Vec<types::DeclItem>>,
    imported_types: BTreeMap<Vec<String>, types::ImportedTypes>,
}

fn render_check_dump(
    modules: &BTreeMap<Vec<String>, Module>,
    programs: &BTreeMap<Vec<String>, typed::TypedProgram>,
    tables: &CheckDumpTables,
) -> Result<String, SemaError> {
    let time_key: Vec<String> = crate::loader::TIME_MODULE_KEY
        .iter()
        .map(|s| (*s).to_string())
        .collect();
    let time_explicitly_imported = modules
        .values()
        .any(|m| m.imports.iter().any(|imp| imp.path == time_key));
    let runtime_key: Vec<String> = crate::loader::RUNTIME_MODULE_KEY
        .iter()
        .map(|s| (*s).to_string())
        .collect();
    let runtime_explicitly_imported = modules
        .values()
        .any(|m| m.imports.iter().any(|imp| imp.path == runtime_key));

    let mut out = String::new();
    for (key, module) in modules {
        if key == &time_key && !time_explicitly_imported {
            continue;
        }
        if key == &runtime_key && !runtime_explicitly_imported {
            continue;
        }
        if key.as_slice() == crate::loader::IMAGE_RUNTIME_MODULE_KEY {
            continue;
        }
        let effects = programs.get(key).map(|p| &p.effects);
        out.push_str(&dump_with_imports(
            module,
            &tables.imported_types[key],
            Some(&tables.decl_items_map[key]),
            effects,
        )?);
    }
    Ok(out)
}

fn load_time_module_as_sema() -> Result<(Vec<String>, crate::loader::LoadedModule), SemaError> {
    crate::loader::load_time_module().map_err(|e| match e {
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
    })
}

fn prepare_stdlib_enums_for_closure(
    modules: &BTreeMap<Vec<String>, Module>,
    paths: &BTreeMap<Vec<String>, String>,
) -> Result<(), SemaError> {
    for (key, module) in modules {
        if matches!(
            key.first().map(|s| s.as_str()),
            Some("core") | Some("drivers")
        ) {
            continue;
        }
        let Some(path) = paths.get(key) else {
            continue;
        };
        match crate::loader::anchor_package_root(Path::new(path), &module.path, module.span) {
            Ok(pkgroot) => return stdlib_enums::prepare(&pkgroot, module.span),
            Err(_) => continue,
        }
    }
    stdlib_enums::prepare_toolchain(Span::default())
}

fn prepare_stdlib_enums_for_file(path: &str, module: &Module) -> Result<(), SemaError> {
    match crate::loader::anchor_package_root(Path::new(path), &module.path, module.span) {
        Ok(pkgroot) => stdlib_enums::prepare(&pkgroot, module.span),
        Err(_) => stdlib_enums::prepare_toolchain(module.span),
    }
}

pub fn check_program(
    modules: &BTreeMap<Vec<String>, Module>,
    paths: &BTreeMap<Vec<String>, String>,
) -> Result<(), SemaError> {
    check_program_typed(modules, paths).map(|_| ())
}

pub fn check_program_typed(
    modules: &BTreeMap<Vec<String>, Module>,
    paths: &BTreeMap<Vec<String>, String>,
) -> Result<BTreeMap<Vec<String>, typed::TypedProgram>, SemaError> {
    check_program_typed_tables(modules, paths).map(|(programs, _)| programs)
}

fn check_program_typed_tables(
    modules: &BTreeMap<Vec<String>, Module>,
    paths: &BTreeMap<Vec<String>, String>,
) -> Result<(BTreeMap<Vec<String>, typed::TypedProgram>, CheckDumpTables), SemaError> {
    for (key, module) in modules {
        check_reserved_source_names(module, paths.get(key).map_or("<unknown>", String::as_str))?;
    }
    prepare_stdlib_enums_for_closure(modules, paths)?;
    let mut specialized: BTreeMap<Vec<String>, Module> = BTreeMap::new();
    let mut layouts: BTreeMap<Vec<String>, Vec<types::LayoutType>> = BTreeMap::new();
    for (key, module) in modules {
        let s = specialize::specialize(module)?;
        layouts.insert(key.clone(), types::check_layouts(&s)?);
        specialized.insert(key.clone(), s);
    }

    let mut symtabs: BTreeMap<Vec<String>, symbols::SymbolTable> = BTreeMap::new();
    let mut exports = imports::Exports::new();
    for (key, module) in &specialized {
        let table = symbols::collect(module)?;
        let public = imports::public_names(module);
        exports.insert(
            key.clone(),
            imports::ModuleExports {
                all: table.clone(),
                public,
            },
        );
        symtabs.insert(key.clone(), table);
    }

    let mut bindings: BTreeMap<Vec<String>, imports::ImportBindings> = BTreeMap::new();
    for (key, module) in &specialized {
        let b = imports::resolve_imports(module, &symtabs[key], &exports)?;
        bindings.insert(key.clone(), b);
    }

    inject_time_prelude_bindings(&mut bindings, &specialized);
    inject_pixels_prelude_bindings(&mut bindings, &specialized);
    let core_render = vec!["core".to_string(), "render".to_string()];
    let image_pixels = crate::loader::IMAGE_PIXELS_MODULE_KEY
        .iter()
        .map(|part| (*part).to_string())
        .collect::<Vec<_>>();
    let core_field = vec!["core".to_string(), "field".to_string()];
    let core_render_interval = vec!["core".to_string(), "render_interval".to_string()];
    let core_render_program = vec!["core".to_string(), "render_program".to_string()];
    let core_render_certificate = vec!["core".to_string(), "render_certificate".to_string()];
    let core_render_coverage = vec!["core".to_string(), "render_coverage".to_string()];
    let core_render_raster = vec!["core".to_string(), "render_raster".to_string()];
    let has_pixels_runtime = symtabs.contains_key(&image_pixels)
        && symtabs.contains_key(&core_render)
        && symtabs.contains_key(&core_field);
    if has_pixels_runtime {
        let scalar_helper_contexts = bindings.keys().cloned().collect::<Vec<_>>();
        // Imported bodies are evaluated in the caller's module context. Make
        // the generated coefficient evaluator's closed scalar helper surface
        // explicit throughout the P7 call closure so a nonlinear scene cannot
        // pass checking and then fail during sealed-runtime lowering.
        for key in scalar_helper_contexts {
            let Some(module_bindings) = bindings.get_mut(&key) else {
                continue;
            };
            for name in ["cos_scalar", "rsqrt_scalar", "sin_scalar", "sqrt_scalar"] {
                if !symtabs[&key].contains_key(name) && !module_bindings.contains_key(name) {
                    module_bindings.insert(
                        name.to_string(),
                        imports::ImportBinding {
                            target_module: core_field.clone(),
                            target_name: name.to_string(),
                        },
                    );
                }
            }
        }
        let pixels_helper_contexts = bindings.keys().cloned().collect::<Vec<_>>();
        for key in pixels_helper_contexts {
            let Some(module_bindings) = bindings.get_mut(&key) else {
                continue;
            };
            for (target_module, names) in [
                (
                    &core_render,
                    &[
                        "__wrela_pixels_p8_charge_raster_run",
                        "__wrela_pixels_p8_geometry_valid",
                        "__wrela_pixels_p8_i32_fits",
                        "__wrela_pixels_p8_i64_magnitude",
                        "__wrela_pixels_p8_raster_regular",
                        "__wrela_pixels_p8_recurrence_valid",
                        "__wrela_pixels_p8_raster_setup",
                    ][..],
                ),
                (
                    &core_render_interval,
                    &[
                        "FixedDomain",
                        "Iv32",
                        "NumericOutcome",
                        "interval_add",
                        "interval_ceil_div",
                        "interval_floor_div",
                        "interval_mul",
                        "interval_narrow",
                        "interval_sub",
                        "interval_valid",
                    ][..],
                ),
                (&core_render_program, &["polynomial_horner9"][..]),
                (
                    &core_render_certificate,
                    &["certify_monotone_root", "certify_quadratic_discriminant"][..],
                ),
                (
                    &core_render_coverage,
                    &["CoverageOutcome", "coverage_line_twice_area"][..],
                ),
                (
                    &core_render_raster,
                    &[
                        "AffineRunSetup",
                        "EventId",
                        "EventPixel",
                        "I32x4",
                        "I32x4Outcome",
                        "IdSlice",
                        "IdentitySetId",
                        "LightSummaryId",
                        "MaterialSummaryId",
                        "OutputProofCode",
                        "QRunScalar",
                        "RasterGeometryLane",
                        "RasterRun",
                        "RunId",
                        "i32x4_add_checked",
                        "pixels_i32x4_backend_add",
                        "raster_geometry_lane_valid",
                        "raster_i32_enclosure_fits",
                        "raster_run4",
                        "reconstruct_packet_world_normal",
                        "reconstruct_packet_world_position",
                    ][..],
                ),
            ] {
                if !symtabs.contains_key(target_module) {
                    continue;
                }
                for name in names {
                    if !symtabs[&key].contains_key(*name) && !module_bindings.contains_key(*name) {
                        module_bindings.insert(
                            (*name).to_string(),
                            imports::ImportBinding {
                                target_module: target_module.clone(),
                                target_name: (*name).to_string(),
                            },
                        );
                    }
                }
            }
        }
    }
    for (key, module_bindings) in &mut bindings {
        if key == &core_render || !has_pixels_runtime {
            continue;
        }
        // The injected Pixels surface comes from one table
        // (`pixels::surface`); see its module docs for why hand-maintained
        // copies of these lists were a standing source of drift.
        for name in crate::pixels::surface::injected_core_render_names() {
            if symtabs[key].contains_key(name) || module_bindings.contains_key(name) {
                continue;
            }
            module_bindings.insert(
                name.to_string(),
                imports::ImportBinding {
                    target_module: core_render.clone(),
                    target_name: name.to_string(),
                },
            );
        }
        for name in crate::pixels::surface::injected_image_pixels_names() {
            if symtabs[key].contains_key(name)
                || module_bindings.contains_key(name)
                || !symtabs.contains_key(&image_pixels)
            {
                continue;
            }
            module_bindings.insert(
                name.to_string(),
                imports::ImportBinding {
                    target_module: image_pixels.clone(),
                    target_name: name.to_string(),
                },
            );
        }
        for (module, names) in [
            (
                vec!["core".to_string(), "render_interval".to_string()],
                &[
                    "FixedDomain",
                    "Iv32",
                    "NumericOutcome",
                    "interval_add",
                    "interval_ceil_div",
                    "interval_floor_div",
                    "interval_mul",
                    "interval_narrow",
                    "interval_valid",
                ][..],
            ),
            (
                vec!["core".to_string(), "render_program".to_string()],
                &[
                    "Polynomial9",
                    "Polynomial9Outcome",
                    "bernstein_from_power9",
                    "bernstein_lerp_ratio",
                    "polynomial_compose9",
                    "polynomial_horner9",
                    "polynomial_multiply9",
                    "program_binomial",
                    "program_ceil_div",
                    "program_floor_div",
                    "program_gcd",
                ][..],
            ),
        ] {
            if !symtabs.contains_key(&module) {
                continue;
            }
            for name in names {
                if symtabs[key].contains_key(*name) || module_bindings.contains_key(*name) {
                    continue;
                }
                module_bindings.insert(
                    (*name).to_string(),
                    imports::ImportBinding {
                        target_module: module.clone(),
                        target_name: (*name).to_string(),
                    },
                );
            }
        }
    }
    let render_program = vec!["core".to_string(), "render_program".to_string()];
    let program_view_modules = bindings
        .iter()
        .filter(|(_, module_bindings)| {
            module_bindings.values().any(|binding| {
                binding.target_module == render_program
                    && matches!(
                        binding.target_name.as_str(),
                        "FrameProgramView" | "FeatureIdSlice"
                    )
            })
        })
        .map(|(key, _)| key.clone())
        .collect::<Vec<_>>();
    for key in program_view_modules {
        let module_bindings = bindings
            .get_mut(&key)
            .expect("program-view module key came from bindings");
        for name in [
            "__wrela_pixels_program_validate",
            "__wrela_pixels_program_header",
            "__wrela_pixels_program_digest_byte",
            "__wrela_pixels_program_record",
            "__wrela_pixels_program_operand",
            "__wrela_pixels_tile_feature",
            "__wrela_pixels_tile_feature_count",
        ] {
            if symtabs[&key].contains_key(name)
                || module_bindings.contains_key(name)
                || !symtabs.contains_key(&image_pixels)
            {
                continue;
            }
            module_bindings.insert(
                name.to_string(),
                imports::ImportBinding {
                    target_module: image_pixels.clone(),
                    target_name: name.to_string(),
                },
            );
        }
    }

    let closure_shapes = imports::closure_type_shapes(
        &specialized
            .iter()
            .map(|(k, m)| (k.clone(), m))
            .collect::<Vec<_>>(),
    );
    let mut imported_types: BTreeMap<Vec<String>, types::ImportedTypes> = BTreeMap::new();
    let mut imported_targets = types::ImportedTypeTargets::new();
    for (key, module) in &specialized {
        let mut imported = imports::imported_type_shapes(module, &closure_shapes);
        inject_time_prelude_types(&mut imported, &closure_shapes);
        imported_types.insert(key.clone(), imported);
        imported_targets.insert(
            key.clone(),
            imports::imported_type_targets(module, &closure_shapes),
        );
    }

    let mut decl_items_map: BTreeMap<Vec<String>, Vec<types::DeclItem>> = BTreeMap::new();
    for (key, module) in &specialized {
        symbols::resolve(module, &symtabs[key], &bindings[key])?;
        let decl_items = types::declare_with_imports(module, &imported_types[key])?;
        types::validate_placed_statics(&decl_items, &layouts[key])?;
        types::check_mmio_claims(module, &decl_items, &layouts[key])?;
        decl_items_map.insert(key.clone(), decl_items);
    }

    types::classify_closure(&mut decl_items_map, &imported_targets)?;

    let mut mctxs: BTreeMap<Vec<String>, bodies::ModuleCtx> = BTreeMap::new();
    for (key, module) in &specialized {
        let mut mctx = bodies::build_module_ctx(module, &decl_items_map[key], &imported_types[key]);
        mctx.loader_key = key.clone();
        mctxs.insert(key.clone(), mctx);
    }

    let splices: Vec<(Vec<String>, String, Vec<String>, String)> = bindings
        .iter()
        .flat_map(|(key, bs)| {
            bs.iter().map(move |(local, b)| {
                (
                    key.clone(),
                    local.clone(),
                    b.target_module.clone(),
                    b.target_name.clone(),
                )
            })
        })
        .collect();
    for (key, local, target_module, target_name) in splices {
        let (
            fn_entry,
            const_entry,
            const_val_entry,
            struct_entry,
            enum_entry,
            static_entry,
            layout_entries,
        ) = {
            let src = &mctxs[&target_module];
            (
                src.fns.get(&target_name).cloned(),
                src.consts.get(&target_name).cloned(),
                src.const_values.get(&target_name).cloned(),
                src.structs.get(&target_name).cloned(),
                src.enums.get(&target_name).cloned(),
                src.statics.get(&target_name).cloned(),
                if src.statics.contains_key(&target_name) {
                    src.layouts.values().cloned().collect::<Vec<_>>()
                } else {
                    Vec::new()
                },
            )
        };
        let dst = mctxs.get_mut(&key).expect("key is a key of mctxs");
        let subs = imports::alias_subs_for_exporter(&bindings[&key], &target_module);
        let origin = target_module.join(".");
        if let Some(mut f) = fn_entry {
            types::rekey_decl_fn_names(&mut f.decl, &subs);
            dst.fn_decl_module.insert(local.clone(), origin.clone());
            dst.fn_decl_name.insert(local.clone(), target_name.clone());
            dst.fns.insert(local.clone(), f);
        }
        if let Some(mut c) = const_entry {
            types::rekey_type_names(&mut c, &subs);
            dst.const_decl_module.insert(local.clone(), origin.clone());
            dst.const_decl_name
                .insert(local.clone(), target_name.clone());
            dst.consts.insert(local.clone(), c);
            if let Some(v) = const_val_entry {
                dst.const_values.insert(local.clone(), v);
            }
        }
        if let Some(mut s) = struct_entry {
            types::rekey_decl_struct_names(&mut s.decl, &subs);
            dst.struct_decl_module.insert(local.clone(), origin.clone());
            dst.type_decl_module.insert(local.clone(), origin.clone());
            dst.type_decl_name
                .insert(local.clone(), target_name.clone());
            dst.structs.insert(local.clone(), s);
        }
        if let Some(mut e) = enum_entry {
            types::rekey_decl_enum_names(&mut e.decl, &subs);
            dst.type_decl_module.insert(local.clone(), origin);
            dst.type_decl_name
                .insert(local.clone(), target_name.clone());
            dst.enums.insert(local.clone(), e);
        }
        if let Some(mut s) = static_entry {
            types::rekey_type_names(&mut s.ty, &subs);
            dst.statics.insert(local, s);
            for layout in layout_entries {
                dst.layouts
                    .entry(layout.name.clone())
                    .or_insert_with(|| layout);
            }
        }
    }

    close_mctx_type_reachability(&mut mctxs, &bindings);

    let mut programs: BTreeMap<Vec<String>, typed::TypedProgram> = BTreeMap::new();
    for (key, module) in &specialized {
        let mctx = &mctxs[key];
        let mut program = bodies::check(module, &decl_items_map[key], mctx)?;
        sync_inferred_error_sets(
            decl_items_map.get_mut(key).expect("decl_items for key"),
            &mctx.inferred_rets.borrow(),
        );
        let decl_items = &decl_items_map[key];
        program.layouts = layouts.get(key).cloned().unwrap_or_default();
        access::check(&mut program, mctx)?;
        flow::check(&program, mctx)?;
        handoff::check(module, decl_items, mctx)?;
        matches::check(&program, mctx)?;
        let empty_path = String::new();
        let path = paths.get(key).unwrap_or(&empty_path);
        program.instantiations = generics::check(module, decl_items, mctx, path)?;
        programs.insert(key.clone(), program);
    }

    splice_imported_decls(&mut programs, &bindings);

    for (key, module) in &specialized {
        let decl_items = &decl_items_map[key];
        let program = &programs[key];
        crate::eval::check_comptime(program)?;
        crate::eval::legal::check_provenance(
            program,
            &types::capability_authority(module, decl_items),
        )?;
        crate::eval::legal::check_isr_effects(program)?;
        crate::eval::legal::check_wake_sites(program)?;
        crate::eval::legal::check_bottom_half(program)?;
    }

    for (key, module) in &specialized {
        let mut layouts = match programs.get_mut(key) {
            Some(p) => std::mem::take(&mut p.layouts),
            None => continue,
        };
        types::complete_layouts(module, &programs[key], &mut layouts)?;
        if let Some(p) = programs.get_mut(key) {
            p.layouts = layouts;
        }
    }

    splice_imported_static_layouts(&mut programs, &bindings);

    let by_name: BTreeMap<String, &typed::TypedProgram> =
        programs.iter().map(|(k, p)| (k.join("."), p)).collect();
    send_proof::check(&by_name)?;
    reserve_proof::check(&by_name)?;

    for program in programs.values() {
        let observes = crate::eval::observes::classify(program);
        crate::eval::observes::check_loop_discharge(
            program,
            &observes,
            &program.unbounded_sync_loops,
        )?;
    }

    Ok((
        programs,
        CheckDumpTables {
            decl_items_map,
            imported_types,
        },
    ))
}

pub fn is_compiler_reserved_source_name(name: &str) -> bool {
    name.starts_with("__wrela_") || name.starts_with("RendererWorker")
}

/// Opt-in marker letting a repository-owned contract fixture name the
/// compiler-reserved Pixels surface in order to pin its contract.
pub const COMPILER_INTERNAL_FIXTURE_MARKER: &str = "@wrela-compiler-internal";

/// Is this source file a toolchain-owned stdlib module?
///
/// Trust follows the stdlib *layout*, not the path the compiler was built at.
/// `loader::stdlib_core_root` resolves `core.*` and `drivers.*` imports either
/// to a `stdlib/` sibling of the package root or to the toolchain copy, so a
/// compiler binary that has been moved, or a project vendoring its own
/// `stdlib/`, is a supported configuration — and pinning trust to
/// `CARGO_MANIFEST_DIR` made the stdlib itself fail this check there, with a
/// baffling "compiler-reserved namespace" error on toolchain code.
fn is_stdlib_source(path: &Path) -> bool {
    let components: Vec<_> = path
        .components()
        .map(|component| component.as_os_str().to_string_lossy().into_owned())
        .collect();
    components
        .windows(2)
        .any(|pair| pair[0] == "stdlib" && (pair[1] == "core" || pair[1] == "drivers"))
}

fn check_reserved_source_names(module: &Module, path: &str) -> Result<(), SemaError> {
    let source_path = Path::new(path);
    // Angle-bracket paths are compiler-owned synthetic/re-check inputs. User
    // files enter through the loader with a filesystem path and are checked
    // before any live-rtconfig re-check replaces that provenance with
    // `<module.path>`.
    let synthetic = path.starts_with('<') && path.ends_with('>');
    if synthetic || is_stdlib_source(source_path) {
        return Ok(());
    }

    let source = std::fs::read_to_string(source_path)
        .unwrap_or_else(|_| crate::syntax::printer::pretty(module));

    // A handful of repository-owned contract fixtures deliberately name
    // generated intrinsics in order to pin their contracts. That trust is
    // declared per file rather than inherited from a directory: blanket
    // trust would prevent fixtures from exercising the fence itself. Both
    // halves are required — a marker outside this repository grants nothing,
    // so a user package cannot opt itself in. The only trusted roots are the
    // golden corpus and stdlib's own contract-test corpus; shipped core and
    // driver modules are handled separately above.
    // Match the directive as a comment line, not as a loose substring: prose
    // that merely mentions the marker (a fixture explaining why it does *not*
    // opt in, say) must not thereby opt itself in.
    let marked = source.lines().any(|line| {
        let line = line.trim_start();
        line.starts_with('#') && line.contains(COMPILER_INTERNAL_FIXTURE_MARKER)
    });
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
    let repository_fixture = marked
        && source_path.canonicalize().is_ok_and(|source| {
            ["../../tests/golden", "../../stdlib/tests"]
                .into_iter()
                .filter_map(|relative| manifest.join(relative).canonicalize().ok())
                .any(|root| source.starts_with(root))
        });
    if repository_fixture {
        return Ok(());
    }

    // Inspect tokens rather than just declarations: a user reference to a
    // magic lowering name is as dangerous as defining it. The original file
    // preserves useful source locations; in-memory callers fall back to the
    // stable pretty-printer and still receive a deterministic diagnostic.
    let tokens = crate::syntax::lexer::lex(&source).map_err(|error| SemaError {
        category: "lex",
        message: error.message,
        line: error.line,
        col: error.col,
        extra_lines: Vec::new(),
        omit_location: false,
        missing_method: None,
    })?;
    if let Some(token) = tokens.into_iter().find(|token| {
        token.kind == crate::syntax::lexer::TokenKind::Ident
            && is_compiler_reserved_source_name(&token.text)
    }) {
        return Err(SemaError::at(
            "name",
            format!(
                "`{}` is in a compiler-reserved namespace and cannot be defined or referenced by user modules",
                token.text
            ),
            Span {
                line: token.line,
                col: token.col,
                byte_start: token.byte_start,
                byte_end: token.byte_end,
            },
        ));
    }
    Ok(())
}

fn splice_imported_decls(
    programs: &mut BTreeMap<Vec<String>, typed::TypedProgram>,
    bindings: &BTreeMap<Vec<String>, imports::ImportBindings>,
) {
    let mut declared: BTreeMap<Vec<String>, BTreeSet<String>> = BTreeMap::new();
    for (key, p) in programs.iter() {
        let mut names: BTreeSet<String> = BTreeSet::new();
        names.extend(p.consts.keys().cloned());
        names.extend(p.fns.keys().cloned());
        names.extend(p.structs.keys().cloned());
        names.extend(p.enums.keys().cloned());
        declared.insert(key.clone(), names);
    }
    let empty_bindings = imports::ImportBindings::new();
    let resolve = |m: &Vec<String>, name: &str| -> Option<(Vec<String>, String)> {
        if declared.get(m).is_some_and(|d| d.contains(name)) {
            return Some((m.clone(), name.to_string()));
        }
        bindings
            .get(m)
            .unwrap_or(&empty_bindings)
            .get(name)
            .map(|b| (b.target_module.clone(), b.target_name.clone()))
    };

    let shadow: BTreeMap<(Vec<String>, Vec<String>), String> = {
        let same_decl = |a: &(Vec<String>, String), b: &(Vec<String>, String)| -> bool {
            if a == b {
                return true;
            }
            let canonical_pixels_wire_alias =
                |left: &(Vec<String>, String), right: &(Vec<String>, String)| {
                    let render_program = ["core", "render_program"];
                    let image_pixels = ["core", "__image_pixels"];
                    let module_is = |module: &[String], expected: &[&str]| {
                        module
                            .iter()
                            .map(String::as_str)
                            .eq(expected.iter().copied())
                    };
                    ((module_is(&left.0, &render_program) && module_is(&right.0, &image_pixels))
                        || (module_is(&left.0, &image_pixels)
                            && module_is(&right.0, &render_program)))
                        && left.1 == right.1
                        && matches!(
                            left.1.as_str(),
                            "FrameProgramHeaderV1"
                                | "FrameProgramTableV1"
                                | "FrameProgramRecordV1"
                                | "FrameProgramImmediateV1"
                        )
                };
            if canonical_pixels_wire_alias(a, b) {
                // core.__image_pixels copies these four declarations
                // mechanically from core.render_program before semantic
                // checking. Treating the copy as the same declaration is
                // narrower than the general shadow exception and preserves
                // the imported-body fail-closed rule for every other name.
                return true;
            }
            let (Some(pa), Some(pb)) = (programs.get(&a.0), programs.get(&b.0)) else {
                return false;
            };
            pa.consts.get(&a.1) == pb.consts.get(&b.1)
                && pa.fns.get(&a.1) == pb.fns.get(&b.1)
                && pa.structs.get(&a.1) == pb.structs.get(&b.1)
                && pa.enums.get(&a.1) == pb.enums.get(&b.1)
        };
        let mut shadow: BTreeMap<(Vec<String>, Vec<String>), String> = BTreeMap::new();
        let mut examined: BTreeSet<(Vec<String>, Vec<String>)> = BTreeSet::new();
        for (m, bs) in bindings {
            for b in bs.values() {
                let n = &b.target_module;
                if n == m || !examined.insert((m.clone(), n.clone())) {
                    continue;
                }
                let mut visible: BTreeSet<String> = declared.get(n).cloned().unwrap_or_default();
                visible.extend(bindings.get(n).unwrap_or(&empty_bindings).keys().cloned());
                for name in &visible {
                    let (Some(from_n), Some(from_m)) = (resolve(n, name), resolve(m, name)) else {
                        continue;
                    };
                    if !same_decl(&from_n, &from_m) {
                        shadow.insert((m.clone(), n.clone()), name.clone());
                        break;
                    }
                }
            }
        }
        shadow
    };

    let mut unresolvable: BTreeMap<Vec<String>, BTreeMap<String, String>> = BTreeMap::new();
    let module_names: Vec<Vec<String>> = programs.keys().cloned().collect();
    for m in &module_names {
        let mut out: BTreeMap<String, String> = BTreeMap::new();
        for (owner, names) in &declared {
            if owner == m {
                continue;
            }
            for name in names {
                if resolve(m, name).is_some() || out.contains_key(name) {
                    continue;
                }
                out.insert(
                    name.clone(),
                    format!(
                        "is declared in module `{}`, which module `{}` does not import; \
                         evaluating an imported body that reaches a declaration present \
                         only in that body's private helpers (not in any imported \
                         signature) is not supported yet",
                        owner.join("."),
                        m.join(".")
                    ),
                );
            }
        }
        unresolvable.insert(m.clone(), out);
    }

    let splices: Vec<(Vec<String>, String, Vec<String>, String)> = bindings
        .iter()
        .flat_map(|(key, bs)| {
            bs.iter().map(move |(local, b)| {
                (
                    key.clone(),
                    local.clone(),
                    b.target_module.clone(),
                    b.target_name.clone(),
                )
            })
        })
        .collect();
    for (key, local, target_module, target_name) in splices {
        let Some(src) = programs.get(&target_module) else {
            continue;
        };
        let withheld = shadow.get(&(key.clone(), target_module.clone())).cloned();
        let const_entry = src.consts.get(&target_name).cloned();
        let fn_entry = src.fns.get(&target_name).cloned();
        let struct_entry = src.structs.get(&target_name).cloned();
        let enum_entry = src.enums.get(&target_name).cloned();
        let static_entry = src.statics.get(&target_name).cloned();
        let companion_statics: Vec<(String, _)> = if fn_entry.is_some() {
            src.statics
                .iter()
                .map(|(n, s)| (n.clone(), s.clone()))
                .collect()
        } else {
            Vec::new()
        };
        let companion_consts: Vec<(String, _)> = if fn_entry.is_some() {
            src.consts
                .iter()
                .map(|(n, c)| (n.clone(), c.clone()))
                .collect()
        } else {
            Vec::new()
        };
        let inst_entries = src.instantiations.clone();
        let body_bearing = const_entry.is_some()
            || fn_entry.is_some()
            || struct_entry.is_some()
            || enum_entry.is_some()
            || static_entry.is_some();
        let dst = programs.get_mut(&key).expect("key is a key of programs");
        if let (Some(witness), true) = (&withheld, body_bearing) {
            dst.imported.unresolvable.insert(
                local.clone(),
                format!(
                    "is imported from module `{}`, which declares `{witness}` — a name module \
                     `{}` resolves differently; evaluating that module's bodies here could \
                     silently pick the wrong `{witness}`, so it is not supported yet \
                     (plans/M9.md item A1b)",
                    target_module.join("."),
                    key.join(".")
                ),
            );
        } else {
            let subs = imports::alias_subs_for_exporter(
                bindings.get(&key).expect("key is a key of bindings"),
                &target_module,
            );
            if let Some(mut c) = const_entry {
                typed::rekey_const_names(&mut c, &subs);
                dst.imported.consts.insert(local.clone(), c);
            }
            if let Some(mut f) = fn_entry {
                typed::rekey_fn_names(&mut f, &subs);
                dst.imported.fns.insert(local.clone(), f);
            }
            if let Some(mut s) = struct_entry {
                typed::rekey_struct_names(&mut s, &subs);
                dst.imported.structs.insert(local.clone(), s);
            }
            if let Some(mut e) = enum_entry {
                typed::rekey_enum_names(&mut e, &subs);
                dst.imported.enums.insert(local.clone(), e);
            }
            if let Some(s) = static_entry {
                dst.statics.insert(local.clone(), s);
            }
            for (name, s) in companion_statics {
                dst.statics.entry(name).or_insert(s);
            }
            for (name, c) in companion_consts {
                dst.consts.entry(name).or_insert(c);
            }
            for (ikey, mut inst) in inst_entries {
                typed::rekey_instantiation(&mut inst, &subs);
                let new_key = typed::rekey_canonical_key(&ikey, &subs);
                dst.imported.instantiations.entry(new_key).or_insert(inst);
            }
        }
    }

    close_typed_type_reachability(programs, bindings);

    for (key, notes) in unresolvable {
        let dst = programs.get_mut(&key).expect("key is a key of programs");
        for (name, note) in notes {
            if dst.imported.structs.contains_key(&name)
                || dst.imported.enums.contains_key(&name)
                || dst.imported.fns.contains_key(&name)
                || dst.imported.consts.contains_key(&name)
                || dst.structs.contains_key(&name)
                || dst.enums.contains_key(&name)
                || dst.fns.contains_key(&name)
                || dst.consts.contains_key(&name)
            {
                continue;
            }
            dst.imported.unresolvable.entry(name).or_insert(note);
        }
    }
}

fn close_mctx_type_reachability(
    mctxs: &mut BTreeMap<Vec<String>, bodies::ModuleCtx>,
    bindings: &BTreeMap<Vec<String>, imports::ImportBindings>,
) {
    let empty = imports::ImportBindings::new();
    let module_keys: Vec<Vec<String>> = mctxs.keys().cloned().collect();
    for importer in &module_keys {
        let own_bindings = bindings.get(importer).unwrap_or(&empty);
        let mut origins: BTreeMap<String, Vec<String>> = BTreeMap::new();
        let mut queue: Vec<String> = Vec::new();
        for (local, b) in own_bindings {
            origins.insert(local.clone(), b.target_module.clone());
            queue.push(local.clone());
        }
        let mut visited: BTreeSet<String> = queue.iter().cloned().collect();
        while let Some(name) = queue.pop() {
            let Some(origin) = origins.get(&name).cloned() else {
                continue;
            };
            let mut mentioned = BTreeSet::new();
            {
                let dst = &mctxs[importer];
                if let Some(s) = dst.structs.get(&name) {
                    types::collect_named_types_from_decl_struct(&s.decl, &mut mentioned);
                } else if let Some(e) = dst.enums.get(&name) {
                    types::collect_named_types_from_decl_enum(&e.decl, &mut mentioned);
                } else if let Some(f) = dst.fns.get(&name) {
                    types::collect_named_types_from_decl_fn(&f.decl, &mut mentioned);
                } else if let Some(ty) = dst.consts.get(&name) {
                    types::collect_named_type_names(ty, &mut mentioned);
                } else if let Some(info) = dst.statics.get(&name) {
                    types::collect_named_type_names(&info.ty, &mut mentioned);
                }
            }
            for tname in mentioned {
                if mctxs[importer].structs.contains_key(&tname)
                    || mctxs[importer].enums.contains_key(&tname)
                {
                    continue;
                }
                if !visited.insert(tname.clone()) {
                    continue;
                }
                let origin_bindings = bindings.get(&origin).unwrap_or(&empty);
                let lookup = imports::lookup_origin_type_name(&tname, &origin, own_bindings);
                let def_module = origin_bindings
                    .get(&lookup)
                    .map(|b| b.target_module.clone())
                    .unwrap_or_else(|| origin.clone());
                let def_name = origin_bindings
                    .get(&lookup)
                    .map(|b| b.target_name.clone())
                    .unwrap_or_else(|| lookup.clone());
                let (struct_entry, enum_entry) = {
                    let src = &mctxs[&def_module];
                    (
                        src.structs.get(&def_name).cloned(),
                        src.enums.get(&def_name).cloned(),
                    )
                };
                let subs = imports::alias_subs_for_exporter(own_bindings, &def_module);
                let dst = mctxs.get_mut(importer).expect("importer is a key");
                if let Some(mut s) = struct_entry {
                    types::rekey_decl_struct_names(&mut s.decl, &subs);
                    if s.decl.name != tname {
                        let mut name_sub = BTreeMap::new();
                        name_sub.insert(s.decl.name.clone(), tname.clone());
                        types::rekey_decl_struct_names(&mut s.decl, &name_sub);
                    }
                    dst.shapes.insert(tname.clone(), s.decl.generics.len());
                    dst.struct_decl_module
                        .insert(tname.clone(), def_module.join("."));
                    dst.type_decl_module
                        .insert(tname.clone(), def_module.join("."));
                    dst.type_decl_name.insert(tname.clone(), def_name.clone());
                    dst.structs.insert(tname.clone(), s);
                    origins.insert(tname.clone(), def_module);
                    queue.push(tname);
                } else if let Some(mut e) = enum_entry {
                    types::rekey_decl_enum_names(&mut e.decl, &subs);
                    if e.decl.name != tname {
                        let mut name_sub = BTreeMap::new();
                        name_sub.insert(e.decl.name.clone(), tname.clone());
                        types::rekey_decl_enum_names(&mut e.decl, &name_sub);
                    }
                    dst.shapes.insert(tname.clone(), e.decl.generics.len());
                    dst.type_decl_module
                        .insert(tname.clone(), def_module.join("."));
                    dst.type_decl_name.insert(tname.clone(), def_name.clone());
                    dst.enums.insert(tname.clone(), e);
                    origins.insert(tname.clone(), def_module);
                    queue.push(tname);
                }
            }
        }
    }
}

fn close_typed_type_reachability(
    programs: &mut BTreeMap<Vec<String>, typed::TypedProgram>,
    bindings: &BTreeMap<Vec<String>, imports::ImportBindings>,
) {
    let empty = imports::ImportBindings::new();
    let module_keys: Vec<Vec<String>> = programs.keys().cloned().collect();
    for importer in &module_keys {
        let own_bindings = bindings.get(importer).unwrap_or(&empty);
        let mut origins: BTreeMap<String, Vec<String>> = BTreeMap::new();
        let mut queue: Vec<String> = Vec::new();
        for (local, b) in own_bindings {
            origins.insert(local.clone(), b.target_module.clone());
            queue.push(local.clone());
        }
        let mut visited: BTreeSet<String> = queue.iter().cloned().collect();
        while let Some(name) = queue.pop() {
            let Some(origin) = origins.get(&name).cloned() else {
                continue;
            };
            let mut mentioned = BTreeSet::new();
            {
                let dst = &programs[importer];
                if let Some(s) = dst
                    .imported
                    .structs
                    .get(&name)
                    .or_else(|| dst.structs.get(&name))
                {
                    typed::collect_named_types_from_struct(s, &mut mentioned);
                } else if let Some(e) = dst
                    .imported
                    .enums
                    .get(&name)
                    .or_else(|| dst.enums.get(&name))
                {
                    typed::collect_named_types_from_enum(e, &mut mentioned);
                } else if let Some(f) = dst.imported.fns.get(&name).or_else(|| dst.fns.get(&name)) {
                    typed::collect_named_types_from_fn(f, &mut mentioned);
                } else if let Some(c) = dst
                    .imported
                    .consts
                    .get(&name)
                    .or_else(|| dst.consts.get(&name))
                {
                    types::collect_named_type_names(&c.ty, &mut mentioned);
                } else if let Some(s) = dst.statics.get(&name) {
                    types::collect_named_type_names(&s.ty, &mut mentioned);
                }
            }
            for tname in mentioned {
                let dst_has = {
                    let dst = &programs[importer];
                    dst.structs.contains_key(&tname)
                        || dst.enums.contains_key(&tname)
                        || dst.imported.structs.contains_key(&tname)
                        || dst.imported.enums.contains_key(&tname)
                };
                if dst_has {
                    continue;
                }
                if !visited.insert(tname.clone()) {
                    continue;
                }
                let origin_bindings = bindings.get(&origin).unwrap_or(&empty);
                let lookup = imports::lookup_origin_type_name(&tname, &origin, own_bindings);
                let def_module = origin_bindings
                    .get(&lookup)
                    .map(|b| b.target_module.clone())
                    .unwrap_or_else(|| origin.clone());
                let def_name = origin_bindings
                    .get(&lookup)
                    .map(|b| b.target_name.clone())
                    .unwrap_or_else(|| lookup.clone());
                let (struct_entry, enum_entry) = {
                    let src = &programs[&def_module];
                    (
                        src.structs
                            .get(&def_name)
                            .or_else(|| src.imported.structs.get(&def_name))
                            .cloned(),
                        src.enums
                            .get(&def_name)
                            .or_else(|| src.imported.enums.get(&def_name))
                            .cloned(),
                    )
                };
                let subs = imports::alias_subs_for_exporter(own_bindings, &def_module);
                let dst = programs.get_mut(importer).expect("importer is a key");
                if let Some(mut s) = struct_entry {
                    typed::rekey_struct_names(&mut s, &subs);
                    if s.name != tname {
                        let mut name_sub = BTreeMap::new();
                        name_sub.insert(s.name.clone(), tname.clone());
                        typed::rekey_struct_names(&mut s, &name_sub);
                    }
                    dst.imported.structs.insert(tname.clone(), s);
                    origins.insert(tname.clone(), def_module);
                    queue.push(tname);
                } else if let Some(mut e) = enum_entry {
                    typed::rekey_enum_names(&mut e, &subs);
                    dst.imported.enums.insert(tname.clone(), e);
                    origins.insert(tname.clone(), def_module);
                    queue.push(tname);
                }
            }
        }
        let mut from_inst = BTreeSet::new();
        {
            let dst = &programs[importer];
            for inst in dst
                .instantiations
                .values()
                .chain(dst.imported.instantiations.values())
            {
                match inst {
                    typed::TypedInstantiation::Struct(s) => {
                        typed::collect_named_types_from_struct(s, &mut from_inst);
                    }
                    typed::TypedInstantiation::Fn(f) => {
                        typed::collect_named_types_from_fn(f, &mut from_inst);
                    }
                    typed::TypedInstantiation::Enum(_) => {}
                }
            }
        }
        for tname in from_inst {
            let dst_has = {
                let dst = &programs[importer];
                dst.structs.contains_key(&tname)
                    || dst.enums.contains_key(&tname)
                    || dst.imported.structs.contains_key(&tname)
                    || dst.imported.enums.contains_key(&tname)
            };
            if dst_has {
                continue;
            }
            let mut struct_entry = None;
            let mut enum_entry = None;
            let mut def_module = None;
            for (mod_key, src) in programs.iter() {
                if mod_key == importer {
                    continue;
                }
                if let Some(s) = src
                    .structs
                    .get(&tname)
                    .or_else(|| src.imported.structs.get(&tname))
                {
                    struct_entry = Some(s.clone());
                    def_module = Some(mod_key.clone());
                    break;
                }
                if let Some(e) = src
                    .enums
                    .get(&tname)
                    .or_else(|| src.imported.enums.get(&tname))
                {
                    enum_entry = Some(e.clone());
                    def_module = Some(mod_key.clone());
                    break;
                }
            }
            let Some(def_module) = def_module else {
                continue;
            };
            let subs = imports::alias_subs_for_exporter(own_bindings, &def_module);
            let dst = programs.get_mut(importer).expect("importer is a key");
            if let Some(mut s) = struct_entry {
                typed::rekey_struct_names(&mut s, &subs);
                if s.name != tname {
                    let mut name_sub = BTreeMap::new();
                    name_sub.insert(s.name.clone(), tname.clone());
                    typed::rekey_struct_names(&mut s, &name_sub);
                }
                dst.imported.structs.insert(tname, s);
            } else if let Some(mut e) = enum_entry {
                typed::rekey_enum_names(&mut e, &subs);
                dst.imported.enums.insert(tname, e);
            }
        }
    }
}

fn splice_imported_static_layouts(
    programs: &mut BTreeMap<Vec<String>, typed::TypedProgram>,
    bindings: &BTreeMap<Vec<String>, imports::ImportBindings>,
) {
    let splices: Vec<(Vec<String>, Vec<String>, String)> = bindings
        .iter()
        .flat_map(|(key, bs)| {
            bs.iter().map(move |(_local, b)| {
                (key.clone(), b.target_module.clone(), b.target_name.clone())
            })
        })
        .collect();
    for (importer, exporter, target_name) in splices {
        let Some(src) = programs.get(&exporter) else {
            continue;
        };
        let layouts: Vec<_> =
            if src.statics.contains_key(&target_name) || src.fns.contains_key(&target_name) {
                src.layouts.clone()
            } else {
                src.layouts
                    .iter()
                    .filter(|l| l.name == target_name)
                    .cloned()
                    .collect()
            };
        if layouts.is_empty() {
            continue;
        }
        let dst = programs.get_mut(&importer).expect("importer key");
        let mut have: BTreeSet<String> = dst.layouts.iter().map(|l| l.name.clone()).collect();
        for layout in layouts {
            if have.insert(layout.name.clone()) {
                dst.layouts.push(layout);
            }
        }
    }
}

fn inject_time_prelude_bindings(
    bindings: &mut BTreeMap<Vec<String>, imports::ImportBindings>,
    specialized: &BTreeMap<Vec<String>, Module>,
) {
    let time_key: Vec<String> = crate::loader::TIME_MODULE_KEY
        .iter()
        .map(|s| (*s).to_string())
        .collect();
    if !specialized.contains_key(&time_key) {
        return;
    }
    for key in specialized.keys() {
        if key == &time_key {
            continue;
        }
        let bs = bindings.entry(key.clone()).or_default();
        for name in crate::loader::TIME_PRELUDE_NAMES {
            bs.entry((*name).to_string())
                .or_insert_with(|| imports::ImportBinding {
                    target_module: time_key.clone(),
                    target_name: (*name).to_string(),
                });
        }
    }
}

fn inject_pixels_prelude_bindings(
    bindings: &mut BTreeMap<Vec<String>, imports::ImportBindings>,
    specialized: &BTreeMap<Vec<String>, Module>,
) {
    let pixels_key = vec!["core".to_string(), "__image_pixels".to_string()];
    let Some(pixels_module) = specialized.get(&pixels_key) else {
        return;
    };
    let accessors = pixels_module
        .items
        .iter()
        .filter_map(|item| match item {
            crate::syntax::ast::Item::Fn(function)
                if function.is_pub && function.name.starts_with("__wrela_pixels_") =>
            {
                Some(function.name.clone())
            }
            _ => None,
        })
        .collect::<Vec<_>>();
    for key in specialized.keys() {
        if key == &pixels_key {
            continue;
        }
        let module_bindings = bindings.entry(key.clone()).or_default();
        for name in &accessors {
            module_bindings
                .entry(name.clone())
                .or_insert_with(|| imports::ImportBinding {
                    target_module: pixels_key.clone(),
                    target_name: name.clone(),
                });
        }
    }
}

fn inject_time_prelude_types(
    imported: &mut types::ImportedTypes,
    closure_shapes: &BTreeMap<Vec<String>, BTreeMap<String, usize>>,
) {
    let time_key: Vec<String> = crate::loader::TIME_MODULE_KEY
        .iter()
        .map(|s| (*s).to_string())
        .collect();
    let Some(shapes) = closure_shapes.get(&time_key) else {
        return;
    };
    for name in ["Duration", "Instant"] {
        if let Some(arity) = shapes.get(name) {
            imported.entry(name.to_string()).or_insert(*arity);
        }
    }
}

pub fn dump_program(modules: &BTreeMap<Vec<String>, Module>) -> Result<String, SemaError> {
    let mut specialized: BTreeMap<Vec<String>, Module> = BTreeMap::new();
    for (k, m) in modules {
        specialized.insert(k.clone(), specialize::specialize(m)?);
    }
    let closure_shapes = imports::closure_type_shapes(
        &specialized
            .iter()
            .map(|(k, m)| (k.clone(), m))
            .collect::<Vec<_>>(),
    );
    let mut imported_targets = types::ImportedTypeTargets::new();
    let mut decl_items_map: BTreeMap<Vec<String>, Vec<types::DeclItem>> = BTreeMap::new();
    let mut imported_types: BTreeMap<Vec<String>, types::ImportedTypes> = BTreeMap::new();
    for (key, module) in &specialized {
        let mut imported = imports::imported_type_shapes(module, &closure_shapes);
        inject_time_prelude_types(&mut imported, &closure_shapes);
        decl_items_map.insert(key.clone(), types::declare_with_imports(module, &imported)?);
        imported_targets.insert(
            key.clone(),
            imports::imported_type_targets(module, &closure_shapes),
        );
        imported_types.insert(key.clone(), imported);
    }
    types::classify_closure(&mut decl_items_map, &imported_targets)?;

    render_check_dump(
        modules,
        &BTreeMap::new(),
        &CheckDumpTables {
            decl_items_map,
            imported_types,
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::syntax::{lexer, parser};

    #[test]
    fn check_typed_rejects_imports() {
        let src = "module m\n\nfrom other import X\n\npub fn f() -> u64:\n    return 1\n";
        let tokens = lexer::lex(src).expect("lex");
        let module = parser::parse(tokens).expect("parse");
        let err = check_typed(&module, "m.wr").err().expect("must reject");
        assert_eq!(err.category, "unimplemented");
        assert!(
            err.message
                .contains("imports through the single-module entry")
        );
    }

    #[test]
    fn user_modules_cannot_enter_compiler_reserved_namespaces() {
        for source in [
            "module m\n\nfn __wrela_pixels_f32_to_bits(value: f32) -> u32:\n    return 0\n",
            "module m\n\nfn f() -> u32:\n    return __wrela_pixels_f32_to_bits(0.0)\n",
            "module m\n\npub struct RendererWorkerPool:\n    value: u32\n",
        ] {
            let module = parser::parse(lexer::lex(source).expect("lex")).expect("parse");
            let error = check_reserved_source_names(&module, "/nonexistent/user-module.wr")
                .expect_err("reserved name must fail before lowering");
            assert_eq!(error.category, "name");
            assert!(error.message.contains("compiler-reserved namespace"));
        }
    }
}
