use std::collections::BTreeSet;
use std::path::Path;

use crate::root;

const PLAN: &str = "docs/designs/WRELA_PIXELS_COMPILER_IMPLEMENTATION_PLAN.md";
const REQUIRED_FIELDS: &[&str] = &[
    "Requires",
    "Produces",
    "Files",
    "Contract/dump delta",
    "Work",
    "Tests",
    "Focused checks",
    "Repository gate",
    "Stop conditions",
];
const DUMPS: &[&str] = &["field-graph", "frame-program", "render-layout"];
const MANIFESTS: &[&str] = &["KERNELS.txt", "EXPECTED_AXIOMS.txt"];

fn task_id(line: &str) -> Option<&str> {
    line.strip_prefix("## Task ")?
        .split_once(" — ")
        .map(|(id, _)| id.trim())
}

fn section<'a>(text: &'a str, start: &str, end: &str) -> Result<&'a str, String> {
    let start_index = text
        .find(start)
        .ok_or_else(|| format!("pixels plan lint: missing `{start}`"))?;
    let tail = &text[start_index + start.len()..];
    let end_index = tail
        .find(end)
        .ok_or_else(|| format!("pixels plan lint: missing `{end}` after `{start}`"))?;
    Ok(&tail[..end_index])
}

fn lint_text(text: &str, repo: &Path) -> Result<(), String> {
    let mut headings = Vec::new();
    let mut starts = Vec::new();
    let mut offset = 0usize;
    for line in text.split_inclusive('\n') {
        if let Some(id) = task_id(line.trim_end()) {
            headings.push(id.to_string());
            starts.push(offset);
        }
        offset += line.len();
    }
    if headings.len() != 154 {
        return Err(format!(
            "pixels plan lint: found {} task headings, expected 154",
            headings.len()
        ));
    }
    let unique: BTreeSet<_> = headings.iter().collect();
    if unique.len() != headings.len() {
        return Err("pixels plan lint: duplicate task ID".to_string());
    }
    starts.push(text.len());
    for (index, id) in headings.iter().enumerate() {
        let body = &text[starts[index]..starts[index + 1]];
        for field in REQUIRED_FIELDS {
            let needle = format!("**{field}:**");
            if !body.contains(&needle) {
                return Err(format!(
                    "pixels plan lint: task {id} is missing field `{field}`"
                ));
            }
        }
    }

    let order = section(text, "## 14. Exact commit order", "## 15.")?;
    let mut queue = Vec::new();
    for line in order.lines() {
        let token = line.split_whitespace().next().unwrap_or("");
        if token.starts_with('P') && token.contains('.') {
            let normalized = token.trim_end_matches(|c: char| c.is_ascii_alphabetic());
            if queue.last().map(String::as_str) != Some(normalized) {
                queue.push(normalized.to_string());
            }
        }
    }
    if queue != headings {
        let mismatch = queue
            .iter()
            .zip(&headings)
            .position(|(a, b)| a != b)
            .unwrap_or(queue.len().min(headings.len()));
        return Err(format!(
            "pixels plan lint: §14/task order mismatch at index {mismatch}: queue={:?} heading={:?}",
            queue.get(mismatch),
            headings.get(mismatch)
        ));
    }

    for (name, expected) in [
        ("enum RenderError:", 1usize),
        ("enum FieldKind {", 1),
        ("struct Iv32:", 1),
        ("struct FrameProgramHeaderV1 {", 1),
    ] {
        let count = text.matches(name).count();
        if count != expected {
            return Err(format!(
                "pixels plan lint: canonical `{name}` occurs {count} times, expected {expected}"
            ));
        }
    }
    for needle in [
        "Header, exactly 80 bytes:",
        "header_bytes: u16,       // 80",
        "magic: [u8; 8],          // b\"WRELAPX\\0\"",
    ] {
        if !text.contains(needle) {
            return Err(format!(
                "pixels plan lint: frame header drift: missing `{needle}`"
            ));
        }
    }

    let cli = section(text, "### 8.1 CLI stages", "### 8.2")?;
    for dump in DUMPS {
        if cli.matches(&format!("--stage={dump}")).count() != 1 {
            return Err(format!(
                "pixels plan lint: dump stage `{dump}` must occur once in §8.1"
            ));
        }
    }
    if cli.matches("--stage=").count() != DUMPS.len() {
        return Err("pixels plan lint: §8.1 defines a noncanonical Pixels dump".to_string());
    }
    for manifest in MANIFESTS {
        if !text.contains(manifest) {
            return Err(format!(
                "pixels plan lint: missing canonical manifest `{manifest}`"
            ));
        }
    }
    let renderer = section(text, "img.renderer[P](", "All labels are required in v1.")?;
    for label in wrela_compiler::pixels::RENDERER_LABELS {
        if !renderer.contains(&format!("{label}=")) {
            return Err(format!(
                "pixels plan lint: renderer declaration is missing `{label}=`"
            ));
        }
    }
    if text.contains("crates/wrela-machine/src/display.rs") {
        return Err(
            "pixels plan lint: stale display owner; machine display ABI lives in \
             `crates/wrela-machine/src/pixels.rs`"
                .to_string(),
        );
    }

    lint_marked_paths(text, repo)?;
    lint_display_contract(repo)?;
    Ok(())
}

fn lint_display_contract(repo: &Path) -> Result<(), String> {
    let source_path = repo.join("stdlib/drivers/display.wr");
    let source = std::fs::read_to_string(&source_path)
        .map_err(|e| format!("read {}: {e}", source_path.display()))?;
    let required = [
        format!(
            "@placed(0x{:08x})\nstatic DISPLAY_MEMORY",
            wrela_machine::pixels::CONTROL_BASE
        ),
        format!(
            "@placed(0x{:08x})\nstatic DISPLAY_DOORBELL",
            wrela_machine::pixels::DOORBELL_ADDR
        ),
        format!(
            "@offset({:#06x}) tile_id",
            wrela_machine::pixels::TILES_BASE - wrela_machine::pixels::CONTROL_BASE
        ),
        format!(
            "@offset({:#06x}) pixels",
            wrela_machine::pixels::FRAMEBUFFER_BASE - wrela_machine::pixels::CONTROL_BASE
        ),
        format!("[u8; {}]", wrela_machine::pixels::FRAME_BYTES),
    ];
    for needle in required {
        if !source.contains(&needle) {
            return Err(format!(
                "pixels plan lint: Wrela display ABI disagrees with machine constants; \
                 missing `{needle}`"
            ));
        }
    }
    Ok(())
}

fn lint_marked_paths(text: &str, repo: &Path) -> Result<(), String> {
    let mut in_files = false;
    let mut in_fence = false;
    for line in text.lines() {
        let mut content = line;
        if let Some(rest) = line.strip_prefix("**Files:**") {
            in_files = true;
            in_fence = false;
            content = rest;
        } else if line.starts_with("**Contract/dump delta:**") {
            in_files = false;
            in_fence = false;
        } else if in_files && line.trim_start().starts_with("```") {
            in_fence = !in_fence;
            continue;
        }
        if !in_files || content.contains("new at P-1 basis") {
            continue;
        }
        if in_fence {
            let token = content.split_whitespace().next().unwrap_or("");
            if token.contains('/') && !token.contains(['*', '{', '}']) && !repo.join(token).exists()
            {
                return Err(format!(
                    "pixels plan lint: unmarked Files path does not exist: `{token}`"
                ));
            }
        }
        for token in content.split('`').skip(1).step_by(2) {
            if token.contains(['*', '{', '}'])
                || token == "new"
                || token == "modified"
                || token.contains(' ')
            {
                continue;
            }
            if token.contains('/') && !repo.join(token.trim_end_matches('/')).exists() {
                return Err(format!(
                    "pixels plan lint: unmarked Files path does not exist: `{token}`"
                ));
            }
        }
    }
    Ok(())
}

pub(crate) fn pixels_plan_lint() -> Result<(), String> {
    let repo = root();
    let text = std::fs::read_to_string(repo.join(PLAN)).map_err(|e| format!("read {PLAN}: {e}"))?;
    lint_text(&text, &repo)?;
    println!("pixels-plan-lint: 154 tasks and canonical contracts match");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn actual() -> String {
        std::fs::read_to_string(root().join(PLAN)).unwrap()
    }

    #[test]
    fn repository_plan_passes() {
        lint_text(&actual(), &root()).unwrap();
    }

    #[test]
    fn missing_field_fails() {
        let changed = actual().replacen("**Requires:**", "**Need:**", 1);
        assert!(
            lint_text(&changed, &root())
                .unwrap_err()
                .contains("Requires")
        );
    }

    #[test]
    fn duplicate_task_fails() {
        let changed = actual().replacen("## Task P-1.2", "## Task P-1.1", 1);
        assert!(
            lint_text(&changed, &root())
                .unwrap_err()
                .contains("duplicate")
        );
    }

    #[test]
    fn dump_and_header_drift_fail() {
        let changed = actual().replace("--stage=render-layout", "--stage=other");
        assert!(
            lint_text(&changed, &root())
                .unwrap_err()
                .contains("render-layout")
        );
        let changed = actual().replace("Header, exactly 80 bytes:", "Header, exactly 88 bytes:");
        assert!(lint_text(&changed, &root()).unwrap_err().contains("header"));
    }

    #[test]
    fn duplicate_definition_magic_fourth_dump_and_manifest_drift_fail() {
        let changed = actual().replacen(
            "enum RenderError:",
            "enum RenderError:\nenum RenderError:",
            1,
        );
        assert!(
            lint_text(&changed, &root())
                .unwrap_err()
                .contains("occurs 2")
        );

        let changed = actual().replacen("b\"WRELAPX\\0\"", "b\"OTHERPX\\0\"", 1);
        assert!(lint_text(&changed, &root()).unwrap_err().contains("header"));

        let changed = actual().replace(
            "--stage=render-layout",
            "--stage=render-layout\n--stage=hidden-fourth-dump",
        );
        assert!(
            lint_text(&changed, &root())
                .unwrap_err()
                .contains("noncanonical")
        );

        let changed = actual().replace("EXPECTED_AXIOMS.txt", "EXPECTED_AXIOMS.md");
        assert!(
            lint_text(&changed, &root())
                .unwrap_err()
                .contains("EXPECTED_AXIOMS.txt")
        );
    }

    #[test]
    fn unmarked_missing_files_path_fails() {
        let changed = actual().replacen("`AGENTS.md`", "`not/a/real.file`", 1);
        assert!(
            lint_text(&changed, &root())
                .unwrap_err()
                .contains("unmarked Files path")
        );
    }

    #[test]
    fn stale_machine_display_owner_fails() {
        let changed = actual().replacen(
            "crates/wrela-machine/src/pixels.rs",
            "crates/wrela-machine/src/display.rs",
            1,
        );
        assert!(
            lint_text(&changed, &root())
                .unwrap_err()
                .contains("stale display owner")
        );
    }
}
