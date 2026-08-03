use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

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
    lint_formal_readme(repo)?;
    lint_permanent_fixtures(text, repo, &headings)?;
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
        "check-pixels-area-light" => &["LightConfig.fixed(capacity=1)", "EMITTER_HALF_WIDTH_RAW"],
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
        "err-pixels-repeat-unbounded" => &["count=params.count", "count: u32"],
        "err-pixels-capacity" => &["count=257", "REQUIRED_FEATURES"],
        "err-pixels-projective-zero" => &["near=0.0", "DENOMINATOR_RAW"],
        "err-pixels-fixed-q" => &["scale=2147483648.0", "REQUIRED_RAW_BITS"],
        "err-pixels-tone-table" => &["INVALID_TONE_TABLE", "[TABLE_LEFT_CODE, TABLE_RIGHT_CODE]"],
        "err-pixels-cost" => &["width=1920", "height=1080", "initialization_deadline_ms=1"],
        "boot-pixels-numeric" => &["smooth_union(", "VECTOR_COUNT"],
        "boot-pixels-plane" => &["plane(", "WIDTH", "HEIGHT"],
        "boot-pixels-quality" => &[
            "round_box(",
            "sinusoidal_displace(",
            "AoConfig.fixed(enabled=true)",
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
        let expected_path = path.join("expected/check.txt");
        let fixture_source = fixture_wrela_sources(&path)?;
        let expected = std::fs::read_to_string(&expected_path)
            .map_err(|error| format!("read {}: {error}", expected_path.display()))?;
        for required in [
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
        .into_iter()
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
