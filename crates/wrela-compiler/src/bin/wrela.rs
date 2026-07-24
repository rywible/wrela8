//! The wrela CLI. Every pipeline stage is reachable as a text dump so the
//! golden suite can pin it: `wrela dump --stage=<stage> <file.wr>`.
//!
//! Dumps print to stdout — including errors, which are themselves stable,
//! golden-testable output. Exit code 0 means "dump produced" (possibly an
//! error dump); nonzero means the CLI itself was misused.
//!
//! `--timings` (ROADMAP.md's compiler measurement lane, M1) adds one more
//! line, to STDERR only, so it never touches a golden: per-phase wall
//! clock time (read, lex, parse, dump, total) in microseconds, measured
//! with `std::time::Instant` at each phase boundary. Dumb on purpose — no
//! sampling, no counters, just a clock a batch pipeline already has phase
//! boundaries for.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};
use std::time::{Duration, Instant};

use wrela_compiler::eval;
use wrela_compiler::layout;
use wrela_compiler::loader;
use wrela_compiler::report;
use wrela_compiler::sema;
use wrela_compiler::sema::typed::{TestKind, TypedProgram};
use wrela_compiler::syntax::ast::Module;
use wrela_compiler::syntax::{lexer, parser, printer};
use wrela_compiler::{codegen, lower};

const USAGE: &str = "usage: wrela dump --stage=<tokens|ast|pretty|check|typed|flowwir|mwir|asm|image|report> [--timings] <file.wr>\n       wrela test <file.wr> [--vmm <path>]\n       wrela build <file.wr> [--out-dir <dir>]\n       wrela version";

/// Renders one `sema::SemaError` exactly the way `print_sema_error` prints
/// it (decision 1's one-line diagnostic, or item H's one multi-line
/// exception, decision 2), as an owned `String` ending in `\n` per line —
/// the shared text both `print_sema_error` (stdout, `dump`/`test`) and
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
    print!("{}", render_sema_error(e));
}

/// Prints one `lexer::LexError`: `error[lex]: <message> at <line>:<col>`.
fn print_lex_error(e: &lexer::LexError) {
    println!("error[lex]: {} at {}:{}", e.message, e.line, e.col);
}

/// Prints one `parser::ParseError`: `error[parse]: <message> at <line>:<col>`.
fn print_parse_error(e: &parser::ParseError) {
    println!("error[parse]: {} at {}:{}", e.message, e.line, e.col);
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
fn run_image_stage(programs: &BTreeMap<String, TypedProgram>) {
    let candidates: Vec<(&String, &String)> = programs
        .iter()
        .filter_map(|(module, p)| p.image_fn.as_ref().map(|f| (module, f)))
        .collect();
    match candidates.len() {
        0 => println!("error[build]: no `@image` fn found in the build closure"),
        1 => {
            let (module, fn_name) = candidates[0];
            let program = &programs[module];
            match eval::interp::eval_image(program, fn_name) {
                Ok(graph) => match eval::image_checks::check_sealed(&graph, program, programs) {
                    Ok(()) => print!("{}", eval::image::dump(&program.enums, &graph)),
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
            println!(
                "error[build]: more than one `@image` fn reachable in the build closure ({})",
                names.join(", ")
            );
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
                            let bytes = std::fs::read(path).map_err(|e| {
                                format!("error[build]: cannot read `{}`: {e}\n", path.display())
                            })?;
                            inputs.push(report::BuildInput {
                                path: report::address_to_relative_path(addr),
                                digest: report::sha256_hex(&bytes),
                            });
                        }
                        match report::render(&inputs, &program.enums, &graph) {
                            Ok(mut text) => {
                                let name = first_field_value(&text, "Name value=")
                                    .unwrap_or("")
                                    .to_string();
                                let target = first_field_value(&text, "Target value=")
                                    .unwrap_or("")
                                    .to_string();
                                let layout_ctx = layout::merge_layout_ctx(modules)
                                    .map_err(|e| render_sema_error(&e))?;
                                let img = match layout::try_layout_program(
                                    programs,
                                    &layout_ctx,
                                    &graph,
                                    modules,
                                ) {
                                    Ok(Some(image_layout)) => {
                                        layout::render_layout_section(&mut text, &image_layout);
                                        Some(image_layout.blob)
                                    }
                                    Ok(None) => None,
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
/// stability). `report::render`'s own decision-10 boundary (a registered
/// `@layout_assert` fails the *report*, never the raw `--stage=image`
/// dump) surfaces here as an ordinary `error[build]` diagnostic, exactly
/// like every other rejection this stage can produce. Plans/M4.md item E:
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
        Err(diag) => print!("{diag}"),
    }
}

fn dump(args: &[String]) -> ExitCode {
    let mut stage = None;
    let mut path = None;
    let mut timings = false;
    for a in args {
        if let Some(s) = a.strip_prefix("--stage=") {
            stage = Some(s.to_string());
        } else if a == "--timings" {
            timings = true;
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
                    Ok(module) if module.imports.is_empty() => match sema::check(&module, &path) {
                        Ok(()) => print!("{}", sema::dump(&module)),
                        Err(e) => print_sema_error(&e),
                    },
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
                            match sema::check_program(&modules, &paths) {
                                Ok(()) => print!("{}", sema::dump_program(&modules)),
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
                    Ok(module) => match sema::check_typed(&module, &path) {
                        Ok(program) => print!("{}", sema::dump_typed(&program)),
                        Err(e) => print_sema_error(&e),
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
        // plans/M5.md item B: the same single-file entry `typed` uses
        // (`check_typed` itself already fails closed on an import-bearing
        // module) — lowers the checked program to `mwir::MwirProgram` and
        // dumps it (`mwir::dump`); a lowering rejection prints in the
        // exact same one-line `error[unimplemented]: ...` house style
        // every other fail-closed stage already uses (`lower::LowerError`
        // carries no location — the typed tree it walks carries none
        // either, decision 1).
        "mwir" => match lex_result {
            Ok(tokens) => {
                let parse_start = Instant::now();
                let parsed = parser::parse(tokens);
                parse_time = parse_start.elapsed();
                let dump_start = Instant::now();
                match parsed {
                    Ok(module) => match sema::check_typed(&module, &path) {
                        Ok(program) => match wrela_compiler::lower::lower_program(&program) {
                            Ok(mwir_program) => {
                                print!("{}", wrela_compiler::mwir::dump(&mwir_program))
                            }
                            Err(e) => println!("error[unimplemented]: {}", e.message),
                        },
                        Err(e) => print_sema_error(&e),
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
        // plans/M6.md item B: the same single-file entry `typed`/`mwir`
        // use — `flowwir_lower::lower_program` walks the checked
        // program's *async* fns/methods only (a sync fn never reaches
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
                    Ok(module) => match sema::check_typed(&module, &path) {
                        Ok(program) => match wrela_compiler::flowwir_lower::lower_program(&program)
                        {
                            Ok(flowwir_program) => {
                                print!("{}", wrela_compiler::flowwir::dump(&flowwir_program))
                            }
                            Err(e) => println!("error[unimplemented]: {}", e.message),
                        },
                        Err(e) => print_sema_error(&e),
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
        // plans/M5.md item C: the same single-file entry `mwir` uses, one
        // stage further — `lower::lower_program` then
        // `mwir::build_layout_ctx` (the one whole-program struct/enum
        // field-type fact codegen needs beyond mwir itself) then
        // `codegen::codegen_program`, dumped via `codegen::dump`. A
        // lowering or codegen rejection prints the same one-line
        // `error[unimplemented]: ...` house style every other fail-closed
        // stage already uses.
        // plans/M6.md item D: one stage further than the sync-only path
        // above still covers — `lower::lower_program` (sync fns/methods,
        // now skipping every `is_async` one) *and*
        // `flowwir_lower::lower_program` (async fns/methods only) each run
        // over the same checked program, then
        // `codegen::codegen_program_with_async` merges both halves into
        // one `CodegenProgram` sharing one rodata pool/dispatch-index
        // table before `codegen::dump` renders it — an async fn's own
        // state-machine form (dispatch header, flattened states,
        // checkpoints) shows up in the identical dump right alongside
        // every sync fn. `actor_method_index_tables` needs a
        // `BTreeMap<String, Module>` the way `layout.rs`'s own multi-module
        // build closure already does; a single-file dump has exactly one
        // module, keyed by its own dotted path (arbitrary — never read
        // back, just a map key).
        "asm" => match lex_result {
            Ok(tokens) => {
                let parse_start = Instant::now();
                let parsed = parser::parse(tokens);
                parse_time = parse_start.elapsed();
                let dump_start = Instant::now();
                match parsed {
                    Ok(module) => match sema::check_typed(&module, &path) {
                        Ok(program) => match wrela_compiler::lower::lower_program(&program) {
                            Ok(mwir_program) => {
                                match wrela_compiler::flowwir_lower::lower_program(&program) {
                                    Ok(flow_program) => {
                                        match wrela_compiler::mwir::build_layout_ctx(&module) {
                                            Ok(layout) => {
                                                let modules: BTreeMap<String, Module> =
                                                    BTreeMap::from([(
                                                        module.path.join("."),
                                                        module.clone(),
                                                    )]);
                                                match layout::actor_method_index_tables(
                                                    &modules, &layout,
                                                ) {
                                                    Ok(method_index) => {
                                                        let group_arena_capacity =
                                                            layout::count_with_group_sites(
                                                                &modules,
                                                            );
                                                        match wrela_compiler::codegen::codegen_program_with_async(
                                                            &mwir_program,
                                                            &flow_program,
                                                            &layout,
                                                            &method_index,
                                                            group_arena_capacity,
                                                        ) {
                                                            Ok(codegen_program) => print!(
                                                                "{}",
                                                                wrela_compiler::codegen::dump(
                                                                    &codegen_program
                                                                )
                                                            ),
                                                            Err(e) => println!(
                                                                "error[unimplemented]: {}",
                                                                e.message
                                                            ),
                                                        }
                                                    }
                                                    Err(e) => println!(
                                                        "error[unimplemented]: {}",
                                                        e.message
                                                    ),
                                                }
                                            }
                                            Err(e) => print_sema_error(&e),
                                        }
                                    }
                                    Err(e) => println!("error[unimplemented]: {}", e.message),
                                }
                            }
                            Err(e) => println!("error[unimplemented]: {}", e.message),
                        },
                        Err(e) => print_sema_error(&e),
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
                // Sema + `@image` evaluation have no phase timer of their
                // own yet, exactly like `check`/`typed` above; both fold
                // into "dump".
                let dump_start = Instant::now();
                match parsed {
                    // plans/M4.md item B: the same single-file/whole-
                    // closure fork `check` above already makes — an
                    // `@image` fn's own module may or may not import
                    // anything else.
                    Ok(module) if module.imports.is_empty() => {
                        match sema::check_typed(&module, &path) {
                            Ok(program) => {
                                let mut programs = BTreeMap::new();
                                programs.insert(module.path.join("."), program);
                                run_image_stage(&programs);
                            }
                            Err(e) => print_sema_error(&e),
                        }
                    }
                    Ok(_) => match loader::load_closure(Path::new(&path)) {
                        Ok(loaded) => {
                            let paths: BTreeMap<Vec<String>, String> = loaded
                                .modules
                                .iter()
                                .map(|(k, m)| (k.clone(), m.file.display().to_string()))
                                .collect();
                            let modules: BTreeMap<Vec<String>, _> = loaded
                                .modules
                                .into_iter()
                                .map(|(k, m)| (k, m.module))
                                .collect();
                            match sema::check_program_typed(&modules, &paths) {
                                Ok(programs) => {
                                    let programs: BTreeMap<String, TypedProgram> = programs
                                        .into_iter()
                                        .map(|(k, p)| (k.join("."), p))
                                        .collect();
                                    run_image_stage(&programs);
                                }
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
                    // The identical single-file/whole-closure fork
                    // `check`/`image` above already make.
                    Ok(module) if module.imports.is_empty() => {
                        match sema::check_typed(&module, &path) {
                            Ok(program) => {
                                let mut programs = BTreeMap::new();
                                let mut file_paths = BTreeMap::new();
                                let mut modules_by_addr = BTreeMap::new();
                                let addr = module.path.join(".");
                                file_paths.insert(addr.clone(), Path::new(&path).to_path_buf());
                                modules_by_addr.insert(addr.clone(), module);
                                programs.insert(addr, program);
                                run_report_stage(&programs, &file_paths, &modules_by_addr);
                            }
                            Err(e) => print_sema_error(&e),
                        }
                    }
                    Ok(_) => match loader::load_closure(Path::new(&path)) {
                        Ok(loaded) => {
                            let paths: BTreeMap<Vec<String>, String> = loaded
                                .modules
                                .iter()
                                .map(|(k, m)| (k.clone(), m.file.display().to_string()))
                                .collect();
                            let file_paths: BTreeMap<String, std::path::PathBuf> = loaded
                                .modules
                                .iter()
                                .map(|(k, m)| (k.join("."), m.file.clone()))
                                .collect();
                            let modules: BTreeMap<Vec<String>, _> = loaded
                                .modules
                                .into_iter()
                                .map(|(k, m)| (k, m.module))
                                .collect();
                            let modules_by_addr: BTreeMap<String, Module> = modules
                                .iter()
                                .map(|(k, m)| (k.join("."), m.clone()))
                                .collect();
                            match sema::check_program_typed(&modules, &paths) {
                                Ok(programs) => {
                                    let programs: BTreeMap<String, TypedProgram> = programs
                                        .into_iter()
                                        .map(|(k, p)| (k.join("."), p))
                                        .collect();
                                    run_report_stage(&programs, &file_paths, &modules_by_addr);
                                }
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
        other => {
            // Fail closed: stages that do not exist yet say so loudly
            // instead of producing a fake dump.
            let dump_start = Instant::now();
            println!("error[unimplemented]: stage `{other}` is not implemented");
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

    ExitCode::SUCCESS
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
/// silent skip; (3) locates and runs the codesigned `wrela-vmm` binary
/// (`find_vmm_binary`) — its absence is itself a named fail-closed line,
/// per the plan's own exact wording; (4) verifies the transcript's own
/// well-formedness (exactly one `test <name>: `-prefixed line per runtime
/// test, in declaration order, plus one trailing summary line) before
/// trusting any of it; (5) prints comptime lines, then the transcript's
/// own per-test lines, then the one merged summary.
fn test_cmd(args: &[String]) -> ExitCode {
    let mut path: Option<String> = None;
    let mut vmm_arg: Option<String> = None;
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
    let program = match sema::check_typed(&module, &path) {
        Ok(p) => p,
        Err(e) => {
            print_sema_error(&e);
            return ExitCode::FAILURE;
        }
    };

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

    let mut modules: BTreeMap<String, Module> = BTreeMap::new();
    modules.insert(module.path.join("."), module.clone());
    let layout_ctx = match layout::merge_layout_ctx(&modules) {
        Ok(c) => c,
        Err(e) => {
            print_sema_error(&e);
            return ExitCode::FAILURE;
        }
    };
    let mwir_program = match lower::lower_program(&program) {
        Ok(p) => p,
        Err(e) => {
            for l in &comptime_lines {
                println!("{l}");
            }
            println!(
                "error[unimplemented]: the runtime test tier could not lower this program: {}",
                e.message
            );
            return ExitCode::FAILURE;
        }
    };
    // plans/M6.md item D: the async half, alongside the sync one above —
    // `flowwir_lower::lower_program` never touches a sync fn (decision 2),
    // so this is additive, never a re-lowering of anything `lower_program`
    // already covers.
    let flow_program = match wrela_compiler::flowwir_lower::lower_program(&program) {
        Ok(p) => p,
        Err(e) => {
            for l in &comptime_lines {
                println!("{l}");
            }
            println!(
                "error[unimplemented]: the runtime test tier could not lower this program's \
                 async fns: {}",
                e.message
            );
            return ExitCode::FAILURE;
        }
    };
    // The real `ImageGraph`, if this file declares an `@image` (plans/M6.md
    // item D: the one necessary step `test_cmd` never took before this
    // item — no actor could ever exist in a runtime-test image otherwise).
    // Absent `@image`, an empty graph (no actors, `compute_runtime_tables`'s
    // own "empty -> None" path) — byte-identical to every pre-M6 `wrela
    // test` golden.
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
    let method_index = match layout::actor_method_index_tables(&modules, &layout_ctx) {
        Ok(m) => m,
        Err(e) => {
            for l in &comptime_lines {
                println!("{l}");
            }
            println!("error[unimplemented]: {}", e.message);
            return ExitCode::FAILURE;
        }
    };
    // plans/M6.md decision 11b: resolve every runtime test's own
    // `Actor[T]` params against this image's own declared instances
    // *before* ever laying out the image — an ambiguity/absence here is
    // exactly the same `error[build]` category `image_checks::check_sealed`
    // already uses for graph-shaped mistakes.
    let test_args = match layout::resolve_runtime_test_args(&program, &runtime_tests, &graph) {
        Ok(a) => a,
        Err(msg) => {
            for l in &comptime_lines {
                println!("{l}");
            }
            println!("error[build]: {msg}");
            return ExitCode::FAILURE;
        }
    };
    let group_arena_capacity = layout::count_with_group_sites(&modules);
    let codegen_program = match codegen::codegen_program_with_async(
        &mwir_program,
        &flow_program,
        &layout_ctx,
        &method_index,
        group_arena_capacity,
    ) {
        Ok(p) => p,
        Err(e) => {
            for l in &comptime_lines {
                println!("{l}");
            }
            println!(
                "error[unimplemented]: the runtime test tier could not compile this program: {}",
                e.message
            );
            return ExitCode::FAILURE;
        }
    };
    // Park-and-resume wiring: each async fn's persistent frame bytes
    // (sizes every turn area) and which runtime tests are async (the
    // entry driver's scheduler loop wraps exactly those — a sync test's
    // return value must never be misread as a TURN_STATUS_* word).
    let async_frames = match codegen::async_frame_sizes(&flow_program, &layout_ctx) {
        Ok(m) => m,
        Err(e) => {
            for l in &comptime_lines {
                println!("{l}");
            }
            println!(
                "error[unimplemented]: the runtime test tier could not size this program's \
                 async frames: {}",
                e.message
            );
            return ExitCode::FAILURE;
        }
    };
    let async_tests: std::collections::BTreeSet<String> = runtime_tests
        .iter()
        .filter(|name| program.fns.get(*name).is_some_and(|f| f.is_async))
        .cloned()
        .collect();
    let group_child_index = match codegen::compute_group_child_indices(&flow_program) {
        Ok(m) => m,
        Err(e) => {
            for l in &comptime_lines {
                println!("{l}");
            }
            println!("error[unimplemented]: {}", e.message);
            return ExitCode::FAILURE;
        }
    };
    let boot = layout::BootCtx {
        graph: &graph,
        modules: &modules,
        layout_ctx: &layout_ctx,
        async_frames: &async_frames,
        group_child_index: &group_child_index,
    };
    let image_layout = match layout::layout_test_image(
        &codegen_program,
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
            println!(
                "error[build]: the runtime test tier could not lay out the test image: {}",
                e.message
            );
            return ExitCode::FAILURE;
        }
    };

    let Some(vmm_path) = find_vmm_binary(vmm_arg.as_deref()) else {
        for l in &comptime_lines {
            println!("{l}");
        }
        println!(
            "error[unimplemented]: the runtime test tier needs the wrela VMM (macOS/HVF at M5)"
        );
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
    let mut report_text = format!(
        "Machine revision={}\nInput path={path} digest={source_digest}\n",
        wrela_machine::MACHINE_REVISION_STR
    );
    for s in &image_layout.sections {
        report_text.push_str(&format!(
            "Section name={} base={:#x} size={}\n",
            s.name, s.base, s.size
        ));
    }
    report_text.push_str(&format!("Entry base={:#x}\n", image_layout.entry));
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
            println!(
                "error[build]: could not run the wrela VMM ({}): {e}",
                vmm_path.display()
            );
            return ExitCode::FAILURE;
        }
    };
    match out.status.code() {
        Some(0) | Some(1) => {}
        _ => {
            for l in &comptime_lines {
                println!("{l}");
            }
            println!(
                "error[build]: the wrela VMM did not boot the test image: {}",
                String::from_utf8_lossy(&out.stderr).trim()
            );
            return ExitCode::FAILURE;
        }
    }

    let transcript = String::from_utf8_lossy(&out.stdout).into_owned();
    let t_lines: Vec<&str> = transcript.lines().collect();
    let well_formed = t_lines.len() == runtime_tests.len() + 1
        && t_lines
            .iter()
            .zip(runtime_tests.iter())
            .all(|(line, name)| line.starts_with(&format!("test {name}: ")));
    let Some((runtime_passed, runtime_failed)) =
        (if well_formed { t_lines.last() } else { None }).and_then(|l| parse_summary_line(l))
    else {
        for l in &comptime_lines {
            println!("{l}");
        }
        println!(
            "error[build]: the wrela VMM's own transcript is not well-formed (expected {} test line(s) then a summary):\n{transcript}",
            runtime_tests.len()
        );
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
/// calls, so the two surfaces can never silently drift apart. Unlike
/// `dump` (exit 0 by convention, even for an error dump — this file's own
/// module doc), `wrela build` exits nonzero on any diagnostic: there is no
/// artifact for a caller relying on a real exit code to inspect. On
/// success it writes `<image-name>.report.txt` next to the root file (or
/// into `--out-dir`, if given) and prints a few fixed summary lines to
/// stdout (decision 8's own "pick dumb facts": the image's name and
/// target, counts of devices/drivers/actors/pools, and the report path
/// itself — printed exactly as derived from the user's own
/// `<file.wr>`/`--out-dir` argument text, never canonicalized or made
/// absolute, so the line stays stable in a pinned golden no matter the
/// invoking directory).
fn build_cmd(args: &[String]) -> ExitCode {
    let mut path = None;
    let mut out_dir: Option<String> = None;
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

    // The identical single-file/whole-closure fork `--stage=check`/`image`/
    // `report` above already make.
    let (programs, file_paths, modules_by_addr): (
        BTreeMap<String, TypedProgram>,
        BTreeMap<String, PathBuf>,
        BTreeMap<String, Module>,
    ) = if module.imports.is_empty() {
        match sema::check_typed(&module, &path) {
            Ok(program) => {
                let addr = module.path.join(".");
                let mut programs = BTreeMap::new();
                let mut file_paths = BTreeMap::new();
                let mut modules_by_addr = BTreeMap::new();
                file_paths.insert(addr.clone(), Path::new(&path).to_path_buf());
                modules_by_addr.insert(addr.clone(), module);
                programs.insert(addr, program);
                (programs, file_paths, modules_by_addr)
            }
            Err(e) => {
                print_sema_error(&e);
                return ExitCode::FAILURE;
            }
        }
    } else {
        match loader::load_closure(Path::new(&path)) {
            Ok(loaded) => {
                let paths: BTreeMap<Vec<String>, String> = loaded
                    .modules
                    .iter()
                    .map(|(k, m)| (k.clone(), m.file.display().to_string()))
                    .collect();
                let file_paths: BTreeMap<String, PathBuf> = loaded
                    .modules
                    .iter()
                    .map(|(k, m)| (k.join("."), m.file.clone()))
                    .collect();
                let modules: BTreeMap<Vec<String>, _> = loaded
                    .modules
                    .into_iter()
                    .map(|(k, m)| (k, m.module))
                    .collect();
                let modules_by_addr: BTreeMap<String, Module> = modules
                    .iter()
                    .map(|(k, m)| (k.join("."), m.clone()))
                    .collect();
                match sema::check_program_typed(&modules, &paths) {
                    Ok(progs) => {
                        let programs: BTreeMap<String, TypedProgram> =
                            progs.into_iter().map(|(k, p)| (k.join("."), p)).collect();
                        (programs, file_paths, modules_by_addr)
                    }
                    Err(e) => {
                        print_sema_error(&e);
                        return ExitCode::FAILURE;
                    }
                }
            }
            Err(loader::LoadError::Lex(e)) => {
                print_lex_error(&e);
                return ExitCode::FAILURE;
            }
            Err(loader::LoadError::Parse(e)) => {
                print_parse_error(&e);
                return ExitCode::FAILURE;
            }
            Err(loader::LoadError::Build(e)) => {
                print_sema_error(&e);
                return ExitCode::FAILURE;
            }
        }
    };

    let r = match build_report(&programs, &file_paths, &modules_by_addr) {
        Ok(r) => r,
        Err(diag) => {
            print!("{diag}");
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
