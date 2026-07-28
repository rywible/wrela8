//! Process-level exit codes for the `wrela-vmm` binary (and callers that
//! must mirror them — `xtask repro`, VMM unit tests). Guest-authored
//! outcomes stay `0`/`1`; these two are runner-authored only.
//!
//! Normative framing lives in `wrela-vmm`'s `main.rs` module doc: a
//! caller checking `$?` alone must never mistake a diverged replay for a
//! successful guest boot.

/// Runner-authored: this process itself could not produce a trustworthy
/// answer (bad report/image, HVF error, timeout, I/O failure, a malformed
/// or unreadable `--replay` log, an unwritable `--record` destination).
pub const EXIT_VMM_FAILURE: i32 = 2;

/// Runner-authored: `--replay` completed but disagreed with its own
/// recording — a determinism finding, distinct from both an ordinary
/// guest outcome (`0`/`1`) and a bare VMM failure (`EXIT_VMM_FAILURE`).
pub const EXIT_REPLAY_DIVERGENCE: i32 = 3;
