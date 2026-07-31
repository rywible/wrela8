//! **Census of corpus sema per-block classifications** (plans/M9.md
//! items J1b/J1c/J3).
//!
//! Pins live in `tests/census.toml` (`[[corpus_sema.pins]]`); this
//! module exposes them for `xtask corpus` verification.

pub use wrela_compiler::census::CorpusSemaPin;

/// Pinned corpus-sema classifications from `tests/census.toml`.
pub fn pins() -> &'static [CorpusSemaPin] {
    &wrela_compiler::census::data().corpus_sema_pins
}
