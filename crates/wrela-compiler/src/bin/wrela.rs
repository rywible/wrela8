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
use std::process::ExitCode;
use std::time::{Duration, Instant};

use wrela_compiler::eval;
use wrela_compiler::loader;
use wrela_compiler::report;
use wrela_compiler::sema;
use wrela_compiler::sema::typed::TypedProgram;
use wrela_compiler::syntax::{lexer, parser, printer};

const USAGE: &str = "usage: wrela dump --stage=<tokens|ast|pretty|check|typed|mwir|image|report> [--timings] <file.wr>\n       wrela test <file.wr>\n       wrela build <file.wr> [--out-dir <dir>]\n       wrela version";

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
/// `programs`/`file_paths` exist, whichever single-file/whole-closure fork
/// produced them: decision 6's one-reachable-`@image` discovery, `eval_image`,
/// item C's `check_sealed`, the input digests, and `report::render` itself.
/// `Ok` carries the rendered report text plus the stdout-summary facts
/// (`BuildReport`); `Err` carries one already fully rendered diagnostic
/// string (in the exact one-line `error[cat]: msg at L:C` house style,
/// trailing `\n` included, extra lines already appended) — every rejection
/// this pipeline can produce, whichever stage it came from, printed exactly
/// the same way `dump`'s other stages already print it.
fn build_report(
    programs: &BTreeMap<String, TypedProgram>,
    file_paths: &BTreeMap<String, std::path::PathBuf>,
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
                            Ok(text) => {
                                let name = first_field_value(&text, "Name value=")
                                    .unwrap_or("")
                                    .to_string();
                                let target = first_field_value(&text, "Target value=")
                                    .unwrap_or("")
                                    .to_string();
                                Ok(BuildReport {
                                    devices: graph.devices.len(),
                                    drivers: graph.drivers.len(),
                                    actors: graph.actors.len(),
                                    pools: graph.pools.len(),
                                    text,
                                    name,
                                    target,
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
) {
    match build_report(programs, file_paths) {
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
                                let addr = module.path.join(".");
                                file_paths.insert(addr.clone(), Path::new(&path).to_path_buf());
                                programs.insert(addr, program);
                                run_report_stage(&programs, &file_paths);
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
                            match sema::check_program_typed(&modules, &paths) {
                                Ok(programs) => {
                                    let programs: BTreeMap<String, TypedProgram> = programs
                                        .into_iter()
                                        .map(|(k, p)| (k.join("."), p))
                                        .collect();
                                    run_report_stage(&programs, &file_paths);
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

/// `wrela test <file.wr>` (plans/M3.md item E, decision 9): runs sema
/// (`check_typed`) then every `@test` fn's own report line
/// (`eval::run_tests`, which owns the whole pinned report format).
/// A lex/parse/sema failure prints that stage's own diagnostic — exactly
/// the `dump --stage=check`/`--stage=typed` house style — and exits
/// nonzero without ever printing a test report at all: there is no
/// checked program yet to run `@test` fns out of. `ExitCode::FAILURE`
/// whenever any test's own line is `FAILED` (`run_tests`'s own second
/// return value) — the report itself is still the complete, stable dump
/// either way (decision 9: never fail-fast across tests).
fn test_cmd(args: &[String]) -> ExitCode {
    let (Some(path), true) = (args.first(), args.len() == 1) else {
        eprintln!("{USAGE}");
        return ExitCode::FAILURE;
    };
    let source = match std::fs::read_to_string(path) {
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
    let program = match sema::check_typed(&module, path) {
        Ok(p) => p,
        Err(e) => {
            print_sema_error(&e);
            return ExitCode::FAILURE;
        }
    };
    let (report, any_failed) = wrela_compiler::eval::run_tests(&program);
    print!("{report}");
    if any_failed {
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
    let (programs, file_paths): (BTreeMap<String, TypedProgram>, BTreeMap<String, PathBuf>) =
        if module.imports.is_empty() {
            match sema::check_typed(&module, &path) {
                Ok(program) => {
                    let addr = module.path.join(".");
                    let mut programs = BTreeMap::new();
                    let mut file_paths = BTreeMap::new();
                    file_paths.insert(addr.clone(), Path::new(&path).to_path_buf());
                    programs.insert(addr, program);
                    (programs, file_paths)
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
                    match sema::check_program_typed(&modules, &paths) {
                        Ok(progs) => {
                            let programs: BTreeMap<String, TypedProgram> =
                                progs.into_iter().map(|(k, p)| (k.join("."), p)).collect();
                            (programs, file_paths)
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

    let r = match build_report(&programs, &file_paths) {
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

    println!("build: name={} target={}", r.name, r.target);
    println!(
        "build: devices={} drivers={} actors={} pools={}",
        r.devices, r.drivers, r.actors, r.pools
    );
    println!("build: report written to {report_path_str}");
    ExitCode::SUCCESS
}
