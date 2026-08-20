use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use crate::root;

const PLAN: &str = "docs/designs/WRELA_PIXELS_COMPILER_IMPLEMENTATION_PLAN.md";
const P8R_PLAN: &str = "docs/designs/WRELA_PIXELS_P8R_TIGHTENING_PLAN.md";
const INVARIANT_MATRIX: &str = "docs/designs/WRELA_PIXELS_INVARIANT_MATRIX.md";
const PACKET_CONSUMER_MATRIX: &str = "docs/designs/WRELA_PIXELS_PACKET_CONSUMER_MATRIX.md";
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

fn fixture_wrela_sources(path: &Path) -> Result<String, String> {
    fn collect(path: &Path, files: &mut Vec<PathBuf>) -> Result<(), String> {
        let mut entries = std::fs::read_dir(path)
            .map_err(|error| format!("read {}: {error}", path.display()))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| format!("read {}: {error}", path.display()))?;
        entries.sort_by_key(std::fs::DirEntry::file_name);
        for entry in entries {
            let entry_path = entry.path();
            if entry_path.is_dir() {
                collect(&entry_path, files)?;
            } else if entry_path
                .extension()
                .is_some_and(|extension| extension == "wr")
            {
                files.push(entry_path);
            }
        }
        Ok(())
    }

    let mut files = Vec::new();
    collect(path, &mut files)?;
    let mut source = String::new();
    for file in files {
        source.push_str(
            &std::fs::read_to_string(&file)
                .map_err(|error| format!("read {}: {error}", file.display()))?,
        );
        source.push('\n');
    }
    Ok(source)
}

fn task_id(line: &str) -> Option<&str> {
    line.strip_prefix("## Task ")?
        .split_once(" — ")
        .map(|(id, _)| id.trim())
}

/// Basis markers that excuse a `**Files:**` entry from having to exist yet.
///
/// One marker per milestone basis. A task that plans a file into existence
/// names the basis it was planned at, so a stale marker is visible instead of
/// a silently missing path.
const NEW_FILE_MARKERS: &[&str] = &["new at P-1 basis", "new at P8 basis"];

fn marks_planned_file(content: &str) -> bool {
    NEW_FILE_MARKERS
        .iter()
        .any(|marker| content.contains(marker))
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
    lint_p8r_chain_link(text)?;
    lint_display_contract(repo)?;
    lint_normative_docs(repo)?;
    lint_formal_readme(repo)?;
    lint_p9_normal_terminal(repo)?;
    lint_permanent_fixtures(text, repo, &headings)?;
    Ok(())
}

fn lint_p9_normal_terminal(repo: &Path) -> Result<(), String> {
    let light_path = repo.join("stdlib/core/render_light.wr");
    let light = std::fs::read_to_string(&light_path)
        .map_err(|error| format!("read {}: {error}", light_path.display()))?;
    let event_path = repo.join("stdlib/core/render_orchestrate.wr");
    let event = std::fs::read_to_string(&event_path)
        .map_err(|error| format!("read {}: {error}", event_path.display()))?;
    for (owner, source, needle) in [
        (
            "regular shading",
            light.as_str(),
            "if encoded[0] == 1 and not normal_terminal_required:",
        ),
        (
            "event shading",
            event.as_str(),
            "if encoded[0] == 1 and not normal_terminal_required:",
        ),
        (
            "normal curvature",
            light.as_str(),
            "material[13] = 400000000000000000000000.0",
        ),
    ] {
        if source.matches(needle).count() != 1 {
            return Err(format!(
                "pixels plan lint: P9 {owner} must contain exactly one `{needle}` guard"
            ));
        }
    }
    Ok(())
}

/// Task IDs of the P8R tightening plan, in execution order.
const P8R_TASKS: &[&str] = &[
    "P8R.0", "P8R.1", "P8R.2", "P8R.3", "P8R.4", "P8R.5", "P8R.6", "P8R.7",
];

/// The canonical plan task whose prerequisite must name the P8R close.
const P8R_CLOSE_TASK: &str = "P8R.7";

/// Lint the P8R tightening plan against the §10.0 executor schema.
///
/// The P8R plan is a separate document from the canonical plan, so the
/// canonical §14 commit-order cross-check does not apply to it. What does
/// apply is the required-section schema, a unique ordered task list, and the
/// same "a `**Files:**` path either exists or is marked as planned" rule.
fn lint_p8r_schema(text: &str, repo: &Path) -> Result<(), String> {
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
    if headings != P8R_TASKS {
        return Err(format!(
            "pixels plan lint: P8R task headings are {headings:?}, expected {P8R_TASKS:?}"
        ));
    }
    starts.push(text.len());
    for (index, id) in headings.iter().enumerate() {
        let body = &text[starts[index]..starts[index + 1]];
        for field in REQUIRED_FIELDS {
            let needle = format!("**{field}:**");
            if !body.contains(&needle) {
                return Err(format!(
                    "pixels plan lint: P8R task {id} is missing field `{field}`"
                ));
            }
        }
    }
    lint_marked_paths(text, repo)
}

fn lint_invariant_matrix(text: &str, repo: &Path) -> Result<(), String> {
    const REQUIRED_AREAS: &[&str] = &[
        "LOWERING",
        "EVENTS",
        "EXCLUSION",
        "PROJECTIVE",
        "CAPACITY",
        "PLACEMENT",
        "SNAPSHOT",
        "CERTIFY",
        "COVERAGE",
        "QUANTIZE",
        "DISPLAY",
        "EVIDENCE",
        "REPLAY",
        "FAILURE",
    ];
    let mut ids = BTreeSet::new();
    let mut areas = BTreeSet::new();
    let mut rows = 0usize;
    for line in text.lines().filter(|line| line.starts_with("| INV-PIX-")) {
        let cells = line.split('|').map(str::trim).collect::<Vec<_>>();
        if cells.len() != 11 || !cells[0].is_empty() || !cells[10].is_empty() {
            return Err(format!(
                "pixels plan lint: invariant row must have exactly 9 columns: `{line}`"
            ));
        }
        let id = cells[1];
        let Some(rest) = id.strip_prefix("INV-PIX-") else {
            unreachable!("row filter established the prefix")
        };
        let Some((area, number)) = rest.rsplit_once('-') else {
            return Err(format!("pixels plan lint: malformed invariant ID `{id}`"));
        };
        if area.is_empty()
            || !area
                .bytes()
                .all(|byte| byte.is_ascii_uppercase() || byte == b'-')
            || number.len() != 3
            || !number.bytes().all(|byte| byte.is_ascii_digit())
        {
            return Err(format!("pixels plan lint: malformed invariant ID `{id}`"));
        }
        if !ids.insert(id.to_string()) {
            return Err(format!("pixels plan lint: duplicate invariant ID `{id}`"));
        }
        areas.insert(area.to_string());
        if cells[2].is_empty() {
            return Err(format!(
                "pixels plan lint: invariant `{id}` has no description"
            ));
        }
        for cell in &cells[3..10] {
            let covered = cell
                .strip_prefix("covered(")
                .and_then(|value| value.strip_suffix(')'));
            if let Some(artifact) = covered {
                if artifact.is_empty() || artifact.contains(char::is_whitespace) {
                    return Err(format!(
                        "pixels plan lint: invariant `{id}` has malformed covered artifact `{cell}`"
                    ));
                }
                let (path, anchor) = artifact
                    .split_once('#')
                    .map_or((artifact, None), |(path, anchor)| (path, Some(anchor)));
                if !repo.join(path).exists() {
                    return Err(format!(
                        "pixels plan lint: invariant `{id}` cites missing artifact `{path}`"
                    ));
                }
                if let Some(anchor) = anchor {
                    if anchor.is_empty() {
                        return Err(format!(
                            "pixels plan lint: invariant `{id}` cites an empty anchor in `{cell}`"
                        ));
                    }
                    let artifact_text = std::fs::read_to_string(repo.join(path)).map_err(|error| {
                        format!(
                            "pixels plan lint: read covered artifact `{path}` for invariant `{id}`: {error}"
                        )
                    })?;
                    if !artifact_text.contains(anchor) {
                        return Err(format!(
                            "pixels plan lint: invariant `{id}` cites missing anchor `{anchor}` in artifact `{path}`"
                        ));
                    }
                }
                continue;
            }
            if cell
                .strip_prefix("not-applicable(")
                .and_then(|value| value.strip_suffix(')'))
                .is_some_and(|reason| !reason.is_empty())
                || cell
                    .strip_prefix("planned(")
                    .and_then(|value| value.strip_suffix(')'))
                    .is_some_and(|task| task.starts_with('P') && task.contains('.'))
            {
                continue;
            }
            let gap = cell
                .strip_prefix("blocking-gap(")
                .and_then(|value| value.strip_suffix(')'));
            let deferred = cell
                .strip_prefix("accepted-deferred(")
                .and_then(|value| value.strip_suffix(')'));
            if let Some(details) = gap.or(deferred) {
                let has_decision = details.split(';').any(|part| {
                    let part = part.trim();
                    part.len() == 9
                        && part.starts_with("D-P8R-")
                        && part[7..].bytes().all(|byte| byte.is_ascii_digit())
                });
                let has_owner = details
                    .split(';')
                    .any(|part| part.trim().starts_with("owner="));
                let has_milestone = deferred.is_none_or(|_| {
                    details
                        .split(';')
                        .any(|part| part.trim().starts_with("milestone="))
                });
                if has_decision && has_owner && has_milestone {
                    continue;
                }
            }
            return Err(format!(
                "pixels plan lint: invariant `{id}` has noncanonical status cell `{cell}`"
            ));
        }
        rows += 1;
    }
    if rows == 0 {
        return Err("pixels plan lint: invariant matrix has no rows".to_string());
    }
    for required in REQUIRED_AREAS {
        if !areas.contains(*required) {
            return Err(format!(
                "pixels plan lint: invariant matrix lacks required `{required}` coverage"
            ));
        }
    }
    Ok(())
}

fn canonical_packet_needs(plan: &str) -> Result<BTreeMap<String, String>, String> {
    const PREFIX: &str = "[P9-PACKET-NEED:";
    let mut needs = BTreeMap::new();
    let mut rest = plan;
    while let Some((_, after)) = rest.split_once(PREFIX) {
        let (payload, tail) = after.split_once(']').ok_or_else(|| {
            "pixels plan lint: unterminated canonical P9 packet-need marker".to_string()
        })?;
        rest = tail;
        let (id, kind) = payload.rsplit_once(':').ok_or_else(|| {
            format!("pixels plan lint: malformed canonical packet need `{payload}`")
        })?;
        if !matches!(kind, "packet" | "scalar") || !id.starts_with("P9.") {
            return Err(format!(
                "pixels plan lint: malformed canonical packet need `{payload}`"
            ));
        }
        if needs.insert(id.to_string(), kind.to_string()).is_some() {
            return Err(format!(
                "pixels plan lint: duplicate canonical packet need `{id}`"
            ));
        }
    }
    if needs.is_empty() {
        return Err("pixels plan lint: canonical plan has no packet-need IDs".to_string());
    }
    Ok(needs)
}

fn lint_packet_consumer_matrix(text: &str, plan: &str) -> Result<(), String> {
    const HEADER: &str = "| need ID | P9 task | packet need | landed operation(s) | resolution |";
    const EXPECTED_TASK_ROWS: &[(&str, usize)] = &[
        ("P9.4", 2),
        ("P9.5", 1),
        ("P9.6", 2),
        ("P9.7", 1),
        ("P9.8", 1),
        ("P9.9", 1),
        ("P9.10", 2),
        ("P9.11", 1),
    ];

    if text.lines().filter(|line| *line == HEADER).count() != 1 {
        return Err(
            "pixels plan lint: packet consumer matrix must have one canonical header".into(),
        );
    }
    let mut in_table = false;
    let mut rows = Vec::new();
    for line in text.lines() {
        if line == HEADER {
            in_table = true;
            continue;
        }
        if !in_table {
            continue;
        }
        if line.starts_with("|---") {
            continue;
        }
        if !line.starts_with('|') {
            break;
        }
        let cells = line.split('|').map(str::trim).collect::<Vec<_>>();
        if cells.len() != 7 || !cells[0].is_empty() || !cells[6].is_empty() {
            return Err(format!(
                "pixels plan lint: packet consumer row must have exactly five columns: `{line}`"
            ));
        }
        if cells[1].is_empty()
            || cells[2].is_empty()
            || cells[3].is_empty()
            || cells[4].is_empty()
            || cells[5].is_empty()
        {
            return Err(format!(
                "pixels plan lint: packet consumer row contains an empty cell: `{line}`"
            ));
        }
        rows.push((cells[1], cells[2], cells[3], cells[4], cells[5]));
    }

    let canonical_needs = canonical_packet_needs(plan)?;
    let mut task_counts = BTreeMap::new();
    let mut row_identities = BTreeSet::new();
    let mut matrix_need_ids = BTreeSet::new();
    let mut matrix_operations = BTreeSet::new();
    let implemented_operations = wrela_compiler::mwir::PIXELS_PACKET_OPERATION_NAMES
        .iter()
        .copied()
        .collect::<BTreeSet<_>>();
    for (need_id, task_cell, need, operations, resolution) in rows {
        let mut task_parts = task_cell.splitn(2, ' ');
        let task = task_parts.next().unwrap_or("");
        if task_parts.next().is_none_or(str::is_empty) {
            return Err(format!(
                "pixels plan lint: packet consumer row `{task_cell}` must name its task and consumer"
            ));
        }
        if !EXPECTED_TASK_ROWS
            .iter()
            .any(|(expected, _)| task == *expected)
        {
            return Err(format!(
                "pixels plan lint: packet consumer matrix has unexpected task `{task}`"
            ));
        }
        *task_counts.entry(task).or_insert(0usize) += 1;
        if !need_id.starts_with(&format!("{task}-")) {
            return Err(format!(
                "pixels plan lint: need ID `{need_id}` does not belong to task `{task}`"
            ));
        }
        let expected_kind = canonical_needs.get(need_id).ok_or_else(|| {
            format!("pixels plan lint: matrix need `{need_id}` has no canonical P9 marker")
        })?;
        if !matrix_need_ids.insert(need_id) {
            return Err(format!(
                "pixels plan lint: packet consumer matrix duplicates need ID `{need_id}`"
            ));
        }
        if !row_identities.insert((task_cell, need)) {
            return Err(format!(
                "pixels plan lint: packet consumer matrix duplicates `{task}` need `{need}`"
            ));
        }

        let landed = resolution == "landed" || resolution.starts_with("landed; ");
        let deliberately_scalar = resolution.starts_with("deliberately scalar; ");
        if !landed && !deliberately_scalar {
            return Err(format!(
                "pixels plan lint: packet consumer `{task}` has unresolved state `{resolution}`"
            ));
        }
        if deliberately_scalar {
            if expected_kind != "scalar" || operations != "scalar" {
                return Err(format!(
                    "pixels plan lint: `{need_id}` contradicts its canonical `{expected_kind}` resolution"
                ));
            }
            continue;
        }
        if expected_kind != "packet" {
            return Err(format!(
                "pixels plan lint: `{need_id}` must use its canonical scalar resolution"
            ));
        }
        if operations == "scalar" {
            return Err(format!(
                "pixels plan lint: landed packet row `{task}` cannot name scalar operations"
            ));
        }
        let mut row_operations = BTreeSet::new();
        for operation in operations.split(", ") {
            if !implemented_operations.contains(operation) {
                return Err(format!(
                    "pixels plan lint: packet consumer `{task}` names unsupported operation `{operation}`"
                ));
            }
            if !row_operations.insert(operation) {
                return Err(format!(
                    "pixels plan lint: packet consumer `{task}` duplicates operation `{operation}`"
                ));
            }
            matrix_operations.insert(operation);
        }
    }
    let canonical_need_ids = canonical_needs
        .keys()
        .map(String::as_str)
        .collect::<BTreeSet<_>>();
    if matrix_need_ids != canonical_need_ids {
        let missing = canonical_need_ids
            .difference(&matrix_need_ids)
            .copied()
            .collect::<Vec<_>>();
        let extra = matrix_need_ids
            .difference(&canonical_need_ids)
            .copied()
            .collect::<Vec<_>>();
        return Err(format!(
            "pixels plan lint: canonical packet-need closure mismatch: missing={missing:?} extra={extra:?}"
        ));
    }
    for (task, expected) in EXPECTED_TASK_ROWS {
        let actual = task_counts.get(task).copied().unwrap_or(0);
        if actual != *expected {
            return Err(format!(
                "pixels plan lint: packet consumer matrix has {actual} row(s) for {task}, expected {expected}"
            ));
        }
    }
    if matrix_operations != implemented_operations {
        let missing = implemented_operations
            .difference(&matrix_operations)
            .copied()
            .collect::<Vec<_>>();
        let extra = matrix_operations
            .difference(&implemented_operations)
            .copied()
            .collect::<Vec<_>>();
        return Err(format!(
            "pixels plan lint: packet operation closure mismatch: missing={missing:?} extra={extra:?}"
        ));
    }
    Ok(())
}

/// Prove the canonical task chain reaches P8R: Task P9.1 requires the P8R
/// close, and the P8 close names the interstitial milestone's plan.
fn lint_p8r_chain_link(plan: &str) -> Result<(), String> {
    let p9_1 = section(plan, "## Task P9.1 — ", "## Task P9.2 — ")?;
    let requires = section(p9_1, "**Requires:**", "**Produces:**")?;
    if !requires.contains(P8R_CLOSE_TASK) {
        return Err(format!(
            "pixels plan lint: Task P9.1 must require Task {P8R_CLOSE_TASK}; its \
             prerequisite reads `{}`",
            requires.trim()
        ));
    }
    let seam = section(plan, "### Milestone P8 close", "# Milestone P9 — ")?;
    if !seam.contains(P8R_PLAN.trim_start_matches("docs/designs/")) {
        return Err(
            "pixels plan lint: the P8 close / P9 entry seam must point at the P8R plan".to_string(),
        );
    }
    Ok(())
}

/// Roots scanned for decision-ID references. Everything a P8R decision could
/// legitimately be cited from: documents, compiler and tool sources, Wrela
/// sources, locked thresholds, and fixtures.
const DECISION_SCAN_ROOTS: &[&str] = &["docs", "crates", "stdlib", "bench", "tests", "formal"];

/// Extensions scanned for decision-ID references.
const DECISION_SCAN_EXTENSIONS: &[&str] = &["md", "rs", "wr", "toml", "txt"];

/// This file defines the registry syntax and its negative tests, so its own
/// literals are not tree evidence.
const DECISION_SCAN_EXCLUDED: &[&str] = &["crates/xtask/src/pixels_plan_lint.rs"];

const DECISION_PREFIX: &str = "D-P8R-";

/// One `D-P8R-nn` occurrence found in the tree.
#[derive(Debug, Clone, PartialEq, Eq)]
struct DecisionCitation {
    id: String,
    path: String,
    is_definition: bool,
}

/// Collect every `D-P8R-nn` occurrence in `text`, classifying each as a
/// definition or a reference.
///
/// A definition is the sealed form `**D-P8R-nn** (sealed YYYY-MM-DD) — `.
/// Every other occurrence is a reference. The two forms are deliberately far
/// apart so prose cannot accidentally define a decision.
fn decision_citations(path: &str, text: &str) -> Result<Vec<DecisionCitation>, String> {
    let mut found = Vec::new();
    let bytes = text.as_bytes();
    let mut at = 0usize;
    while let Some(relative) = text[at..].find(DECISION_PREFIX) {
        let start = at + relative;
        let digits_at = start + DECISION_PREFIX.len();
        at = digits_at;
        let digits = &text[digits_at..text.len().min(digits_at + 2)];
        // `D-P8R-nn` is the schema placeholder these documents use when they
        // describe the registry rather than cite a decision.
        if digits == "nn" {
            continue;
        }
        if digits.len() != 2 || !digits.bytes().all(|b| b.is_ascii_digit()) {
            return Err(format!(
                "pixels plan lint: {path} cites `{DECISION_PREFIX}` without a two-digit ID"
            ));
        }
        let after = digits_at + 2;
        let is_definition = text[..start].ends_with("**")
            && text[after..].starts_with("** (sealed ")
            && text[after + "** (sealed ".len()..].starts_with(|c: char| c.is_ascii_digit());
        // A longer numeric tail would silently truncate to a different ID.
        if bytes.get(after).is_some_and(u8::is_ascii_digit) {
            return Err(format!(
                "pixels plan lint: {path} cites `{DECISION_PREFIX}` with more than two digits"
            ));
        }
        found.push(DecisionCitation {
            id: format!("{DECISION_PREFIX}{digits}"),
            path: path.to_string(),
            is_definition,
        });
    }
    Ok(found)
}

/// Every `D-P8R-nn` referenced anywhere resolves to exactly one definition.
fn lint_decision_registry(citations: &[DecisionCitation]) -> Result<(), String> {
    let mut ids: Vec<&str> = citations.iter().map(|c| c.id.as_str()).collect();
    ids.sort_unstable();
    ids.dedup();
    for id in ids {
        let definitions: Vec<&DecisionCitation> = citations
            .iter()
            .filter(|c| c.id == id && c.is_definition)
            .collect();
        match definitions.len() {
            1 => {}
            0 => {
                let sites: BTreeSet<&str> = citations
                    .iter()
                    .filter(|c| c.id == id)
                    .map(|c| c.path.as_str())
                    .collect();
                return Err(format!(
                    "pixels plan lint: decision `{id}` is referenced by {sites:?} but never \
                     sealed; a definition reads `**{id}** (sealed YYYY-MM-DD) — ...`"
                ));
            }
            n => {
                let sites: Vec<&str> = definitions.iter().map(|c| c.path.as_str()).collect();
                return Err(format!(
                    "pixels plan lint: decision `{id}` is sealed {n} times, in {sites:?}; \
                     exactly one normative document owns each decision"
                ));
            }
        }
    }
    Ok(())
}

/// Walk the decision-scan roots, in a deterministic order.
fn decision_scan_files(repo: &Path) -> Result<Vec<PathBuf>, String> {
    fn collect(path: &Path, files: &mut Vec<PathBuf>) -> Result<(), String> {
        let mut entries = std::fs::read_dir(path)
            .map_err(|error| format!("read {}: {error}", path.display()))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| format!("read {}: {error}", path.display()))?;
        entries.sort_by_key(std::fs::DirEntry::file_name);
        for entry in entries {
            let entry_path = entry.path();
            let name = entry.file_name();
            let name = name.to_string_lossy();
            // Build outputs and proof-tool caches are not repository evidence.
            if name == "target" || name == ".lake" || name.starts_with('.') {
                continue;
            }
            if entry_path.is_dir() {
                collect(&entry_path, files)?;
            } else if entry_path.extension().is_some_and(|extension| {
                DECISION_SCAN_EXTENSIONS
                    .iter()
                    .any(|wanted| extension == *wanted)
            }) {
                files.push(entry_path);
            }
        }
        Ok(())
    }

    let mut files = Vec::new();
    for root_name in DECISION_SCAN_ROOTS {
        let root_path = repo.join(root_name);
        if root_path.is_dir() {
            collect(&root_path, &mut files)?;
        }
    }
    Ok(files)
}

/// Gather decision citations across the tree.
fn tree_decision_citations(repo: &Path) -> Result<Vec<DecisionCitation>, String> {
    let mut citations = Vec::new();
    for path in decision_scan_files(repo)? {
        let relative = path
            .strip_prefix(repo)
            .unwrap_or(&path)
            .to_string_lossy()
            .replace('\\', "/");
        if DECISION_SCAN_EXCLUDED.contains(&relative.as_str()) {
            continue;
        }
        let text = std::fs::read_to_string(&path)
            .map_err(|error| format!("read {}: {error}", path.display()))?;
        if !text.contains(DECISION_PREFIX) {
            continue;
        }
        citations.extend(decision_citations(&relative, &text)?);
    }
    Ok(citations)
}

/// The losing `RTDATA_BASE` statements struck by D-P8R-09.
fn superseded_rtdata_base_values() -> [String; 2] {
    [
        format!("IMAGE_BASE + {} MiB", 2),
        format!("0x{:04x}_{:04x}", 0x4054, 0),
    ]
}

/// D-P8R-09: the machine chapter states the packing base the machine crate
/// actually defines, and no live source restates the losing value. Markdown
/// may retain it only in an explicitly historical paragraph.
fn lint_rtdata_base(repo: &Path) -> Result<(), String> {
    let base = wrela_machine::layout::RTDATA_BASE;
    let image_base = wrela_machine::layout::IMAGE_BASE;
    let mib = (base - image_base) / (1 << 20);
    if image_base + mib * (1 << 20) != base {
        return Err(format!(
            "pixels plan lint: RTDATA_BASE ({base:#x}) is not a whole number of MiB above \
             IMAGE_BASE ({image_base:#x}); restate the machine chapter by hand"
        ));
    }
    let chapter_path = repo.join("docs/language/06-machine.md");
    let chapter = std::fs::read_to_string(&chapter_path)
        .map_err(|error| format!("read {}: {error}", chapter_path.display()))?;
    for needle in [
        format!("`RTDATA_BASE = IMAGE_BASE + {mib} MiB`"),
        format!("(`{:#06x}_{:04x}`)", base >> 16, base & 0xffff),
    ] {
        if !chapter.contains(&needle) {
            return Err(format!(
                "pixels plan lint: docs/language/06-machine.md must state the machine's own \
                 packing base; missing `{needle}`"
            ));
        }
    }

    for path in decision_scan_files(repo)? {
        let relative = path
            .strip_prefix(repo)
            .unwrap_or(&path)
            .to_string_lossy()
            .replace('\\', "/");
        let text = std::fs::read_to_string(&path)
            .map_err(|error| format!("read {}: {error}", path.display()))?;
        if path.extension().is_some_and(|extension| extension == "md") {
            lint_superseded_rtdata_base(&relative, &text)?;
        } else {
            lint_live_rtdata_base(&relative, &text)?;
        }
    }
    Ok(())
}

fn lint_live_rtdata_base(path: &str, text: &str) -> Result<(), String> {
    for superseded in superseded_rtdata_base_values() {
        if text.contains(&superseded) {
            return Err(format!(
                "pixels plan lint: {path} contains the superseded RTDATA_BASE value \
                 `{superseded}` in live source"
            ));
        }
    }
    Ok(())
}

/// A superseded `RTDATA_BASE` value may appear only in a paragraph that cites
/// the decision that superseded it, which is what makes it historical text
/// rather than a second live contract.
fn lint_superseded_rtdata_base(path: &str, text: &str) -> Result<(), String> {
    for paragraph in text.split("\n\n") {
        for superseded in superseded_rtdata_base_values() {
            if paragraph.contains(&superseded) && !paragraph.contains("D-P8R-09") {
                return Err(format!(
                    "pixels plan lint: {path} restates the superseded RTDATA_BASE value \
                     `{superseded}` outside a paragraph citing D-P8R-09"
                ));
            }
        }
    }
    Ok(())
}

fn lint_formal_readme(repo: &Path) -> Result<(), String> {
    let path = repo.join("formal/pixels/README.md");
    let readme = std::fs::read_to_string(&path)
        .map_err(|error| format!("read {}: {error}", path.display()))?;
    let words = readme.split_whitespace().collect::<Vec<_>>().join(" ");
    for required in [
        "Lean 4.30.0",
        "Mathlib 4.30.0",
        "does not modify this repository",
        "elan toolchain install leanprover/lean4:v4.30.0",
        "lake exe cache get",
        "normative project check",
        "lake build",
        "cargo xtask pixels-formal-scan",
        "does not certify arbitrary",
        "compiler-side proof-object checks",
    ] {
        if !words.contains(required) {
            return Err(format!(
                "pixels plan lint: formal project README lacks `{required}`"
            ));
        }
    }
    Ok(())
}

fn lint_normative_docs(repo: &Path) -> Result<(), String> {
    let pixels_path = repo.join("docs/language/07-pixels.md");
    let pixels = std::fs::read_to_string(&pixels_path)
        .map_err(|error| format!("read {}: {error}", pixels_path.display()))?;
    for heading in [
        "## 0. Delivered contract",
        "### 0.1 Definition of done",
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
    let normalized_pixels = pixels.split_whitespace().collect::<Vec<_>>().join(" ");
    let mut ordering_cursor = 0;
    for ordering_key in [
        "renderer declaration order in the sealed image graph",
        "canonical callee key",
        "source span `(module path, byte start, byte end)`",
        "structural child IDs",
        "exact immediate bits",
    ] {
        let Some(relative) = normalized_pixels[ordering_cursor..].find(ordering_key) else {
            return Err(format!(
                "pixels plan lint: normative canonical ordering lacks `{ordering_key}`"
            ));
        };
        ordering_cursor += relative + ordering_key.len();
    }
    for contract in [
        "correct without",
        "without dense truth, a sample lattice, a dense edge mask, or previous-frame",
        "Kinetic maintenance is optional",
        "root existence, root uniqueness,",
        "`Length3` remains a fused scalar operation",
        "must remain defined at zero",
        "generated Wrela scalar kernels",
        "machine-v1 display conformance lane",
        "exact sealed renderer cycle proxy",
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
    if names.len() != 44 {
        return Err(format!(
            "pixels plan lint: §11 contains {} permanent fixtures, expected 44",
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

fn fixture_source_requirements(name: &str) -> &'static [&'static str] {
    match name {
        "check-pixels-plane" => &["plane(", "PLANE_NORMAL_Y_RAW"],
        "check-pixels-hard-csg" => &["union(", "intersection(", "subtract("],
        "check-pixels-smooth-csg" => &["smooth_union(", "k=0.25"],
        "check-pixels-repeat" => &["finite_repeat_x(", "count=5", "first=-2"],
        "check-pixels-displace" => &["sinusoidal_displace(", "amplitude=0.125"],
        "check-pixels-close-depth" => &["z=0.99609375", "FRONT_Q_RAW", "BACK_Q_RAW"],
        "check-pixels-thin-feature" => &["x=0.00390625", "HALF_WIDTH_RAW"],
        "check-pixels-enclosed-feature" => &["subtract(", "radius=0.00390625"],
        "check-pixels-material-edge" => &["MaterialId.Accent", "LEFT_MATERIAL_ID"],
        "check-pixels-transparent-tail" => &["LAYER_COUNT", "z=1.5", "radius=0.25"],
        "check-pixels-area-light" => &[
            "LightConfig.declared(",
            "capacity=1",
            "LightKind.Rectangle",
            "EMITTER_HALF_WIDTH_RAW",
        ],
        "check-pixels-kinetic" => &["@rate(", "params.phase", "FRAME_COUNT"],
        "check-pixels-camera-inside" => &["radius=2.0", "CAMERA_X_RAW"],
        "check-pixels-torus-roots" => &["torus(", "major_radius=2.0"],
        "check-pixels-tangent" => &["y=1.0", "radius=1.0", "RAY_HEIGHT_RAW"],
        "check-pixels-simultaneous-event" => &["x=-1.0", "x=1.0", "EVENT_FRAME_INDEX"],
        "check-pixels-tile-boundary" => &["TILE_WIDTH", "EDGE_X"],
        "check-pixels-fixed-q-range" => &["offset=-64.0", "Q_FRAC_BITS"],
        "check-pixels-texture-seam" => &["U_LEFT_RAW", "U_RIGHT_RAW", "UV_FRAC_BITS"],
        "check-pixels-normal-moments" => &["round_box(", "sinusoidal_displace("],
        "check-pixels-probe-wall" => &["WALL_X_RAW", "x=-1.0", "x=1.0"],
        "check-pixels-probe-shift" => &["ProbeConfig.fixed(enabled=true)", "params.phase"],
        "err-pixels-unsupported-op" => &["unsupported_twist(", "p.x * p.y"],
        "err-pixels-missing-range" => &[
            "fn motion(read params: SceneParams) -> Motion:",
            "const INDEX: usize = 1",
            "return params.motions[INDEX]",
            "selected = wrap(motion(params).phase)",
            "radius=selected.value",
            "struct Motion:\n    phase: f32",
            "struct Wrapper:\n    value: f32",
        ],
        "err-pixels-rate" => &["max_delta=-0.25", "NEGATIVE_DELTA_RAW"],
        "err-pixels-topology-branch" => &[
            "if true:",
            "enabled = params.enabled",
            "if enabled:",
            "sphere(",
            "box(",
        ],
        "err-pixels-repeat-unbounded" => &[
            "count=4",
            "period=params.phase",
            "world_min=Vec3(x=-64.0, y=-64.0, z=-64.0)",
        ],
        "err-pixels-capacity" => &["count=257", "REQUIRED_FEATURES"],
        "err-pixels-projective-zero" => &["near=0.0", "DENOMINATOR_RAW"],
        "err-pixels-fixed-q" => &["near=0.000000000000000000000000000001", "REQUIRED_RAW_BITS"],
        "err-pixels-tone-table" => &["INVALID_TONE_TABLE", "[TABLE_LEFT_CODE, TABLE_RIGHT_CODE]"],
        "err-pixels-cost" => &["width=1920", "height=1080", "initialization_deadline_ms=1"],
        "boot-pixels-numeric" => &["interval_add(", "byte_singleton(", "VECTOR_COUNT"],
        "boot-pixels-plane" => &["plane(", "WIDTH", "HEIGHT"],
        "boot-pixels-plane-one-core" => &["plane(", "WIDTH", "HEIGHT", "cores=1"],
        "boot-pixels-quality" => &[
            "plane(",
            "NormalDetail.TextureSlopeUv(",
            "AoConfig.fixed(enabled=true)",
            "cores=3",
        ],
        "boot-pixels-transparent" => &["TRANSPARENT_LAYERS", "z=1.5", "radius=0.25"],
        "boot-pixels-gi" => &["ProbeConfig.fixed(enabled=true)", "PROBE_LEVELS"],
        "boot-pixels-kinetic" => &["@rate(", "params.phase", "SEQUENCE_FRAMES"],
        _ => &[],
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FixtureActivation<'a> {
    Pending(&'a str),
    Active(&'a str),
}

fn parse_fixture_activation(status: &str) -> Result<FixtureActivation<'_>, String> {
    const PLACEHOLDER: &str = "production Pixels stage is not implemented; implemented in task ";
    if let Some(task) = status.strip_prefix(PLACEHOLDER) {
        Ok(FixtureActivation::Pending(task.trim_end_matches('.')))
    } else if let Some(task) = status.strip_prefix("activated; implemented in task ") {
        Ok(FixtureActivation::Active(task.trim_end_matches('.')))
    } else {
        Err(format!("invalid P0 status `{status}`"))
    }
}

fn lint_fixture_placeholder(
    name: &str,
    kind: &str,
    activation: FixtureActivation<'_>,
    source: &str,
) -> Result<(), String> {
    const PLACEHOLDER: &str = "production Pixels stage is not implemented; implemented in task ";
    let source_task = source.split_once(PLACEHOLDER).and_then(|(_, tail)| {
        tail.split(|ch: char| !ch.is_ascii_alphanumeric() && ch != '.' && ch != '-')
            .next()
    });
    match activation {
        FixtureActivation::Pending(task) if source_task != Some(task) => Err(format!(
            "pixels plan lint: {name} pending {kind} must contain its task-owned `{task}` placeholder"
        )),
        FixtureActivation::Active(_) if source_task.is_some() => Err(format!(
            "pixels plan lint: {name} active {kind} retains a P0 placeholder"
        )),
        _ => Ok(()),
    }
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
        let status = readme
            .lines()
            .find_map(|line| line.strip_prefix("P0 status: "))
            .ok_or_else(|| {
                format!(
                    "pixels plan lint: {} lacks a `P0 status:` value",
                    readme_path.display()
                )
            })?;
        let activation = parse_fixture_activation(status).map_err(|message| {
            format!("pixels plan lint: {} has {message}", readme_path.display())
        })?;
        let status_task = match activation {
            FixtureActivation::Pending(task) | FixtureActivation::Active(task) => task,
        };
        if !tasks.contains(status_task) {
            return Err(format!(
                "pixels plan lint: {directory_name} status names nonexistent task `{status_task}`"
            ));
        }
        let input_path = if path.join("root").is_file() {
            let root_path = path.join("root");
            let root_text = std::fs::read_to_string(&root_path)
                .map_err(|error| format!("read {}: {error}", root_path.display()))?;
            let root_file = root_text.trim();
            let relative = Path::new(root_file);
            if root_file.is_empty()
                || relative.is_absolute()
                || relative
                    .components()
                    .any(|component| matches!(component, std::path::Component::ParentDir))
            {
                return Err(format!(
                    "pixels plan lint: {} has invalid project root `{root_file}`",
                    root_path.display()
                ));
            }
            path.join(relative)
        } else {
            path.join("input.wr")
        };
        if !input_path.is_file() {
            return Err(format!(
                "pixels plan lint: fixture source does not exist: {}",
                input_path.display()
            ));
        }
        let check_expected_path = path.join("expected/check.txt");
        let image_expected_path = path.join("expected/image.txt");
        let expected_path = if check_expected_path.is_file() {
            check_expected_path
        } else if image_expected_path.is_file() {
            image_expected_path
        } else {
            return Err(format!(
                "pixels plan lint: fixture has no check/image expectation: {}",
                path.display()
            ));
        };
        let fixture_source = fixture_wrela_sources(&path)?;
        // Instrumented conformance dumps are optional in production source,
        // but when a fixture carries the dump hook it must be reachable. A
        // direct return immediately before the hook silently produced a green
        // production transcript while making the expensive instrumented lane
        // fail only after all guest boots had completed.
        let dump_hook = "dump_header = __wrela_pixels_p7_debug_frame_dump_word";
        let mut search_from = 0usize;
        while let Some(relative) = fixture_source[search_from..].find(dump_hook) {
            let offset = search_from + relative;
            let previous = fixture_source[..offset]
                .lines()
                .rev()
                .find(|line| !line.trim().is_empty())
                .unwrap_or("")
                .trim_start();
            if previous.starts_with("return ") {
                return Err(format!(
                    "pixels plan lint: {directory_name} returns before its instrumented frame dump"
                ));
            }
            search_from = offset + dump_hook.len();
        }
        if directory_name == "check-pixels-tile-boundary" {
            for required in [
                "telemetry_words = telemetry_words_from_dump_counts(dump_counts[1])",
                "dump_header[1] - telemetry_words + 1",
                "@budget(bound=649)",
            ] {
                if !fixture_source.contains(required) {
                    return Err(format!(
                        "pixels plan lint: check-pixels-tile-boundary must derive its complete telemetry tail from the authenticated dump header (`{required}`)"
                    ));
                }
            }
        }
        let expected = std::fs::read_to_string(&expected_path)
            .map_err(|error| format!("read {}: {error}", expected_path.display()))?;
        let scene_requirements: &[&str] = if directory_name == "boot-pixels-numeric" {
            &["@test(runtime)"]
        } else {
            &[
                "@field",
                "@material",
                "@test(runtime)",
                "fn pinned_scene_contract()",
                "@image",
                "img.renderer[SceneParams](",
                "field=world",
                "material=shade",
                "camera_bounds=",
                "light_config=",
                "world_min=",
                "world_max=",
            ]
        };
        for required in scene_requirements
            .into_iter()
            .copied()
            .chain(fixture_source_requirements(directory_name).iter().copied())
        {
            if !fixture_source.contains(required) {
                return Err(format!(
                    "pixels plan lint: {directory_name} does not pin required source `{required}`"
                ));
            }
        }
        lint_fixture_placeholder(directory_name, "input", activation, fixture_source.as_str())?;
        // P1 accepts and seals the source declaration even when the production
        // renderer behavior remains owned by a later milestone. The task
        // sentinel therefore stays in source, while stable expectations must
        // describe the accepted compiler stage rather than repeat a failure.
        let expectation_activation = match (directory_name.starts_with("check-pixels-"), activation)
        {
            (true, FixtureActivation::Pending(task)) | (_, FixtureActivation::Active(task)) => {
                FixtureActivation::Active(task)
            }
            (false, FixtureActivation::Pending(task)) => FixtureActivation::Pending(task),
        };
        lint_fixture_placeholder(
            directory_name,
            "expectation",
            expectation_activation,
            expected.as_str(),
        )?;
        let lower_input = fixture_source.to_ascii_lowercase();
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
        ) && (!fixture_source.contains("_RAW:") || !fixture_source.contains("_FRAC_BITS:"))
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
        "pub fn __wrela_pixels_display_submit_and_wait(doorbell_addr: u64, control_addr: u64) -> unit:"
            .to_string(),
        format!(
            "@offset({:#06x}) tile_pixels_addr: u64",
            wrela_machine::pixels::TILES_BASE - wrela_machine::pixels::CONTROL_BASE
        ),
        "@offset(0x0004) format: u8".to_string(),
        "@offset(0x0005) generation: u8".to_string(),
        "@offset(0x0006) renderer_index: u16".to_string(),
        format!(
            "@offset({:#06x}) guest_visible_digest: [u64; 4]",
            wrela_machine::pixels::GUEST_VISIBLE_DIGEST_OFFSET
        ),
        format!(
            "@offset({:#06x}) guest_raw_tile_digest: [u64; 4]",
            wrela_machine::pixels::GUEST_RAW_TILE_DIGEST_OFFSET
        ),
        format!(
            "@offset({:#06x}) guest_tile_descriptor_digest: [u64; 4]",
            wrela_machine::pixels::GUEST_TILE_DESCRIPTOR_DIGEST_OFFSET
        ),
        format!(
            "@offset({:#06x}) completion_status: u32",
            wrela_machine::pixels::COMPLETION_STATUS_OFFSET
        ),
        "@offset(0x0110) tile_stride_bytes: u16".to_string(),
        "@offset(0x0112) tile_format: u8".to_string(),
        "@offset(0x0113) tile_reserved: [u8; 5]".to_string(),
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
        if !in_files || marks_planned_file(content) {
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
    let p8r = std::fs::read_to_string(repo.join(P8R_PLAN))
        .map_err(|e| format!("read {P8R_PLAN}: {e}"))?;
    lint_p8r_schema(&p8r, &repo)?;
    let invariant_matrix = std::fs::read_to_string(repo.join(INVARIANT_MATRIX))
        .map_err(|e| format!("read {INVARIANT_MATRIX}: {e}"))?;
    lint_invariant_matrix(&invariant_matrix, &repo)?;
    let packet_matrix = std::fs::read_to_string(repo.join(PACKET_CONSUMER_MATRIX))
        .map_err(|e| format!("read {PACKET_CONSUMER_MATRIX}: {e}"))?;
    lint_packet_consumer_matrix(&packet_matrix, &text)?;
    let citations = tree_decision_citations(&repo)?;
    lint_decision_registry(&citations)?;
    lint_rtdata_base(&repo)?;
    let sealed = citations.iter().filter(|c| c.is_definition).count();
    println!(
        "pixels-plan-lint: 154 canonical tasks, {} P8R tasks, canonical contracts, \
         {sealed} sealed decision(s), invariant/packet matrices, and 44 fixtures match",
        P8R_TASKS.len()
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn actual() -> String {
        std::fs::read_to_string(root().join(PLAN)).unwrap()
    }

    fn actual_p8r() -> String {
        std::fs::read_to_string(root().join(P8R_PLAN)).unwrap()
    }

    fn actual_invariant_matrix() -> String {
        std::fs::read_to_string(root().join(INVARIANT_MATRIX)).unwrap()
    }

    #[test]
    fn repository_p8r_plan_passes_its_schema() {
        lint_p8r_schema(&actual_p8r(), &root()).unwrap();
    }

    #[test]
    fn repository_invariant_and_packet_matrices_close() {
        lint_invariant_matrix(&actual_invariant_matrix(), &root()).unwrap();
        let packet = std::fs::read_to_string(root().join(PACKET_CONSUMER_MATRIX)).unwrap();
        lint_packet_consumer_matrix(&packet, &actual()).unwrap();
    }

    #[test]
    fn packet_matrix_rejects_unresolved_rows_and_operation_drift() {
        let packet = std::fs::read_to_string(root().join(PACKET_CONSUMER_MATRIX)).unwrap();

        let unresolved = packet.replacen("| landed |", "| planned |", 1);
        let plan = actual();
        let error = lint_packet_consumer_matrix(&unresolved, &plan).unwrap_err();
        assert!(error.contains("unresolved state"), "{error}");

        let unsupported = packet.replacen("f32x4.add", "f32x4.divide", 1);
        let error = lint_packet_consumer_matrix(&unsupported, &plan).unwrap_err();
        assert!(error.contains("unsupported operation"), "{error}");

        let omitted = packet.replacen(
            "| P9.5-NORMAL-MOMENTS | P9.5 normal moments | accumulate first/second moments | f32x4.load, f32x4.store, f32x4.splat, f32x4.add, f32x4.sub, f32x4.mul, f32x4.min, f32x4.max, f32x4.fma | landed |\n",
            "",
            1,
        );
        let error = lint_packet_consumer_matrix(&omitted, &plan).unwrap_err();
        assert!(error.contains("P9.5"), "{error}");

        let malformed = packet.replacen(
            "| P9.4-MATERIAL-SOA | P9.4 material summaries | load/store four SoA coefficients and broadcast parameters |",
            "| P9.4-MATERIAL-SOA | P9.4 material summaries |",
            1,
        );
        let error = lint_packet_consumer_matrix(&malformed, &plan).unwrap_err();
        assert!(error.contains("exactly five columns"), "{error}");

        let missing_marker = plan.replace(
            "[P9-PACKET-NEED:P9.6-POINT-ATTENUATION:scalar]",
            "[P9 packet need omitted]",
        );
        let error = lint_packet_consumer_matrix(&packet, &missing_marker).unwrap_err();
        assert!(error.contains("no canonical P9 marker"), "{error}");

        let wrong_kind = plan.replace(
            "[P9-PACKET-NEED:P9.6-POINT-ATTENUATION:scalar]",
            "[P9-PACKET-NEED:P9.6-POINT-ATTENUATION:packet]",
        );
        let error = lint_packet_consumer_matrix(&packet, &wrong_kind).unwrap_err();
        assert!(error.contains("contradicts"), "{error}");
    }

    #[test]
    fn invariant_matrix_rejects_missing_artifacts_and_duplicate_ids() {
        let actual = actual_invariant_matrix();
        let missing = actual.replacen(
            "covered(crates/wrela-compiler/src/pixels/symbolic.rs)",
            "covered(crates/wrela-compiler/src/pixels/does-not-exist.rs)",
            1,
        );
        let error = lint_invariant_matrix(&missing, &root()).unwrap_err();
        assert!(error.contains("missing artifact"), "{error}");

        let missing_anchor = actual.replacen(
            "#fn.__wrela_pixels_p7_union_silhouette_coverage_at_slack",
            "#fn.__wrela_pixels_missing_census_target",
            1,
        );
        let error = lint_invariant_matrix(&missing_anchor, &root()).unwrap_err();
        assert!(error.contains("missing anchor"), "{error}");

        let duplicate = actual.replacen("INV-PIX-EVENTS-001", "INV-PIX-LOWERING-001", 1);
        let error = lint_invariant_matrix(&duplicate, &root()).unwrap_err();
        assert!(error.contains("duplicate invariant ID"), "{error}");
    }

    #[test]
    fn p8r_schema_rejects_a_missing_field_and_a_dropped_task() {
        let changed = actual_p8r().replacen("**Stop conditions:**", "**Stopping:**", 1);
        assert!(
            lint_p8r_schema(&changed, &root())
                .unwrap_err()
                .contains("Stop conditions")
        );
        let changed = actual_p8r().replacen(
            "## Task P8R.5 — renderer-internal packet substrate",
            "## Not a task — renderer-internal packet substrate",
            1,
        );
        assert!(
            lint_p8r_schema(&changed, &root())
                .unwrap_err()
                .contains("expected")
        );
    }

    #[test]
    fn p8r_files_accept_the_p8_basis_marker_but_not_an_unmarked_ghost() {
        assert!(marks_planned_file(
            "tests/census/p8-baseline/ # new at P8 basis"
        ));
        assert!(marks_planned_file("stdlib/core/gone.wr # new at P-1 basis"));
        assert!(!marks_planned_file("stdlib/core/gone.wr"));
        // Synthetic rather than tied to a path that happens to be missing
        // today: the point is that an unmarked, nonexistent path fails,
        // whichever path it is.
        let changed = actual_p8r().replacen(
            "crates/wrela-compiler/src/cost/rule.rs",
            "crates/wrela-compiler/src/cost/not_a_real_module.rs",
            1,
        );
        assert!(
            lint_p8r_schema(&changed, &root())
                .unwrap_err()
                .contains("unmarked Files path")
        );
    }

    #[test]
    fn repository_chain_links_p9_1_to_the_p8r_close() {
        lint_p8r_chain_link(&actual()).unwrap();
    }

    #[test]
    fn chain_link_fails_when_p9_1_or_the_seam_forgets_p8r() {
        // Prose around the prerequisite moves; the requirement does not.
        let changed = actual().replace(P8R_CLOSE_TASK, "P8.11");
        assert!(
            lint_p8r_chain_link(&changed)
                .unwrap_err()
                .contains("must require Task P8R.7")
        );
        let changed = actual().replace("WRELA_PIXELS_P8R_TIGHTENING_PLAN.md", "SOME_OTHER_PLAN.md");
        assert!(lint_p8r_chain_link(&changed).unwrap_err().contains("seam"));
    }

    #[test]
    fn repository_decision_registry_resolves_every_reference_once() {
        let citations = tree_decision_citations(&root()).unwrap();
        assert!(
            citations.iter().filter(|c| c.is_definition).count() >= 9,
            "the P8R.0 ledger seals D-P8R-01..09: {citations:?}"
        );
        lint_decision_registry(&citations).unwrap();
    }

    #[test]
    fn decision_citations_separate_the_sealed_form_from_prose() {
        let text = "seal per D-P8R-04 below.\n\n> **D-P8R-04** (sealed 2026-08-15) — no FMA.\n";
        let found = decision_citations("fake.md", text).unwrap();
        assert_eq!(
            found
                .iter()
                .map(|c| (c.id.as_str(), c.is_definition))
                .collect::<Vec<_>>(),
            vec![("D-P8R-04", false), ("D-P8R-04", true)]
        );
        // A bolded mention that is not the sealed form stays a reference.
        let found = decision_citations("fake.md", "**D-P8R-04** is closed.").unwrap();
        assert_eq!(found[0].is_definition, false);

        // Byte offsets immediately following a multi-byte character must not
        // be sliced as though they were UTF-8 character boundaries.
        let found = decision_citations("fake.md", "— D-P8R-04 owns this.").unwrap();
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].id, "D-P8R-04");
        assert!(!found[0].is_definition);
    }

    #[test]
    fn decision_citations_reject_a_malformed_id() {
        assert!(
            decision_citations("fake.md", "see D-P8R-4 for that")
                .unwrap_err()
                .contains("two-digit")
        );
        assert!(
            decision_citations("fake.md", "see D-P8R-041 for that")
                .unwrap_err()
                .contains("more than two digits")
        );
    }

    #[test]
    fn decision_registry_fails_on_a_duplicate_or_dangling_id() {
        let sealed = |path: &str, id: &str| DecisionCitation {
            id: id.to_string(),
            path: path.to_string(),
            is_definition: true,
        };
        let cited = |path: &str, id: &str| DecisionCitation {
            id: id.to_string(),
            path: path.to_string(),
            is_definition: false,
        };

        lint_decision_registry(&[sealed("a.md", "D-P8R-01"), cited("b.md", "D-P8R-01")]).unwrap();

        let duplicate = [
            sealed("a.md", "D-P8R-01"),
            sealed("b.md", "D-P8R-01"),
            cited("c.md", "D-P8R-01"),
        ];
        let error = lint_decision_registry(&duplicate).unwrap_err();
        assert!(error.contains("sealed 2 times"), "{error}");

        let dangling = [cited("c.md", "D-P8R-42")];
        let error = lint_decision_registry(&dangling).unwrap_err();
        assert!(error.contains("never sealed"), "{error}");
    }

    #[test]
    fn repository_rtdata_base_matches_the_machine_constant() {
        lint_rtdata_base(&root()).unwrap();
    }

    #[test]
    fn superseded_rtdata_base_is_rejected_in_live_source() {
        let losing_hex = superseded_rtdata_base_values()[1].clone();
        let error = lint_live_rtdata_base(
            "crates/example/src/lib.rs",
            &format!("const RTDATA_BASE: u64 = {losing_hex};"),
        )
        .unwrap_err();
        assert!(error.contains("live source"), "{error}");
        assert!(error.contains("crates/example/src/lib.rs"), "{error}");
    }

    #[test]
    fn superseded_rtdata_base_value_needs_its_decision_citation() {
        let losing_formula = superseded_rtdata_base_values()[0].clone();
        let losing_hex = superseded_rtdata_base_values()[1].clone();
        lint_superseded_rtdata_base(
            "fake.md",
            &format!("D-P8R-09 records that this chapter previously said {losing_formula}.\n"),
        )
        .unwrap();
        let error = lint_superseded_rtdata_base(
            "fake.md",
            &format!("The packing base is at {losing_formula}.\n\nSomething else.\n"),
        )
        .unwrap_err();
        assert!(error.contains("superseded RTDATA_BASE"), "{error}");
        let error =
            lint_superseded_rtdata_base("fake.md", &format!("It sits at {losing_hex} today."))
                .unwrap_err();
        assert!(error.contains(&losing_hex), "{error}");
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

    #[test]
    fn fixture_placeholder_check_allows_activation_but_not_stale_sentinels() {
        let pending = parse_fixture_activation(
            "production Pixels stage is not implemented; implemented in task P1.3.",
        )
        .unwrap();
        let active = parse_fixture_activation("activated; implemented in task P1.3.").unwrap();
        let placeholder =
            "error: production Pixels stage is not implemented; implemented in task P1.3";

        lint_fixture_placeholder("fixture", "input", pending, placeholder).unwrap();
        assert!(
            lint_fixture_placeholder("fixture", "input", pending, "real source")
                .unwrap_err()
                .contains("must contain")
        );
        lint_fixture_placeholder("fixture", "input", active, "real source").unwrap();
        assert!(
            lint_fixture_placeholder("fixture", "input", active, placeholder)
                .unwrap_err()
                .contains("retains")
        );
    }
}
