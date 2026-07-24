//! `wrela-vmm <report> <img> [--record <path>] [--replay <path>]` — the
//! one small, codesigned binary that actually calls Hypervisor.framework
//! (plans/M5.md item E: "the binary that needs signing is whichever
//! actually calls HVF ... keeps xtask itself unsigned and the signed
//! surface one small binary"). Prints the captured console transcript to
//! stdout, byte-for-byte, and nothing else.
//!
//! ## The exit-code contract (fail-closed; a caller — `xtask`, CI,
//! `wrela test`'s runtime tier — must be able to trust `$?` alone, never
//! parse stdout/stderr to learn whether this process succeeded)
//!
//! Two disjoint sources, never conflated:
//!
//! - **Guest-authored** (`0`/`1`, plain boot and `--record` alike): the
//!   guest's own reported outcome (`machine_info::OFF_EXIT_CODE`, the
//!   trapping `EXIT_MMIO_ADDR` store's own value) collapses to this
//!   process's exit code as `0` (guest reported `0`) or `1` (any other
//!   guest value, however large — an image with no `@test(runtime)` fn at
//!   all halts with the M5-D placeholder code `0xE0000001`
//!   (`layout::EXIT_CODE_NO_RUNTIME`), which is guest data, not a VMM
//!   failure, and still collapses to the ordinary `1` here; the *raw*
//!   guest value is never lost — it survives byte-for-byte in a
//!   `--record`ed `RecordFile::exit_code` and in the guest's own
//!   `machine_info::OFF_EXIT_CODE` memory dump, just not as this
//!   process's own exit code, which is deliberately coarsened to `0`/`1`
//!   the same way a Unix shell's own `$?` coarsens a signal-killed
//!   process). Successful `--replay` (no divergence) reflects this
//!   identical `0`/`1` contract — replaying is otherwise indistinguishable
//!   from a plain boot from the caller's point of view.
//! - **Runner-authored** (`EXIT_VMM_FAILURE = 2` / `EXIT_REPLAY_DIVERGENCE
//!   = 3`, below): a fact about *this process*, never the guest's — `2`
//!   is any failure that prevented an ordinary boot/record/replay outcome
//!   from ever being reached at all (a malformed report, a bad image, an
//!   HVF error, a timeout, an unreadable/malformed `--replay <path>` log,
//!   an unwritable `--record <path>` destination); `3` is `--replay`'s own
//!   determinism finding (`record::replay`'s own non-empty `Divergence`
//!   list) — the boot itself completed, but disagreed with the recording,
//!   every divergence printed by name on stderr. Both are always outside
//!   `0..=1`, so a caller can tell "the guest ran (successfully or not)"
//!   from "this process itself failed to produce a trustworthy answer" by
//!   a single range check, and a diverged replay can never be mistaken
//!   for a successful one.
//!
//! `--record <path>` (plans/M5.md item F, grown by plans/M6.md item E
//! into the choice-sequence shape): boots live, then writes the
//! resulting `wrela_vmm::record::RecordFile` (the ordered choice
//! sequence — clock reads, deadline wakes, vector raises — plus
//! transcript digest, exit code, exits count; `record.rs`'s own stable,
//! versioned `ChoiceLog v1` text format) to `<path>` — a plain recording,
//! never a golden (nothing here is ever compared against a pinned
//! expectation; only the *format* is unit-tested, in `wrela-vmm`'s own
//! `record.rs`). A write failure (a bad directory, no permission) exits
//! `EXIT_VMM_FAILURE` with a diagnostic on stderr — the live boot itself
//! already succeeded, but the recording could not be persisted, which is
//! exactly as untrustworthy an outcome as a boot that never ran, never
//! silently downgraded to "well, at least stdout is right."
//!
//! `--replay <path>` boots again, feeding `<path>`'s own recorded choice
//! sequence back through `record::Chooser::choose_next` instead of the
//! live clock/sleep, and diagnoses any divergence (06 §8) — exits
//! `EXIT_REPLAY_DIVERGENCE` with every divergence found, one per line, on
//! stderr, rather than folding a determinism finding into the ordinary
//! `0`/`1`/`EXIT_VMM_FAILURE` outcomes above (a caller checking only `$?
//! == 0` must never see success on a failed replay). `--record`/
//! `--replay` are mutually exclusive; neither changes the ordinary
//! transcript-to-stdout contract on the path that does run.

use std::path::PathBuf;
use std::process::ExitCode;

/// Runner-authored: this process itself could not produce a trustworthy
/// answer (bad report/image, HVF error, timeout, I/O failure, a malformed
/// or unreadable `--replay` log, an unwritable `--record` destination) —
/// never the guest's own outcome (module doc's own "guest-authored" vs
/// "runner-authored" distinction).
const EXIT_VMM_FAILURE: u8 = 2;

/// Runner-authored: `--replay` completed but disagreed with its own
/// recording (`record::replay`'s own non-empty `Divergence` list) — a
/// determinism finding, distinct from both an ordinary guest outcome
/// (`0`/`1`) and a bare VMM failure (`EXIT_VMM_FAILURE`), so a caller
/// checking `$? == 0` can never mistake a diverged replay for a
/// successful one.
const EXIT_REPLAY_DIVERGENCE: u8 = 3;

fn usage() -> ! {
    eprintln!("usage: wrela-vmm <report> <img> [--record <path>] [--replay <path>]");
    std::process::exit(EXIT_VMM_FAILURE as i32);
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
        return ExitCode::from(EXIT_VMM_FAILURE);
    }
    let report_path = PathBuf::from(report);
    let img_path = PathBuf::from(img);

    if let Some(replay_path) = replay_path {
        let text = match std::fs::read_to_string(&replay_path) {
            Ok(t) => t,
            Err(e) => {
                eprintln!("wrela-vmm: cannot read {}: {e}", replay_path.display());
                return ExitCode::from(EXIT_VMM_FAILURE);
            }
        };
        let recorded = match wrela_vmm::record::RecordFile::parse(&text) {
            Ok(r) => r,
            Err(e) => {
                eprintln!(
                    "wrela-vmm: malformed record file {}: {e}",
                    replay_path.display()
                );
                return ExitCode::from(EXIT_VMM_FAILURE);
            }
        };
        return match wrela_vmm::record::replay(&report_path, &img_path, &recorded) {
            // Guest-authored (module doc above): a clean (non-diverging)
            // replay mirrors the identical `0`/`1` contract a plain boot
            // uses — never unconditionally `SUCCESS`, a real bug this
            // item's own verification pass caught (a replay of a guest
            // that reported nonzero previously still exited `0`). Zero
            // divergence guarantees `recorded.exit_code` already equals
            // this replay's own actual outcome (no `ExitCodeMismatch` was
            // found), so it is the value to collapse from — `record::replay`
            // itself never hands back the fresh `BootOutcome` a second
            // time.
            Ok(divergences) if divergences.is_empty() => {
                if recorded.exit_code == 0 {
                    ExitCode::SUCCESS
                } else {
                    ExitCode::FAILURE
                }
            }
            Ok(divergences) => {
                eprintln!("wrela-vmm: replay diverged from the recording:");
                for d in &divergences {
                    eprintln!("  {d}");
                }
                // Fail-closed (verification's own finding, plans/M6.md item
                // E): a caller must never see `$? == 0` on a diverged
                // replay — `ExitCode::from(EXIT_REPLAY_DIVERGENCE)` is
                // returned here (never conflated with the ordinary `0`/`1`
                // guest-outcome range or silently coerced to `SUCCESS` by
                // any later refactor); the process-level contract is
                // pinned by `tests::replay_divergence_and_record_failures_exit_nonzero_through_the_real_binary`
                // (`lib.rs`) and `xtask`'s own `repro_replay_exit_code_contract`.
                ExitCode::from(EXIT_REPLAY_DIVERGENCE)
            }
            Err(e) => {
                eprintln!("wrela-vmm: {e}");
                ExitCode::from(EXIT_VMM_FAILURE)
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
                    return ExitCode::from(EXIT_VMM_FAILURE);
                }
            }
            use std::io::Write;
            let mut stdout = std::io::stdout();
            if stdout.write_all(&outcome.transcript).is_err() {
                return ExitCode::from(EXIT_VMM_FAILURE);
            }
            // Guest-authored (module doc above): the guest's own reported
            // `machine_info::OFF_EXIT_CODE` value collapses to `0`/`1`
            // here — an image with no runtime tests at all (a plain
            // `wrela build` image) halts with the M5-D placeholder
            // `EXIT_CODE_NO_RUNTIME = 0xE0000001`, which is guest data,
            // not a VMM failure, and lands in the ordinary `else` arm
            // below (`1`) exactly like any other nonzero guest outcome —
            // the raw value still survives byte-for-byte in a
            // `--record`ed `RecordFile::exit_code`.
            if outcome.exit_code == 0 {
                ExitCode::SUCCESS
            } else {
                ExitCode::FAILURE
            }
        }
        Err(e) => {
            eprintln!("wrela-vmm: {e}");
            ExitCode::from(EXIT_VMM_FAILURE)
        }
    }
}
