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
//! **Each row is keyed by the block's content**, not by its line number
//! (plans/M10.md item A3, decisions 710/711): `key` is
//! `corpus_sema_context::block_key` of the fence body — the first 12 hex
//! chars of SHA-256 over its exact text. Before A3 the key was
//! `path:start_line`, so inserting a paragraph above a fence broke its pin
//! and J3's gate reported that as *pin decay* — the wrong cause entirely.
//! `loc` is kept as the human's map back to the block and is never matched
//! on; it may go stale after an insertion. `cargo xtask corpus --sema`
//! prints every block's live `path:line` **and** its key, which is where
//! the values in this file come from.
//!
//! An edit *to* a block changes its key, so the pin stops matching and the
//! harness says so by name. That is deliberate: a changed block's pinned
//! classification genuinely needs re-review, and it is not decay.
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
    /// Content key of the block's body (`corpus_sema_context::block_key`).
    /// **The match key**; nothing else identifies the block.
    pub key: &'static str,
    /// `path:start_line` as of the last review — a human aid for finding
    /// the block. **Not matched on**, and allowed to go stale when
    /// something is inserted above the fence.
    pub loc: &'static str,
    /// `"ok"` or `"disagreement"`.
    pub kind: &'static str,
    /// Ledger gap owning an accepted disagreement; `None` for ok rows.
    pub gap: Option<&'static str>,
}

/// Measured 2026-07-25 through J2d: **18 ok, 5 disagreements** out of 23
/// parseable-for-sema blocks; **+1 ok** at plans/M10.md item A2c (03 §3.1
/// example fence is now ```wrela) → **19 ok, 5 disagreements** out of 24.
/// The five disagreements are deliberate gaps (J2b rulings / decision 523);
/// J3 gates on this pin, not on zero. Re-keyed by content 2026-07-26
/// (item A3); classifications unchanged except the new A2c row.
pub const CORPUS_SEMA_CENSUS: &[CorpusSemaPin] = &[
    CorpusSemaPin {
        key: "8949ad8e2912",
        loc: "docs/language/02-language.md:43",
        kind: "ok",
        gap: None,
    },
    CorpusSemaPin {
        key: "91f2c87cf267",
        loc: "docs/language/02-language.md:150",
        kind: "ok",
        gap: None,
    },
    CorpusSemaPin {
        key: "be05640fe581",
        loc: "docs/language/02-language.md:185",
        kind: "ok",
        gap: None,
    },
    CorpusSemaPin {
        key: "ae1b6fca2a64",
        loc: "docs/language/02-language.md:206",
        kind: "disagreement",
        gap: Some("values.regions.two-binding-disciplines"),
    },
    CorpusSemaPin {
        key: "dd7b0c3dadfe",
        loc: "docs/language/02-language.md:250",
        kind: "ok",
        gap: None,
    },
    CorpusSemaPin {
        key: "af47d81c4f9f",
        loc: "docs/language/02-language.md:314",
        kind: "ok",
        gap: None,
    },
    CorpusSemaPin {
        key: "5bc7a67c7fee",
        loc: "docs/language/02-language.md:331",
        kind: "ok",
        gap: None,
    },
    CorpusSemaPin {
        key: "b49b24ba36e0",
        loc: "docs/language/02-language.md:348",
        kind: "ok",
        gap: None,
    },
    CorpusSemaPin {
        key: "7595bd03723a",
        loc: "docs/language/02-language.md:370",
        kind: "ok",
        gap: None,
    },
    CorpusSemaPin {
        key: "8e5ee656b323",
        loc: "docs/language/02-language.md:396",
        kind: "ok",
        gap: None,
    },
    CorpusSemaPin {
        key: "d81156810e9a",
        loc: "docs/language/02-language.md:410",
        kind: "ok",
        gap: None,
    },
    CorpusSemaPin {
        key: "8d7336ba95d3",
        loc: "docs/language/02-language.md:458",
        kind: "ok",
        gap: None,
    },
    CorpusSemaPin {
        key: "b05b056b05b1",
        loc: "docs/language/02-language.md:473",
        kind: "ok",
        gap: None,
    },
    CorpusSemaPin {
        key: "276e58180606",
        loc: "docs/language/02-language.md:530",
        kind: "disagreement",
        gap: Some("sema.generics.method-params"),
    },
    CorpusSemaPin {
        key: "f56a6f9b7f60",
        loc: "docs/language/02-language.md:604",
        kind: "disagreement",
        gap: Some("actors.calls.callerror-nameable"),
    },
    CorpusSemaPin {
        key: "e18a87c8f714",
        loc: "docs/language/02-language.md:656",
        kind: "disagreement",
        gap: Some("actors.messages.take-non-own-resource"),
    },
    CorpusSemaPin {
        key: "84c332141ae2",
        loc: "docs/language/02-language.md:674",
        kind: "disagreement",
        gap: Some("actors.calls.callerror-nameable"),
    },
    CorpusSemaPin {
        key: "6e8e60909180",
        loc: "docs/language/02-language.md:870",
        kind: "ok",
        gap: None,
    },
    CorpusSemaPin {
        key: "aefff425cad3",
        loc: "docs/language/03-hardware.md:51",
        kind: "ok",
        gap: None,
    },
    CorpusSemaPin {
        key: "f4ee6fe571a7",
        loc: "docs/language/03-hardware.md:86",
        kind: "ok",
        gap: None,
    },
    // plans/M10.md item A2c: §3.1 example is now ```wrela (was ```text).
    CorpusSemaPin {
        key: "4ab31cc43898",
        loc: "docs/language/03-hardware.md:136",
        kind: "ok",
        gap: None,
    },
    CorpusSemaPin {
        key: "5bbef735fe9c",
        loc: "docs/language/03-hardware.md:227",
        kind: "ok",
        gap: None,
    },
    CorpusSemaPin {
        key: "f0f2c62e8a44",
        loc: "docs/language/03-hardware.md:271",
        kind: "ok",
        gap: None,
    },
    CorpusSemaPin {
        key: "19e016849751",
        loc: "docs/language/03-hardware.md:305",
        kind: "ok",
        gap: None,
    },
];
