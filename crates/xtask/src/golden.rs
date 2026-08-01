use std::path::{Path, PathBuf};
use std::process::Command;

use crate::{golden_case_dirs, root, run};

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

pub(crate) fn test_wrela_vmm_signed() -> Result<(), String> {
    let output = Command::new("cargo")
        .current_dir(root())
        .args(["test", "-p", "wrela-vmm", "--no-run"])
        .output()
        .map_err(|e| format!("cargo test -p wrela-vmm --no-run: {e}"))?;
    if !output.status.success() {
        return Err(format!(
            "cargo test -p wrela-vmm --no-run failed:\n{}",
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
            "cargo test -p wrela-vmm --no-run: found no test executable(s) to sign".to_string(),
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
        run(
            Command::new(exe).arg("--test-threads=1"),
            &format!("run {}", exe.display()),
        )?;
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

pub(crate) const DEFAULT_BOOT_JOBS: usize = 4;

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

fn case_boots(expected_dir: &Path) -> bool {
    let Ok(entries) = std::fs::read_dir(expected_dir) else {
        return false;
    };
    entries.filter_map(Result::ok).any(|e| {
        e.path()
            .file_stem()
            .and_then(|s| s.to_str())
            .is_some_and(|s| s == "test" || s == "test-omit-dmb")
    })
}

fn run_case(
    case: &Path,
    wrela: &Path,
    vmm: &Path,
    update: bool,
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
        let mut expected_files: Vec<_> = std::fs::read_dir(&expected_dir)
            .map_err(|e| format!("read {}: {e}", expected_dir.display()))?
            .filter_map(Result::ok)
            .map(|e| e.path())
            .collect();
        expected_files.sort();
        for exp in expected_files {
            let stage = exp
                .file_stem()
                .and_then(|s| s.to_str())
                .ok_or_else(|| format!("bad expected file name: {}", exp.display()))?
                .to_string();
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
                let written: Vec<_> = std::fs::read_dir(&build_out_dir_abs)
                    .map_err(|e| format!("read {}: {e}", build_out_dir_abs.display()))?
                    .filter_map(Result::ok)
                    .map(|e| e.path())
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
            let out = if stage == "test" || stage == "test-omit-dmb" {
                let mut cmd = Command::new(&wrela);
                cmd.current_dir(root()).arg("test").arg(rel_input);
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
                Command::new(&wrela)
                    .current_dir(root())
                    .arg("dump")
                    .arg(format!("--stage={stage}"))
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
                    let written: Vec<_> = std::fs::read_dir(&build_out_dir_abs)
                        .map_err(|e| format!("read {}: {e}", build_out_dir_abs.display()))?
                        .filter_map(Result::ok)
                        .map(|e| e.path())
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
    vmm: &Path,
    update: bool,
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
                loop {
                    let i = cursor.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    if i >= cases.len() {
                        return;
                    }
                    match run_case(&cases[i], wrela, vmm, update) {
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
    let vmm = build_and_sign_vmm()?;
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
        let boots = case_boots(&case.join("expected"));
        match (boots, opts.boot) {
            (true, BootSel::None) => continue,
            (false, BootSel::Only) => continue,
            _ => {}
        }
        selected_names.push(name);
        if boots {
            boot_cases.push(case);
        } else {
            dump_cases.push(case);
        }
    }

    if dump_cases.is_empty() && boot_cases.is_empty() {
        return Err(match &opts.filter {
            Some(f) => format!("golden: --filter `{f}` matched no case under tests/golden/"),
            None => "golden: no cases selected".to_string(),
        });
    }

    let (n1, mut failures) = run_cases_parallel(&dump_cases, &wrela, &vmm, opts.update, opts.jobs)?;
    let (n2, boot_failures) = run_cases_parallel(
        &boot_cases,
        &wrela,
        &vmm,
        opts.update,
        opts.boot_jobs.min(opts.jobs),
    )?;
    failures.extend(boot_failures);
    let cases = n1 + n2;

    if opts.update {
        println!("golden: updated {cases} expectation(s) — review the diff before committing");
        return Ok(());
    }
    assert_no_internal_error_in_goldens(&golden_dir)?;
    if failures.is_empty() {
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
                    BootSel::Only => ", boots only",
                    BootSel::None => ", boots skipped",
                }
            ),
        };
        println!("golden: {cases} expectation(s) ok{scope}");
        Ok(())
    } else {
        for f in &failures {
            eprintln!("{f}\n");
        }
        Err(format!("golden: {} failure(s)", failures.len()))
    }
}

pub(crate) fn assert_no_internal_error_in_goldens(golden_dir: &Path) -> Result<(), String> {
    const PREFIX: &str = "internal error: ";
    let mut hits = Vec::new();
    for case in golden_case_dirs(golden_dir)? {
        let expected_dir = case.join("expected");
        if !expected_dir.is_dir() {
            continue;
        }
        let mut files: Vec<_> = std::fs::read_dir(&expected_dir)
            .map_err(|e| format!("read {}: {e}", expected_dir.display()))?
            .filter_map(Result::ok)
            .map(|e| e.path())
            .filter(|p| p.extension().and_then(|e| e.to_str()) == Some("txt"))
            .collect();
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
             pinned outcome — plans/M9.md item II):\n  {}",
            hits.len(),
            hits.join("\n  ")
        ))
    }
}
