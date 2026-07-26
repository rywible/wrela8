//! Semantic checking: `syntax::ast::Module -> ()` (a diagnostic) or the
//! `check` stage's dump. Normative source: docs/language/, chiefly
//! §2-§8 (02-language.md). Shape frozen by plans/M2.md:
//!
//! - One file per pass (decision 10), in the frozen pass order (decision
//!   3): collect + resolve (symbols.rs) -> declare (types.rs) -> bodies
//!   (bodies.rs) -> access (access.rs) -> flow (flow.rs, storage paths
//!   shared with paths.rs) -> matches (matches.rs) -> generics
//!   (generics.rs). Every file exists from item A onward, stubbed until
//!   its own item lands, so later items land in their own file rather
//!   than growing this one.
//! - One diagnostic shape throughout (decision 1): `error[<category>]:
//!   <message> at <line>:<col>`, fail-fast — the first error in pass
//!   order, source order within a pass.
//! - A fail-closed helper (decision 7) any pass can reuse: a reachable
//!   construct sema does not check yet reports `error[unimplemented]:
//!   <what> is not checked yet at L:C` instead of silently accepting it.
//!
//! Item A lands collect + resolve and the name-resolution surface
//! (`symbols::is_resolvable_without_import`; the old `prelude.rs`
//! placeholder was deleted at plans/M9.md item I); every later pass is a
//! stub (a no-op) until its own item lands, so `check` below only calls
//! what item A actually implements.

pub mod access;
pub mod bodies;
pub mod flow;
/// plans/M9.md item D: f-string desugar onto Format + `String` concat.
pub mod fstring;
pub mod generics;
pub mod handoff;
pub mod imports;
/// plans/M9.md item AA: the compiler's intrinsic surface, written down
/// and locked against `bodies.rs` (there is no runtime code here — the
/// list *is* the deliverable, and its test is the ratchet).
pub mod intrinsics;
pub mod matches;
pub mod paths;
pub mod reserve_proof;
pub mod send_proof;
pub mod specialize;
/// plans/M9.md item I: five formerly-prelude enums loaded from
/// `stdlib/core/*.wr` (variant order for tags / exhaustiveness).
pub mod stdlib_enums;
pub mod symbols;
pub mod typed;
pub mod types;

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use crate::syntax::ast::{Module, Span};

/// One sema diagnostic, printed by the CLI exactly like a lex/parse
/// error: `error[<category>]: <message> at <line>:<col>` (decision 1).
/// `category` is one of the fixed set the plan names — `name`, `type`,
/// `access`, `move`, `init`, `overlap`, `match`, `generic`,
/// `unimplemented` — so a `&'static str` is enough; no enum is needed
/// (decision 4: dumb, no seams for their own sake).
///
/// The one multi-line exception (decision 2, item H): a generic
/// instantiation's requirement-chain diagnostic needs more than one line.
/// Rather than growing a second error type, this struct carries two extra
/// fields that stay empty/false for every other diagnostic in the
/// compiler: `extra_lines` (already-rendered, already-indented lines
/// appended after the primary line — the `required by`/`instantiated at`
/// chain) and `omit_location` (true only for the chain's own primary
/// line, which carries no ` at L:C` suffix at all — its location is the
/// `required by` line instead). The CLI (`wrela.rs`) and the fuzzer/bench
/// harness (`xtask`) both print through these two fields so a plain
/// one-line diagnostic renders exactly as before.
#[derive(Debug)]
pub struct SemaError {
    pub category: &'static str,
    pub message: String,
    pub line: u32,
    pub col: u32,
    pub extra_lines: Vec<String>,
    pub omit_location: bool,
    /// Diagnostic metadata only — never rendered. Set exactly at
    /// `bodies.rs`'s five "no method"/"no operator method" sites
    /// (`(type name, method name)`) so `generics.rs`'s requirement-chain
    /// diagnostic (item H, decision 2) can recognize that shape from
    /// structured data instead of parsing the rendered message text.
    pub missing_method: Option<(String, String)>,
}

impl SemaError {
    /// `pub(crate)` (plans/M4.md item A): `loader.rs` (the new `build`
    /// category) and `sema::imports` construct `SemaError`s directly,
    /// same as every existing pass in this module.
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
}

/// The fail-closed diagnostic (decision 7): `error[unimplemented]:
/// <subject> not checked yet at L:C`. `subject` is the whole clause
/// including its verb (e.g. `"imports are"`, `"await is"`) so the
/// message reads grammatically for both plural and singular
/// constructs — this helper only supplies " not checked yet" and the
/// category. Every pass that reaches a construct it does not check yet
/// returns this instead of silently accepting it. Item A's only user is
/// `symbols::resolve` (imports); later items reuse it verbatim for their
/// own fail-closed sets.
pub fn unimplemented_at(subject: &str, span: Span) -> SemaError {
    SemaError::at("unimplemented", format!("{subject} not checked yet"), span)
}

/// Runs the sema pipeline in frozen pass order (decision 3) and returns
/// the first diagnostic, if any. Item A lands collect + resolve; item B
/// adds declare (types.rs: every signature's types, data-vs-resource
/// classification, `deriving` validation). `bodies`/`access`/`flow`/
/// `matches` all share one `ModuleCtx` (built once here) so item H's
/// instantiation queue (`bodies::ModuleCtx::generics_queue`) accumulates
/// every generic use discovered by any of them; `generics::check` (item
/// H) then drains it. `flow` (items E/F) runs its one CFG pass —
/// definite init, moves, exclusivity (02-language.md §3) — between
/// `access` and `matches`, the frozen pass order (decision 3).
///
/// `path` is the file path exactly as given to `wrela dump` — item H's
/// requirement-chain diagnostic cites it verbatim for both its `required
/// by`/`instantiated at` locations (decision 2; the M2 CLI checks one
/// file, so every location in a chain is in the same file).
///
/// plans/M3.md item B: delegates to `check_typed` and discards the
/// checked program — `check_typed`'s own doc comment already promised
/// identical diagnostics/behavior either way, so this stays a plain
/// wrapper instead of re-running the same pipeline a second time (and
/// picks up const-initializer comptime evaluation for free: a `const`
/// whose initializer abandons is a build error at the `check` stage
/// exactly like it is at `typed`).
pub fn check(module: &Module, path: &str) -> Result<(), SemaError> {
    check_typed(module, path).map(|_| ())
}

/// The `typed` stage's pipeline (plans/M3.md item A): the same frozen
/// pass order runs, keeping `bodies::check`'s own typed-program output
/// (decision 1) instead of discarding it, and folding in
/// `generics::check`'s drained instantiation map afterward — every
/// generic use *any* pass (`bodies`/`access`/`flow`/`matches`, each
/// enqueuing into the same shared `mctx`) discovered ends up checked and
/// typed exactly once.
///
/// plans/M3.md item D: `specialize::specialize` runs first, before
/// `collect` even sees the module — every `comptime if` node (module,
/// member, or statement scope) is replaced by its own selected branch's
/// items/members/statements, spliced in directly (decision 8: "the graph
/// that is checked is the graph that exists"); every pass below this
/// line only ever walks that already-specialized module
/// (`specialize.rs`'s own module doc states the exact reading pinned:
/// a condition may reference literals and top-level consts only).
///
/// plans/M3.md item B: once the program is fully assembled (past
/// `generics::check`, so a const initializer calling into a generic-fn
/// instantiation can resolve it), every module-level `const`'s own
/// initializer runs through the real evaluator (`eval::check_consts`) —
/// the integration surface replacing M2-H's literal-only const-argument
/// subset; abandonment (overflow, a failed `assert`, an explicit
/// `panic`, a blown quota) is a build error here, `error[comptime]`.
///
/// plans/M3.md item D: right after, every `comptime assert` statement
/// anywhere in the program is evaluated exactly once
/// (`eval::check_comptime_asserts`), unconditionally — decision 8:
/// "`comptime assert` evaluates after typing; failure is a build error
/// with the message." Both this and `check_consts` share one
/// `eval::legal::classify` call (item C×D's own legality wiring) rather
/// than each computing the whole-program callee graph separately.
pub fn check_typed(module: &Module, path: &str) -> Result<typed::TypedProgram, SemaError> {
    // plans/M4.md item A (orchestrator verification fix): this is the
    // *single-module* entry — only `check_program` (fed by the loader's
    // closure) can bind imports, so an import-bearing module here must
    // fail closed with an honest diagnostic, exactly as it did before
    // item A. Without this arm, the empty `ImportBindings` below would
    // let resolution reach the use site and misreport the import as
    // `error[name]: unknown name` — a diagnostic that names the wrong
    // cause, which is an approximation, not a fail-closed error.
    if let Some(import) = module.imports.first() {
        return Err(unimplemented_at(
            "imports through the single-module entry (`--stage=typed`, `wrela test`) are",
            import.span,
        ));
    }
    // plans/M9.md item E: when the module mentions a time-prelude name
    // (or `now`), run through the whole-closure path with `core.time`
    // loaded so constructors are ordinary Calls into stdlib wrela —
    // still no user-facing import (prelude visibility via IMAGE_BUILDER /
    // ACTOR_SURFACE). Modules that never mention time keep the exact
    // single-module path (byte-identical dumps).
    let text = crate::syntax::printer::pretty(module);
    let needs_time = text
        .split(|c: char| !c.is_ascii_alphanumeric() && c != '_')
        .any(|tok| tok == "now" || crate::loader::TIME_PRELUDE_NAMES.contains(&tok));
    if needs_time {
        return check_typed_with_time_prelude(module, path);
    }
    check_typed_single(module, path)
}

/// Single-module pipeline with `core.time` spliced in (plans/M9.md item E).
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

/// Same as `check_typed_single`, then render the check dump from the
/// DeclItems that pass produced (plans/M9.md item LL).
fn check_typed_single_dump(module: &Module, path: &str) -> Result<String, SemaError> {
    let (_program, decl_items) = check_typed_single_with_decls(module, path)?;
    dump_with_imports(module, &types::ImportedTypes::new(), Some(&decl_items))
}

fn check_typed_single_with_decls(
    module: &Module,
    path: &str,
) -> Result<(typed::TypedProgram, Vec<types::DeclItem>), SemaError> {
    // plans/M9.md item QQ: load auto-visible stdlib enums via the same
    // two-candidate rule as the loader, before specialize reads them.
    prepare_stdlib_enums_for_file(path, module)?;
    let specialized = specialize::specialize(module)?;
    // plans/M7.md item B: the `@layout` exact-bytes pass runs before name
    // resolution — see `types::check_layouts`' own section note for the
    // two reasons (a `@layout` field's type is a closed encoding set, not
    // an ordinary annotation; and 03-hardware.md §3's capability rule must
    // be live before plans/M7.md item A makes a capability name
    // resolvable at all). Its table is discarded here — `--stage=layout-types`
    // and the image report call the same fn for it — and its rejections
    // are still the point. Two later items read the table rather than
    // recompute it: plans/M7.md item C's claim-partitioning check below,
    // and item D, which *keeps* it on the typed program
    // (`TypedProgram::layouts`) so the post-seal pool checks can ask
    // whether an `img.dma_pool[T]`'s own `T` is `@layout(dma)`.
    let layouts = types::check_layouts(&specialized)?;
    let symtab = symbols::collect(&specialized)?;
    symbols::resolve(&specialized, &symtab, &imports::ImportBindings::new())?;
    let decl_items = types::declare(&specialized)?;
    // plans/M10.md item A2c: `@placed` on a `static` needs declare's
    // resolved type and `check_layouts`' table — runtime-layout kind and
    // at-most-one-per-address.
    types::validate_placed_statics(&decl_items, &layouts)?;
    // plans/M7.md item C, 03-hardware.md §2: "Minting a layout consumes
    // those byte ranges from the claim; two live layouts can never alias a
    // register." Runs here because it needs both halves — `declare`'s
    // resolved field types and `@driver` facts, and `check_layouts`' own
    // byte table — and before any body is typed, so an aliasing partition
    // is rejected at the declaration that created it rather than at
    // whichever access happened to be checked first.
    types::check_mmio_claims(&specialized, &decl_items, &layouts)?;
    let mctx = bodies::build_module_ctx(&specialized, &decl_items, &types::ImportedTypes::new());
    let mut program = bodies::check(&specialized, &decl_items, &mctx)?;
    program.layouts = layouts;
    access::check(&specialized, &decl_items, &mctx)?;
    flow::check(&specialized, &decl_items, &mctx)?;
    // plans/M7.md item E3: handoff signature + producer-transition body
    // (03-hardware.md §5). Runs after flow so a missing return is already
    // diagnosed; this pass only insists every `return` is publish/reject.
    handoff::check(&specialized, &decl_items, &mctx)?;
    matches::check(&specialized, &decl_items, &mctx)?;
    program.instantiations = generics::check(&specialized, &decl_items, &mctx, path)?;
    crate::eval::check_comptime(&program)?;
    // plans/M10.md item A2b, decision 581: the **later layout-completion
    // pass**. `check_layouts` above deferred any `runtime` layout whose array
    // length is a `const` name (03 §3.1's own `[TurnArea; N_TURNS]`), because
    // it runs before name resolution and evaluates nothing — decision 580,
    // unchanged. Here every `const` has been type-checked and evaluated by
    // the one real evaluator, so the lengths resolve and the deferred layouts
    // get their real sizes, offsets and padding, with every size-dependent
    // rule (overlap, alignment, total bytes) applied to the completed table.
    // It runs immediately after the comptime pass: that is the earliest point
    // where a `const` has a value, and it is before anything reads
    // `TypedProgram::layouts`.
    let mut layouts = std::mem::take(&mut program.layouts);
    types::complete_layouts(&specialized, &program, &mut layouts)?;
    program.layouts = layouts;
    // plans/M7.md item A, decision 3: 03-hardware.md §1's provenance
    // sentence, checked over the same whole-graph reachability
    // `eval::legal` already computes for comptime legality. It runs here,
    // after `generics::check` has filled `instantiations` (an
    // instantiation is a graph node too) and after the typed program is
    // otherwise complete, for exactly the reason `send_proof` runs where
    // it does: this is a whole-program fact about an already-finished
    // program, not a per-item check.
    crate::eval::legal::check_provenance(
        &program,
        &types::capability_authority(&specialized, &decl_items),
    )?;
    // plans/M7.md item G, decision 3: 03-hardware.md §6's ISR effect
    // restriction — same whole-graph reachability, a third color. Seeds
    // are every `IrqCap.bind` handler in this module.
    crate::eval::legal::check_isr_effects(&program)?;
    // plans/M7.md item G: `wake` only from an ISR or a `@task`; `@task`
    // bodies forbid await/receipt-shaped work (item E owns receipts).
    crate::eval::legal::check_wake_sites(&program)?;
    crate::eval::legal::check_bottom_half(&program)?;
    // plans/M6.md item G, decision 5: the bare `send` statement is the
    // language's one proof-conditioned form, and the proof is a
    // *whole-image* fact (a mailbox's declared capacity lives in the
    // `@image` fn). It therefore runs here — after every pass above has
    // produced the typed program the proof reads, and before any
    // consumer of that program exists. `send_proof::check` returns
    // immediately unless the closure actually contains a bare `send`
    // statement, so this is a no-op for every other program.
    // Single-module entry: the "closure" is this one module.
    let one = BTreeMap::from([(specialized.path.join("."), &program)]);
    send_proof::check(&one)?;
    // plans/M7.md item E2, decision 6: `reserve_proven`'s whole-image
    // descriptor-capacity proof — same shape as `send_proof`, same
    // placement (after the typed program exists, before any consumer).
    reserve_proof::check(&one)?;
    Ok((program, decl_items))
}

/// The `--stage=typed` dump (decision 2): delegates entirely to
/// `typed::dump` — `mod.rs` only ever owns the stage wiring, never the
/// dump's own text (mirrors how `dump` above delegates every declaration
/// line to `types::render_items`).
pub fn dump_typed(program: &typed::TypedProgram) -> String {
    typed::dump(program)
}

/// The `check` stage's dump (decision 8): on success, `Module path=...`
/// then one two-space-indented line per module-level declaration, no
/// spans, resolved types spelled fully (types.rs's `render_items` owns
/// every declaration's exact grammar — item A's dump was names only;
/// item B graduates it to full resolved signatures).
///
/// plans/M9.md item LL: prefer [`check_dump`] / [`check_program_dump`] —
/// those render from the DeclItems the successful check actually used.
/// This entry still exists for callers that already checked; it returns
/// `Err` on a declare mismatch instead of panicking (a dump that can
/// disagree with check must surface as a diagnostic, never `expect`).
/// plans/M3.md item D: specializing first (exactly like `check_typed`)
/// means this dump shows only the selected branch of any `comptime if`
/// — the golden-visible surface the M3-D task names explicitly.
pub fn dump(module: &Module) -> Result<String, SemaError> {
    // Same time-prelude routing as `check_typed`: a module that names
    // Duration/Instant/seconds/… needs `core.time` in the declare table.
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
    dump_with_imports(module, &types::ImportedTypes::new(), None)
}

/// Check one module and render the `--stage=check` dump from the same
/// DeclItems / ImportedTypes the check used (plans/M9.md item LL).
/// Preferred over `check` + `dump`: a re-derived dump can silently
/// disagree with sema (the Duration-in-type-position panic).
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
    // Single-module path: check first, then dump from the DeclItems that
    // same specialize/declare produced (threaded via `check_typed_single_dump`).
    check_typed_single_dump(module, path)
}

/// Check a whole closure and render the check dump from the DeclItems /
/// ImportedTypes that check produced (plans/M9.md item LL).
pub fn check_program_dump(
    modules: &BTreeMap<Vec<String>, Module>,
    paths: &BTreeMap<Vec<String>, String>,
) -> Result<String, SemaError> {
    let (_programs, tables) = check_program_typed_tables(modules, paths)?;
    render_check_dump(modules, &tables)
}

/// `dump` for one module of a build closure (plans/M9.md item A1):
/// `imported` is the same imported-type arity table `declare_with_imports`
/// was given — without it, re-running `declare` here would fail on a
/// signature naming an imported type. `classification` carries
/// `classify_closure`'s whole-closure answer for this module's own
/// declarations, so the dump prints the same `data`/`resource` word sema
/// used rather than the module-local approximation a fresh `declare`
/// would recompute.
///
/// plans/M9.md item LL: returns `Err` instead of panicking when declare
/// disagrees with a prior check — ordinary input must never `expect`.
fn dump_with_imports(
    module: &Module,
    imported: &types::ImportedTypes,
    classification: Option<&[types::DeclItem]>,
) -> Result<String, SemaError> {
    let specialized = specialize::specialize(module)?;
    let decl_items = match classification {
        Some(items) => items.to_vec(),
        None => types::declare_with_imports(&specialized, imported)?,
    };
    let effects = access::infer_effects(&specialized, &decl_items, imported);
    let mut out = format!("Module path={}\n", specialized.path.join("."));
    types::render_items(&decl_items, &effects, &mut out);
    Ok(out)
}

/// Declaration tables a successful whole-program check produced — the
/// check-stage dump must render from these, not re-declare (item LL).
struct CheckDumpTables {
    decl_items_map: BTreeMap<Vec<String>, Vec<types::DeclItem>>,
    imported_types: BTreeMap<Vec<String>, types::ImportedTypes>,
}

fn render_check_dump(
    modules: &BTreeMap<Vec<String>, Module>,
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
        // plans/M10.md item A2d / decision 667: omit auto-injected
        // `core.runtime` from check dumps unless some module imported it.
        if key == &runtime_key && !runtime_explicitly_imported {
            continue;
        }
        // DeclItems are already classified; specialize only for the path
        // line + effect inference. Uses the tables check produced — never
        // a fresh declare (item LL).
        out.push_str(&dump_with_imports(
            module,
            &tables.imported_types[key],
            Some(&tables.decl_items_map[key]),
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

/// plans/M9.md item QQ: pick a package root from the closure and load
/// the five auto-visible stdlib enums from the same `stdlib/core/` the
/// loader would use for `from core.X` imports.
fn prepare_stdlib_enums_for_closure(
    modules: &BTreeMap<Vec<String>, Module>,
    paths: &BTreeMap<Vec<String>, String>,
) -> Result<(), SemaError> {
    for (key, module) in modules {
        if key.first().map(|s| s.as_str()) == Some("core") {
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

/// The whole-program entry (plans/M4.md item A, decision 2): `modules`
/// is `crate::loader::load_closure`'s own output (module address ->
/// file + parsed `Module`, already in BTree order by construction), and
/// `paths` gives each module's own file path string for
/// `generics::check`'s requirement-chain diagnostic (mirrors
/// `check_typed`'s own `path` parameter, one file per chain there —
/// each module here is exactly that "one file" for its own chain).
///
/// Four whole-program passes, then the existing per-module pipeline:
///
/// 1. `specialize` every module independently (comptime-if expansion
///    never needs another module — `specialize.rs`'s own const skeleton
///    already always resolves with zero imports, so this is unaffected
///    either way).
/// 2. `symbols::collect` + this item's own `imports::public_names` for
///    every module — decision 2's "global symbol table: module path ->
///    public names".
/// 3. `imports::resolve_imports` per module against that whole-program
///    table (missing name / non-pub / collision; a missing *module*
///    cannot happen here — `crate::loader` already guaranteed every
///    imported module path is a key of `modules`).
/// 4. The existing single-module pipeline — `symbols::resolve` (now
///    with imports bound), `types::declare`, `bodies::build_module_ctx`
///    — runs per module, exactly unchanged from `check_typed` above.
///
/// Then the **splice**: every import binding copies its target's
/// already-built `fn`/`const`/`struct`/`enum` entry from the exporting
/// module's own, completely independent `ModuleCtx` into the importing
/// module's, under the (possibly aliased) local name — read-only reuse
/// of another module's already-finished output, never a re-check (the
/// importing module's own `bodies::check` only ever walks *its own*
/// `module.items`, never `mctx.fns`/`consts`/`structs`/`enums`
/// directly, so a spliced entry is available for a call/field-
/// access/construction lookup but is never independently re-checked).
/// This is exactly what makes import cycles free (decision 3): each
/// module's own `declare`/`build_module_ctx` needs nothing from any
/// other module to run to completion; only the splice afterward reads
/// another module's already-finished output, and by the time it runs,
/// every module's own output already exists regardless of which one
/// imports which.
///
/// An imported `const`/`fn` is fully usable as a *value* (called,
/// referenced) via the splice above, and an imported `struct`/`enum` is
/// fully usable as a value too (constructed, field-accessed) via the same
/// mechanism.
///
/// plans/M9.md item A1 closes what used to be the matching *type*-position
/// gap: an imported `struct`/`enum` name is now legal wherever a type is
/// legal — fn parameter, fn return, struct field, `const` type, `let`
/// annotation, generic argument — because `imports::imported_type_shapes`
/// merges the closure's type names into `types::declare`'s own arity table
/// (and into `bodies::ModuleCtx::shapes`, which is the same table for
/// bodies). Both halves are read off raw AST, so neither waits on any
/// module's `declare`, and the cycle property above is untouched.
/// Data-vs-resource classification, which *does* need another module's
/// resolved declarations, is therefore not done inside `declare` at all:
/// `types::classify_closure` recomputes it for the whole closure between
/// the declare loop and the splice (decision 10).
///
/// One shape stays module-local and fails closed rather than approximate:
/// `Actor[T]`/capability-argument validation still requires `T` to name a
/// struct declared in the *same* module (`types::validate_actor_handles`,
/// `validate_capability_types`), so an imported `@actor` struct there is
/// rejected by the existing named diagnostic — cross-module actor handles
/// are a milestone question (02-language.md §9.1), not a side effect of a
/// type-name table.
///
/// Finally, the rest of the existing per-module pipeline —
/// `bodies::check`/`access::check`/`flow::check`/`matches::check`/
/// `generics::check`/`eval::check_comptime` — runs per module, in
/// `modules`'s own BTree order (decision 2's "one deterministic batch"),
/// fail-fast: the first module (BTree order) to raise a diagnostic wins,
/// exactly mirroring the existing single-module "first error in pass
/// order, source order within a pass" discipline one level up.
pub fn check_program(
    modules: &BTreeMap<Vec<String>, Module>,
    paths: &BTreeMap<Vec<String>, String>,
) -> Result<(), SemaError> {
    check_program_typed(modules, paths).map(|_| ())
}

/// plans/M4.md item B: the multi-module *typed* entry — identical to
/// `check_program` above except every module's own checked
/// `typed::TypedProgram` is kept (`check_program` discards it and only
/// returns `()`, delegating here unchanged, mirroring `check`'s own
/// relationship to `check_typed`) rather than thrown away. The
/// `--stage=image` evaluator (`eval::image`, driven from `bin/wrela.rs`)
/// needs exactly this: every module's own checked program, so it can
/// find the one reachable `@image` fn (`typed::TypedProgram::image_fn`)
/// across the whole build closure and evaluate it.
pub fn check_program_typed(
    modules: &BTreeMap<Vec<String>, Module>,
    paths: &BTreeMap<Vec<String>, String>,
) -> Result<BTreeMap<Vec<String>, typed::TypedProgram>, SemaError> {
    check_program_typed_tables(modules, paths).map(|(programs, _)| programs)
}

/// Same as `check_program_typed`, also returning the DeclItems /
/// ImportedTypes that check used so the check-stage dump can render
/// from them (plans/M9.md item LL).
fn check_program_typed_tables(
    modules: &BTreeMap<Vec<String>, Module>,
    paths: &BTreeMap<Vec<String>, String>,
) -> Result<(BTreeMap<Vec<String>, typed::TypedProgram>, CheckDumpTables), SemaError> {
    // plans/M9.md item QQ: before specialize (which reads Target/Restart
    // in comptime-if conditions), load the five auto-visible enums from
    // the same `stdlib/core/` the loader would pick for this package.
    prepare_stdlib_enums_for_closure(modules, paths)?;
    let mut specialized: BTreeMap<Vec<String>, Module> = BTreeMap::new();
    let mut layouts: BTreeMap<Vec<String>, Vec<types::LayoutType>> = BTreeMap::new();
    for (key, module) in modules {
        let s = specialize::specialize(module)?;
        // plans/M7.md item B: the whole-closure half of the same
        // pre-resolution `@layout` pass `check_typed` runs (see
        // `types::check_layouts`). One module at a time — a `@layout`
        // type is a module-local declaration, so nothing here needs the
        // closure. plans/M7.md item D keeps each module's own table on
        // its `TypedProgram` (see `check_typed`'s own note).
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

    // plans/M9.md item E: splice the time prelude names into every module
    // that is not `core.time` itself, without requiring a source import.
    // Explicit `from core.time import ...` wins (entry already present).
    inject_time_prelude_bindings(&mut bindings, &specialized);

    // plans/M9.md item A1: the closure's type-name arity table, read off
    // raw AST on both sides (`imports::closure_type_shapes`) so it is
    // complete before any module's `declare` runs — that is what keeps
    // this item from needing module A's type table finished before module
    // B's can be built, i.e. what keeps import cycles free.
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
        // Same inject for type-position Duration/Instant.
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
        // plans/M10.md item A2c: placed-static rules need declare + layouts.
        types::validate_placed_statics(&decl_items, &layouts[key])?;
        // plans/M7.md item C: the whole-closure half of the claim
        // partitioning check, per module for the same reason
        // `check_layouts` above is — an `Mmio[L]`'s own `L` must be a
        // `@layout(mmio)` struct declared in the *same* module
        // (`types::validate_capability_args` checks that against
        // `declare`'s own module-local table), so a driver's partition
        // never spans the closure.
        types::check_mmio_claims(module, &decl_items, &layouts[key])?;
        decl_items_map.insert(key.clone(), decl_items);
    }

    // plans/M9.md item A1, decision 10: data-vs-resource classification,
    // recomputed over the whole closure now that every module's own
    // `declare` has finished. It runs *here* — after the loop above, before
    // any `ModuleCtx` clones a `DeclStruct`/`DeclEnum` — for exactly the
    // reason the splice below runs where it does: it is a read of
    // already-finished output, so which module imports which does not
    // matter, and no module's `declare` ever waits on another's.
    types::classify_closure(&mut decl_items_map, &imported_targets)?;

    let mut mctxs: BTreeMap<Vec<String>, bodies::ModuleCtx> = BTreeMap::new();
    for (key, module) in &specialized {
        let mctx = bodies::build_module_ctx(module, &decl_items_map[key], &imported_types[key]);
        mctxs.insert(key.clone(), mctx);
    }

    // Splice (order-independent — every mctx above is already fully
    // built, so which module imports which does not matter here).
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
        let (fn_entry, const_entry, const_val_entry, struct_entry, enum_entry) = {
            let src = &mctxs[&target_module];
            (
                src.fns.get(&target_name).cloned(),
                src.consts.get(&target_name).cloned(),
                src.const_values.get(&target_name).cloned(),
                src.structs.get(&target_name).cloned(),
                src.enums.get(&target_name).cloned(),
            )
        };
        let dst = mctxs.get_mut(&key).expect("key is a key of mctxs");
        // plans/M9.md item GG: one simultaneous substitution of every
        // exporter spelling this importer aliased from `target_module`.
        // Applied even when the owning name itself is unaliased (a peer
        // in the signature may still be). Empty when nothing is aliased.
        let subs = imports::alias_subs_for_exporter(&bindings[&key], &target_module);
        if let Some(mut f) = fn_entry {
            types::rekey_decl_fn_names(&mut f.decl, &subs);
            dst.fns.insert(local.clone(), f);
        }
        if let Some(mut c) = const_entry {
            types::rekey_type_names(&mut c, &subs);
            dst.consts.insert(local.clone(), c);
            if let Some(v) = const_val_entry {
                dst.const_values.insert(local.clone(), v);
            }
        }
        if let Some(mut s) = struct_entry {
            // plans/M9.md items DD / GG / decision 9: the map key is the
            // local spelling; every `Type::Named` in the declaration —
            // owner, parameters, returns, fields, generic args — must
            // match the importer's bindings.
            types::rekey_decl_struct_names(&mut s.decl, &subs);
            dst.structs.insert(local.clone(), s);
        }
        if let Some(mut e) = enum_entry {
            // plans/M9.md item B2 / GG: same whole-signature re-key.
            types::rekey_decl_enum_names(&mut e.decl, &subs);
            dst.enums.insert(local, e);
        }
    }

    // plans/M9.md item HH: after the explicit-import splice, close each
    // importer's ModuleCtx over every type reachable through those
    // declarations (pub and non-pub). Field/method lookup on a value the
    // importer already holds needs the DeclStruct present; without this,
    // `b.n` on a `Box` returned by an imported `Maker.build` reports the
    // false diagnostic `type \`Box\` has no field \`n\``. Decision 13
    // stands: these entries live only in the importer's lookup tables,
    // never merged into the exporter's declaration emission set.
    close_mctx_type_reachability(&mut mctxs, &bindings);

    let mut programs: BTreeMap<Vec<String>, typed::TypedProgram> = BTreeMap::new();
    for (key, module) in &specialized {
        let decl_items = &decl_items_map[key];
        let mctx = &mctxs[key];
        let mut program = bodies::check(module, decl_items, mctx)?;
        program.layouts = layouts.get(key).cloned().unwrap_or_default();
        access::check(module, decl_items, mctx)?;
        flow::check(module, decl_items, mctx)?;
        handoff::check(module, decl_items, mctx)?;
        matches::check(module, decl_items, mctx)?;
        let empty_path = String::new();
        let path = paths.get(key).unwrap_or(&empty_path);
        program.instantiations = generics::check(module, decl_items, mctx, path)?;
        programs.insert(key.clone(), program);
    }

    // plans/M9.md item A1b: the *typed* splice — the same read-only reuse
    // of another module's already-finished output the `ModuleCtx` splice
    // above does, one layer down. `bodies::check` only ever fills
    // `TypedProgram::consts`/`fns`/`structs`/`enums` from *this* module's
    // own `module.items`, so the comptime evaluator (`eval::interp`,
    // which is handed one `TypedProgram` and nothing else) could not see
    // an imported declaration at all: constructing an imported struct or
    // reading an imported `const` at comptime abandoned with `internal
    // error: struct/const ... not found`, and an imported enum's variant
    // or an imported fn call abandoned with a named diagnostic that
    // blamed generics. Runs *after* the loop above, for exactly the
    // reason the `ModuleCtx` splice runs where it does: every module's
    // own `bodies::check` needs nothing from any other module, so no
    // module's evaluation waits on another's and import cycles stay free
    // (golden/check-import-comptime-cycle).
    splice_imported_decls(&mut programs, &bindings);

    // The comptime/legality tail, in its own loop over the same modules
    // in the same BTree order — it runs after the splice above because
    // `eval::check_comptime` is the pass the splice exists for. Moving it
    // out of the loop above also makes the closure behave like the
    // single-module pipeline it mirrors: every module finishes a pass
    // before any module starts the next one, so the first diagnostic is
    // the earliest one in *pass* order, then module order (decision 14).
    for (key, module) in &specialized {
        let decl_items = &decl_items_map[key];
        let program = &programs[key];
        crate::eval::check_comptime(program)?;
        // plans/M7.md item A: the whole-closure half of the provenance
        // check, per module for the same reason every pass in this loop
        // is — and, unlike them, with a real consequence, since the
        // callee graph a `CalleeKey` names is module-local. See
        // `eval::legal`'s own provenance section: a capability-touching
        // helper in a module that declares no `@driver` is rejected even
        // if a driver elsewhere calls it, which is the fail-closed
        // direction, and the diagnostic says "in this module".
        crate::eval::legal::check_provenance(
            program,
            &types::capability_authority(module, decl_items),
        )?;
        crate::eval::legal::check_isr_effects(program)?;
        // plans/M7.md item G: same wake/bottom-half checks the
        // single-module path runs. Item E routes every module that
        // mentions `seconds`/`now`/… through this multi-module path, so
        // omitting them here would let `err-wake-outside-isr` pass.
        crate::eval::legal::check_wake_sites(program)?;
        crate::eval::legal::check_bottom_half(program)?;
    }

    // plans/M10.md item A2b: the whole-closure half of the later
    // layout-completion pass (see `check_typed_single_with_decls`, which
    // carries the full note). Its own loop, after the comptime loop above,
    // for that loop's own stated reason — every module finishes a pass before
    // any module starts the next — and because a `const` only has a value
    // once `check_comptime` has run on the module that declares it. Per
    // module, like `check_layouts` itself: a `@layout` type is a module-local
    // declaration, and an imported `const` reaches it through the typed
    // splice above, not through this pass.
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

    // plans/M6.md item G: the whole-closure half of the send proof (see
    // `check_typed` above) — every module is typed by now, which is
    // exactly what "the whole-image count of static send/call sites"
    // needs. Runs once, over all of them, after the per-module loop.
    let by_name: BTreeMap<String, &typed::TypedProgram> =
        programs.iter().map(|(k, p)| (k.join("."), p)).collect();
    send_proof::check(&by_name)?;
    // plans/M7.md item E2: whole-closure half of the reserve proof.
    reserve_proof::check(&by_name)?;

    Ok((
        programs,
        CheckDumpTables {
            decl_items_map,
            imported_types,
        },
    ))
}

/// plans/M9.md item A1b: fills every module's `TypedProgram::imported`
/// from the modules it imports, and — for everything that splice cannot
/// honestly carry — fills `TypedProgram::imported::unresolvable` with the
/// sentence the evaluator prints instead of abandoning with an `internal
/// error:`.
///
/// The splice itself is deliberately narrow: one entry per *import
/// binding*, keyed by the importing module's own local (possibly aliased)
/// spelling, which is the same key the typed tree itself uses (decision
/// 9). Nothing is re-checked, nothing is re-typed, and nothing here
/// requires one module's evaluation to finish before another's can begin
/// — every `TypedProgram` in `programs` is already complete when this
/// runs, so import cycles stay free exactly as they do for the
/// `ModuleCtx` splice one layer up.
///
/// **Decision 15, the fail-closed half.** `eval::interp` walks one
/// `TypedProgram`'s flat name tables and has no notion of which module a
/// body came from, so an *imported body* is evaluated against the
/// importing module's tables. Two consequences, both recorded in
/// `unresolvable` rather than papered over:
///
/// - A name only the exporting module has (a private helper, const, or
///   type) is simply absent from the importer's tables. That is a miss,
///   and a miss must name its real cause.
/// - Worse, a name the exporting module has *and the importing module
///   also resolves differently* would silently resolve to the importer's
///   declaration — a wrong value, not a missing one. So a body-bearing
///   splice (`const`/`fn`/`struct`; an `enum` is a variant-name list with
///   no body and is always safe) from a module that shadows any name with
///   its importer is **withheld**, turning the wrong value back into a
///   named miss.
///
/// Making the evaluator evaluate each body against its own module's
/// program is the real fix and is not A1b's: it changes every
/// `eval::` entry point's signature. A1b's contract is narrower and
/// exact — the declarations a module *names* are evaluable, and
/// everything else says so out loud.
fn splice_imported_decls(
    programs: &mut BTreeMap<Vec<String>, typed::TypedProgram>,
    bindings: &BTreeMap<Vec<String>, imports::ImportBindings>,
) {
    // Every module's own declaration names, and every name it can resolve
    // at all (its own, plus the locals its imports bind). Both are read
    // off the already-finished programs — no new analysis.
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
    // What module `m` means by the name `name`, as a (module, name) pair
    // — its own declaration first, else whatever its imports bound.
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

    // The shadowing witness for each (importer, exporter) pair, if any:
    // the first name (BTree order, so deterministic) the two modules
    // resolve to genuinely different declarations. "Different module" is
    // not enough on its own — `bodies::check` injects the five remaining
    // prelude enums (`Target`, `Restart`, `BootError`, `DriverMode`,
    // `CompletionOutcome`) into *every* module's own
    // `TypedProgram::enums`, so every pair of modules in every build
    // "declares" all five. Those are the same declaration by value, and
    // the evaluator cannot tell them apart either, so value equality is
    // the honest test. (`IoError` left this set at plans/M9.md item A2.)
    let shadow: BTreeMap<(Vec<String>, Vec<String>), String> = {
        let same_decl = |a: &(Vec<String>, String), b: &(Vec<String>, String)| -> bool {
            if a == b {
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
        for (m, bs) in bindings {
            for b in bs.values() {
                let n = &b.target_module;
                if n == m || shadow.contains_key(&(m.clone(), n.clone())) {
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

    // Names some module of the closure declares that `m` cannot resolve
    // at all — reachable when an imported body refers to something
    // private to its own module.
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
        // The exporter's own instantiations come across under the
        // *importer-facing* canonical-key spelling (plans/M9.md item II):
        // bodies are re-keyed under `subs`, and so are the map keys, so a
        // `StructLiteral` typed `Box[Item]` finds `struct:Box[Item]`
        // rather than missing the exporter's `struct:Box[Src]`. Withheld
        // under the same shadowing rule as the bodies they belong to.
        let inst_entries = src.instantiations.clone();
        let body_bearing = const_entry.is_some()
            || fn_entry.is_some()
            || struct_entry.is_some()
            || enum_entry.is_some();
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
            // plans/M9.md item GG: same whole-signature substitution the
            // ModuleCtx splice applies — one map, one simultaneous pass.
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
                // plans/M9.md items DD / GG / decision 9: re-key the typed
                // body under every aliased exporter spelling, so method
                // bodies that name `Self` or a peer type as `Type::Named`
                // resolve under the same keys the splice installed.
                typed::rekey_struct_names(&mut s, &subs);
                dst.imported.structs.insert(local.clone(), s);
            }
            if let Some(mut e) = enum_entry {
                // plans/M9.md item B2 / GG: enums carry methods; re-key
                // like structs.
                typed::rekey_enum_names(&mut e, &subs);
                dst.imported.enums.insert(local.clone(), e);
            }
            for (ikey, mut inst) in inst_entries {
                typed::rekey_instantiation(&mut inst, &subs);
                let new_key = typed::rekey_canonical_key(&ikey, &subs);
                dst.imported.instantiations.entry(new_key).or_insert(inst);
            }
        }
    }

    // plans/M9.md item HH: close the typed import tables the same way
    // `close_mctx_type_reachability` closed ModuleCtx — so comptime
    // construction of a reachable-but-unimported type (GG finding #3's
    // `internal error: struct \`Box\` not found`) resolves.
    close_typed_type_reachability(programs, bindings);

    // Finally the closure-wide "declared elsewhere" notes, under every
    // name the withheld entries above did not already claim — but only
    // for names the reachability closure did not install (plans/M9.md
    // item HH). A reachable type is present in `imported.structs`/
    // `enums`; leaving it in `unresolvable` would make eval prefer the
    // miss note over the real declaration (`abandon_missing`).
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

/// plans/M9.md item HH: close each importer's `ModuleCtx` over types
/// reachable from its already-spliced import bindings. Seeded by the
/// explicit splice above; walks signatures and copies missing
/// struct/enum entries from the **defining** module's finished mctx
/// (not merely the module that re-exported the name — a two-module-deep
/// chain `A→B→C` with only `A` imported must still find `C` in `B`'s
/// module). Pub and non-pub alike — §2's privacy gate is the *import*
/// of a non-pub name, not inference over a value the importer already
/// holds.
fn close_mctx_type_reachability(
    mctxs: &mut BTreeMap<Vec<String>, bodies::ModuleCtx>,
    bindings: &BTreeMap<Vec<String>, imports::ImportBindings>,
) {
    let empty = imports::ImportBindings::new();
    let module_keys: Vec<Vec<String>> = mctxs.keys().cloned().collect();
    for importer in &module_keys {
        let own_bindings = bindings.get(importer).unwrap_or(&empty);
        // local name in importer -> module whose *own* (or further
        // imported) mctx holds the declaration we walk next.
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
                // Prefer the origin module's own table; if the name is
                // only there via *its* imports, chase the defining module
                // so a peer return type declared one hop further still
                // resolves (A→B→C with only A imported).
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
                    dst.enums.insert(tname.clone(), e);
                    origins.insert(tname.clone(), def_module);
                    queue.push(tname);
                }
            }
        }
    }
}

/// plans/M9.md item HH: same reachability closure for `TypedProgram::
/// imported`, so comptime eval and lower find a reachable-but-unimported
/// struct instead of `internal error: struct \`X\` not found`. Runs
/// inside `splice_imported_decls` after the explicit-binding loop. Same
/// defining-module chase as `close_mctx_type_reachability`.
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
        // plans/M9.md item MM: generic templates splice with empty method
        // tables; monomorphized method signatures / bodies (and names
        // like `SlotMapFull` inside `Result[Key, SlotMapFull]` /
        // `EnumConstruct`) live only on instantiations. Install any
        // still-missing names those bodies mention so lower's
        // `variant_index` agrees with the evaluator.
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
                    typed::TypedInstantiation::Enum => {}
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

/// plans/M9.md item E: bind `TIME_PRELUDE_NAMES` from `core.time` into
/// every module that is not `core.time` itself. Explicit imports win.
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

/// Same inject for type-position `Duration`/`Instant` arity.
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

/// The multi-module `--stage=check` dump (plans/M4.md item A): every
/// module's own dump concatenated in `modules`'s own BTree order — an
/// imported name is bound, never declared, so it never appears in *its
/// importer's* own block.
///
/// plans/M9.md item LL: prefer [`check_program_dump`] (threads the tables
/// check used). This entry still re-derives, but injects the time-prelude
/// types the same way check does, and returns `Err` instead of panicking.
pub fn dump_program(modules: &BTreeMap<Vec<String>, Module>) -> Result<String, SemaError> {
    // plans/M9.md item A1: the dump re-derives `specialize`/`declare` the
    // same dumb way `dump` above always has, so it needs the same two
    // whole-closure inputs `check_program_typed` computed — the imported
    // type-name arity table (or a signature naming an imported type would
    // not resolve here at all) and `classify_closure`'s answer (or a
    // struct with an imported resource field would print `data` here and
    // be a resource everywhere else).
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
        // plans/M9.md item E / LL: same inject check_program_typed uses —
        // without it, `: Duration` in type position fails declare here.
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

    /// plans/M9.md item A2: the CLI's `--stage=typed` now loads closures,
    /// but the single-module `check_typed` entry itself must still fail
    /// closed on an import — empty bindings would otherwise misreport the
    /// import as `unknown name`. Pins `unit:check_typed_rejects_imports`.
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
}
