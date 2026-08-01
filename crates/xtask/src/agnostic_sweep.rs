use std::process::Command;

use crate::root;

const EXCLUDED_PREFIXES: &[(&str, &str)] = &[(
    "crates/xtask/src/agnostic_sweep.rs",
    "defines the phrase list",
)];

const SUPERSEDED: &[(&str, &str)] = &[
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
    (
        "differential ISA ranking",
        "ranking is against a published-record A76 model now",
    ),
    (
        "differential ISA proxy",
        "ranking is against a published-record A76 model now",
    ),
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

const REQUIRED: &[(&str, &[&str])] = &[(
    "crates/wrela-compiler/src/cost/dump.rs",
    &["ignore_cache=0", "ignore_mispredict=0", "target=a76_pi5"],
)];

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
            continue;
        }
        let bytes = std::fs::read(&abs).map_err(|e| format!("read {rel}: {e}"))?;
        let text = String::from_utf8_lossy(&bytes);
        scanned += 1;
        sweep_prose(rel, &text, &mut hits);
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
        "agnostic-sweep: {} superseded assertion(s) and {} missing required phrase(s); \
         the whole tree must move together or it contradicts itself",
        hits.len(),
        missing.len()
    ))
}

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
            "a completed migration claimed this was lifted: not an A76 SOG port map.\n",
            &mut hits,
        );
        assert!(
            !hits.is_empty(),
            "ordinary prose must not be exempted by the marker"
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
