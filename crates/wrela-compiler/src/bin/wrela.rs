//! The wrela CLI. Every pipeline stage is reachable as a text dump so the
//! golden suite can pin it: `wrela dump --stage=<stage> <file.wr>`.
//!
//! Dumps print to stdout — including errors, which are themselves stable,
//! golden-testable output. Exit code 0 means the dump completed without a
//! diagnostic; nonzero means a diagnostic was printed (lex/parse/sema/
//! build/unimplemented) **or** the CLI itself was misused. Plans/M9.md
//! item NN: the previous "exit 0 even for an error dump" convention lied
//! to every script and CI harness that called `dump`; goldens compare
//! stdout and accept a nonzero exit when the pinned expectation is itself
//! a diagnostic (see `xtask`'s golden runner).
//!
//! `--timings` (ROADMAP.md's compiler measurement lane, M1) adds one more
//! line, to STDERR only, so it never touches a golden: per-phase wall
//! clock time (read, lex, parse, dump, total) in microseconds, measured
//! with `std::time::Instant` at each phase boundary. Dumb on purpose — no
//! sampling, no counters, just a clock a batch pipeline already has phase
//! boundaries for.

use std::cell::Cell;
use std::collections::BTreeMap;
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

const USAGE: &str = "usage: wrela dump --stage=<tokens|ast|pretty|check|typed|layout-types|flowwir|mwir|asm|cost|image|report|rtconfig> [--timings] [--omit-dmb] <file.wr>\n       wrela test <file.wr> [--vmm <path>] [--omit-dmb]\n       wrela build <file.wr> [--out-dir <dir>] [--omit-dmb]\n       wrela version";

// Set by every diagnostic printer while `dump` runs; cleared at the
// start of each `dump` invocation. Plans/M9.md item NN: `dump` exits
// nonzero when this is set. `test`/`build` ignore it (they already
// return `FAILURE` on their own diagnostic paths).
thread_local! {
    static DUMP_HAD_DIAGNOSTIC: Cell<bool> = const { Cell::new(false) };
}

fn note_dump_diagnostic() {
    DUMP_HAD_DIAGNOSTIC.with(|c| c.set(true));
}

/// Renders one `sema::SemaError` exactly the way `print_sema_error` prints
/// it (decision 1's one-line diagnostic, or item H's one multi-line
/// exception, decision 2), as an owned `String` ending in `\n` per line —
/// the shared text both `print_sema_error` (`dump`/`test`) and
/// `build_report`/`build_cmd` (plans/M4.md item E: `wrela build`'s own
/// diagnostic output, which must print in "the exact existing one-line
/// style") need, without printing anything themselves.
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

/// Prints one `sema::SemaError` (decision 1's one-line diagnostic, or
/// item H's one multi-line exception, decision 2): `extra_lines` is
/// empty and `omit_location` is `false` for every ordinary diagnostic, so
/// this reduces to the plain `error[cat]: msg at L:C` line unchanged;
/// the generic-instantiation chain sets both, printing its own already-
/// indented `required by`/`instantiated at` lines below the primary one.
fn print_sema_error(e: &sema::SemaError) {
    eprint!("{}", render_sema_error(e));
    note_dump_diagnostic();
}

/// Prints one `lexer::LexError`: `error[lex]: <message> at <line>:<col>`.
fn print_lex_error(e: &lexer::LexError) {
    eprintln!("error[lex]: {} at {}:{}", e.message, e.line, e.col);
    note_dump_diagnostic();
}

/// Prints one `parser::ParseError`: `error[parse]: <message> at <line>:<col>`.
fn print_parse_error(e: &parser::ParseError) {
    eprintln!("error[parse]: {} at {}:{}", e.message, e.line, e.col);
    note_dump_diagnostic();
}

/// One-line `error[unimplemented]: …` / `error[build]: …` that does not
/// go through `print_sema_error` — still counts as a dump diagnostic
/// (plans/M9.md item NN).
///
/// **Diagnostics go to stderr** (plans/M9.md item RR). Every diagnostic
/// this binary emits funnels through this fn, `print_sema_error`,
/// `print_lex_error` or `print_parse_error`, and all four write to
/// stderr: a stage dump's *output* is stdout, and an error is not output.
/// On stdout, `wrela dump --stage=asm f.wr > f.s` wrote the diagnostic
/// into `f.s` and left stderr empty, so a failed compile produced a
/// plausible-looking artifact and any wrapper watching stderr saw
/// nothing. The pinned diagnostic goldens are unaffected: the golden
/// runner compares stdout followed by stderr, so a case whose whole
/// expectation is one `error[…]` line reads byte-identically either way
/// (see `xtask`'s `golden`).
fn print_line_diagnostic(line: &str) {
    eprintln!("{line}");
    note_dump_diagnostic();
}

/// One fully-checked build closure (plans/M4.md item A's single-file /
/// whole-closure fork). Extended to `--stage=typed` / `mwir` / `flowwir` /
/// `asm` and `wrela test` at plans/M9.md item A2 so an import-bearing
/// module (including `from core...`) is no longer rejected by the
/// single-module entry with a wrong-cause diagnostic.
struct CheckedClosure {
    /// Dotted address of the file the CLI was pointed at.
    root: String,
    programs: BTreeMap<String, TypedProgram>,
    modules: BTreeMap<String, Module>,
}

/// Runs the identical single-file / whole-closure fork `dump --stage=check`
/// already makes: no imports → `check_typed` on one module; any import →
/// `loader::load_closure` then `check_program_typed`. On success the root
/// module's typed program has every imported decl spliced in, so
/// `lower`/`mwir`/`test` over `programs[root]` see the same surface a
/// single-file prelude name used to provide.
///
/// plans/M10.md item A2d / decision 582: a no-import but runtime-bearing
/// root still loads `core.runtime` into the programs map (kept, not
/// discarded like the time-prelude inject). Names stay non-auto-visible
/// (decision 584).
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
        // Runtime-bearing, no user imports: splice `core.runtime` into a
        // two-module closure and keep both TypedPrograms.
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

/// No-import runtime-bearing root: load `core.runtime` (and `core.time`
/// when the root mentions a time-prelude name), check as a multi-module
/// closure, keep root + runtime TypedPrograms (plans/M10.md item A2d).
/// `core.time` is loaded only for the prelude inject then discarded from
/// the returned maps — same discipline as `check_typed_with_time_prelude`
/// — so no-import reports do not gain a surprise `Input path=core/time.wr`.
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
    // plans/M11.md item E / decision 780: stub so runtime.wr's import resolves.
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

/// `wrela dump --stage=image`'s own driver (plans/M4.md item B, grown by
/// item C): `programs` is every module the build closure checked, keyed
/// by its own dotted module path (a single entry for the no-imports
/// case). Decision 6's own "exactly one reachable `@image`" rule: zero or
/// more than one is a named `error[build]` listing every candidate found,
/// `module.path::fn_name` each, in the same BTree order `programs` itself
/// is keyed by (item C's own full form — item B's slice only handled the
/// same-module duplicate, `sema::bodies::check`'s own guard). Exactly one
/// runs on the M3 evaluator (`eval::interp::eval_image`); once it returns
/// a sealed `ImageGraph`, item C's own post-seal pass
/// (`eval::image_checks::check_sealed`) runs before the graph is ever
/// printed — a graph check failure prints its own `error[build]`
/// diagnostic in place of the dump, exactly like every other rejection
/// here. An `EvalError` (a blown quota, `img.dma_pool`'s own fail-closed
/// gap, an unsealed return, ...) still renders through the identical
/// `error[comptime]` path every other comptime failure already uses
/// (`eval::to_sema_error`) — graph checks never run at all in that case,
/// since there is no sealed graph to check.
/// `wrela dump --stage=layout-types`'s own driver (plans/M7.md item B,
/// decision 4 — the plan named the stage `layout-types` with "name TBD in
/// item B"; it is kept, since the artifact is exactly a table of the
/// build closure's `@layout` *types*, and nothing about the name needed
/// improving).
///
/// The one input every `@layout` fact comes from is the *specialized* ast
/// (`sema::types::check_layouts`), so this stage runs the full sema check
/// first — a program that does not check has no layout table worth
/// printing, and its own diagnostic is the honest dump — then re-derives
/// the table from the same specialized module. `check_layouts` is a pure
/// function of that module and already ran (and passed) inside the check
/// above, so the second call cannot fail; it is still handled as a real
/// `Err` rather than unwrapped, because "cannot fail" is a property of
/// today's pass order and not of this call site.
///
/// `modules` is supplied in the caller's own deterministic order: one
/// entry for the single-file fork, or the whole closure in `BTreeMap`
/// key order (the same dotted-address order `--stage=check`'s own
/// multi-module dump concatenates in).
/// `programs` is the same closure's checked programs, keyed by dotted
/// module address — the completion pass's own input (plans/M10.md item A2b):
/// `check_layouts` defers a `const`-length layout, and the *dump* must show
/// the completed sizes, not an absence. A module with no entry here would
/// leave a deferred layout uncompleted, which `dump_layouts` then refuses by
/// name rather than printing a zero.
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
                    Ok(()) => {
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

/// One successful `build_report` outcome: the rendered report text itself
/// plus the handful of already-computed facts `wrela build`'s own stdout
/// summary (plans/M4.md item E, decision 8) prints without re-deriving
/// them a second way — `devices`/`drivers`/`actors`/`pools` are read
/// straight off the sealed `ImageGraph` (still in scope at the point
/// `build_report` renders `text`), while `name`/`target` are read back out
/// of `text` itself (the first `Name value=`/`Target value=` line the
/// report's own section 1/2 already rendered via `eval::image::render_value`
/// — that renderer is `pub(crate)` inside the library crate, unreachable
/// from this binary, and re-deriving the identical string a second way
/// would risk it drifting from what the report file itself says; scraping
/// the one line back out is the dumbest way to guarantee both always
/// agree).
struct BuildReport {
    text: String,
    name: String,
    target: String,
    devices: usize,
    drivers: usize,
    actors: usize,
    pools: usize,
    /// plans/M5.md item D: `Some(bytes)` exactly when this program's own
    /// reachable surface fully lowers/codegens/lays out (`layout::
    /// try_layout_program`'s "all or nothing" rule) — the same condition
    /// that appends the `Layout` section to `text` above. `wrela build`
    /// writes this to `<name>.img`; `--stage=report` never writes a file
    /// at all (dumps never do), so this field is simply unused there.
    img: Option<Vec<u8>>,
}

/// The first line of `text` (after trimming leading indentation) that
/// starts with `prefix`, with `prefix` itself stripped — used to scrape
/// `Name value=`/`Target value=` back out of an already-rendered report
/// (see `BuildReport`'s own doc comment for why).
fn first_field_value<'a>(text: &'a str, prefix: &str) -> Option<&'a str> {
    text.lines()
        .find_map(|line| line.trim_start().strip_prefix(prefix))
}

/// The shared pipeline tail `wrela dump --stage=report` (`run_report_stage`,
/// below) and `wrela build` (`build_cmd`, plans/M4.md item E) both run once
/// `programs`/`file_paths`/`modules` exist, whichever single-file/whole-
/// closure fork produced them: decision 6's one-reachable-`@image`
/// discovery, `eval_image`, item C's `check_sealed`, the input digests,
/// `report::render` itself, and (plans/M5.md item D, new) an attempt to
/// lower/codegen/lay out the same `@image`-owning module's own program —
/// `layout::try_layout_program`'s "all or nothing" rule (see that fn's own
/// doc comment): `Some(image_layout)` appends the `Layout` section to the
/// rendered text and carries the emitted blob back for `wrela build` to
/// write; `None` (this program's reachable surface does not fully lower)
/// leaves the report exactly as M4 left it — no `Layout` section, no
/// image — for both callers alike (`modules`, keyed the same dotted-
/// address way as `programs`/`file_paths`, supplies every raw `ast::Module`
/// in the closure so `layout::merge_layout_ctx` can compute struct/enum
/// field types across every file, not just the one holding `@image` — a
/// project case may spread a spliced-in struct's own declaration across a
/// different file entirely). `Ok` carries the rendered report text plus
/// the stdout-summary facts (`BuildReport`); `Err` carries one already
/// fully rendered diagnostic string (in the exact one-line `error[cat]:
/// msg at L:C` house style, trailing `\n` included, extra lines already
/// appended) — every rejection this pipeline can produce, whichever stage
/// it came from, printed exactly the same way `dump`'s other stages
/// already print it.
fn build_report(
    programs: &BTreeMap<String, TypedProgram>,
    file_paths: &BTreeMap<String, std::path::PathBuf>,
    modules: &BTreeMap<String, Module>,
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
                    Ok(()) => {
                        let mut inputs = Vec::with_capacity(file_paths.len());
                        for (addr, path) in file_paths {
                            let rel = report::address_to_relative_path(addr);
                            // plans/M10.md item A2d / decision 667: omit
                            // auto-injected `core.runtime` from BuildInput
                            // unless some module explicitly imported it —
                            // keeps ~83 boot reports byte-identical on the
                            // load. Explicit import stays honest.
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
                            // plans/M11.md item E / decision 780: stub /
                            // live `core.__image_runtime` is not a disk
                            // file — digest is inserted after layout via
                            // `rtconfig::insert_generated_input_line`.
                            if rel == rtconfig::GENERATED_INPUT_PATH
                                || addr == rtconfig::MODULE_ADDR
                                || path.to_string_lossy() == rtconfig::GENERATED_INPUT_PATH
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
                        // plans/M8.md item B: placement needs the same
                        // LayoutCtx layout itself uses (state / mailbox
                        // bytes), including generic instantiations
                        // (`BlkDriver[DriverMode.Irq]` — decision 18).
                        let mut layout_ctx =
                            layout::merge_layout_ctx(modules).map_err(|e| render_sema_error(&e))?;
                        layout::enrich_layout_ctx_with_instantiations(&mut layout_ctx, programs);
                        let placement = placement::place(&graph, modules, &layout_ctx, graph.cores)
                            .map_err(|e| format!("error[build]: {e}\n"))?;
                        // Own + imported enums: an `@image` module that
                        // only *imports* `Feature` (etc.) still needs
                        // variant names for `required_features=` report
                        // lines — otherwise render falls back to `Enum.?`
                        // (plans/M16.md item D1 / drivers.blk import).
                        let mut enum_variants: BTreeMap<String, Vec<String>> = BTreeMap::new();
                        for (k, e) in program.enums.iter().chain(program.imported.enums.iter()) {
                            enum_variants
                                .entry(k.clone())
                                .or_insert_with(|| e.variants.clone());
                        }
                        match report::render(&inputs, &enum_variants, &graph, &placement) {
                            Ok(mut text) => {
                                let name = first_field_value(&text, "Name value=")
                                    .unwrap_or("")
                                    .to_string();
                                let target = first_field_value(&text, "Target value=")
                                    .unwrap_or("")
                                    .to_string();
                                // plans/M7.md item B: the exact-bytes
                                // section (03-hardware.md §3's own "the
                                // compiler reports"), appended between
                                // `report::render`'s own sections and the
                                // M5 memory map below — declaration facts
                                // before emission facts. Every module in
                                // the closure is walked in `BTreeMap` key
                                // order, and `check_layouts` already ran
                                // (and passed) for each of them inside the
                                // sema check that produced `programs`, so
                                // neither call here can fail; both are
                                // still handled as real errors rather than
                                // unwrapped.
                                //
                                // plans/M10.md item A2b: and then completed —
                                // a `const` array length is resolved by the
                                // later pass, and the report must show the
                                // completed size or refuse. Each module's own
                                // checked program supplies its `const`s.
                                let mut layout_types = Vec::new();
                                for (key, module) in modules {
                                    // plans/M11.md item E / decision 780: stub
                                    // layouts are not user source; live
                                    // `Input path=<generated>` carries the
                                    // digest. Exact-bytes layouts for RT
                                    // overlays arrive via import reachability
                                    // into `core.runtime` when present.
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
                                let img = match layout::try_layout_with_codegen(
                                    programs,
                                    &layout_ctx,
                                    &graph,
                                    modules,
                                ) {
                                    Ok(Some((image_layout, codegen))) => {
                                        // plans/M11.md item D: generate the
                                        // facts-only config from the live
                                        // RuntimeTables (post-stripe, with
                                        // rings), batch-2 typecheck, and
                                        // record the digest in the report.
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
                                        // plans/M18.md item R: short Cost
                                        // summary after layout (owners
                                        // only). Same CodegenProgram layout
                                        // already built — no second lower.
                                        // Missing table fails the build.
                                        report::append_cost_summary(&mut text, &codegen).map_err(
                                            |e| {
                                                if e.ends_with('\n') {
                                                    format!("error[build]: {e}")
                                                } else {
                                                    format!("error[build]: {e}\n")
                                                }
                                            },
                                        )?;
                                        // plans/M9.md item H: run registered
                                        // `@layout_assert` fns against a real
                                        // ImageReport after layout (04 §8).
                                        eval::layout_assert::run(program, &graph, &image_layout)?;
                                        Some(image_layout.blob)
                                    }
                                    Ok(None) => {
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
                                    Err(e) => {
                                        return Err(format!("error[build]: layout: {e}\n"));
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

/// `wrela dump --stage=report`'s own driver (plans/M4.md item D): the
/// identical one-`@image`-in-the-closure discovery as `run_image_stage`
/// above, kept as its own small duplicate rather than refactored out from
/// underneath item C's own already-golden-pinned behavior (CLAUDE.md:
/// "prefer long obvious files over deep indirection; keep behavior
/// local"). Once a sealed, checked `ImageGraph` exists, this renders the
/// versioned report artifact (`report::render`) instead of the raw graph
/// dump: `file_paths` (module address -> the real file the loader/single-
/// file path read it from) is this stage's own extra input beyond
/// `run_image_stage`'s — read straight off disk and hashed with
/// `report::sha256_hex`, one `report::BuildInput` per file, keyed by the
/// package-root-relative path `report::address_to_relative_path` derives
/// from the module's own address (never the real path in `file_paths`
/// itself, which can be absolute or working-directory-relative —
/// `report.rs`'s own module doc explains why that would break byte-
/// stability). Registered `@layout_assert` fns run after layout
/// (`eval::layout_assert`); a failure surfaces here as an ordinary
/// `error[build]` diagnostic, exactly like every other rejection this
/// stage can produce. Plans/M4.md item E:
/// this now just calls the shared `build_report` (above, also used by
/// `wrela build`/`build_cmd`) and prints whichever of its two outcomes
/// came back — zero behavior change from before the refactor, since both
/// outcomes are already fully rendered, trailing-newline-included text.
fn run_report_stage(
    programs: &BTreeMap<String, TypedProgram>,
    file_paths: &BTreeMap<String, std::path::PathBuf>,
    modules: &BTreeMap<String, Module>,
) {
    match build_report(programs, file_paths, modules) {
        Ok(r) => print!("{}", r.text),
        Err(diag) => {
            eprint!("{diag}");
            note_dump_diagnostic();
        }
    }
}

/// `wrela dump --stage=rtconfig` (plans/M11.md item D): generate the
/// facts-only `core.__image_runtime` module from the laid-out image's
/// `RuntimeTables` and print it. Same `@image` discovery as report.
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
            eval::image_checks::check_sealed(&graph, program, programs)
                .map_err(|e| render_sema_error(&e))?;
            let mut layout_ctx =
                layout::merge_layout_ctx(modules).map_err(|e| render_sema_error(&e))?;
            layout::enrich_layout_ctx_with_instantiations(&mut layout_ctx, programs);
            match layout::try_layout_program(programs, &layout_ctx, &graph, modules) {
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

/// Report/build/image load: delegates to [`check_closure`], then adapts
/// to the `(programs, file_paths, modules)` triple. `file_paths` is the
/// checked root plus `core.runtime` when present (same as the former
/// runtime-bearing branch); import-bearing closures also recover every
/// module's real file from `load_closure` so report digests stay complete.
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

fn dump(args: &[String]) -> ExitCode {
    // plans/M9.md item NN: every diagnostic printer sets this; clear at
    // the start of each dump so a prior `test`/`build` call in-process
    // cannot leak a stale bit (the CLI is one-shot today, but the rule
    // is cheap and local).
    DUMP_HAD_DIAGNOSTIC.with(|c| c.set(false));
    // plans/M15.md item K: reset the mutation front-door every invocation.
    wrela_compiler::codegen::set_omit_dmb(false);

    let mut stage = None;
    let mut path = None;
    let mut timings = false;
    let mut omit_dmb = false;
    for a in args {
        if let Some(s) = a.strip_prefix("--stage=") {
            stage = Some(s.to_string());
        } else if a == "--timings" {
            timings = true;
        } else if a == "--omit-dmb" {
            // plans/M15.md item K / decision 1098: test-only barrier
            // deletion. Only the xtask golden runner passes this.
            omit_dmb = true;
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
    wrela_compiler::codegen::set_omit_dmb(omit_dmb);

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

    // parse_time/dump_time default to zero and are filled in only for the
    // phases that actually run for this stage/outcome (e.g. `tokens` runs
    // no real parse unless --timings asks for the extra measurement below;
    // a lex error skips both). Printing the phase itself — even an error
    // line — counts as "dump" per this CLI's own doc comment: dumps are
    // stable output whether they are a tree or a diagnostic.
    let mut parse_time = Duration::ZERO;
    let dump_time;

    match stage.as_str() {
        "tokens" => match lex_result {
            Ok(tokens) => {
                // No real parse is needed to dump tokens, but --timings
                // reports all four phases uniformly, so time a throwaway
                // parse purely for measurement when it was asked for.
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
                // Sema has no phase timer of its own yet (ROADMAP.md:
                // measurement only lands with a profile); its time folds
                // into "dump" here, exactly like every other stage's
                // artifact-production step.
                let dump_start = Instant::now();
                match parsed {
                    // plans/M4.md item A: a module with no imports keeps
                    // the exact single-file path (byte-identical to
                    // before this item — the hard constraint every
                    // existing goldens depends on); a module with any
                    // import loads its whole closure through the loader
                    // instead, since resolving even one import needs the
                    // package root and every module it reaches.
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
        // plans/M7.md item B: every `@layout` type in the build closure,
        // laid out and printed — same `check_closure` fork as typed/mwir.
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
                // Sema (typed production included) has no phase timer of
                // its own yet, exactly like the `check` stage above; its
                // time folds into "dump".
                let dump_start = Instant::now();
                match parsed {
                    // plans/M9.md item A2: same single-file/whole-closure
                    // fork as `check` — an import-bearing module (stdlib
                    // included) dumps every module's typed program in
                    // BTree order, each prefixed with `Module path=`.
                    Ok(module) => match check_closure(&path, module) {
                        Ok(checked) => {
                            // Single-file dumps stay byte-identical to the
                            // pre-A2 shape (no `Module path=` prefix) —
                            // every existing typed golden depends on it.
                            // A multi-module closure prefixes each program
                            // so a stdlib-defined name is visible in the
                            // dump (golden/check-stdlib-loaded).
                            // plans/M10.md item A2d / decision 667: after
                            // omitting auto-injected `core.runtime` (and
                            // `core.time`), a singleton root keeps the
                            // no-prefix shape.
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
                                // plans/M11.md item E / decision 780: omit
                                // generated config from typed dumps.
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
        // plans/M5.md item B: the same single-file/whole-closure fork
        // `check`/`typed` use — lowers the *root* module's typed program
        // to `mwir::MwirProgram` and dumps it (`mwir::dump`). Imported
        // decls are already spliced into the root (plans/M9.md item A2),
        // so a stdlib enum construction lowers with no prelude table.
        // A lowering rejection prints in the exact same one-line
        // `error[unimplemented]: ...` house style every other fail-closed
        // stage already uses (`lower::LowerError` carries no location —
        // the typed tree it walks carries none either, decision 1).
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
        // plans/M6.md item B: the same single-file/whole-closure fork
        // `typed`/`mwir` use — `flowwir_lower::lower_program` walks the
        // root module's *async* fns/methods only (a sync fn never reaches
        // this path at all, decision 2's own hard constraint: it keeps
        // the exact M5 `typed` -> `mwir` path above) and dumps the
        // resulting state machines via `flowwir::dump`. A lowering
        // rejection prints the same one-line `error[unimplemented]: ...`
        // house style every other fail-closed stage already uses.
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
        // plans/M5.md item C / M6.md item D / M10.md item A2d: lower the
        // whole closure (sync + async) with `guest_reachable_keys_closure`
        // so force-rooted `core.runtime` helpers enter the emit set —
        // root-only lower never sees them. Builds a whole-closure
        // LayoutCtx and dumps the merged codegen program.
        // plans/M18.md items D+E: `--stage=cost` shares this pipeline, then
        // scores with `wrela-cost-v1` instead of printing asm text.
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
                                                    // M10 D: specialized enqueue keys when an
                                                    // `@image` graph is available.
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
                                                            match wrela_compiler::cost::load_default()
                                                            {
                                                                Ok(table) => {
                                                                    match wrela_compiler::cost::dump(
                                                                        &codegen_program,
                                                                        &table,
                                                                    ) {
                                                                        Ok(text) => print!("{text}"),
                                                                        Err(e) => {
                                                                            print_line_diagnostic(
                                                                                &format!(
                                                                                    "error[unimplemented]: {e}"
                                                                                ),
                                                                            )
                                                                        }
                                                                    }
                                                                }
                                                                Err(e) => print_line_diagnostic(
                                                                    &format!(
                                                                        "error[unimplemented]: {e}"
                                                                    ),
                                                                ),
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
        "image" => match lex_result {
            Ok(tokens) => {
                let parse_start = Instant::now();
                let parsed = parser::parse(tokens);
                parse_time = parse_start.elapsed();
                // Sema + `@image` evaluation have no phase timer of their
                // own yet, exactly like `check`/`typed` above; both fold
                // into "dump".
                let dump_start = Instant::now();
                match parsed {
                    // plans/M4.md item B / M10.md item A2d: single-file,
                    // runtime-bearing singleton, or whole-closure.
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
                // Sema + `@image` evaluation + report rendering have no
                // phase timer of their own yet, exactly like `image`
                // above; everything folds into "dump".
                let dump_start = Instant::now();
                match parsed {
                    // The identical single-file / runtime-bearing /
                    // whole-closure fork `image` above already makes.
                    Ok(module) => match load_build_closure(&path, module) {
                        Ok((programs, file_paths, modules_by_addr)) => {
                            run_report_stage(&programs, &file_paths, &modules_by_addr);
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
        // plans/M11.md item D / decision 701: generated facts-only config.
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
            // Fail closed: stages that do not exist yet say so loudly
            // instead of producing a fake dump.
            let dump_start = Instant::now();
            print_line_diagnostic(&format!(
                "error[unimplemented]: stage `{other}` is not implemented"
            ));
            dump_time = dump_start.elapsed();
        }
    }

    let total_time = total_start.elapsed();

    if timings {
        // STDERR only: goldens compare stdout, so this line never touches
        // pinned output no matter what stage or outcome ran above.
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

/// Parses `eval::run_tests`'s own pinned final line, `"<N> passed, <M>
/// failed"` — the one piece of that stable text format this binary needs
/// to pick apart (never to reinterpret; `eval::run_tests` still owns the
/// wording) so the comptime and runtime tiers can be merged into one
/// summary (plans/M5.md decision 1).
fn parse_summary_line(line: &str) -> Option<(usize, usize)> {
    let rest = line.strip_suffix(" failed")?;
    let (p, f) = rest.split_once(" passed, ")?;
    Some((p.parse().ok()?, f.parse().ok()?))
}

/// Locates the codesigned `wrela-vmm` binary the runtime tier shells out
/// to (plans/M5.md item E: "`wrela` the compiler must not shell to
/// cargo"). `--vmm <path>` (explicit, always wins — `xtask` passes the
/// build+signed path for goldens); otherwise a `wrela-vmm` binary sitting
/// right next to this executable's own directory (the natural place for
/// it once both binaries are installed together). `None` when neither
/// exists — the caller's own fail-closed path, never a silent skip.
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

/// `wrela test <file.wr> [--vmm <path>]` (plans/M3.md item E, decision 9;
/// grown by plans/M5.md item E, decision 1, into the runtime tier). Runs
/// sema (`check_typed`) then every `@test` fn's own report line
/// (`eval::run_tests`, which owns the whole pinned comptime-tier report
/// format). A lex/parse/sema failure prints that stage's own diagnostic
/// — exactly the `dump --stage=check`/`--stage=typed` house style — and
/// exits nonzero without ever printing a test report at all: there is no
/// checked program yet to run `@test` fns out of.
///
/// When the program declares no `@test(runtime)` fns, this is byte-
/// identical to the pre-M5 behavior (every existing `wrela test` golden
/// stays pinned, untouched): `eval::run_tests`'s own text, printed
/// verbatim, `ExitCode::FAILURE` iff any line is `FAILED`.
///
/// When it declares one or more: decision 1's own merge rule — "all
/// comptime lines first, then all runtime lines, then the one summary
/// line" — is built here, since `eval::run_tests` itself (read-only,
/// `eval/`) always treats a `TestKind::Runtime` test as an automatic,
/// fixed-wording `FAILED` line folded into its own summary (the correct
/// behavior for a build with *no* runtime tier at all, decision 9's own
/// pre-M5 fallback). This fn instead: (1) recovers the *comptime-only*
/// lines and counts by stripping every one of those fixed placeholder
/// lines back out of `run_tests`'s own text (`comptime_passed` is
/// `run_tests`'s own total — a runtime test never contributes there;
/// `comptime_failed` is the total minus exactly `runtime_tests.len()`,
/// since each contributes exactly one placeholder failure, unconditionally);
/// (2) lowers/codegens/lays out the whole program as one test image
/// (`layout::layout_test_image`) — a lowering/codegen failure here fails
/// closed with a named `error[unimplemented]` line, never a panic or a
/// silent skip; (2b) runs `eval::image_checks::check_sealed` over this
/// file's own sealed `ImageGraph`, when it declares an `@image` at all —
/// the same graph checks `--stage=image`/`--stage=report`/`wrela build`
/// run, on the path that actually boots the thing (see the inline comment
/// at the `graph` binding below for why they belong here too);
/// (3) locates and runs the codesigned `wrela-vmm` binary
/// (`find_vmm_binary`) — its absence is itself a named fail-closed line,
/// per the plan's own exact wording; (4) verifies the transcript's own
/// well-formedness (exactly one `test <name>: `-prefixed line per runtime
/// test, in declaration order, plus one trailing summary line) before
/// trusting any of it; (5) prints comptime lines, then the transcript's
/// own per-test lines, then the one merged summary.
fn test_cmd(args: &[String]) -> ExitCode {
    // plans/M15.md item K: reset the mutation front-door every invocation.
    wrela_compiler::codegen::set_omit_dmb(false);

    let mut path: Option<String> = None;
    let mut vmm_arg: Option<String> = None;
    let mut omit_dmb = false;
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
            // plans/M15.md item K / decision 1098: test-only barrier
            // deletion for the mutated arm of
            // boot-cross-core-publish-acquire. Only xtask golden passes it.
            omit_dmb = true;
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
    // plans/M9.md item A2: same single-file/whole-closure fork as
    // `dump --stage=check` / `wrela build` — an import-bearing root
    // (including `from core.io_error import IoError`) is checked as a
    // closure, and tests run against the root module's typed program
    // (imports already spliced in).
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
        // Byte-identical to every pre-M5 `wrela test` golden.
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
    // plans/M9.md item MM: same enrich dump `--stage=asm` already does —
    // without it, codegen for a guest that uses an imported generic
    // (`List[u64, 2]`, `SlotMap[u64, 2]`) fails closed on sizing.
    layout::enrich_layout_ctx_with_instantiations(&mut layout_ctx, &checked.programs);
    let layout_ctx = layout_ctx;
    // The real `ImageGraph`, if this file declares an `@image` (plans/M6.md
    // item D). Evaluated *before* lower so plans/M7.md item E1 can stamp
    // `capacity_sectors=` onto the TypedProgram `read_capacity_sectors`
    // lowers from — a build constant, not a register.
    // The graph checks (`eval::image_checks::check_sealed`) run here too,
    // on exactly the sealed graph this command is about to boot.
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
    if program.image_fn.is_some() {
        if let Err(e) = eval::image_checks::check_sealed(&graph, &program, &checked.programs) {
            print_sema_error(&e);
            return ExitCode::FAILURE;
        }
    }
    // One-check → one-lower: swap live rtconfig before the single
    // CodegenProgram (`layout::lower_and_codegen_image`).
    let async_tests: std::collections::BTreeSet<String> = runtime_tests
        .iter()
        .filter(|name| program.fns.get(*name).is_some_and(|f| f.is_async))
        .cloned()
        .collect();
    let test_args = match layout::resolve_runtime_test_args(&program, &runtime_tests, &graph) {
        Ok(a) => a,
        Err(msg) => {
            for l in &comptime_lines {
                println!("{l}");
            }
            print_line_diagnostic(&format!("error[build]: {msg}"));
            return ExitCode::FAILURE;
        }
    };
    let compiled = match layout::lower_and_codegen_image(
        &modules,
        &checked.programs,
        &layout_ctx,
        &graph,
        &runtime_tests,
        &async_tests,
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
    let boot = layout::BootCtx {
        graph: &graph,
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
    // plans/M7.md item E1: BlkDevice/BlkQueue from configure + pools.
    {
        if let Err(e) = layout::attach_blk_report(&mut image_layout, &graph, &compiled.programs) {
            for l in &comptime_lines {
                println!("{l}");
            }
            print_line_diagnostic(&format!("error[build]: {}", e.message));
            return ExitCode::FAILURE;
        }
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
    // Structured VMM report → `wrela_machine::report::render` (shared schema
    // with the VMM parser). Ring lines are still appended separately:
    // `RequestRing` is lossy vs the live `Ring kind=…` spelling.
    let mut parsed = layout::parsed_runtime_tail(&image_layout);
    parsed.entry = image_layout.entry;
    parsed.image_sha256 = image_digest;
    parsed.input_digests = vec![(path.clone(), source_digest)];
    parsed.exec_sections = image_layout
        .sections
        .iter()
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
    // plans/M7.md item H1: a *boot* failure is its own shape. An `assert`
    // inside a declared actor's or driver's `init` aborts before any test
    // line is opened (06-machine.md §3 step 3: initialization runs before
    // the event loops), so the transcript is one `FAILED <message>` line
    // followed by the summary and no test lines at all — image-fatal with
    // a diagnosable line, which is exactly plans/M6.md decision 12's and
    // plans/M7.md decision 8's story for a driver fault. Until this
    // commit that abort branched through an unwritten continuation word
    // and faulted the guest at `pc=0x0` instead
    // (`layout::build_entry_driver`'s own note); recognizing the shape
    // here is the reporting half of the same fix.
    let boot_failed = t_lines.len() == 2 && t_lines[0].starts_with("FAILED ");
    let well_formed = boot_failed
        || (t_lines.len() == runtime_tests.len() + 1
            && t_lines
                .iter()
                .zip(runtime_tests.iter())
                .all(|(line, name)| line.starts_with(&format!("test {name}: "))));
    let Some((runtime_passed, runtime_failed)) =
        (if well_formed { t_lines.last() } else { None }).and_then(|l| parse_summary_line(l))
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
    for l in &t_lines[..t_lines.len() - 1] {
        println!("{l}");
    }
    let passed = comptime_passed + runtime_passed;
    let failed = comptime_failed + runtime_failed;
    println!("{passed} passed, {failed} failed");
    if failed > 0 {
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    }
}

/// `wrela build <file.wr> [--out-dir <dir>]` (plans/M4.md item E, decision
/// 11): the CLI surface `dump --stage=report` never was — a real "compile"
/// entry point rather than a dump-and-inspect one. Loads the closure (the
/// identical single-file/whole-closure fork every `dump` stage above
/// already makes), checks it, then reuses `build_report`'s own shared
/// pipeline tail (one-`@image` discovery, `eval_image`, `check_sealed`, the
/// input digests, `report::render`) — the same function `run_report_stage`
/// calls, so the two surfaces can never silently drift apart. Like
/// `dump` after plans/M9.md item NN, `wrela build` exits nonzero on any
/// diagnostic: there is no artifact for a caller relying on a real exit
/// code to inspect. On
/// success it writes `<image-name>.report.txt` next to the root file (or
/// into `--out-dir`, if given) and prints a few fixed summary lines to
/// stdout (decision 8's own "pick dumb facts": the image's name and
/// target, counts of devices/drivers/actors/pools, and the report path
/// itself — printed exactly as derived from the user's own
/// `<file.wr>`/`--out-dir` argument text, never canonicalized or made
/// absolute, so the line stays stable in a pinned golden no matter the
/// invoking directory).
fn build_cmd(args: &[String]) -> ExitCode {
    // plans/M15.md item K: reset the mutation front-door every invocation.
    wrela_compiler::codegen::set_omit_dmb(false);

    let mut path = None;
    let mut out_dir: Option<String> = None;
    let mut omit_dmb = false;
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
            // plans/M15.md item K / decision 1098: test-only front-door.
            omit_dmb = true;
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

    // The identical single-file / runtime-bearing / whole-closure fork
    // `--stage=image`/`report` above already make.
    let (programs, file_paths, modules_by_addr) = match load_build_closure(&path, module) {
        Ok(v) => v,
        Err(()) => return ExitCode::FAILURE,
    };

    let r = match build_report(&programs, &file_paths, &modules_by_addr) {
        Ok(r) => r,
        Err(diag) => {
            eprint!("{diag}");
            return ExitCode::FAILURE;
        }
    };

    // "next to the root file (or --out-dir)": the directory text is taken
    // verbatim from whichever the caller gave — `--out-dir`'s own argument
    // string if present, else `<file.wr>`'s own parent directory component
    // — never canonicalized or resolved to an absolute path (decision 11's
    // own stability requirement, this fn's own doc comment above).
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

    // plans/M5.md item D: `<name>.img`, next to the report, exactly when
    // `r.img` is `Some` (the program's own reachable surface fully lowers
    // — `layout::try_layout_program`'s "all or nothing" rule). No new
    // stdout line here on purpose: every existing `build.txt` golden's own
    // pinned stdout stays exactly 3 lines (house rule — only the four M4
    // report goldens may move, and build.txt is not one of them); the
    // image's own presence/bytes are golden-covered separately, by
    // comparing the written file itself (`xtask`'s `golden` runner, "img"
    // expectation).
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

    println!("build: name={} target={}", r.name, r.target);
    println!(
        "build: devices={} drivers={} actors={} pools={}",
        r.devices, r.drivers, r.actors, r.pools
    );
    println!("build: report written to {report_path_str}");
    ExitCode::SUCCESS
}
