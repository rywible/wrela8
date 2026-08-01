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
/// This is the **parked** pass's pipeline entry point (CLAUDE.md's "a
/// refused opt is parked, not deleted"; plans/codegen-pareto-2.md decisions
/// 1910 and 1940). `classes` comes from [`super::layout_classes`] over the
/// *same* path's sidecar and a block partition;
/// [`crate::cost::LayoutClasses::Unmeasured`] plans the identity for every
/// fn, so this reduces to [`codegen_cost_stage_with_placement`] exactly
/// (proved, not asserted: `unit:no_sidecar_degrades_to_a_byte_identical_layout`
/// at the pass, and the whole-program check in `blocklayout`'s measurement
/// unit).
///
/// It is a **second** entry point rather than a parameter on the first
/// because item D is not installed on the default compile path — see
/// `blocklayout`'s "Why this pass is not installed" note and decision 1755.
/// Nothing in the compiler calls it; the only callers are `blocklayout`'s
/// own units, which is what keeps a parked pass from rotting
/// (`unit:the_parked_pass_is_not_on_the_compile_path`).
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

/// The program a scored budget line describes
/// (plans/codegen-pareto-2.md item K, decision 1953).
///
/// `--stage=cost` and `--stage=report` printed two `hot_text_bytes` under
/// two lines both called `Budget`, and on the appliance they read 7 936 B
/// and 89 024 B. Neither was wrong; they are two different programs, and
/// nothing on either line said so. This is what says so.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TextScope {
    /// `lower::guest_reachable_keys_closure` against the **stub**
    /// `core.__image_runtime`, with `emit_comptime_tests: false` and no
    /// image build. Everything the runtime reaches only through the live
    /// dispatch tables is absent, which is most of the runtime.
    Closure,
    /// The program `wrela build` emits: live rtconfig, image force-roots,
    /// entry and vector text. What the appliance ships.
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

/// The program the appliance would **ship** for `path`, and its placement.
///
/// A root that declares an `@image` is compiled through
/// [`crate::layout::lower_and_codegen_image`] — byte-for-byte the pipeline
/// `wrela build` and `--stage=report` use — so a budget taken here is the
/// budget of the shipped image rather than of a truncated closure. A root
/// with no `@image` ships nothing, so the closure is all there is and the
/// scope says so.
///
/// **This is what the ∀ gate scores** (decision 1954). Item H measured the
/// gap and declined to close it; the gap is 11× on the flagship's hot text
/// and it is the whole reason round 1's item D could not be scored — its
/// premise (93–98 KB of text against a 64 KiB L1I) is a fact about the
/// image column, and the gate read the closure one.
pub fn codegen_shipped_program(
    path: &Path,
) -> Result<(CodegenProgram, crate::placement::PlacementTable, TextScope), String> {
    let checked = load_cost_stage_closure(path)?;
    let root = &checked.programs[&checked.root];
    let Some(image_fn) = root.image_fn.clone() else {
        let (prog, placement) = codegen_cost_stage_with_placement(path)?;
        return Ok((prog, placement, TextScope::Closure));
    };
    let graph = crate::eval::interp::eval_image(root, &image_fn)
        .map_err(|e| format!("eval @image `{image_fn}`: {}", e.message))?;
    let layout_ctx = crate::layout::merge_layout_ctx(&checked.modules).map_err(sema_err)?;
    let compiled = crate::layout::lower_and_codegen_image(
        &checked.modules,
        &checked.programs,
        &layout_ctx,
        &graph,
        &[],
        &std::collections::BTreeSet::new(),
        false,
    )?;
    let placement =
        crate::placement::place(&graph, &compiled.modules, &compiled.layout_ctx, graph.cores)
            .unwrap_or_default();
    Ok((compiled.program, placement, TextScope::Image))
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
            b.iter().map(|c| c.hot_text_bytes).sum(),
            b.iter().map(|c| c.over_l1i_lines).sum(),
            b.iter().map(|c| c.charge).sum(),
        )
    }

    /// **K2's regression test (decision 1953/1954).** The two stages do not
    /// agree, they measure two different programs — and the difference is
    /// now named on the line and measured here rather than discovered by a
    /// reader comparing two dumps.
    ///
    /// It fails on the old behaviour in the only way it can: before
    /// `codegen_shipped_program` existed there was no way to ask for the
    /// shipped program's budget from the cost side at all, so the gap could
    /// not be stated as one number, let alone pinned.
    #[test]
    fn the_cost_stage_closure_and_the_shipped_image_are_two_programs_and_say_so() {
        crate::opts::apply_mode(crate::opts::CompileMode::Release);
        let path = case("cost-product-appliance");
        let (closure, cplace) = codegen_cost_stage_with_placement(&path).expect("closure");
        let (image, iplace, scope) = codegen_shipped_program(&path).expect("image");
        assert_eq!(scope, TextScope::Image, "the flagship declares an @image");
        assert_eq!(TextScope::Closure.as_str(), "closure");

        let (c_hot, c_over, c_charge) = budget(&closure, &cplace);
        let (i_hot, i_over, i_charge) = budget(&image, &iplace);
        eprintln!(
            "K2 appliance closure hot={c_hot} over_l1i={c_over} charge={c_charge} fns={}",
            closure.fns.len()
        );
        eprintln!(
            "K2 appliance image   hot={i_hot} over_l1i={i_over} charge={i_charge} fns={}",
            image.fns.len()
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
        assert!(
            i_over > 0 && i_charge > 0,
            "the shipped image is over its L1I and is charged for it: over={i_over} \
             charge={i_charge}"
        );
    }

    /// A root with no `@image` ships nothing, so the closure is the whole
    /// program and the scope says so rather than pretending.
    #[test]
    fn a_root_with_no_image_is_scored_as_a_closure_and_labelled_one() {
        crate::opts::apply_mode(crate::opts::CompileMode::Release);
        let (_, _, scope) = codegen_shipped_program(&case("cost-arith")).expect("closure");
        assert_eq!(scope, TextScope::Closure);
    }
}
