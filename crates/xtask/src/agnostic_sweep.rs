//! `cargo xtask agnostic-sweep` — plans/M20.md item A's own oracle.
//!
//! Until M20, the tree asserted in some two dozen places that the cost
//! model was hardware-agnostic: "not an A76 SOG port map", "no real
//! L1/L2/L3 KB geometry", "differential ISA ranking only",
//! `target=isa_baseline`. M20 decision 1600 reverses that — the documented
//! A76 port map **is** the model, pinned to `a76-pi5` — so every one of
//! those sites had to move in item A's commit train or the tree
//! contradicts itself, with CLAUDE.md making the docs ground truth over
//! the code.
//!
//! The failure mode of that sweep is a **missed** site, not a wrong one,
//! which is a thing a human diff review is bad at and a grep is good at.
//! So the sweep gets a standing check rather than a one-time review: no
//! superseded phrase survives anywhere in the tracked tree, and the phrase
//! the reversal introduced is positively present where it belongs.
//!
//! Fail closed: any hit is an error naming every offending `path:line`.
//! There is no allowlist of "known remaining" sites — the point of the
//! check is that the set is empty.

use std::process::Command;

use crate::root;

/// Directory prefixes the sweep does not scan, each for its own reason —
/// not for convenience. Any other tracked file is fair game.
const EXCLUDED_PREFIXES: &[(&str, &str)] = &[
    // CLAUDE.md: "docs/archive/ — the superseded draft spec; read-only
    // history." Rewriting history to match current doctrine is exactly
    // what an archive exists to prevent.
    ("docs/archive/", "read-only history (CLAUDE.md)"),
    // Historical milestone plans legitimately record the state of the
    // tree when they were written, and plans/M20.md itself quotes, at
    // length and on purpose, every phrase it is retiring — its item A
    // inventory table is a list of the strings this check hunts.
    ("plans/", "plans record the superseded state deliberately"),
    // This file. It is the list — it has to contain every phrase it hunts,
    // and a check that flagged its own definition could never pass. The
    // exclusion is therefore exact (one path, not a directory) so nothing
    // else in `crates/xtask/` hides behind it.
    (
        "crates/xtask/src/agnostic_sweep.rs",
        "defines the phrase list",
    ),
];

/// The ledger is scanned, but by a different rule (see `sweep_ledger`).
const LEDGER: &str = "ledger/ledger.toml";

/// The marker a line must carry to be allowed to quote a superseded
/// phrase. Deliberately specific: any text containing it is text that is
/// recording this exact lift, not text still asserting the old claim.
const LIFT_MARKER: &str = "plans/M20.md item A";

/// Every superseded assertion, in the normalized (whitespace-collapsed)
/// form the scanner matches. Enumerated by grepping the tree before item A
/// touched it, not guessed: each entry had at least one live occurrence,
/// except the `*-agnostic` spellings, which are forward guards against the
/// claim coming back in words the tree never happened to use.
///
/// Kept out of this list on purpose, because M20 does **not** reverse
/// them: "no physical calibration", "not calibrated to host wall clocks",
/// "no host PGO as a gate input", and every statement that physical
/// measurement never gates an opt landing (M19 freeze 1404 / M20 freeze
/// 1631). Those are still doctrine, and a check that flagged them would be
/// arguing the opposite of the milestone.
const SUPERSEDED: &[(&str, &str)] = &[
    // The two pinned dump strings. These move every `--stage=cost`
    // golden, so they are the clearest single signal of the reversal.
    (
        "ignore_cache=1",
        "the cache hierarchy is modelled now (item F)",
    ),
    (
        "ignore_mispredict=1",
        "mispredict is charged from measured bias now (item H)",
    ),
    (
        "target=isa_baseline",
        "the target is the a76-pi5 profile now (freeze 1621)",
    ),
    // The port-map denials, in every spelling the tree used.
    (
        "not an A76",
        "the A76 port map is the model (decision 1600)",
    ),
    ("not A76", "the A76 port map is the model (decision 1600)"),
    ("A76 SOG port map", "that port map is now the model"),
    ("A76 SOG ports", "those ports are now the model"),
    (
        "A76 Software Optimization Guide port map",
        "that port map is now the model",
    ),
    (
        "port maps into the proxy",
        "pasting the published port map in is now the deliverable",
    ),
    (
        "A76 absolute",
        "the model is A76-specific now (freeze 1621)",
    ),
    (
        "A76\u{2019}s real decode",
        "the real dispatch constraints are modelled now (item E)",
    ),
    // The geometry denials.
    (
        "no real L1",
        "real L1/L2/L3 and TLB geometry is modelled now (item F)",
    ),
    (
        "no real cache",
        "real cache geometry is modelled now (item F)",
    ),
    (
        "physical cache geometry",
        "the geometry table is real now (item F)",
    ),
    (
        "cache/L2/L3/branch-mispredict",
        "those models are in scope now (items F and H)",
    ),
    // The scope denials.
    (
        "differential ISA ranking",
        "ranking is against a published-record A76 model now",
    ),
    (
        "differential ISA proxy",
        "ranking is against a published-record A76 model now",
    ),
    // Forward guards: spellings the claim could return in.
    (
        "hardware-agnostic",
        "the model is board-specific by mandate",
    ),
    ("host-agnostic", "the model is board-specific by mandate"),
    (
        "microarchitecture-agnostic",
        "the model is a microarchitecture model now",
    ),
];

/// Phrases that must be **present**, and where. The negative half of this
/// check can be satisfied by deleting the dump line entirely; this half
/// says what has to be there instead.
const REQUIRED: &[(&str, &[&str])] = &[(
    "crates/wrela-compiler/src/cost/dump.rs",
    &["ignore_cache=0", "ignore_mispredict=0", "target=a76_pi5"],
)];

/// One offending site.
struct Hit {
    path: String,
    line: u32,
    phrase: &'static str,
    why: &'static str,
}

pub(crate) fn agnostic_sweep() -> Result<(), String> {
    let files = tracked_files()?;
    if files.is_empty() {
        return Err(
            "agnostic-sweep: `git ls-files` returned nothing — refusing to pass on an empty scan"
                .to_string(),
        );
    }

    let mut scanned = 0usize;
    let mut hits: Vec<Hit> = Vec::new();
    for rel in &files {
        if EXCLUDED_PREFIXES
            .iter()
            .any(|(prefix, _why)| rel.starts_with(prefix))
        {
            continue;
        }
        let abs = root().join(rel);
        if !abs.is_file() {
            // A tracked path that is not a regular file (submodule,
            // symlink to nowhere) — nothing to read, nothing to claim.
            continue;
        }
        let bytes = std::fs::read(&abs).map_err(|e| format!("read {rel}: {e}"))?;
        // Lossy on purpose: a tracked non-UTF-8 file must still be
        // scanned rather than silently skipped. Every tracked file in this
        // tree is valid UTF-8 today, so this never actually lossifies.
        let text = String::from_utf8_lossy(&bytes);
        scanned += 1;
        if rel == LEDGER {
            sweep_ledger(rel, &text, &mut hits);
        } else {
            sweep_prose(rel, &text, &mut hits);
        }
    }

    let mut missing: Vec<String> = Vec::new();
    for (rel, required) in REQUIRED {
        let abs = root().join(rel);
        let text = std::fs::read_to_string(&abs).map_err(|e| format!("read {rel}: {e}"))?;
        for phrase in *required {
            if !text.contains(phrase) {
                missing.push(format!("{rel}: missing required `{phrase}`"));
            }
        }
    }

    if hits.is_empty() && missing.is_empty() {
        println!(
            "agnostic-sweep: {scanned} tracked file(s) scanned, {} superseded phrase(s) enforced, 0 found",
            SUPERSEDED.len()
        );
        return Ok(());
    }

    for h in &hits {
        println!(
            "  {}:{}: superseded `{}` — {}",
            h.path, h.line, h.phrase, h.why
        );
    }
    for m in &missing {
        println!("  {m}");
    }
    Err(format!(
        "agnostic-sweep: {} superseded assertion(s) and {} missing required phrase(s) \
         (plans/M20.md item A: the whole tree moves together, or it contradicts itself)",
        hits.len(),
        missing.len()
    ))
}

/// Every tracked path, repo-relative, in git's order.
fn tracked_files() -> Result<Vec<String>, String> {
    let out = Command::new("git")
        .args(["ls-files", "-z"])
        .current_dir(root())
        .output()
        .map_err(|e| format!("git ls-files: {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "git ls-files failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    Ok(String::from_utf8_lossy(&out.stdout)
        .split('\0')
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .collect())
}

/// Ordinary files: whitespace-collapsed whole-file matching, no exemption.
///
/// Whole-file rather than per-line because the two normative documents and
/// ROADMAP.md wrap prose at ~72 columns, so "not an A76 SOG port map"
/// routinely straddles a newline and a line-at-a-time matcher would sail
/// straight past the very sites this check exists to catch.
///
/// Known limitation, recorded rather than papered over: markdown emphasis
/// *inside* a phrase (`not **A76** SOG`) defeats plain matching. No
/// occurrence in this tree does that — the emphasis always wraps the whole
/// clause — and the alternative (stripping markup) would make the matcher
/// clever in exchange for making its failures hard to read.
fn sweep_prose(rel: &str, text: &str, hits: &mut Vec<Hit>) {
    let (normal, lines) = normalize_with_lines(text);
    for (phrase, why) in SUPERSEDED {
        for (at, _) in normal.match_indices(phrase) {
            hits.push(Hit {
                path: rel.to_string(),
                line: lines.get(at).copied().unwrap_or(0),
                phrase,
                why,
            });
        }
    }
}

/// `ledger/ledger.toml`: per **line**, exempt when the same line carries
/// `LIFT_MARKER`.
///
/// The ledger is append-only by house convention — a clause's `note` grows
/// note-updates and never loses its history — so it legitimately quotes
/// the phrase it is retiring, and excluding the file outright would leave
/// the largest concentration of these assertions unchecked. One `note`
/// is one physical line here (every clause in the file is written that
/// way), so "the quote and the record of its lift are on the same line"
/// is exactly "the quote sits inside a note that records the lift". A
/// future multi-line note would put the quote on a line without the
/// marker and fail this check — the safe direction.
fn sweep_ledger(rel: &str, text: &str, hits: &mut Vec<Hit>) {
    for (i, line) in text.lines().enumerate() {
        if line.contains(LIFT_MARKER) {
            continue;
        }
        for (phrase, why) in SUPERSEDED {
            if line.contains(phrase) {
                hits.push(Hit {
                    path: rel.to_string(),
                    line: (i + 1) as u32,
                    phrase,
                    why,
                });
            }
        }
    }
}

/// Collapse every whitespace run to one space, and record, for each byte
/// of the result, the 1-based source line it came from. A phrase spanning
/// a wrap reports the line where it starts.
fn normalize_with_lines(text: &str) -> (String, Vec<u32>) {
    let mut out = String::with_capacity(text.len());
    let mut lines: Vec<u32> = Vec::with_capacity(text.len());
    let mut line = 1u32;
    let mut in_ws = false;
    for ch in text.chars() {
        if ch.is_whitespace() {
            if !in_ws {
                out.push(' ');
                lines.push(line);
                in_ws = true;
            }
            if ch == '\n' {
                line += 1;
            }
        } else {
            in_ws = false;
            let before = out.len();
            out.push(ch);
            for _ in before..out.len() {
                lines.push(line);
            }
        }
    }
    debug_assert_eq!(out.len(), lines.len());
    (out, lines)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prose_sweep_catches_a_phrase_split_across_a_wrap() {
        let mut hits = Vec::new();
        sweep_prose(
            "fake.md",
            "the model is not\nan A76 SOG port map, and never was.\n",
            &mut hits,
        );
        assert!(
            hits.iter().any(|h| h.phrase == "not an A76" && h.line == 1),
            "wrapped phrase missed: {:?}",
            hits.iter().map(|h| (h.phrase, h.line)).collect::<Vec<_>>()
        );
    }

    #[test]
    fn prose_sweep_has_no_lift_marker_exemption() {
        let mut hits = Vec::new();
        sweep_prose(
            "fake.md",
            "plans/M20.md item A lifted this: not an A76 SOG port map.\n",
            &mut hits,
        );
        assert!(
            !hits.is_empty(),
            "ordinary prose must not be exempted by the marker"
        );
    }

    #[test]
    fn ledger_sweep_exempts_only_marked_lines() {
        let mut hits = Vec::new();
        sweep_ledger(
            LEDGER,
            "note = \"a: not A76 SOG ports. Note-update (plans/M20.md item A): lifted.\"\n\
             note = \"b: not A76 SOG ports.\"\n",
            &mut hits,
        );
        // The fixture's phrase trips two entries at once ("not A76" and
        // "A76 SOG ports" both match), which is fine — what the exemption
        // has to get right is *which line* is reported, not how many
        // phrases a line manages to violate.
        assert!(!hits.is_empty(), "the unmarked line must be reported");
        assert!(
            hits.iter().all(|h| h.line == 2),
            "the marked line leaked: {:?}",
            hits.iter().map(|h| (h.phrase, h.line)).collect::<Vec<_>>()
        );
    }

    #[test]
    fn required_positive_phrases_are_present_in_the_dump_source() {
        for (rel, required) in REQUIRED {
            let text = std::fs::read_to_string(root().join(rel)).expect("read required file");
            for phrase in *required {
                assert!(text.contains(phrase), "{rel} lost `{phrase}`");
            }
        }
    }
}
