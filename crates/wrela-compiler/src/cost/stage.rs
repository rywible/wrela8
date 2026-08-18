use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use crate::codegen::{self, CodegenProgram};
use crate::flowwir;
use crate::flowwir_lower;
use crate::loader::{self, LoadError};
use crate::lower::{self, LowerOpts};
use crate::rtconfig;
use crate::sema::typed::TypedProgram;
use crate::sema::{self, SemaError};
use crate::syntax::ast::Module;
use crate::syntax::{lexer, parser};

use super::score::CostReport;
use super::table::load_default;

pub struct CostStageClosure {
    pub root: String,
    pub programs: BTreeMap<String, TypedProgram>,
    pub modules: BTreeMap<String, Module>,
}

fn generated_internal_source_keys(paths: &BTreeMap<Vec<String>, String>) -> BTreeSet<Vec<String>> {
    paths
        .iter()
        .filter_map(|(key, path)| {
            matches!(
                path.as_str(),
                rtconfig::GENERATED_INPUT_PATH
                    | loader::GENERATED_PIXELS_INPUT_PATH
                    | loader::GENERATED_PIXELS_STUB_INPUT_PATH
            )
            .then(|| key.clone())
        })
        .collect()
}

fn load_err(e: LoadError) -> String {
    match e {
        LoadError::Lex(e) => format!("lex: {e:?}"),
        LoadError::Parse(e) => format!("parse: {e:?}"),
        LoadError::Build(e) => format!("build: {}", e.message),
    }
}

fn sema_err(e: SemaError) -> String {
    format!("sema: {}", e.message)
}

pub fn load_cost_stage_closure(path: &Path) -> Result<CostStageClosure, String> {
    load_cost_stage_closure_impl(path, false)
}

fn load_cost_stage_closure_impl(
    path: &Path,
    trust_census_stdlib_snapshot: bool,
) -> Result<CostStageClosure, String> {
    let path_str = path.to_string_lossy().into_owned();
    let src = std::fs::read_to_string(path).map_err(|e| format!("read {path_str}: {e}"))?;
    let tokens = lexer::lex(&src).map_err(|e| format!("lex: {e:?}"))?;
    let module = parser::parse(tokens).map_err(|e| format!("parse: {e:?}"))?;

    if module.imports.is_empty() && !loader::module_is_runtime_bearing(&module) {
        let program = sema::check_typed(&module, &path_str).map_err(sema_err)?;
        let addr = module.path.join(".");
        let mut programs = BTreeMap::new();
        let mut modules = BTreeMap::new();
        modules.insert(addr.clone(), module);
        programs.insert(addr.clone(), program);
        return Ok(CostStageClosure {
            root: addr,
            programs,
            modules,
        });
    }

    if module.imports.is_empty() {
        return load_runtime_bearing_singleton(&path_str, module);
    }

    let loaded = loader::load_closure(path).map_err(load_err)?;
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
    let mut internal_sources = generated_internal_source_keys(&paths);
    if trust_census_stdlib_snapshot {
        internal_sources.extend(paths.iter().filter_map(|(key, path)| {
            let components = Path::new(path)
                .components()
                .filter_map(|component| component.as_os_str().to_str())
                .collect::<Vec<_>>();
            components
                .windows(2)
                .any(|pair| pair[0] == "stdlib" && matches!(pair[1], "core" | "drivers"))
                .then(|| key.clone())
        }));
    }
    let progs =
        sema::check_program_typed_with_internal_sources(&modules_by_key, &paths, &internal_sources)
            .map_err(sema_err)?;
    let programs: BTreeMap<String, TypedProgram> =
        progs.into_iter().map(|(k, p)| (k.join("."), p)).collect();
    let modules: BTreeMap<String, Module> = modules_by_key
        .into_iter()
        .map(|(k, m)| (k.join("."), m))
        .collect();
    Ok(CostStageClosure {
        root,
        programs,
        modules,
    })
}

fn load_runtime_bearing_singleton(path: &str, module: Module) -> Result<CostStageClosure, String> {
    let (runtime_key, runtime_loaded) = loader::load_runtime_module().map_err(load_err)?;
    let root_key = module.path.clone();
    let runtime_path = runtime_loaded.file.display().to_string();
    let mut modules_by_key = BTreeMap::new();
    modules_by_key.insert(root_key.clone(), module.clone());
    modules_by_key.insert(runtime_key.clone(), runtime_loaded.module);
    let stub_module = rtconfig::parse_generated(&rtconfig::stub_text())
        .map_err(|e| format!("rtconfig stub: {e}"))?;
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
        let (time_key, time_loaded) = loader::load_time_module().map_err(load_err)?;
        let time_path = time_loaded.file.display().to_string();
        paths.insert(time_key.clone(), time_path);
        modules_by_key.insert(time_key.clone(), time_loaded.module);
        Some(time_key)
    } else {
        None
    };
    let internal_sources = generated_internal_source_keys(&paths);
    let mut progs =
        sema::check_program_typed_with_internal_sources(&modules_by_key, &paths, &internal_sources)
            .map_err(sema_err)?;
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
    Ok(CostStageClosure {
        root: root_key.join("."),
        programs,
        modules,
    })
}

pub fn codegen_cost_stage(path: &Path) -> Result<CodegenProgram, String> {
    codegen_cost_stage_with_placement(path).map(|(prog, _)| prog)
}

pub fn codegen_cost_stage_with_placement(
    path: &Path,
) -> Result<(CodegenProgram, crate::placement::PlacementTable), String> {
    codegen_cost_stage_with_placement_direct_fp(path, true)
}

/// Compile the cost-stage closure with the renderer direct-FP marker either
/// honored or stripped. The disabled form exists solely to reproduce the
/// ordered P8R.3 pre-codegen checkpoint from the current decomposed source;
/// shipped compilation always uses [`codegen_cost_stage_with_placement`].
pub fn codegen_cost_stage_with_placement_direct_fp(
    path: &Path,
    direct_fp: bool,
) -> Result<(CodegenProgram, crate::placement::PlacementTable), String> {
    let mut pieces = cost_stage_pieces(path)?;
    if !direct_fp {
        pieces.mwir.direct_fp_fns.clear();
    }
    let placement = pieces.placement.clone();
    Ok((pieces.codegen()?, placement))
}

/// Compile a checked-in census source snapshot whose vendored stdlib was
/// reconstructed by xtask from pinned repository inputs. This explicit API is
/// the provenance capability; ordinary source paths cannot acquire it.
pub fn codegen_cost_stage_census_snapshot(
    path: &Path,
    direct_fp: bool,
) -> Result<(CodegenProgram, crate::placement::PlacementTable), String> {
    codegen_cost_stage_census(path, direct_fp, true)
}

/// Compile the hot-census basis with every sealed numeric helper as an
/// explicit root. Product reachability is intentionally narrower; the census
/// contract is not, and must fail if any literal target cannot be emitted.
pub fn codegen_cost_stage_census(
    path: &Path,
    direct_fp: bool,
    trust_census_stdlib_snapshot: bool,
) -> Result<(CodegenProgram, crate::placement::PlacementTable), String> {
    let checked = load_cost_stage_closure_impl(path, trust_census_stdlib_snapshot)?;
    let mut pieces = cost_stage_pieces_from_with_extra_roots(
        &checked,
        &["sqrt_scalar", "rsqrt_scalar", "raster_rsqrt"],
    )?;
    if !direct_fp {
        pieces.mwir.direct_fp_fns.clear();
    }
    let placement = pieces.placement.clone();
    Ok((pieces.codegen()?, placement))
}

pub fn codegen_cost_stage_with_block_layout(
    path: &Path,
    classes: &crate::cost::LayoutClasses,
) -> Result<
    (
        CodegenProgram,
        crate::placement::PlacementTable,
        crate::blocklayout::LayoutSummary,
    ),
    String,
> {
    let mut pieces = cost_stage_pieces(path)?;
    let (relaid, summary) = crate::blocklayout::relayout_program(&pieces.mwir, classes)?;
    pieces.mwir = relaid;
    let placement = pieces.placement.clone();
    Ok((pieces.codegen()?, placement, summary))
}

struct CostStagePieces {
    mwir: crate::mwir::MwirProgram,
    flow: flowwir::FlowWirProgram,
    layout_ctx: crate::mwir::LayoutCtx,
    method_index: BTreeMap<String, BTreeMap<String, usize>>,
    group_arena_capacity: u64,
    enqueue_specs: Vec<(String, u64, u64)>,
    placement: crate::placement::PlacementTable,
}

impl CostStagePieces {
    fn codegen(&self) -> Result<CodegenProgram, String> {
        codegen::codegen_program_with_async(
            &self.mwir,
            &self.flow,
            &self.layout_ctx,
            &self.method_index,
            self.group_arena_capacity,
            &self.enqueue_specs,
        )
        .map_err(|e| e.message)
    }
}

fn cost_stage_pieces(path: &Path) -> Result<CostStagePieces, String> {
    let checked = load_cost_stage_closure(path)?;
    cost_stage_pieces_from(&checked)
}

fn cost_stage_pieces_from(checked: &CostStageClosure) -> Result<CostStagePieces, String> {
    cost_stage_pieces_from_with_extra_roots(checked, &[])
}

fn cost_stage_pieces_from_with_extra_roots(
    checked: &CostStageClosure,
    extra_roots: &[&str],
) -> Result<CostStagePieces, String> {
    let mut reachable =
        lower::guest_reachable_keys_closure(&checked.programs, &LowerOpts::default());
    for root in extra_roots {
        if !checked
            .programs
            .values()
            .any(|program| program.fns.contains_key(*root))
        {
            return Err(format!(
                "cost census: required explicit root `{root}` is absent from the checked closure"
            ));
        }
        reachable.insert((*root).to_string());
    }
    let lower_opts = LowerOpts {
        emit_comptime_tests: false,
        only: Some(reachable),
    };

    let mut mwir_programs = Vec::with_capacity(checked.programs.len());
    let mut flow_fns = BTreeMap::new();
    for typed in checked.programs.values() {
        let p = lower::lower_program_with(typed, &lower_opts).map_err(|e| e.message)?;
        mwir_programs.push(p);
        let flow = flowwir_lower::lower_program_with(typed, &lower_opts).map_err(|e| e.message)?;
        flow_fns.extend(flow.fns);
    }
    let mwir_program = crate::layout::merge_mwir_programs(mwir_programs);
    let flow_program = flowwir::FlowWirProgram { fns: flow_fns };

    let mut layout_ctx = crate::layout::merge_layout_ctx(&checked.modules).map_err(sema_err)?;
    crate::layout::enrich_layout_ctx_with_instantiations(&mut layout_ctx, &checked.programs);
    let method_index = crate::layout::actor_method_index_tables(&checked.modules, &layout_ctx)
        .map_err(|e| e.message)?;
    let group_arena_capacity = crate::layout::count_with_group_sites(&checked.modules);

    let (enqueue_specs, placement) = {
        let root = &checked.programs[&checked.root];
        match &root.image_fn {
            Some(name) => match crate::eval::interp::eval_image(root, name) {
                Ok(graph) => {
                    let specs =
                        crate::layout::mailbox_enqueue_specs(&graph, &checked.modules, &layout_ctx)
                            .unwrap_or_default();
                    let table =
                        crate::placement::place(&graph, &checked.modules, &layout_ctx, graph.cores)
                            .unwrap_or_default();
                    (specs, table)
                }
                Err(_) => (Vec::new(), crate::placement::PlacementTable::default()),
            },
            None => (Vec::new(), crate::placement::PlacementTable::default()),
        }
    };

    Ok(CostStagePieces {
        mwir: mwir_program,
        flow: flow_program,
        layout_ctx,
        method_index,
        group_arena_capacity,
        enqueue_specs,
        placement,
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TextScope {
    Closure,
    Image,
}

impl TextScope {
    pub fn as_str(self) -> &'static str {
        match self {
            TextScope::Closure => "closure",
            TextScope::Image => "image",
        }
    }
}

pub fn codegen_shipped_program(
    path: &Path,
) -> Result<(CodegenProgram, crate::placement::PlacementTable, TextScope), String> {
    codegen_shipped_from(&load_shipped_front(path)?)
}

/// Return the final-address program, including layout-injected runtime and
/// fixed executable sections, for an image root.  The legacy CodegenProgram
/// API remains available for closure-only diagnostics.
pub fn linked_shipped_program(
    path: &Path,
) -> Result<
    (
        crate::linked::LinkedProgram,
        crate::placement::PlacementTable,
        TextScope,
    ),
    String,
> {
    let already_recording = crate::codegen::block_bridge_enabled();
    if !already_recording {
        crate::codegen::set_block_bridge(true);
    }
    let result = linked_shipped_program_recording(path);
    if !already_recording {
        crate::codegen::set_block_bridge(false);
    }
    result
}

fn linked_shipped_program_recording(
    path: &Path,
) -> Result<
    (
        crate::linked::LinkedProgram,
        crate::placement::PlacementTable,
        TextScope,
    ),
    String,
> {
    let front = load_shipped_front(path)?;
    let Some(img) = &front.image else {
        let pieces = cost_stage_pieces_from(&front.checked)?;
        let program = pieces.codegen()?;
        let relaxed = crate::relax::relax_immediates(&program)
            .map_err(|e| format!("late immediate relaxation: {e}"))?;
        let linked = crate::linked::link_wide(&relaxed.program, wrela_machine::layout::IMAGE_BASE)?;
        let (linked, _) = crate::relax::relax_linked_addresses(&linked)
            .map_err(|e| format!("late address relaxation: {e}"))?;
        return Ok((linked, pieces.placement.clone(), TextScope::Closure));
    };
    let (layout, placement) = layout_shipped_image(&front, img)?;
    let linked = layout
        .linked
        .ok_or_else(|| "image layout did not produce a linked executable stream".to_string())?;
    Ok((linked, placement, TextScope::Image))
}

/// Digest the exact sealed image blob for a source basis.
///
/// The hot-path census records this beside its emitted-code dump digest, so a
/// checked-in report identifies both the executable stream it counted and the
/// complete image whose renderer it describes.
pub fn shipped_image_digest(path: &Path) -> Result<String, String> {
    let front = load_shipped_front(path)?;
    shipped_image_digest_from_front(path, &front, true)
}

pub fn shipped_image_digest_census(path: &Path, direct_fp: bool) -> Result<String, String> {
    let front = load_shipped_front(path)?;
    shipped_image_digest_from_front(path, &front, direct_fp)
}

pub fn shipped_image_digest_census_snapshot(path: &Path) -> Result<String, String> {
    let front = load_shipped_front_impl(path, true)?;
    shipped_image_digest_from_front(path, &front, false)
}

fn shipped_image_digest_from_front(
    path: &Path,
    front: &ShippedFront,
    direct_fp: bool,
) -> Result<String, String> {
    let img = front
        .image
        .as_ref()
        .ok_or_else(|| format!("{} does not declare an image", path.display()))?;
    let root = &front.checked.programs[&front.checked.root];
    let runtime_tests: Vec<String> = root
        .tests
        .iter()
        .filter(|test| test.kind == crate::sema::typed::TestKind::Runtime)
        .map(|test| test.name.clone())
        .collect();
    if runtime_tests.is_empty() {
        return Err(format!(
            "{} has no runtime image test to identify",
            path.display()
        ));
    }
    let async_tests: std::collections::BTreeSet<String> = runtime_tests
        .iter()
        .filter(|name| {
            root.fns
                .get(*name)
                .is_some_and(|function| function.is_async)
        })
        .cloned()
        .collect();
    let compiled = crate::layout::lower_and_codegen_image_with_direct_fp(
        &front.checked.modules,
        &front.checked.programs,
        &img.layout_ctx,
        &img.graph,
        Some(&img.checked.renderer_configs),
        Some(&img.pixels),
        false,
        &runtime_tests,
        &async_tests,
        false,
        direct_fp,
    )?;
    let test_args =
        crate::layout::resolve_runtime_test_args(root, &runtime_tests, &compiled.graph)?;
    let boot = crate::layout::BootCtx {
        graph: &compiled.graph,
        modules: &compiled.modules,
        programs: &compiled.programs,
        layouts: &compiled.layouts,
        layout_ctx: &compiled.layout_ctx,
        async_frames: &compiled.async_frames,
        group_child_index: &compiled.group_child_index,
        flow: &compiled.flow,
    };
    let mut layout = crate::layout::layout_test_image(
        &compiled.program,
        &runtime_tests,
        &async_tests,
        Some(boot),
        &test_args,
    )
    .map_err(|error| error.message)?;
    crate::layout::attach_pixels(&mut layout, &img.pixels.compiled_renderers, true)
        .map_err(|error| error.message)?;
    crate::layout::attach_blk_report(&mut layout, &img.graph, &compiled.programs)
        .map_err(|error| error.message)?;
    Ok(wrela_machine::sha256::sha256_hex(&layout.blob))
}

fn layout_shipped_image(
    front: &ShippedFront,
    img: &ShippedImage,
) -> Result<(crate::layout::ImageLayout, crate::placement::PlacementTable), String> {
    let compiled = crate::layout::lower_and_codegen_image(
        &front.checked.modules,
        &front.checked.programs,
        &img.layout_ctx,
        &img.graph,
        Some(&img.checked.renderer_configs),
        Some(&img.pixels),
        false,
        &[],
        &std::collections::BTreeSet::new(),
        false,
    )?;
    let placement = crate::placement::place(
        &compiled.graph,
        &compiled.modules,
        &compiled.layout_ctx,
        compiled.graph.cores,
    )
    .unwrap_or_default();
    let boot = crate::layout::BootCtx {
        graph: &compiled.graph,
        modules: &compiled.modules,
        programs: &compiled.programs,
        layouts: &compiled.layouts,
        layout_ctx: &compiled.layout_ctx,
        async_frames: &compiled.async_frames,
        group_child_index: &compiled.group_child_index,
        flow: &compiled.flow,
    };
    let mut layout =
        crate::layout::layout_program(&compiled.program, Some(boot)).map_err(|e| e.message)?;
    crate::layout::attach_pixels(&mut layout, &img.pixels.compiled_renderers, false)
        .map_err(|error| error.message)?;
    Ok((layout, placement))
}

pub struct ShippedFront {
    checked: CostStageClosure,
    image: Option<ShippedImage>,
}

struct ShippedImage {
    graph: crate::eval::image::ImageGraph,
    layout_ctx: crate::mwir::LayoutCtx,
    checked: crate::eval::image_checks::CheckedImage,
    pixels: crate::pixels::PixelsProgramSet,
}

pub fn load_shipped_front(path: &Path) -> Result<ShippedFront, String> {
    load_shipped_front_impl(path, false)
}

fn load_shipped_front_impl(
    path: &Path,
    trust_census_stdlib_snapshot: bool,
) -> Result<ShippedFront, String> {
    let checked = load_cost_stage_closure_impl(path, trust_census_stdlib_snapshot)?;
    let root = &checked.programs[&checked.root];
    let image = match root.image_fn.clone() {
        None => None,
        Some(image_fn) => {
            let graph = crate::eval::interp::eval_image(root, &image_fn)
                .map_err(|e| format!("eval @image `{image_fn}`: {}", e.message))?;
            let image_checked =
                crate::eval::image_checks::check_sealed(&graph, root, &checked.programs)
                    .map_err(|error| error.message)?;
            let pixels = crate::pixels::compile_all(
                &checked.programs,
                &root.module_path,
                &graph,
                &image_checked.renderer_configs,
            )
            .map_err(|error| error.diagnostic().message.clone())?;
            let layout_ctx = crate::layout::merge_layout_ctx(&checked.modules).map_err(sema_err)?;
            Some(ShippedImage {
                graph,
                layout_ctx,
                checked: image_checked,
                pixels,
            })
        }
    };
    Ok(ShippedFront { checked, image })
}

/// Compile and return the sealed Pixels programs for source-level tools that
/// need the semantic graph rather than linked machine code.
pub fn load_pixels_programs(path: &Path) -> Result<crate::pixels::PixelsProgramSet, String> {
    load_shipped_front(path)?
        .image
        .map(|image| image.pixels)
        .ok_or_else(|| format!("{} does not seal an image", path.display()))
}

pub fn codegen_shipped_from(
    front: &ShippedFront,
) -> Result<(CodegenProgram, crate::placement::PlacementTable, TextScope), String> {
    let Some(img) = &front.image else {
        let pieces = cost_stage_pieces_from(&front.checked)?;
        let placement = pieces.placement.clone();
        return Ok((pieces.codegen()?, placement, TextScope::Closure));
    };
    let compiled = crate::layout::lower_and_codegen_image(
        &front.checked.modules,
        &front.checked.programs,
        &img.layout_ctx,
        &img.graph,
        Some(&img.checked.renderer_configs),
        Some(&img.pixels),
        false,
        &[],
        &std::collections::BTreeSet::new(),
        false,
    )?;
    let placement = crate::placement::place(
        &compiled.graph,
        &compiled.modules,
        &compiled.layout_ctx,
        compiled.graph.cores,
    )
    .unwrap_or_default();
    Ok((compiled.program, placement, TextScope::Image))
}

pub fn report_cost_stage_path(path: &Path) -> Result<CostReport, String> {
    let (linked, placement, _) = linked_shipped_program(path)?;
    let table = load_default()?;
    let mut report = crate::cost::score::score_linked_program(&linked, &table, &placement)?;
    let attach = crate::cost::compose::WorkloadAttach::load_default_for_linked(
        Some(path),
        &linked,
        &table,
        &placement,
    )?;
    crate::cost::compose::attach_workloads(&mut report, &attach)?;
    Ok(report)
}

pub fn score_cost_stage_path(path: &Path) -> Result<u64, String> {
    Ok(report_cost_stage_path(path)?.total_proxy_cycles)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cost::footprint::{self, HotBlocks};
    use crate::cost::sweep::SweepPoint;

    fn case(name: &str) -> std::path::PathBuf {
        let dir = super::super::repo_root().join("tests/golden").join(name);
        match std::fs::read_to_string(dir.join("root")) {
            Ok(r) => dir.join(r.trim()),
            Err(_) => dir.join("input.wr"),
        }
    }

    fn budget(prog: &CodegenProgram, place: &crate::placement::PlacementTable) -> (u64, u64, u64) {
        let t = load_default().expect("table");
        let p = SweepPoint::pinned(&t);
        let b = footprint::compute(prog, &t, &p, place, HotBlocks::All).expect("footprint");
        (
            b.iter().map(|c| c.fetched_text_bytes).sum(),
            b.iter().map(|c| c.over_l1i_lines).sum(),
            b.iter().map(|c| c.charge).sum(),
        )
    }

    #[test]
    fn the_cost_stage_closure_and_the_shipped_image_are_two_programs_and_say_so() {
        crate::opts::apply_mode(crate::opts::CompileMode::Release);
        let path = case("cost-product-appliance");
        let (closure, cplace) = codegen_cost_stage_with_placement(&path).expect("closure");
        let (linked, iplace, scope) = linked_shipped_program(&path).expect("image");
        assert_eq!(scope, TextScope::Image, "the flagship declares an @image");
        assert_eq!(TextScope::Closure.as_str(), "closure");

        let (c_hot, c_over, c_charge) = budget(&closure, &cplace);
        let table = load_default().expect("table");
        let linked_budget = footprint::compute_linked(
            &linked,
            &table,
            &SweepPoint::pinned(&table),
            &iplace,
            HotBlocks::All,
        )
        .expect("linked footprint");
        let i_hot = linked_budget
            .iter()
            .map(|c| c.fetched_text_bytes)
            .sum::<u64>();
        let i_over = linked_budget.iter().map(|c| c.over_l1i_lines).sum::<u64>();
        let i_charge = linked_budget.iter().map(|c| c.charge).sum::<u64>();
        eprintln!(
            "K2 appliance closure hot={c_hot} over_l1i={c_over} charge={c_charge} fns={}",
            closure.fns.len()
        );
        eprintln!(
            "K2 appliance image   hot={i_hot} over_l1i={i_over} charge={i_charge} fns={}",
            linked.fns.len()
        );
        assert!(
            i_hot > c_hot * 5,
            "the shipped image is an order of magnitude more text than the cost-stage \
             closure; if that stops being true the reconciliation has changed shape: \
             closure={c_hot} image={i_hot}"
        );
        assert_eq!(
            c_over, 0,
            "the closure fits its L1I — which is why the gate never saw the constraint"
        );
        assert_eq!(
            i_over, 0,
            "the final linked executable is below L1I under actual addresses"
        );
        assert_eq!(i_charge, 0, "no synthetic padding charge remains");
        // Stage-1 vector handling and atomic pending-word consumption are part
        // of the shipped executable footprint, even though fixed table bytes
        // remain outside this instruction count.
        assert_eq!(linked.executable_words(), 13628);
    }

    #[test]
    fn shipped_workload_join_resolves_every_production_window_observation() {
        crate::opts::apply_mode(crate::opts::CompileMode::Release);
        let report = report_cost_stage_path(&case("boot-actors")).expect("linked report");
        assert_eq!(
            report.workload_coverage["boot-actors"],
            (18278, 18278),
            "production reports must use exact source-aware linked origins, never fallback charge"
        );
        assert!(report.workload_totals["boot-actors"] > 0);
    }

    #[test]
    fn a_root_with_no_image_is_scored_as_a_closure_and_labelled_one() {
        crate::opts::apply_mode(crate::opts::CompileMode::Release);
        let (_, _, scope) = codegen_shipped_program(&case("cost-arith")).expect("closure");
        assert_eq!(scope, TextScope::Closure);
    }
}
