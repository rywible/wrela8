//! `wrela-vmm <report> <img> [--record <path>] [--replay <path>]` — the
//! one small, codesigned binary that actually calls Hypervisor.framework
//! (plans/M5.md item E: "the binary that needs signing is whichever
//! actually calls HVF ... keeps xtask itself unsigned and the signed
//! surface one small binary"). Prints the captured console transcript to
//! stdout, byte-for-byte, and nothing else; the guest's own reported exit
//! code becomes this process's exit code (`0`/`1`), a VMM-level failure
//! (couldn't boot, malformed report, HVF error, timeout) exits `2` with a
//! diagnostic on stderr — `wrela test`'s own runtime tier (`bin/
//! wrela.rs`) distinguishes the two by this process's own exit code,
//! never by parsing stdout.
//!
//! `--record <path>` (plans/M5.md item F): boots live, then writes the
//! resulting `wrela_vmm::record::RecordFile` (clock log + transcript
//! digest + exit code + exits count, `record.rs`'s own stable text
//! format) to `<path>` — a plain recording, never a golden (nothing here
//! is ever compared against a pinned expectation; only the *format* is
//! unit-tested, in `wrela-vmm`'s own `record.rs`). `--replay <path>`
//! boots again, feeding `<path>`'s own recorded clock log back instead of
//! the live clock, and diagnoses any divergence (06 §8) — exits `3` with
//! every divergence found, one per line, on stderr, rather than folding
//! a determinism finding into the ordinary `0`/`1`/`2` outcomes above.
//! `--record`/`--replay` are mutually exclusive; neither changes the
//! ordinary transcript-to-stdout / exit-code-mirrors-guest contract on
//! the path that does run.

use std::path::PathBuf;
use std::process::ExitCode;

fn usage() -> ! {
    eprintln!("usage: wrela-vmm <report> <img> [--record <path>] [--replay <path>]");
    std::process::exit(2);
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut positional: Vec<String> = Vec::new();
    let mut record_path: Option<PathBuf> = None;
    let mut replay_path: Option<PathBuf> = None;
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--record" => {
                i += 1;
                match args.get(i) {
                    Some(p) => record_path = Some(PathBuf::from(p)),
                    None => usage(),
                }
            }
            "--replay" => {
                i += 1;
                match args.get(i) {
                    Some(p) => replay_path = Some(PathBuf::from(p)),
                    None => usage(),
                }
            }
            other => positional.push(other.to_string()),
        }
        i += 1;
    }
    let [report, img] = positional.as_slice() else {
        usage();
    };
    if record_path.is_some() && replay_path.is_some() {
        eprintln!("wrela-vmm: --record and --replay are mutually exclusive");
        return ExitCode::from(2);
    }
    let report_path = PathBuf::from(report);
    let img_path = PathBuf::from(img);

    if let Some(replay_path) = replay_path {
        let text = match std::fs::read_to_string(&replay_path) {
            Ok(t) => t,
            Err(e) => {
                eprintln!("wrela-vmm: cannot read {}: {e}", replay_path.display());
                return ExitCode::from(2);
            }
        };
        let recorded = match wrela_vmm::record::RecordFile::parse(&text) {
            Ok(r) => r,
            Err(e) => {
                eprintln!(
                    "wrela-vmm: malformed record file {}: {e}",
                    replay_path.display()
                );
                return ExitCode::from(2);
            }
        };
        return match wrela_vmm::record::replay(&report_path, &img_path, &recorded) {
            Ok(divergences) if divergences.is_empty() => ExitCode::SUCCESS,
            Ok(divergences) => {
                eprintln!("wrela-vmm: replay diverged from the recording:");
                for d in &divergences {
                    eprintln!("  {d}");
                }
                ExitCode::from(3)
            }
            Err(e) => {
                eprintln!("wrela-vmm: {e}");
                ExitCode::from(2)
            }
        };
    }

    // Plain boot (below) and `--record` share the identical transcript/
    // exit-code contract — `--record` only *additionally* writes the
    // recorded facts to disk, it never changes what stdout/the exit code
    // mean, so a caller (`xtask bench guest`/`profile`) can pass
    // `--record` unconditionally and still read the transcript exactly
    // as it always has.
    match wrela_vmm::boot_image(&report_path, &img_path) {
        Ok(outcome) => {
            if let Some(record_path) = record_path {
                let recorded = wrela_vmm::record::RecordFile::from_outcome(&outcome);
                if let Err(e) = std::fs::write(&record_path, recorded.to_text()) {
                    eprintln!("wrela-vmm: cannot write {}: {e}", record_path.display());
                    return ExitCode::from(2);
                }
            }
            use std::io::Write;
            let mut stdout = std::io::stdout();
            if stdout.write_all(&outcome.transcript).is_err() {
                return ExitCode::from(2);
            }
            if outcome.exit_code == 0 {
                ExitCode::SUCCESS
            } else {
                ExitCode::FAILURE
            }
        }
        Err(e) => {
            eprintln!("wrela-vmm: {e}");
            ExitCode::from(2)
        }
    }
}
