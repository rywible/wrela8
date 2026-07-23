//! The wrela CLI. Every pipeline stage is reachable as a text dump so the
//! golden suite can pin it: `wrela dump --stage=<stage> <file.wr>`.
//!
//! Dumps print to stdout — including errors, which are themselves stable,
//! golden-testable output. Exit code 0 means "dump produced" (possibly an
//! error dump); nonzero means the CLI itself was misused.

use std::process::ExitCode;

use wrela_compiler::syntax::{lexer, parser};

const USAGE: &str = "usage: wrela dump --stage=<tokens|ast> <file.wr>\n       wrela version";

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
    for a in args {
        if let Some(s) = a.strip_prefix("--stage=") {
            stage = Some(s.to_string());
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
    let source = match std::fs::read_to_string(&path) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("error: cannot read {path}: {e}");
            return ExitCode::FAILURE;
        }
    };
    match stage.as_str() {
        "tokens" => {
            match lexer::lex(&source) {
                Ok(tokens) => print!("{}", lexer::dump(&tokens)),
                Err(e) => println!("error[lex]: {} at {}:{}", e.message, e.line, e.col),
            }
            ExitCode::SUCCESS
        }
        "ast" => {
            match lexer::lex(&source) {
                Ok(tokens) => match parser::parse(tokens) {
                    Ok(module) => print!("{}", parser::dump(&module)),
                    Err(e) => println!("error[parse]: {} at {}:{}", e.message, e.line, e.col),
                },
                Err(e) => println!("error[lex]: {} at {}:{}", e.message, e.line, e.col),
            }
            ExitCode::SUCCESS
        }
        other => {
            // Fail closed: stages that do not exist yet say so loudly
            // instead of producing a fake dump.
            println!("error[unimplemented]: stage `{other}` is not implemented");
            ExitCode::SUCCESS
        }
    }
}
