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
pub mod matches;
pub mod paths;
pub mod prelude;
pub mod specialize;
pub mod symbols;
pub mod typed;
pub mod types;

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
    fn at(category: &'static str, message: String, span: Span) -> SemaError {
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
    let specialized = specialize::specialize(module)?;
    let symtab = symbols::collect(&specialized)?;
    symbols::resolve(&specialized, &symtab)?;
    let decl_items = types::declare(&specialized)?;
    let mctx = bodies::build_module_ctx(&specialized, &decl_items);
    let mut program = bodies::check(&specialized, &decl_items, &mctx)?;
    access::check(&specialized, &decl_items, &mctx)?;
    flow::check(&specialized, &decl_items, &mctx)?;
    matches::check(&specialized, &decl_items, &mctx)?;
    program.instantiations = generics::check(&specialized, &decl_items, &mctx, path)?;
    crate::eval::check_comptime(&program)?;
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
