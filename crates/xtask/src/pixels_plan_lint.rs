use std::collections::BTreeSet;
use std::path::Path;

use crate::root;

const PLAN: &str = "docs/designs/WRELA_PIXELS_COMPILER_IMPLEMENTATION_PLAN.md";
const PIXELS_CASES: &str = "tests/pixels-cases.txt";
const PIXELS_CASE_MARKER: &str = "# Permanent Pixels fixture: ";
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
    lint_normative_docs(repo)?;
    lint_permanent_fixtures(text, repo, &headings)?;
    Ok(())
}

fn lint_normative_docs(repo: &Path) -> Result<(), String> {
    let pixels_path = repo.join("docs/language/07-pixels.md");
    let pixels = std::fs::read_to_string(&pixels_path)
        .map_err(|error| format!("read {}: {error}", pixels_path.display()))?;
    for heading in [
        "## 0. Delivered contract",
        "## 1. Closed architectural decisions",
        "## 2. Source and semantic contract",
        "## 3. Compiler pipeline and ownership",
        "## 4. Internal data model and image format",
        "## 5. Runtime mathematics",
    ] {
        if !pixels.contains(heading) {
            return Err(format!(
                "pixels plan lint: normative chapter lacks `{heading}`"
            ));
        }
    }
    for spelling in ["@field", "@material", "@range", "@rate", "Image.renderer"] {
        if !pixels.contains(spelling) {
            return Err(format!(
                "pixels plan lint: normative chapter lacks source spelling `{spelling}`"
            ));
        }
    }
    for contract in [
        "correct without",
        "Kinetic maintenance is optional",
        "`AaaByteExact` rejects unsupported",
        "FieldGraph",
        "FrameProgram",
        "field-graph",
        "frame-program",
        "render-layout",
        "WRELAPX\\0",
        "exactly 80 bytes",
        "frameprog",
        "pixelsdata",
        "FEAT_DotProd",
    ] {
        let all_docs = if contract == "FEAT_DotProd" {
            std::fs::read_to_string(repo.join("docs/language/04-compiler.md"))
                .map_err(|error| format!("read compiler chapter: {error}"))?
        } else {
            pixels.clone()
        };
        if !all_docs.contains(contract) {
            return Err(format!(
                "pixels plan lint: normative Pixels contract lacks `{contract}`"
            ));
        }
    }

    let historical_path = repo.join("docs/designs/pixels.md");
    let historical = std::fs::read_to_string(&historical_path)
        .map_err(|error| format!("read {}: {error}", historical_path.display()))?;
    let historical_words = historical.split_whitespace().collect::<Vec<_>>().join(" ");
    for phrase in [
        "HISTORICAL EVIDENCE",
        "unfavorable online-result history",
        "does not validate the production renderer",
    ] {
        if !historical_words.contains(phrase) {
            return Err(format!(
                "pixels plan lint: historical fieldprobe evidence lacks `{phrase}`"
            ));
        }
    }

    for relative in [
        "docs/language/04-compiler.md",
        "docs/language/05-library.md",
        "docs/language/06-machine.md",
        "docs/language/07-pixels.md",
        "docs/designs/pixels.md",
    ] {
        lint_relative_markdown_links(&repo.join(relative))?;
    }
    Ok(())
}

fn lint_relative_markdown_links(path: &Path) -> Result<(), String> {
    let text = std::fs::read_to_string(path)
        .map_err(|error| format!("read {}: {error}", path.display()))?;
    let mut rest = text.as_str();
    while let Some((_, after_open)) = rest.split_once("](") {
        let Some((target, after_close)) = after_open.split_once(')') else {
            return Err(format!(
                "pixels plan lint: unterminated Markdown link in {}",
                path.display()
            ));
        };
        rest = after_close;
        let file = target.split('#').next().unwrap_or("");
        if file.is_empty()
            || file.starts_with("http://")
            || file.starts_with("https://")
            || file.starts_with("mailto:")
            || !(file.contains('/') || file.ends_with(".md") || file.ends_with(".wr"))
        {
            continue;
        }
        let resolved = path
            .parent()
            .expect("tracked Markdown file has a parent")
            .join(file);
        if !resolved.exists() {
            return Err(format!(
                "pixels plan lint: broken relative link `{target}` in {}",
                path.display()
            ));
        }
    }
    Ok(())
}

fn matrix_fixture_names(text: &str) -> Result<Vec<String>, String> {
    let matrix = section(
        text,
        "## 11. Permanent correctness and conformance matrix",
        "### 11.1",
    )?;
    let mut names = Vec::new();
    for line in matrix.lines() {
        let Some(after_tick) = line.strip_prefix("| `") else {
            continue;
        };
        let Some((name, _)) = after_tick.split_once('`') else {
            continue;
        };
        if name.starts_with("check-") || name.starts_with("err-") || name.starts_with("boot-") {
            names.push(name.to_string());
        }
    }
    if names.len() != 38 {
        return Err(format!(
            "pixels plan lint: §11 contains {} permanent fixtures, expected 38",
            names.len()
        ));
    }
    let unique: BTreeSet<_> = names.iter().collect();
    if unique.len() != names.len() {
        return Err("pixels plan lint: §11 contains a duplicate fixture name".to_string());
    }
    Ok(names)
}

fn manifest_fixture_names(text: &str) -> Result<Vec<String>, String> {
    let names: Vec<String> = text
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .map(str::to_string)
        .collect();
    let unique: BTreeSet<_> = names.iter().collect();
    if unique.len() != names.len() {
        return Err("pixels plan lint: tests/pixels-cases.txt contains a duplicate".to_string());
    }
    Ok(names)
}

fn census_fixture_names(text: &str) -> Result<(usize, Vec<String>), String> {
    let start = text
        .find("[pixels_cases]")
        .ok_or_else(|| "pixels plan lint: tests/census.toml lacks [pixels_cases]".to_string())?;
    let tail = &text[start + "[pixels_cases]".len()..];
    let body = tail.split_once("\n[").map_or(tail, |(pixels, _)| pixels);
    let count = body
        .lines()
        .find_map(|line| line.trim().strip_prefix("count = "))
        .ok_or_else(|| "pixels plan lint: [pixels_cases] lacks count".to_string())?
        .parse::<usize>()
        .map_err(|error| format!("pixels plan lint: invalid [pixels_cases] count: {error}"))?;
    let names = body
        .lines()
        .filter_map(|line| {
            let line = line.trim();
            line.strip_prefix('"')
                .and_then(|value| value.strip_suffix("\","))
                .map(str::to_string)
        })
        .collect();
    Ok((count, names))
}

fn compare_fixture_names(
    matrix: &[String],
    manifest: &[String],
    census_count: usize,
    census: &[String],
    discovered: &BTreeSet<String>,
) -> Result<(), String> {
    if matrix != manifest {
        return Err(
            "pixels plan lint: §11 and tests/pixels-cases.txt fixture order/set differ".to_string(),
        );
    }
    if matrix != census || census_count != matrix.len() {
        return Err(
            "pixels plan lint: §11 and tests/census.toml [pixels_cases] differ".to_string(),
        );
    }
    let expected: BTreeSet<String> = matrix.iter().cloned().collect();
    if &expected != discovered {
        let missing: Vec<_> = expected.difference(discovered).cloned().collect();
        let extra: Vec<_> = discovered.difference(&expected).cloned().collect();
        return Err(format!(
            "pixels plan lint: permanent fixture directories differ; missing={missing:?} extra={extra:?}"
        ));
    }
    Ok(())
}

fn lint_permanent_fixtures(plan: &str, repo: &Path, task_ids: &[String]) -> Result<(), String> {
    let matrix = matrix_fixture_names(plan)?;
    let manifest_text = std::fs::read_to_string(repo.join(PIXELS_CASES))
        .map_err(|error| format!("read {PIXELS_CASES}: {error}"))?;
    let manifest = manifest_fixture_names(&manifest_text)?;
    let census_path = repo.join("tests/census.toml");
    let census_text = std::fs::read_to_string(&census_path)
        .map_err(|error| format!("read {}: {error}", census_path.display()))?;
    let (census_count, census) = census_fixture_names(&census_text)?;
    let tasks: BTreeSet<&str> = task_ids.iter().map(String::as_str).collect();

    let golden = repo.join("tests/golden");
    let mut discovered = BTreeSet::new();
    for entry in
        std::fs::read_dir(&golden).map_err(|error| format!("read {}: {error}", golden.display()))?
    {
        let path = entry
            .map_err(|error| format!("read {} entry: {error}", golden.display()))?
            .path();
        if !path.is_dir() {
            continue;
        }
        let readme_path = path.join("README.md");
        if !readme_path.is_file() {
            continue;
        }
        let readme = std::fs::read_to_string(&readme_path)
            .map_err(|error| format!("read {}: {error}", readme_path.display()))?;
        let Some(declared_name) = readme
            .lines()
            .next()
            .and_then(|line| line.strip_prefix(PIXELS_CASE_MARKER))
        else {
            continue;
        };
        let directory_name = path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("");
        if declared_name != directory_name {
            return Err(format!(
                "pixels plan lint: {} declares fixture `{declared_name}`",
                readme_path.display()
            ));
        }
        for field in [
            "Protects:",
            "Deterministic geometry:",
            "First active:",
            "P0 status:",
        ] {
            if !readme.contains(field) {
                return Err(format!(
                    "pixels plan lint: {} lacks `{field}`",
                    readme_path.display()
                ));
            }
        }
        if !readme.contains("production Pixels stage is not implemented; implemented in task P") {
            return Err(format!(
                "pixels plan lint: {} lacks the task-owned P0 placeholder",
                readme_path.display()
            ));
        }
        let input_path = path.join("input.wr");
        let expected_path = path.join("expected/check.txt");
        let input = std::fs::read_to_string(&input_path)
            .map_err(|error| format!("read {}: {error}", input_path.display()))?;
        let expected = std::fs::read_to_string(&expected_path)
            .map_err(|error| format!("read {}: {error}", expected_path.display()))?;
        let placeholder = "production Pixels stage is not implemented; implemented in task ";
        for (kind, source) in [
            ("input", input.as_str()),
            ("expectation", expected.as_str()),
        ] {
            let Some(task) = source.split_once(placeholder).and_then(|(_, tail)| {
                tail.split(|ch: char| !ch.is_ascii_alphanumeric() && ch != '.' && ch != '-')
                    .next()
            }) else {
                return Err(format!(
                    "pixels plan lint: {directory_name} {kind} lacks a task-owned placeholder"
                ));
            };
            if !tasks.contains(task) {
                return Err(format!(
                    "pixels plan lint: {directory_name} {kind} names nonexistent task `{task}`"
                ));
            }
        }
        let lower_input = input.to_ascii_lowercase();
        for forbidden in [
            "dense edge mask",
            "renderer_hint",
            "precomputed renderer hint",
        ] {
            if lower_input.contains(forbidden) {
                return Err(format!(
                    "pixels plan lint: {directory_name} source contains forbidden hint `{forbidden}`"
                ));
            }
        }
        if matches!(
            directory_name,
            "check-pixels-close-depth"
                | "check-pixels-thin-feature"
                | "check-pixels-enclosed-feature"
        ) && (!input.contains("_RAW:") || !input.contains("_FRAC_BITS:"))
        {
            return Err(format!(
                "pixels plan lint: {directory_name} must encode its adversarial geometry as exact dyadic raw constants"
            ));
        }
        discovered.insert(directory_name.to_string());
    }
    compare_fixture_names(&matrix, &manifest, census_count, &census, &discovered)
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
    println!("pixels-plan-lint: 154 tasks, canonical contracts, and 38 fixtures match");
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

    #[test]
    fn fixture_matrix_manifest_census_and_discovery_match() {
        let matrix = matrix_fixture_names(&actual()).unwrap();
        let manifest =
            manifest_fixture_names(&std::fs::read_to_string(root().join(PIXELS_CASES)).unwrap())
                .unwrap();
        let (count, census) = census_fixture_names(
            &std::fs::read_to_string(root().join("tests/census.toml")).unwrap(),
        )
        .unwrap();
        let discovered: BTreeSet<String> = matrix.iter().cloned().collect();
        compare_fixture_names(&matrix, &manifest, count, &census, &discovered).unwrap();
    }

    #[test]
    fn fixture_deletion_or_unregistered_class_fails() {
        let names = vec!["check-pixels-a".to_string(), "err-pixels-b".to_string()];
        let missing: BTreeSet<String> = ["check-pixels-a".to_string()].into_iter().collect();
        assert!(
            compare_fixture_names(&names, &names, names.len(), &names, &missing)
                .unwrap_err()
                .contains("missing")
        );

        let extra: BTreeSet<String> = names
            .iter()
            .cloned()
            .chain(["boot-pixels-c".to_string()])
            .collect();
        assert!(
            compare_fixture_names(&names, &names, names.len(), &names, &extra)
                .unwrap_err()
                .contains("extra")
        );
    }
}
