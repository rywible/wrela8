//! Cost-stage emit pipeline (shared by `wrela dump --stage=cost` and the
//! M19 proxy-win oracle).
//!
//! Matches the dump path: check the import/runtime closure, lower the
//! guest-reachable set (so force-rooted `core.runtime` enters the emit
//! set for `@test(runtime)`), codegen with async, then score. Opt TLS
//! must already be set by the caller (`opts::apply_mode` /
//! `opts::apply_opts`) before codegen.

use std::collections::BTreeMap;
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

use super::score::{CostReport, score_program};
use super::table::load_default;

/// Checked multi-module closure ready for cost-stage lower/codegen.
pub struct CostStageClosure {
    pub root: String,
    pub programs: BTreeMap<String, TypedProgram>,
    pub modules: BTreeMap<String, Module>,
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

/// Load + check the same closure shape `wrela dump --stage=cost` uses
/// (`check_closure` in `bin/wrela.rs`).
pub fn load_cost_stage_closure(path: &Path) -> Result<CostStageClosure, String> {
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
    let progs = sema::check_program_typed(&modules_by_key, &paths).map_err(sema_err)?;
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
    let mut progs = sema::check_program_typed(&modules_by_key, &paths).map_err(sema_err)?;
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

/// Lower + codegen the cost-stage closure (same pipeline as
/// `wrela dump --stage=cost` / `--stage=asm`). Caller must have set opt
/// TLS already.
pub fn codegen_cost_stage(path: &Path) -> Result<CodegenProgram, String> {
    codegen_cost_stage_with_placement(path).map(|(prog, _)| prog)
}

/// Same, plus the sealed placement table the scorer needs.
///
/// plans/M20.md decision 1603: the I-side, TLB and cross-core terms are
/// per **core**, and `PlacementTable::core_of` is what makes a load's
/// local-vs-remote verdict a static fact. The cost-stage path builds the
/// same table the image report publishes, from the same `ImageGraph` it
/// already evaluates for the enqueue specs — so items F and G score
/// against real placement rather than against an empty stand-in. A
/// closure with no `@image` (most `cost-*` cases) has no placement to
/// build and gets the default, which classifies nothing.
pub fn codegen_cost_stage_with_placement(
    path: &Path,
) -> Result<(CodegenProgram, crate::placement::PlacementTable), String> {
    let pieces = cost_stage_pieces(path)?;
    let placement = pieces.placement.clone();
    Ok((pieces.codegen()?, placement))
}

/// [`codegen_cost_stage_with_placement`], with plans/codegen-pareto.md item
/// D's hot/cold block layout applied to the MWIR program in between.
///
/// This is the pass's real pipeline entry point. `classes` comes from
/// [`super::layout_classes`] over the *same* path's sidecar and a block
/// partition; [`crate::blocklayout::LayoutClasses::Unmeasured`] plans the
/// identity for every fn, so this reduces to
/// [`codegen_cost_stage_with_placement`] exactly (proved, not asserted:
/// `unit:no_sidecar_degrades_to_a_byte_identical_layout` at the pass, and
/// the whole-program check in `blocklayout`'s measurement unit).
///
/// It is a **second** entry point rather than a parameter on the first
/// because item D is not installed on the default compile path — see
/// `blocklayout`'s `WHY NOT WIRED` note and decision 1755.
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

/// Everything `codegen_cost_stage_*` needs between lowering and emission.
/// Not an abstraction — the two entry points above would otherwise be the
/// same fifty lines twice, and item D has to reach the MWIR program in the
/// middle of them.
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
    let reachable = lower::guest_reachable_keys_closure(&checked.programs, &LowerOpts::default());
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

/// Full scored report for `path` under the current opt TLS (caller sets
/// mode/opts first). The gate needs more than the cycle total — static
/// word count and measured-W coverage are side conditions (04 §5).
pub fn report_cost_stage_path(path: &Path) -> Result<CostReport, String> {
    let (prog, placement) = codegen_cost_stage_with_placement(path)?;
    let table = load_default()?;
    score_program(&prog, &table, &placement)
}

/// Score `path` under the current opt TLS (caller sets mode/opts first).
pub fn score_cost_stage_path(path: &Path) -> Result<u64, String> {
    Ok(report_cost_stage_path(path)?.total_proxy_cycles)
}
