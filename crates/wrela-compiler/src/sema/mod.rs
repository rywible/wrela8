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

/// Check one source module once and render the stable `check` observation
/// from the same declarations and effect table. This is the single-module
/// counterpart to [`check_program_typed_with_dump`].
pub fn check_typed_with_dump(
    module: &Module,
    path: &str,
) -> Result<(typed::TypedProgram, String), SemaError> {
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
        paths.insert(root_key.clone(), path.to_string());
        paths.insert(time_key, time_path);
        let (mut programs, dump) = check_program_typed_with_dump(&modules, &paths)?;
        let program = programs.remove(&root_key).ok_or_else(|| {
            SemaError::at(
                "internal",
                "internal error: time-prelude check lost the root module".to_string(),
                Span::default(),
            )
        })?;
        return Ok((program, dump));
    }
    let (program, decl_items) = check_typed_single_with_decls(module, path)?;
    let dump = dump_with_imports(
        module,
        &types::ImportedTypes::new(),
        Some(&decl_items),
        Some(&program.effects),
    )?;
    Ok((program, dump))
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
    check_reserved_source_names(module, path, false)?;
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
    let (programs, tables) = check_program_typed_tables(modules, paths, &BTreeSet::new())?;
    render_check_dump(modules, &programs, &tables)
}

pub fn check_program_dump_with_internal_sources(
    modules: &BTreeMap<Vec<String>, Module>,
    paths: &BTreeMap<Vec<String>, String>,
    internal_sources: &BTreeSet<Vec<String>>,
) -> Result<String, SemaError> {
    let (programs, tables) = check_program_typed_tables(modules, paths, internal_sources)?;
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
    check_program_typed_tables(modules, paths, &BTreeSet::new()).map(|(programs, _)| programs)
}

/// Check an import closure with loader-authenticated compiler-owned modules.
///
/// The ordinary public entry points deliberately pass an empty set. Callers
/// use this only for generated modules whose ASTs were created inside the
/// compiler; filesystem spelling and angle-bracket display paths never grant
/// the capability.
pub fn check_program_typed_with_internal_sources(
    modules: &BTreeMap<Vec<String>, Module>,
    paths: &BTreeMap<Vec<String>, String>,
    internal_sources: &BTreeSet<Vec<String>>,
) -> Result<BTreeMap<Vec<String>, typed::TypedProgram>, SemaError> {
    check_program_typed_tables(modules, paths, internal_sources).map(|(programs, _)| programs)
}

/// Check an import closure once and render the stable `check` observation
/// from the exact same semantic tables.
pub fn check_program_typed_with_dump(
    modules: &BTreeMap<Vec<String>, Module>,
    paths: &BTreeMap<Vec<String>, String>,
) -> Result<(BTreeMap<Vec<String>, typed::TypedProgram>, String), SemaError> {
    let (programs, tables) = check_program_typed_tables(modules, paths, &BTreeSet::new())?;
    let dump = render_check_dump(modules, &programs, &tables)?;
    Ok((programs, dump))
}

pub fn check_program_typed_with_dump_and_internal_sources(
    modules: &BTreeMap<Vec<String>, Module>,
    paths: &BTreeMap<Vec<String>, String>,
    internal_sources: &BTreeSet<Vec<String>>,
) -> Result<(BTreeMap<Vec<String>, typed::TypedProgram>, String), SemaError> {
    let (programs, tables) = check_program_typed_tables(modules, paths, internal_sources)?;
    let dump = render_check_dump(modules, &programs, &tables)?;
    Ok((programs, dump))
}

fn check_program_typed_tables(
    modules: &BTreeMap<Vec<String>, Module>,
    paths: &BTreeMap<Vec<String>, String>,
    internal_sources: &BTreeSet<Vec<String>>,
) -> Result<(BTreeMap<Vec<String>, typed::TypedProgram>, CheckDumpTables), SemaError> {
    let timings = std::env::var_os("WRELA_COMPILER_TIMINGS").is_some();
    let mut last = std::time::Instant::now();
    let mut timing = |stage: &str| {
        if timings {
            eprintln!(
                "compiler-timing: {:.3}s sema-{stage}",
                last.elapsed().as_secs_f64()
            );
        }
        last = std::time::Instant::now();
    };
    for (key, module) in modules {
        check_reserved_source_names(
            module,
            paths.get(key).map_or("<unknown>", String::as_str),
            internal_sources.contains(key),
        )?;
    }
    prepare_stdlib_enums_for_closure(modules, paths)?;
    let mut specialized: BTreeMap<Vec<String>, std::borrow::Cow<'_, Module>> = BTreeMap::new();
    let mut layouts: BTreeMap<Vec<String>, Vec<types::LayoutType>> = BTreeMap::new();
    for (key, module) in modules {
        let s = specialize::specialize_cow(module)?;
        layouts.insert(key.clone(), types::check_layouts(&s)?);
        specialized.insert(key.clone(), s);
    }
    timing("specialize-layouts");

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
    let alias_subs: BTreeMap<Vec<String>, BTreeMap<Vec<String>, BTreeMap<String, String>>> =
        bindings
            .iter()
            .map(|(module, module_bindings)| {
                (
                    module.clone(),
                    imports::alias_subs_by_exporter(module_bindings),
                )
            })
            .collect();
    timing("symbols-imports");
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
    let core_render_arrangement = vec!["core".to_string(), "render_arrangement".to_string()];
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
            let imports_packet_selftest = module_bindings
                .get("__wrela_pixels_p8r_packet_selftest")
                .is_some_and(|binding| {
                    binding.target_module == image_pixels
                        && binding.target_name == "__wrela_pixels_p8r_packet_selftest"
                });
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
                    &core_render_arrangement,
                    &[
                        "RendererWorkerP8RSubdivisionCell",
                        "RendererWorkerP8RSubdivisionStack",
                    ][..],
                ),
                (
                    &core_render_raster,
                    &[
                        "AffineRunSetup",
                        "EventId",
                        "EventPixel",
                        "F32x4",
                        "I32x4",
                        "I32x4Outcome",
                        "__pixels_f32_exponent",
                        "__pixels_f32_mantissa",
                        "__pixels_u128_add",
                        "__pixels_u128_bit",
                        "__pixels_u128_compare",
                        "__pixels_u128_from_u64",
                        "__pixels_u128_is_zero",
                        "__pixels_u128_lower_bits_nonzero",
                        "__pixels_u128_round_shift_even",
                        "__pixels_u128_scale_to",
                        "__pixels_u128_shift_left",
                        "__pixels_u128_shift_right",
                        "__pixels_u128_shift_right_jam",
                        "__pixels_u128_sub",
                        "__pixels_u128_top_bit",
                        "__pixels_u128_zero",
                        "IdSlice",
                        "IdentitySetId",
                        "LightSummaryId",
                        "MaterialSummaryId",
                        "OutputProofCode",
                        "QRunScalar",
                        "RasterGeometryLane",
                        "RasterRun",
                        "RunId",
                        "PIXELS_REGION_COVERAGE_CELL_WALK",
                        "PIXELS_REGION_COVERAGE_ENTRY",
                        "PIXELS_REGION_RASTER_CHARGE",
                        "PIXELS_REGION_RASTER_PACKET_LOOP",
                        "PIXELS_REGION_RASTER_SCALAR_PREFIX",
                        "PIXELS_REGION_RASTER_SCALAR_SUFFIX",
                        "i32x4_add_checked",
                        "pixels_f32_fma_bits_fallback",
                        "pixels_f32_fma_scalar",
                        "pixels_f32_from_bits",
                        "pixels_f32_max_scalar",
                        "pixels_f32_min_scalar",
                        "pixels_f32_select_ge_scalar",
                        "pixels_f32_select_gt_scalar",
                        "pixels_f32_to_bits",
                        "pixels_f32_to_i32_scalar",
                        "pixels_f32x4_backend_add",
                        "pixels_f32x4_backend_fma",
                        "pixels_f32x4_backend_max",
                        "pixels_f32x4_backend_min",
                        "pixels_f32x4_backend_mul",
                        "pixels_f32x4_backend_select_ge",
                        "pixels_f32x4_backend_select_gt",
                        "pixels_f32x4_backend_splat",
                        "pixels_f32x4_backend_sub",
                        "pixels_f32x4_backend_to_i32x4",
                        "pixels_census_region",
                        "pixels_i32x4_backend_add",
                        "pixels_i32x4_backend_and",
                        "pixels_i32x4_backend_or",
                        "pixels_i32x4_backend_select_gt",
                        "pixels_i32x4_backend_shr_arith_imm",
                        "pixels_i32x4_backend_splat",
                        "pixels_i32x4_backend_sub",
                        "pixels_i32x4_backend_to_f32x4",
                        "pixels_i32_select_gt_scalar",
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
                    if target_module == &core_render_raster
                        && is_pixels_packet_internal_name(name)
                        && !is_pixels_packet_visibility_key(&key)
                        && !paths
                            .get(&key)
                            .is_some_and(|path| is_compiler_internal_fixture_path(Path::new(path)))
                        && !imports_packet_selftest
                    {
                        continue;
                    }
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
    // Where each injected `core.render` surface symbol is actually declared.
    //
    // The renderer is more than one module, so binding the whole injected
    // surface to `core.render` would point every renderer body at the wrong
    // module for any helper that lives in a sibling — and, because the import
    // shadow rule compares declaring modules, would withhold whole imports.
    // Asking the loaded symbol tables who declares a name keeps this correct
    // by construction when a helper moves, instead of through a second list
    // that has to move with it.
    let renderer_surface_owners = {
        let renderer_modules: Vec<&Vec<String>> = symtabs
            .keys()
            .filter(|path| path.len() == 2 && path[0] == "core" && path[1].starts_with("render"))
            .collect();
        let mut owners: BTreeMap<&'static str, Vec<String>> = BTreeMap::new();
        for name in crate::pixels::surface::injected_core_render_names() {
            let declaring: Vec<&Vec<String>> = renderer_modules
                .iter()
                .copied()
                .filter(|path| symtabs[*path].contains_key(name))
                .collect();
            match declaring.as_slice() {
                [] => {}
                [only] => {
                    owners.insert(name, (*only).clone());
                }
                many => {
                    // Two renderer modules declaring one reserved name would
                    // make the injected binding a coin flip, so it fails
                    // closed rather than picking one.
                    return Err(SemaError::at(
                        "sema",
                        format!(
                            "reserved Pixels symbol `{name}` is declared by {} renderer \
                             modules ({}); exactly one module owns each reserved name",
                            many.len(),
                            many.iter()
                                .map(|path| path.join("."))
                                .collect::<Vec<_>>()
                                .join(", ")
                        ),
                        Span::default(),
                    ));
                }
            }
        }
        owners
    };

    for (key, module_bindings) in &mut bindings {
        if !has_pixels_runtime {
            continue;
        }
        // The injected Pixels surface comes from one table
        // (`pixels::surface`); see its module docs for why hand-maintained
        // copies of these lists were a standing source of drift.
        for name in crate::pixels::surface::injected_core_render_names() {
            if symtabs[key].contains_key(name) || module_bindings.contains_key(name) {
                continue;
            }
            let Some(owner) = renderer_surface_owners.get(name) else {
                continue;
            };
            if owner == key {
                continue;
            }
            module_bindings.insert(
                name.to_string(),
                imports::ImportBinding {
                    target_module: owner.clone(),
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
            .map(|(k, m)| (k.clone(), m.as_ref()))
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
    timing("declare-types");

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
        let empty_subs = BTreeMap::new();
        let subs = alias_subs
            .get(&key)
            .and_then(|by_exporter| by_exporter.get(&target_module))
            .unwrap_or(&empty_subs);
        let origin = target_module.join(".");
        if let Some(mut f) = fn_entry {
            types::rekey_decl_fn_names(&mut f.decl, subs);
            dst.fn_decl_module.insert(local.clone(), origin.clone());
            dst.fn_decl_name.insert(local.clone(), target_name.clone());
            dst.fns.insert(local.clone(), f);
        }
        if let Some(mut c) = const_entry {
            types::rekey_type_names(&mut c, subs);
            dst.const_decl_module.insert(local.clone(), origin.clone());
            dst.const_decl_name
                .insert(local.clone(), target_name.clone());
            dst.consts.insert(local.clone(), c);
            if let Some(v) = const_val_entry {
                dst.const_values.insert(local.clone(), v);
            }
        }
        if let Some(mut s) = struct_entry {
            types::rekey_decl_struct_names(&mut s.decl, subs);
            dst.struct_decl_module.insert(local.clone(), origin.clone());
            dst.type_decl_module.insert(local.clone(), origin.clone());
            dst.type_decl_name
                .insert(local.clone(), target_name.clone());
            dst.structs.insert(local.clone(), s);
        }
        if let Some(mut e) = enum_entry {
            types::rekey_decl_enum_names(&mut e.decl, subs);
            dst.type_decl_module.insert(local.clone(), origin);
            dst.type_decl_name
                .insert(local.clone(), target_name.clone());
            dst.enums.insert(local.clone(), e);
        }
        if let Some(mut s) = static_entry {
            types::rekey_type_names(&mut s.ty, subs);
            dst.statics.insert(local, s);
            for layout in layout_entries {
                dst.layouts
                    .entry(layout.name.clone())
                    .or_insert_with(|| layout);
            }
        }
    }

    close_mctx_type_reachability(&mut mctxs, &bindings, &alias_subs);
    timing("module-contexts");

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
    timing("bodies-generics");

    splice_imported_decls(&mut programs, &bindings, &alias_subs);
    timing("splice-imported");

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
    timing("legal");

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
    timing("complete-layouts");

    splice_imported_static_layouts(&mut programs, &bindings);
    timing("splice-static-layouts");

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
    timing("closure-proofs");

    Ok((
        programs,
        CheckDumpTables {
            decl_items_map,
            imported_types,
        },
    ))
}

fn is_pixels_packet_visibility_key(key: &[String]) -> bool {
    // Imported stdlib bodies are evaluated in each caller's module context.
    // Their transitive renderer references therefore need declaration
    // visibility throughout the toolchain-owned closure, while the source
    // token fence and lowering identity check keep the operations unavailable
    // to ordinary project modules.
    matches!(key.first().map(String::as_str), Some("core" | "drivers"))
}

fn is_pixels_packet_internal_name(name: &str) -> bool {
    matches!(
        name,
        "I32x4" | "F32x4" | "I32x4Outcome" | "i32x4_add_checked"
    ) || name.starts_with("pixels_f32")
        || name.starts_with("pixels_i32_")
        || name.starts_with("pixels_i32x4_")
}

pub fn is_compiler_reserved_source_name(name: &str) -> bool {
    name.starts_with("__wrela_")
        || name.starts_with("__pixels_")
        || name.starts_with("RendererWorker")
        || is_pixels_packet_internal_name(name)
}

/// Opt-in marker letting a repository-owned contract fixture name the
/// compiler-reserved Pixels surface in order to pin its contract.
pub const COMPILER_INTERNAL_FIXTURE_MARKER: &str = "@wrela-compiler-internal";

/// Authenticate a filesystem or re-check copy of a shipped stdlib module.
///
/// A vendored stdlib remains supported when its parsed AST is the shipped
/// module's AST. Merely placing a counterfeit file under `stdlib/core` (or
/// supplying a pseudo-path) grants nothing. Re-checks may use synthetic
/// display paths because their AST still has to match the toolchain source.
fn is_authenticated_stdlib_source(module: &Module, source_path: &Path) -> bool {
    let core = crate::loader::toolchain_stdlib_core();
    let drivers = crate::loader::toolchain_stdlib_drivers();
    let canonical_source = source_path.canonicalize().ok();
    if [core.as_path(), drivers.as_path()].into_iter().any(|root| {
        root.canonicalize().is_ok_and(|root| {
            canonical_source
                .as_ref()
                .is_some_and(|source| source.starts_with(root))
        })
    }) {
        return true;
    }

    // A sibling vendored stdlib is authenticated by AST equality with the
    // shipped module, never by the adjacent component names alone.
    let components = source_path.components().collect::<Vec<_>>();
    let Some((namespace, suffix_start)) = components.windows(2).find_map(|pair| {
        let first = pair[0].as_os_str().to_str()?;
        let second = pair[1].as_os_str().to_str()?;
        (first == "stdlib" && matches!(second, "core" | "drivers")).then_some((second, pair[1]))
    }) else {
        return false;
    };
    let root = if namespace == "core" { core } else { drivers };
    let Some(namespace_index) = components
        .iter()
        .position(|component| *component == suffix_start)
    else {
        return false;
    };
    let mut expected = root;
    for component in components.iter().skip(namespace_index + 1) {
        expected.push(component.as_os_str());
    }
    let Ok(source) = std::fs::read_to_string(expected) else {
        return false;
    };
    let Ok(tokens) = crate::syntax::lexer::lex(&source) else {
        return false;
    };
    let Ok(expected_module) = crate::syntax::parser::parse(tokens) else {
        return false;
    };
    crate::syntax::printer::pretty(module) == crate::syntax::printer::pretty(&expected_module)
}

fn is_compiler_internal_fixture_path(source_path: &Path) -> bool {
    let marked = std::fs::read_to_string(source_path).is_ok_and(|source| {
        source.lines().any(|line| {
            let line = line.trim_start();
            line.starts_with('#') && line.contains(COMPILER_INTERNAL_FIXTURE_MARKER)
        })
    });
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
    marked
        && source_path.canonicalize().is_ok_and(|source| {
            [
                "../../tests/golden",
                "../../stdlib/tests",
                "../../bench/proxy-fixtures",
            ]
            .into_iter()
            .filter_map(|relative| manifest.join(relative).canonicalize().ok())
            .any(|root| source.starts_with(root))
        })
}

fn check_reserved_source_names(
    module: &Module,
    path: &str,
    authenticated_internal: bool,
) -> Result<(), SemaError> {
    let source_path = Path::new(path);
    if authenticated_internal || is_authenticated_stdlib_source(module, source_path) {
        return Ok(());
    }

    if matches!(
        module.path.first().map(String::as_str),
        Some("core" | "drivers")
    ) {
        return Err(SemaError::at(
            "name",
            format!(
                "module `{}` is in a compiler-reserved namespace and cannot be declared by user sources",
                module.path.join(".")
            ),
            module.span,
        ));
    }

    let source = std::fs::read_to_string(source_path)
        .unwrap_or_else(|_| crate::syntax::printer::pretty(module));

    // A handful of repository-owned contract fixtures deliberately name
    // generated intrinsics in order to pin their contracts. That trust is
    // declared per file rather than inherited from a directory: blanket
    // trust would prevent fixtures from exercising the fence itself. Both
    // halves are required — a marker outside this repository grants nothing,
    // so a user package cannot opt itself in. The only trusted roots are the
    // golden corpus, stdlib's own contract-test corpus, and the physical
    // proxy corpus; shipped core and driver modules are handled separately
    // above.
    // Match the directive as a comment line, not as a loose substring: prose
    // that merely mentions the marker (a fixture explaining why it does *not*
    // opt in, say) must not thereby opt itself in.
    if is_compiler_internal_fixture_path(source_path) {
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
    alias_subs: &BTreeMap<Vec<String>, BTreeMap<Vec<String>, BTreeMap<String, String>>>,
) {
    let timings = std::env::var_os("WRELA_COMPILER_TIMINGS").is_some();
    let mut last = std::time::Instant::now();
    let mut timing = |stage: &str| {
        if timings {
            eprintln!(
                "compiler-timing: {:.3}s sema-splice-{stage}",
                last.elapsed().as_secs_f64()
            );
        }
        last = std::time::Instant::now();
    };
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
    // The shadow rule below asks whether two modules disagree about a name.
    // Looking only at the first import hop makes a re-export look like a
    // disagreement: after a helper moves out of `core.render` into a sibling
    // module, an injected binding still names `core.render` while
    // `core.render` itself now names the sibling. Chasing to the declaring
    // module makes "same declaration, different number of hops" the non-event
    // it is.
    let resolved_declarations: std::cell::RefCell<
        BTreeMap<(Vec<String>, String), Option<(Vec<String>, String)>>,
    > = std::cell::RefCell::new(BTreeMap::new());
    let resolve_declaring = |m: &Vec<String>, name: &str| -> Option<(Vec<String>, String)> {
        let mut module = m.clone();
        let mut name = name.to_string();
        let mut path: Vec<(Vec<String>, String)> = Vec::new();
        let mut seen: BTreeSet<(Vec<String>, String)> = BTreeSet::new();
        loop {
            let state = (module.clone(), name.clone());
            let cached = { resolved_declarations.borrow().get(&state).cloned() };
            if let Some(cached) = cached {
                let mut cache = resolved_declarations.borrow_mut();
                for visited in path {
                    cache.insert(visited, cached.clone());
                }
                return cached;
            }
            if declared.get(&module).is_some_and(|d| d.contains(&name)) {
                let resolved = Some((module, name));
                let mut cache = resolved_declarations.borrow_mut();
                cache.insert(state, resolved.clone());
                for visited in path {
                    cache.insert(visited, resolved.clone());
                }
                return resolved;
            }
            // Import cycles are legal, so the walk needs its own terminator:
            // a chain that returns to a pair it has already visited declares
            // the name nowhere.
            if !seen.insert(state.clone()) {
                let mut cache = resolved_declarations.borrow_mut();
                cache.insert(state, None);
                for visited in path {
                    cache.insert(visited, None);
                }
                return None;
            }
            path.push(state);
            let Some(binding) = bindings.get(&module).unwrap_or(&empty_bindings).get(&name) else {
                let mut cache = resolved_declarations.borrow_mut();
                for visited in path {
                    cache.insert(visited, None);
                }
                return None;
            };
            module = binding.target_module.clone();
            name = binding.target_name.clone();
        }
    };

    let mut visible_by_module: BTreeMap<Vec<String>, BTreeSet<String>> = BTreeMap::new();
    let mut all_visible_names = BTreeSet::new();
    for module in declared.keys().chain(bindings.keys()) {
        let visible = visible_by_module.entry(module.clone()).or_default();
        visible.extend(declared.get(module).into_iter().flatten().cloned());
        visible.extend(
            bindings
                .get(module)
                .unwrap_or(&empty_bindings)
                .keys()
                .cloned(),
        );
        all_visible_names.extend(visible.iter().cloned());
    }
    // Shadow checks compare the same `(module, name)` resolutions across many
    // importer/exporter pairs. Resolve the rectangular closure once instead
    // of walking (and allocating keys for) every pairwise comparison.
    let resolved_by_module: BTreeMap<Vec<String>, BTreeMap<String, (Vec<String>, String)>> =
        visible_by_module
            .keys()
            .map(|module| {
                let resolved = all_visible_names
                    .iter()
                    .filter_map(|name| {
                        resolve_declaring(module, name).map(|decl| (name.clone(), decl))
                    })
                    .collect();
                (module.clone(), resolved)
            })
            .collect();

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
                for name in &visible_by_module[n] {
                    let resolved = (
                        resolved_by_module[n].get(name).cloned(),
                        resolved_by_module[m].get(name).cloned(),
                    );
                    let (Some(from_n), Some(from_m)) = resolved else {
                        continue;
                    };
                    let same = same_decl(&from_n, &from_m);
                    if !same {
                        shadow.insert((m.clone(), n.clone()), name.clone());
                        break;
                    }
                }
            }
        }
        shadow
    };
    timing("shadow");

    let mut owner_by_name = BTreeMap::new();
    for (owner, names) in &declared {
        for name in names {
            owner_by_name
                .entry(name.clone())
                .or_insert_with(|| owner.join("."));
        }
    }
    let unimported_owners = std::sync::Arc::new(owner_by_name);

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
    // Imported bodies are immutable after checking. Share one typed body per
    // declaration across caller views; only an alias substitution needs an
    // owned rewritten copy. The Pixels closure injects the same large helper
    // surface into many modules, so cloning every body for every view was a
    // quadratic memory and cold-compile tax.
    let shared_fns: BTreeMap<(Vec<String>, String), std::sync::Arc<typed::TypedFn>> = programs
        .iter()
        .flat_map(|(module, program)| {
            program.fns.iter().map(move |(name, function)| {
                (
                    (module.clone(), name.clone()),
                    std::sync::Arc::new(function.clone()),
                )
            })
        })
        .collect();
    let shared_structs: BTreeMap<(Vec<String>, String), std::sync::Arc<typed::TypedStruct>> =
        programs
            .iter()
            .flat_map(|(module, program)| {
                program.structs.iter().map(move |(name, structure)| {
                    (
                        (module.clone(), name.clone()),
                        std::sync::Arc::new(structure.clone()),
                    )
                })
            })
            .collect();
    let shared_enums: BTreeMap<(Vec<String>, String), std::sync::Arc<typed::TypedEnum>> = programs
        .iter()
        .flat_map(|(module, program)| {
            program.enums.iter().map(move |(name, enumeration)| {
                (
                    (module.clone(), name.clone()),
                    std::sync::Arc::new(enumeration.clone()),
                )
            })
        })
        .collect();
    timing("share");
    let mut companions_done: BTreeSet<(Vec<String>, Vec<String>)> = BTreeSet::new();
    let mut instantiations_done: BTreeSet<(Vec<String>, Vec<String>)> = BTreeSet::new();
    for (key, local, target_module, target_name) in splices {
        let withheld = shadow.get(&(key.clone(), target_module.clone())).cloned();
        let Some((const_entry, fn_entry, struct_entry, enum_entry, static_entry)) =
            programs.get(&target_module).map(|src| {
                (
                    src.consts.get(&target_name).cloned(),
                    shared_fns
                        .get(&(target_module.clone(), target_name.clone()))
                        .cloned(),
                    shared_structs
                        .get(&(target_module.clone(), target_name.clone()))
                        .cloned(),
                    shared_enums
                        .get(&(target_module.clone(), target_name.clone()))
                        .cloned(),
                    src.statics.get(&target_name).cloned(),
                )
            })
        else {
            continue;
        };
        let body_bearing = const_entry.is_some()
            || fn_entry.is_some()
            || struct_entry.is_some()
            || enum_entry.is_some()
            || static_entry.is_some();
        if let (Some(witness), true) = (&withheld, body_bearing) {
            let dst = programs.get_mut(&key).expect("key is a key of programs");
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
            let empty_subs = BTreeMap::new();
            let subs = alias_subs
                .get(&key)
                .and_then(|by_exporter| by_exporter.get(&target_module))
                .unwrap_or(&empty_subs);
            let pair = (key.clone(), target_module.clone());
            let copy_companions = fn_entry.is_some() && companions_done.insert(pair.clone());
            let copy_instantiations = instantiations_done.insert(pair);
            let (companion_statics, companion_consts, inst_entries) = {
                let src = programs
                    .get(&target_module)
                    .expect("target module was present above");
                let dst = programs.get(&key).expect("key is a key of programs");
                let companion_statics = if copy_companions {
                    src.statics
                        .iter()
                        .filter(|(name, _)| !dst.statics.contains_key(*name))
                        .map(|(name, value)| (name.clone(), value.clone()))
                        .collect()
                } else {
                    Vec::new()
                };
                let companion_consts = if copy_companions {
                    src.consts
                        .iter()
                        .filter(|(name, _)| !dst.consts.contains_key(*name))
                        .map(|(name, value)| (name.clone(), value.clone()))
                        .collect()
                } else {
                    Vec::new()
                };
                let inst_entries = if copy_instantiations {
                    src.instantiations
                        .iter()
                        .filter_map(|(instantiation_key, value)| {
                            let new_key = typed::rekey_canonical_key(instantiation_key, subs);
                            (!dst.imported.instantiations.contains_key(&new_key)).then(|| {
                                let mut value = value.clone();
                                typed::rekey_instantiation(&mut value, subs);
                                (new_key, value)
                            })
                        })
                        .collect::<Vec<_>>()
                } else {
                    Vec::new()
                };
                (companion_statics, companion_consts, inst_entries)
            };
            let dst = programs.get_mut(&key).expect("key is a key of programs");
            if let Some(mut c) = const_entry {
                typed::rekey_const_names(&mut c, subs);
                dst.imported.consts.insert(local.clone(), c);
            }
            if let Some(f) = fn_entry {
                let f = if subs.is_empty() {
                    f
                } else {
                    let mut f = (*f).clone();
                    typed::rekey_fn_names(&mut f, subs);
                    std::sync::Arc::new(f)
                };
                dst.imported.fns.insert(local.clone(), f);
            }
            if let Some(s) = struct_entry {
                let s = if subs.is_empty() {
                    s
                } else {
                    let mut s = (*s).clone();
                    typed::rekey_struct_names(&mut s, subs);
                    std::sync::Arc::new(s)
                };
                dst.imported.structs.insert(local.clone(), s);
            }
            if let Some(e) = enum_entry {
                let e = if subs.is_empty() {
                    e
                } else {
                    let mut e = (*e).clone();
                    typed::rekey_enum_names(&mut e, subs);
                    std::sync::Arc::new(e)
                };
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
            for (new_key, inst) in inst_entries {
                dst.imported.instantiations.entry(new_key).or_insert(inst);
            }
        }
    }
    timing("bindings");

    close_typed_type_reachability(programs, bindings, alias_subs);
    timing("types");

    for program in programs.values_mut() {
        program.imported.unimported_owners = unimported_owners.clone();
    }
}

fn close_mctx_type_reachability(
    mctxs: &mut BTreeMap<Vec<String>, bodies::ModuleCtx>,
    bindings: &BTreeMap<Vec<String>, imports::ImportBindings>,
    alias_subs: &BTreeMap<Vec<String>, BTreeMap<Vec<String>, BTreeMap<String, String>>>,
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
                let empty_subs = BTreeMap::new();
                let subs = alias_subs
                    .get(importer)
                    .and_then(|by_exporter| by_exporter.get(&def_module))
                    .unwrap_or(&empty_subs);
                let dst = mctxs.get_mut(importer).expect("importer is a key");
                if let Some(mut s) = struct_entry {
                    types::rekey_decl_struct_names(&mut s.decl, subs);
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
                    types::rekey_decl_enum_names(&mut e.decl, subs);
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
    alias_subs: &BTreeMap<Vec<String>, BTreeMap<Vec<String>, BTreeMap<String, String>>>,
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
                    .structs
                    .get(&name)
                    .or_else(|| dst.imported.structs.get(&name).map(|value| value.as_ref()))
                {
                    typed::collect_named_types_from_struct(s, &mut mentioned);
                } else if let Some(e) = dst
                    .enums
                    .get(&name)
                    .or_else(|| dst.imported.enums.get(&name).map(|value| value.as_ref()))
                {
                    typed::collect_named_types_from_enum(e, &mut mentioned);
                } else if let Some(f) = dst.fns.get(&name).or_else(|| {
                    dst.imported
                        .fns
                        .get(&name)
                        .map(|function| function.as_ref())
                }) {
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
                            .or_else(|| {
                                src.imported
                                    .structs
                                    .get(&def_name)
                                    .map(|value| value.as_ref())
                            })
                            .cloned(),
                        src.enums
                            .get(&def_name)
                            .or_else(|| {
                                src.imported
                                    .enums
                                    .get(&def_name)
                                    .map(|value| value.as_ref())
                            })
                            .cloned(),
                    )
                };
                let empty_subs = BTreeMap::new();
                let subs = alias_subs
                    .get(importer)
                    .and_then(|by_exporter| by_exporter.get(&def_module))
                    .unwrap_or(&empty_subs);
                let dst = programs.get_mut(importer).expect("importer is a key");
                if let Some(mut s) = struct_entry {
                    typed::rekey_struct_names(&mut s, subs);
                    if s.name != tname {
                        let mut name_sub = BTreeMap::new();
                        name_sub.insert(s.name.clone(), tname.clone());
                        typed::rekey_struct_names(&mut s, &name_sub);
                    }
                    dst.imported
                        .structs
                        .insert(tname.clone(), std::sync::Arc::new(s));
                    origins.insert(tname.clone(), def_module);
                    queue.push(tname);
                } else if let Some(mut e) = enum_entry {
                    typed::rekey_enum_names(&mut e, subs);
                    dst.imported
                        .enums
                        .insert(tname.clone(), std::sync::Arc::new(e));
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
                    .or_else(|| src.imported.structs.get(&tname).map(|value| value.as_ref()))
                {
                    struct_entry = Some(s.clone());
                    def_module = Some(mod_key.clone());
                    break;
                }
                if let Some(e) = src
                    .enums
                    .get(&tname)
                    .or_else(|| src.imported.enums.get(&tname).map(|value| value.as_ref()))
                {
                    enum_entry = Some(e.clone());
                    def_module = Some(mod_key.clone());
                    break;
                }
            }
            let Some(def_module) = def_module else {
                continue;
            };
            let empty_subs = BTreeMap::new();
            let subs = alias_subs
                .get(importer)
                .and_then(|by_exporter| by_exporter.get(&def_module))
                .unwrap_or(&empty_subs);
            let dst = programs.get_mut(importer).expect("importer is a key");
            if let Some(mut s) = struct_entry {
                typed::rekey_struct_names(&mut s, subs);
                if s.name != tname {
                    let mut name_sub = BTreeMap::new();
                    name_sub.insert(s.name.clone(), tname.clone());
                    typed::rekey_struct_names(&mut s, &name_sub);
                }
                dst.imported.structs.insert(tname, std::sync::Arc::new(s));
            } else if let Some(mut e) = enum_entry {
                typed::rekey_enum_names(&mut e, subs);
                dst.imported.enums.insert(tname, std::sync::Arc::new(e));
            }
        }
    }
}

fn splice_imported_static_layouts(
    programs: &mut BTreeMap<Vec<String>, typed::TypedProgram>,
    bindings: &BTreeMap<Vec<String>, imports::ImportBindings>,
) {
    // A function/static import makes every layout from its exporter visible.
    // Compute that closure once per importer/exporter pair: the Pixels prelude
    // injects hundreds of functions from one generated module, and cloning the
    // same complete layout vector once per binding is needlessly quadratic.
    let mut requests: BTreeMap<(Vec<String>, Vec<String>), (bool, BTreeSet<String>)> =
        BTreeMap::new();
    for (importer, module_bindings) in bindings {
        for binding in module_bindings.values() {
            let Some(src) = programs.get(&binding.target_module) else {
                continue;
            };
            let request = requests
                .entry((importer.clone(), binding.target_module.clone()))
                .or_default();
            if src.statics.contains_key(&binding.target_name)
                || src.fns.contains_key(&binding.target_name)
            {
                request.0 = true;
            } else {
                request.1.insert(binding.target_name.clone());
            }
        }
    }
    for ((importer, exporter), (all, names)) in requests {
        let src = &programs[&exporter];
        let layouts: Vec<_> = if all {
            src.layouts.clone()
        } else {
            src.layouts
                .iter()
                .filter(|layout| names.contains(&layout.name))
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
    specialized: &BTreeMap<Vec<String>, std::borrow::Cow<'_, Module>>,
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
    specialized: &BTreeMap<Vec<String>, std::borrow::Cow<'_, Module>>,
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
    fn combined_typed_check_preserves_both_stable_observations() {
        let src = concat!(
            "module m\n\n",
            "pub struct Pair:\n",
            "    left: u64\n",
            "    right: u64\n\n",
            "pub fn sum(pair: Pair) -> u64:\n",
            "    return pair.left + pair.right\n",
        );
        let module = parser::parse(lexer::lex(src).expect("lex")).expect("parse");
        let expected_program = check_typed(&module, "m.wr").expect("typed");
        let expected_dump = check_dump(&module, "m.wr").expect("check dump");
        let (program, dump) = check_typed_with_dump(&module, "m.wr").expect("combined check");
        assert_eq!(program, expected_program);
        assert_eq!(dump, expected_dump);
    }

    #[test]
    fn imported_static_layout_batches_are_complete_and_deduplicated() {
        fn layout(name: &str) -> types::LayoutType {
            types::LayoutType {
                name: name.to_string(),
                kind: types::LayoutKind::Wire,
                endian: types::LayoutEndian::Little,
                size: Some(0),
                padding: 0,
                entries: Vec::new(),
            }
        }

        let importer = vec!["app".to_string()];
        let exporter = vec!["wire".to_string()];
        let mut programs = BTreeMap::from([
            (
                importer.clone(),
                typed::TypedProgram {
                    layouts: vec![layout("Header")],
                    ..typed::TypedProgram::default()
                },
            ),
            (
                exporter.clone(),
                typed::TypedProgram {
                    layouts: vec![layout("Header"), layout("Record")],
                    ..typed::TypedProgram::default()
                },
            ),
        ]);
        let bindings = BTreeMap::from([(
            importer.clone(),
            imports::ImportBindings::from([
                (
                    "LocalHeader".to_string(),
                    imports::ImportBinding {
                        target_module: exporter.clone(),
                        target_name: "Header".to_string(),
                    },
                ),
                (
                    "LocalRecord".to_string(),
                    imports::ImportBinding {
                        target_module: exporter,
                        target_name: "Record".to_string(),
                    },
                ),
            ]),
        )]);

        splice_imported_static_layouts(&mut programs, &bindings);
        let names: Vec<_> = programs[&importer]
            .layouts
            .iter()
            .map(|layout| layout.name.as_str())
            .collect();
        assert_eq!(names, ["Header", "Record"]);
    }

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
            "module m\n\nfrom core.render_raster import F32x4\n",
            "module m\n\nfn f() -> u32:\n    return pixels_f32_to_bits(0.0)\n",
        ] {
            let module = parser::parse(lexer::lex(source).expect("lex")).expect("parse");
            let error = check_reserved_source_names(&module, "/nonexistent/user-module.wr", false)
                .expect_err("reserved name must fail before lowering");
            assert_eq!(error.category, "name");
            assert!(error.message.contains("compiler-reserved namespace"));
        }
    }

    #[test]
    fn marked_physical_proxy_fixture_can_use_internal_trace_hook() {
        let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join(
            "../../bench/proxy-fixtures/boot-pixels-partial-mode-three-core/src/examples/boot_pixels_partial_mode_three_core.wr",
        );
        assert!(fixture.is_file());
        assert!(is_compiler_internal_fixture_path(&fixture));
    }

    #[test]
    fn user_modules_cannot_claim_toolchain_module_namespaces() {
        for source in [
            "module core.render_raster\n\nfn harmless() -> u32:\n    return 0\n",
            "module drivers.display\n\nfn harmless() -> u32:\n    return 0\n",
        ] {
            let module = parser::parse(lexer::lex(source).expect("lex")).expect("parse");
            let error = check_reserved_source_names(&module, "/nonexistent/user-module.wr", false)
                .expect_err("toolchain namespace must fail before declaration resolution");
            assert_eq!(error.category, "name");
            assert_eq!(
                error.message,
                format!(
                    "module `{}` is in a compiler-reserved namespace and cannot be declared by user sources",
                    module.path.join(".")
                )
            );
        }
    }

    #[test]
    fn counterfeit_stdlib_path_does_not_grant_internal_source_capability() {
        let source = "module core.render_raster\n\nfn pixels_f32_to_bits(value: f32) -> u32:\n    return 0\n";
        let module = parser::parse(lexer::lex(source).expect("lex")).expect("parse");
        let error = check_reserved_source_names(
            &module,
            "/tmp/untrusted-project/nested/stdlib/core/render_raster.wr",
            false,
        )
        .expect_err("path spelling must not authenticate a counterfeit stdlib module");
        assert_eq!(error.category, "name");
        assert!(error.message.contains("compiler-reserved namespace"));
    }

    #[test]
    fn untrusted_synthetic_path_does_not_grant_internal_source_capability() {
        let source = "module user\n\nfn pixels_f32_to_bits(value: f32) -> u32:\n    return 0\n";
        let module = parser::parse(lexer::lex(source).expect("lex")).expect("parse");
        let error = check_typed(&module, "<user>")
            .expect_err("caller-provided pseudo-path must remain untrusted");
        assert_eq!(error.category, "name");
        assert!(error.message.contains("compiler-reserved namespace"));
    }

    #[test]
    fn imported_typed_bodies_share_storage_but_alias_rewrites_are_isolated() {
        let sources = [
            (
                vec!["a".to_string()],
                "module a\n\npub fn value() -> u32:\n    return 7\n",
            ),
            (
                vec!["b".to_string()],
                "module b\n\nfrom a import value\n\npub fn b() -> u32:\n    return value()\n",
            ),
            (
                vec!["c".to_string()],
                "module c\n\nfrom a import value\n\npub fn c() -> u32:\n    return value()\n",
            ),
            (
                vec!["d".to_string()],
                "module d\n\nfrom a import value as renamed\n\npub fn d() -> u32:\n    return renamed()\n",
            ),
        ];
        let modules = sources
            .iter()
            .map(|(key, source)| {
                (
                    key.clone(),
                    parser::parse(lexer::lex(source).expect("lex")).expect("parse"),
                )
            })
            .collect::<BTreeMap<_, _>>();
        let paths = sources
            .iter()
            .map(|(key, _)| (key.clone(), format!("<{}>", key.join("."))))
            .collect::<BTreeMap<_, _>>();
        let programs = check_program_typed(&modules, &paths).expect("check closure");
        let b = &programs[&vec!["b".to_string()]].imported.fns["value"];
        let c = &programs[&vec!["c".to_string()]].imported.fns["value"];
        let d = &programs[&vec!["d".to_string()]].imported.fns["renamed"];
        assert!(std::sync::Arc::ptr_eq(b, c));
        assert!(!std::sync::Arc::ptr_eq(b, d));
        assert_eq!(
            d.as_ref(),
            b.as_ref(),
            "an alias with no internal name references keeps the body exact"
        );
    }
}
