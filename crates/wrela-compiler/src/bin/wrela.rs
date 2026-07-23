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

use std::process::ExitCode;
use std::time::{Duration, Instant};

use wrela_compiler::sema;
use wrela_compiler::syntax::{lexer, parser, printer};

const USAGE: &str = "usage: wrela dump --stage=<tokens|ast|pretty|check> [--timings] <file.wr>\n       wrela version";

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
        _ => {
            eprintln!("{USAGE}");
            ExitCode::FAILURE
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
                println!("error[lex]: {} at {}:{}", e.message, e.line, e.col);
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
                    Err(e) => println!("error[parse]: {} at {}:{}", e.message, e.line, e.col),
                }
                dump_time = dump_start.elapsed();
            }
            Err(e) => {
                let dump_start = Instant::now();
                println!("error[lex]: {} at {}:{}", e.message, e.line, e.col);
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
                    Err(e) => println!("error[parse]: {} at {}:{}", e.message, e.line, e.col),
                }
                dump_time = dump_start.elapsed();
            }
            Err(e) => {
                let dump_start = Instant::now();
                println!("error[lex]: {} at {}:{}", e.message, e.line, e.col);
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
                    Ok(module) => match sema::check(&module) {
                        Ok(()) => print!("{}", sema::dump(&module)),
                        Err(e) => {
                            println!(
                                "error[{}]: {} at {}:{}",
                                e.category, e.message, e.line, e.col
                            )
                        }
                    },
                    Err(e) => println!("error[parse]: {} at {}:{}", e.message, e.line, e.col),
                }
                dump_time = dump_start.elapsed();
            }
            Err(e) => {
                let dump_start = Instant::now();
                println!("error[lex]: {} at {}:{}", e.message, e.line, e.col);
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
