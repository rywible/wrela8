//! Forge-facing cold build/run/restart process contract.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

const FORMAT: &str = "wrela-forge-run-v1";

pub(crate) fn forge(args: &[String]) -> Result<(), String> {
    match args {
        [command, source] if command == "run" => run(Path::new(source), "native"),
        [command, source, flag, display]
            if command == "run" && flag == "--display" && matches!(display.as_str(), "headless" | "native") =>
        {
            run(Path::new(source), display)
        }
        [command] if command == "restart" => restart(),
        _ => Err("usage: cargo xtask forge run <source-or-project-root> [--display headless|native]\n       cargo xtask forge restart".into()),
    }
}

fn run(source: &Path, display: &str) -> Result<(), String> {
    if !(cfg!(target_os = "macos") && cfg!(target_arch = "aarch64")) {
        return Err("forge: the native loop requires macOS/aarch64 Hypervisor.framework".into());
    }
    let source = source
        .canonicalize()
        .map_err(|error| format!("forge: resolve {}: {error}", source.display()))?;
    if source.to_string_lossy().contains(['\n', '\r', '\0']) {
        return Err("forge: source path contains a control byte".into());
    }
    let source_sha256 = digest_source(&source)?;
    let root = crate::root().join("target/wrela-forge");
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|error| format!("forge: system clock: {error}"))?
        .as_nanos();
    let run_dir = root.join(format!("run-{source_sha256}-{nonce:032x}"));
    let candidate = root.join(format!("candidate-{source_sha256}-{nonce:032x}"));
    std::fs::create_dir_all(&candidate)
        .map_err(|error| format!("forge: create {}: {error}", candidate.display()))?;
    let compiler =
        std::env::current_exe().map_err(|error| format!("forge: locate xtask: {error}"))?;
    let compile = Command::new(compiler)
        .current_dir(crate::root())
        .arg("__wrela")
        .arg("test")
        .arg(&source)
        .arg("--emit-image-dir")
        .arg(&candidate)
        .arg("--mode=release")
        .output()
        .map_err(|error| format!("forge: start compiler: {error}"))?;
    std::fs::write(candidate.join("compiler.stdout"), &compile.stdout)
        .map_err(|error| format!("forge: retain compiler stdout: {error}"))?;
    std::fs::write(candidate.join("compiler.stderr"), &compile.stderr)
        .map_err(|error| format!("forge: retain compiler stderr: {error}"))?;
    if !compile.status.success() {
        return Err(format!(
            "forge: compile failed; candidate diagnostics are in {}, and the previous successful run remains current",
            candidate.display()
        ));
    }
    let (report, image) = find_artifacts(&candidate)?;
    let vmm = build_and_sign_vmm(display == "native")?;
    std::fs::rename(&candidate, &run_dir)
        .map_err(|error| format!("forge: promote candidate: {error}"))?;
    let report = run_dir.join(report.file_name().unwrap());
    let image = run_dir.join(image.file_name().unwrap());
    let stdout = std::fs::File::create(run_dir.join("vmm.stdout"))
        .map_err(|error| format!("forge: create VMM stdout: {error}"))?;
    let stderr = std::fs::File::create(run_dir.join("vmm.stderr"))
        .map_err(|error| format!("forge: create VMM stderr: {error}"))?;
    let input_events = run_dir.join("input.events");
    std::fs::File::create(&input_events)
        .map_err(|error| format!("forge: create input transport: {error}"))?;
    let status = Command::new(&vmm)
        .args([report.as_os_str(), image.as_os_str()])
        .args(["--display", display])
        .arg("--input-events")
        .arg(&input_events)
        .arg("--record")
        .arg(run_dir.join("record.txt"))
        .arg("--metrics")
        .arg(run_dir.join("metrics.txt"))
        .env("WRELA_HOST_PROFILE", "diagnostic")
        .stdin(Stdio::null())
        .stdout(stdout)
        .stderr(stderr)
        .status()
        .map_err(|error| format!("forge: start VMM child: {error}"))?;
    if !status.success() {
        return Err(format!(
            "forge: VMM child failed; diagnostics are retained in {}, and the previous successful run remains current",
            run_dir.display()
        ));
    }
    if digest_source(&source)? != source_sha256 {
        return Err(format!(
            "forge: source changed while compiling; run artifacts remain in {}, and the previous successful run remains current",
            run_dir.display()
        ));
    }
    let manifest = encode_manifest(
        &source,
        &source_sha256,
        display,
        &report,
        &image,
        &vmm,
        &run_dir,
    )?;
    let current_candidate = root.join(format!("current-{nonce:032x}.tmp"));
    std::fs::write(&current_candidate, manifest)
        .map_err(|error| format!("forge: stage current run: {error}"))?;
    std::fs::rename(&current_candidate, root.join("current.txt"))
        .map_err(|error| format!("forge: publish current run atomically: {error}"))?;
    println!(
        "forge: real compiler + fresh HVF VMM run retained at {}",
        run_dir.display()
    );
    Ok(())
}

fn restart() -> Result<(), String> {
    let current = crate::root().join("target/wrela-forge/current.txt");
    let text = std::fs::read_to_string(&current)
        .map_err(|error| format!("forge: no successful current run to restart: {error}"))?;
    let fields = parse_manifest(&text)?;
    run(Path::new(&fields["source"]), &fields["display"])
}

fn build_and_sign_vmm(native: bool) -> Result<PathBuf, String> {
    let mut build = Command::new("cargo");
    build.args([
        "build",
        "--release",
        "-p",
        "wrela-vmm",
        "--bin",
        "wrela-vmm",
    ]);
    if native {
        build.args(["--features", "native-presentation"]);
    }
    crate::run(&mut build, "forge VMM build")?;
    let vmm = crate::root().join("target/release/wrela-vmm");
    crate::run(
        Command::new("/usr/bin/codesign")
            .args(["--force", "--sign", "-", "--entitlements"])
            .arg(crate::root().join("crates/wrela-vmm/entitlements.plist"))
            .arg(&vmm),
        "forge diagnostic VMM signing",
    )?;
    Ok(vmm)
}

fn find_artifacts(dir: &Path) -> Result<(PathBuf, PathBuf), String> {
    let mut reports = Vec::new();
    let mut images = Vec::new();
    for entry in std::fs::read_dir(dir).map_err(|error| format!("forge: read outputs: {error}"))? {
        let path = entry
            .map_err(|error| format!("forge: read output entry: {error}"))?
            .path();
        match path.extension().and_then(|value| value.to_str()) {
            Some("report") => reports.push(path),
            Some("img") => images.push(path),
            _ => {}
        }
    }
    if reports.len() != 1 || images.len() != 1 {
        return Err("forge: compiler must emit exactly one report and one image".into());
    }
    Ok((reports.remove(0), images.remove(0)))
}

fn encode_manifest(
    source: &Path,
    source_sha256: &str,
    display: &str,
    report: &Path,
    image: &Path,
    vmm: &Path,
    run_dir: &Path,
) -> Result<String, String> {
    Ok(format!(
        "format={FORMAT}\ndisplay={display}\nimage_sha256={}\nreport_sha256={}\nrun_dir={}\nsource={}\nsource_sha256={}\nvmm_sha256={}\n",
        digest_file(image)?,
        digest_file(report)?,
        run_dir.display(),
        source.display(),
        source_sha256,
        digest_file(vmm)?,
    ))
}

fn parse_manifest(text: &str) -> Result<std::collections::BTreeMap<String, String>, String> {
    if !text.ends_with('\n') || text.contains(['\r', '\0']) {
        return Err("forge: current manifest is not canonical".into());
    }
    let mut fields = std::collections::BTreeMap::new();
    let mut prior = "";
    for (index, line) in text.lines().enumerate() {
        let (key, value) = line
            .split_once('=')
            .ok_or("forge: malformed current manifest")?;
        if index == 0 {
            if key != "format" || value != FORMAT {
                return Err("forge: wrong current manifest version".into());
            }
            continue;
        }
        if key <= prior || fields.insert(key.to_string(), value.to_string()).is_some() {
            return Err("forge: current manifest fields are not sorted and unique".into());
        }
        prior = key;
    }
    let expected = [
        "display",
        "image_sha256",
        "report_sha256",
        "run_dir",
        "source",
        "source_sha256",
        "vmm_sha256",
    ];
    if fields.keys().map(String::as_str).ne(expected)
        || !matches!(fields["display"].as_str(), "headless" | "native")
    {
        return Err("forge: current manifest fields are invalid".into());
    }
    Ok(fields)
}

fn digest_file(path: &Path) -> Result<String, String> {
    std::fs::read(path)
        .map(|bytes| wrela_machine::sha256::sha256_hex(&bytes))
        .map_err(|error| format!("forge: read {}: {error}", path.display()))
}

fn digest_source(path: &Path) -> Result<String, String> {
    if path.is_file() {
        return digest_file(path);
    }
    if !path.is_dir() {
        return Err(format!(
            "forge: source {} is neither a file nor a project directory",
            path.display()
        ));
    }
    fn visit(root: &Path, directory: &Path, files: &mut Vec<PathBuf>) -> Result<(), String> {
        let mut entries = std::fs::read_dir(directory)
            .map_err(|error| format!("forge: read project {}: {error}", directory.display()))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| format!("forge: read project entry: {error}"))?;
        entries.sort_by_key(|entry| entry.file_name());
        for entry in entries {
            let path = entry.path();
            let relative = path
                .strip_prefix(root)
                .map_err(|_| "forge: project traversal escaped its root".to_string())?;
            if relative
                .components()
                .next()
                .is_some_and(|part| matches!(part.as_os_str().to_str(), Some(".git" | "target")))
            {
                continue;
            }
            let kind = entry
                .file_type()
                .map_err(|error| format!("forge: inspect {}: {error}", path.display()))?;
            if kind.is_symlink() {
                return Err(format!(
                    "forge: project source refuses symlink {}",
                    relative.display()
                ));
            }
            if kind.is_dir() {
                visit(root, &path, files)?;
            } else if kind.is_file() {
                files.push(path);
            } else {
                return Err(format!(
                    "forge: project source refuses special file {}",
                    relative.display()
                ));
            }
        }
        Ok(())
    }
    let mut files = Vec::new();
    visit(path, path, &mut files)?;
    if files.is_empty() {
        return Err("forge: project source contains no files".into());
    }
    let mut authenticated = b"wrela-forge-project-source-v1\0".to_vec();
    for file in files {
        let relative = file
            .strip_prefix(path)
            .map_err(|_| "forge: project file escaped its root".to_string())?;
        let relative = relative
            .to_str()
            .ok_or_else(|| format!("forge: non-UTF-8 project path {}", relative.display()))?;
        let bytes = std::fs::read(&file)
            .map_err(|error| format!("forge: read {}: {error}", file.display()))?;
        authenticated.extend_from_slice(&(relative.len() as u64).to_le_bytes());
        authenticated.extend_from_slice(relative.as_bytes());
        authenticated.extend_from_slice(&(bytes.len() as u64).to_le_bytes());
        authenticated.extend_from_slice(&bytes);
    }
    Ok(wrela_machine::sha256::sha256_hex(&authenticated))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn current_manifest_rejects_stale_unversioned_and_extra_state() {
        let d = "a".repeat(64);
        let valid = format!(
            "format={FORMAT}\ndisplay=headless\nimage_sha256={d}\nreport_sha256={d}\nrun_dir=/tmp/run\nsource=/tmp/input.wr\nsource_sha256={d}\nvmm_sha256={d}\n"
        );
        assert!(parse_manifest(&valid).is_ok());
        assert!(parse_manifest(&valid.replace(FORMAT, "wrela-forge-run-v0")).is_err());
        assert!(parse_manifest(&(valid + "unknown=1\n")).is_err());
    }

    #[test]
    fn manifest_attests_the_digest_of_the_compiled_snapshot() {
        let root = std::env::temp_dir().join(format!(
            "wrela-forge-manifest-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir(&root).unwrap();
        let source = root.join("input.wr");
        let report = root.join("image.report");
        let image = root.join("image.img");
        let vmm = root.join("wrela-vmm");
        for path in [&source, &report, &image, &vmm] {
            std::fs::write(path, path.as_os_str().as_encoded_bytes()).unwrap();
        }
        let compiled = digest_source(&source).unwrap();
        std::fs::write(&source, b"changed after compile").unwrap();
        let manifest =
            encode_manifest(&source, &compiled, "headless", &report, &image, &vmm, &root).unwrap();
        assert_eq!(
            parse_manifest(&manifest).unwrap()["source_sha256"],
            compiled
        );
        assert_ne!(digest_source(&source).unwrap(), compiled);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn project_source_digest_is_sorted_sensitive_and_excludes_build_state() {
        let root = std::env::temp_dir().join(format!(
            "wrela-forge-digest-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::create_dir_all(root.join("target")).unwrap();
        std::fs::write(root.join("Wrela.toml"), b"project").unwrap();
        std::fs::write(root.join("src/main.wr"), b"main").unwrap();
        std::fs::write(root.join("target/transient"), b"one").unwrap();
        let first = digest_source(&root).unwrap();
        std::fs::write(root.join("target/transient"), b"two").unwrap();
        assert_eq!(digest_source(&root).unwrap(), first);
        std::fs::write(root.join("src/main.wr"), b"changed").unwrap();
        assert_ne!(digest_source(&root).unwrap(), first);
        std::fs::remove_dir_all(root).unwrap();
    }
}
