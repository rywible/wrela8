use std::path::{Path, PathBuf};
use std::process::Command;

#[derive(Debug, PartialEq, Eq)]
struct PixelsArtifacts {
    field_graph: Vec<u8>,
    frame_program: Vec<u8>,
    frame_program_bytes: Vec<u8>,
    render_layout: Vec<u8>,
    report: Vec<u8>,
    image: Vec<u8>,
}

pub(crate) fn copy_tree(source: &Path, destination: &Path) -> Result<(), String> {
    std::fs::create_dir_all(destination)
        .map_err(|error| format!("pixels-repro: create {}: {error}", destination.display()))?;
    let mut entries = std::fs::read_dir(source)
        .map_err(|error| format!("pixels-repro: read {}: {error}", source.display()))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| format!("pixels-repro: read {}: {error}", source.display()))?;
    entries.sort_by_key(|entry| entry.file_name());
    for entry in entries {
        let from = entry.path();
        let to = destination.join(entry.file_name());
        if from.is_dir() {
            copy_tree(&from, &to)?;
        } else {
            std::fs::copy(&from, &to).map_err(|error| {
                format!(
                    "pixels-repro: copy {} to {}: {error}",
                    from.display(),
                    to.display()
                )
            })?;
        }
    }
    Ok(())
}

fn fresh_case(case: &Path, destination: &Path) -> Result<PathBuf, String> {
    copy_tree(case, destination)?;
    crate::golden_case_target(destination)?.ok_or_else(|| {
        format!(
            "pixels-repro: copied case {} has no root target",
            destination.display()
        )
    })
}

fn dump_output(
    target: &Path,
    stage: &str,
    renderer: Option<usize>,
) -> Result<std::process::Output, String> {
    let mut command = Command::new(crate::root().join("target/debug/wrela"));
    command
        .current_dir(crate::root())
        .arg("dump")
        .arg(format!("--stage={stage}"));
    if let Some(renderer) = renderer {
        command.arg(format!("--renderer={renderer}"));
    }
    command
        .arg(target)
        .output()
        .map_err(|error| format!("pixels-repro: run {stage} dump: {error}"))
}

fn run_dump(target: &Path, stage: &str, renderer: usize) -> Result<Vec<u8>, String> {
    let output = dump_output(target, stage, Some(renderer))?;
    if !output.status.success() {
        return Err(format!(
            "pixels-repro: {stage} dump failed:\n{}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr),
        ));
    }
    Ok(output.stdout)
}

fn extract_frame_program_bytes(
    report: &str,
    image: &[u8],
    renderer: usize,
) -> Result<Vec<u8>, String> {
    let prefix = format!("RendererPlacement index={renderer} ");
    let line = report
        .lines()
        .find(|line| line.trim_start().starts_with(&prefix))
        .ok_or_else(|| format!("pixels-repro: report has no renderer#{renderer} placement"))?;
    let field = |name: &str| -> Result<u64, String> {
        let raw = line
            .split_whitespace()
            .find_map(|part| part.strip_prefix(&format!("{name}=")))
            .ok_or_else(|| format!("pixels-repro: renderer placement has no {name}"))?;
        if let Some(hex) = raw.strip_prefix("0x") {
            u64::from_str_radix(hex, 16)
        } else {
            raw.parse()
        }
        .map_err(|error| format!("pixels-repro: invalid {name} `{raw}`: {error}"))
    };
    let base = field("frameprog_base")?;
    let bytes = field("frameprog_bytes")?;
    let start = base
        .checked_sub(wrela_machine::layout::IMAGE_BASE)
        .and_then(|value| usize::try_from(value).ok())
        .ok_or_else(|| "pixels-repro: frame program begins below image base".to_string())?;
    let len = usize::try_from(bytes)
        .map_err(|_| "pixels-repro: frame program length exceeds usize".to_string())?;
    let end = start
        .checked_add(len)
        .ok_or_else(|| "pixels-repro: frame program end overflows".to_string())?;
    image
        .get(start..end)
        .map(<[u8]>::to_vec)
        .ok_or_else(|| "pixels-repro: frame program lies outside image bytes".to_string())
}

fn produce_artifacts(target: &Path, renderer: usize) -> Result<PixelsArtifacts, String> {
    let (report, image) =
        crate::produce_report_and_image_with_discovery_order(target, false, false)?;
    let image = image.ok_or_else(|| "pixels-repro: renderer emitted no image".to_string())?;
    let frame_program_bytes = extract_frame_program_bytes(&report, &image, renderer)?;
    Ok(PixelsArtifacts {
        field_graph: run_dump(target, "field-graph", renderer)?,
        frame_program: run_dump(target, "frame-program", renderer)?,
        frame_program_bytes,
        render_layout: run_dump(target, "render-layout", renderer)?,
        report: report.into_bytes(),
        image,
    })
}

fn first_difference(left: &[u8], right: &[u8]) -> String {
    let offset = left
        .iter()
        .zip(right)
        .position(|(a, b)| a != b)
        .unwrap_or(left.len().min(right.len()));
    let line = left[..offset.min(left.len())]
        .iter()
        .filter(|byte| **byte == b'\n')
        .count()
        + 1;
    format!(
        "first difference at byte {offset}, line {line} (left={} bytes, right={} bytes)",
        left.len(),
        right.len()
    )
}

fn renderer_count(target: &Path) -> Result<usize, String> {
    let output = dump_output(target, "field-graph", None)?;
    if output.status.success() {
        return Ok(1);
    }
    let message = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let marker = "Pixels dump requires exactly one renderer, found ";
    let count = message
        .split_once(marker)
        .and_then(|(_, rest)| rest.split_once(';'))
        .and_then(|(raw, _)| raw.parse::<usize>().ok())
        .ok_or_else(|| {
            format!(
                "pixels-repro: cannot discover renderer count for {}:\n{message}",
                target.display()
            )
        })?;
    Ok(count)
}

fn accepted_renderer_cases(smoke: bool) -> Result<Vec<(String, PathBuf, usize)>, String> {
    let golden = crate::root().join("tests/golden");
    let names: Vec<String> = if smoke {
        ["check-pixels-plane", "check-pixels-smooth-csg"]
            .into_iter()
            .map(str::to_string)
            .collect()
    } else {
        crate::golden_case_dirs(&golden)?
            .into_iter()
            .filter_map(|path| {
                let name = path.file_name()?.to_str()?;
                (name.starts_with("check-pixels-") && path.join("expected/image.txt").is_file())
                    .then(|| name.to_string())
            })
            .collect()
    };
    let mut cases = Vec::new();
    for name in names {
        let source = golden.join(&name);
        let target = crate::golden_case_target(&source)?
            .ok_or_else(|| format!("pixels-repro: accepted case {name} has no root target"))?;
        let count = renderer_count(&target)?;
        for renderer in 0..count {
            cases.push((name.clone(), source.clone(), renderer));
        }
    }
    if cases.is_empty() {
        return Err("pixels-repro: discovered no accepted renderer fixtures".to_string());
    }
    Ok(cases)
}

fn reproduce_cases(smoke: bool) -> Result<(), String> {
    let scratch = crate::root().join("target/pixels-repro");
    if scratch.exists() {
        std::fs::remove_dir_all(&scratch)
            .map_err(|error| format!("pixels-repro: clear {}: {error}", scratch.display()))?;
    }
    std::fs::create_dir_all(&scratch)
        .map_err(|error| format!("pixels-repro: create {}: {error}", scratch.display()))?;
    let result = (|| {
        let mut selected_programs: std::collections::BTreeMap<String, Vec<Vec<u8>>> =
            std::collections::BTreeMap::new();
        for (name, source, renderer) in accepted_renderer_cases(smoke)? {
            let case_name = format!("{name}-renderer-{renderer}");
            let first_target = fresh_case(&source, &scratch.join(format!("{case_name}-a")))?;
            let second_target = fresh_case(&source, &scratch.join(format!("{case_name}-b")))?;
            let first = produce_artifacts(&first_target, renderer)?;
            let second = produce_artifacts(&second_target, renderer)?;
            for (artifact, left, right) in [
                ("field-graph", &first.field_graph, &second.field_graph),
                (
                    "frame-program dump",
                    &first.frame_program,
                    &second.frame_program,
                ),
                (
                    "encoded frame-program bytes",
                    &first.frame_program_bytes,
                    &second.frame_program_bytes,
                ),
                ("render-layout", &first.render_layout, &second.render_layout),
                ("report", &first.report, &second.report),
                ("image", &first.image, &second.image),
            ] {
                if left != right {
                    return Err(format!(
                        "pixels-repro: {name} renderer#{renderer} {artifact} differs across fresh directories: {}",
                        first_difference(left, right)
                    ));
                }
            }
            selected_programs
                .entry(name.clone())
                .or_default()
                .push(first.frame_program_bytes.clone());
            println!(
                "pixels-repro: {name} renderer#{renderer} reproduced field graph, frame-program dump/bytes, \
                 render layout, report, and image bytes"
            );
        }
        for (name, programs) in selected_programs {
            if programs.len() > 1 {
                for (index, earlier) in programs.iter().enumerate() {
                    if programs[index + 1..].iter().any(|later| later == earlier) {
                        return Err(format!(
                            "pixels-repro: {name} returned identical encoded programs for \
                             distinct renderer selectors"
                        ));
                    }
                }
            }
        }
        Ok(())
    })();
    let cleanup = std::fs::remove_dir_all(&scratch)
        .map_err(|error| format!("pixels-repro: remove {}: {error}", scratch.display()));
    result.and(cleanup)
}

pub(crate) fn pixels_repro() -> Result<(), String> {
    reproduce_cases(false)
}

pub(crate) fn pixels_repro_smoke() -> Result<(), String> {
    reproduce_cases(true)
}
