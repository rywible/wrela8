//! Narrow remote helper for the Rasputin lane.
//!
//! It accepts one canonical manifest on stdin. No field becomes a shell
//! fragment; subprocesses use fixed executables and individually validated
//! arguments.

use std::collections::BTreeMap;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode, Stdio};
use std::time::{Duration, Instant};

const FORMAT: &str = "wrela-lab-manifest-v1";
const MAX_MANIFEST: usize = 64 * 1024;
const LAB_ROOT: &str = "/var/tmp/wrela-lab";

fn main() -> ExitCode {
    match run() {
        Ok(output) => {
            print!("{output}");
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("wrela-lab-agent: {error}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<String, String> {
    let mut bytes = Vec::new();
    std::io::stdin()
        .take((MAX_MANIFEST + 1) as u64)
        .read_to_end(&mut bytes)
        .map_err(|error| format!("read manifest: {error}"))?;
    if bytes.len() > MAX_MANIFEST {
        return Err("manifest exceeds 65536 bytes".into());
    }
    let text = String::from_utf8(bytes).map_err(|_| "manifest is not UTF-8")?;
    let fields = parse_manifest(&text)?;
    match fields["action"].as_str() {
        "probe-identity" => probe_identity(),
        "probe-profile" => probe_profile(&fields),
        "probe-environment" => probe_environment(),
        "probe-binary" => probe_binary(&fields),
        "run" => run_artifact(&fields),
        "sustain" => sustain_artifact(&fields),
        "remote-build" => remote_build(&fields),
        "cleanup" => cleanup_run(&fields),
        other => Err(format!("unknown action `{other}`")),
    }
}

fn probe_binary(fields: &BTreeMap<String, String>) -> Result<String, String> {
    require_exact(fields, &["action", "binary", "binary_sha256"])?;
    let path = safe_cached_binary(&fields["binary"], &fields["binary_sha256"])?;
    let verdict = if path.exists() {
        if digest_file(&path)? != fields["binary_sha256"] {
            return Err("content-addressed binary exists with the wrong digest".into());
        }
        "hit"
    } else {
        "miss"
    };
    encode_record(
        "wrela-lab-binary-cache-v1",
        &BTreeMap::from([
            ("binary", fields["binary"].clone()),
            ("binary_sha256", fields["binary_sha256"].clone()),
            ("verdict", verdict.into()),
        ]),
    )
}

fn cleanup_run(fields: &BTreeMap<String, String>) -> Result<String, String> {
    require_exact(fields, &["action", "run_dir"])?;
    let run_dir = safe_run_dir(&fields["run_dir"])?;
    std::fs::remove_dir_all(&run_dir)
        .map_err(|error| format!("remove retained run {}: {error}", run_dir.display()))?;
    let mut record = BTreeMap::new();
    record.insert("run_dir", fields["run_dir"].clone());
    record.insert("verdict", "removed".into());
    encode_record("wrela-lab-cleanup-v1", &record)
}

fn sustain_artifact(fields: &BTreeMap<String, String>) -> Result<String, String> {
    let mut run_fields = fields.clone();
    let seconds = run_fields
        .remove("duration_seconds")
        .ok_or("sustain manifest lacks duration_seconds")?
        .parse::<u64>()
        .map_err(|_| "duration_seconds is not canonical u64")?;
    if !(120..=600).contains(&seconds) {
        return Err("duration_seconds must be in 120..=600".into());
    }
    run_fields.insert("action".into(), "run".into());
    let result = run_artifact(&run_fields)?;
    let parsed = parse_record(&result, "wrela-lab-run-result-v1")?;
    if parsed["exit_code"] != "0" {
        return Err("sustained VMM process exited nonzero".into());
    }
    let duration = parsed["elapsed_ns"]
        .parse::<u64>()
        .map_err(|_| "sustained elapsed_ns is not canonical u64")?;
    if duration < Duration::from_secs(seconds).as_nanos() as u64 {
        return Err("single sustained VMM process ended before the requested duration".into());
    }
    let run_dir = safe_run_dir(&run_fields["run_dir"])?;
    let record_path = safe_child(&run_dir, &run_fields["record"])?;
    let choices = wrela_vmm::record::RecordFile::parse(
        &std::fs::read_to_string(&record_path)
            .map_err(|error| format!("read sustained record: {error}"))?,
    )?;
    let frames = choices
        .choices
        .iter()
        .filter_map(|choice| match choice {
            wrela_vmm::record::ChoiceEntry::FrameOutputV1(frame) => Some(frame),
            _ => None,
        })
        .collect::<Vec<_>>();
    if frames.len() < 2 {
        return Err("single sustained VMM process presented fewer than two frames".into());
    }
    for (expected, frame) in frames.iter().enumerate() {
        if frame.sequence != expected as u64 || frame.vsync_id != expected as u64 {
            return Err("sustained frame or vblank sequence is not contiguous from zero".into());
        }
    }
    let mut record = BTreeMap::new();
    record.insert("duration_ns", duration.to_string());
    record.insert("frame_count", frames.len().to_string());
    record.insert(
        "frame_digest_sequence",
        frames
            .iter()
            .map(|frame| frame.visible_digest.as_str())
            .collect::<Vec<_>>()
            .join(","),
    );
    record.insert("launch_count", "1".into());
    record.insert("record_sha256", parsed["record_sha256"].clone());
    record.insert("refresh_hz", frames[0].refresh_hz.to_string());
    record.insert("stdout_sha256", parsed["stdout_sha256"].clone());
    record.insert(
        "vsync_sequence",
        frames
            .iter()
            .map(|frame| frame.vsync_id.to_string())
            .collect::<Vec<_>>()
            .join(","),
    );
    encode_record("wrela-lab-sustain-v1", &record)
}

fn remote_build(fields: &BTreeMap<String, String>) -> Result<String, String> {
    require_exact(fields, &["action", "archive", "archive_sha256", "run_dir"])?;
    let run_dir = safe_run_dir(&fields["run_dir"])?;
    let archive = safe_child(&run_dir, &fields["archive"])?;
    if digest_file(&archive)? != fields["archive_sha256"] {
        return Err("remote-build archive digest mismatch".into());
    }
    let listing = Command::new("/usr/bin/tar")
        .args(["-tf"])
        .arg(&archive)
        .output()
        .map_err(|error| format!("list fallback archive: {error}"))?;
    if !listing.status.success() {
        return Err("fallback archive cannot be listed".into());
    }
    let listing =
        String::from_utf8(listing.stdout).map_err(|_| "fallback archive listing is not UTF-8")?;
    for entry in listing.lines() {
        let path = Path::new(entry);
        if path.is_absolute()
            || path.components().any(|component| {
                matches!(
                    component,
                    std::path::Component::ParentDir | std::path::Component::Prefix(_)
                )
            })
        {
            return Err(format!("fallback archive contains unsafe path `{entry}`"));
        }
    }
    let source = run_dir.join("source");
    std::fs::create_dir(&source).map_err(|error| format!("create fallback source: {error}"))?;
    let extract = Command::new("/usr/bin/tar")
        .args(["-xf"])
        .arg(&archive)
        .arg("-C")
        .arg(&source)
        .status()
        .map_err(|error| format!("extract fallback source: {error}"))?;
    if !extract.success() {
        return Err("fallback source extraction failed".into());
    }
    for tool in ["/usr/bin/cargo", "/usr/bin/rustc"] {
        if !Path::new(tool).is_file() {
            return Err(format!(
                "explicit remote build requires provisioned `{tool}`; ambient PATH/rustup state is refused"
            ));
        }
    }
    let cargo_identity = fixed_command("/usr/bin/cargo", &["-V"])?;
    let rustc_identity = fixed_command("/usr/bin/rustc", &["-Vv"])?;
    let cargo_lock = source.join("Cargo.lock");
    let cargo_lock_sha256 = digest_file(&cargo_lock)?;
    let cargo_home = run_dir.join("cargo-home");
    std::fs::create_dir(&cargo_home)
        .map_err(|error| format!("create isolated fallback Cargo home: {error}"))?;
    let output = Command::new("/usr/bin/cargo")
        .current_dir(&source)
        .args([
            "build",
            "--locked",
            "--release",
            "-p",
            "wrela-vmm",
            "--bin",
            "wrela-vmm",
            "--features",
            "native-presentation",
        ])
        .env_clear()
        .env("HOME", &run_dir)
        .env("CARGO_HOME", &cargo_home)
        .env(
            "CARGO_TARGET_AARCH64_UNKNOWN_LINUX_GNU_LINKER",
            "/usr/bin/cc",
        )
        .env("PATH", "/usr/bin:/bin")
        .output()
        .map_err(|error| format!("start explicit remote Cargo build: {error}"))?;
    std::fs::write(run_dir.join("cargo.stdout"), &output.stdout)
        .map_err(|error| format!("retain fallback Cargo stdout: {error}"))?;
    std::fs::write(run_dir.join("cargo.stderr"), &output.stderr)
        .map_err(|error| format!("retain fallback Cargo stderr: {error}"))?;
    if !output.status.success() {
        return Err("explicit remote Cargo build failed; stdout/stderr were retained".into());
    }
    let binary = source.join("target/release/wrela-vmm");
    let binary_sha256 = digest_file(&binary)?;
    let remote = PathBuf::from(format!("{LAB_ROOT}/bin/wrela-vmm-{binary_sha256}"));
    std::fs::copy(&binary, &remote)
        .map_err(|error| format!("cache fallback VMM at {}: {error}", remote.display()))?;
    let mut permissions = std::fs::metadata(&remote)
        .map_err(|error| format!("stat fallback VMM: {error}"))?
        .permissions();
    use std::os::unix::fs::PermissionsExt;
    permissions.set_mode(0o755);
    std::fs::set_permissions(&remote, permissions)
        .map_err(|error| format!("chmod fallback VMM: {error}"))?;
    let agent = std::env::current_exe().map_err(|error| format!("locate lab agent: {error}"))?;
    let mut record = BTreeMap::new();
    record.insert("agent_sha256", digest_file(&agent)?);
    record.insert("archive_sha256", fields["archive_sha256"].clone());
    record.insert("build_features", "native-presentation".into());
    record.insert("build_profile", "release".into());
    record.insert("build_target", "aarch64-unknown-linux-gnu".into());
    record.insert("cargo_identity", cargo_identity);
    record.insert("cargo_lock_sha256", cargo_lock_sha256);
    record.insert("rustc_identity", rustc_identity);
    record.insert("vmm_binary_sha256", binary_sha256);
    record.insert("vmm_remote_path", remote.display().to_string());
    encode_record("wrela-remote-build-v1", &record)
}

fn parse_manifest(text: &str) -> Result<BTreeMap<String, String>, String> {
    if !text.ends_with('\n') || text.contains('\r') || text.contains('\0') {
        return Err("manifest must be LF-terminated and contain no CR/NUL".into());
    }
    let mut lines = text.lines();
    if lines.next() != Some(&format!("format={FORMAT}")) {
        return Err(format!("first line must be `format={FORMAT}`"));
    }
    let mut fields = BTreeMap::new();
    let mut prior = "";
    for line in lines {
        let (key, value) = line
            .split_once('=')
            .ok_or_else(|| format!("malformed line `{line}`"))?;
        if key <= prior
            || key.is_empty()
            || !key
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_')
        {
            return Err(format!("manifest fields are not canonical at `{key}`"));
        }
        prior = key;
        if fields
            .insert(key.to_string(), percent_decode(value)?)
            .is_some()
        {
            return Err(format!("repeated field `{key}`"));
        }
    }
    if !fields.contains_key("action") {
        return Err("manifest is missing `action`".into());
    }
    Ok(fields)
}

fn probe_identity() -> Result<String, String> {
    let mut r = BTreeMap::new();
    let model = read_trim("/proc/device-tree/model").unwrap_or_else(|_| "unknown".into());
    let cpuinfo =
        std::fs::read_to_string("/proc/cpuinfo").map_err(|e| format!("read cpuinfo: {e}"))?;
    let board_ok = model.contains("Raspberry Pi 5");
    r.insert("architecture", std::env::consts::ARCH.into());
    r.insert("board_model", model);
    let cpu_part = cpuinfo_value(&cpuinfo, "CPU part").unwrap_or("unknown");
    r.insert(
        "cpu_model",
        cpuinfo_value(&cpuinfo, "model name")
            .unwrap_or(if cpu_part == "0xd0b" {
                "Cortex-A76"
            } else {
                "unknown"
            })
            .into(),
    );
    r.insert("cpu_part", cpu_part.into());
    r.insert(
        "cpu_revision",
        cpuinfo_value(&cpuinfo, "CPU revision")
            .unwrap_or("unknown")
            .into(),
    );
    r.insert(
        "device_tree_sha256",
        digest_tree(Path::new("/proc/device-tree"))?,
    );
    r.insert("drm_module", module_identity("drm"));
    r.insert(
        "eeprom_config_sha256",
        digest_file(Path::new("/boot/firmware/config.txt"))?,
    );
    r.insert(
        "eeprom_version",
        fixed_command("/usr/bin/vcgencmd", &["bootloader_version"])
            .unwrap_or_else(|_| "unavailable".into()),
    );
    let kernel = fixed_command("/usr/bin/uname", &["-r"])?;
    let config = PathBuf::from(format!("/boot/config-{kernel}"));
    r.insert("kernel_config_sha256", digest_file(&config)?);
    r.insert("kernel_release", kernel);
    r.insert(
        "host_page_size",
        fixed_command("/usr/bin/getconf", &["PAGESIZE"])?,
    );
    #[cfg(all(target_os = "linux", target_arch = "aarch64"))]
    let kvm_capabilities = wrela_vmm::host_capability_probe().map_err(|e| e.to_string())?;
    #[cfg(not(all(target_os = "linux", target_arch = "aarch64")))]
    let kvm_capabilities = "api_version:not-queried,immediate_exit:not-queried,ipa_size:not-queried,isa_baseline:not-queried,one_reg:not-queried,readonly_mem:not-queried,target:not-queried,user_memory:not-queried,vcpu_count:not-queried,writable_id_regs:not-queried".to_string();
    let capabilities_ok = required_kvm_capabilities_ok(&kvm_capabilities)?;
    r.insert("kvm_capabilities", kvm_capabilities);
    r.insert("kvm_module", module_identity("kvm"));
    r.insert("module_set_sha256", module_set_digest()?);
    let pmu_identity = std::fs::read_dir("/sys/bus/event_source/devices")
        .ok()
        .into_iter()
        .flatten()
        .filter_map(Result::ok)
        .filter_map(|entry| entry.file_name().into_string().ok())
        .find(|name| name.starts_with("armv8"))
        .unwrap_or_else(|| "unavailable".into());
    r.insert("pmu_identity", pmu_identity);
    r.insert(
        "acceptance_verdict",
        if board_ok && cpu_part == "0xd0b" && capabilities_ok {
            "conforming"
        } else {
            "refused"
        }
        .into(),
    );
    encode_record("wrela-host-identity-v1", &r)
}

fn required_kvm_capabilities_ok(rows: &str) -> Result<bool, String> {
    let mut seen = BTreeMap::new();
    for row in rows.split(',') {
        let (key, value) = row
            .split_once(':')
            .ok_or_else(|| format!("malformed KVM capability row `{row}`"))?;
        if !matches!(value, "yes" | "no" | "not-queried") || seen.insert(key, value).is_some() {
            return Err(format!("invalid KVM capability row `{row}`"));
        }
    }
    let required = [
        "api_version",
        "immediate_exit",
        "ipa_size",
        "isa_baseline",
        "one_reg",
        "target",
        "user_memory",
        "vcpu_count",
        "writable_id_regs",
    ];
    if seen.len() != required.len() + 1 || !seen.contains_key("readonly_mem") {
        return Err("KVM capability row set is not exact".into());
    }
    Ok(required
        .into_iter()
        .all(|key| seen.get(key) == Some(&"yes")))
}

fn probe_profile(fields: &BTreeMap<String, String>) -> Result<String, String> {
    require_exact(fields, &["action", "hardening_mode"])?;
    let mode = fields["hardening_mode"].as_str();
    if !matches!(mode, "diagnostic" | "development" | "product") {
        return Err("hardening_mode is invalid".into());
    }
    let cmdline = read_trim("/proc/cmdline").unwrap_or_default();
    let mut r = BTreeMap::new();
    let isolated =
        cmdline.contains("isolcpus=domain,managed_irq,1-3") || cmdline.contains("isolcpus=1-3");
    let forced_display = cmdline.contains("video=HDMI-A-1:1280x720@60D");
    r.insert(
        "cpu_isolation",
        if isolated {
            "isolcpus=domain,managed_irq,1-3"
        } else {
            "none"
        }
        .into(),
    );
    r.insert("frequency_policy", policy_values("scaling_max_freq"));
    r.insert("governor", policy_values("scaling_governor"));
    r.insert("hardening_mode", mode.into());
    let mut hardening = b"wrela-rasputin-hardening-v1\0".to_vec();
    hardening.extend_from_slice(cmdline.as_bytes());
    hardening.push(0);
    let status = std::fs::read_to_string("/proc/self/status")
        .map_err(|error| format!("read profile process status: {error}"))?;
    let groups = status
        .lines()
        .find(|line| line.starts_with("Groups:"))
        .ok_or("profile process status lacks Groups")?;
    let uid = status
        .lines()
        .find_map(|line| line.strip_prefix("Uid:")?.split_whitespace().next())
        .ok_or("profile process status lacks Uid")?;
    let passwd = std::fs::read_to_string("/etc/passwd")
        .map_err(|error| format!("read profile passwd: {error}"))?;
    let account = passwd.lines().find_map(|line| {
        let columns = line.split(':').collect::<Vec<_>>();
        (columns.len() >= 4 && columns[2] == uid).then_some((columns[0], columns[3]))
    });
    let dedicated_account = account.is_some_and(|(name, _)| name == "wrela");
    let primary_gid = account.map(|(_, gid)| gid).unwrap_or_default();
    let group_db = std::fs::read_to_string("/etc/group")
        .map_err(|error| format!("read profile groups: {error}"))?;
    let gid_names = group_db
        .lines()
        .filter_map(|line| {
            let columns = line.split(':').collect::<Vec<_>>();
            (columns.len() >= 3).then_some((columns[2], columns[0]))
        })
        .collect::<BTreeMap<_, _>>();
    let mut group_names = groups
        .trim_start_matches("Groups:")
        .split_whitespace()
        .map(|gid| {
            gid_names
                .get(gid)
                .copied()
                .ok_or_else(|| format!("profile group id `{gid}` has no name"))
        })
        .collect::<Result<Vec<_>, _>>()?;
    if !group_names.contains(&"wrela")
        && primary_gid != ""
        && gid_names.get(primary_gid).copied() == Some("wrela")
    {
        group_names.push("wrela");
    }
    group_names.sort_unstable();
    group_names.dedup();
    let groups_exact = group_names == ["kvm", "render", "video", "wrela"];
    hardening.extend_from_slice(groups.as_bytes());
    hardening.push(0);
    let memlock = std::fs::read("/etc/systemd/system/user@.service.d/wrela-memlock.conf")
        .map_err(|error| format!("read Wrela memlock profile: {error}"))?;
    let memlock_text = std::str::from_utf8(&memlock)
        .map_err(|_| "Wrela memlock profile is not UTF-8".to_string())?;
    let memlock_exact =
        memlock_text.lines().collect::<Vec<_>>() == ["[Service]", "LimitMEMLOCK=536870912"];
    hardening.extend_from_slice(&memlock);
    // The lab lane launches an authenticated transient unit rather than the
    // appliance's installed service. Embed the package policy into the agent
    // so the checked host-profile digest still binds the exact reviewed
    // package controls, independent of ambient files on the development Pi.
    let service_text = include_str!("../../../../packaging/linux/wrela-vmm.service");
    let service = service_text.as_bytes();
    let service_exact = linux_service_policy_exact(service_text);
    hardening.extend_from_slice(&service);
    let product_controls = isolated
        && forced_display
        && dedicated_account
        && groups_exact
        && memlock_exact
        && service_exact;
    r.insert(
        "acceptance_verdict",
        if mode == "product" && product_controls {
            "conforming"
        } else {
            "nonconforming"
        }
        .into(),
    );
    r.insert(
        "hardening_config_sha256",
        wrela_machine::sha256::sha256_hex(&hardening),
    );
    r.insert("host_affinity", "0".into());
    r.insert("host_protection_profile", mode.into());
    r.insert("memory_reservation", "prefault-mlock-ordinary-pages".into());
    r.insert("name", format!("rasputin-{mode}-v1"));
    r.insert(
        "provenance",
        "docs/designs/rasputin-target-profile.md".into(),
    );
    r.insert("vcpu_affinity", "1,2,3".into());
    encode_record("wrela-host-profile-v1", &r)
}

fn linux_service_policy_exact(service: &str) -> bool {
    [
        "User=wrela",
        "Group=wrela",
        "NoNewPrivileges=yes",
        "PrivateNetwork=yes",
        "PrivatePIDs=yes",
        "DevicePolicy=closed",
        "DeviceAllow=/dev/kvm rw",
        "DeviceAllow=/dev/dri/card0 rw",
        "DeviceAllow=/dev/dri/card1 rw",
        "DeviceAllow=/dev/dri/renderD128 rw",
        "ProtectSystem=strict",
        "RestrictAddressFamilies=AF_UNIX",
        "SystemCallFilter=@system-service",
        "SystemCallFilter=perf_event_open",
        "SystemCallErrorNumber=EPERM",
        "CapabilityBoundingSet=",
        "AmbientCapabilities=",
        "LimitMEMLOCK=536870912",
        "MemoryMax=768M",
        "MemorySwapMax=0",
        "CPUQuota=400%",
        "CPUAffinity=0 1 2 3",
        "TasksMax=16",
    ]
    .into_iter()
    .all(|required| service.lines().any(|line| line == required))
}

fn probe_environment() -> Result<String, String> {
    let mut r = BTreeMap::new();
    let online = read_trim("/sys/devices/system/cpu/online").unwrap_or_else(|_| "unknown".into());
    r.insert(
        "acceptance_verdict",
        if online == "0-3" {
            "conforming"
        } else {
            "nonconforming"
        }
        .into(),
    );
    r.insert("actual_frequencies_khz", policy_values("scaling_cur_freq"));
    r.insert("available_memory_bytes", mem_available_bytes()?.to_string());
    r.insert("context_switches", vmstat_value("ctxt")?.to_string());
    r.insert("display_mode", drm_mode());
    r.insert("irq_census", irq_census()?);
    r.insert(
        "observed_at_utc",
        fixed_command("/usr/bin/date", &["-u", "+%Y-%m-%dT%H:%M:%SZ"])?,
    );
    r.insert("online_cores", expand_cpu_list(&online)?);
    r.insert(
        "temperature_millic",
        read_trim("/sys/class/thermal/thermal_zone0/temp")?,
    );
    r.insert(
        "throttle_flags",
        fixed_command("/usr/bin/vcgencmd", &["get_throttled"])
            .unwrap_or_else(|_| "unavailable".into()),
    );
    encode_record("wrela-run-environment-v1", &r)
}

fn run_artifact(fields: &BTreeMap<String, String>) -> Result<String, String> {
    require_exact(
        fields,
        &[
            "action",
            "binary",
            "binary_sha256",
            "display",
            "host_profile",
            "image",
            "image_sha256",
            "measurement",
            "record",
            "record_mode",
            "record_sha256",
            "report",
            "report_sha256",
            "run_dir",
            "timeout_seconds",
            "translation",
        ],
    )?;
    let run_dir = safe_run_dir(&fields["run_dir"])?;
    let binary = safe_cached_binary(&fields["binary"], &fields["binary_sha256"])?;
    if digest_file(&binary)? != fields["binary_sha256"] {
        return Err("cached binary digest mismatch".into());
    }
    for (path_key, digest_key) in [("image", "image_sha256"), ("report", "report_sha256")] {
        let path = safe_child(&run_dir, &fields[path_key])?;
        let got = digest_file(&path)?;
        if got != fields[digest_key] {
            return Err(format!(
                "{path_key} digest mismatch: expected {}, got {got}",
                fields[digest_key]
            ));
        }
    }
    let timeout: u64 = canonical_u64(&fields["timeout_seconds"])?;
    if timeout == 0 || timeout > 3600 {
        return Err("timeout_seconds must be in 1..=3600".into());
    }
    if !matches!(fields["display"].as_str(), "headless" | "native") {
        return Err("display is invalid".into());
    }
    if !matches!(fields["host_profile"].as_str(), "diagnostic" | "product") {
        return Err("host_profile is invalid".into());
    }
    if !matches!(
        fields["measurement"].as_str(),
        "none" | "perf-stat" | "guest-pmu"
    ) {
        return Err("measurement is invalid".into());
    }
    if !matches!(fields["record_mode"].as_str(), "record" | "replay") {
        return Err("record_mode is invalid".into());
    }
    if !matches!(
        fields["translation"].as_str(),
        "sealed-stage1" | "diagnostic-mmu-off"
    ) {
        return Err("translation is invalid".into());
    }
    if fields["translation"] == "diagnostic-mmu-off" && fields["host_profile"] != "diagnostic" {
        return Err("diagnostic-mmu-off translation requires the diagnostic host profile".into());
    }
    if fields["translation"] == "diagnostic-mmu-off" && fields["record_mode"] != "record" {
        return Err("diagnostic-mmu-off translation does not support replay".into());
    }
    let record = safe_child(&run_dir, &fields["record"])?;
    match fields["record_mode"].as_str() {
        "record" if fields["record_sha256"] == "none" => {}
        "replay" if digest_file(&record)? == fields["record_sha256"] => {}
        "record" => return Err("record mode requires record_sha256=none".into()),
        "replay" => return Err("replay record digest mismatch".into()),
        _ => unreachable!(),
    }
    let metrics = run_dir.join("metrics.txt");
    let report = safe_child(&run_dir, &fields["report"])?;
    let image = safe_child(&run_dir, &fields["image"])?;
    let stdout_path = run_dir.join("stdout.bin");
    let stderr_path = run_dir.join("stderr.txt");
    std::fs::File::create(&metrics).map_err(|e| format!("create metrics placeholder: {e}"))?;
    if fields["record_mode"] == "record" {
        std::fs::File::create(&record).map_err(|e| format!("create record placeholder: {e}"))?;
    }
    if fields["measurement"] == "perf-stat" {
        std::fs::File::create(run_dir.join("perf.csv"))
            .map_err(|e| format!("create perf placeholder: {e}"))?;
    }
    let stdout = std::fs::File::create(&stdout_path).map_err(|e| format!("create stdout: {e}"))?;
    let stderr = std::fs::File::create(&stderr_path).map_err(|e| format!("create stderr: {e}"))?;
    let mut command = if fields["host_profile"] == "product" {
        product_command(
            &run_dir,
            &binary,
            &report,
            &image,
            &fields["display"],
            &fields["measurement"],
            &fields["record_mode"],
            &record,
            &metrics,
            &fields["translation"],
        )?
    } else {
        let mut command = if fields["measurement"] == "perf-stat" {
            let mut command = Command::new("/usr/bin/perf");
            append_perf_args(&mut command, &run_dir.join("perf.csv"));
            command.arg("--").arg(&binary);
            command
        } else {
            Command::new(&binary)
        };
        append_vmm_args(
            &mut command,
            &report,
            &image,
            &fields["display"],
            &fields["record_mode"],
            &record,
            &metrics,
        );
        if fields["measurement"] == "guest-pmu" {
            command.arg("--guest-counters");
        }
        command.env("WRELA_HOST_PROFILE", "diagnostic");
        if fields["translation"] == "diagnostic-mmu-off" {
            command.arg("--diagnostic-mmu-off");
        }
        command
    };
    let mut child = command
        .stdin(Stdio::null())
        .stdout(stdout)
        .stderr(stderr)
        .spawn()
        .map_err(|e| format!("start VMM: {e}"))?;
    let started = Instant::now();
    let unit = (fields["host_profile"] == "product").then(|| product_unit_name(&run_dir));
    let mut timed_out = false;
    let mut timeout_cleanup_error = None;
    let status = loop {
        if let Some(status) = child.try_wait().map_err(|e| format!("wait VMM: {e}"))? {
            break Some(status);
        }
        if started.elapsed() >= Duration::from_secs(timeout) {
            if let Some(unit) = &unit {
                if let Err(error) = stop_product_unit(unit) {
                    timeout_cleanup_error = Some(error);
                }
            }
            // Always reap the systemd-run wrapper and continue to result
            // creation, even when unit cleanup itself failed. The nonzero
            // result then remains retrievable for diagnosis.
            let _ = child.kill();
            let _ = child.wait();
            timed_out = true;
            break None;
        }
        std::thread::sleep(Duration::from_millis(10));
    };
    if let Some(error) = &timeout_cleanup_error {
        use std::io::Write as _;
        let mut retained = std::fs::OpenOptions::new()
            .append(true)
            .open(&stderr_path)
            .map_err(|open| format!("retain timeout cleanup failure: {open}"))?;
        writeln!(retained, "wrela-lab-agent: {error}")
            .map_err(|write| format!("retain timeout cleanup failure: {write}"))?;
    }
    let mut result = BTreeMap::new();
    result.insert("elapsed_ns", started.elapsed().as_nanos().to_string());
    result.insert(
        "exit_code",
        status
            .and_then(|status| status.code())
            .unwrap_or(if timeout_cleanup_error.is_some() {
                125
            } else if timed_out {
                124
            } else {
                255
            })
            .to_string(),
    );
    result.insert("host_profile", fields["host_profile"].clone());
    result.insert("measurement", fields["measurement"].clone());
    result.insert("metrics_sha256", digest_file(&metrics)?);
    result.insert(
        "perf_sha256",
        if fields["measurement"] == "perf-stat" {
            digest_file(&run_dir.join("perf.csv"))?
        } else {
            wrela_machine::sha256::sha256_hex(b"")
        },
    );
    result.insert("record_sha256", digest_file(&record)?);
    result.insert("stderr_sha256", digest_file(&stderr_path)?);
    result.insert("stdout_sha256", digest_file(&stdout_path)?);
    result.insert("translation", fields["translation"].clone());
    result.insert("timed_out", timed_out.to_string());
    let text = encode_record("wrela-lab-run-result-v1", &result)?;
    std::fs::write(run_dir.join("result.txt"), &text).map_err(|e| format!("write result: {e}"))?;
    Ok(text)
}

fn product_command(
    run_dir: &Path,
    binary: &Path,
    report: &Path,
    image: &Path,
    display: &str,
    measurement: &str,
    record_mode: &str,
    record: &Path,
    metrics: &Path,
    translation: &str,
) -> Result<Command, String> {
    if translation != "sealed-stage1" {
        return Err("product command refuses diagnostic translation".into());
    }
    let unit = product_unit_name(run_dir);
    let writable = format!("ReadWritePaths={}", run_dir.display());
    let mut command = Command::new("/usr/bin/systemd-run");
    command.args([
        "--user",
        "--quiet",
        "--wait",
        "--pipe",
        "--collect",
        &format!("--unit={unit}"),
        "--setenv=WRELA_HOST_PROFILE=product",
        "--property=PrivateNetwork=yes",
        "--property=PrivatePIDs=yes",
        "--property=NoNewPrivileges=yes",
        "--property=ProtectSystem=strict",
        "--property=ProtectHome=yes",
        "--property=ProtectKernelTunables=yes",
        "--property=ProtectKernelModules=yes",
        "--property=ProtectControlGroups=yes",
        "--property=ProtectClock=yes",
        "--property=RestrictSUIDSGID=yes",
        "--property=RestrictRealtime=yes",
        "--property=LockPersonality=yes",
        "--property=RestrictAddressFamilies=AF_UNIX",
        "--property=SystemCallArchitectures=native",
        "--property=SystemCallFilter=@system-service",
        "--property=SystemCallFilter=perf_event_open",
        "--property=SystemCallErrorNumber=EPERM",
        "--property=DevicePolicy=closed",
        "--property=DeviceAllow=/dev/kvm rw",
        "--property=DeviceAllow=/dev/dri/card0 rw",
        "--property=DeviceAllow=/dev/dri/card1 rw",
        "--property=DeviceAllow=/dev/dri/renderD128 rw",
        "--property=CPUQuota=400%",
        "--property=CPUAffinity=0 1 2 3",
        "--property=MemoryMax=805306368",
        "--property=MemorySwapMax=0",
        "--property=LimitMEMLOCK=536870912",
        "--property=TasksMax=16",
        "--property=UMask=0077",
        &format!("--property={writable}"),
    ]);
    if measurement == "perf-stat" {
        command.arg("/usr/bin/perf");
        append_perf_args(&mut command, &run_dir.join("perf.csv"));
        command.arg("--");
    }
    command.arg(binary);
    append_vmm_args(
        &mut command,
        report,
        image,
        display,
        record_mode,
        record,
        metrics,
    );
    if measurement == "guest-pmu" {
        command.arg("--guest-counters");
    }
    Ok(command)
}

fn product_unit_name(run_dir: &Path) -> String {
    let leaf = run_dir
        .file_name()
        .and_then(|value| value.to_str())
        .expect("validated run directory has a UTF-8 leaf");
    format!("wrela-{leaf}")
}

fn stop_product_unit(unit: &str) -> Result<(), String> {
    stop_product_unit_with(Path::new("/usr/bin/systemctl"), unit)
}

fn stop_product_unit_with(systemctl: &Path, unit: &str) -> Result<(), String> {
    let mut failures = Vec::new();
    match Command::new(systemctl)
        .args(["--user", "kill", "--kill-who=all", "--signal=KILL", unit])
        .status()
    {
        Ok(status) if status.success() => {}
        Ok(_) => failures.push(format!(
            "systemctl could not kill timed-out product unit `{unit}`"
        )),
        Err(error) => failures.push(format!("kill timed-out product unit `{unit}`: {error}")),
    }
    match Command::new(systemctl)
        .args(["--user", "stop", unit])
        .status()
    {
        Ok(status) if status.success() => {}
        Ok(_) => failures.push(format!(
            "systemctl could not stop timed-out product unit `{unit}`"
        )),
        Err(error) => failures.push(format!("stop timed-out product unit `{unit}`: {error}")),
    }
    match Command::new(systemctl)
        .args(["--user", "show", "--property=ActiveState", "--value", unit])
        .output()
    {
        Ok(state) => {
            let active = String::from_utf8_lossy(&state.stdout).trim().to_string();
            if !state.status.success() || !matches!(active.as_str(), "inactive" | "failed") {
                failures.push(format!(
                    "timed-out product unit `{unit}` remains in state `{active}`"
                ));
            }
        }
        Err(error) => failures.push(format!("inspect timed-out product unit `{unit}`: {error}")),
    }
    if failures.is_empty() {
        Ok(())
    } else {
        Err(failures.join("; "))
    }
}

fn append_perf_args(command: &mut Command, output: &Path) {
    command.args([
        "stat".as_ref(),
        "--no-big-num".as_ref(),
        "-x,".as_ref(),
        "--output".as_ref(),
        output.as_os_str(),
        "-e".as_ref(),
        "branch-misses".as_ref(),
        "-e".as_ref(),
        "cpu_cycles".as_ref(),
        "-e".as_ref(),
        "instructions".as_ref(),
        "-e".as_ref(),
        "l1d_cache_refill".as_ref(),
        "-e".as_ref(),
        "l2d_cache_refill".as_ref(),
        "-e".as_ref(),
        "stall_backend".as_ref(),
        "-e".as_ref(),
        "stall_frontend".as_ref(),
    ]);
}

fn append_vmm_args(
    command: &mut Command,
    report: &Path,
    image: &Path,
    display: &str,
    record_mode: &str,
    record: &Path,
    metrics: &Path,
) {
    command.args([
        report.as_os_str(),
        image.as_os_str(),
        "--display".as_ref(),
        display.as_ref(),
        "--metrics".as_ref(),
        metrics.as_os_str(),
        if record_mode == "record" {
            "--record"
        } else {
            "--replay"
        }
        .as_ref(),
        record.as_os_str(),
    ]);
}

fn require_exact(fields: &BTreeMap<String, String>, allowed: &[&str]) -> Result<(), String> {
    let got = fields.keys().map(String::as_str).collect::<Vec<_>>();
    let mut want = allowed.to_vec();
    want.sort_unstable();
    if got == want {
        Ok(())
    } else {
        Err(format!("manifest fields are {got:?}, expected {want:?}"))
    }
}

fn safe_run_dir(raw: &str) -> Result<PathBuf, String> {
    let path = Path::new(raw);
    let root = Path::new(LAB_ROOT);
    let name = path
        .file_name()
        .and_then(|n| n.to_str())
        .ok_or("run_dir has no UTF-8 leaf")?;
    if path.parent() != Some(&root.join("runs"))
        || !name.starts_with("run-")
        || name.len() != 20
        || !name[4..]
            .bytes()
            .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
    {
        return Err("run_dir is not a validated narrow directory beneath the lab root".into());
    }
    Ok(path.to_path_buf())
}

fn safe_child(dir: &Path, raw: &str) -> Result<PathBuf, String> {
    let name = Path::new(raw);
    if name.components().count() != 1
        || raw.starts_with('-')
        || raw.is_empty()
        || !raw
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-' | b'_'))
    {
        return Err(format!("unsafe artifact name `{raw}`"));
    }
    Ok(dir.join(name))
}

fn safe_cached_binary(name: &str, digest: &str) -> Result<PathBuf, String> {
    if !wrela_machine::sha256::is_sha256_hex(digest) || name != format!("wrela-vmm-{digest}") {
        return Err("cached binary name is not bound to its lowercase SHA-256 digest".into());
    }
    Ok(Path::new(LAB_ROOT).join("bin").join(name))
}

fn fixed_command(program: &str, args: &[&str]) -> Result<String, String> {
    let output = Command::new(program)
        .args(args)
        .output()
        .map_err(|e| format!("run {program}: {e}"))?;
    if !output.status.success() {
        return Err(format!("{program} returned {}", output.status));
    }
    String::from_utf8(output.stdout)
        .map(|s| s.trim_matches(['\0', '\n', '\r']).to_string())
        .map_err(|_| format!("{program} output is not UTF-8"))
}

fn read_trim(path: &str) -> Result<String, String> {
    std::fs::read(path)
        .map_err(|e| format!("read {path}: {e}"))
        .and_then(|b| String::from_utf8(b).map_err(|_| format!("{path} is not UTF-8")))
        .map(|s| s.trim_matches(['\0', '\n', '\r']).to_string())
}

fn cpuinfo_value<'a>(text: &'a str, key: &str) -> Option<&'a str> {
    text.lines().find_map(|line| {
        let (k, v) = line.split_once(':')?;
        (k.trim() == key).then(|| v.trim())
    })
}
fn digest_file(path: &Path) -> Result<String, String> {
    std::fs::read(path)
        .map(|b| wrela_machine::sha256::sha256_hex(&b))
        .map_err(|error| format!("read declared artifact {}: {error}", path.display()))
}
fn digest_tree(root: &Path) -> Result<String, String> {
    let mut rows = Vec::new();
    walk(root, root, &mut rows)?;
    rows.sort();
    let mut b = Vec::new();
    for (p, d) in rows {
        b.extend_from_slice(p.as_bytes());
        b.push(0);
        b.extend_from_slice(&d);
        b.push(0xff);
    }
    Ok(wrela_machine::sha256::sha256_hex(&b))
}
fn walk(root: &Path, path: &Path, out: &mut Vec<(String, Vec<u8>)>) -> Result<(), String> {
    let mut entries = std::fs::read_dir(path)
        .map_err(|e| format!("read {}: {e}", path.display()))?
        .filter_map(Result::ok)
        .collect::<Vec<_>>();
    entries.sort_by_key(|e| e.file_name());
    for e in entries {
        let p = e.path();
        if p.is_dir() {
            walk(root, &p, out)?
        } else if p.is_file() {
            out.push((
                p.strip_prefix(root).unwrap().to_string_lossy().into_owned(),
                std::fs::read(&p).map_err(|e| format!("read {}: {e}", p.display()))?,
            ));
        }
    }
    Ok(())
}
fn module_identity(name: &str) -> String {
    let version = read_trim(&format!("/sys/module/{name}/version"))
        .unwrap_or_else(|_| "builtin-or-unversioned".into());
    format!("{name}:{version}")
}
fn module_set_digest() -> Result<String, String> {
    let mut modules = std::fs::read_dir("/sys/module")
        .map_err(|error| format!("read module set: {error}"))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| format!("read module set entry: {error}"))?;
    modules.sort_by_key(std::fs::DirEntry::file_name);
    let mut bytes = b"wrela-linux-module-set-v1\0".to_vec();
    for module in modules {
        let name = module
            .file_name()
            .into_string()
            .map_err(|_| "module name is not UTF-8")?;
        bytes.extend_from_slice(name.as_bytes());
        bytes.push(b'=');
        bytes.extend_from_slice(module_identity(&name).as_bytes());
        bytes.push(0);
    }
    Ok(wrela_machine::sha256::sha256_hex(&bytes))
}
fn policy_values(name: &str) -> String {
    (0..4)
        .map(|cpu| {
            read_trim(&format!("/sys/devices/system/cpu/cpu{cpu}/cpufreq/{name}"))
                .unwrap_or_else(|_| "unavailable".into())
        })
        .collect::<Vec<_>>()
        .join(",")
}
fn mem_available_bytes() -> Result<u64, String> {
    let t = std::fs::read_to_string("/proc/meminfo").map_err(|e| e.to_string())?;
    let kb = t
        .lines()
        .find_map(|l| {
            l.strip_prefix("MemAvailable:")?
                .split_whitespace()
                .next()?
                .parse::<u64>()
                .ok()
        })
        .ok_or("MemAvailable missing")?;
    Ok(kb * 1024)
}
fn vmstat_value(key: &str) -> Result<u64, String> {
    let t = std::fs::read_to_string("/proc/stat").map_err(|e| e.to_string())?;
    t.lines()
        .find_map(|l| {
            let mut p = l.split_whitespace();
            (p.next()? == key).then(|| p.next()?.parse().ok()).flatten()
        })
        .ok_or_else(|| format!("{key} missing"))
}
fn irq_census() -> Result<String, String> {
    let t = std::fs::read_to_string("/proc/interrupts").map_err(|e| e.to_string())?;
    Ok(t.lines()
        .skip(1)
        .map(|l| {
            l.split_whitespace()
                .skip(1)
                .take(4)
                .filter_map(|v| v.parse::<u64>().ok())
                .sum::<u64>()
        })
        .sum::<u64>()
        .to_string())
}
fn drm_mode() -> String {
    std::fs::read_dir("/sys/class/drm")
        .ok()
        .into_iter()
        .flatten()
        .filter_map(Result::ok)
        .filter_map(|e| read_trim(&format!("{}/modes", e.path().display())).ok())
        .find(|v| !v.is_empty())
        .unwrap_or_else(|| "headless".into())
}
fn expand_cpu_list(raw: &str) -> Result<String, String> {
    if let Some((a, b)) = raw.split_once('-') {
        let a = canonical_u64(a)?;
        let b = canonical_u64(b)?;
        if a > b {
            return Err("online CPU range reversed".into());
        }
        Ok((a..=b).map(|v| v.to_string()).collect::<Vec<_>>().join(","))
    } else {
        canonical_u64(raw).map(|v| v.to_string())
    }
}
fn canonical_u64(raw: &str) -> Result<u64, String> {
    if raw.is_empty()
        || (raw.len() > 1 && raw.starts_with('0'))
        || !raw.bytes().all(|b| b.is_ascii_digit())
    {
        return Err(format!("noncanonical integer `{raw}`"));
    }
    raw.parse().map_err(|e| format!("bad integer `{raw}`: {e}"))
}
fn encode_record(format: &str, fields: &BTreeMap<&str, String>) -> Result<String, String> {
    let mut out = format!("format={format}\n");
    for (k, v) in fields {
        out.push_str(k);
        out.push('=');
        encode_value(v.as_bytes(), &mut out);
        out.push('\n');
    }
    Ok(out)
}

fn parse_record(text: &str, format: &str) -> Result<BTreeMap<String, String>, String> {
    if !text.ends_with('\n') || text.contains(['\r', '\0']) {
        return Err("record is not canonical LF text".into());
    }
    let mut lines = text.lines();
    if lines.next() != Some(&format!("format={format}")) {
        return Err(format!("record is not `{format}`"));
    }
    let mut fields = BTreeMap::new();
    let mut prior = "";
    for line in lines {
        let (key, value) = line.split_once('=').ok_or("malformed record field")?;
        if key <= prior
            || fields
                .insert(key.to_string(), percent_decode(value)?)
                .is_some()
        {
            return Err("record fields are not sorted and unique".into());
        }
        prior = key;
    }
    Ok(fields)
}
fn encode_value(bytes: &[u8], out: &mut String) {
    const H: &[u8; 16] = b"0123456789ABCDEF";
    for &b in bytes {
        if b.is_ascii_alphanumeric() || matches!(b, b' ' | b'.' | b'_' | b'-' | b'/' | b':' | b',')
        {
            out.push(b as char)
        } else {
            out.push('%');
            out.push(H[(b >> 4) as usize] as char);
            out.push(H[(b & 15) as usize] as char)
        }
    }
}
fn percent_decode(raw: &str) -> Result<String, String> {
    let b = raw.as_bytes();
    let mut o = Vec::new();
    let mut i = 0;
    while i < b.len() {
        if b[i] != b'%' {
            o.push(b[i]);
            i += 1
        } else {
            let p = b.get(i + 1..i + 3).ok_or("truncated percent escape")?;
            let n = |x| match x {
                b'0'..=b'9' => Ok(x - b'0'),
                b'A'..=b'F' => Ok(x - b'A' + 10),
                _ => Err("bad percent escape"),
            };
            o.push((n(p[0])? << 4) | n(p[1])?);
            i += 3
        }
    }
    String::from_utf8(o).map_err(|_| "decoded value is not UTF-8".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn required_kvm_capabilities_are_exact_and_readonly_memory_is_optional() {
        let yes = "api_version:yes,immediate_exit:yes,ipa_size:yes,isa_baseline:yes,one_reg:yes,readonly_mem:no,target:yes,user_memory:yes,vcpu_count:yes,writable_id_regs:yes";
        assert!(required_kvm_capabilities_ok(yes).unwrap());
        assert!(
            required_kvm_capabilities_ok(&yes.replace("readonly_mem:no", "readonly_mem:yes"))
                .unwrap()
        );
        for required in [
            "api_version",
            "immediate_exit",
            "ipa_size",
            "isa_baseline",
            "one_reg",
            "target",
            "user_memory",
            "vcpu_count",
            "writable_id_regs",
        ] {
            assert!(
                !required_kvm_capabilities_ok(
                    &yes.replace(&format!("{required}:yes"), &format!("{required}:no"))
                )
                .unwrap(),
                "{required} must be required"
            );
        }
        assert!(required_kvm_capabilities_ok(&format!("{yes},extra:yes")).is_err());
    }

    #[test]
    fn product_service_attestation_requires_resource_seccomp_and_device_controls() {
        let service = std::fs::read_to_string(
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../../packaging/linux/wrela-vmm.service"),
        )
        .unwrap();
        assert!(linux_service_policy_exact(&service));
        for row in [
            "SystemCallFilter=@system-service",
            "DevicePolicy=closed",
            "MemoryMax=768M",
            "MemorySwapMax=0",
            "LimitMEMLOCK=536870912",
            "CPUQuota=400%",
        ] {
            assert!(!linux_service_policy_exact(&service.replace(row, "")));
        }
    }

    #[test]
    fn hostile_manifests_fail_closed() {
        for text in [
            "action=probe-identity\n",
            &format!("format={FORMAT}\naction=run\naction=run\n"),
            &format!("format={FORMAT}\nz=1\naction=run\n"),
        ] {
            assert!(parse_manifest(text).is_err());
        }
    }
    #[test]
    fn cleanup_and_artifact_targets_are_narrow() {
        assert!(safe_run_dir("/var/tmp/wrela-lab/runs/run-0123456789abcdef").is_ok());
        for p in [
            "/",
            "/var/tmp/wrela-lab",
            "/var/tmp/wrela-lab/runs/*",
            "/var/tmp/wrela-lab/runs/run-xyz",
        ] {
            assert!(safe_run_dir(p).is_err());
        }
        let d = Path::new("/var/tmp/wrela-lab/runs/run-0123456789abcdef");
        assert!(safe_child(d, "image-a.img").is_ok());
        assert!(safe_child(d, "../image").is_err());
        assert!(safe_child(d, "--help").is_err());
    }

    #[test]
    fn cleanup_removes_only_the_exact_manifest_run() {
        let run_dir = format!("{LAB_ROOT}/runs/run-{:016x}", std::process::id());
        let path = Path::new(&run_dir);
        std::fs::create_dir_all(path).unwrap();
        std::fs::write(path.join("retained"), b"test").unwrap();
        let fields = BTreeMap::from([
            ("action".to_string(), "cleanup".to_string()),
            ("run_dir".to_string(), run_dir.clone()),
        ]);
        let response = cleanup_run(&fields).unwrap();
        assert!(!path.exists());
        assert!(response.contains("verdict=removed\n"));
        assert!(cleanup_run(&fields).is_err());
    }

    #[test]
    fn failed_vmm_still_emits_a_retrievable_result_and_diagnostics() {
        use std::os::unix::fs::PermissionsExt;

        let run_dir = format!("{LAB_ROOT}/runs/run-{:016x}", std::process::id() as u64 + 1);
        let path = Path::new(&run_dir);
        std::fs::create_dir_all(path).unwrap();
        let binary_bytes = b"#!/bin/sh\nexit 7\n";
        let binary_digest = wrela_machine::sha256::sha256_hex(binary_bytes);
        let binary_name = format!("wrela-vmm-{binary_digest}");
        let binary = Path::new(LAB_ROOT).join("bin").join(&binary_name);
        std::fs::create_dir_all(binary.parent().unwrap()).unwrap();
        std::fs::write(&binary, binary_bytes).unwrap();
        std::fs::set_permissions(&binary, std::fs::Permissions::from_mode(0o700)).unwrap();
        std::fs::write(path.join("image.img"), b"image").unwrap();
        std::fs::write(path.join("report.txt"), b"report").unwrap();
        let fields = BTreeMap::from([
            ("action".into(), "run".into()),
            ("binary".into(), binary_name),
            ("binary_sha256".into(), binary_digest),
            ("display".into(), "headless".into()),
            ("host_profile".into(), "diagnostic".into()),
            ("image".into(), "image.img".into()),
            (
                "image_sha256".into(),
                digest_file(&path.join("image.img")).unwrap(),
            ),
            ("measurement".into(), "none".into()),
            ("record".into(), "record.txt".into()),
            ("record_mode".into(), "record".into()),
            ("record_sha256".into(), "none".into()),
            ("report".into(), "report.txt".into()),
            (
                "report_sha256".into(),
                digest_file(&path.join("report.txt")).unwrap(),
            ),
            ("run_dir".into(), run_dir.clone()),
            ("timeout_seconds".into(), "1".into()),
            ("translation".into(), "sealed-stage1".into()),
        ]);
        let result = run_artifact(&fields).unwrap();
        let parsed = parse_record(&result, "wrela-lab-run-result-v1").unwrap();
        assert_ne!(parsed["exit_code"], "0");
        for artifact in [
            "metrics.txt",
            "record.txt",
            "result.txt",
            "stderr.txt",
            "stdout.bin",
        ] {
            assert!(path.join(artifact).is_file());
        }
        std::fs::remove_dir_all(path).unwrap();
        std::fs::remove_file(binary).unwrap();
    }

    #[test]
    fn timeout_cleanup_kills_stops_and_requires_an_inactive_unit() {
        use std::os::unix::fs::PermissionsExt;

        let temp = std::env::temp_dir().join(format!(
            "wrela-lab-agent-systemctl-{:016x}",
            std::process::id()
        ));
        std::fs::create_dir_all(&temp).unwrap();
        let calls = temp.join("calls");
        let systemctl = temp.join("systemctl");
        std::fs::write(
            &systemctl,
            format!(
                "#!/bin/sh\nprintf '%s\\n' \"$*\" >> '{}'\nif [ \"$2\" = show ]; then printf 'inactive\\n'; fi\n",
                calls.display()
            ),
        )
        .unwrap();
        std::fs::set_permissions(&systemctl, std::fs::Permissions::from_mode(0o700)).unwrap();

        stop_product_unit_with(&systemctl, "wrela-run-0123456789abcdef").unwrap();
        assert_eq!(
            std::fs::read_to_string(&calls).unwrap(),
            "--user kill --kill-who=all --signal=KILL wrela-run-0123456789abcdef\n\
             --user stop wrela-run-0123456789abcdef\n\
             --user show --property=ActiveState --value wrela-run-0123456789abcdef\n"
        );

        std::fs::write(
            &systemctl,
            "#!/bin/sh\nif [ \"$2\" = show ]; then printf 'active\\n'; fi\n",
        )
        .unwrap();
        assert!(stop_product_unit_with(&systemctl, "wrela-run-0123456789abcdef").is_err());
        std::fs::remove_dir_all(temp).unwrap();
    }
}
