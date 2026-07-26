//! **Census of corpus `--sema` per-block classifications** (plans/M9.md
//! items J1b/J1c).
//!
//! Patterned on `guest_fn_key_census` / `internal_error_census`: a count
//! of disagreements is gameable (ok blocks can all decay while a gate that
//! only checks `disagreements == 0` stays green). This table pins each
//! parseable doc block's classification so a human reading the file sees
//! which fences genuinely typecheck, and so an `ok` → anything decay
//! fails `cargo xtask check`.
//!
//! Update deliberately after reviewing a live `cargo xtask corpus --sema`
//! report. The disagreement *count* is not a gate here (J3); classification
//! drift is. J1c's `assert_fragment_items_preserved` is a separate
//! fail-closed guard (fence text must appear in the wrap).

/// `(repo-relative loc, kind)` where `loc` is `path:start_line` as printed
/// by the `--sema` report, and `kind` is `ok` or `disagreement`.
///
/// Measured 2026-07-25 (J1b; unchanged under J1c): 14 ok, 10 disagreements
/// out of 24. J2a (2026-07-25): aspirational `virtio_storage.wr` scoped
/// out of corpus-sema (lex/parse only) after the hyphen/underscore path
/// fix; §9.5 `read` → `read_file` (+ bare `await`) — 15 ok, 8
/// disagreements out of 23. The J1 "noise" keyhole is gone — every
/// former noise block has a context stub in `corpus_sema_context.rs` or
/// is counted as a disagreement.
pub const CORPUS_SEMA_CENSUS: &[(&str, &str)] = &[
    ("docs/language/02-language.md:43", "ok"),
    ("docs/language/02-language.md:150", "ok"),
    ("docs/language/02-language.md:185", "ok"),
    ("docs/language/02-language.md:206", "disagreement"),
    ("docs/language/02-language.md:250", "ok"),
    ("docs/language/02-language.md:314", "ok"),
    ("docs/language/02-language.md:331", "ok"),
    ("docs/language/02-language.md:348", "ok"),
    ("docs/language/02-language.md:370", "disagreement"),
    ("docs/language/02-language.md:396", "ok"),
    ("docs/language/02-language.md:410", "ok"),
    ("docs/language/02-language.md:458", "ok"),
    ("docs/language/02-language.md:473", "disagreement"),
    ("docs/language/02-language.md:530", "disagreement"),
    ("docs/language/02-language.md:604", "disagreement"),
    ("docs/language/02-language.md:656", "disagreement"),
    ("docs/language/02-language.md:674", "ok"),
    ("docs/language/02-language.md:870", "ok"),
    ("docs/language/03-hardware.md:51", "ok"),
    ("docs/language/03-hardware.md:85", "disagreement"),
    ("docs/language/03-hardware.md:180", "disagreement"),
    ("docs/language/03-hardware.md:224", "ok"),
    ("docs/language/03-hardware.md:258", "ok"),
];
