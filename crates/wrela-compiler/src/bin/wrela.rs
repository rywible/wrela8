use std::cell::Cell;
use std::collections::BTreeMap;
use std::fs::OpenOptions;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};
use std::time::{Duration, Instant};

use wrela_compiler::eval;
use wrela_compiler::layout;
use wrela_compiler::loader;
use wrela_compiler::lower;
use wrela_compiler::placement;
use wrela_compiler::report;
use wrela_compiler::rtconfig;
use wrela_compiler::sema;
use wrela_compiler::sema::typed::{TestKind, TypedProgram};
use wrela_compiler::syntax::ast::Module;
use wrela_compiler::syntax::{lexer, parser, printer};

const USAGE: &str = "usage: wrela dump --stage=<tokens|ast|pretty|check|typed|layout-types|cfg|frame|mwir-opt|relax|flowwir|mwir|asm|cost|image|field-graph|frame-program|render-layout|report|rtconfig> [--renderer=<index>] [--timings] [--omit-dmb] [--block-count] [--mode=dev|release] [--ghz=<n>] <file.wr>\n       wrela test <file.wr> [--vmm <path>] [--pixels-telemetry] [--omit-dmb] [--block-count] [--mode=dev|release] [--ghz=<n>]\n       wrela build <file.wr> [--out-dir <dir>] [--omit-dmb] [--block-count] [--mode=dev|release] [--ghz=<n>]\n       wrela version";

thread_local! {
    static DUMP_HAD_DIAGNOSTIC: Cell<bool> = const { Cell::new(false) };
}

fn note_dump_diagnostic() {
    DUMP_HAD_DIAGNOSTIC.with(|c| c.set(true));
}

fn render_sema_error(e: &sema::SemaError) -> String {
    let mut s = if e.omit_location {
        format!("error[{}]: {}\n", e.category, e.message)
    } else {
        format!(
            "error[{}]: {} at {}:{}\n",
            e.category, e.message, e.line, e.col
        )
    };
    for line in &e.extra_lines {
        s.push_str(line);
        s.push('\n');
    }
    s
}

fn print_sema_error(e: &sema::SemaError) {
    eprint!("{}", render_sema_error(e));
    note_dump_diagnostic();
}

fn print_lex_error(e: &lexer::LexError) {
    eprintln!("error[lex]: {} at {}:{}", e.message, e.line, e.col);
    note_dump_diagnostic();
}

fn print_parse_error(e: &parser::ParseError) {
    eprintln!("error[parse]: {} at {}:{}", e.message, e.line, e.col);
    note_dump_diagnostic();
}

fn print_line_diagnostic(line: &str) {
    eprintln!("{line}");
    note_dump_diagnostic();
}

struct CheckedClosure {
    root: String,
    programs: BTreeMap<String, TypedProgram>,
    modules: BTreeMap<String, Module>,
}

fn check_closure(path: &str, module: Module) -> Result<CheckedClosure, ()> {
    if module.imports.is_empty() && !loader::module_is_runtime_bearing(&module) {
        match sema::check_typed(&module, path) {
            Ok(program) => {
                let addr = module.path.join(".");
                let mut programs = BTreeMap::new();
                let mut modules = BTreeMap::new();
                modules.insert(addr.clone(), module);
                programs.insert(addr.clone(), program);
                Ok(CheckedClosure {
                    root: addr,
                    programs,
                    modules,
                })
            }
            Err(e) => {
                print_sema_error(&e);
                Err(())
            }
        }
    } else if module.imports.is_empty() {
        match load_runtime_bearing_singleton(path, module) {
            Ok(c) => Ok(c),
            Err(()) => Err(()),
        }
    } else {
        match loader::load_closure(Path::new(path)) {
            Ok(loaded) => {
                let paths: BTreeMap<Vec<String>, String> = loaded
                    .modules
                    .iter()
                    .map(|(k, m)| (k.clone(), m.file.display().to_string()))
                    .collect();
                let modules_by_key: BTreeMap<Vec<String>, Module> = loaded
                    .modules
                    .into_iter()
                    .map(|(k, m)| (k, m.module))
                    .collect();
                let root = loaded.root.join(".");
                match sema::check_program_typed(&modules_by_key, &paths) {
                    Ok(progs) => {
                        let programs: BTreeMap<String, TypedProgram> =
                            progs.into_iter().map(|(k, p)| (k.join("."), p)).collect();
                        let modules: BTreeMap<String, Module> = modules_by_key
                            .into_iter()
                            .map(|(k, m)| (k.join("."), m))
                            .collect();
                        Ok(CheckedClosure {
                            root,
                            programs,
                            modules,
                        })
                    }
                    Err(e) => {
                        print_sema_error(&e);
                        Err(())
                    }
                }
            }
            Err(loader::LoadError::Lex(e)) => {
                print_lex_error(&e);
                Err(())
            }
            Err(loader::LoadError::Parse(e)) => {
                print_parse_error(&e);
                Err(())
            }
            Err(loader::LoadError::Build(e)) => {
                print_sema_error(&e);
                Err(())
            }
        }
    }
}

fn load_runtime_bearing_singleton(path: &str, module: Module) -> Result<CheckedClosure, ()> {
    let (runtime_key, runtime_loaded) = match loader::load_runtime_module() {
        Ok(v) => v,
        Err(loader::LoadError::Lex(e)) => {
            print_lex_error(&e);
            return Err(());
        }
        Err(loader::LoadError::Parse(e)) => {
            print_parse_error(&e);
            return Err(());
        }
        Err(loader::LoadError::Build(e)) => {
            print_sema_error(&e);
            return Err(());
        }
    };
    let root_key = module.path.clone();
    let runtime_path = runtime_loaded.file.display().to_string();
    let mut modules_by_key = BTreeMap::new();
    modules_by_key.insert(root_key.clone(), module.clone());
    modules_by_key.insert(runtime_key.clone(), runtime_loaded.module);
    let stub_module = match rtconfig::parse_generated(&rtconfig::stub_text()) {
        Ok(m) => m,
        Err(e) => {
            print_line_diagnostic(&format!("error[build]: {e}"));
            return Err(());
        }
    };
    let gen_key: Vec<String> = loader::IMAGE_RUNTIME_MODULE_KEY
        .iter()
        .map(|s| (*s).to_string())
        .collect();
    modules_by_key.insert(gen_key.clone(), stub_module);
    let mut paths = BTreeMap::new();
    paths.insert(root_key.clone(), path.to_string());
    paths.insert(runtime_key.clone(), runtime_path);
    paths.insert(gen_key, rtconfig::GENERATED_INPUT_PATH.to_string());
    let time_key: Option<Vec<String>> = if loader::module_mentions_time(&module) {
        let (time_key, time_loaded) = match loader::load_time_module() {
            Ok(v) => v,
            Err(loader::LoadError::Lex(e)) => {
                print_lex_error(&e);
                return Err(());
            }
            Err(loader::LoadError::Parse(e)) => {
                print_parse_error(&e);
                return Err(());
            }
            Err(loader::LoadError::Build(e)) => {
                print_sema_error(&e);
                return Err(());
            }
        };
        let time_path = time_loaded.file.display().to_string();
        paths.insert(time_key.clone(), time_path);
        modules_by_key.insert(time_key.clone(), time_loaded.module);
        Some(time_key)
    } else {
        None
    };
    match sema::check_program_typed(&modules_by_key, &paths) {
        Ok(mut progs) => {
            if let Some(tk) = &time_key {
                progs.remove(tk);
                modules_by_key.remove(tk);
            }
            let programs: BTreeMap<String, TypedProgram> =
                progs.into_iter().map(|(k, p)| (k.join("."), p)).collect();
            let modules: BTreeMap<String, Module> = modules_by_key
                .into_iter()
                .map(|(k, m)| (k.join("."), m))
                .collect();
            Ok(CheckedClosure {
                root: root_key.join("."),
                programs,
                modules,
            })
        }
        Err(e) => {
            print_sema_error(&e);
            Err(())
        }
    }
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("version") => {
            println!(
                "wrela {} (machine v{})",
                env!("CARGO_PKG_VERSION"),
                wrela_machine::MACHINE_REVISION
            );
            ExitCode::SUCCESS
        }
        Some("dump") => dump(&args[1..]),
        Some("test") => test_cmd(&args[1..]),
        Some("build") => build_cmd(&args[1..]),
        _ => {
            eprintln!("{USAGE}");
            ExitCode::FAILURE
        }
    }
}

fn run_layout_types_stage(
    modules: &[(String, Module)],
    programs: &BTreeMap<String, &TypedProgram>,
) {
    let mut by_module = Vec::with_capacity(modules.len());
    for (path, module) in modules {
        let specialized = match sema::specialize::specialize(module) {
            Ok(m) => m,
            Err(e) => return print_sema_error(&e),
        };
        let mut layouts = match sema::types::check_layouts(&specialized) {
            Ok(layouts) => layouts,
            Err(e) => return print_sema_error(&e),
        };
        if let Some(program) = programs.get(path) {
            if let Err(e) = sema::types::complete_layouts(&specialized, program, &mut layouts) {
                return print_sema_error(&e);
            }
        }
        by_module.push((path.clone(), layouts));
    }
    match sema::types::dump_layouts(&by_module) {
        Ok(text) => print!("{text}"),
        Err(e) => print_sema_error(&e),
    }
}

fn run_image_stage(programs: &BTreeMap<String, TypedProgram>) {
    let candidates: Vec<(&String, &String)> = programs
        .iter()
        .filter_map(|(module, p)| p.image_fn.as_ref().map(|f| (module, f)))
        .collect();
    match candidates.len() {
        0 => print_line_diagnostic("error[build]: no `@image` fn found in the build closure"),
        1 => {
            let (module, fn_name) = candidates[0];
            let program = &programs[module];
            match eval::interp::eval_image(program, fn_name) {
                Ok(graph) => match eval::image_checks::check_sealed(&graph, program, programs) {
                    Ok(checked_image) => {
                        if let Err(error) = wrela_compiler::pixels::compile_all(
                            programs,
                            module,
                            &graph,
                            &checked_image.renderer_configs,
                        ) {
                            print_sema_error(&eval::image_checks::pixels_error(error));
                            return;
                        }
                        let mut enum_variants: BTreeMap<String, Vec<String>> = BTreeMap::new();
                        for (k, e) in program.enums.iter().chain(program.imported.enums.iter()) {
                            enum_variants
                                .entry(k.clone())
                                .or_insert_with(|| e.variants.clone());
                        }
                        print!("{}", eval::image::dump(&enum_variants, &graph));
                    }
                    Err(e) => print_sema_error(&e),
                },
                Err(e) => print_sema_error(&eval::to_sema_error(e)),
            }
        }
        _ => {
            let names: Vec<String> = candidates
                .iter()
                .map(|(module, fn_name)| format!("{module}::{fn_name}"))
                .collect();
            print_line_diagnostic(&format!(
                "error[build]: more than one `@image` fn reachable in the build closure ({})",
                names.join(", ")
            ));
        }
    }
}

fn pixels_dump_stage(stage: &str) -> Option<wrela_compiler::pixels::PixelsDumpStage> {
    use wrela_compiler::pixels::PixelsDumpStage;

    match stage {
        "field-graph" => Some(PixelsDumpStage::FieldGraph),
        "frame-program" => Some(PixelsDumpStage::FrameProgram),
        "render-layout" => Some(PixelsDumpStage::RenderLayout),
        _ => None,
    }
}

fn parse_renderer_index(value: &str) -> Result<usize, &'static str> {
    value
        .parse()
        .map_err(|_| "--renderer requires a nonnegative integer index")
}

fn validate_renderer_stage(stage: &str, renderer_index: Option<usize>) -> Result<(), &'static str> {
    if renderer_index.is_some() && pixels_dump_stage(stage).is_none() {
        Err("--renderer is valid only with --stage=field-graph, \
             --stage=frame-program, or --stage=render-layout")
    } else {
        Ok(())
    }
}

fn select_renderer_index(count: usize, requested: Option<usize>) -> Result<usize, String> {
    if let Some(index) = requested {
        if index >= count {
            return Err(format!(
                "renderer index {index} is out of range; image declares {count} renderer(s)"
            ));
        }
        return Ok(index);
    }
    if count != 1 {
        return Err(format!(
            "Pixels dump requires exactly one renderer, found {count}; use --renderer=<index>"
        ));
    }
    Ok(0)
}

fn print_pixels_error(message: &str) {
    let message = message.strip_prefix("pixels: ").unwrap_or(message);
    print_line_diagnostic(&format!("error[pixels]: {message}"));
}

fn run_pixels_stage(
    programs: &BTreeMap<String, TypedProgram>,
    modules: &BTreeMap<String, Module>,
    stage: wrela_compiler::pixels::PixelsDumpStage,
    renderer_index: Option<usize>,
) {
    let candidates: Vec<(&String, &String)> = programs
        .iter()
        .filter_map(|(module, program)| program.image_fn.as_ref().map(|f| (module, f)))
        .collect();
    let [(module, fn_name)] = candidates.as_slice() else {
        print_line_diagnostic(&format!(
            "error[pixels]: Pixels dump requires exactly one `@image` fn, found {}",
            candidates.len()
        ));
        return;
    };
    let program = &programs[*module];
    let graph = match eval::interp::eval_image(program, fn_name) {
        Ok(graph) => graph,
        Err(error) => {
            print_sema_error(&eval::to_sema_error(error));
            return;
        }
    };
    let checked = match eval::image_checks::check_sealed(&graph, program, programs) {
        Ok(checked) => checked,
        Err(error) => {
            print_sema_error(&error);
            return;
        }
    };
    let program_set = match wrela_compiler::pixels::compile_all(
        programs,
        module,
        &graph,
        &checked.renderer_configs,
    ) {
        Ok(program_set) => program_set,
        Err(error) => {
            print_sema_error(&eval::image_checks::pixels_error(error));
            return;
        }
    };
    if graph.renderers.is_empty() {
        if let Some(index) = renderer_index {
            print_pixels_error(&format!(
                "renderer index {index} is out of range; image declares 0 renderers"
            ));
        } else {
            print!("{}", wrela_compiler::pixels::dump_zero_renderers(stage));
        }
        return;
    }
    let selected_index = match select_renderer_index(graph.renderers.len(), renderer_index) {
        Ok(index) => index,
        Err(message) => {
            print_pixels_error(&message);
            return;
        }
    };
    if stage == wrela_compiler::pixels::PixelsDumpStage::FieldGraph {
        print!(
            "{}",
            wrela_compiler::pixels::dump_structural_graphs(
                &[(
                    selected_index,
                    program_set.symbolic_graphs[selected_index].clone(),
                    program_set.structural_programs[selected_index].clone(),
                    program_set.projective_programs[selected_index].clone(),
                )],
                &checked.renderer_configs,
            )
        );
        return;
    }
    let mut layout_ctx = match layout::merge_layout_ctx(modules) {
        Ok(layout_ctx) => layout_ctx,
        Err(error) => {
            print_sema_error(&error);
            return;
        }
    };
    layout::enrich_layout_ctx_with_instantiations(&mut layout_ctx, programs);
    let image_layout = match layout::try_layout_program(
        programs,
        &layout_ctx,
        &graph,
        modules,
        Some(&checked.renderer_configs),
        Some(&program_set),
        false,
    ) {
        Ok(Some(layout)) => layout,
        Ok(None) => {
            print_pixels_error("ordinary image code generation is unavailable");
            return;
        }
        Err(message) => {
            print_pixels_error(&message);
            return;
        }
    };
    let Some(placement) = image_layout
        .renderers
        .iter()
        .find(|placement| placement.index == selected_index)
    else {
        print_pixels_error("compiled renderer has no image placement");
        return;
    };
    let renderer = &program_set.compiled_renderers[selected_index];
    match stage {
        wrela_compiler::pixels::PixelsDumpStage::FieldGraph => unreachable!(),
        wrela_compiler::pixels::PixelsDumpStage::FrameProgram => {
            let generated_source = match wrela_compiler::pixels::glue::configuration_source(
                &image_layout.renderers,
                &program_set.compiled_renderers,
                false,
            ) {
                Ok(source) => source,
                Err(message) => {
                    print_pixels_error(&message);
                    return;
                }
            };
            match wrela_compiler::pixels::dump_frame_program(renderer, placement, &generated_source)
            {
                Ok(text) => print!("{text}"),
                Err(message) => print_pixels_error(&message),
            }
        }
        wrela_compiler::pixels::PixelsDumpStage::RenderLayout => {
            print!(
                "{}",
                wrela_compiler::pixels::dump_render_layout(renderer, placement)
            )
        }
    }
}

struct BuildReport {
    text: String,
    name: String,
    target: String,
    devices: usize,
    drivers: usize,
    actors: usize,
    pools: usize,
    img: Option<Vec<u8>>,
}

fn first_field_value<'a>(text: &'a str, prefix: &str) -> Option<&'a str> {
    text.lines()
        .find_map(|line| line.trim_start().strip_prefix(prefix))
}

fn build_report(
    programs: &BTreeMap<String, TypedProgram>,
    file_paths: &BTreeMap<String, std::path::PathBuf>,
    modules: &BTreeMap<String, Module>,
    ghz: f64,
) -> Result<BuildReport, String> {
    let candidates: Vec<(&String, &String)> = programs
        .iter()
        .filter_map(|(module, p)| p.image_fn.as_ref().map(|f| (module, f)))
        .collect();
    match candidates.len() {
        0 => Err("error[build]: no `@image` fn found in the build closure\n".to_string()),
        1 => {
            let (module, fn_name) = candidates[0];
            let program = &programs[module];
            match eval::interp::eval_image(program, fn_name) {
                Ok(graph) => match eval::image_checks::check_sealed(&graph, program, programs) {
                    Ok(checked_image) => {
                        let pixels_programs = wrela_compiler::pixels::compile_all(
                            programs,
                            module,
                            &graph,
                            &checked_image.renderer_configs,
                        )
                        .map_err(|error| {
                            render_sema_error(&eval::image_checks::pixels_error(error))
                        })?;
                        let mut inputs = Vec::with_capacity(file_paths.len());
                        for (addr, path) in file_paths {
                            let rel = report::address_to_relative_path(addr);
                            if rel == loader::RUNTIME_INPUT_PATH {
                                let runtime_key: Vec<String> = loader::RUNTIME_MODULE_KEY
                                    .iter()
                                    .map(|s| (*s).to_string())
                                    .collect();
                                let explicit = modules
                                    .values()
                                    .any(|m| m.imports.iter().any(|imp| imp.path == runtime_key));
                                if !explicit {
                                    continue;
                                }
                            }
                            if rel == rtconfig::GENERATED_INPUT_PATH
                                || addr == rtconfig::MODULE_ADDR
                                || path.to_string_lossy() == rtconfig::GENERATED_INPUT_PATH
                                || addr.as_str() == loader::IMAGE_PIXELS_MODULE_ADDR
                                || path.to_string_lossy()
                                    == loader::GENERATED_PIXELS_STUB_INPUT_PATH
                                || path.to_string_lossy() == loader::GENERATED_PIXELS_INPUT_PATH
                            {
                                continue;
                            }
                            let bytes = std::fs::read(path).map_err(|e| {
                                format!("error[build]: cannot read `{}`: {e}\n", path.display())
                            })?;
                            inputs.push(report::BuildInput {
                                path: rel,
                                digest: report::sha256_hex(&bytes),
                            });
                        }
                        let mut layout_ctx =
                            layout::merge_layout_ctx(modules).map_err(|e| render_sema_error(&e))?;
                        layout::enrich_layout_ctx_with_instantiations(&mut layout_ctx, programs);
                        let layout_result = layout::try_layout_with_codegen(
                            programs,
                            &layout_ctx,
                            &graph,
                            modules,
                            Some(&checked_image.renderer_configs),
                            Some(&pixels_programs),
                            false,
                        )
                        .map_err(|e| format!("error[build]: layout: {e}\n"))?;
                        let fallback_placement;
                        let (report_graph, placement) =
                            if let Some((_, _, generated_graph, generated_placement)) =
                                layout_result.as_ref()
                            {
                                (generated_graph, generated_placement)
                            } else {
                                fallback_placement =
                                    placement::place(&graph, modules, &layout_ctx, graph.cores)
                                        .map_err(|e| format!("error[build]: {e}\n"))?;
                                (&graph, &fallback_placement)
                            };
                        let mut enum_variants: BTreeMap<String, Vec<String>> = BTreeMap::new();
                        for (k, e) in program.enums.iter().chain(program.imported.enums.iter()) {
                            enum_variants
                                .entry(k.clone())
                                .or_insert_with(|| e.variants.clone());
                        }
                        match report::render(&inputs, &enum_variants, report_graph, placement) {
                            Ok(mut text) => {
                                if !pixels_programs.structural_programs.is_empty() {
                                    wrela_compiler::pixels::report::append_program_set(
                                        &mut text,
                                        &pixels_programs,
                                    )?;
                                }
                                let name = first_field_value(&text, "Name value=")
                                    .unwrap_or("")
                                    .to_string();
                                let target = first_field_value(&text, "Target value=")
                                    .unwrap_or("")
                                    .to_string();
                                let mut layout_types = Vec::new();
                                for (key, module) in modules {
                                    if key == rtconfig::MODULE_ADDR || key == "__image_runtime" {
                                        continue;
                                    }
                                    let specialized = sema::specialize::specialize(module)
                                        .map_err(|e| render_sema_error(&e))?;
                                    let mut layouts = sema::types::check_layouts(&specialized)
                                        .map_err(|e| render_sema_error(&e))?;
                                    if let Some(p) = programs.get(key) {
                                        sema::types::complete_layouts(
                                            &specialized,
                                            p,
                                            &mut layouts,
                                        )
                                        .map_err(|e| render_sema_error(&e))?;
                                    }
                                    layout_types.extend(layouts);
                                }
                                report::render_exact_bytes_section(&mut text, &layout_types)
                                    .map_err(|e| render_sema_error(&e))?;
                                let img = match layout_result {
                                    Some((image_layout, codegen, _, placement)) => {
                                        if let Some(ref tables) = image_layout.runtime {
                                            let rt_text = rtconfig::generate_and_typecheck(tables)
                                                .map_err(|e| {
                                                    if e.ends_with('\n') {
                                                        e
                                                    } else {
                                                        format!("{e}\n")
                                                    }
                                                })?;
                                            let digest = report::sha256_hex(rt_text.as_bytes());
                                            rtconfig::insert_generated_input_line(
                                                &mut text, &digest,
                                            );
                                        }
                                        layout::render_layout_section(&mut text, &image_layout);
                                        wrela_compiler::pixels::report::append_layout(
                                            &mut text,
                                            &pixels_programs,
                                            &image_layout,
                                            false,
                                        )?;
                                        let cost_source =
                                            file_paths.get(module.as_str()).map(|p| p.as_path());
                                        if let Some(linked) = image_layout.linked.as_ref() {
                                            report::append_linked_cost_summary(
                                                &mut text, linked, &placement, ghz,
                                            )
                                        } else {
                                            report::append_cost_summary(
                                                &mut text,
                                                &codegen,
                                                &placement,
                                                ghz,
                                                cost_source,
                                            )
                                        }
                                        .map_err(|e| {
                                            if e.ends_with('\n') {
                                                format!("error[build]: {e}")
                                            } else {
                                                format!("error[build]: {e}\n")
                                            }
                                        })?;
                                        report::append_convention_section(&mut text, &codegen);
                                        eval::layout_assert::run(program, &graph, &image_layout)?;
                                        Some(image_layout.blob)
                                    }
                                    None => {
                                        if !graph.layout_asserts.is_empty() {
                                            let names: Vec<&str> = graph
                                                .layout_asserts
                                                .iter()
                                                .map(|a| a.fn_key.as_str())
                                                .collect();
                                            return Err(format!(
                                                "error[build]: registered `@layout_assert` fn(s) \
                                                 ({}) require a laid-out image; this program's \
                                                 reachable surface did not fully lower\n",
                                                names.join(", ")
                                            ));
                                        }
                                        None
                                    }
                                };
                                Ok(BuildReport {
                                    devices: graph.devices.len(),
                                    drivers: graph.drivers.len(),
                                    actors: graph.actors.len(),
                                    pools: graph.pools.len(),
                                    text,
                                    name,
                                    target,
                                    img,
                                })
                            }
                            Err(e) => Err(format!("error[build]: {e}\n")),
                        }
                    }
                    Err(e) => Err(render_sema_error(&e)),
                },
                Err(e) => Err(render_sema_error(&eval::to_sema_error(e))),
            }
        }
        _ => {
            let names: Vec<String> = candidates
                .iter()
                .map(|(module, fn_name)| format!("{module}::{fn_name}"))
                .collect();
            Err(format!(
                "error[build]: more than one `@image` fn reachable in the build closure ({})\n",
                names.join(", ")
            ))
        }
    }
}

fn run_report_stage(
    programs: &BTreeMap<String, TypedProgram>,
    file_paths: &BTreeMap<String, std::path::PathBuf>,
    modules: &BTreeMap<String, Module>,
    ghz: f64,
) {
    match build_report(programs, file_paths, modules, ghz) {
        Ok(r) => print!("{}", r.text),
        Err(diag) => {
            eprint!("{diag}");
            note_dump_diagnostic();
        }
    }
}

fn run_rtconfig_stage(
    programs: &BTreeMap<String, TypedProgram>,
    modules: &BTreeMap<String, Module>,
) {
    match build_rtconfig(programs, modules) {
        Ok(text) => print!("{text}"),
        Err(diag) => {
            eprint!("{diag}");
            note_dump_diagnostic();
        }
    }
}

fn build_rtconfig(
    programs: &BTreeMap<String, TypedProgram>,
    modules: &BTreeMap<String, Module>,
) -> Result<String, String> {
    let candidates: Vec<(&String, &String)> = programs
        .iter()
        .filter_map(|(module, p)| p.image_fn.as_ref().map(|f| (module, f)))
        .collect();
    match candidates.len() {
        0 => Err("error[build]: no `@image` fn found in the build closure\n".to_string()),
        1 => {
            let (module, fn_name) = candidates[0];
            let program = &programs[module];
            let graph = eval::interp::eval_image(program, fn_name)
                .map_err(|e| render_sema_error(&eval::to_sema_error(e)))?;
            let checked_image = eval::image_checks::check_sealed(&graph, program, programs)
                .map_err(|e| render_sema_error(&e))?;
            let pixels_programs = wrela_compiler::pixels::compile_all(
                programs,
                module,
                &graph,
                &checked_image.renderer_configs,
            )
            .map_err(|error| render_sema_error(&eval::image_checks::pixels_error(error)))?;
            let mut layout_ctx =
                layout::merge_layout_ctx(modules).map_err(|e| render_sema_error(&e))?;
            layout::enrich_layout_ctx_with_instantiations(&mut layout_ctx, programs);
            match layout::try_layout_program(
                programs,
                &layout_ctx,
                &graph,
                modules,
                Some(&checked_image.renderer_configs),
                Some(&pixels_programs),
                false,
            ) {
                Ok(Some(image_layout)) => {
                    let Some(tables) = image_layout.runtime.as_ref() else {
                        return Err(
                            "error[build]: image has no runtime tables; nothing to generate for \
                             --stage=rtconfig\n"
                                .to_string(),
                        );
                    };
                    let text = rtconfig::generate_and_typecheck(tables).map_err(|e| {
                        if e.ends_with('\n') {
                            e
                        } else {
                            format!("{e}\n")
                        }
                    })?;
                    Ok(text)
                }
                Ok(None) => Err(
                    "error[build]: this program's reachable surface did not fully lower; \
                     --stage=rtconfig needs a laid-out image\n"
                        .to_string(),
                ),
                Err(e) => Err(format!("error[build]: layout: {e}\n")),
            }
        }
        _ => {
            let names: Vec<String> = candidates
                .iter()
                .map(|(module, fn_name)| format!("{module}::{fn_name}"))
                .collect();
            Err(format!(
                "error[build]: more than one `@image` fn reachable in the build closure ({})\n",
                names.join(", ")
            ))
        }
    }
}

fn load_build_closure(
    path: &str,
    module: Module,
) -> Result<
    (
        BTreeMap<String, TypedProgram>,
        BTreeMap<String, PathBuf>,
        BTreeMap<String, Module>,
    ),
    (),
> {
    let had_imports = !module.imports.is_empty();
    let checked = check_closure(path, module)?;
    let mut file_paths = BTreeMap::new();
    file_paths.insert(checked.root.clone(), Path::new(path).to_path_buf());
    if checked.programs.contains_key("core.runtime") {
        if let Ok((_, runtime_loaded)) = loader::load_runtime_module() {
            file_paths.insert("core.runtime".to_string(), runtime_loaded.file);
        }
    }
    if had_imports {
        if let Ok(loaded) = loader::load_closure(Path::new(path)) {
            for (k, m) in loaded.modules {
                file_paths.insert(k.join("."), m.file);
            }
        }
    }
    Ok((checked.programs, file_paths, checked.modules))
}

fn run_cfg_stage(checked: &CheckedClosure) -> Result<String, String> {
    let reachable =
        lower::guest_reachable_keys_closure(&checked.programs, &lower::LowerOpts::default());
    let opts = lower::LowerOpts {
        emit_comptime_tests: false,
        only: Some(reachable),
    };
    let mut mwir_programs = Vec::with_capacity(checked.programs.len());
    let mut flow_fns = BTreeMap::new();
    for typed in checked.programs.values() {
        mwir_programs.push(lower::lower_program_with(typed, &opts).map_err(|e| e.message.clone())?);
        flow_fns.extend(
            wrela_compiler::flowwir_lower::lower_program_with(typed, &opts)
                .map_err(|e| e.message.clone())?
                .fns,
        );
    }
    let mwir = layout::merge_mwir_programs(mwir_programs);
    let flow = wrela_compiler::flowwir::FlowWirProgram { fns: flow_fns };
    let mut out = wrela_compiler::liveness::dump_program(&mwir)?;
    out.push_str(&wrela_compiler::flow_liveness::dump_program(&flow)?);
    Ok(out)
}

fn run_frame_stage(checked: &CheckedClosure) -> Result<String, String> {
    let reachable =
        lower::guest_reachable_keys_closure(&checked.programs, &lower::LowerOpts::default());
    let opts = lower::LowerOpts {
        emit_comptime_tests: false,
        only: Some(reachable),
    };
    let mut flow_fns = BTreeMap::new();
    for typed in checked.programs.values() {
        flow_fns.extend(
            wrela_compiler::flowwir_lower::lower_program_with(typed, &opts)
                .map_err(|e| e.message.clone())?
                .fns,
        );
    }
    let flow = wrela_compiler::flowwir::FlowWirProgram { fns: flow_fns };
    let mut layout_ctx =
        layout::merge_layout_ctx(&checked.modules).map_err(|e| e.message.clone())?;
    layout::enrich_layout_ctx_with_instantiations(&mut layout_ctx, &checked.programs);
    wrela_compiler::frame_plan::dump_program(&flow, &layout_ctx)
}

fn run_mwir_opt_stage(checked: &CheckedClosure) -> Result<String, String> {
    let reachable =
        lower::guest_reachable_keys_closure(&checked.programs, &lower::LowerOpts::default());
    let opts = lower::LowerOpts {
        emit_comptime_tests: false,
        only: Some(reachable),
    };
    let mut mwir_programs = Vec::new();
    let mut flow_fns = BTreeMap::new();
    for typed in checked.programs.values() {
        mwir_programs.push(lower::lower_program_with(typed, &opts).map_err(|e| e.message.clone())?);
        flow_fns.extend(
            wrela_compiler::flowwir_lower::lower_program_with(typed, &opts)
                .map_err(|e| e.message.clone())?
                .fns,
        );
    }
    let mwir = layout::merge_mwir_programs(mwir_programs);
    let flow = wrela_compiler::flowwir::FlowWirProgram { fns: flow_fns };
    let mut layout_ctx =
        layout::merge_layout_ctx(&checked.modules).map_err(|e| e.message.clone())?;
    layout::enrich_layout_ctx_with_instantiations(&mut layout_ctx, &checked.programs);
    let mut out = wrela_compiler::sroa::dump_program(&mwir, &layout_ctx);
    out.push_str(&wrela_compiler::sroa::dump_flow_program(
        &flow,
        &layout_ctx,
    )?);
    for (key, f) in &mwir.fns {
        let analysis = wrela_compiler::range::analyze(f)?;
        out.push_str(&format!("  range function {key}\n"));
        out.push_str(&wrela_compiler::range::dump(f, &analysis));
    }
    out.push_str(&wrela_compiler::range::dump_flow_program(&flow)?);
    Ok(out)
}

fn dump(args: &[String]) -> ExitCode {
    DUMP_HAD_DIAGNOSTIC.with(|c| c.set(false));
    wrela_compiler::codegen::set_omit_dmb(false);
    wrela_compiler::codegen::set_block_count(false);
    wrela_compiler::codegen::set_block_bridge(true);

    let mut stage = None;
    let mut renderer_index = None;
    let mut path = None;
    let mut timings = false;
    let mut omit_dmb = false;
    let mut block_count = false;
    let mut mode = wrela_compiler::opts::CompileMode::Release;
    let mut ghz = wrela_compiler::cost::profile_ghz();
    for a in args {
        if let Some(s) = a.strip_prefix("--stage=") {
            stage = Some(s.to_string());
        } else if let Some(index) = a.strip_prefix("--renderer=") {
            if renderer_index.is_some() {
                eprintln!("error: --renderer may be specified only once");
                return ExitCode::FAILURE;
            }
            renderer_index = match parse_renderer_index(index) {
                Ok(index) => Some(index),
                Err(message) => {
                    eprintln!("error: {message}");
                    return ExitCode::FAILURE;
                }
            };
        } else if a == "--timings" {
            timings = true;
        } else if a == "--omit-dmb" {
            omit_dmb = true;
        } else if a == "--block-count" {
            block_count = true;
        } else if a == "--no-bounds-elide" {
            eprintln!("error: --no-bounds-elide was removed; use --mode=dev");
            return ExitCode::FAILURE;
        } else if let Some(m) = a.strip_prefix("--mode=") {
            mode = match m {
                "dev" => wrela_compiler::opts::CompileMode::Dev,
                "release" => wrela_compiler::opts::CompileMode::Release,
                _ => {
                    eprintln!("{USAGE}");
                    return ExitCode::FAILURE;
                }
            };
        } else if let Some(g) = a.strip_prefix("--ghz=") {
            match wrela_compiler::cost::parse_ghz(g) {
                Ok(v) => ghz = v,
                Err(_) => {
                    eprintln!("{USAGE}");
                    return ExitCode::FAILURE;
                }
            }
        } else if path.is_none() {
            path = Some(a.clone());
        } else {
            eprintln!("{USAGE}");
            return ExitCode::FAILURE;
        }
    }
    let (Some(stage), Some(path)) = (stage, path) else {
        eprintln!("{USAGE}");
        return ExitCode::FAILURE;
    };
    if let Err(message) = validate_renderer_stage(&stage, renderer_index) {
        eprintln!("error: {message}");
        return ExitCode::FAILURE;
    }
    wrela_compiler::codegen::set_omit_dmb(omit_dmb);
    wrela_compiler::codegen::set_block_count(block_count);
    wrela_compiler::opts::apply_mode(mode);

    let total_start = Instant::now();

    let read_start = Instant::now();
    let source = match std::fs::read_to_string(&path) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("error: cannot read {path}: {e}");
            return ExitCode::FAILURE;
        }
    };
    let read_time = read_start.elapsed();

    let lex_start = Instant::now();
    let lex_result = lexer::lex(&source);
    let lex_time = lex_start.elapsed();

    let mut parse_time = Duration::ZERO;
    let dump_time;

    match stage.as_str() {
        "tokens" => match lex_result {
            Ok(tokens) => {
                if timings {
                    let parse_start = Instant::now();
                    let _ = parser::parse_any(tokens.clone());
                    parse_time = parse_start.elapsed();
                }
                let dump_start = Instant::now();
                print!("{}", lexer::dump(&tokens));
                dump_time = dump_start.elapsed();
            }
            Err(e) => {
                let dump_start = Instant::now();
                print_lex_error(&e);
                dump_time = dump_start.elapsed();
            }
        },
        "ast" => match lex_result {
            Ok(tokens) => {
                let parse_start = Instant::now();
                let parsed = parser::parse(tokens);
                parse_time = parse_start.elapsed();
                let dump_start = Instant::now();
                match parsed {
                    Ok(module) => print!("{}", parser::dump(&module)),
                    Err(e) => print_parse_error(&e),
                }
                dump_time = dump_start.elapsed();
            }
            Err(e) => {
                let dump_start = Instant::now();
                print_lex_error(&e);
                dump_time = dump_start.elapsed();
            }
        },
        "pretty" => match lex_result {
            Ok(tokens) => {
                let parse_start = Instant::now();
                let parsed = parser::parse(tokens);
                parse_time = parse_start.elapsed();
                let dump_start = Instant::now();
                match parsed {
                    Ok(module) => print!("{}", printer::pretty(&module)),
                    Err(e) => print_parse_error(&e),
                }
                dump_time = dump_start.elapsed();
            }
            Err(e) => {
                let dump_start = Instant::now();
                print_lex_error(&e);
                dump_time = dump_start.elapsed();
            }
        },
        "check" => match lex_result {
            Ok(tokens) => {
                let parse_start = Instant::now();
                let parsed = parser::parse(tokens);
                parse_time = parse_start.elapsed();
                let dump_start = Instant::now();
                match parsed {
                    Ok(module) if module.imports.is_empty() => {
                        match sema::check_dump(&module, &path) {
                            Ok(text) => print!("{text}"),
                            Err(e) => print_sema_error(&e),
                        }
                    }
                    Ok(_) => match loader::load_closure(Path::new(&path)) {
                        Ok(program) => {
                            let paths: BTreeMap<Vec<String>, String> = program
                                .modules
                                .iter()
                                .map(|(k, m)| (k.clone(), m.file.display().to_string()))
                                .collect();
                            let modules: BTreeMap<Vec<String>, _> = program
                                .modules
                                .into_iter()
                                .map(|(k, m)| (k, m.module))
                                .collect();
                            match sema::check_program_dump(&modules, &paths) {
                                Ok(text) => print!("{text}"),
                                Err(e) => print_sema_error(&e),
                            }
                        }
                        Err(loader::LoadError::Lex(e)) => print_lex_error(&e),
                        Err(loader::LoadError::Parse(e)) => print_parse_error(&e),
                        Err(loader::LoadError::Build(e)) => print_sema_error(&e),
                    },
                    Err(e) => print_parse_error(&e),
                }
                dump_time = dump_start.elapsed();
            }
            Err(e) => {
                let dump_start = Instant::now();
                print_lex_error(&e);
                dump_time = dump_start.elapsed();
            }
        },
        "layout-types" => match lex_result {
            Ok(tokens) => {
                let parse_start = Instant::now();
                let parsed = parser::parse(tokens);
                parse_time = parse_start.elapsed();
                let dump_start = Instant::now();
                match parsed {
                    Ok(module) => match check_closure(&path, module) {
                        Ok(checked) => {
                            let programs: BTreeMap<String, &TypedProgram> = checked
                                .programs
                                .iter()
                                .map(|(k, p)| (k.clone(), p))
                                .collect();
                            let ordered: Vec<(String, Module)> =
                                checked.modules.into_iter().collect();
                            run_layout_types_stage(&ordered, &programs);
                        }
                        Err(()) => {}
                    },
                    Err(e) => print_parse_error(&e),
                }
                dump_time = dump_start.elapsed();
            }
            Err(e) => {
                let dump_start = Instant::now();
                print_lex_error(&e);
                dump_time = dump_start.elapsed();
            }
        },
        "typed" => match lex_result {
            Ok(tokens) => {
                let parse_start = Instant::now();
                let parsed = parser::parse(tokens);
                parse_time = parse_start.elapsed();
                let dump_start = Instant::now();
                match parsed {
                    Ok(module) => match check_closure(&path, module) {
                        Ok(checked) => {
                            let time_key: Vec<String> =
                                ["core", "time"].iter().map(|s| (*s).to_string()).collect();
                            let time_explicit = checked
                                .modules
                                .values()
                                .any(|m| m.imports.iter().any(|imp| imp.path == time_key));
                            let runtime_key: Vec<String> = ["core", "runtime"]
                                .iter()
                                .map(|s| (*s).to_string())
                                .collect();
                            let runtime_explicit = checked
                                .modules
                                .values()
                                .any(|m| m.imports.iter().any(|imp| imp.path == runtime_key));
                            let mut visible: Vec<(String, &TypedProgram)> = Vec::new();
                            for (addr, program) in &checked.programs {
                                let label = checked
                                    .modules
                                    .get(addr)
                                    .map(|m| m.path.join("."))
                                    .unwrap_or_else(|| addr.clone());
                                if label == "time" && !time_explicit {
                                    continue;
                                }
                                if label == "runtime" && !runtime_explicit {
                                    continue;
                                }
                                if label == "__image_runtime" || addr == "core.__image_runtime" {
                                    continue;
                                }
                                visible.push((label, program));
                            }
                            if visible.len() == 1 {
                                print!("{}", sema::dump_typed(visible[0].1));
                            } else {
                                let mut out = String::new();
                                for (label, program) in visible {
                                    out.push_str(&format!("Module path={label}\n"));
                                    out.push_str(&sema::dump_typed(program));
                                }
                                print!("{out}");
                            }
                        }
                        Err(()) => {}
                    },
                    Err(e) => print_parse_error(&e),
                }
                dump_time = dump_start.elapsed();
            }
            Err(e) => {
                let dump_start = Instant::now();
                print_lex_error(&e);
                dump_time = dump_start.elapsed();
            }
        },
        "relax" => {
            let dump_start = Instant::now();
            match wrela_compiler::cost::linked_shipped_program(Path::new(&path)) {
                Ok((linked, _, scope)) if scope == wrela_compiler::cost::TextScope::Image => {
                    match wrela_compiler::relax::relax_linked_immediates(&linked) {
                        Ok((linked, dump)) => {
                            match wrela_compiler::relax::relax_linked_addresses(&linked) {
                                Ok((_, address_dump)) => print!("{dump}{address_dump}"),
                                Err(e) => {
                                    print_line_diagnostic(&format!("error[unimplemented]: {e}"))
                                }
                            }
                        }
                        Err(e) => print_line_diagnostic(&format!("error[unimplemented]: {e}")),
                    }
                }
                Ok(_) => match wrela_compiler::cost::codegen_cost_stage(Path::new(&path)) {
                    Ok(program) => match wrela_compiler::relax::relax_immediates(&program) {
                        Ok(relaxed) => print!("{}", relaxed.dump),
                        Err(e) => print_line_diagnostic(&format!("error[unimplemented]: {e}")),
                    },
                    Err(e) => print_line_diagnostic(&format!("error[unimplemented]: {e}")),
                },
                Err(e) => print_line_diagnostic(&format!("error[unimplemented]: {e}")),
            }
            dump_time = dump_start.elapsed();
        }
        "cfg" => match lex_result {
            Ok(tokens) => {
                let parse_start = Instant::now();
                let parsed = parser::parse(tokens);
                parse_time = parse_start.elapsed();
                let dump_start = Instant::now();
                match parsed {
                    Ok(module) => match check_closure(&path, module) {
                        Ok(checked) => match run_cfg_stage(&checked) {
                            Ok(text) => print!("{text}"),
                            Err(e) => print_line_diagnostic(&format!("error[unimplemented]: {e}")),
                        },
                        Err(()) => {}
                    },
                    Err(e) => print_parse_error(&e),
                }
                dump_time = dump_start.elapsed();
            }
            Err(e) => {
                let dump_start = Instant::now();
                print_lex_error(&e);
                dump_time = dump_start.elapsed();
            }
        },
        "mwir-opt" => match lex_result {
            Ok(tokens) => {
                let parse_start = Instant::now();
                let parsed = parser::parse(tokens);
                parse_time = parse_start.elapsed();
                let dump_start = Instant::now();
                match parsed {
                    Ok(module) => match check_closure(&path, module) {
                        Ok(checked) => match run_mwir_opt_stage(&checked) {
                            Ok(text) => print!("{text}"),
                            Err(e) => print_line_diagnostic(&format!("error[unimplemented]: {e}")),
                        },
                        Err(()) => {}
                    },
                    Err(e) => print_parse_error(&e),
                }
                dump_time = dump_start.elapsed();
            }
            Err(e) => {
                let dump_start = Instant::now();
                print_lex_error(&e);
                dump_time = dump_start.elapsed();
            }
        },
        "frame" => match lex_result {
            Ok(tokens) => {
                let parse_start = Instant::now();
                let parsed = parser::parse(tokens);
                parse_time = parse_start.elapsed();
                let dump_start = Instant::now();
                match parsed {
                    Ok(module) => match check_closure(&path, module) {
                        Ok(checked) => match run_frame_stage(&checked) {
                            Ok(text) => print!("{text}"),
                            Err(e) => print_line_diagnostic(&format!("error[unimplemented]: {e}")),
                        },
                        Err(()) => {}
                    },
                    Err(e) => print_parse_error(&e),
                }
                dump_time = dump_start.elapsed();
            }
            Err(e) => {
                let dump_start = Instant::now();
                print_lex_error(&e);
                dump_time = dump_start.elapsed();
            }
        },
        "mwir" => match lex_result {
            Ok(tokens) => {
                let parse_start = Instant::now();
                let parsed = parser::parse(tokens);
                parse_time = parse_start.elapsed();
                let dump_start = Instant::now();
                match parsed {
                    Ok(module) => match check_closure(&path, module) {
                        Ok(checked) => {
                            let program = &checked.programs[&checked.root];
                            match wrela_compiler::lower::lower_program(program) {
                                Ok(mwir_program) => {
                                    print!("{}", wrela_compiler::mwir::dump(&mwir_program))
                                }
                                Err(e) => print_line_diagnostic(&format!(
                                    "error[unimplemented]: {}",
                                    e.message
                                )),
                            }
                        }
                        Err(()) => {}
                    },
                    Err(e) => print_parse_error(&e),
                }
                dump_time = dump_start.elapsed();
            }
            Err(e) => {
                let dump_start = Instant::now();
                print_lex_error(&e);
                dump_time = dump_start.elapsed();
            }
        },
        "flowwir" => match lex_result {
            Ok(tokens) => {
                let parse_start = Instant::now();
                let parsed = parser::parse(tokens);
                parse_time = parse_start.elapsed();
                let dump_start = Instant::now();
                match parsed {
                    Ok(module) => match check_closure(&path, module) {
                        Ok(checked) => {
                            let program = &checked.programs[&checked.root];
                            match wrela_compiler::flowwir_lower::lower_program(program) {
                                Ok(flowwir_program) => {
                                    print!("{}", wrela_compiler::flowwir::dump(&flowwir_program))
                                }
                                Err(e) => print_line_diagnostic(&format!(
                                    "error[unimplemented]: {}",
                                    e.message
                                )),
                            }
                        }
                        Err(()) => {}
                    },
                    Err(e) => print_parse_error(&e),
                }
                dump_time = dump_start.elapsed();
            }
            Err(e) => {
                let dump_start = Instant::now();
                print_lex_error(&e);
                dump_time = dump_start.elapsed();
            }
        },
        "asm" | "cost" => {
            let dump_cost = stage == "cost";
            match lex_result {
                Ok(tokens) => {
                    let parse_start = Instant::now();
                    let parsed = parser::parse(tokens);
                    parse_time = parse_start.elapsed();
                    let dump_start = Instant::now();
                    match parsed {
                        Ok(module) => match check_closure(&path, module) {
                            Ok(checked) => {
                                let reachable = lower::guest_reachable_keys_closure(
                                    &checked.programs,
                                    &lower::LowerOpts::default(),
                                );
                                let lower_opts = lower::LowerOpts {
                                    emit_comptime_tests: false,
                                    only: Some(reachable),
                                };
                                let mut mwir_programs = Vec::with_capacity(checked.programs.len());
                                let mut flow_fns = BTreeMap::new();
                                let mut lower_err: Option<String> = None;
                                for typed in checked.programs.values() {
                                    match lower::lower_program_with(typed, &lower_opts) {
                                        Ok(p) => mwir_programs.push(p),
                                        Err(e) => {
                                            lower_err = Some(e.message);
                                            break;
                                        }
                                    }
                                    match wrela_compiler::flowwir_lower::lower_program_with(
                                        typed,
                                        &lower_opts,
                                    ) {
                                        Ok(p) => flow_fns.extend(p.fns),
                                        Err(e) => {
                                            lower_err = Some(e.message);
                                            break;
                                        }
                                    }
                                }
                                if let Some(msg) = lower_err {
                                    print_line_diagnostic(&format!("error[unimplemented]: {msg}"));
                                } else {
                                    let mwir_program = layout::merge_mwir_programs(mwir_programs);
                                    let flow_program =
                                        wrela_compiler::flowwir::FlowWirProgram { fns: flow_fns };
                                    match layout::merge_layout_ctx(&checked.modules) {
                                        Ok(mut layout_ctx) => {
                                            layout::enrich_layout_ctx_with_instantiations(
                                                &mut layout_ctx,
                                                &checked.programs,
                                            );
                                            match layout::actor_method_index_tables(
                                                &checked.modules,
                                                &layout_ctx,
                                            ) {
                                                Ok(method_index) => {
                                                    let group_arena_capacity =
                                                        layout::count_with_group_sites(
                                                            &checked.modules,
                                                        );
                                                    let enqueue_specs = {
                                                        let root = &checked.programs[&checked.root];
                                                        match &root.image_fn {
                                                        Some(name) => {
                                                            match eval::interp::eval_image(
                                                                root, name,
                                                            ) {
                                                                Ok(graph) => layout::mailbox_enqueue_specs(
                                                                    &graph,
                                                                    &checked.modules,
                                                                    &layout_ctx,
                                                                )
                                                                .unwrap_or_default(),
                                                                Err(_) => Vec::new(),
                                                            }
                                                        }
                                                        None => Vec::new(),
                                                    }
                                                    };
                                                    match wrela_compiler::codegen::codegen_program_with_async(
                                                    &mwir_program,
                                                    &flow_program,
                                                    &layout_ctx,
                                                    &method_index,
                                                    group_arena_capacity,
                                                    &enqueue_specs,
                                                ) {
                                                    Ok(codegen_program) => {
                                                        if dump_cost {
                                                            let placement = {
                                                                let root = &checked.programs
                                                                    [&checked.root];
                                                                match &root.image_fn {
                                                                    Some(name) => {
                                                                        match eval::interp::eval_image(
                                                                            root, name,
                                                                        ) {
                                                                            Ok(graph) => {
                                                                                placement::place(
                                                                                    &graph,
                                                                                    &checked.modules,
                                                                                    &layout_ctx,
                                                                                    graph.cores,
                                                                                )
                                                                                .unwrap_or_else(
                                                                                    |_| {
                                                                                        placement::PlacementTable {
                                                                                            entries: Vec::new(),
                                                                                            cores: 0,
                                                                                        }
                                                                                    },
                                                                                )
                                                                            }
                                                                            Err(_) => {
                                                                                placement::PlacementTable {
                                                                                    entries: Vec::new(),
                                                                                    cores: 0,
                                                                                }
                                                                            }
                                                                        }
                                                                    }
                                                                    None => {
                                                                        placement::PlacementTable {
                                                                            entries: Vec::new(),
                                                                            cores: 0,
                                                                        }
                                                                    }
                                                                }
                                                            };
                                                            if checked.programs[&checked.root]
                                                                .image_fn
                                                                .is_some()
                                                            {
                                                                match wrela_compiler::cost::linked_shipped_program(
                                                                    Path::new(&path),
                                                                ) {
                                                                    Ok((linked, linked_place, _)) => {
                                                                        match wrela_compiler::cost::load_default()
                                                                        {
                                                                            Ok(table) => match wrela_compiler::cost::dump_linked_for_source(
                                                                                &linked,
                                                                                &table,
                                                                                &linked_place,
                                                                                ghz,
                                                                                Some(Path::new(&path)),
                                                                            ) {
                                                                                Ok(text) => print!("{text}"),
                                                                                Err(e) => print_line_diagnostic(&format!("error[unimplemented]: {e}")),
                                                                            },
                                                                            Err(e) => print_line_diagnostic(&format!("error[unimplemented]: {e}")),
                                                                        }
                                                                    }
                                                                    Err(e) => print_line_diagnostic(&format!("error[unimplemented]: {e}")),
                                                                }
                                                            } else {
                                                                match wrela_compiler::cost::load_default()
                                                                {
                                                                    Ok(table) => {
                                                                        match wrela_compiler::cost::dump_for_source(
                                                                            &codegen_program,
                                                                            &table,
                                                                            &placement,
                                                                            ghz,
                                                                            Some(Path::new(&path)),
                                                                        ) {
                                                                            Ok(text) => print!("{text}"),
                                                                            Err(e) => print_line_diagnostic(&format!("error[unimplemented]: {e}")),
                                                                        }
                                                                    }
                                                                    Err(e) => print_line_diagnostic(&format!("error[unimplemented]: {e}")),
                                                                }
                                                            }
                                                        } else {
                                                            print!(
                                                                "{}",
                                                                wrela_compiler::codegen::dump(
                                                                    &codegen_program
                                                                )
                                                            );
                                                        }
                                                    }
                                                    Err(e) => print_line_diagnostic(&format!(
                                                        "error[unimplemented]: {}",
                                                        e.message
                                                    )),
                                                }
                                                }
                                                Err(e) => print_line_diagnostic(&format!(
                                                    "error[unimplemented]: {}",
                                                    e.message
                                                )),
                                            }
                                        }
                                        Err(e) => print_sema_error(&e),
                                    }
                                }
                            }
                            Err(()) => {}
                        },
                        Err(e) => print_parse_error(&e),
                    }
                    dump_time = dump_start.elapsed();
                }
                Err(e) => {
                    let dump_start = Instant::now();
                    print_lex_error(&e);
                    dump_time = dump_start.elapsed();
                }
            }
        }
        "field-graph" | "frame-program" | "render-layout" => match lex_result {
            Ok(tokens) => {
                let parse_start = Instant::now();
                let parsed = parser::parse(tokens);
                parse_time = parse_start.elapsed();
                let dump_start = Instant::now();
                match parsed {
                    Ok(module) => match load_build_closure(&path, module) {
                        Ok((programs, _, modules)) => run_pixels_stage(
                            &programs,
                            &modules,
                            pixels_dump_stage(&stage)
                                .expect("Pixels match arm has a canonical dump stage"),
                            renderer_index,
                        ),
                        Err(()) => {}
                    },
                    Err(e) => print_parse_error(&e),
                }
                dump_time = dump_start.elapsed();
            }
            Err(e) => {
                let dump_start = Instant::now();
                print_lex_error(&e);
                dump_time = dump_start.elapsed();
            }
        },
        "image" => match lex_result {
            Ok(tokens) => {
                let parse_start = Instant::now();
                let parsed = parser::parse(tokens);
                parse_time = parse_start.elapsed();
                let dump_start = Instant::now();
                match parsed {
                    Ok(module) => match load_build_closure(&path, module) {
                        Ok((programs, _, _)) => run_image_stage(&programs),
                        Err(()) => {}
                    },
                    Err(e) => print_parse_error(&e),
                }
                dump_time = dump_start.elapsed();
            }
            Err(e) => {
                let dump_start = Instant::now();
                print_lex_error(&e);
                dump_time = dump_start.elapsed();
            }
        },
        "report" => match lex_result {
            Ok(tokens) => {
                let parse_start = Instant::now();
                let parsed = parser::parse(tokens);
                parse_time = parse_start.elapsed();
                let dump_start = Instant::now();
                match parsed {
                    Ok(module) => match load_build_closure(&path, module) {
                        Ok((programs, file_paths, modules_by_addr)) => {
                            run_report_stage(&programs, &file_paths, &modules_by_addr, ghz);
                        }
                        Err(()) => {}
                    },
                    Err(e) => print_parse_error(&e),
                }
                dump_time = dump_start.elapsed();
            }
            Err(e) => {
                let dump_start = Instant::now();
                print_lex_error(&e);
                dump_time = dump_start.elapsed();
            }
        },
        "rtconfig" => match lex_result {
            Ok(tokens) => {
                let parse_start = Instant::now();
                let parsed = parser::parse(tokens);
                parse_time = parse_start.elapsed();
                let dump_start = Instant::now();
                match parsed {
                    Ok(module) => match load_build_closure(&path, module) {
                        Ok((programs, _, modules_by_addr)) => {
                            run_rtconfig_stage(&programs, &modules_by_addr);
                        }
                        Err(()) => {}
                    },
                    Err(e) => print_parse_error(&e),
                }
                dump_time = dump_start.elapsed();
            }
            Err(e) => {
                let dump_start = Instant::now();
                print_lex_error(&e);
                dump_time = dump_start.elapsed();
            }
        },
        other => {
            let dump_start = Instant::now();
            print_line_diagnostic(&format!(
                "error[unimplemented]: stage `{other}` is not implemented"
            ));
            dump_time = dump_start.elapsed();
        }
    }

    let total_time = total_start.elapsed();

    if timings {
        eprintln!(
            "timings: read={}us lex={}us parse={}us dump={}us total={}us",
            read_time.as_micros(),
            lex_time.as_micros(),
            parse_time.as_micros(),
            dump_time.as_micros(),
            total_time.as_micros(),
        );
    }

    if DUMP_HAD_DIAGNOSTIC.with(|c| c.get()) {
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    }
}

fn parse_summary_line(line: &str) -> Option<(usize, usize)> {
    let rest = line.strip_suffix(" failed")?;
    let (p, f) = rest.split_once(" passed, ")?;
    Some((p.parse().ok()?, f.parse().ok()?))
}

fn find_vmm_binary(explicit: Option<&str>) -> Option<PathBuf> {
    if let Some(p) = explicit {
        let pb = PathBuf::from(p);
        return if pb.is_file() { Some(pb) } else { None };
    }
    let exe = std::env::current_exe().ok()?;
    let dir = exe.parent()?;
    let candidate = dir.join("wrela-vmm");
    if candidate.is_file() {
        Some(candidate)
    } else {
        None
    }
}

struct VmmSlot {
    _file: std::fs::File,
}

impl VmmSlot {
    fn acquire() -> Result<Option<Self>, String> {
        let Some(raw_dir) = std::env::var_os("WRELA_VMM_SLOT_DIR") else {
            return Ok(None);
        };
        let dir = PathBuf::from(raw_dir);
        let mut slots = std::fs::read_dir(&dir)
            .map_err(|error| format!("read VMM slot directory {}: {error}", dir.display()))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| format!("read VMM slot entry: {error}"))?;
        slots.sort_by_key(|entry| entry.file_name());
        if slots.is_empty() {
            return Err(format!("VMM slot directory {} is empty", dir.display()));
        }
        loop {
            for entry in &slots {
                let file = OpenOptions::new()
                    .read(true)
                    .write(true)
                    .open(entry.path())
                    .map_err(|error| {
                        format!("open VMM slot {}: {error}", entry.path().display())
                    })?;
                match file.try_lock() {
                    Ok(()) => return Ok(Some(Self { _file: file })),
                    Err(std::fs::TryLockError::WouldBlock) => {}
                    Err(std::fs::TryLockError::Error(error)) => {
                        return Err(format!("lock VMM slot {}: {error}", entry.path().display()));
                    }
                }
            }
            std::thread::sleep(Duration::from_millis(5));
        }
    }
}

fn test_cmd(args: &[String]) -> ExitCode {
    wrela_compiler::codegen::set_omit_dmb(false);
    wrela_compiler::codegen::set_block_count(false);
    wrela_compiler::codegen::set_block_bridge(true);

    let mut path: Option<String> = None;
    let mut vmm_arg: Option<String> = None;
    let mut omit_dmb = false;
    let mut block_count = false;
    let mut pixels_telemetry = false;
    let mut mode = wrela_compiler::opts::CompileMode::Release;
    let mut _ghz = wrela_compiler::cost::profile_ghz();
    let mut i = 0;
    while i < args.len() {
        if args[i] == "--vmm" {
            i += 1;
            match args.get(i) {
                Some(p) => vmm_arg = Some(p.clone()),
                None => {
                    eprintln!("{USAGE}");
                    return ExitCode::FAILURE;
                }
            }
        } else if args[i] == "--omit-dmb" {
            omit_dmb = true;
        } else if args[i] == "--block-count" {
            block_count = true;
        } else if args[i] == "--pixels-telemetry" {
            pixels_telemetry = true;
        } else if args[i] == "--no-bounds-elide" {
            eprintln!("error: --no-bounds-elide was removed; use --mode=dev");
            return ExitCode::FAILURE;
        } else if let Some(m) = args[i].strip_prefix("--mode=") {
            mode = match m {
                "dev" => wrela_compiler::opts::CompileMode::Dev,
                "release" => wrela_compiler::opts::CompileMode::Release,
                _ => {
                    eprintln!("{USAGE}");
                    return ExitCode::FAILURE;
                }
            };
        } else if let Some(g) = args[i].strip_prefix("--ghz=") {
            match wrela_compiler::cost::parse_ghz(g) {
                Ok(v) => _ghz = v,
                Err(_) => {
                    eprintln!("{USAGE}");
                    return ExitCode::FAILURE;
                }
            }
        } else if path.is_none() {
            path = Some(args[i].clone());
        } else {
            eprintln!("{USAGE}");
            return ExitCode::FAILURE;
        }
        i += 1;
    }
    let Some(path) = path else {
        eprintln!("{USAGE}");
        return ExitCode::FAILURE;
    };
    wrela_compiler::codegen::set_omit_dmb(omit_dmb);
    wrela_compiler::codegen::set_block_count(block_count);
    wrela_compiler::opts::apply_mode(mode);

    let source = match std::fs::read_to_string(&path) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("error: cannot read {path}: {e}");
            return ExitCode::FAILURE;
        }
    };
    let tokens = match lexer::lex(&source) {
        Ok(t) => t,
        Err(e) => {
            print_lex_error(&e);
            return ExitCode::FAILURE;
        }
    };
    let module = match parser::parse(tokens) {
        Ok(m) => m,
        Err(e) => {
            print_parse_error(&e);
            return ExitCode::FAILURE;
        }
    };
    let checked = match check_closure(&path, module) {
        Ok(c) => c,
        Err(()) => return ExitCode::FAILURE,
    };
    let program = checked.programs[&checked.root].clone();
    let modules = checked.modules;

    let (comptime_report, _) = eval::run_tests(&program);
    let runtime_tests: Vec<String> = program
        .tests
        .iter()
        .filter(|t| t.kind == TestKind::Runtime)
        .map(|t| t.name.clone())
        .collect();

    if runtime_tests.is_empty() {
        print!("{comptime_report}");
        let any_failed = comptime_report
            .lines()
            .next_back()
            .and_then(parse_summary_line)
            .is_some_and(|(_, f)| f > 0);
        return if any_failed {
            ExitCode::FAILURE
        } else {
            ExitCode::SUCCESS
        };
    }

    let mut lines: Vec<&str> = comptime_report.lines().collect();
    let summary_line = lines.pop().unwrap_or("");
    let (total_passed, total_failed) = parse_summary_line(summary_line).unwrap_or((0, 0));
    let comptime_passed = total_passed;
    let comptime_failed = total_failed.saturating_sub(runtime_tests.len());
    let placeholder_lines: std::collections::BTreeSet<String> = runtime_tests
        .iter()
        .map(|name| {
            format!(
                "test {name}: FAILED `@test(runtime)` is not run yet (M5: generated image tests)"
            )
        })
        .collect();
    let comptime_lines: Vec<&str> = lines
        .into_iter()
        .filter(|l| !placeholder_lines.contains(*l))
        .collect();

    let mut layout_ctx = match layout::merge_layout_ctx(&modules) {
        Ok(c) => c,
        Err(e) => {
            print_sema_error(&e);
            return ExitCode::FAILURE;
        }
    };
    layout::enrich_layout_ctx_with_instantiations(&mut layout_ctx, &checked.programs);
    let layout_ctx = layout_ctx;
    let graph = match &program.image_fn {
        Some(fn_name) => match eval::interp::eval_image(&program, fn_name) {
            Ok(g) => g,
            Err(e) => {
                print_sema_error(&eval::to_sema_error(e));
                return ExitCode::FAILURE;
            }
        },
        None => eval::image::ImageGraph::default(),
    };
    let mut pixels_programs = None;
    let checked_image = if program.image_fn.is_some() {
        match eval::image_checks::check_sealed(&graph, &program, &checked.programs) {
            Ok(checked_image) => {
                match wrela_compiler::pixels::compile_all(
                    &checked.programs,
                    &program.module_path,
                    &graph,
                    &checked_image.renderer_configs,
                ) {
                    Ok(programs) => pixels_programs = Some(programs),
                    Err(error) => {
                        print_sema_error(&eval::image_checks::pixels_error(error));
                        return ExitCode::FAILURE;
                    }
                }
                Some(checked_image)
            }
            Err(e) => {
                print_sema_error(&e);
                return ExitCode::FAILURE;
            }
        }
    } else {
        None
    };
    let async_tests: std::collections::BTreeSet<String> = runtime_tests
        .iter()
        .filter(|name| program.fns.get(*name).is_some_and(|f| f.is_async))
        .cloned()
        .collect();
    let compiled = match layout::lower_and_codegen_image(
        &modules,
        &checked.programs,
        &layout_ctx,
        &graph,
        checked_image
            .as_ref()
            .map(|checked| &checked.renderer_configs),
        pixels_programs.as_ref(),
        // `--pixels-telemetry` selects the instrumented renderer layout; a
        // plain test run gets the production layout so goldens pin the
        // uninstrumented image and telemetry can never leak into them.
        pixels_telemetry,
        &runtime_tests,
        &async_tests,
        false,
    ) {
        Ok(c) => c,
        Err(e) => {
            for l in &comptime_lines {
                println!("{l}");
            }
            print_line_diagnostic(&format!(
                "error[unimplemented]: the runtime test tier could not compile this program: {e}"
            ));
            return ExitCode::FAILURE;
        }
    };
    let test_args =
        match layout::resolve_runtime_test_args(&program, &runtime_tests, &compiled.graph) {
            Ok(a) => a,
            Err(msg) => {
                for l in &comptime_lines {
                    println!("{l}");
                }
                print_line_diagnostic(&format!("error[build]: {msg}"));
                return ExitCode::FAILURE;
            }
        };
    let boot = layout::BootCtx {
        graph: &compiled.graph,
        modules: &compiled.modules,
        programs: &compiled.programs,
        layout_ctx: &compiled.layout_ctx,
        async_frames: &compiled.async_frames,
        group_child_index: &compiled.group_child_index,
        flow: &compiled.flow,
    };
    let mut image_layout = match layout::layout_test_image(
        &compiled.program,
        &runtime_tests,
        &async_tests,
        Some(boot),
        &test_args,
    ) {
        Ok(l) => l,
        Err(e) => {
            for l in &comptime_lines {
                println!("{l}");
            }
            print_line_diagnostic(&format!(
                "error[build]: the runtime test tier could not lay out the test image: {}",
                e.message
            ));
            return ExitCode::FAILURE;
        }
    };
    if let Some(programs) = pixels_programs.as_ref()
        && let Err(error) =
            layout::attach_pixels(&mut image_layout, &programs.compiled_renderers, true)
    {
        print_line_diagnostic(&format!("error[build]: {}", error.message));
        return ExitCode::FAILURE;
    }
    {
        if let Err(e) = layout::attach_blk_report(&mut image_layout, &graph, &compiled.programs) {
            for l in &comptime_lines {
                println!("{l}");
            }
            print_line_diagnostic(&format!("error[build]: {}", e.message));
            return ExitCode::FAILURE;
        }
    }

    if block_count {
        eprintln!(
            "test: block-count ids={} pool={}",
            wrela_compiler::codegen::block_ids_assigned(),
            wrela_compiler::rtconfig::BLOCK_POOL_COUNT
        );
    }

    let Some(vmm_path) = find_vmm_binary(vmm_arg.as_deref()) else {
        for l in &comptime_lines {
            println!("{l}");
        }
        print_line_diagnostic(&format!(
            "error[unimplemented]: the runtime test tier needs the wrela VMM (macOS/HVF at M5)"
        ));
        return ExitCode::FAILURE;
    };

    let tmp_dir = std::env::temp_dir().join(format!(
        "wrela-test-{}-{}",
        std::process::id(),
        report::sha256_hex(path.as_bytes())
    ));
    if let Err(e) = std::fs::create_dir_all(&tmp_dir) {
        eprintln!("error: cannot create {}: {e}", tmp_dir.display());
        return ExitCode::FAILURE;
    }
    let img_path = tmp_dir.join("test.img");
    let report_path = tmp_dir.join("test.report.txt");
    if let Err(e) = std::fs::write(&img_path, &image_layout.blob) {
        eprintln!("error: cannot write {}: {e}", img_path.display());
        return ExitCode::FAILURE;
    }
    let source_digest = report::sha256_hex(source.as_bytes());
    let image_digest = report::sha256_hex(&image_layout.blob);
    let mut parsed = layout::parsed_runtime_tail(&image_layout);
    parsed.entry = image_layout.entry;
    parsed.image_sha256 = image_digest;
    parsed.input_digests = vec![(path.clone(), source_digest)];
    parsed.exec_sections = image_layout
        .sections
        .iter()
        .filter(|section| {
            matches!(
                section.name,
                "entry" | "code" | "abort" | "checkpoint" | "rtcode"
            )
        })
        .map(|s| wrela_machine::report::ReportSection {
            name: s.name.to_string(),
            base: s.base,
            size: s.size,
        })
        .collect();
    let mut report_text = wrela_machine::report::render(&parsed);
    layout::append_ring_vmm_lines(&mut report_text, &image_layout);
    if let Err(e) = std::fs::write(&report_path, &report_text) {
        eprintln!("error: cannot write {}: {e}", report_path.display());
        return ExitCode::FAILURE;
    }
    let _vmm_slot = match VmmSlot::acquire() {
        Ok(slot) => slot,
        Err(error) => {
            let _ = std::fs::remove_dir_all(&tmp_dir);
            for line in &comptime_lines {
                println!("{line}");
            }
            print_line_diagnostic(&format!(
                "error[build]: could not acquire VMM slot: {error}"
            ));
            return ExitCode::FAILURE;
        }
    };
    let out = Command::new(&vmm_path)
        .arg(&report_path)
        .arg(&img_path)
        .output();
    let _ = std::fs::remove_dir_all(&tmp_dir);

    let out = match out {
        Ok(o) => o,
        Err(e) => {
            for l in &comptime_lines {
                println!("{l}");
            }
            print_line_diagnostic(&format!(
                "error[build]: could not run the wrela VMM ({}): {e}",
                vmm_path.display()
            ));
            return ExitCode::FAILURE;
        }
    };
    match out.status.code() {
        Some(0) | Some(1) => {}
        _ => {
            for l in &comptime_lines {
                println!("{l}");
            }
            print_line_diagnostic(&format!(
                "error[build]: the wrela VMM did not boot the test image: {}",
                String::from_utf8_lossy(&out.stderr).trim()
            ));
            return ExitCode::FAILURE;
        }
    }

    let transcript = String::from_utf8_lossy(&out.stdout).into_owned();
    let t_lines: Vec<&str> = transcript.lines().collect();
    let protocol_lines: Vec<&str> = t_lines
        .iter()
        .copied()
        .filter(|line| !line.starts_with("p7 "))
        .collect();
    let observations_well_formed = t_lines.iter().all(|line| {
        !line.starts_with("p7 ")
            || line
                .strip_prefix("p7 ")
                .is_some_and(|value| value.len() == 16 && u64::from_str_radix(value, 16).is_ok())
    });
    let trailing_ok = |lines: &[&str]| {
        lines.iter().all(|l| {
            l.starts_with("lane1 ") || l.starts_with("lane2 ") || l.starts_with("display ")
        })
    };
    let boot_failed = protocol_lines.len() >= 2
        && protocol_lines[0].starts_with("FAILED ")
        && parse_summary_line(protocol_lines[1]).is_some()
        && trailing_ok(&protocol_lines[2..]);
    let summary_idx = if !observations_well_formed {
        None
    } else if boot_failed {
        Some(1usize)
    } else if protocol_lines.len() >= runtime_tests.len() + 1
        && protocol_lines
            .iter()
            .zip(runtime_tests.iter())
            .all(|(line, name)| line.starts_with(&format!("test {name}: ")))
        && parse_summary_line(protocol_lines[runtime_tests.len()]).is_some()
        && trailing_ok(&protocol_lines[runtime_tests.len() + 1..])
    {
        Some(runtime_tests.len())
    } else {
        None
    };
    let Some(summary_i) = summary_idx else {
        for l in &comptime_lines {
            println!("{l}");
        }
        print_line_diagnostic(&format!(
            "error[build]: the wrela VMM's own transcript is not well-formed (expected {} test line(s) then a summary):\n{transcript}",
            runtime_tests.len()
        ));
        return ExitCode::FAILURE;
    };
    let Some((runtime_passed, runtime_failed)) = parse_summary_line(protocol_lines[summary_i])
    else {
        for l in &comptime_lines {
            println!("{l}");
        }
        print_line_diagnostic(&format!(
            "error[build]: the wrela VMM's own transcript is not well-formed (expected {} test line(s) then a summary):\n{transcript}",
            runtime_tests.len()
        ));
        return ExitCode::FAILURE;
    };

    for l in &comptime_lines {
        println!("{l}");
    }
    for line in &t_lines {
        if *line == protocol_lines[summary_i] || trailing_ok(&[*line]) {
            continue;
        }
        println!("{line}");
    }
    let passed = comptime_passed + runtime_passed;
    let failed = comptime_failed + runtime_failed;
    println!("{passed} passed, {failed} failed");
    for l in &protocol_lines[summary_i + 1..] {
        println!("{l}");
    }
    if failed > 0 {
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    }
}

fn build_cmd(args: &[String]) -> ExitCode {
    wrela_compiler::codegen::set_omit_dmb(false);
    wrela_compiler::codegen::set_block_count(false);
    wrela_compiler::codegen::set_block_bridge(true);

    let mut path = None;
    let mut out_dir: Option<String> = None;
    let mut omit_dmb = false;
    let mut block_count = false;
    let mut mode = wrela_compiler::opts::CompileMode::Release;
    let mut ghz = wrela_compiler::cost::profile_ghz();
    let mut i = 0;
    while i < args.len() {
        let a = &args[i];
        if a == "--out-dir" {
            i += 1;
            match args.get(i) {
                Some(d) => out_dir = Some(d.clone()),
                None => {
                    eprintln!("{USAGE}");
                    return ExitCode::FAILURE;
                }
            }
        } else if a == "--omit-dmb" {
            omit_dmb = true;
        } else if a == "--block-count" {
            block_count = true;
        } else if a == "--no-bounds-elide" {
            eprintln!("error: --no-bounds-elide was removed; use --mode=dev");
            return ExitCode::FAILURE;
        } else if let Some(m) = a.strip_prefix("--mode=") {
            mode = match m {
                "dev" => wrela_compiler::opts::CompileMode::Dev,
                "release" => wrela_compiler::opts::CompileMode::Release,
                _ => {
                    eprintln!("{USAGE}");
                    return ExitCode::FAILURE;
                }
            };
        } else if let Some(g) = a.strip_prefix("--ghz=") {
            match wrela_compiler::cost::parse_ghz(g) {
                Ok(v) => ghz = v,
                Err(_) => {
                    eprintln!("{USAGE}");
                    return ExitCode::FAILURE;
                }
            }
        } else if path.is_none() {
            path = Some(a.clone());
        } else {
            eprintln!("{USAGE}");
            return ExitCode::FAILURE;
        }
        i += 1;
    }
    let Some(path) = path else {
        eprintln!("{USAGE}");
        return ExitCode::FAILURE;
    };
    wrela_compiler::codegen::set_omit_dmb(omit_dmb);
    wrela_compiler::codegen::set_block_count(block_count);
    wrela_compiler::opts::apply_mode(mode);

    let source = match std::fs::read_to_string(&path) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("error: cannot read {path}: {e}");
            return ExitCode::FAILURE;
        }
    };
    let tokens = match lexer::lex(&source) {
        Ok(t) => t,
        Err(e) => {
            print_lex_error(&e);
            return ExitCode::FAILURE;
        }
    };
    let module = match parser::parse(tokens) {
        Ok(m) => m,
        Err(e) => {
            print_parse_error(&e);
            return ExitCode::FAILURE;
        }
    };

    let (programs, file_paths, modules_by_addr) = match load_build_closure(&path, module) {
        Ok(v) => v,
        Err(()) => return ExitCode::FAILURE,
    };

    let r = match build_report(&programs, &file_paths, &modules_by_addr, ghz) {
        Ok(r) => r,
        Err(diag) => {
            eprint!("{diag}");
            return ExitCode::FAILURE;
        }
    };

    let dir_str: String = match &out_dir {
        Some(d) => d.clone(),
        None => Path::new(&path)
            .parent()
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_default(),
    };
    let report_file_name = format!("{}.report.txt", r.name);
    let report_path_str = if dir_str.is_empty() {
        report_file_name
    } else {
        format!("{}/{report_file_name}", dir_str.trim_end_matches('/'))
    };
    let report_path = PathBuf::from(&report_path_str);
    if let Some(parent) = report_path.parent() {
        if !parent.as_os_str().is_empty() {
            if let Err(e) = std::fs::create_dir_all(parent) {
                eprintln!("error: cannot create directory {}: {e}", parent.display());
                return ExitCode::FAILURE;
            }
        }
    }
    if let Err(e) = std::fs::write(&report_path, &r.text) {
        eprintln!("error: cannot write {}: {e}", report_path.display());
        return ExitCode::FAILURE;
    }

    if let Some(img) = &r.img {
        let img_path_str = if dir_str.is_empty() {
            format!("{}.img", r.name)
        } else {
            format!("{}/{}.img", dir_str.trim_end_matches('/'), r.name)
        };
        if let Err(e) = std::fs::write(&img_path_str, img) {
            eprintln!("error: cannot write {img_path_str}: {e}");
            return ExitCode::FAILURE;
        }
    }

    if block_count {
        eprintln!(
            "build: block-count ids={} pool={}",
            wrela_compiler::codegen::block_ids_assigned(),
            wrela_compiler::rtconfig::BLOCK_POOL_COUNT
        );
    }
    println!("build: name={} target={}", r.name, r.target);
    println!(
        "build: devices={} drivers={} actors={} pools={}",
        r.devices, r.drivers, r.actors, r.pools
    );
    println!("build: report written to {report_path_str}");
    ExitCode::SUCCESS
}

#[cfg(test)]
mod tests {
    use super::*;
    use wrela_compiler::pixels::PixelsDumpStage;

    #[test]
    fn pixels_dump_stage_names_are_canonical() {
        assert_eq!(
            pixels_dump_stage("field-graph"),
            Some(PixelsDumpStage::FieldGraph)
        );
        assert_eq!(
            pixels_dump_stage("frame-program"),
            Some(PixelsDumpStage::FrameProgram)
        );
        assert_eq!(
            pixels_dump_stage("render-layout"),
            Some(PixelsDumpStage::RenderLayout)
        );
        assert_eq!(pixels_dump_stage("pixels-hidden"), None);
        for stage in ["field-graph", "frame-program", "render-layout"] {
            assert!(USAGE.contains(stage));
        }
    }

    #[test]
    fn renderer_selector_is_numeric_and_pixels_only() {
        assert_eq!(parse_renderer_index("0"), Ok(0));
        assert!(parse_renderer_index("").is_err());
        assert!(parse_renderer_index("-1").is_err());
        assert!(parse_renderer_index("one").is_err());
        assert!(validate_renderer_stage("field-graph", Some(0)).is_ok());
        assert!(validate_renderer_stage("frame-program", Some(0)).is_ok());
        assert!(validate_renderer_stage("render-layout", Some(0)).is_ok());
        assert!(validate_renderer_stage("typed", Some(0)).is_err());
        assert!(validate_renderer_stage("typed", None).is_ok());
    }

    #[test]
    fn pixels_renderer_selection_is_explicit_when_ambiguous() {
        assert_eq!(select_renderer_index(1, None), Ok(0));
        assert_eq!(select_renderer_index(2, Some(1)), Ok(1));
        assert!(
            select_renderer_index(2, None)
                .unwrap_err()
                .contains("--renderer=<index>")
        );
        assert!(
            select_renderer_index(2, Some(2))
                .unwrap_err()
                .contains("out of range")
        );
    }
}
