use std::path::{Path, PathBuf};

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

fn produce_artifacts(target: &Path) -> Result<Vec<PixelsArtifacts>, String> {
    let produced = crate::produce_image_artifacts(target)?;
    let image = produced
        .image
        .ok_or_else(|| "pixels-repro: renderer emitted no image".to_string())?;
    if produced.renderers.is_empty() {
        return Err("pixels-repro: accepted renderer fixture emitted no renderers".to_string());
    }
    let mut artifacts = Vec::with_capacity(produced.renderers.len());
    for (renderer, dumps) in produced.renderers.into_iter().enumerate() {
        artifacts.push(PixelsArtifacts {
            field_graph: dumps.field_graph.into_bytes(),
            frame_program: dumps.frame_program.into_bytes(),
            frame_program_bytes: extract_frame_program_bytes(&produced.report, &image, renderer)?,
            render_layout: dumps.render_layout.into_bytes(),
            report: produced.report.as_bytes().to_vec(),
            image: image.clone(),
        });
    }
    Ok(artifacts)
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

fn case_uses_compiler_reserved_names(path: &Path) -> Result<bool, String> {
    let mut entries = std::fs::read_dir(path)
        .map_err(|error| format!("pixels-repro: read {}: {error}", path.display()))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| format!("pixels-repro: read {}: {error}", path.display()))?;
    entries.sort_by_key(|entry| entry.file_name());
    for entry in entries {
        let source = entry.path();
        if source.is_dir() {
            if case_uses_compiler_reserved_names(&source)? {
                return Ok(true);
            }
            continue;
        }
        if source.extension().and_then(|extension| extension.to_str()) != Some("wr") {
            continue;
        }
        let text = std::fs::read_to_string(&source)
            .map_err(|error| format!("pixels-repro: read {}: {error}", source.display()))?;
        let tokens = wrela_compiler::syntax::lexer::lex(&text).map_err(|error| {
            format!(
                "pixels-repro: lex {} at {}:{}: {}",
                source.display(),
                error.line,
                error.col,
                error.message
            )
        })?;
        if tokens.iter().any(|token| {
            token.kind == wrela_compiler::syntax::lexer::TokenKind::Ident
                && wrela_compiler::sema::is_compiler_reserved_source_name(&token.text)
        }) {
            return Ok(true);
        }
    }
    Ok(false)
}

fn accepted_renderer_cases(smoke: bool) -> Result<Vec<(String, PathBuf)>, String> {
    let golden = crate::root().join("tests/golden");
    let names: Vec<String> = if smoke {
        ["check-pixels-plane", "check-pixels-two-renderers"]
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
        if case_uses_compiler_reserved_names(&source)? {
            if smoke {
                return Err(format!(
                    "pixels-repro: smoke fixture {name} uses compiler-reserved instrumentation"
                ));
            }
            println!(
                "pixels-repro: skip {name}: compiler-reserved instrumentation is not relocatable"
            );
            continue;
        }
        cases.push((name, source));
    }
    if cases.is_empty() {
        return Err("pixels-repro: discovered no accepted renderer fixtures".to_string());
    }
    Ok(cases)
}

fn reproduce_cases(smoke: bool) -> Result<(), String> {
    // Reproduction compares the production artifacts emitted by the golden
    // lane. Compile copied fixtures under the same release optimization set;
    // debug-only renderer code growth is not part of the artifact contract
    // and can exceed the sealed pre-rtdata address range.
    let _mode = crate::CompileOptsGuard::mode(wrela_compiler::opts::CompileMode::Release);
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
        for (name, source) in accepted_renderer_cases(smoke)? {
            let first_target = fresh_case(&source, &scratch.join(format!("{name}-a")))?;
            let second_target = fresh_case(&source, &scratch.join(format!("{name}-b")))?;
            let first = produce_artifacts(&first_target)
                .map_err(|error| format!("pixels-repro: {name} copy a: {error}"))?;
            let second = produce_artifacts(&second_target)
                .map_err(|error| format!("pixels-repro: {name} copy b: {error}"))?;
            if first.len() != second.len() {
                return Err(format!(
                    "pixels-repro: {name} renderer count differs across fresh directories: {} vs {}",
                    first.len(),
                    second.len()
                ));
            }
            for (renderer, (first, second)) in first.iter().zip(&second).enumerate() {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn relocation_filter_distinguishes_user_surface_from_instrumented_fixtures() {
        let golden = crate::root().join("tests/golden");
        assert!(
            !case_uses_compiler_reserved_names(&golden.join("check-pixels-plane"))
                .expect("scan plane fixture")
        );
        assert!(
            !case_uses_compiler_reserved_names(&golden.join("check-pixels-two-renderers"))
                .expect("scan two-renderer fixture")
        );
        assert!(
            case_uses_compiler_reserved_names(&golden.join("check-pixels-smooth-csg"))
                .expect("scan instrumented fixture")
        );
    }
}
