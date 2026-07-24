//! `wrela-vmm <report> <img>` — the one small, codesigned binary that
//! actually calls Hypervisor.framework (plans/M5.md item E: "the binary
//! that needs signing is whichever actually calls HVF ... keeps xtask
//! itself unsigned and the signed surface one small binary"). Prints the
//! captured console transcript to stdout, byte-for-byte, and nothing
//! else; the guest's own reported exit code becomes this process's exit
//! code (`0`/`1`), a VMM-level failure (couldn't boot, malformed report,
//! HVF error, timeout) exits `2` with a diagnostic on stderr — `wrela
//! test`'s own runtime tier (`bin/wrela.rs`) distinguishes the two by
//! this process's own exit code, never by parsing stdout.

use std::path::PathBuf;
use std::process::ExitCode;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let [report, img] = args.as_slice() else {
        eprintln!("usage: wrela-vmm <report> <img>");
        return ExitCode::from(2);
    };
    let report_path = PathBuf::from(report);
    let img_path = PathBuf::from(img);

    match wrela_vmm::boot_image(&report_path, &img_path) {
        Ok(outcome) => {
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
