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
//! Item A lands collect + resolve and the builtin prelude (prelude.rs);
//! every later pass is a stub (a no-op) until its own item lands, so
//! `check` below only calls what item A actually implements.

pub mod access;
pub mod bodies;
pub mod flow;
pub mod generics;
pub mod handoff;
pub mod imports;
pub mod matches;
pub mod paths;
pub mod prelude;
pub mod reserve_proof;
pub mod send_proof;
pub mod specialize;
pub mod symbols;
pub mod typed;
pub mod types;

use std::collections::BTreeMap;

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
    // plans/M7.md item C, 03-hardware.md §2: "Minting a layout consumes
    // those byte ranges from the claim; two live layouts can never alias a
    // register." Runs here because it needs both halves — `declare`'s
    // resolved field types and `@driver` facts, and `check_layouts`' own
    // byte table — and before any body is typed, so an aliasing partition
    // is rejected at the declaration that created it rather than at
    // whichever access happened to be checked first.
    types::check_mmio_claims(&specialized, &decl_items, &layouts)?;
    let mctx = bodies::build_module_ctx(&specialized, &decl_items);
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
    Ok(program)
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
/// item B graduates it to full resolved signatures). Only call this
/// after `check` returns `Ok` — `specialize`/`declare` are re-run here
/// (dumb, no state threaded from `check`) and the result unwrapped,
/// since success is already guaranteed by the caller's contract.
/// plans/M3.md item D: specializing first (exactly like `check_typed`)
/// means this dump shows only the selected branch of any `comptime if`
/// — the golden-visible surface the M3-D task names explicitly.
pub fn dump(module: &Module) -> String {
    let specialized =
        specialize::specialize(module).expect("dump is only called after check returns Ok");
    let decl_items =
        types::declare(&specialized).expect("dump is only called after check returns Ok");
    let effects = access::infer_effects(&specialized, &decl_items);
    let mut out = format!("Module path={}\n", specialized.path.join("."));
    types::render_items(&decl_items, &effects, &mut out);
    out
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
/// This item's own scope line: an imported `const`/`fn` is fully usable
/// as a *value* (called, referenced) via the splice above, and an
/// imported `struct`/`enum` is fully usable as a value too (constructed,
/// field-accessed) via the same mechanism — but `types::declare`'s own
/// type-name table stays module-local, so writing an imported
/// `struct`/`enum` name as an explicit *type annotation* (a fn
/// param/return, a struct field, a `const`'s own declared type) still
/// fails with the ordinary `error[type]: unknown type` (a real, narrow,
/// reported gap: see this module's own report for the full reasoning,
/// not a silent wrong answer either way).
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

    let mut decl_items_map: BTreeMap<Vec<String>, Vec<types::DeclItem>> = BTreeMap::new();
    let mut mctxs: BTreeMap<Vec<String>, bodies::ModuleCtx> = BTreeMap::new();
    for (key, module) in &specialized {
        symbols::resolve(module, &symtabs[key], &bindings[key])?;
        let decl_items = types::declare(module)?;
        // plans/M7.md item C: the whole-closure half of the claim
        // partitioning check, per module for the same reason
        // `check_layouts` above is — an `Mmio[L]`'s own `L` must be a
        // `@layout(mmio)` struct declared in the *same* module
        // (`types::validate_capability_args` checks that against
        // `declare`'s own module-local table), so a driver's partition
        // never spans the closure.
        types::check_mmio_claims(module, &decl_items, &layouts[key])?;
        let mctx = bodies::build_module_ctx(module, &decl_items);
        decl_items_map.insert(key.clone(), decl_items);
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
        if let Some(f) = fn_entry {
            dst.fns.insert(local.clone(), f);
        }
        if let Some(c) = const_entry {
            dst.consts.insert(local.clone(), c);
            if let Some(v) = const_val_entry {
                dst.const_values.insert(local.clone(), v);
            }
        }
        if let Some(s) = struct_entry {
            dst.structs.insert(local.clone(), s);
        }
        if let Some(e) = enum_entry {
            dst.enums.insert(local, e);
        }
    }

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
        crate::eval::check_comptime(&program)?;
        // plans/M7.md item A: the whole-closure half of the provenance
        // check, per module for the same reason every pass in this loop
        // is — and, unlike them, with a real consequence, since the
        // callee graph a `CalleeKey` names is module-local. See
        // `eval::legal`'s own provenance section: a capability-touching
        // helper in a module that declares no `@driver` is rejected even
        // if a driver elsewhere calls it, which is the fail-closed
        // direction, and the diagnostic says "in this module".
        crate::eval::legal::check_provenance(
            &program,
            &types::capability_authority(module, decl_items),
        )?;
        crate::eval::legal::check_isr_effects(&program)?;
        programs.insert(key.clone(), program);
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

    Ok(programs)
}

/// The multi-module `--stage=check` dump (plans/M4.md item A): every
/// module's own dump (`dump` above, unchanged), concatenated in
/// `modules`'s own BTree order — an imported name is bound, never
/// declared, so it never appears in *its importer's* own block, exactly
/// like the single-module dump already never mentions an import
/// statement at all.
pub fn dump_program(modules: &BTreeMap<Vec<String>, Module>) -> String {
    let mut out = String::new();
    for module in modules.values() {
        out.push_str(&dump(module));
    }
    out
}
