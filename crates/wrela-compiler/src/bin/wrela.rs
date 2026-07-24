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
use std::path::Path;
use std::process::ExitCode;
use std::time::{Duration, Instant};

use wrela_compiler::eval;
use wrela_compiler::loader;
use wrela_compiler::sema;
use wrela_compiler::sema::typed::TypedProgram;
use wrela_compiler::syntax::{lexer, parser, printer};

const USAGE: &str = "usage: wrela dump --stage=<tokens|ast|pretty|check|typed|image> [--timings] <file.wr>\n       wrela test <file.wr>\n       wrela version";

/// Prints one `sema::SemaError` (decision 1's one-line diagnostic, or
/// item H's one multi-line exception, decision 2): `extra_lines` is
/// empty and `omit_location` is `false` for every ordinary diagnostic, so
/// this reduces to the plain `error[cat]: msg at L:C` line unchanged;
/// the generic-instantiation chain sets both, printing its own already-
/// indented `required by`/`instantiated at` lines below the primary one.
fn print_sema_error(e: &sema::SemaError) {
    if e.omit_location {
        println!("error[{}]: {}", e.category, e.message);
    } else {
        println!(
            "error[{}]: {} at {}:{}",
            e.category, e.message, e.line, e.col
        );
    }
    for line in &e.extra_lines {
        println!("{line}");
    }
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
