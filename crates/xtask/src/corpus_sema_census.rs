//! **Census of corpus sema per-block classifications** (plans/M9.md
//! items J1b/J1c/J3).
//!
//! Patterned on `guest_fn_key_census` / `internal_error_census`: a count
//! of disagreements is gameable (ok blocks can all decay while a gate that
//! only checks `disagreements == 0` stays green). This table pins each
//! parseable doc block's classification so a human reading the file sees
//! which fences genuinely typecheck, and so drift fails `cargo xtask
//! check` in both directions (J3): a new disagreement fails; an accepted
//! disagreement that starts typechecking also fails — loudly, naming the
//! cited ledger gap so the pin and the gap list cannot silently diverge.
//!
//! Accepted disagreements carry the ledger gap id that owns them. J3's
//! verifier checks each cited id exists in `ledger/ledger.toml` and is
//! still `status = "gap"`. Update deliberately after reviewing a live
//! `cargo xtask corpus --sema` report; when a gap closes, un-accept the
//! row in the same commit.
//!
//! J1c's `assert_fragment_items_preserved` is a separate fail-closed
//! guard (fence text must appear in the wrap).

/// One pinned corpus-sema classification.
///
/// `gap` is `Some(ledger clause id)` exactly when `kind == "disagreement"`,
/// and `None` when `kind == "ok"`. The verifier rejects any other shape.
#[derive(Clone, Copy)]
pub struct CorpusSemaPin {
    /// `path:start_line` as printed by the `--sema` report.
    pub loc: &'static str,
    /// `"ok"` or `"disagreement"`.
    pub kind: &'static str,
    /// Ledger gap owning an accepted disagreement; `None` for ok rows.
    pub gap: Option<&'static str>,
}

/// Measured 2026-07-25 through J2d: **18 ok, 5 disagreements** out of 23
/// parseable-for-sema blocks. The five disagreements are deliberate gaps
/// (J2b rulings / decision 523); J3 gates on this pin, not on zero.
pub const CORPUS_SEMA_CENSUS: &[CorpusSemaPin] = &[
    CorpusSemaPin {
        loc: "docs/language/02-language.md:43",
        kind: "ok",
        gap: None,
    },
    CorpusSemaPin {
        loc: "docs/language/02-language.md:150",
        kind: "ok",
        gap: None,
    },
    CorpusSemaPin {
        loc: "docs/language/02-language.md:185",
        kind: "ok",
        gap: None,
    },
    CorpusSemaPin {
        loc: "docs/language/02-language.md:206",
        kind: "disagreement",
        gap: Some("values.regions.two-binding-disciplines"),
    },
    CorpusSemaPin {
        loc: "docs/language/02-language.md:250",
        kind: "ok",
        gap: None,
    },
    CorpusSemaPin {
        loc: "docs/language/02-language.md:314",
        kind: "ok",
        gap: None,
    },
    CorpusSemaPin {
        loc: "docs/language/02-language.md:331",
        kind: "ok",
        gap: None,
    },
    CorpusSemaPin {
        loc: "docs/language/02-language.md:348",
        kind: "ok",
        gap: None,
    },
    CorpusSemaPin {
        loc: "docs/language/02-language.md:370",
        kind: "ok",
        gap: None,
    },
    CorpusSemaPin {
        loc: "docs/language/02-language.md:396",
        kind: "ok",
        gap: None,
    },
    CorpusSemaPin {
        loc: "docs/language/02-language.md:410",
        kind: "ok",
        gap: None,
    },
    CorpusSemaPin {
        loc: "docs/language/02-language.md:458",
        kind: "ok",
        gap: None,
    },
    CorpusSemaPin {
        loc: "docs/language/02-language.md:473",
        kind: "ok",
        gap: None,
    },
    CorpusSemaPin {
        loc: "docs/language/02-language.md:530",
        kind: "disagreement",
        gap: Some("sema.generics.method-params"),
    },
    CorpusSemaPin {
        loc: "docs/language/02-language.md:604",
        kind: "disagreement",
        gap: Some("actors.calls.callerror-nameable"),
    },
    CorpusSemaPin {
        loc: "docs/language/02-language.md:656",
        kind: "disagreement",
        gap: Some("actors.messages.take-non-own-resource"),
    },
    CorpusSemaPin {
        loc: "docs/language/02-language.md:674",
        kind: "disagreement",
        gap: Some("actors.calls.callerror-nameable"),
    },
    CorpusSemaPin {
        loc: "docs/language/02-language.md:870",
        kind: "ok",
        gap: None,
    },
    CorpusSemaPin {
        loc: "docs/language/03-hardware.md:51",
        kind: "ok",
        gap: None,
    },
    CorpusSemaPin {
        loc: "docs/language/03-hardware.md:86",
        kind: "ok",
        gap: None,
    },
    CorpusSemaPin {
        loc: "docs/language/03-hardware.md:222",
        kind: "ok",
        gap: None,
    },
    CorpusSemaPin {
        loc: "docs/language/03-hardware.md:266",
        kind: "ok",
        gap: None,
    },
    CorpusSemaPin {
        loc: "docs/language/03-hardware.md:300",
        kind: "ok",
        gap: None,
    },
];
