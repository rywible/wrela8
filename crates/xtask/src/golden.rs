use std::path::{Path, PathBuf};
use std::process::Command;

use crate::{CompileOptsGuard, golden_case_dirs, root, run};
use wrela_compiler::opts::CompileMode;

fn read_dir_paths(dir: &Path) -> Result<Vec<PathBuf>, String> {
    let entries = std::fs::read_dir(dir).map_err(|e| format!("read {}: {e}", dir.display()))?;
    let mut paths = Vec::new();
    for entry in entries {
        paths.push(
            entry
                .map_err(|e| format!("read {} entry: {e}", dir.display()))?
                .path(),
        );
    }
    Ok(paths)
}

pub(crate) fn hex_dump(bytes: &[u8]) -> String {
    let mut out = String::new();
    for (i, chunk) in bytes.chunks(16).enumerate() {
        let offset = i * 16;
        let hex: Vec<String> = chunk.iter().map(|b| format!("{b:02x}")).collect();
        out.push_str(&format!("{offset:08x}: {}\n", hex.join(" ")));
    }
    out
}

pub(crate) fn golden_case_is_borrowed(case: &Path) -> Result<bool, String> {
    let Some(target) = golden_case_target(case)? else {
        return Ok(false);
    };
    let norm = |p: &Path| -> PathBuf {
        let mut out = PathBuf::new();
        for c in p.components() {
            match c {
                std::path::Component::CurDir => {}
                std::path::Component::ParentDir => {
                    if !out.pop() {
                        out.push("..");
                    }
                }
                other => out.push(other),
            }
        }
        out
    };
    Ok(!norm(&target).starts_with(norm(case)))
}

pub(crate) fn golden_case_target(case: &Path) -> Result<Option<PathBuf>, String> {
    let root_marker = case.join("root");
    if root_marker.is_file() {
        let rel = std::fs::read_to_string(&root_marker)
            .map_err(|e| format!("read {}: {e}", root_marker.display()))?;
        let rel = rel.trim();
        if rel.is_empty() {
            return Err(format!("{}: `root` file is empty", root_marker.display()));
        }
        return Ok(Some(case.join(rel)));
    }
    let input = case.join("input.wr");
    if input.exists() {
        return Ok(Some(input));
    }
    Ok(None)
}

pub(crate) fn build_and_sign_vmm() -> Result<PathBuf, String> {
    static VMM: std::sync::OnceLock<Result<PathBuf, String>> = std::sync::OnceLock::new();
    VMM.get_or_init(build_and_sign_vmm_uncached).clone()
}

fn build_and_sign_vmm_uncached() -> Result<PathBuf, String> {
    if !(cfg!(target_os = "macos") && cfg!(target_arch = "aarch64")) {
        return Err(
            "wrela-vmm boot lanes require macOS/aarch64 and Hypervisor.framework; refusing to run a stub VMM"
                .to_string(),
        );
    }
    run(
        Command::new("cargo").args(["build", "--quiet", "-p", "wrela-vmm", "--bin", "wrela-vmm"]),
        "cargo build wrela-vmm",
    )?;
    let bin = root().join("target/debug/wrela-vmm");
    if cfg!(target_os = "macos") {
        let mut cmd = Command::new("codesign");
        cmd.args(["--force", "--sign", "-", "--entitlements"]);
        cmd.arg(root().join("crates/wrela-vmm/entitlements.plist"));
        cmd.arg(&bin);
        run(&mut cmd, "codesign wrela-vmm")?;
    }
    Ok(bin)
}

fn build_signed_vmm_test_executables() -> Result<Vec<PathBuf>, String> {
    let output = Command::new("cargo")
        .current_dir(root())
        .args(["test", "-p", "wrela-vmm", "--lib", "--no-run"])
        .output()
        .map_err(|e| format!("cargo test -p wrela-vmm --lib --no-run: {e}"))?;
    if !output.status.success() {
        return Err(format!(
            "cargo test -p wrela-vmm --lib --no-run failed:\n{}",
            String::from_utf8_lossy(&output.stderr)
        ));
    }
    let stderr = String::from_utf8_lossy(&output.stderr);
    let mut executables: Vec<PathBuf> = Vec::new();
    for line in stderr.lines() {
        let line = line.trim();
        let Some(rest) = line.strip_prefix("Executable ") else {
            continue;
        };
        let (Some(open), Some(close)) = (rest.rfind('('), rest.rfind(')')) else {
            continue;
        };
        if close > open {
            executables.push(root().join(&rest[open + 1..close]));
        }
    }
    if executables.is_empty() {
        return Err(
            "cargo test -p wrela-vmm --lib --no-run: found no test executable to sign".to_string(),
        );
    }
    for exe in &executables {
        if cfg!(target_os = "macos") {
            let mut cmd = Command::new("codesign");
            cmd.args(["--force", "--sign", "-", "--entitlements"]);
            cmd.arg(root().join("crates/wrela-vmm/entitlements.plist"));
            cmd.arg(exe);
            run(&mut cmd, "codesign wrela-vmm test binary")?;
        }
    }
    Ok(executables)
}

fn ignored_tests(exe: &Path) -> Result<Vec<String>, String> {
    let listed = Command::new(exe)
        .args(["--ignored", "--list"])
        .output()
        .map_err(|e| format!("list ignored tests in {}: {e}", exe.display()))?;
    if !listed.status.success() {
        return Err(format!(
            "list ignored tests in {} failed:\n{}",
            exe.display(),
            String::from_utf8_lossy(&listed.stderr)
        ));
    }
    Ok(String::from_utf8_lossy(&listed.stdout)
        .lines()
        .filter_map(|line| line.strip_suffix(": test").map(str::to_string))
        .collect())
}

pub(crate) fn test_wrela_vmm_hvf_signed_smoke() -> Result<(), String> {
    const TEST: &str =
        "tests::park_and_resume_fifo_second_message_waits_for_the_suspended_turn_over_hvf";
    let executables = build_signed_vmm_test_executables()?;
    let mut found = 0usize;
    for exe in &executables {
        if !ignored_tests(exe)?.iter().any(|name| name == TEST) {
            continue;
        }
        found += 1;
        run(
            Command::new(exe).args([TEST, "--ignored", "--exact", "--test-threads=1"]),
            &format!("run focused HVF test `{TEST}` in {}", exe.display()),
        )?;
    }
    if found != 1 {
        return Err(format!(
            "focused HVF test census changed: found `{TEST}` in {found} test executable(s), expected 1"
        ));
    }
    Ok(())
}

pub(crate) fn test_wrela_vmm_hvf_signed() -> Result<(), String> {
    let executables = build_signed_vmm_test_executables()?;
    let mut total = 0usize;
    for exe in &executables {
        let tests = ignored_tests(exe)?;
        let count = tests.len();
        if count == 0 {
            continue;
        }
        total += count;
        run(
            Command::new(exe).args(["--ignored", "--test-threads=1"]),
            &format!("run {count} ignored HVF tests in {}", exe.display()),
        )?;
    }
    if total != VMM_HVF_TESTS {
        return Err(format!(
            "wrela-vmm HVF test census changed: found {total}, expected {VMM_HVF_TESTS}"
        ));
    }
    Ok(())
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum BootSel {
    All,
    Only,
    None,
}

pub(crate) struct GoldenOpts {
    pub(crate) update: bool,
    pub(crate) filter: Option<String>,
    pub(crate) boot: BootSel,
    pub(crate) jobs: usize,
    pub(crate) boot_jobs: usize,
}

pub(crate) fn default_jobs() -> usize {
    std::thread::available_parallelism().map_or(1, |n| n.get())
}

// Image compilation uses the ordinary worker count. Only the VMM subprocess is
// throttled: four concurrent HVF guests intermittently starve multicore
// quiescence long enough to produce a false `quiesce=timeout` transcript.
pub(crate) const DEFAULT_BOOT_JOBS: usize = 2;
pub(crate) const VMM_HVF_TESTS: usize = 24;

impl Default for GoldenOpts {
    fn default() -> Self {
        Self {
            update: false,
            filter: None,
            boot: BootSel::All,
            jobs: default_jobs(),
            boot_jobs: DEFAULT_BOOT_JOBS,
        }
    }
}

fn stage_boots(stage: &str) -> bool {
    stage == "test" || stage == "test-omit-dmb"
}

fn renderer_dump_stage(stage: &str) -> Result<(&str, Option<usize>), String> {
    let Some((base, raw_renderer)) = stage.rsplit_once("-renderer-") else {
        return Ok((stage, None));
    };
    if !matches!(base, "field-graph" | "frame-program" | "render-layout") {
        return Ok((stage, None));
    }
    let renderer = raw_renderer
        .parse::<usize>()
        .map_err(|error| format!("bad renderer-qualified golden stage `{stage}`: {error}"))?;
    Ok((base, Some(renderer)))
}

fn stage_selected(stage: &str, boot: BootSel) -> bool {
    match boot {
        BootSel::All => true,
        BootSel::Only => stage_boots(stage),
        BootSel::None => !stage_boots(stage),
    }
}

fn case_has_selected_stage(case: &Path, boot: BootSel) -> Result<(bool, bool), String> {
    let expected_dir = case.join("expected");
    let entries = std::fs::read_dir(&expected_dir)
        .map_err(|e| format!("read {}: {e}", expected_dir.display()))?;
    let mut selected = false;
    let mut selected_boot = false;
    for entry in entries {
        let path = entry
            .map_err(|e| format!("read {} entry: {e}", expected_dir.display()))?
            .path();
        let stage = path
            .file_stem()
            .and_then(|s| s.to_str())
            .ok_or_else(|| format!("bad expected file name: {}", path.display()))?;
        if stage_selected(stage, boot) {
            selected = true;
            selected_boot |= stage_boots(stage);
        }
    }
    Ok((selected, selected_boot))
}

fn first_text_difference(expected: &str, actual: &str) -> String {
    let mut expected_lines = expected.lines();
    let mut actual_lines = actual.lines();
    let mut line = 1usize;
    loop {
        match (expected_lines.next(), actual_lines.next()) {
            (Some(left), Some(right)) if left == right => line += 1,
            (Some(left), Some(right)) => {
                return format!("line {line}\nexpected: {left}\nactual:   {right}");
            }
            (Some(left), None) => {
                return format!("line {line}\nexpected: {left}\nactual:   <end of output>");
            }
            (None, Some(right)) => {
                return format!("line {line}\nexpected: <end of output>\nactual:   {right}");
            }
            (None, None) => {
                return format!(
                    "byte mismatch after the final complete line (expected={} bytes, actual={} bytes)",
                    expected.len(),
                    actual.len()
                );
            }
        }
    }
}

fn run_case(
    case: &Path,
    wrela: &Path,
    vmm: Option<&Path>,
    vmm_slots: Option<&Path>,
    update: bool,
    boot: BootSel,
) -> Result<(usize, Vec<String>), String> {
    let mut cases = 0usize;
    let mut failures = Vec::new();
    {
        let expected_dir = case.join("expected");
        let input = match golden_case_target(case)? {
            Some(target) if target.exists() && expected_dir.is_dir() => target,
            _ => {
                failures.push(format!(
                    "{}: missing input.wr (or `root`'s target) or expected/",
                    case.display()
                ));
                return Ok((cases, failures));
            }
        };
        let entries = std::fs::read_dir(&expected_dir)
            .map_err(|e| format!("read {}: {e}", expected_dir.display()))?;
        let mut expected_files = Vec::new();
        for entry in entries {
            expected_files.push(
                entry
                    .map_err(|e| format!("read {} entry: {e}", expected_dir.display()))?
                    .path(),
            );
        }
        expected_files.sort();
        let accepted_pixels = case
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name.starts_with("check-pixels-"))
            && expected_dir.join("image.txt").is_file();
        let bundle_needed = accepted_pixels
            && expected_files.iter().any(|path| {
                let stage = path
                    .file_stem()
                    .and_then(|stage| stage.to_str())
                    .unwrap_or("");
                let (base, _) = renderer_dump_stage(stage).unwrap_or((stage, None));
                matches!(
                    base,
                    "field-graph" | "frame-program" | "render-layout" | "report" | "image"
                ) && stage_selected(stage, boot)
            });
        let pixels_bundle = if bundle_needed {
            Some(crate::produce_image_artifacts(&input)?)
        } else {
            None
        };
        for exp in expected_files {
            let stage = exp
                .file_stem()
                .and_then(|s| s.to_str())
                .ok_or_else(|| format!("bad expected file name: {}", exp.display()))?
                .to_string();
            let (command_stage, renderer) = renderer_dump_stage(&stage)?;
            if !stage_selected(&stage, boot) {
                continue;
            }
            let rel_input = input.strip_prefix(root()).unwrap_or(&input);
            let case_name = case
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("case")
                .to_string();
            let build_out_dir_rel = format!("target/golden-build-tmp/{case_name}");
            let build_out_dir_abs = root().join(&build_out_dir_rel);
            if stage == "build" || stage == "build-err" {
                if build_out_dir_abs.exists() {
                    std::fs::remove_dir_all(&build_out_dir_abs)
                        .map_err(|e| format!("remove {}: {e}", build_out_dir_abs.display()))?;
                }
                std::fs::create_dir_all(&build_out_dir_abs)
                    .map_err(|e| format!("create {}: {e}", build_out_dir_abs.display()))?;
            }
            if stage == "img" {
                let written: Vec<_> = read_dir_paths(&build_out_dir_abs)?
                    .into_iter()
                    .filter(|p| p.extension().and_then(|e| e.to_str()) == Some("img"))
                    .collect();
                let img_bytes = match written.as_slice() {
                    [one] => {
                        std::fs::read(one).map_err(|e| format!("read {}: {e}", one.display()))?
                    }
                    other => {
                        failures.push(format!(
                            "{} [img]: expected exactly one `*.img` written to {}, found {} \
                             (does this case also carry a `build.txt` expectation, sorted \
                             before `img.hex`?)",
                            case.display(),
                            build_out_dir_abs.display(),
                            other.len()
                        ));
                        continue;
                    }
                };
                let actual = hex_dump(&img_bytes);
                cases += 1;
                if update {
                    std::fs::write(&exp, &actual)
                        .map_err(|e| format!("write {}: {e}", exp.display()))?;
                    continue;
                }
                let expected = std::fs::read_to_string(&exp)
                    .map_err(|e| format!("read {}: {e}", exp.display()))?;
                if actual != expected {
                    failures.push(format!(
                        "{} [img]: image bytes differ from expectation\n--- expected\n{expected}--- actual\n{actual}",
                        case.display()
                    ));
                }
                continue;
            }
            let bundled_actual = pixels_bundle
                .as_ref()
                .and_then(|bundle| match command_stage {
                    "report" => Some(bundle.report.clone()),
                    "image" => bundle.image_dump.clone(),
                    "field-graph" | "frame-program" | "render-layout" => {
                        let selected = match renderer {
                            Some(index) => bundle.renderers.get(index),
                            None if bundle.renderers.len() == 1 => bundle.renderers.first(),
                            None => None,
                        }?;
                        Some(
                            match command_stage {
                                "field-graph" => &selected.field_graph,
                                "frame-program" => &selected.frame_program,
                                "render-layout" => &selected.render_layout,
                                _ => unreachable!(),
                            }
                            .clone(),
                        )
                    }
                    _ => None,
                });
            if let Some(actual) = bundled_actual {
                cases += 1;
                if actual
                    .lines()
                    .next()
                    .is_some_and(|line| line.starts_with("error["))
                {
                    failures.push(format!(
                        "{} [{stage}]: accepted Pixels bundle normalized a diagnostic:\n{actual}",
                        case.display()
                    ));
                    continue;
                }
                if update {
                    std::fs::write(&exp, &actual)
                        .map_err(|e| format!("write {}: {e}", exp.display()))?;
                    continue;
                }
                let expected = std::fs::read_to_string(&exp)
                    .map_err(|e| format!("read {}: {e}", exp.display()))?;
                if actual != expected {
                    failures.push(format!(
                        "{} [{stage}]: bundled output differs from expectation at {}",
                        case.display(),
                        first_text_difference(&expected, &actual),
                    ));
                }
                continue;
            }
            let out = if stage == "test" || stage == "test-omit-dmb" {
                let vmm = vmm.ok_or_else(|| {
                    format!(
                        "{} [{stage}]: boot stage selected without a VMM",
                        case.display()
                    )
                })?;
                let mut cmd = Command::new(&wrela);
                cmd.current_dir(root()).arg("test").arg(rel_input);
                if let Some(slots) = vmm_slots {
                    cmd.env("WRELA_VMM_SLOT_DIR", slots);
                }
                if stage == "test-omit-dmb" {
                    cmd.arg("--omit-dmb");
                }
                cmd.arg("--vmm")
                    .arg(&vmm)
                    .output()
                    .map_err(|e| format!("run wrela: {e}"))?
            } else if stage == "build" || stage == "build-err" {
                Command::new(&wrela)
                    .current_dir(root())
                    .arg("build")
                    .arg(rel_input)
                    .arg("--out-dir")
                    .arg(&build_out_dir_rel)
                    .output()
                    .map_err(|e| format!("run wrela: {e}"))?
            } else {
                let mut command = Command::new(&wrela);
                command
                    .current_dir(root())
                    .arg("dump")
                    .arg(format!("--stage={command_stage}"));
                if let Some(renderer) = renderer {
                    command.arg(format!("--renderer={renderer}"));
                }
                command
                    .arg(rel_input)
                    .output()
                    .map_err(|e| format!("run wrela: {e}"))?
            };
            if stage == "build-err" {
                if out.status.success() {
                    failures.push(format!(
                        "{} [build-err]: wrela build unexpectedly exited successfully",
                        case.display()
                    ));
                    continue;
                }
            } else if stage != "test" && stage != "test-omit-dmb" && !out.status.success() {
                let mut fresh = String::from_utf8_lossy(&out.stdout).into_owned();
                fresh.push_str(&String::from_utf8_lossy(&out.stderr));
                let expected_is_diagnostic = std::fs::read_to_string(&exp)
                    .map(|t| t.lines().next().is_some_and(|l| l.starts_with("error[")))
                    .unwrap_or(false)
                    || (update
                        && fresh
                            .lines()
                            .next()
                            .is_some_and(|l| l.starts_with("error[")));
                if !expected_is_diagnostic {
                    failures.push(format!(
                        "{} [{stage}]: wrela exited with failure:\n{}",
                        case.display(),
                        String::from_utf8_lossy(&out.stderr)
                    ));
                    continue;
                }
            }
            let mut actual = String::from_utf8_lossy(&out.stdout).into_owned();
            actual.push_str(&String::from_utf8_lossy(&out.stderr));
            cases += 1;
            if accepted_pixels
                && matches!(command_stage, "frame-program" | "render-layout" | "report")
                && actual
                    .lines()
                    .next()
                    .is_some_and(|line| line.starts_with("error["))
            {
                failures.push(format!(
                    "{} [{stage}]: accepted Pixels output normalized a diagnostic:\n{actual}",
                    case.display()
                ));
                continue;
            }
            if update {
                std::fs::write(&exp, &actual)
                    .map_err(|e| format!("write {}: {e}", exp.display()))?;
                continue;
            }
            let expected = std::fs::read_to_string(&exp)
                .map_err(|e| format!("read {}: {e}", exp.display()))?;
            if actual != expected {
                failures.push(format!(
                    "{} [{stage}]: output differs from expectation\n--- expected\n{expected}--- actual\n{actual}",
                    case.display()
                ));
            }
            if stage == "build" {
                let report_expected = expected_dir.join("report.txt");
                if report_expected.is_file() {
                    let written: Vec<_> = read_dir_paths(&build_out_dir_abs)?
                        .into_iter()
                        .filter(|p| p.extension().and_then(|e| e.to_str()) == Some("txt"))
                        .filter(|p| {
                            p.file_name()
                                .and_then(|n| n.to_str())
                                .is_some_and(|n| n.ends_with(".report.txt"))
                        })
                        .collect();
                    match written.as_slice() {
                        [one] => {
                            let written_text = std::fs::read_to_string(one)
                                .map_err(|e| format!("read {}: {e}", one.display()))?;
                            let expected_text = std::fs::read_to_string(&report_expected)
                                .map_err(|e| format!("read {}: {e}", report_expected.display()))?;
                            if written_text != expected_text {
                                failures.push(format!(
                                    "{} [build]: the report file `wrela build` wrote differs from expected/report.txt\n--- expected\n{expected_text}--- actual\n{written_text}",
                                    case.display()
                                ));
                            }
                        }
                        other => failures.push(format!(
                            "{} [build]: expected exactly one `*.report.txt` written to {}, found {}",
                            case.display(),
                            build_out_dir_abs.display(),
                            other.len()
                        )),
                    }
                }
            }
        }
        let case_name = case
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("case")
            .to_string();
        let build_out_dir_abs = root().join(format!("target/golden-build-tmp/{case_name}"));
        if build_out_dir_abs.exists() {
            std::fs::remove_dir_all(&build_out_dir_abs)
                .map_err(|e| format!("remove {}: {e}", build_out_dir_abs.display()))?;
        }
    }
    Ok((cases, failures))
}

fn run_cases_parallel(
    cases: &[PathBuf],
    wrela: &Path,
    vmm: Option<&Path>,
    vmm_slots: Option<&Path>,
    update: bool,
    boot: BootSel,
    jobs: usize,
) -> Result<(usize, Vec<String>), String> {
    if cases.is_empty() {
        return Ok((0, Vec::new()));
    }
    let jobs = jobs.max(1).min(cases.len());
    let cursor = std::sync::atomic::AtomicUsize::new(0);
    let results: std::sync::Mutex<Vec<(usize, usize, Vec<String>)>> =
        std::sync::Mutex::new(Vec::new());
    let hard_error: std::sync::Mutex<Option<String>> = std::sync::Mutex::new(None);

    std::thread::scope(|scope| {
        for _ in 0..jobs {
            scope.spawn(|| {
                let _mode = CompileOptsGuard::mode(CompileMode::Release);
                loop {
                    let i = cursor.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    if i >= cases.len() {
                        return;
                    }
                    match run_case(&cases[i], wrela, vmm, vmm_slots, update, boot) {
                        Ok((n, f)) => results.lock().expect("results lock").push((i, n, f)),
                        Err(e) => {
                            let mut slot = hard_error.lock().expect("error lock");
                            if slot.is_none() {
                                *slot = Some(e);
                            }
                        }
                    }
                }
            });
        }
    });

    if let Some(e) = hard_error.into_inner().expect("error lock") {
        return Err(e);
    }
    let mut out = results.into_inner().expect("results lock");
    out.sort_by_key(|(i, _, _)| *i);
    let mut total = 0usize;
    let mut failures = Vec::new();
    for (_, n, f) in out {
        total += n;
        failures.extend(f);
    }
    Ok((total, failures))
}

pub(crate) fn golden(opts: &GoldenOpts) -> Result<(), String> {
    run(
        Command::new("cargo").args(["build", "--quiet", "-p", "wrela-compiler", "--bin", "wrela"]),
        "cargo build wrela",
    )?;
    let wrela = root().join("target/debug/wrela");
    let golden_dir = root().join("tests/golden");

    let mut dump_cases = Vec::new();
    let mut boot_cases = Vec::new();
    let mut selected_names = Vec::new();
    for case in golden_case_dirs(&golden_dir)? {
        let name = case
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or_default()
            .to_string();
        if let Some(f) = &opts.filter {
            if !name.contains(f.as_str()) {
                continue;
            }
        }
        let (selected, selected_boot) = case_has_selected_stage(&case, opts.boot)?;
        if !selected {
            continue;
        }
        selected_names.push(name);
        if selected_boot {
            boot_cases.push(case);
        } else {
            dump_cases.push(case);
        }
    }

    if dump_cases.is_empty() && boot_cases.is_empty() {
        return Err(match &opts.filter {
            Some(f) => format!(
                "golden: --filter `{f}` matched no selected expectation under tests/golden/"
            ),
            None => "golden: no expectations selected".to_string(),
        });
    }

    let vmm = if boot_cases.is_empty() {
        None
    } else {
        Some(build_and_sign_vmm()?)
    };
    let vmm_slot_dir = if boot_cases.is_empty() {
        None
    } else {
        let dir = root().join(format!("target/golden-vmm-slots-{}", std::process::id()));
        if dir.exists() {
            std::fs::remove_dir_all(&dir)
                .map_err(|error| format!("remove {}: {error}", dir.display()))?;
        }
        std::fs::create_dir_all(&dir)
            .map_err(|error| format!("create {}: {error}", dir.display()))?;
        for index in 0..opts.boot_jobs {
            std::fs::write(dir.join(format!("slot-{index}")), b"")
                .map_err(|error| format!("create VMM slot {index}: {error}"))?;
        }
        Some(dir)
    };
    let (n1, mut failures) = run_cases_parallel(
        &dump_cases,
        &wrela,
        vmm.as_deref(),
        None,
        opts.update,
        opts.boot,
        opts.jobs,
    )?;
    let (n2, boot_failures) = run_cases_parallel(
        &boot_cases,
        &wrela,
        vmm.as_deref(),
        vmm_slot_dir.as_deref(),
        opts.update,
        opts.boot,
        opts.jobs,
    )?;
    if let Some(dir) = &vmm_slot_dir {
        std::fs::remove_dir_all(dir)
            .map_err(|error| format!("remove {}: {error}", dir.display()))?;
    }
    failures.extend(boot_failures);
    let cases = n1 + n2;

    if !failures.is_empty() {
        for f in &failures {
            eprintln!("{f}\n");
        }
        return Err(format!("golden: {} failure(s)", failures.len()));
    }
    assert_no_internal_error_in_goldens(&golden_dir)?;
    if opts.update {
        println!("golden: updated {cases} expectation(s) — review the diff before committing");
        return Ok(());
    }
    let scope = match (&opts.filter, opts.boot) {
        (None, BootSel::All) => String::new(),
        _ => format!(
            " ({} case(s){}{})",
            selected_names.len(),
            match &opts.filter {
                Some(f) => format!(", filter `{f}`"),
                None => String::new(),
            },
            match opts.boot {
                BootSel::All => "",
                BootSel::Only => ", boot stages only",
                BootSel::None => ", boot stages skipped",
            }
        ),
    };
    println!("golden: {cases} expectation(s) ok{scope}");
    Ok(())
}

pub(crate) fn assert_no_internal_error_in_goldens(golden_dir: &Path) -> Result<(), String> {
    const PREFIX: &str = "internal error: ";
    let mut hits = Vec::new();
    for case in golden_case_dirs(golden_dir)? {
        let expected_dir = case.join("expected");
        if !expected_dir.is_dir() {
            continue;
        }
        let entries = std::fs::read_dir(&expected_dir)
            .map_err(|e| format!("read {}: {e}", expected_dir.display()))?;
        let mut files = Vec::new();
        for entry in entries {
            let path = entry
                .map_err(|e| format!("read {} entry: {e}", expected_dir.display()))?
                .path();
            if path.extension().and_then(|e| e.to_str()) == Some("txt") {
                files.push(path);
            }
        }
        files.sort();
        for path in files {
            let text = std::fs::read_to_string(&path)
                .map_err(|e| format!("read {}: {e}", path.display()))?;
            if text.contains(PREFIX) {
                hits.push(
                    path.strip_prefix(root())
                        .unwrap_or(&path)
                        .display()
                        .to_string(),
                );
            }
        }
    }
    if hits.is_empty() {
        Ok(())
    } else {
        Err(format!(
            "golden: {} expectation(s) contain `internal error:` (a compiler bug, never a \
             pinned outcome):\n  {}",
            hits.len(),
            hits.join("\n  ")
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn boot_selection_is_per_expectation_not_per_case() {
        assert!(stage_selected("asm", BootSel::None));
        assert!(stage_selected("check", BootSel::None));
        assert!(!stage_selected("test", BootSel::None));
        assert!(!stage_selected("asm", BootSel::Only));
        assert!(stage_selected("test", BootSel::Only));
        assert!(stage_selected("test-omit-dmb", BootSel::Only));
        assert!(stage_selected("asm", BootSel::All));
        assert!(stage_selected("test", BootSel::All));
    }
}
