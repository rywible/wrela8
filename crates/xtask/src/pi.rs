//! Deterministic Rasputin deployment and execution lane.

use std::collections::BTreeMap;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::evidence;

const LAB_ROOT: &str = "/var/tmp/wrela-lab";
const REMOTE_BIN: &str = "/var/tmp/wrela-lab/bin";
const FALLBACK_VMM_MANIFEST: &str = "[package]\nname = \"wrela-vmm\"\nversion.workspace = true\nedition.workspace = true\n\n[features]\nnative-presentation = []\n\n[dependencies]\nwrela-machine = { path = \"../wrela-machine\" }\n\n[target.'cfg(all(target_os = \"linux\", target_arch = \"aarch64\"))'.dependencies]\nkvm-bindings = { version = \"=0.14.1\", default-features = false, features = [\"fam-wrappers\"] }\nkvm-ioctls = { version = \"=0.25.0\", default-features = false }\n";
const SSH_POLICY_ARGS: &[&str] = &[
    "-oBatchMode=yes",
    "-oClearAllForwardings=yes",
    "-oExitOnForwardFailure=yes",
    "-oForwardAgent=no",
    "-oForwardX11=no",
    "-oPermitLocalCommand=no",
    "-oRequestTTY=no",
    "-oStrictHostKeyChecking=yes",
    "-oUpdateHostKeys=no",
];

pub(crate) fn pi(args: &[String]) -> Result<(), String> {
    match args {
        [command, host] if command == "probe" => probe(host),
        [command, host] if command == "prepare" => prepare(host).map(|_| ()),
        [command, host] if command == "remote-build-fallback" => remote_build_fallback(host),
        [command, host, run] if command == "cleanup" => cleanup(host, run),
        [command, host, case] if command == "run" => {
            run_case(host, case, false, "diagnostic", "none")
        }
        [command, host, case] if command == "guest-pmu" => {
            run_case(host, case, false, "product", "guest-pmu")
        }
        [command, host] if command == "conformance" => conformance(host),
        [command, host] if command == "stage1-pair" => stage1_pair(host),
        [command, host, workload] if command == "bench" => benchmark(host, workload),
        [command, host, flag, class]
            if command == "validate-proxy" && flag == "--class" => validate_proxy(host, class),
        _ => Err("usage: cargo xtask pi probe|prepare|remote-build-fallback <host>\n       cargo xtask pi cleanup <host> <run-id>\n       cargo xtask pi run|guest-pmu <host> <golden-case>\n       cargo xtask pi conformance <host>\n       cargo xtask pi stage1-pair <host>\n       cargo xtask pi bench <host> <workload>\n       cargo xtask pi validate-proxy <host> --class kernel|frame|sequence".into()),
    }
}

fn cleanup(host: &str, run: &str) -> Result<(), String> {
    validate_host(host)?;
    if run.len() != 20
        || !run.starts_with("run-")
        || !run[4..]
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
    {
        return Err("pi cleanup: run id must be `run-` plus 16 hex digits".into());
    }
    let (agent, _) = prepare_agent(host)?;
    let run_dir = format!("{LAB_ROOT}/runs/{run}");
    let request = manifest("cleanup", [("run_dir", run_dir.as_str())])?;
    let response = invoke_agent(host, &agent, &request)?;
    let record = evidence::parse(&response, "wrela-lab-cleanup-v1")?;
    record.require_exact_fields(&["run_dir", "verdict"])?;
    if record.fields["run_dir"] != run_dir || record.fields["verdict"] != "removed" {
        return Err("pi cleanup: remote response differs from the exact requested run".into());
    }
    println!("pi cleanup: removed retained remote run {run_dir}");
    Ok(())
}

#[derive(Debug)]
struct Prepared {
    agent_remote: String,
    agent_sha256: String,
    vmm_sha256: String,
}

struct RunDeployment {
    fields: BTreeMap<String, String>,
}

fn prepare_agent(host: &str) -> Result<(String, String), String> {
    validate_host(host)?;
    let (_, agent) = cached_cross_build(false)?;
    let digest = digest_file(&agent)?;
    let remote = format!("{REMOTE_BIN}/wrela-lab-agent-{digest}");
    if !remote_file_exists(host, &remote)? {
        sftp_batch(
            host,
            &[
                format!("-mkdir {}", quote_sftp(LAB_ROOT)?),
                format!("-mkdir {}", quote_sftp(REMOTE_BIN)?),
                format!("put {} {}", quote_sftp_path(&agent)?, quote_sftp(&remote)?),
                format!("chmod 0755 {}", quote_sftp(&remote)?),
            ],
        )?;
    }
    Ok((remote, digest))
}

fn remote_build_fallback(host: &str) -> Result<(), String> {
    validate_host(host)?;
    let (agent, agent_digest) = prepare_agent(host)?;
    let nonce = run_nonce()?;
    let local = crate::root()
        .join("target/wrela-lab")
        .join(host)
        .join(format!("remote-build-{nonce}"));
    std::fs::create_dir_all(&local)
        .map_err(|error| format!("create {}: {error}", local.display()))?;
    let staged = local.join("source");
    std::fs::create_dir(&staged)
        .map_err(|error| format!("create curated fallback source: {error}"))?;
    std::fs::write(
        staged.join("Cargo.toml"),
        "[workspace]\nresolver = \"2\"\nmembers = [\"crates/wrela-machine\", \"crates/wrela-vmm\"]\n\n[workspace.package]\nversion = \"0.1.0\"\nedition = \"2024\"\n\n[profile.release]\ndebug-assertions = true\noverflow-checks = true\n",
    )
    .map_err(|error| format!("write curated fallback workspace: {error}"))?;
    let workspace_lock = std::fs::read_to_string(crate::root().join("Cargo.lock"))
        .map_err(|error| format!("read fallback Cargo.lock source: {error}"))?;
    std::fs::write(
        staged.join("Cargo.lock"),
        curated_fallback_lock(&workspace_lock)?,
    )
    .map_err(|error| format!("write curated fallback Cargo.lock: {error}"))?;
    for crate_name in ["wrela-machine", "wrela-vmm"] {
        let source = crate::root().join("crates").join(crate_name);
        let destination = staged.join("crates").join(crate_name);
        copy_fallback_source(&source, &destination)?;
    }
    // The repository manifest has compiler-only dev dependencies which are
    // deliberately absent from this production-only diagnosis bundle. Keep
    // the fallback workspace closed over exactly the two copied crates.
    std::fs::write(
        staged.join("crates/wrela-vmm/Cargo.toml"),
        FALLBACK_VMM_MANIFEST,
    )
    .map_err(|error| format!("write curated fallback VMM manifest: {error}"))?;
    let archive = local.join("source.tar");
    let archive_path = archive
        .to_str()
        .ok_or("fallback archive path is not UTF-8")?;
    crate::run(
        Command::new("/usr/bin/tar").current_dir(&staged).args([
            "-cf",
            archive_path,
            "Cargo.toml",
            "Cargo.lock",
            "crates",
        ]),
        "archive explicit remote-build fallback source",
    )?;
    let archive_digest = digest_file(&archive)?;
    let remote_dir = format!("{LAB_ROOT}/runs/run-{nonce}");
    let remote_archive_name = format!("source-{archive_digest}.tar");
    let remote_archive = format!("{remote_dir}/{remote_archive_name}");
    sftp_batch(
        host,
        &[
            format!("-mkdir {}", quote_sftp(&format!("{LAB_ROOT}/runs"))?),
            format!("mkdir {}", quote_sftp(&remote_dir)?),
            format!(
                "put {} {}",
                quote_sftp_path(&archive)?,
                quote_sftp(&remote_archive)?
            ),
        ],
    )?;
    let manifest = manifest(
        "remote-build",
        [
            ("archive", remote_archive_name.as_str()),
            ("archive_sha256", archive_digest.as_str()),
            ("run_dir", remote_dir.as_str()),
        ],
    )?;
    let result = invoke_agent(host, &agent, &manifest)?;
    let record = evidence::parse(&result, "wrela-remote-build-v1")?;
    record.require_exact_fields(&[
        "agent_sha256",
        "archive_sha256",
        "build_features",
        "build_profile",
        "build_target",
        "cargo_identity",
        "cargo_lock_sha256",
        "rustc_identity",
        "vmm_binary_sha256",
        "vmm_remote_path",
    ])?;
    for key in ["archive_sha256", "cargo_lock_sha256", "vmm_binary_sha256"] {
        evidence::require_sha256(key, &record.fields[key])?;
    }
    if record.fields["agent_sha256"] != agent_digest
        || record.fields["archive_sha256"] != archive_digest
        || record.fields["build_features"] != "native-presentation"
        || record.fields["build_profile"] != "release"
        || record.fields["build_target"] != "aarch64-unknown-linux-gnu"
    {
        return Err("pi remote-build-fallback: provenance response differs from request".into());
    }
    std::fs::write(local.join("wrela-remote-build-v1.txt"), result)
        .map_err(|error| format!("retain fallback provenance: {error}"))?;
    println!(
        "pi remote-build-fallback: explicit non-release fallback retained at {}; primary release provenance remains the Mac cross-build",
        local.display()
    );
    Ok(())
}

fn curated_fallback_lock(workspace_lock: &str) -> Result<String, String> {
    const ALLOWED: &[&str] = &[
        "bitflags",
        "kvm-bindings",
        "kvm-ioctls",
        "libc",
        "vmm-sys-util",
        "wrela-machine",
        "wrela-vmm",
    ];
    let (header, packages) = workspace_lock
        .split_once("[[package]]\n")
        .ok_or("fallback Cargo.lock has no package rows")?;
    let mut out = header.to_string();
    let mut seen = std::collections::BTreeSet::new();
    for package in packages.split("\n[[package]]\n") {
        let name = package
            .lines()
            .find_map(|line| line.strip_prefix("name = \"")?.strip_suffix('"'))
            .ok_or("fallback Cargo.lock package has no name")?;
        if !ALLOWED.contains(&name) {
            continue;
        }
        seen.insert(name);
        let package = if name == "wrela-vmm" {
            package.replace(" \"wrela-compiler\",\n", "")
        } else {
            package.to_string()
        };
        out.push_str("[[package]]\n");
        out.push_str(&package);
        out.push('\n');
    }
    if ALLOWED.iter().any(|name| !seen.contains(name))
        || out.contains("wrela-compiler")
        || out.contains("name = \"toml\"")
    {
        return Err(
            "fallback Cargo.lock does not close over the approved VMM dependency set".into(),
        );
    }
    Ok(out)
}

fn copy_fallback_source(source: &Path, destination: &Path) -> Result<(), String> {
    std::fs::create_dir_all(destination)
        .map_err(|error| format!("create {}: {error}", destination.display()))?;
    let mut entries = std::fs::read_dir(source)
        .map_err(|error| format!("read fallback source {}: {error}", source.display()))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| format!("read fallback source entry: {error}"))?;
    entries.sort_by_key(std::fs::DirEntry::file_name);
    for entry in entries {
        let kind = entry
            .file_type()
            .map_err(|error| format!("inspect fallback source: {error}"))?;
        let from = entry.path();
        let to = destination.join(entry.file_name());
        if kind.is_symlink() {
            return Err(format!(
                "fallback source refuses symlink {}",
                from.display()
            ));
        }
        if kind.is_dir() {
            copy_fallback_source(&from, &to)?;
        } else if kind.is_file() {
            std::fs::copy(&from, &to)
                .map_err(|error| format!("copy fallback source {}: {error}", from.display()))?;
        } else {
            return Err(format!(
                "fallback source refuses special file {}",
                from.display()
            ));
        }
    }
    Ok(())
}

fn validate_host(host: &str) -> Result<(), String> {
    let mut parts = host.split('@');
    let first = parts.next().unwrap_or_default();
    let second = parts.next();
    if parts.next().is_some() {
        return Err(format!("pi: unsafe SSH host `{host}`"));
    }
    let (user, name) = match second {
        Some(name) => (Some(first), name),
        None => (None, first),
    };
    if host.is_empty()
        || host.starts_with('-')
        || host.len() > 255
        || name.is_empty()
        || name.starts_with('-')
        || !name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-' | b'_'))
        || user.is_some_and(|user| {
            user.is_empty()
                || user.starts_with('-')
                || !user
                    .bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_')
        })
    {
        Err(format!("pi: unsafe SSH host `{host}`"))
    } else {
        Ok(())
    }
}

fn validate_case(case: &str) -> Result<(), String> {
    if case.is_empty()
        || case.starts_with('-')
        || case.len() > 128
        || !case
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
    {
        Err(format!("pi: unsafe case/workload name `{case}`"))
    } else {
        Ok(())
    }
}

fn case_target(case: &str) -> Result<PathBuf, String> {
    validate_case(case)?;
    let case_dir = crate::proxy_validation::proxy_fixture_dir(&crate::root(), case)?;
    crate::golden::golden_case_target(&case_dir)?
        .ok_or_else(|| format!("pi: `{case}` has no input.wr or valid root marker"))
}

fn prepare(host: &str) -> Result<Prepared, String> {
    prepare_for(host, false)
}

fn prepare_for(host: &str, native_presentation: bool) -> Result<Prepared, String> {
    validate_host(host)?;
    let (vmm, agent) = cached_cross_build(native_presentation)?;
    let vmm_digest = digest_file(&vmm)?;
    let agent_digest = digest_file(&agent)?;
    let agent_name = format!("wrela-lab-agent-{agent_digest}");
    let vmm_name = format!("wrela-vmm-{vmm_digest}");
    let agent_remote = format!("{REMOTE_BIN}/{agent_name}");
    if !remote_file_exists(host, &agent_remote)? {
        sftp_batch(
            host,
            &[
                format!("-mkdir {}", quote_sftp(LAB_ROOT)?),
                format!("-mkdir {}", quote_sftp(REMOTE_BIN)?),
                format!(
                    "put {} {}",
                    quote_sftp_path(&agent)?,
                    quote_sftp(&format!("{REMOTE_BIN}/{agent_name}"))?
                ),
                format!(
                    "chmod 0755 {}",
                    quote_sftp(&format!("{REMOTE_BIN}/{agent_name}"))?
                ),
            ],
        )?;
    }
    let probe_manifest = manifest(
        "probe-binary",
        [
            ("binary", vmm_name.as_str()),
            ("binary_sha256", vmm_digest.as_str()),
        ],
    )?;
    ensure_remote_binary(
        &vmm_name,
        &vmm_digest,
        &vmm,
        || invoke_agent(host, &agent_remote, &probe_manifest),
        |commands| sftp_batch(host, commands),
    )?;
    println!(
        "pi prepare: static VMM {vmm_digest} and lab agent {agent_digest} are present on {host}"
    );
    Ok(Prepared {
        agent_remote,
        agent_sha256: agent_digest,
        vmm_sha256: vmm_digest,
    })
}

fn ensure_remote_binary(
    vmm_name: &str,
    vmm_digest: &str,
    vmm: &Path,
    mut probe: impl FnMut() -> Result<String, String>,
    mut upload: impl FnMut(&[String]) -> Result<(), String>,
) -> Result<(), String> {
    let parse_probe = |text: &str| -> Result<evidence::Record, String> {
        let cache = evidence::parse(text, "wrela-lab-binary-cache-v1")?;
        cache.require_exact_fields(&["binary", "binary_sha256", "verdict"])?;
        if cache.fields["binary"] != vmm_name || cache.fields["binary_sha256"] != vmm_digest {
            return Err(
                "pi prepare: remote cache response does not name the requested binary".into(),
            );
        }
        Ok(cache)
    };
    let cache_text = probe()?;
    let cache = parse_probe(&cache_text)?;
    let uploaded = match cache.fields["verdict"].as_str() {
        "hit" => false,
        "miss" => {
            upload(&[
                format!(
                    "put {} {}",
                    quote_sftp_path(&vmm)?,
                    quote_sftp(&format!("{REMOTE_BIN}/{vmm_name}"))?
                ),
                format!(
                    "chmod 0755 {}",
                    quote_sftp(&format!("{REMOTE_BIN}/{vmm_name}"))?
                ),
            ])?;
            true
        }
        other => {
            return Err(format!(
                "pi prepare: unknown remote cache verdict `{other}`"
            ));
        }
    };
    if uploaded {
        let confirmed_text = probe()?;
        let confirmed = parse_probe(&confirmed_text)?;
        if confirmed.fields["verdict"] != "hit" {
            return Err("pi prepare: uploaded binary did not pass the agent digest probe".into());
        }
    }
    Ok(())
}

fn cached_cross_build(native_presentation: bool) -> Result<(PathBuf, PathBuf), String> {
    let root = crate::root();
    let source_sha256 = vmm_source_digest()?;
    let lock_sha256 = digest_file(&root.join("Cargo.lock"))?;
    let rustc_identity = command_text("rustc", &["-Vv"])?;
    let mut linker_bytes = std::fs::read(root.join("tools/zigcc-aarch64-linux-musl"))
        .map_err(|error| format!("read musl linker wrapper: {error}"))?;
    linker_bytes.extend_from_slice(command_text("zig", &["version"])?.as_bytes());
    let linker_sha256 = wrela_machine::sha256::sha256_hex(&linker_bytes);
    let features = if native_presentation {
        "native-presentation"
    } else {
        "none"
    };
    let key = cross_build_cache_key(&[
        ("dependency_source", lock_sha256.as_str()),
        ("features", features),
        ("hardening", "product-and-diagnostic-v1"),
        ("linker", linker_sha256.as_str()),
        ("lock", lock_sha256.as_str()),
        ("profile", "release"),
        ("rustc", rustc_identity.as_str()),
        ("source", source_sha256.as_str()),
        ("target", "aarch64-unknown-linux-musl"),
    ]);
    let cache_root = root.join("target/wrela-lab/build-cache");
    let cache = cache_root.join(&key);
    let cached_vmm = cache.join("wrela-vmm");
    let cached_agent = cache.join("wrela-lab-agent");
    if cache.exists() {
        let record = evidence::parse(
            &std::fs::read_to_string(cache.join("wrela-cross-build-cache-v1.txt"))
                .map_err(|error| format!("read cross-build cache record: {error}"))?,
            "wrela-cross-build-cache-v1",
        )?;
        record.require_exact_fields(&["agent_sha256", "cache_key", "features", "vmm_sha256"])?;
        if record.fields["cache_key"] != key
            || record.fields["features"] != features
            || record.fields["vmm_sha256"] != digest_file(&cached_vmm)?
            || record.fields["agent_sha256"] != digest_file(&cached_agent)?
        {
            return Err(format!(
                "cross-build cache {} failed digest validation; refuse stale binaries",
                cache.display()
            ));
        }
        println!("pi prepare: local cross-build cache hit {key}");
        return Ok((cached_vmm, cached_agent));
    }

    let mut command = Command::new("cargo");
    command.args([
        "build",
        "--release",
        "--target",
        "aarch64-unknown-linux-musl",
        "-p",
        "wrela-vmm",
        "--bin",
        "wrela-vmm",
        "--bin",
        "wrela-lab-agent",
    ]);
    if native_presentation {
        command.args(["--features", "native-presentation"]);
    }
    crate::run(
        &mut command,
        "cross-build static Rasputin VMM and lab agent",
    )?;
    if vmm_source_digest()? != source_sha256 {
        return Err(
            "cross-build source changed while Cargo was running; refuse to cache it".into(),
        );
    }
    let base = root.join("target/aarch64-unknown-linux-musl/release");
    let vmm = base.join("wrela-vmm");
    let agent = base.join("wrela-lab-agent");
    std::fs::create_dir_all(&cache_root)
        .map_err(|error| format!("create cross-build cache root: {error}"))?;
    let candidate = cache_root.join(format!("candidate-{}", run_nonce()?));
    std::fs::create_dir(&candidate)
        .map_err(|error| format!("create cross-build cache candidate: {error}"))?;
    std::fs::copy(&vmm, candidate.join("wrela-vmm"))
        .map_err(|error| format!("cache cross-built VMM: {error}"))?;
    std::fs::copy(&agent, candidate.join("wrela-lab-agent"))
        .map_err(|error| format!("cache cross-built lab agent: {error}"))?;
    let mut record = evidence::Record::new("wrela-cross-build-cache-v1")?;
    for (field, value) in [
        ("agent_sha256", digest_file(&agent)?),
        ("cache_key", key.clone()),
        ("features", features.to_string()),
        ("vmm_sha256", digest_file(&vmm)?),
    ] {
        record.insert(field, value)?;
    }
    std::fs::write(
        candidate.join("wrela-cross-build-cache-v1.txt"),
        record.encode()?,
    )
    .map_err(|error| format!("write cross-build cache record: {error}"))?;
    std::fs::rename(&candidate, &cache)
        .map_err(|error| format!("publish cross-build cache: {error}"))?;
    println!("pi prepare: populated local cross-build cache {key}");
    Ok((cached_vmm, cached_agent))
}

fn cross_build_cache_key(fields: &[(&str, &str)]) -> String {
    let mut bytes = b"wrela-cross-build-cache-v1\0".to_vec();
    for (key, value) in fields {
        bytes.extend_from_slice(key.as_bytes());
        bytes.push(0);
        bytes.extend_from_slice(value.as_bytes());
        bytes.push(0xff);
    }
    wrela_machine::sha256::sha256_hex(&bytes)
}

fn probe(host: &str) -> Result<(), String> {
    let prepared = prepare(host)?;
    let artifact = artifact_dir(host, "probe")?;
    collect_probe(host, &prepared, &artifact, "diagnostic")?;
    println!(
        "pi probe: canonical records retrieved to {}",
        artifact.display()
    );
    Ok(())
}

struct ProbeSet {
    identity: evidence::Record,
    profile: evidence::Record,
    environment: evidence::Record,
}

fn collect_probe(
    host: &str,
    prepared: &Prepared,
    artifact: &Path,
    hardening_mode: &str,
) -> Result<ProbeSet, String> {
    std::fs::create_dir_all(&artifact)
        .map_err(|e| format!("create {}: {e}", artifact.display()))?;
    let specs = [
        ("identity", "probe-identity", None, evidence::HOST_IDENTITY),
        (
            "profile",
            "probe-profile",
            Some(("hardening_mode", hardening_mode)),
            evidence::HOST_PROFILE,
        ),
        (
            "environment",
            "probe-environment",
            None,
            evidence::RUN_ENVIRONMENT,
        ),
    ];
    let mut parsed = BTreeMap::new();
    for (name, action, extra, format) in specs {
        let manifest = manifest(action, extra.into_iter())?;
        let output = invoke_agent(host, &prepared.agent_remote, &manifest)?;
        let record = evidence::parse(&output, format)?;
        evidence::validate_typed(&record)?;
        std::fs::write(artifact.join(format!("{name}.txt")), output)
            .map_err(|e| format!("write probe {name}: {e}"))?;
        parsed.insert(name, record);
    }
    Ok(ProbeSet {
        identity: parsed.remove("identity").unwrap(),
        profile: parsed.remove("profile").unwrap(),
        environment: parsed.remove("environment").unwrap(),
    })
}

fn run_case(
    host: &str,
    case: &str,
    native: bool,
    host_profile: &str,
    measurement: &str,
) -> Result<(), String> {
    run_case_mode(
        host,
        case,
        native,
        host_profile,
        measurement,
        "sealed-stage1",
        None,
        native,
        None,
    )
    .map(|_| ())
}

fn run_case_mode(
    host: &str,
    case: &str,
    native: bool,
    host_profile: &str,
    measurement: &str,
    translation: &str,
    replay_record: Option<&Path>,
    native_build: bool,
    prepared: Option<&Prepared>,
) -> Result<RunDeployment, String> {
    validate_host(host)?;
    let input = case_target(case)?;
    let owned_prepared;
    let prepared = if let Some(prepared) = prepared {
        prepared
    } else {
        owned_prepared = prepare_for(host, native_build)?;
        &owned_prepared
    };
    let local = artifact_dir(host, case)?;
    std::fs::create_dir_all(&local).map_err(|e| format!("create {}: {e}", local.display()))?;
    let build = local.join("build");
    if build.exists() {
        std::fs::remove_dir_all(&build).map_err(|e| format!("clear {}: {e}", build.display()))?;
    }
    let exe = std::env::current_exe().map_err(|e| format!("locate xtask: {e}"))?;
    let output = Command::new(exe)
        .current_dir(crate::root())
        .arg("__wrela")
        .arg("test")
        .arg(&input)
        .arg("--emit-image-dir")
        .arg(&build)
        .arg("--mode=release")
        .output()
        .map_err(|e| format!("build `{case}`: {e}"))?;
    if !output.status.success() {
        return Err(format!(
            "build `{case}` failed:\n{}",
            String::from_utf8_lossy(&output.stderr)
        ));
    }
    let (report, image) = find_build_artifacts(&build)?;
    let report_digest = digest_file(&report)?;
    let image_digest = digest_file(&image)?;
    let run_leaf = format!("run-{}", run_nonce()?);
    let remote_dir = format!("{LAB_ROOT}/runs/{run_leaf}");
    let report_name = format!("report-{report_digest}.txt");
    let image_name = format!("image-{image_digest}.img");
    let vmm_name = format!("wrela-vmm-{}", prepared.vmm_sha256);
    let mut transfers = vec![
        format!("-mkdir {}", quote_sftp(&format!("{LAB_ROOT}/runs"))?),
        format!("mkdir {}", quote_sftp(&remote_dir)?),
        format!(
            "put {} {}",
            quote_sftp_path(&report)?,
            quote_sftp(&format!("{remote_dir}/{report_name}"))?
        ),
        format!(
            "put {} {}",
            quote_sftp_path(&image)?,
            quote_sftp(&format!("{remote_dir}/{image_name}"))?
        ),
    ];
    let replay_digest = if let Some(path) = replay_record {
        let digest = digest_file(path)?;
        transfers.push(format!(
            "put {} {}",
            quote_sftp_path(path)?,
            quote_sftp(&format!("{remote_dir}/record.txt"))?
        ));
        digest
    } else {
        "none".to_string()
    };
    sftp_batch(host, &transfers)?;
    let display = if native { "native" } else { "headless" };
    let fields = BTreeMap::from([
        ("binary".to_string(), vmm_name),
        ("binary_sha256".to_string(), prepared.vmm_sha256.clone()),
        ("display".to_string(), display.to_string()),
        ("host_profile".to_string(), host_profile.to_string()),
        ("image".to_string(), image_name),
        ("image_sha256".to_string(), image_digest),
        ("measurement".to_string(), measurement.to_string()),
        ("record".to_string(), "record.txt".to_string()),
        (
            "record_mode".to_string(),
            if replay_record.is_some() {
                "replay".to_string()
            } else {
                "record".to_string()
            },
        ),
        ("record_sha256".to_string(), replay_digest),
        ("report".to_string(), report_name),
        ("report_sha256".to_string(), report_digest),
        ("run_dir".to_string(), remote_dir.clone()),
        ("timeout_seconds".to_string(), "1800".to_string()),
        ("translation".to_string(), translation.to_string()),
    ]);
    let manifest = manifest(
        "run",
        fields
            .iter()
            .map(|(key, value)| (key.as_str(), value.as_str())),
    )?;
    let result = invoke_agent(host, &prepared.agent_remote, &manifest)?;
    let parsed = evidence::parse(&result, "wrela-lab-run-result-v1")?;
    parsed.require_exact_fields(&[
        "elapsed_ns",
        "exit_code",
        "host_profile",
        "measurement",
        "metrics_sha256",
        "perf_sha256",
        "record_sha256",
        "stderr_sha256",
        "stdout_sha256",
        "timed_out",
        "translation",
    ])?;
    if parsed.fields["host_profile"] != host_profile {
        return Err("pi run: lab result host profile differs from the requested profile".into());
    }
    if parsed.fields["measurement"] != measurement {
        return Err("pi run: lab result measurement mode differs from the request".into());
    }
    if parsed.fields["translation"] != translation {
        return Err("pi run: lab result translation differs from the request".into());
    }
    if parsed.fields["timed_out"] != "false" {
        return Err("pi run: remote VMM exceeded its bounded wall deadline".into());
    }
    for key in [
        "perf_sha256",
        "metrics_sha256",
        "record_sha256",
        "stderr_sha256",
        "stdout_sha256",
    ] {
        evidence::require_sha256(key, &parsed.fields[key])?;
    }
    for key in ["elapsed_ns", "exit_code"] {
        evidence::canonical_u64(key, &parsed.fields[key])?;
    }
    retrieve_run(host, &remote_dir, &local, measurement == "perf-stat")?;
    for (field, file) in [
        ("record_sha256", "record.txt"),
        ("metrics_sha256", "metrics.txt"),
        ("stderr_sha256", "stderr.txt"),
        ("stdout_sha256", "stdout.bin"),
    ] {
        let got = digest_file(&local.join(file))?;
        if parsed.fields[field] != got {
            return Err(format!(
                "pi run `{case}`: retrieved {file} digest is {got}, result declared {}",
                parsed.fields[field]
            ));
        }
    }
    if measurement == "perf-stat" {
        let got = digest_file(&local.join("perf.csv"))?;
        if parsed.fields["perf_sha256"] != got {
            return Err(format!(
                "pi run `{case}`: retrieved perf.csv digest is {got}, result declared {}",
                parsed.fields["perf_sha256"]
            ));
        }
    }
    if parsed.fields["exit_code"] != "0" {
        return Err(format!(
            "pi run `{case}`: remote VMM exited {}; artifacts: {}",
            parsed.fields["exit_code"],
            local.display()
        ));
    }
    println!(
        "pi run: `{case}` passed on {host}; artifacts: {}",
        local.display()
    );
    Ok(RunDeployment { fields })
}

fn conformance(host: &str) -> Result<(), String> {
    for case in [
        "boot-actor-smoke",
        "boot-actors",
        "boot-blk-roundtrip",
        "boot-blk-two-devices",
        "boot-cross-core-admission-order",
        "boot-device-claim",
        "boot-irq-isr",
        "boot-secondary-core-vector",
        "boot-deadline-cancel",
    ] {
        run_case(host, case, false, "product", "none")?;
        let actual = std::fs::read(artifact_dir(host, case)?.join("stdout.bin"))
            .map_err(|error| format!("pi conformance: read `{case}` output: {error}"))?;
        let expected = std::fs::read(
            crate::root()
                .join("tests/golden")
                .join(case)
                .join("expected/test.txt"),
        )
        .map_err(|error| format!("pi conformance: read `{case}` golden: {error}"))?;
        if actual != expected {
            return Err(format!(
                "pi conformance: `{case}` KVM transcript differs from the shared golden"
            ));
        }
    }
    cross_host_record_replay(host)?;
    println!("pi conformance: named portable KVM corpus passed on {host}");
    Ok(())
}

fn cross_host_record_replay(host: &str) -> Result<(), String> {
    let prepared = prepare_for(host, false)?;
    let probe = collect_probe(
        host,
        &prepared,
        &artifact_dir(host, "backend-conformance-probe")?,
        "product",
    )?;
    require_conforming_probe(&probe)?;
    std::fs::write(
        crate::root().join("bench/results/rasputin-host-identity-v1.txt"),
        probe.identity.encode()?,
    )
    .map_err(|error| format!("write checked host identity: {error}"))?;
    std::fs::write(
        crate::root().join("bench/results/rasputin-product-host-profile-v1.txt"),
        probe.profile.encode()?,
    )
    .map_err(|error| format!("write checked product host profile: {error}"))?;
    let source_contract = backend_conformance_source_digest()?;
    for case in [
        "boot-instant-monotonic",
        "boot-entropy",
        "boot-pixels-plane-one-core",
    ] {
        cross_host_record_replay_case(host, case, &prepared, &probe, &source_contract)?;
    }
    Ok(())
}

fn cross_host_record_replay_case(
    host: &str,
    case: &str,
    prepared: &Prepared,
    probe: &ProbeSet,
    source_contract: &str,
) -> Result<(), String> {
    let live_deployment = run_case_mode(
        host,
        case,
        false,
        "product",
        "none",
        "sealed-stage1",
        None,
        false,
        Some(prepared),
    )?;
    let live = artifact_dir(host, case)?;
    let out = artifact_dir(host, &format!("backend-conformance-{case}"))?;
    if out.exists() {
        std::fs::remove_dir_all(&out)
            .map_err(|error| format!("clear {}: {error}", out.display()))?;
    }
    std::fs::create_dir_all(&out).map_err(|error| format!("create {}: {error}", out.display()))?;
    for name in ["record.txt", "stdout.bin"] {
        std::fs::copy(live.join(name), out.join(format!("kvm-{name}")))
            .map_err(|error| format!("preserve KVM {name}: {error}"))?;
    }
    let (report, image) = find_build_artifacts(&live.join("build"))?;
    let hvf_record = out.join("hvf-record.txt");
    let hvf = crate::golden::build_and_sign_vmm()?;
    let hvf_binary_sha256 = digest_file(&hvf)?;
    let hvf_live = Command::new(&hvf)
        .args([
            report.as_os_str(),
            image.as_os_str(),
            "--record".as_ref(),
            hvf_record.as_os_str(),
        ])
        .env("WRELA_HOST_PROFILE", "diagnostic")
        .output()
        .map_err(|error| format!("run signed HVF record: {error}"))?;
    if !hvf_live.status.success() {
        return Err(format!(
            "signed HVF recording failed: {}",
            String::from_utf8_lossy(&hvf_live.stderr)
        ));
    }
    let hvf_stdout = out.join("hvf-stdout.bin");
    std::fs::write(&hvf_stdout, &hvf_live.stdout)
        .map_err(|error| format!("write HVF transcript: {error}"))?;
    let kvm_stdout = std::fs::read(out.join("kvm-stdout.bin"))
        .map_err(|error| format!("read KVM transcript: {error}"))?;
    if hvf_live.stdout != kvm_stdout {
        return Err("backend conformance: live HVF and KVM transcripts differ".into());
    }
    let kvm_stdout_path = out.join("kvm-stdout.bin");
    let kvm_record = out.join("kvm-record.txt");
    for (record_backend, record_path) in
        [("hvf", hvf_record.as_path()), ("kvm", kvm_record.as_path())]
    {
        let hvf_replay = Command::new(&hvf)
            .args([
                report.as_os_str(),
                image.as_os_str(),
                "--replay".as_ref(),
                record_path.as_os_str(),
            ])
            .env("WRELA_HOST_PROFILE", "diagnostic")
            .output()
            .map_err(|error| {
                format!("replay {record_backend} record through signed HVF: {error}")
            })?;
        if !hvf_replay.status.success() || hvf_replay.stdout != kvm_stdout {
            return Err(format!(
                "backend conformance: {record_backend}-to-HVF replay failed or changed output: {}",
                String::from_utf8_lossy(&hvf_replay.stderr)
            ));
        }
        std::fs::write(
            out.join(format!("hvf-replay-{record_backend}-stdout.bin")),
            &hvf_replay.stdout,
        )
        .map_err(|error| format!("retain HVF replay output: {error}"))?;

        run_case_mode(
            host,
            case,
            false,
            "product",
            "none",
            "sealed-stage1",
            Some(record_path),
            false,
            Some(prepared),
        )?;
        let replayed = std::fs::read(artifact_dir(host, case)?.join("stdout.bin"))
            .map_err(|error| format!("read KVM replay output: {error}"))?;
        if replayed != kvm_stdout {
            return Err(format!(
                "backend conformance: {record_backend}-to-KVM replay changed output"
            ));
        }
        std::fs::write(
            out.join(format!("kvm-replay-{record_backend}-stdout.bin")),
            replayed,
        )
        .map_err(|error| format!("retain KVM replay output: {error}"))?;
    }
    let replay_outputs = [
        ("hvf-from-hvf", out.join("hvf-replay-hvf-stdout.bin")),
        ("kvm-from-hvf", out.join("kvm-replay-hvf-stdout.bin")),
        ("hvf-from-kvm", out.join("hvf-replay-kvm-stdout.bin")),
        ("kvm-from-kvm", out.join("kvm-replay-kvm-stdout.bin")),
    ];
    let cross_replay_matrix_sha256 = replay_matrix_digest(&replay_outputs, &kvm_stdout)?;

    for (backend, record_path, stdout_path) in [
        ("hvf", hvf_record.as_path(), hvf_stdout.as_path()),
        ("kvm", kvm_record.as_path(), kvm_stdout_path.as_path()),
    ] {
        let stats = parse_choice_record(
            &std::fs::read_to_string(record_path)
                .map_err(|error| format!("read {backend} record: {error}"))?,
        )?;
        let transcript_sha256 = digest_file(stdout_path)?;
        if stats.transcript_digest != transcript_sha256 {
            return Err(format!(
                "backend conformance: {backend} record transcript digest does not bind its output"
            ));
        }
        let mut evidence = evidence::Record::new(evidence::BACKEND_CONFORMANCE)?;
        for (key, value) in [
            ("backend", backend.to_string()),
            (
                "backend_binary_sha256",
                if backend == "hvf" {
                    hvf_binary_sha256.clone()
                } else {
                    live_deployment.fields["binary_sha256"].clone()
                },
            ),
            ("case", case.to_string()),
            ("choice_count", stats.choices.to_string()),
            (
                "cross_replay_matrix_sha256",
                cross_replay_matrix_sha256.clone(),
            ),
            ("exit_class", "guest-exit".to_string()),
            ("exit_count", stats.exits.to_string()),
            ("frame_count", stats.frame_digests.len().to_string()),
            ("frame_digest_sequence", stats.frame_digests.join(",")),
            ("guest_exit_code", stats.exit_code.to_string()),
            ("image_sha256", digest_file(&image)?),
            ("kvm_host_identity_sha256", probe.identity.digest_hex()?),
            ("kvm_host_profile_sha256", probe.profile.digest_hex()?),
            (
                "machine_revision",
                wrela_machine::MACHINE_REVISION_STR.to_string(),
            ),
            ("record_sha256", digest_file(record_path)?),
            ("report_sha256", digest_file(&report)?),
            ("source_contract_sha256", source_contract.to_string()),
            ("transcript_sha256", transcript_sha256),
        ] {
            evidence.insert(key, value)?;
        }
        evidence::validate_typed(&evidence)?;
        std::fs::write(out.join(format!("{backend}.txt")), evidence.encode()?)
            .map_err(|error| format!("write {backend} conformance: {error}"))?;
    }
    let checked = crate::root().join("bench/results/backend-conformance");
    std::fs::create_dir_all(&checked)
        .map_err(|error| format!("create {}: {error}", checked.display()))?;
    for backend in ["hvf", "kvm"] {
        std::fs::copy(
            out.join(format!("{backend}.txt")),
            checked.join(format!("{case}-{backend}.txt")),
        )
        .map_err(|error| format!("check in {backend} conformance record: {error}"))?;
        std::fs::copy(
            out.join(format!("{backend}-record.txt")),
            checked.join(format!("{case}-{backend}.record.txt")),
        )
        .map_err(|error| format!("check in {backend} choice record: {error}"))?;
    }
    for (label, path) in replay_outputs {
        std::fs::copy(path, checked.join(format!("{case}-{label}.stdout.bin")))
            .map_err(|error| format!("check in {label} replay output: {error}"))?;
    }
    std::fs::copy(&kvm_stdout_path, checked.join(format!("{case}.stdout.bin")))
        .map_err(|error| format!("check in shared conformance transcript: {error}"))?;
    Ok(())
}

fn benchmark(host: &str, workload: &str) -> Result<(), String> {
    validate_host(host)?;
    validate_case(workload)?;
    let prepared = prepare(host)?;
    let out = artifact_dir(host, &format!("benchmark-{workload}"))?;
    if out.exists() {
        std::fs::remove_dir_all(&out)
            .map_err(|error| format!("clear {}: {error}", out.display()))?;
    }
    std::fs::create_dir_all(&out).map_err(|error| format!("create {}: {error}", out.display()))?;
    let before = collect_probe(host, &prepared, &out.join("before"), "product")?;
    require_conforming_probe(&before)?;

    run_case(host, workload, false, "product", "perf-stat")?;
    preserve_run_artifacts(host, workload, &out.join("warmup"))?;

    const SAMPLES: usize = 5;
    let mut samples = Vec::new();
    let mut vcpu_run_samples: Vec<Vec<u64>> = Vec::new();
    let mut record_digests = Vec::new();
    let mut output_digest: Option<String> = None;
    let mut exit_count: Option<u64> = None;
    let mut frame_digests: Option<Vec<String>> = None;
    let mut counters: BTreeMap<String, Vec<u64>> = BTreeMap::new();
    for index in 0..SAMPLES {
        run_case(host, workload, false, "product", "perf-stat")?;
        let run = artifact_dir(host, workload)?;
        let result_text = std::fs::read_to_string(run.join("result.txt"))
            .map_err(|error| format!("read benchmark result: {error}"))?;
        let result = evidence::parse(&result_text, "wrela-lab-run-result-v1")?;
        samples.push(evidence::canonical_u64(
            "benchmark elapsed_ns",
            &result.fields["elapsed_ns"],
        )?);
        vcpu_run_samples.push(parse_metrics(
            &std::fs::read_to_string(run.join("metrics.txt"))
                .map_err(|error| format!("read benchmark vCPU metrics: {error}"))?,
            "product",
            "sealed-stage1",
        )?);
        let digest = result.fields["stdout_sha256"].clone();
        if output_digest.as_ref().is_some_and(|old| old != &digest) {
            return Err("pi bench: output digest changed between samples".into());
        }
        output_digest = Some(digest);
        record_digests.push(result.fields["record_sha256"].clone());
        let record_text = std::fs::read_to_string(run.join("record.txt"))
            .map_err(|error| format!("read benchmark choice record: {error}"))?;
        let stats = parse_choice_record(&record_text)?;
        if exit_count.is_some_and(|old| old != stats.exits) {
            return Err("pi bench: host exit count changed between samples".into());
        }
        if frame_digests
            .as_ref()
            .is_some_and(|old| old != &stats.frame_digests)
        {
            return Err("pi bench: frame digest sequence changed between samples".into());
        }
        exit_count = Some(stats.exits);
        frame_digests = Some(stats.frame_digests);
        let perf_text = std::fs::read_to_string(run.join("perf.csv"))
            .map_err(|error| format!("read benchmark perf output: {error}"))?;
        for (name, value) in parse_perf(&perf_text)? {
            counters.entry(name).or_default().push(value);
        }
        preserve_run_artifacts(host, workload, &out.join(format!("sample-{index:04}")))?;
    }
    let after = collect_probe(host, &prepared, &out.join("after"), "product")?;
    require_conforming_probe(&after)?;
    if before.identity.digest_hex()? != after.identity.digest_hex()?
        || before.profile.digest_hex()? != after.profile.digest_hex()?
    {
        return Err("pi bench: host identity or product profile changed during sampling".into());
    }

    let run = artifact_dir(host, workload)?;
    let (report_path, image_path) = find_build_artifacts(&run.join("build"))?;
    let report_text = std::fs::read_to_string(&report_path)
        .map_err(|error| format!("read benchmark report: {error}"))?;
    let report = wrela_machine::report::parse_report(&report_text)
        .map_err(|error| format!("parse benchmark report: {error}"))?;
    let stage1 = report
        .stage1
        .as_ref()
        .ok_or("pi bench: report has no sealed stage-1 tables")?;
    let mut sorted = samples.clone();
    sorted.sort_unstable();
    let median = sorted[(sorted.len() - 1) / 2];
    let counter_rows = counters
        .iter()
        .map(|(name, values)| {
            let mut values = values.clone();
            values.sort_unstable();
            format!("{name}:{}", values[(values.len() - 1) / 2])
        })
        .collect::<Vec<_>>()
        .join(",");
    let frames = frame_digests.unwrap_or_default();
    let input = case_target(workload)?;
    let mut record = evidence::Record::new(evidence::PI_BENCHMARK)?;
    for (key, value) in [
        ("acceptance_verdict", "pass".to_string()),
        ("cpu_model", before.identity.fields["cpu_model"].clone()),
        ("display_mode", "headless".to_string()),
        ("frame_count", frames.len().to_string()),
        ("frame_digest_sequence", frames.join(",")),
        (
            "frequency_policy",
            before.profile.fields["frequency_policy"].clone(),
        ),
        ("governor", before.profile.fields["governor"].clone()),
        (
            "guest_exit_count",
            exit_count.unwrap_or_default().to_string(),
        ),
        (
            "host_affinity",
            before.profile.fields["host_affinity"].clone(),
        ),
        ("host_identity_sha256", before.identity.digest_hex()?),
        ("host_profile_sha256", before.profile.digest_hex()?),
        (
            "host_protection_profile",
            before.profile.fields["host_protection_profile"].clone(),
        ),
        ("image_sha256", digest_file(&image_path)?),
        (
            "kernel_release",
            before.identity.fields["kernel_release"].clone(),
        ),
        ("max_ns", sorted.last().unwrap().to_string()),
        (
            "measurement_scope",
            "protected-vmm-process-perf-stat-v1".to_string(),
        ),
        ("median_ns", median.to_string()),
        ("min_ns", sorted[0].to_string()),
        ("network_time_excluded", "yes".to_string()),
        (
            "online_cores",
            before.environment.fields["online_cores"].clone(),
        ),
        ("optional_counter_rows", counter_rows),
        ("output_sha256", output_digest.unwrap()),
        ("record_digest_sequence", record_digests.join(",")),
        ("report_sha256", digest_file(&report_path)?),
        (
            "run_environment_after_sha256",
            after.environment.digest_hex()?,
        ),
        (
            "run_environment_before_sha256",
            before.environment.digest_hex()?,
        ),
        ("sample_count", SAMPLES.to_string()),
        (
            "samples_ns",
            samples
                .iter()
                .map(u64::to_string)
                .collect::<Vec<_>>()
                .join(","),
        ),
        ("stage1_tables_sha256", stage1.tables_sha256.clone()),
        (
            "temperature_after_millic",
            after.environment.fields["temperature_millic"].clone(),
        ),
        (
            "temperature_before_millic",
            before.environment.fields["temperature_millic"].clone(),
        ),
        (
            "throttle_after",
            after.environment.fields["throttle_flags"].clone(),
        ),
        (
            "throttle_before",
            before.environment.fields["throttle_flags"].clone(),
        ),
        (
            "vcpu_affinity",
            before.profile.fields["vcpu_affinity"].clone(),
        ),
        (
            "vcpu_run_ns_samples",
            vcpu_run_samples
                .iter()
                .map(|row| row.iter().map(u64::to_string).collect::<Vec<_>>().join(","))
                .collect::<Vec<_>>()
                .join(";"),
        ),
        ("vmm_binary_sha256", prepared.vmm_sha256.clone()),
        ("vmm_source_sha256", vmm_source_digest()?),
        ("warmup_count", "1".to_string()),
        ("workload", workload.to_string()),
        ("workload_sha256", digest_file(&input)?),
    ] {
        record.insert(key, value)?;
    }
    evidence::validate_typed(&record)?;
    let text = record.encode()?;
    std::fs::write(out.join("benchmark.txt"), text)
        .map_err(|error| format!("write benchmark record: {error}"))?;
    println!(
        "pi bench: passing product record written to {}",
        out.display()
    );
    Ok(())
}

fn stage1_pair(host: &str) -> Result<(), String> {
    const WORKLOAD: &str = "boot-tile-compositor";
    const SAMPLES: usize = 5;
    validate_host(host)?;
    let prepared = prepare(host)?;
    let out = artifact_dir(host, "stage1-pair")?;
    if out.exists() {
        std::fs::remove_dir_all(&out)
            .map_err(|error| format!("clear {}: {error}", out.display()))?;
    }
    std::fs::create_dir_all(&out).map_err(|error| format!("create {}: {error}", out.display()))?;
    let before = collect_probe(host, &prepared, &out.join("before"), "diagnostic")?;
    require_stage1_probe(&before)?;

    for translation in ["diagnostic-mmu-off", "sealed-stage1"] {
        run_case_mode(
            host,
            WORKLOAD,
            false,
            "diagnostic",
            "perf-stat",
            translation,
            None,
            false,
            None,
        )?;
        preserve_run_artifacts(host, WORKLOAD, &out.join(format!("warmup-{translation}")))?;
    }

    let mut off = Stage1Samples::default();
    let mut on = Stage1Samples::default();
    let mut output_digest: Option<String> = None;
    for index in 0..SAMPLES {
        for (translation, samples) in [("diagnostic-mmu-off", &mut off), ("sealed-stage1", &mut on)]
        {
            run_case_mode(
                host,
                WORKLOAD,
                false,
                "diagnostic",
                "perf-stat",
                translation,
                None,
                false,
                None,
            )?;
            let run = artifact_dir(host, WORKLOAD)?;
            let result = evidence::parse(
                &std::fs::read_to_string(run.join("result.txt"))
                    .map_err(|error| format!("read paired result: {error}"))?,
                "wrela-lab-run-result-v1",
            )?;
            let digest = result.fields["stdout_sha256"].clone();
            if output_digest.as_ref().is_some_and(|old| old != &digest) {
                return Err("pi stage1-pair: MMU-off/on output digests differ".into());
            }
            output_digest = Some(digest);
            let metrics = parse_metrics(
                &std::fs::read_to_string(run.join("metrics.txt"))
                    .map_err(|error| format!("read paired vCPU metrics: {error}"))?,
                "diagnostic-nonconforming",
                translation,
            )?;
            if metrics.len() != 1 {
                return Err("pi stage1-pair: fixture must run on exactly one vCPU".into());
            }
            samples.vcpu_run_ns.push(metrics[0]);
            samples
                .record_digests
                .push(result.fields["record_sha256"].clone());
            let stats = parse_choice_record(
                &std::fs::read_to_string(run.join("record.txt"))
                    .map_err(|error| format!("read paired choice record: {error}"))?,
            )?;
            samples.exit_counts.push(stats.exits);
            preserve_run_artifacts(
                host,
                WORKLOAD,
                &out.join(format!("sample-{index:04}-{translation}")),
            )?;
        }
    }
    let after = collect_probe(host, &prepared, &out.join("after"), "diagnostic")?;
    require_stage1_probe(&after)?;
    if before.identity.digest_hex()? != after.identity.digest_hex()?
        || before.profile.digest_hex()? != after.profile.digest_hex()?
    {
        return Err("pi stage1-pair: host identity or diagnostic profile changed".into());
    }
    let run = artifact_dir(host, WORKLOAD)?;
    let (report_path, image_path) = find_build_artifacts(&run.join("build"))?;
    let parsed_report = wrela_machine::report::parse_report(
        &std::fs::read_to_string(&report_path)
            .map_err(|error| format!("read paired report: {error}"))?,
    )
    .map_err(|error| format!("parse paired report: {error}"))?;
    let stage1 = parsed_report
        .stage1
        .as_ref()
        .ok_or("pi stage1-pair: report has no sealed stage-1 tables")?;
    let input = case_target(WORKLOAD)?;
    let mut record = evidence::Record::new(evidence::STAGE1_PAIR)?;
    for (key, value) in [
        ("acceptance_verdict", "pass".to_string()),
        ("host_identity_sha256", before.identity.digest_hex()?),
        (
            "host_page_size",
            before.identity.fields["host_page_size"].clone(),
        ),
        ("host_profile_sha256", before.profile.digest_hex()?),
        ("image_sha256", digest_file(&image_path)?),
        (
            "kernel_release",
            before.identity.fields["kernel_release"].clone(),
        ),
        ("measurement_scope", "kvm-vcpu-run-paired-v1".to_string()),
        ("mmu_off_exit_counts", join_u64(&off.exit_counts)),
        ("mmu_off_record_digests", off.record_digests.join(",")),
        ("mmu_off_vcpu_run_ns", join_u64(&off.vcpu_run_ns)),
        ("mmu_on_exit_counts", join_u64(&on.exit_counts)),
        ("mmu_on_record_digests", on.record_digests.join(",")),
        ("mmu_on_vcpu_run_ns", join_u64(&on.vcpu_run_ns)),
        ("output_sha256", output_digest.unwrap()),
        ("report_sha256", digest_file(&report_path)?),
        (
            "run_environment_after_sha256",
            after.environment.digest_hex()?,
        ),
        (
            "run_environment_before_sha256",
            before.environment.digest_hex()?,
        ),
        ("sample_count", SAMPLES.to_string()),
        ("stage1_tables_sha256", stage1.tables_sha256.clone()),
        (
            "temperature_after_millic",
            after.environment.fields["temperature_millic"].clone(),
        ),
        (
            "temperature_before_millic",
            before.environment.fields["temperature_millic"].clone(),
        ),
        (
            "throttle_after",
            after.environment.fields["throttle_flags"].clone(),
        ),
        (
            "throttle_before",
            before.environment.fields["throttle_flags"].clone(),
        ),
        ("vmm_binary_sha256", prepared.vmm_sha256),
        ("vmm_source_sha256", vmm_source_digest()?),
        ("warmup_count", "1".to_string()),
        ("workload", WORKLOAD.to_string()),
        ("workload_sha256", digest_file(&input)?),
    ] {
        record.insert(key, value)?;
    }
    evidence::validate_typed(&record)?;
    std::fs::write(out.join("stage1-pair.txt"), record.encode()?)
        .map_err(|error| format!("write paired stage-1 record: {error}"))?;
    println!(
        "pi stage1-pair: passing record written to {}",
        out.display()
    );
    Ok(())
}

#[derive(Default)]
struct Stage1Samples {
    vcpu_run_ns: Vec<u64>,
    exit_counts: Vec<u64>,
    record_digests: Vec<String>,
}

fn require_stage1_probe(probe: &ProbeSet) -> Result<(), String> {
    if probe.identity.fields["acceptance_verdict"] != "conforming"
        || probe.environment.fields["acceptance_verdict"] != "conforming"
        || probe.profile.fields["hardening_mode"] != "diagnostic"
        || probe.profile.fields["cpu_isolation"] != "isolcpus=domain,managed_irq,1-3"
        || probe.profile.fields["governor"] != "performance,performance,performance,performance"
        || probe.environment.fields["throttle_flags"] != "throttled=0x0"
    {
        return Err(
            "pi stage1-pair: Rasputin host state is not conforming for paired diagnostics".into(),
        );
    }
    Ok(())
}

fn join_u64(values: &[u64]) -> String {
    values
        .iter()
        .map(u64::to_string)
        .collect::<Vec<_>>()
        .join(",")
}

fn validate_proxy(host: &str, class: &str) -> Result<(), String> {
    validate_host(host)?;
    if !matches!(class, "kernel" | "frame" | "sequence") {
        return Err(format!("pi validate-proxy: unknown class `{class}`"));
    }
    let root = crate::root();
    let calibration = crate::proxy_validation::read_manifest(
        &root.join("bench/proxy-calibration-v1.txt"),
        "calibration",
    )?;
    let holdout = crate::proxy_validation::read_manifest(
        &root.join("bench/proxy-holdout-v1.txt"),
        "holdout",
    )?;
    crate::proxy_validation::validate_candidate_pairs(&calibration, &holdout)?;
    let calibration_case = calibration
        .iter()
        .find(|case| case.workload_class == class)
        .ok_or_else(|| format!("pi validate-proxy: calibration lacks class `{class}`"))?;
    let holdout_case = holdout
        .iter()
        .find(|case| case.workload_class == class)
        .ok_or_else(|| format!("pi validate-proxy: holdout lacks class `{class}`"))?;
    let envelope_path = root.join("bench/proxy-envelopes-v1.txt");
    if !envelope_path.exists() {
        collect_validation_case(host, calibration_case, class == "sequence")?;
        maybe_write_envelope_candidate(host, &calibration)?;
        println!(
            "pi validate-proxy: collected `{class}` calibration evidence; holdout remains sealed until bench/proxy-envelopes-v1.txt is reviewed"
        );
        return Ok(());
    }
    let envelope_text = std::fs::read_to_string(&envelope_path)
        .map_err(|error| format!("read {}: {error}", envelope_path.display()))?;
    crate::proxy_validation::parse_envelopes(&envelope_text)?;
    let calibration_fragment = validation_fragment_path(host, &calibration_case.case)?;
    if !calibration_fragment.exists() {
        return Err(format!(
            "pi validate-proxy: calibration fragment {} is missing; remove the checked envelope, recollect calibration, and review a fresh candidate",
            calibration_fragment.display()
        ));
    }
    collect_validation_case(host, holdout_case, class == "sequence")?;
    maybe_write_validation_candidate(host, &calibration, &holdout)?;
    Ok(())
}

const PROXY_SAMPLES: usize = 5;
const PROXY_WARMUPS: usize = 1;
const PROXY_FRAGMENT_FORMAT: &str = "wrela-proxy-case-fragment-v1";

fn validation_root(host: &str) -> Result<PathBuf, String> {
    artifact_dir(host, "proxy-validation")
}

fn validation_fragment_path(host: &str, case: &str) -> Result<PathBuf, String> {
    validate_case(case)?;
    Ok(validation_root(host)?
        .join("fragments")
        .join(format!("{case}.txt")))
}

fn collect_validation_case(
    host: &str,
    corpus: &crate::proxy_validation::CorpusCase,
    native: bool,
) -> Result<(), String> {
    let prepared = prepare_for(host, true)?;
    let case_root = validation_root(host)?.join("runs").join(&corpus.case);
    std::fs::create_dir_all(&case_root)
        .map_err(|error| format!("create {}: {error}", case_root.display()))?;
    let before = collect_probe(host, &prepared, &case_root.join("before"), "product")?;
    require_conforming_probe(&before)?;
    if native && before.environment.fields["display_mode"] == "headless" {
        return Err(
            "pi validate-proxy: sequence validation requires an active DRM mode before collection"
                .into(),
        );
    }
    let prediction = crate::lane2_freq::validation_prediction(&corpus.case)?;
    run_case_mode(
        host,
        &corpus.case,
        native,
        "product",
        "guest-pmu",
        "sealed-stage1",
        None,
        true,
        Some(&prepared),
    )?;
    preserve_validation_run(host, &corpus.case, &case_root.join("warmup-0000"))?;

    let mut cycle_samples = Vec::new();
    let mut run_ns_samples = Vec::new();
    let mut counter_samples = vec![Vec::<Vec<u64>>::new(); GUEST_COUNTER_NAMES.len()];
    let mut image_sha256 = None;
    let mut report_sha256 = None;
    let mut stage1_tables_sha256 = None;
    let mut record_digests = Vec::new();
    let mut stdout_sha256 = None;
    let mut frame_digests = None;
    let mut frame_count = None;
    let mut last_deployment = None;
    for sample in 0..PROXY_SAMPLES {
        let deployment = run_case_mode(
            host,
            &corpus.case,
            native,
            "product",
            "guest-pmu",
            "sealed-stage1",
            None,
            true,
            Some(&prepared),
        )?;
        last_deployment = Some(deployment);
        let run = artifact_dir(host, &corpus.case)?;
        let metrics = parse_guest_metrics(
            &std::fs::read_to_string(run.join("metrics.txt"))
                .map_err(|error| format!("read proxy metrics: {error}"))?,
            "product",
            "sealed-stage1",
        )?;
        if metrics.counters.len() != prediction.cycles_per_core.len() {
            return Err(format!(
                "pi validate-proxy: `{}` measured {} vCPUs but predicts {}",
                corpus.case,
                metrics.counters.len(),
                prediction.cycles_per_core.len()
            ));
        }
        cycle_samples.push(
            metrics
                .counters
                .iter()
                .map(|row| row[1])
                .collect::<Vec<_>>(),
        );
        run_ns_samples.push(metrics.run_ns.clone());
        for counter in 0..GUEST_COUNTER_NAMES.len() {
            counter_samples[counter]
                .push(metrics.counters.iter().map(|row| row[counter]).collect());
        }
        let result_text = std::fs::read_to_string(run.join("result.txt"))
            .map_err(|error| format!("read proxy result: {error}"))?;
        let result = evidence::parse(&result_text, "wrela-lab-run-result-v1")?;
        let choices = parse_choice_record(
            &std::fs::read_to_string(run.join("record.txt"))
                .map_err(|error| format!("read proxy record: {error}"))?,
        )?;
        let (report, image) = find_build_artifacts(&run.join("build"))?;
        let parsed_report = wrela_machine::report::parse_report(
            &std::fs::read_to_string(&report)
                .map_err(|error| format!("read proxy image report: {error}"))?,
        )
        .map_err(|error| format!("parse proxy image report: {error}"))?;
        let stage1 = parsed_report
            .stage1
            .as_ref()
            .ok_or("pi validate-proxy: image report lacks sealed stage-1 tables")?;
        require_stable(&mut image_sha256, digest_file(&image)?, "image digest")?;
        require_stable(&mut report_sha256, digest_file(&report)?, "report digest")?;
        require_stable(
            &mut stage1_tables_sha256,
            stage1.tables_sha256.clone(),
            "stage-1 table digest",
        )?;
        record_digests.push(result.fields["record_sha256"].clone());
        require_stable(
            &mut stdout_sha256,
            result.fields["stdout_sha256"].clone(),
            "stdout digest",
        )?;
        require_stable(
            &mut frame_digests,
            choices.frame_digests.clone(),
            "frame digest sequence",
        )?;
        require_stable(
            &mut frame_count,
            choices.frame_digests.len() as u64,
            "frame count",
        )?;
        preserve_validation_run(
            host,
            &corpus.case,
            &case_root.join(format!("sample-{sample:04}")),
        )?;
    }
    let sustained = if native {
        sustain_validation(
            host,
            &prepared,
            last_deployment.as_ref().unwrap(),
            &case_root,
        )?
    } else {
        (
            0_u64,
            0_u64,
            String::new(),
            0_u64,
            String::new(),
            0_u64,
            String::new(),
            String::new(),
        )
    };
    let after = collect_probe(host, &prepared, &case_root.join("after"), "product")?;
    require_conforming_probe(&after)?;
    if before.identity.digest_hex()? != after.identity.digest_hex()?
        || before.profile.digest_hex()? != after.profile.digest_hex()?
    {
        return Err("pi validate-proxy: host identity or profile changed during collection".into());
    }
    let counter_rows = encode_counter_vectors(&counter_samples);
    crate::proxy_validation::validate_counter_attribution(
        &counter_rows,
        PROXY_SAMPLES,
        prediction.cycles_per_core.len(),
        prediction.modeled_branch_paths,
        prediction.modeled_memory_accesses,
        prediction.modeled_memory_transitions,
    )?;
    let mut record = evidence::Record::new(PROXY_FRAGMENT_FORMAT)?;
    for (key, value) in [
        ("branch_attribution_verdict", "pass".to_string()),
        ("cache_state", "warm".to_string()),
        ("case", corpus.case.clone()),
        ("corpus_set", corpus.set.clone()),
        ("counter_rows", counter_rows),
        ("frame_count", frame_count.unwrap().to_string()),
        ("frame_digest_sequence", frame_digests.unwrap().join(",")),
        ("host_identity_sha256", before.identity.digest_hex()?),
        ("host_profile_sha256", before.profile.digest_hex()?),
        ("image_sha256", image_sha256.unwrap()),
        ("lab_agent_sha256", prepared.agent_sha256.clone()),
        ("memory_attribution_verdict", "pass".to_string()),
        (
            "modeled_branch_paths",
            prediction.modeled_branch_paths.to_string(),
        ),
        (
            "modeled_memory_accesses",
            prediction.modeled_memory_accesses.to_string(),
        ),
        (
            "modeled_memory_transitions",
            prediction.modeled_memory_transitions.to_string(),
        ),
        (
            "predicted_cycles_per_core",
            join_u64(&prediction.cycles_per_core),
        ),
        (
            "presentation_mode",
            if native { "drm-active" } else { "headless" }.to_string(),
        ),
        (
            "proxy_rules_sha256",
            crate::proxy_validation::proxy_rules_digest(&crate::root())?,
        ),
        ("record_digests", record_digests.join(",")),
        ("report_sha256", report_sha256.unwrap()),
        (
            "run_environment_after_sha256",
            after.environment.digest_hex()?,
        ),
        (
            "run_environment_before_sha256",
            before.environment.digest_hex()?,
        ),
        ("sample_count", PROXY_SAMPLES.to_string()),
        ("samples_cycles_per_vcpu", encode_vectors(&cycle_samples)),
        (
            "samples_frame_cadence_ns",
            if frame_count.unwrap() == 0 {
                String::new()
            } else {
                run_ns_samples
                    .iter()
                    .map(|row| row.iter().copied().max().unwrap().to_string())
                    .collect::<Vec<_>>()
                    .join(",")
            },
        ),
        ("selection_decision", corpus.selection_decision.clone()),
        ("stage1_tables_sha256", stage1_tables_sha256.unwrap()),
        ("stdout_sha256", stdout_sha256.unwrap()),
        ("sustained_duration_ns", sustained.0.to_string()),
        ("sustained_frame_count", sustained.1.to_string()),
        ("sustained_frame_digest_sequence", sustained.2),
        ("sustained_launch_count", sustained.3.to_string()),
        ("sustained_record_sha256", sustained.4),
        ("sustained_refresh_hz", sustained.5.to_string()),
        ("sustained_stdout_sha256", sustained.6),
        ("sustained_vsync_sequence", sustained.7),
        (
            "temperature_after_millic",
            after.environment.fields["temperature_millic"].clone(),
        ),
        (
            "temperature_before_millic",
            before.environment.fields["temperature_millic"].clone(),
        ),
        (
            "throttle_after",
            after.environment.fields["throttle_flags"].clone(),
        ),
        (
            "throttle_before",
            before.environment.fields["throttle_flags"].clone(),
        ),
        ("vmm_binary_sha256", prepared.vmm_sha256),
        ("warmup_count", PROXY_WARMUPS.to_string()),
        ("workload_class", corpus.workload_class.clone()),
        ("workload_sha256", prediction.workload_sha256),
    ] {
        record.insert(key, value)?;
    }
    let fragment = validation_fragment_path(host, &corpus.case)?;
    std::fs::create_dir_all(fragment.parent().unwrap())
        .map_err(|error| format!("create fragment directory: {error}"))?;
    std::fs::write(&fragment, record.encode()?)
        .map_err(|error| format!("write {}: {error}", fragment.display()))?;
    println!(
        "pi validate-proxy: wrote {} conforming samples for `{}` to {}",
        PROXY_SAMPLES,
        corpus.case,
        fragment.display()
    );
    Ok(())
}

fn sustain_validation(
    host: &str,
    prepared: &Prepared,
    deployment: &RunDeployment,
    case_root: &Path,
) -> Result<(u64, u64, String, u64, String, u64, String, String), String> {
    let mut fields = deployment.fields.clone();
    fields.insert("duration_seconds".into(), "120".into());
    let manifest = manifest(
        "sustain",
        fields
            .iter()
            .map(|(key, value)| (key.as_str(), value.as_str())),
    )?;
    let text = invoke_agent(host, &prepared.agent_remote, &manifest)?;
    let record = evidence::parse(&text, "wrela-lab-sustain-v1")?;
    record.require_exact_fields(&[
        "duration_ns",
        "frame_count",
        "frame_digest_sequence",
        "launch_count",
        "record_sha256",
        "refresh_hz",
        "stdout_sha256",
        "vsync_sequence",
    ])?;
    let duration = evidence::canonical_u64("duration_ns", &record.fields["duration_ns"])?;
    let frame_count = evidence::canonical_u64("frame_count", &record.fields["frame_count"])?;
    let launches = evidence::canonical_u64("launch_count", &record.fields["launch_count"])?;
    let refresh_hz = evidence::canonical_u64("refresh_hz", &record.fields["refresh_hz"])?;
    if duration < 120_000_000_000 || frame_count < 2 || launches != 1 || refresh_hz == 0 {
        return Err(
            "pi validate-proxy: sustained sequence was not one continuous two-minute VMM run"
                .into(),
        );
    }
    for key in ["record_sha256", "stdout_sha256"] {
        evidence::require_sha256(key, &record.fields[key])?;
    }
    let frame_digests = record.fields["frame_digest_sequence"]
        .split(',')
        .collect::<Vec<_>>();
    if frame_digests.len() != frame_count as usize {
        return Err("pi validate-proxy: sustained frame digest sequence is incomplete".into());
    }
    for digest in frame_digests {
        evidence::require_sha256("sustained frame digest", digest)?;
    }
    let vsync = evidence::parse_u64_list("vsync_sequence", &record.fields["vsync_sequence"])?;
    if vsync.len() != frame_count as usize
        || vsync
            .iter()
            .enumerate()
            .any(|(index, value)| *value != index as u64)
    {
        return Err("pi validate-proxy: sustained vblank sequence is incomplete".into());
    }
    std::fs::write(case_root.join("sustain.txt"), text)
        .map_err(|error| format!("retain sustained sequence evidence: {error}"))?;
    Ok((
        duration,
        frame_count,
        record.fields["frame_digest_sequence"].clone(),
        launches,
        record.fields["record_sha256"].clone(),
        refresh_hz,
        record.fields["stdout_sha256"].clone(),
        record.fields["vsync_sequence"].clone(),
    ))
}

fn require_stable<T: PartialEq>(slot: &mut Option<T>, value: T, what: &str) -> Result<(), String> {
    if slot.as_ref().is_some_and(|old| old != &value) {
        return Err(format!("pi validate-proxy: {what} changed between samples"));
    }
    *slot = Some(value);
    Ok(())
}

fn encode_vectors(rows: &[Vec<u64>]) -> String {
    rows.iter()
        .map(|row| join_u64(row))
        .collect::<Vec<_>>()
        .join(";")
}

fn encode_counter_vectors(rows: &[Vec<Vec<u64>>]) -> String {
    GUEST_COUNTER_NAMES
        .iter()
        .zip(rows)
        .map(|(name, samples)| format!("{name}:{}", encode_vectors(samples)))
        .collect::<Vec<_>>()
        .join("/")
}

fn preserve_validation_run(host: &str, workload: &str, destination: &Path) -> Result<(), String> {
    std::fs::create_dir_all(destination)
        .map_err(|error| format!("create {}: {error}", destination.display()))?;
    let source = artifact_dir(host, workload)?;
    for name in [
        "metrics.txt",
        "record.txt",
        "result.txt",
        "stderr.txt",
        "stdout.bin",
    ] {
        std::fs::copy(source.join(name), destination.join(name))
            .map_err(|error| format!("preserve validation {name}: {error}"))?;
    }
    Ok(())
}

const PROXY_FRAGMENT_FIELDS: &[&str] = &[
    "branch_attribution_verdict",
    "cache_state",
    "case",
    "corpus_set",
    "counter_rows",
    "frame_count",
    "frame_digest_sequence",
    "host_identity_sha256",
    "host_profile_sha256",
    "image_sha256",
    "lab_agent_sha256",
    "memory_attribution_verdict",
    "modeled_branch_paths",
    "modeled_memory_accesses",
    "modeled_memory_transitions",
    "predicted_cycles_per_core",
    "presentation_mode",
    "proxy_rules_sha256",
    "record_digests",
    "report_sha256",
    "run_environment_after_sha256",
    "run_environment_before_sha256",
    "sample_count",
    "samples_cycles_per_vcpu",
    "samples_frame_cadence_ns",
    "selection_decision",
    "stage1_tables_sha256",
    "stdout_sha256",
    "sustained_duration_ns",
    "sustained_frame_count",
    "sustained_frame_digest_sequence",
    "sustained_launch_count",
    "sustained_record_sha256",
    "sustained_refresh_hz",
    "sustained_stdout_sha256",
    "sustained_vsync_sequence",
    "temperature_after_millic",
    "temperature_before_millic",
    "throttle_after",
    "throttle_before",
    "vmm_binary_sha256",
    "warmup_count",
    "workload_class",
    "workload_sha256",
];

fn read_validation_fragment(host: &str, case: &str) -> Result<evidence::Record, String> {
    let path = validation_fragment_path(host, case)?;
    let record = evidence::parse(
        &std::fs::read_to_string(&path)
            .map_err(|error| format!("read {}: {error}", path.display()))?,
        PROXY_FRAGMENT_FORMAT,
    )?;
    record.require_exact_fields(PROXY_FRAGMENT_FIELDS)?;
    if record.fields["case"] != case {
        return Err(format!(
            "proxy fragment {} names the wrong case",
            path.display()
        ));
    }
    for key in [
        "host_identity_sha256",
        "host_profile_sha256",
        "image_sha256",
        "lab_agent_sha256",
        "proxy_rules_sha256",
        "report_sha256",
        "run_environment_after_sha256",
        "run_environment_before_sha256",
        "stage1_tables_sha256",
        "stdout_sha256",
        "vmm_binary_sha256",
        "workload_sha256",
    ] {
        evidence::require_sha256(key, &record.fields[key])?;
    }
    let current_rules = crate::proxy_validation::proxy_rules_digest(&crate::root())?;
    require_current_proxy_rules(&record, &current_rules)?;
    if evidence::canonical_u64("sample_count", &record.fields["sample_count"])?
        != PROXY_SAMPLES as u64
        || evidence::canonical_u64("warmup_count", &record.fields["warmup_count"])?
            != PROXY_WARMUPS as u64
    {
        return Err(format!(
            "proxy fragment `{case}` has the wrong sampling policy"
        ));
    }
    let predicted = evidence::parse_u64_list(
        "predicted_cycles_per_core",
        &record.fields["predicted_cycles_per_core"],
    )?;
    crate::proxy_validation::validate_counter_attribution(
        &record.fields["counter_rows"],
        PROXY_SAMPLES,
        predicted.len(),
        evidence::canonical_u64(
            "modeled_branch_paths",
            &record.fields["modeled_branch_paths"],
        )?,
        evidence::canonical_u64(
            "modeled_memory_accesses",
            &record.fields["modeled_memory_accesses"],
        )?,
        evidence::canonical_u64(
            "modeled_memory_transitions",
            &record.fields["modeled_memory_transitions"],
        )?,
    )?;
    if record.fields["branch_attribution_verdict"] != "pass"
        || record.fields["memory_attribution_verdict"] != "pass"
    {
        return Err(format!(
            "proxy fragment `{case}` has a non-passing modeled-counter attribution"
        ));
    }
    let digests = record.fields["record_digests"]
        .split(',')
        .collect::<Vec<_>>();
    if digests.len() != PROXY_SAMPLES {
        return Err(format!(
            "proxy fragment `{case}` has the wrong record digest count"
        ));
    }
    for digest in digests {
        evidence::require_sha256("record digest", digest)?;
    }
    let duration = evidence::canonical_u64(
        "sustained_duration_ns",
        &record.fields["sustained_duration_ns"],
    )?;
    let frames = evidence::canonical_u64(
        "sustained_frame_count",
        &record.fields["sustained_frame_count"],
    )?;
    let launches = evidence::canonical_u64(
        "sustained_launch_count",
        &record.fields["sustained_launch_count"],
    )?;
    if record.fields["workload_class"] == "sequence" {
        if duration < 120_000_000_000
            || frames < 2
            || launches != 1
            || record.fields["presentation_mode"] != "drm-active"
        {
            return Err(format!(
                "proxy fragment `{case}` lacks one continuous two-minute active-DRM sequence"
            ));
        }
        for key in ["sustained_record_sha256", "sustained_stdout_sha256"] {
            evidence::require_sha256(key, &record.fields[key])?;
        }
        let digests = record.fields["sustained_frame_digest_sequence"]
            .split(',')
            .collect::<Vec<_>>();
        let vsync = evidence::parse_u64_list(
            "sustained_vsync_sequence",
            &record.fields["sustained_vsync_sequence"],
        )?;
        if digests.len() != frames as usize
            || vsync.len() != frames as usize
            || vsync
                .iter()
                .enumerate()
                .any(|(index, value)| *value != index as u64)
            || evidence::canonical_u64(
                "sustained_refresh_hz",
                &record.fields["sustained_refresh_hz"],
            )? == 0
        {
            return Err(format!(
                "proxy fragment `{case}` has incomplete frame/vblank evidence"
            ));
        }
        for digest in digests {
            evidence::require_sha256("sustained frame digest", digest)?;
        }
    } else if duration != 0
        || frames != 0
        || launches != 0
        || !record.fields["sustained_frame_digest_sequence"].is_empty()
        || !record.fields["sustained_record_sha256"].is_empty()
        || record.fields["sustained_refresh_hz"] != "0"
        || !record.fields["sustained_stdout_sha256"].is_empty()
        || !record.fields["sustained_vsync_sequence"].is_empty()
    {
        return Err(format!(
            "proxy fragment `{case}` has unexpected sustained sequence evidence"
        ));
    }
    Ok(record)
}

fn require_current_proxy_rules(
    fragment: &evidence::Record,
    current_rules: &str,
) -> Result<(), String> {
    if fragment
        .fields
        .get("proxy_rules_sha256")
        .map(String::as_str)
        != Some(current_rules)
    {
        return Err(format!(
            "proxy fragment is stale for the current proxy rules (fragment {}, current {current_rules})",
            fragment
                .fields
                .get("proxy_rules_sha256")
                .map(String::as_str)
                .unwrap_or("missing")
        ));
    }
    Ok(())
}

fn parse_sample_vectors(raw: &str) -> Result<Vec<Vec<u64>>, String> {
    raw.split(';')
        .map(|row| evidence::parse_u64_list("proxy sample vector", row))
        .collect()
}

fn proxy_counter_config() -> &'static str {
    "armv8_pmuv3:br_mis_pred=0x10,cpu_cycles=0x11,inst_retired=0x08,l1d_cache_refill=0x03,l2d_cache_refill=0x17,stall_backend=0x24,stall_frontend=0x23;exclude_host=1;exclude_guest=0;pinned-group=yes;time_enabled=time_running;overflow=u64-read"
}

fn maybe_write_envelope_candidate(
    host: &str,
    calibration: &[crate::proxy_validation::CorpusCase],
) -> Result<(), String> {
    if calibration
        .iter()
        .any(|case| !validation_fragment_path(host, &case.case).is_ok_and(|path| path.exists()))
    {
        return Ok(());
    }
    let mut fragments = calibration
        .iter()
        .map(|case| read_validation_fragment(host, &case.case))
        .collect::<Result<Vec<_>, _>>()?;
    fragments
        .sort_by(|left, right| left.fields["workload_class"].cmp(&right.fields["workload_class"]));
    let first = &fragments[0];
    for fragment in &fragments {
        for key in [
            "host_identity_sha256",
            "host_profile_sha256",
            "lab_agent_sha256",
            "proxy_rules_sha256",
            "vmm_binary_sha256",
        ] {
            if fragment.fields[key] != first.fields[key] {
                return Err(format!(
                    "proxy calibration fragments differ in `{key}` provenance"
                ));
            }
        }
    }
    let root = crate::root();
    let mut envelope = evidence::Record::new(crate::proxy_validation::ENVELOPE_FORMAT)?;
    for (key, value) in [
        (
            "calibration_manifest_sha256",
            digest_file(&root.join("bench/proxy-calibration-v1.txt"))?,
        ),
        ("counter_config", proxy_counter_config().to_string()),
        (
            "cost_profile_sha256",
            digest_file(&root.join("bench/a76-pi5.toml"))?,
        ),
        (
            "host_identity_sha256",
            first.fields["host_identity_sha256"].clone(),
        ),
        (
            "host_profile_sha256",
            first.fields["host_profile_sha256"].clone(),
        ),
        ("lab_agent_sha256", first.fields["lab_agent_sha256"].clone()),
        (
            "measurement_error_model",
            "per-class-per-core-max-range-v2".to_string(),
        ),
        (
            "proxy_revision",
            crate::proxy_validation::PROXY_REVISION.to_string(),
        ),
        (
            "proxy_rules_sha256",
            first.fields["proxy_rules_sha256"].clone(),
        ),
        ("sample_count", PROXY_SAMPLES.to_string()),
        (
            "vmm_binary_sha256",
            first.fields["vmm_binary_sha256"].clone(),
        ),
        ("warmup_count", PROXY_WARMUPS.to_string()),
    ] {
        envelope.insert(key, value)?;
    }
    for (index, fragment) in fragments.iter().enumerate() {
        let samples = parse_sample_vectors(&fragment.fields["samples_cycles_per_vcpu"])?;
        let error = crate::proxy_validation::per_core_measurement_error(&samples);
        let ratio = crate::proxy_validation::sealed_overprediction_envelope_milli(
            &fragment.fields["workload_class"],
        )
        .ok_or_else(|| "proxy envelope: unknown workload class".to_string())?;
        for (field, value) in [
            ("case", fragment.fields["case"].clone()),
            ("measurement_error_cycles", error.to_string()),
            ("overprediction_envelope_milli", ratio.max(1000).to_string()),
            (
                "predicted_cycles_per_core",
                fragment.fields["predicted_cycles_per_core"].clone(),
            ),
            (
                "samples_cycles_per_vcpu",
                fragment.fields["samples_cycles_per_vcpu"].clone(),
            ),
            ("workload_class", fragment.fields["workload_class"].clone()),
        ] {
            envelope.insert(&format!("class.{index:04}.{field}"), value)?;
        }
    }
    let encoded = envelope.encode()?;
    crate::proxy_validation::parse_envelopes(&encoded)?;
    let candidate = validation_root(host)?.join("proxy-envelopes-v1.candidate.txt");
    std::fs::write(&candidate, encoded)
        .map_err(|error| format!("write {}: {error}", candidate.display()))?;
    println!(
        "pi validate-proxy: calibration is complete; review the sealed envelope candidate at {} before any holdout run",
        candidate.display()
    );
    Ok(())
}

fn maybe_write_validation_candidate(
    host: &str,
    calibration: &[crate::proxy_validation::CorpusCase],
    holdout: &[crate::proxy_validation::CorpusCase],
) -> Result<(), String> {
    crate::proxy_validation::validate_candidate_pairs(calibration, holdout)?;
    let mut corpus = calibration
        .iter()
        .chain(holdout)
        .cloned()
        .collect::<Vec<_>>();
    if corpus
        .iter()
        .any(|case| !validation_fragment_path(host, &case.case).is_ok_and(|path| path.exists()))
    {
        return Ok(());
    }
    corpus.sort_by(|left, right| left.case.cmp(&right.case));
    let fragments = corpus
        .iter()
        .map(|case| read_validation_fragment(host, &case.case))
        .collect::<Result<Vec<_>, _>>()?;
    let root = crate::root();
    let envelope_path = root.join("bench/proxy-envelopes-v1.txt");
    let envelope_text = std::fs::read_to_string(&envelope_path)
        .map_err(|error| format!("read {}: {error}", envelope_path.display()))?;
    let (envelope_record, envelopes) = crate::proxy_validation::parse_envelopes(&envelope_text)?;
    let envelope_by_class = envelopes
        .iter()
        .map(|envelope| (envelope.workload_class.as_str(), envelope))
        .collect::<BTreeMap<_, _>>();
    let first = &fragments[0];
    for fragment in &fragments {
        for key in [
            "host_identity_sha256",
            "host_profile_sha256",
            "lab_agent_sha256",
            "proxy_rules_sha256",
            "vmm_binary_sha256",
        ] {
            if fragment.fields[key] != first.fields[key] {
                return Err(format!(
                    "proxy validation fragments differ in `{key}` provenance"
                ));
            }
        }
    }
    let mut report = evidence::Record::new(crate::proxy_validation::FORMAT)?;
    let retrieval_time = command_text("/bin/date", &["-u", "+%Y-%m-%dT%H:%M:%SZ"])?;
    let operator = std::env::var("USER").unwrap_or_else(|_| "unknown-local-operator".into());
    let run_environment_sha256 =
        fragments.last().unwrap().fields["run_environment_after_sha256"].clone();
    let linker_identity = {
        let mut bytes = std::fs::read(root.join("tools/zigcc-aarch64-linux-musl"))
            .map_err(|error| format!("read musl linker wrapper: {error}"))?;
        bytes.extend_from_slice(command_text("zig", &["version"])?.as_bytes());
        wrela_machine::sha256::sha256_hex(&bytes)
    };
    for (key, value) in [
        ("build_features", "native-presentation".to_string()),
        ("build_profile", "release".to_string()),
        ("build_target", "aarch64-unknown-linux-musl".to_string()),
        ("cargo_lock_sha256", digest_file(&root.join("Cargo.lock"))?),
        (
            "corpus_manifest_sha256",
            digest_file(&root.join("bench/proxy-calibration-v1.txt"))?,
        ),
        (
            "cost_profile_sha256",
            digest_file(&root.join("bench/a76-pi5.toml"))?,
        ),
        ("counter_config", proxy_counter_config().to_string()),
        (
            "envelope_provenance",
            "repeated-conforming-calibration-v1".to_string(),
        ),
        (
            "envelopes_sha256",
            wrela_machine::sha256::sha256_hex(envelope_text.as_bytes()),
        ),
        (
            "holdout_manifest_sha256",
            digest_file(&root.join("bench/proxy-holdout-v1.txt"))?,
        ),
        (
            "host_identity_sha256",
            first.fields["host_identity_sha256"].clone(),
        ),
        (
            "host_profile_sha256",
            first.fields["host_profile_sha256"].clone(),
        ),
        ("linker_identity_sha256", linker_identity),
        ("lab_agent_sha256", first.fields["lab_agent_sha256"].clone()),
        (
            "measurement_error_model",
            "per-class-per-core-max-range-v2".to_string(),
        ),
        ("operator", operator),
        (
            "proxy_revision",
            crate::proxy_validation::PROXY_REVISION.to_string(),
        ),
        (
            "proxy_rules_sha256",
            first.fields["proxy_rules_sha256"].clone(),
        ),
        ("retrieval_method", "content-addressed-sftp-v1".to_string()),
        ("retrieved_at_utc", retrieval_time),
        ("run_environment_sha256", run_environment_sha256),
        (
            "rustc_identity_sha256",
            wrela_machine::sha256::sha256_hex(command_text("rustc", &["-Vv"])?.as_bytes()),
        ),
        (
            "vmm_binary_sha256",
            first.fields["vmm_binary_sha256"].clone(),
        ),
        ("vmm_source_sha256", vmm_source_digest()?),
    ] {
        report.insert(key, value)?;
    }
    for class in ["frame", "kernel", "sequence"] {
        let envelope = envelope_by_class[class];
        report.insert(
            &format!("{class}_measurement_error_cycles"),
            envelope.measurement_error_cycles.to_string(),
        )?;
        report.insert(
            &format!("{class}_overprediction_envelope_milli"),
            envelope.overprediction_envelope_milli.to_string(),
        )?;
    }

    let mut violations = 0_u64;
    let mut envelope_violations = 0_u64;
    let mut max_ratio = 0_u64;
    let mut case_orders = BTreeMap::new();
    for (index, (corpus, fragment)) in corpus.iter().zip(&fragments).enumerate() {
        if fragment.fields["corpus_set"] != corpus.set
            || fragment.fields["workload_class"] != corpus.workload_class
            || fragment.fields["selection_decision"] != corpus.selection_decision
        {
            return Err(format!(
                "proxy fragment `{}` differs from its manifest",
                corpus.case
            ));
        }
        let samples = parse_sample_vectors(&fragment.fields["samples_cycles_per_vcpu"])?;
        let mut maxima = samples
            .iter()
            .map(|row| row.iter().copied().max().unwrap())
            .collect::<Vec<_>>();
        maxima.sort_unstable();
        let min = maxima[0];
        let median = maxima[(maxima.len() - 1) / 2];
        let max = *maxima.last().unwrap();
        let predicted = evidence::parse_u64_list(
            "predicted cycles",
            &fragment.fields["predicted_cycles_per_core"],
        )?;
        let envelope = envelope_by_class[corpus.workload_class.as_str()];
        let (conservative, ratio) = crate::proxy_validation::per_core_bounds(
            &predicted,
            &samples,
            envelope.measurement_error_cycles,
        );
        let bound = predicted.iter().copied().max().unwrap();
        let inside_envelope = ratio <= envelope.overprediction_envelope_milli;
        violations += u64::from(!conservative);
        envelope_violations += u64::from(!inside_envelope);
        max_ratio = max_ratio.max(ratio);
        case_orders.insert(
            corpus.workload_class.clone(),
            (corpus.case.clone(), bound, median),
        );
        for (field, value) in [
            (
                "branch_attribution_verdict",
                fragment.fields["branch_attribution_verdict"].clone(),
            ),
            ("cache_state", fragment.fields["cache_state"].clone()),
            ("case", corpus.case.clone()),
            (
                "conservatism_verdict",
                if conservative { "pass" } else { "violation" }.to_string(),
            ),
            ("corpus_set", corpus.set.clone()),
            ("counter_rows", fragment.fields["counter_rows"].clone()),
            ("frame_count", fragment.fields["frame_count"].clone()),
            (
                "frame_digest_sequence",
                fragment.fields["frame_digest_sequence"].clone(),
            ),
            ("image_sha256", fragment.fields["image_sha256"].clone()),
            (
                "memory_attribution_verdict",
                fragment.fields["memory_attribution_verdict"].clone(),
            ),
            ("measured_max_cycles", max.to_string()),
            ("measured_median_cycles", median.to_string()),
            ("measured_min_cycles", min.to_string()),
            (
                "measurement_error_cycles",
                envelope.measurement_error_cycles.to_string(),
            ),
            (
                "modeled_branch_paths",
                fragment.fields["modeled_branch_paths"].clone(),
            ),
            (
                "modeled_memory_accesses",
                fragment.fields["modeled_memory_accesses"].clone(),
            ),
            (
                "modeled_memory_transitions",
                fragment.fields["modeled_memory_transitions"].clone(),
            ),
            (
                "overprediction_envelope_verdict",
                if inside_envelope { "pass" } else { "breach" }.to_string(),
            ),
            ("overprediction_ratio_milli", ratio.to_string()),
            (
                "presentation_mode",
                fragment.fields["presentation_mode"].clone(),
            ),
            (
                "predicted_cycles_per_core",
                fragment.fields["predicted_cycles_per_core"].clone(),
            ),
            ("record_digests", fragment.fields["record_digests"].clone()),
            ("report_sha256", fragment.fields["report_sha256"].clone()),
            (
                "run_environment_after_sha256",
                fragment.fields["run_environment_after_sha256"].clone(),
            ),
            (
                "run_environment_before_sha256",
                fragment.fields["run_environment_before_sha256"].clone(),
            ),
            ("sample_count", fragment.fields["sample_count"].clone()),
            (
                "samples_cycles_per_vcpu",
                fragment.fields["samples_cycles_per_vcpu"].clone(),
            ),
            (
                "samples_frame_cadence_ns",
                fragment.fields["samples_frame_cadence_ns"].clone(),
            ),
            (
                "stage1_tables_sha256",
                fragment.fields["stage1_tables_sha256"].clone(),
            ),
            ("stdout_sha256", fragment.fields["stdout_sha256"].clone()),
            (
                "sustained_duration_ns",
                fragment.fields["sustained_duration_ns"].clone(),
            ),
            (
                "sustained_frame_count",
                fragment.fields["sustained_frame_count"].clone(),
            ),
            (
                "sustained_frame_digest_sequence",
                fragment.fields["sustained_frame_digest_sequence"].clone(),
            ),
            (
                "sustained_launch_count",
                fragment.fields["sustained_launch_count"].clone(),
            ),
            (
                "sustained_record_sha256",
                fragment.fields["sustained_record_sha256"].clone(),
            ),
            (
                "sustained_refresh_hz",
                fragment.fields["sustained_refresh_hz"].clone(),
            ),
            (
                "sustained_stdout_sha256",
                fragment.fields["sustained_stdout_sha256"].clone(),
            ),
            (
                "sustained_vsync_sequence",
                fragment.fields["sustained_vsync_sequence"].clone(),
            ),
            (
                "temperature_after_millic",
                fragment.fields["temperature_after_millic"].clone(),
            ),
            (
                "temperature_before_millic",
                fragment.fields["temperature_before_millic"].clone(),
            ),
            ("throttle_after", fragment.fields["throttle_after"].clone()),
            (
                "throttle_before",
                fragment.fields["throttle_before"].clone(),
            ),
            ("workload_class", corpus.workload_class.clone()),
            (
                "workload_sha256",
                fragment.fields["workload_sha256"].clone(),
            ),
        ] {
            report.insert(&format!("case.{index:04}.{field}"), value)?;
        }
    }
    let mut discordant = 0_u64;
    for (index, class) in ["frame", "kernel", "sequence"].into_iter().enumerate() {
        let calibration_case = calibration
            .iter()
            .find(|case| case.workload_class == class)
            .unwrap();
        let holdout_case = holdout
            .iter()
            .find(|case| case.workload_class == class)
            .unwrap();
        if calibration_case.selection_decision != holdout_case.selection_decision {
            return Err(format!(
                "proxy report: `{class}` candidates do not name one compiler selection decision"
            ));
        }
        let find = |name: &str| {
            corpus
                .iter()
                .zip(&fragments)
                .find(|(case, _)| case.case == name)
                .map(|(_, fragment)| fragment)
                .unwrap()
        };
        let first = find(&calibration_case.case);
        let second = find(&holdout_case.case);
        let summary = |fragment: &evidence::Record| -> Result<(u64, u64), String> {
            let predicted = evidence::parse_u64_list(
                "pair predicted cycles",
                &fragment.fields["predicted_cycles_per_core"],
            )?
            .into_iter()
            .max()
            .unwrap();
            let mut measured = parse_sample_vectors(&fragment.fields["samples_cycles_per_vcpu"])?
                .into_iter()
                .map(|row| row.into_iter().max().unwrap())
                .collect::<Vec<_>>();
            measured.sort_unstable();
            Ok((predicted, measured[(measured.len() - 1) / 2]))
        };
        let first_values = summary(first)?;
        let second_values = summary(second)?;
        let noise = envelope_by_class[class].measurement_error_cycles;
        let predicted_order = proxy_order(first_values.0, second_values.0, 0);
        let measured_order = proxy_order(first_values.1, second_values.1, noise);
        let mismatch = predicted_order != measured_order && measured_order != "noise-tie";
        discordant += u64::from(mismatch);
        let pair_name = format!("{class}:{}", calibration_case.selection_decision);
        for (field, value) in [
            (
                "discordant",
                if mismatch { "yes" } else { "no" }.to_string(),
            ),
            ("first_case", calibration_case.case.clone()),
            ("measured_order", measured_order.to_string()),
            ("noise_cycles", noise.to_string()),
            ("pair", pair_name),
            ("predicted_order", predicted_order.to_string()),
            ("second_case", holdout_case.case.clone()),
        ] {
            report.insert(&format!("pair.{index:04}.{field}"), value)?;
        }
    }
    let rate = discordant * 1000 / 3;
    let calibration_ok = fragments
        .iter()
        .zip(&corpus)
        .filter(|(_, case)| case.set == "calibration")
        .all(|(fragment, case)| {
            let envelope = envelope_by_class[case.workload_class.as_str()];
            let predicted = evidence::parse_u64_list(
                "predicted cycles",
                &fragment.fields["predicted_cycles_per_core"],
            )
            .unwrap();
            let samples =
                parse_sample_vectors(&fragment.fields["samples_cycles_per_vcpu"]).unwrap();
            crate::proxy_validation::per_core_bounds(
                &predicted,
                &samples,
                envelope.measurement_error_cycles,
            )
            .0
        });
    let holdout_ok = violations == 0;
    let verdict = calibration_ok && holdout_ok && envelope_violations == 0 && discordant == 0;
    for (key, value) in [
        (
            "calibration_verdict",
            if calibration_ok { "pass" } else { "fail" }.to_string(),
        ),
        ("conservatism_violations", violations.to_string()),
        ("discordance_rate_milli", rate.to_string()),
        (
            "holdout_verdict",
            if holdout_ok { "pass" } else { "fail" }.to_string(),
        ),
        ("max_overprediction_ratio_milli", max_ratio.to_string()),
        ("verdict", if verdict { "pass" } else { "fail" }.to_string()),
    ] {
        report.insert(key, value)?;
    }
    let encoded = report.encode()?;
    crate::proxy_validation::parse_and_validate(&encoded)?;
    let candidate = validation_root(host)?.join("rasputin-proxy-validation-v1.candidate.txt");
    std::fs::write(&candidate, encoded)
        .map_err(|error| format!("write {}: {error}", candidate.display()))?;
    println!(
        "pi validate-proxy: all corpus classes are complete; report candidate is {}",
        candidate.display()
    );
    let _ = envelope_record;
    let _ = case_orders;
    Ok(())
}

fn proxy_order(first: u64, second: u64, noise: u64) -> &'static str {
    if first.abs_diff(second) <= noise {
        "noise-tie"
    } else if first < second {
        "first"
    } else {
        "second"
    }
}

fn command_text(program: &str, args: &[&str]) -> Result<String, String> {
    let output = Command::new(program)
        .args(args)
        .output()
        .map_err(|error| format!("run {program}: {error}"))?;
    if !output.status.success() {
        return Err(format!("{program} {:?} failed", args));
    }
    String::from_utf8(output.stdout)
        .map(|text| text.trim_end().to_string())
        .map_err(|_| format!("{program} output is not UTF-8"))
}

fn require_conforming_probe(probe: &ProbeSet) -> Result<(), String> {
    for (kind, record) in [
        ("identity", &probe.identity),
        ("profile", &probe.profile),
        ("environment", &probe.environment),
    ] {
        if record.fields["acceptance_verdict"] != "conforming" {
            return Err(format!("pi: {kind} record is not conforming"));
        }
    }
    if probe.profile.fields["hardening_mode"] != "product"
        || probe.profile.fields["governor"] != "performance,performance,performance,performance"
        || probe.environment.fields["throttle_flags"] != "throttled=0x0"
    {
        return Err("pi: product frequency, isolation, or throttle state is nonconforming".into());
    }
    Ok(())
}

fn preserve_run_artifacts(host: &str, workload: &str, destination: &Path) -> Result<(), String> {
    std::fs::create_dir_all(destination)
        .map_err(|error| format!("create {}: {error}", destination.display()))?;
    let source = artifact_dir(host, workload)?;
    for name in [
        "perf.csv",
        "metrics.txt",
        "record.txt",
        "result.txt",
        "stderr.txt",
        "stdout.bin",
    ] {
        std::fs::copy(source.join(name), destination.join(name))
            .map_err(|error| format!("preserve benchmark {name}: {error}"))?;
    }
    Ok(())
}

fn parse_metrics(
    text: &str,
    expected_profile: &str,
    expected_translation: &str,
) -> Result<Vec<u64>, String> {
    Ok(parse_vcpu_metrics(text, expected_profile, expected_translation, false)?.run_ns)
}

const GUEST_COUNTER_NAMES: [&str; 7] = [
    "br_mis_pred",
    "cpu_cycles",
    "inst_retired",
    "l1d_cache_refill",
    "l2d_cache_refill",
    "stall_backend",
    "stall_frontend",
];

#[derive(Debug, Clone, PartialEq, Eq)]
struct VcpuMetrics {
    run_ns: Vec<u64>,
    counters: Vec<[u64; 7]>,
}

fn parse_guest_metrics(
    text: &str,
    expected_profile: &str,
    expected_translation: &str,
) -> Result<VcpuMetrics, String> {
    parse_vcpu_metrics(text, expected_profile, expected_translation, true)
}

fn parse_vcpu_metrics(
    text: &str,
    expected_profile: &str,
    expected_translation: &str,
    require_counters: bool,
) -> Result<VcpuMetrics, String> {
    if !text.ends_with('\n') || text.lines().next() != Some("format=wrela-vcpu-run-metrics-v1") {
        return Err("vCPU metrics are not canonical wrela-vcpu-run-metrics-v1".into());
    }
    let lines = text.lines().skip(1).collect::<Vec<_>>();
    let mut cursor = 0_usize;
    let mut run_ns = Vec::new();
    while let Some(line) = lines.get(cursor) {
        let expected = format!("core.{:04}.run_ns=", run_ns.len());
        let Some(value) = line.strip_prefix(&expected) else {
            break;
        };
        if value.contains('=') {
            return Err(format!("malformed vCPU run metric `{line}`"));
        }
        run_ns.push(evidence::canonical_u64("vCPU run ns", value)?);
        cursor += 1;
    }
    if run_ns.is_empty() {
        return Err("vCPU metrics contain no contiguous core rows".into());
    }

    let mut counters = Vec::new();
    if require_counters {
        for core in 0..run_ns.len() {
            let mut values = [0_u64; 7];
            for (counter, name) in GUEST_COUNTER_NAMES.iter().enumerate() {
                let line = lines
                    .get(cursor)
                    .ok_or_else(|| format!("vCPU metrics omit core {core} counter `{name}`"))?;
                let expected = format!("core.{core:04}.{name}=");
                let value = line.strip_prefix(&expected).ok_or_else(|| {
                    format!("vCPU metrics expected `{expected}<u64>`, got `{line}`")
                })?;
                if value.contains('=') {
                    return Err(format!("malformed guest counter `{line}`"));
                }
                values[counter] = evidence::canonical_u64("guest PMU counter", value)?;
                cursor += 1;
            }
            counters.push(values);
        }
    }

    let profile = lines
        .get(cursor)
        .and_then(|line| line.strip_prefix("host_profile="))
        .ok_or("vCPU metrics omit the canonical host_profile row")?;
    cursor += 1;
    let translation = lines
        .get(cursor)
        .and_then(|line| line.strip_prefix("translation_profile="))
        .ok_or("vCPU metrics omit the canonical translation_profile row")?;
    cursor += 1;
    if cursor != lines.len() {
        return Err(format!(
            "vCPU metrics contain an unexpected trailing row `{}`",
            lines[cursor]
        ));
    }
    if profile != expected_profile || translation != expected_translation {
        return Err(format!(
            "vCPU metrics profile/translation is `{profile}`/`{translation}`, expected `{expected_profile}`/`{expected_translation}`"
        ));
    }
    Ok(VcpuMetrics { run_ns, counters })
}

struct ChoiceStats {
    choices: usize,
    exits: u64,
    exit_code: u64,
    transcript_digest: String,
    frame_digests: Vec<String>,
}

fn parse_choice_record(text: &str) -> Result<ChoiceStats, String> {
    if !text.ends_with('\n') || text.lines().next() != Some("ChoiceLog v1") {
        return Err("choice record is not canonical ChoiceLog v1".into());
    }
    let mut exits = None;
    let mut exit_code = None;
    let mut transcript_digest = None;
    let mut declared = None;
    let mut choices = 0_usize;
    let mut frames = Vec::new();
    let mut seen = std::collections::BTreeSet::new();
    for line in text.lines().skip(1) {
        if let Some(value) = line.strip_prefix("choice_count=") {
            if !seen.insert("choice_count") {
                return Err("choice record repeats choice_count".into());
            }
            declared = Some(evidence::canonical_u64("choice_count", value)? as usize);
        } else if let Some(value) = line.strip_prefix("exits=") {
            if !seen.insert("exits") {
                return Err("choice record repeats exits".into());
            }
            exits = Some(evidence::canonical_u64("exits", value)?);
        } else if let Some(value) = line.strip_prefix("exit_code=") {
            if !seen.insert("exit_code") {
                return Err("choice record repeats exit_code".into());
            }
            exit_code = Some(evidence::canonical_u64("exit_code", value)?);
        } else if let Some(value) = line.strip_prefix("transcript_digest=") {
            if !seen.insert("transcript_digest") {
                return Err("choice record repeats transcript_digest".into());
            }
            evidence::require_sha256("transcript_digest", value)?;
            transcript_digest = Some(value.to_string());
        } else if let Some((raw_index, value)) = line
            .strip_prefix("choice[")
            .and_then(|rest| rest.split_once("]="))
        {
            let index = evidence::canonical_u64("choice index", raw_index)? as usize;
            if index != choices {
                return Err(format!(
                    "choice record expected choice[{choices}], got choice[{index}]"
                ));
            }
            choices += 1;
            if let Some(fields) = value.strip_prefix("FrameOutputV1 ") {
                let digest = fields
                    .split_whitespace()
                    .find_map(|field| field.strip_prefix("visible="))
                    .ok_or("FrameOutputV1 choice lacks visible digest")?;
                evidence::require_sha256("FrameOutputV1 visible digest", digest)?;
                frames.push(digest.to_string());
            }
        } else {
            return Err(format!("choice record has unknown row `{line}`"));
        }
    }
    if seen.len() != 4 || declared != Some(choices) {
        return Err("choice_count does not match indexed choice rows".into());
    }
    Ok(ChoiceStats {
        choices,
        exits: exits.ok_or("choice record lacks exits")?,
        exit_code: exit_code.ok_or("choice record lacks exit_code")?,
        transcript_digest: transcript_digest.ok_or("choice record lacks transcript_digest")?,
        frame_digests: frames,
    })
}

fn replay_matrix_digest(
    outputs: &[(&str, PathBuf)],
    expected_transcript: &[u8],
) -> Result<String, String> {
    let mut bytes = b"wrela-cross-replay-matrix-v1\0".to_vec();
    for (label, path) in outputs {
        let actual = std::fs::read(path)
            .map_err(|error| format!("read replay output {}: {error}", path.display()))?;
        if actual != expected_transcript {
            return Err(format!(
                "backend conformance: `{label}` replay output differs"
            ));
        }
        bytes.extend_from_slice(label.as_bytes());
        bytes.push(0);
        bytes.extend_from_slice(&actual);
        bytes.push(0xff);
    }
    Ok(wrela_machine::sha256::sha256_hex(&bytes))
}

fn parse_perf(text: &str) -> Result<BTreeMap<String, u64>, String> {
    let names = BTreeMap::from([
        ("branch-misses", "br_mis_pred"),
        ("cpu_cycles", "cpu_cycles"),
        ("instructions", "inst_retired"),
        ("l1d_cache_refill", "l1d_cache_refill"),
        ("l2d_cache_refill", "l2d_cache_refill"),
        ("stall_backend", "stall_backend"),
        ("stall_frontend", "stall_frontend"),
    ]);
    let mut out = BTreeMap::new();
    for line in text
        .lines()
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
    {
        let columns = line.split(',').collect::<Vec<_>>();
        if columns.len() < 5 || columns[4] != "100.00" {
            return Err(format!("perf row is missing or multiplexed: `{line}`"));
        }
        let Some(&canonical) = names.get(columns[2]) else {
            return Err(format!("perf row has unexpected event `{}`", columns[2]));
        };
        let value = evidence::canonical_u64("perf counter", columns[0])?;
        if out.insert(canonical.to_string(), value).is_some() {
            return Err(format!("perf output repeats `{canonical}`"));
        }
    }
    if out.len() != names.len() {
        return Err("perf output lacks one or more required counter rows".into());
    }
    Ok(out)
}

pub(crate) fn vmm_source_digest() -> Result<String, String> {
    let root = crate::root();
    let mut files = Vec::new();
    for relative in [
        "Cargo.lock",
        "Cargo.toml",
        "crates/wrela-machine/Cargo.toml",
        "crates/wrela-machine/src",
        "crates/wrela-vmm/Cargo.toml",
        "crates/wrela-vmm/src",
    ] {
        collect_source_files(&root, &root.join(relative), &mut files)?;
    }
    files.sort();
    let mut bytes = Vec::new();
    for path in files {
        bytes.extend_from_slice(
            path.strip_prefix(&root)
                .unwrap()
                .to_string_lossy()
                .as_bytes(),
        );
        bytes.push(0);
        bytes.extend_from_slice(
            &std::fs::read(&path).map_err(|error| format!("read {}: {error}", path.display()))?,
        );
        bytes.push(0xff);
    }
    Ok(wrela_machine::sha256::sha256_hex(&bytes))
}

pub(crate) fn backend_conformance_source_digest() -> Result<String, String> {
    let root = crate::root();
    let mut bytes = b"wrela-backend-conformance-source-v1\0".to_vec();
    bytes.extend_from_slice(crate::proxy_validation::proxy_rules_digest(&root)?.as_bytes());
    bytes.push(0);
    bytes.extend_from_slice(vmm_source_digest()?.as_bytes());
    bytes.push(0xff);
    let mut files = Vec::new();
    for relative in ["crates/xtask/src/evidence.rs", "crates/xtask/src/pi.rs"] {
        collect_source_files(&root, &root.join(relative), &mut files)?;
    }
    for case in [
        "boot-instant-monotonic",
        "boot-entropy",
        "boot-pixels-plane-one-core",
    ] {
        collect_source_files(&root, &root.join("tests/golden").join(case), &mut files)?;
    }
    files.retain(|path| {
        !path.strip_prefix(&root).is_ok_and(|relative| {
            relative
                .components()
                .any(|part| part.as_os_str() == "expected")
        })
    });
    files.sort();
    files.dedup();
    for path in files {
        bytes.extend_from_slice(
            path.strip_prefix(&root)
                .expect("collected repository source")
                .to_string_lossy()
                .as_bytes(),
        );
        bytes.push(0);
        bytes.extend_from_slice(
            &std::fs::read(&path).map_err(|error| format!("read {}: {error}", path.display()))?,
        );
        bytes.push(0xff);
    }
    Ok(wrela_machine::sha256::sha256_hex(&bytes))
}

pub(crate) fn verify_backend_conformance() -> Result<(), String> {
    let root = crate::root();
    let directory = root.join("bench/results/backend-conformance");
    let source_contract = backend_conformance_source_digest()?;
    let identity = evidence::parse(
        &std::fs::read_to_string(root.join("bench/results/rasputin-host-identity-v1.txt"))
            .map_err(|error| format!("read checked host identity: {error}"))?,
        evidence::HOST_IDENTITY,
    )?;
    let profile = evidence::parse(
        &std::fs::read_to_string(root.join("bench/results/rasputin-product-host-profile-v1.txt"))
            .map_err(|error| format!("read checked product host profile: {error}"))?,
        evidence::HOST_PROFILE,
    )?;
    if identity.fields["acceptance_verdict"] != "conforming"
        || profile.fields["acceptance_verdict"] != "conforming"
        || profile.fields["hardening_mode"] != "product"
    {
        return Err("backend conformance: checked KVM host evidence is nonconforming".into());
    }
    let identity_digest = identity.digest_hex()?;
    let profile_digest = profile.digest_hex()?;
    let cases = [
        "boot-entropy",
        "boot-instant-monotonic",
        "boot-pixels-plane-one-core",
    ];
    let mut expected_files = std::collections::BTreeSet::new();
    for case in cases {
        expected_files.insert(format!("{case}.stdout.bin"));
        let transcript_bytes = std::fs::read(directory.join(format!("{case}.stdout.bin")))
            .map_err(|error| format!("read `{case}` checked transcript: {error}"))?;
        let replay_outputs = [
            (
                "hvf-from-hvf",
                directory.join(format!("{case}-hvf-from-hvf.stdout.bin")),
            ),
            (
                "kvm-from-hvf",
                directory.join(format!("{case}-kvm-from-hvf.stdout.bin")),
            ),
            (
                "hvf-from-kvm",
                directory.join(format!("{case}-hvf-from-kvm.stdout.bin")),
            ),
            (
                "kvm-from-kvm",
                directory.join(format!("{case}-kvm-from-kvm.stdout.bin")),
            ),
        ];
        for (_, path) in &replay_outputs {
            expected_files.insert(
                path.file_name()
                    .and_then(|name| name.to_str())
                    .ok_or("backend conformance replay filename is not UTF-8")?
                    .to_string(),
            );
        }
        let replay_matrix = replay_matrix_digest(&replay_outputs, &transcript_bytes)?;
        let transcript = directory.join(format!("{case}.stdout.bin"));
        for backend in ["hvf", "kvm"] {
            expected_files.insert(format!("{case}-{backend}.txt"));
            expected_files.insert(format!("{case}-{backend}.record.txt"));
            let record_path = directory.join(format!("{case}-{backend}.record.txt"));
            let evidence_path = directory.join(format!("{case}-{backend}.txt"));
            let record = evidence::parse(
                &std::fs::read_to_string(&evidence_path).map_err(|error| {
                    format!(
                        "read conformance record {}: {error}",
                        evidence_path.display()
                    )
                })?,
                evidence::BACKEND_CONFORMANCE,
            )?;
            if record.fields["case"] != case
                || record.fields["backend"] != backend
                || record.fields["source_contract_sha256"] != source_contract
                || record.fields["cross_replay_matrix_sha256"] != replay_matrix
                || record.fields["kvm_host_identity_sha256"] != identity_digest
                || record.fields["kvm_host_profile_sha256"] != profile_digest
                || record.fields["record_sha256"] != digest_file(&record_path)?
                || record.fields["transcript_sha256"] != digest_file(&transcript)?
            {
                return Err(format!(
                    "backend conformance: `{case}` `{backend}` is stale or misbound"
                ));
            }
            let stats = parse_choice_record(
                &std::fs::read_to_string(&record_path)
                    .map_err(|error| format!("read {}: {error}", record_path.display()))?,
            )?;
            if stats.transcript_digest != record.fields["transcript_sha256"]
                || stats.choices.to_string() != record.fields["choice_count"]
                || stats.exits.to_string() != record.fields["exit_count"]
                || stats.exit_code.to_string() != record.fields["guest_exit_code"]
                || stats.frame_digests.join(",") != record.fields["frame_digest_sequence"]
            {
                return Err(format!(
                    "backend conformance: `{case}` `{backend}` summary is not derived"
                ));
            }
        }
        let hvf = evidence::parse(
            &std::fs::read_to_string(directory.join(format!("{case}-hvf.txt")))
                .map_err(|error| format!("read `{case}` HVF conformance: {error}"))?,
            evidence::BACKEND_CONFORMANCE,
        )?;
        let kvm = evidence::parse(
            &std::fs::read_to_string(directory.join(format!("{case}-kvm.txt")))
                .map_err(|error| format!("read `{case}` KVM conformance: {error}"))?,
            evidence::BACKEND_CONFORMANCE,
        )?;
        for key in [
            "case",
            "choice_count",
            "cross_replay_matrix_sha256",
            "exit_class",
            "frame_count",
            "frame_digest_sequence",
            "guest_exit_code",
            "image_sha256",
            "kvm_host_identity_sha256",
            "kvm_host_profile_sha256",
            "machine_revision",
            "report_sha256",
            "source_contract_sha256",
            "transcript_sha256",
        ] {
            if hvf.fields[key] != kvm.fields[key] {
                return Err(format!(
                    "backend conformance: `{case}` HVF/KVM pair differs in `{key}`"
                ));
            }
        }
    }
    let actual_files = std::fs::read_dir(&directory)
        .map_err(|error| format!("read {}: {error}", directory.display()))?
        .map(|entry| {
            entry
                .map_err(|error| format!("read conformance directory entry: {error}"))?
                .file_name()
                .into_string()
                .map_err(|_| "backend conformance filename is not UTF-8".to_string())
        })
        .collect::<Result<std::collections::BTreeSet<_>, _>>()?;
    if actual_files != expected_files {
        return Err("backend conformance: checked file set is not exact".into());
    }
    Ok(())
}

fn collect_source_files(root: &Path, path: &Path, out: &mut Vec<PathBuf>) -> Result<(), String> {
    if path.is_file() {
        out.push(path.to_path_buf());
        return Ok(());
    }
    let mut entries = std::fs::read_dir(path)
        .map_err(|error| format!("read {}: {error}", path.display()))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| format!("read {} entry: {error}", path.display()))?;
    entries.sort_by_key(std::fs::DirEntry::file_name);
    for entry in entries {
        let child = entry.path();
        if child.is_dir() || child.extension().is_some_and(|extension| extension == "rs") {
            collect_source_files(root, &child, out)?;
        }
    }
    let _ = root;
    Ok(())
}

fn manifest<'a>(
    action: &'a str,
    fields: impl IntoIterator<Item = (&'a str, &'a str)>,
) -> Result<String, String> {
    let mut map = BTreeMap::new();
    map.insert("action", action);
    for (key, value) in fields {
        if map.insert(key, value).is_some() {
            return Err(format!("manifest repeated `{key}`"));
        }
    }
    let mut out = "format=wrela-lab-manifest-v1\n".to_string();
    for (key, value) in map {
        out.push_str(key);
        out.push('=');
        percent_encode(value.as_bytes(), &mut out);
        out.push('\n');
    }
    Ok(out)
}

fn invoke_agent(host: &str, remote: &str, manifest: &str) -> Result<String, String> {
    invoke_agent_with_command(host, remote, manifest, Command::new("ssh"))
}

fn invoke_agent_with_command(
    host: &str,
    remote: &str,
    manifest: &str,
    mut command: Command,
) -> Result<String, String> {
    validate_host(host)?;
    let Some(digest) = remote.strip_prefix("/var/tmp/wrela-lab/bin/wrela-lab-agent-") else {
        return Err("pi: remote agent path failed content-address validation".into());
    };
    if digest.len() != 64
        || !digest
            .bytes()
            .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
    {
        return Err("pi: remote agent digest failed validation".into());
    }
    let mut child = command
        .args(SSH_POLICY_ARGS)
        .args(["--", host, remote])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("start ssh agent: {e}"))?;
    child
        .stdin
        .take()
        .ok_or("ssh stdin unavailable")?
        .write_all(manifest.as_bytes())
        .map_err(|e| format!("write agent manifest: {e}"))?;
    let output = child
        .wait_with_output()
        .map_err(|e| format!("wait ssh agent: {e}"))?;
    if !output.status.success() {
        return Err(format!(
            "remote lab agent failed:\n{}",
            String::from_utf8_lossy(&output.stderr)
        ));
    }
    String::from_utf8(output.stdout).map_err(|_| "remote lab agent output is not UTF-8".into())
}

fn sftp_batch(host: &str, commands: &[String]) -> Result<(), String> {
    sftp_batch_with_command(host, commands, Command::new("sftp"))
}

fn remote_file_exists(host: &str, remote: &str) -> Result<bool, String> {
    validate_host(host)?;
    Ok(sftp_batch(host, &[remote_file_probe_command(remote)?]).is_ok())
}

fn remote_file_probe_command(remote: &str) -> Result<String, String> {
    if !remote.starts_with(&format!("{REMOTE_BIN}/")) {
        return Err("pi: cache probe path is outside the remote binary directory".into());
    }
    // `stat` is an SFTP protocol operation, but it is not a command accepted
    // by OpenSSH's batch-mode client. Listing one exact quoted path has the
    // desired success/missing exit status without transferring its contents.
    Ok(format!("ls -l {}", quote_sftp(remote)?))
}

fn sftp_batch_with_command(
    host: &str,
    commands: &[String],
    mut command: Command,
) -> Result<(), String> {
    validate_host(host)?;
    let mut child = command
        .args(SSH_POLICY_ARGS)
        .args(["-q", "-b", "-", "--", host])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("start sftp: {e}"))?;
    let mut stdin = child.stdin.take().ok_or("sftp stdin unavailable")?;
    for command in commands {
        stdin
            .write_all(command.as_bytes())
            .and_then(|_| stdin.write_all(b"\n"))
            .map_err(|e| format!("write sftp batch: {e}"))?;
    }
    drop(stdin);
    let output = child
        .wait_with_output()
        .map_err(|e| format!("wait sftp: {e}"))?;
    if output.status.success() {
        Ok(())
    } else {
        Err(format!(
            "sftp failed:\n{}",
            String::from_utf8_lossy(&output.stderr)
        ))
    }
}

fn retrieve_run(host: &str, remote: &str, local: &Path, perf: bool) -> Result<(), String> {
    let mut commands = vec![
        format!(
            "get {} {}",
            quote_sftp(&format!("{remote}/result.txt"))?,
            quote_sftp_path(&local.join("result.txt"))?
        ),
        format!(
            "get {} {}",
            quote_sftp(&format!("{remote}/stdout.bin"))?,
            quote_sftp_path(&local.join("stdout.bin"))?
        ),
        format!(
            "get {} {}",
            quote_sftp(&format!("{remote}/stderr.txt"))?,
            quote_sftp_path(&local.join("stderr.txt"))?
        ),
        format!(
            "get {} {}",
            quote_sftp(&format!("{remote}/record.txt"))?,
            quote_sftp_path(&local.join("record.txt"))?
        ),
        format!(
            "get {} {}",
            quote_sftp(&format!("{remote}/metrics.txt"))?,
            quote_sftp_path(&local.join("metrics.txt"))?
        ),
    ];
    if perf {
        commands.push(format!(
            "get {} {}",
            quote_sftp(&format!("{remote}/perf.csv"))?,
            quote_sftp_path(&local.join("perf.csv"))?
        ));
    }
    sftp_batch(host, &commands)
}

fn quote_sftp_path(path: &Path) -> Result<String, String> {
    quote_sftp(
        path.to_str()
            .ok_or_else(|| format!("non-UTF-8 path {}", path.display()))?,
    )
}

fn quote_sftp(raw: &str) -> Result<String, String> {
    if raw.contains(['\n', '\r', '\0']) {
        return Err("SFTP path contains a control byte".into());
    }
    let mut out = String::from("\"");
    for ch in raw.chars() {
        if matches!(ch, '\\' | '"') {
            out.push('\\');
        }
        out.push(ch);
    }
    out.push('"');
    Ok(out)
}

fn percent_encode(bytes: &[u8], out: &mut String) {
    const HEX: &[u8; 16] = b"0123456789ABCDEF";
    for &byte in bytes {
        if byte.is_ascii_alphanumeric()
            || matches!(byte, b' ' | b'.' | b'_' | b'-' | b'/' | b':' | b',')
        {
            out.push(byte as char);
        } else {
            out.push('%');
            out.push(HEX[(byte >> 4) as usize] as char);
            out.push(HEX[(byte & 15) as usize] as char);
        }
    }
}

fn digest_file(path: &Path) -> Result<String, String> {
    std::fs::read(path)
        .map(|bytes| wrela_machine::sha256::sha256_hex(&bytes))
        .map_err(|e| format!("read {}: {e}", path.display()))
}

fn artifact_dir(host: &str, name: &str) -> Result<PathBuf, String> {
    validate_host(host)?;
    validate_case(name)?;
    Ok(crate::root().join("target/wrela-lab").join(host).join(name))
}

fn run_nonce() -> Result<String, String> {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|e| e.to_string())?
        .as_nanos();
    Ok(format!("{:016x}", now as u64))
}

fn find_build_artifacts(dir: &Path) -> Result<(PathBuf, PathBuf), String> {
    let mut reports = Vec::new();
    let mut images = Vec::new();
    for entry in std::fs::read_dir(dir).map_err(|e| format!("read {}: {e}", dir.display()))? {
        let path = entry.map_err(|e| e.to_string())?.path();
        let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
        match path.extension().and_then(|e| e.to_str()) {
            Some("report") | Some("txt") if name.contains("report") => reports.push(path),
            Some("img") | Some("bin") => images.push(path),
            _ => {}
        }
    }
    if reports.len() != 1 || images.len() != 1 {
        return Err(format!(
            "build output must contain exactly one report and image; reports={reports:?} images={images:?}"
        ));
    }
    Ok((reports.remove(0), images.remove(0)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    fn fake_transport(script: &str) -> (PathBuf, PathBuf) {
        use std::os::unix::fs::PermissionsExt;
        use std::sync::atomic::{AtomicU64, Ordering};

        static NEXT: AtomicU64 = AtomicU64::new(0);
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!(
            "wrela-fake-ssh-{}-{now:032x}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir(&root).unwrap();
        let executable = root.join("transport");
        std::fs::write(&executable, script).unwrap();
        let mut permissions = std::fs::metadata(&executable).unwrap().permissions();
        permissions.set_mode(0o700);
        std::fs::set_permissions(&executable, permissions).unwrap();
        (root, executable)
    }

    #[test]
    fn fallback_lock_is_closed_over_the_curated_production_workspace() {
        let lock = std::fs::read_to_string(crate::root().join("Cargo.lock")).unwrap();
        let curated = curated_fallback_lock(&lock).unwrap();
        assert!(curated.contains("name = \"wrela-vmm\""));
        assert!(curated.contains("name = \"kvm-ioctls\""));
        assert!(!curated.contains("wrela-compiler"));
        assert!(!curated.contains("name = \"xtask\""));
        assert!(!curated.contains("name = \"toml\""));

        let root = std::env::temp_dir().join(format!(
            "wrela-fallback-lock-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(root.join("crates/wrela-machine/src")).unwrap();
        std::fs::create_dir_all(root.join("crates/wrela-vmm/src")).unwrap();
        std::fs::write(
            root.join("Cargo.toml"),
            "[workspace]\nresolver = \"2\"\nmembers = [\"crates/wrela-machine\", \"crates/wrela-vmm\"]\n[workspace.package]\nversion = \"0.1.0\"\nedition = \"2024\"\n",
        )
        .unwrap();
        std::fs::write(root.join("Cargo.lock"), curated).unwrap();
        std::fs::write(
            root.join("crates/wrela-machine/Cargo.toml"),
            "[package]\nname = \"wrela-machine\"\nversion.workspace = true\nedition.workspace = true\n",
        )
        .unwrap();
        std::fs::write(root.join("crates/wrela-machine/src/lib.rs"), "").unwrap();
        std::fs::write(
            root.join("crates/wrela-vmm/Cargo.toml"),
            FALLBACK_VMM_MANIFEST,
        )
        .unwrap();
        std::fs::write(root.join("crates/wrela-vmm/src/lib.rs"), "").unwrap();
        let output = Command::new("cargo")
            .current_dir(&root)
            .args([
                "metadata",
                "--locked",
                "--offline",
                "--format-version=1",
                "--no-deps",
            ])
            .output()
            .unwrap();
        std::fs::remove_dir_all(root).unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    #[test]
    fn cross_build_cache_key_is_sensitive_to_every_declared_input() {
        let fields = [
            ("dependency_source", "a"),
            ("features", "b"),
            ("hardening", "c"),
            ("linker", "d"),
            ("lock", "e"),
            ("profile", "f"),
            ("rustc", "g"),
            ("source", "h"),
            ("target", "i"),
        ];
        let baseline = cross_build_cache_key(&fields);
        for index in 0..fields.len() {
            let mut changed = fields;
            changed[index].1 = "changed";
            assert_ne!(cross_build_cache_key(&changed), baseline);
        }
    }

    #[test]
    fn second_content_addressed_prepare_sends_no_vmm_payload() {
        use std::cell::Cell;

        let digest = "a".repeat(64);
        let name = format!("wrela-vmm-{digest}");
        let local = Path::new("/local/wrela-vmm");
        let present = Cell::new(false);
        let uploads = Cell::new(0_u32);
        let response = || {
            Ok(format!(
                "format=wrela-lab-binary-cache-v1\nbinary={name}\nbinary_sha256={digest}\nverdict={}\n",
                if present.get() { "hit" } else { "miss" }
            ))
        };
        let upload = |commands: &[String]| {
            uploads.set(uploads.get() + 1);
            assert!(commands.iter().any(|command| command.starts_with("put ")));
            present.set(true);
            Ok(())
        };

        ensure_remote_binary(&name, &digest, local, response, upload).unwrap();
        assert_eq!(uploads.get(), 1);
        ensure_remote_binary(&name, &digest, local, response, upload).unwrap();
        assert_eq!(uploads.get(), 1, "cache hit must not upload the VMM again");
    }

    #[test]
    fn host_and_case_arguments_cannot_be_options_or_shell_fragments() {
        for host in ["-oProxyCommand=x", "rasputin;id", "ras putin", ""] {
            assert!(validate_host(host).is_err());
        }
        assert!(validate_host("rasputin.local").is_ok());
        assert!(validate_host("wrela@rasputin.local").is_ok());
        assert!(validate_host("root@@rasputin.local").is_err());
        assert!(validate_host("-root@rasputin.local").is_err());
        for case in ["--help", "../boot-hello", "boot hello", "boot;id"] {
            assert!(validate_case(case).is_err());
        }
        assert!(cleanup("rasputin.local", "run-*").is_err());
        assert!(cleanup("rasputin.local", "RUN-0123456789abcdef").is_err());
    }

    #[test]
    fn evidence_workloads_resolve_input_and_root_marker_fixtures() {
        assert!(
            case_target("boot-actor-smoke")
                .unwrap()
                .ends_with("tests/golden/boot-actor-smoke/input.wr")
        );
        assert!(
            case_target("boot-pixels-plane-one-core")
                .unwrap()
                .ends_with(
                    "tests/golden/boot-pixels-plane-one-core/src/examples/boot_pixels_plane_one_core.wr"
                )
        );
        let sustained = case_target("boot-pixels-plane-three-core").unwrap();
        assert!(sustained.ends_with(
            "bench/proxy-fixtures/boot-pixels-plane-three-core/src/examples/boot_pixels_plane_three_core.wr"
        ));
        assert!(
            std::fs::read_to_string(sustained)
                .unwrap()
                .contains("while index < 3072"),
            "the physical-only resolver must retain the full sustained workload"
        );
    }

    #[test]
    fn manifests_are_sorted_and_values_are_encoded() {
        let manifest = manifest(
            "run",
            [
                ("run_dir", "/var/tmp/wrela-lab/runs/run-0000000000000000"),
                ("display", "headless"),
            ],
        )
        .unwrap();
        assert_eq!(
            manifest,
            "format=wrela-lab-manifest-v1\naction=run\ndisplay=headless\nrun_dir=/var/tmp/wrela-lab/runs/run-0000000000000000\n"
        );
    }

    #[test]
    fn sftp_paths_quote_spaces_and_reject_lines() {
        assert_eq!(quote_sftp("a b").unwrap(), "\"a b\"");
        assert!(quote_sftp("a\nput evil").is_err());
    }

    #[test]
    fn remote_cache_probe_uses_an_openssh_sftp_command() {
        let remote = format!("{REMOTE_BIN}/wrela-vmm-{}", "a".repeat(64));
        assert_eq!(
            remote_file_probe_command(&remote).unwrap(),
            format!("ls -l \"{remote}\"")
        );
        assert!(remote_file_probe_command("/tmp/wrela-vmm-unsafe").is_err());
    }

    #[test]
    fn remote_transport_policy_is_batch_pinned_and_has_no_forwarding_or_tty() {
        assert!(SSH_POLICY_ARGS.contains(&"-oBatchMode=yes"));
        assert!(SSH_POLICY_ARGS.contains(&"-oClearAllForwardings=yes"));
        assert!(SSH_POLICY_ARGS.contains(&"-oForwardAgent=no"));
        assert!(SSH_POLICY_ARGS.contains(&"-oForwardX11=no"));
        assert!(SSH_POLICY_ARGS.contains(&"-oRequestTTY=no"));
        assert!(SSH_POLICY_ARGS.contains(&"-oStrictHostKeyChecking=yes"));
        assert!(!SSH_POLICY_ARGS.iter().any(|arg| arg.contains("accept-new")));
    }

    #[cfg(unix)]
    #[test]
    fn fake_ssh_process_receives_one_literal_command_and_manifest_on_stdin() {
        let (root, executable) = fake_transport(
            "#!/bin/sh\nprintf '%s\\n' \"$@\" > \"$WRELA_FAKE_ARGS\"\ncat > \"$WRELA_FAKE_STDIN\"\nprintf 'format=wrela-lab-response-v1\\nstatus=ok\\n'\n",
        );
        let args = root.join("args");
        let stdin = root.join("stdin");
        let mut command = Command::new(&executable);
        command
            .env("WRELA_FAKE_ARGS", &args)
            .env("WRELA_FAKE_STDIN", &stdin);
        let digest = "a".repeat(64);
        let remote = format!("/var/tmp/wrela-lab/bin/wrela-lab-agent-{digest}");
        let input = "format=wrela-lab-manifest-v1\naction=probe\n";
        let response =
            invoke_agent_with_command("wrela@rasputin.local", &remote, input, command).unwrap();
        assert_eq!(response, "format=wrela-lab-response-v1\nstatus=ok\n");
        assert_eq!(std::fs::read_to_string(stdin).unwrap(), input);
        let actual = std::fs::read_to_string(args).unwrap();
        assert!(actual.ends_with(&format!("--\nwrela@rasputin.local\n{remote}\n")));
        assert_eq!(actual.matches(&remote).count(), 1);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn fake_transport_failures_are_reported_without_accepting_partial_success() {
        let (ssh_root, ssh) =
            fake_transport("#!/bin/sh\ncat >/dev/null\nprintf refused >&2\nexit 23\n");
        let remote = format!("/var/tmp/wrela-lab/bin/wrela-lab-agent-{}", "b".repeat(64));
        let error = invoke_agent_with_command(
            "rasputin.local",
            &remote,
            "format=wrela-lab-manifest-v1\naction=probe\n",
            Command::new(&ssh),
        )
        .unwrap_err();
        assert!(error.contains("refused"));
        std::fs::remove_dir_all(ssh_root).unwrap();

        let (sftp_root, sftp) =
            fake_transport("#!/bin/sh\ncat >/dev/null\nprintf interrupted >&2\nexit 24\n");
        let error = sftp_batch_with_command(
            "rasputin.local",
            &["put \"local\" \"remote\"".to_string()],
            Command::new(&sftp),
        )
        .unwrap_err();
        assert!(error.contains("interrupted"));
        std::fs::remove_dir_all(sftp_root).unwrap();
    }

    #[test]
    fn choice_parser_is_exact_and_extracts_frame_output_digests() {
        let d = "a".repeat(64);
        let text = format!(
            "ChoiceLog v1\nchoice_count=1\nchoice[0]=FrameOutputV1 renderer=0 visible={d}\ntranscript_digest={d}\nexit_code=0\nexits=2\n"
        );
        let parsed = parse_choice_record(&text).unwrap();
        assert_eq!(parsed.frame_digests, vec![d]);
        assert!(parse_choice_record(&text.replace("choice[0]", "choice[1]")).is_err());
        assert!(parse_choice_record(&(text.clone() + "unknown=1\n")).is_err());
        assert!(parse_choice_record(&text.replace("exits=2", "exits=2\nexits=2")).is_err());
    }

    #[test]
    fn guest_pmu_metrics_require_every_counter_for_contiguous_vcpus() {
        let text = "format=wrela-vcpu-run-metrics-v1\
\ncore.0000.run_ns=10\
\ncore.0001.run_ns=20\
\ncore.0000.br_mis_pred=1\
\ncore.0000.cpu_cycles=2\
\ncore.0000.inst_retired=3\
\ncore.0000.l1d_cache_refill=4\
\ncore.0000.l2d_cache_refill=5\
\ncore.0000.stall_backend=6\
\ncore.0000.stall_frontend=7\
\ncore.0001.br_mis_pred=8\
\ncore.0001.cpu_cycles=9\
\ncore.0001.inst_retired=10\
\ncore.0001.l1d_cache_refill=11\
\ncore.0001.l2d_cache_refill=12\
\ncore.0001.stall_backend=13\
\ncore.0001.stall_frontend=14\
\nhost_profile=product\
\ntranslation_profile=sealed-stage1\n";
        let parsed = parse_guest_metrics(text, "product", "sealed-stage1").unwrap();
        assert_eq!(parsed.run_ns, vec![10, 20]);
        assert_eq!(parsed.counters[0], [1, 2, 3, 4, 5, 6, 7]);
        assert_eq!(parsed.counters[1], [8, 9, 10, 11, 12, 13, 14]);
        assert!(
            parse_guest_metrics(
                &text.replace("core.0001.stall_frontend=14\n", ""),
                "product",
                "sealed-stage1"
            )
            .is_err()
        );
        assert!(
            parse_guest_metrics(
                &text.replace("core.0001.run_ns=20", "core.0002.run_ns=20"),
                "product",
                "sealed-stage1"
            )
            .is_err()
        );
        assert!(
            parse_guest_metrics(
                &(text.to_string() + "unknown=1\n"),
                "product",
                "sealed-stage1"
            )
            .is_err()
        );
    }

    #[test]
    fn validation_fragments_cannot_cross_proxy_rule_revisions() {
        let current = "a".repeat(64);
        let mut fragment = evidence::Record::new(PROXY_FRAGMENT_FORMAT).unwrap();
        fragment
            .insert("proxy_rules_sha256", current.clone())
            .unwrap();
        require_current_proxy_rules(&fragment, &current).unwrap();

        fragment
            .fields
            .insert("proxy_rules_sha256".into(), "b".repeat(64));
        let error = require_current_proxy_rules(&fragment, &current).unwrap_err();
        assert!(error.contains("stale for the current proxy rules"));
    }
}
