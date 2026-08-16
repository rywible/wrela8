use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::pixels_cache::{Cache, file_digest, is_sha256_hex, key_of, tree_digest};
use crate::{CompileOptsGuard, golden_case_dirs, root, run};
use wrela_compiler::opts::CompileMode;

fn wrela_command(wrela: &Path) -> Command {
    let mut command = Command::new(wrela);
    command.arg("__wrela");
    command
}

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

fn pixels_test_report_green(output: &str) -> bool {
    output.lines().any(|line| {
        line.trim()
            .strip_suffix(" passed, 0 failed")
            .and_then(|count| count.parse::<usize>().ok())
            .is_some_and(|count| count > 0)
    })
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
    /// Exact case-name selection. When set, a case is selected iff its name
    /// is in this list (the substring `filter` still applies on top). Lets a
    /// harness run one parallel golden pass over a fixed case set instead of
    /// one serial `golden()` invocation per case.
    pub(crate) cases: Option<Vec<String>>,
    pub(crate) boot: BootSel,
    pub(crate) jobs: usize,
    pub(crate) boot_jobs: usize,
    pub(crate) pixels_telemetry: bool,
    /// Run accepted Pixels artifact bundles in one exact-case child process
    /// apiece. Their compiler arenas are intentionally large; process
    /// isolation returns those arenas to the OS between cases.
    pub(crate) isolate_pixels_bundles: bool,
    /// Compatibility marker accepted from older dev-loop invocations. The
    /// compiler CLI is embedded now, so every task-runner is already built.
    pub(crate) assume_built: bool,
    /// Internal recursion guard for the one-bundle-per-process cold-static
    /// path. This is deliberately distinct from `assume_built`: that public
    /// compatibility flag must not silently select the slower in-process
    /// renderer path.
    pub(crate) isolated_child: bool,
    /// Remove successful golden boot transcripts before selecting cases.
    pub(crate) clear_boot_cache: bool,
}

pub(crate) fn default_jobs() -> usize {
    std::thread::available_parallelism().map_or(1, |n| n.get())
}

// Image compilation uses the ordinary worker count. Only the VMM subprocess is
// throttled: four concurrent HVF guests intermittently starve multicore
// quiescence long enough to produce a false `quiesce=timeout` transcript.
pub(crate) const DEFAULT_BOOT_JOBS: usize = 2;
pub(crate) const VMM_HVF_TESTS: usize = 25;

// Renderer-bearing pixels work (in-process bundle compiles, isolated child
// processes, renderer dump subprocesses, and pixels guest-boot compiles)
// retains large compiler arenas. This process-wide gate bounds how many run
// concurrently so the ordinary worker pool can stay as wide as the host
// without risking memory pressure on a 16 GiB machine; cheap stages
// (check/typed/...) never take a permit.
//
// The bound is memory, not merely CPU. After the P8R arena and import-splice
// reductions, an uncached whole-static-corpus run peaks below 5 GiB with ten
// renderer compiles active on the 16 GiB baseline host. Ten also keeps every
// physical core useful once the compiler's shared work has been eliminated;
// raising it further adds allocator contention without improving wall time.
// Hosts that run several worktrees at once — or a smaller machine — can dial
// this down through the environment rather than editing a constant.
pub(crate) const HEAVY_PIXELS_JOBS: usize = 10;

pub(crate) fn heavy_pixels_jobs() -> usize {
    std::env::var("WRELA_HEAVY_PIXELS_JOBS")
        .ok()
        .and_then(|value| value.parse::<usize>().ok())
        .filter(|jobs| *jobs >= 1)
        .unwrap_or(HEAVY_PIXELS_JOBS)
        .min(default_jobs().max(1))
}

struct HeavyGate {
    permits: std::sync::Mutex<usize>,
    available: std::sync::Condvar,
}

struct HeavyPermit<'a>(&'a HeavyGate);

impl HeavyGate {
    fn acquire(&self) -> HeavyPermit<'_> {
        let mut permits = self.permits.lock().unwrap_or_else(|e| e.into_inner());
        while *permits == 0 {
            permits = self
                .available
                .wait(permits)
                .unwrap_or_else(|e| e.into_inner());
        }
        *permits -= 1;
        HeavyPermit(self)
    }
}

impl Drop for HeavyPermit<'_> {
    fn drop(&mut self) {
        let mut permits = self.0.permits.lock().unwrap_or_else(|e| e.into_inner());
        *permits += 1;
        self.0.available.notify_one();
    }
}

fn heavy_gate() -> &'static HeavyGate {
    static GATE: std::sync::OnceLock<HeavyGate> = std::sync::OnceLock::new();
    GATE.get_or_init(|| HeavyGate {
        permits: std::sync::Mutex::new(heavy_pixels_jobs()),
        available: std::sync::Condvar::new(),
    })
}

fn heavy_pixels_stage(case_name: &str, command_stage: &str) -> bool {
    case_name.contains("pixels")
        && matches!(
            command_stage,
            "report"
                | "image"
                | "field-graph"
                | "frame-program"
                | "render-layout"
                | "asm"
                | "mwir"
                | "test"
                | "test-omit-dmb"
        )
}

fn scheduling_sensitive_boot_case(case: &Path) -> bool {
    case.file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name.starts_with("boot-cross-core-"))
}

impl Default for GoldenOpts {
    fn default() -> Self {
        Self {
            update: false,
            filter: None,
            cases: None,
            boot: BootSel::All,
            jobs: default_jobs(),
            boot_jobs: DEFAULT_BOOT_JOBS,
            pixels_telemetry: false,
            isolate_pixels_bundles: false,
            assume_built: false,
            isolated_child: false,
            clear_boot_cache: false,
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

fn accepted_pixels_case(case: &Path) -> bool {
    case.file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| {
            name.starts_with("check-pixels-")
                || (name.starts_with("boot-pixels-") && case.join("expected/test.txt").is_file())
        })
}

fn golden_boot_cache_eligible(stage: &str) -> bool {
    matches!(stage, "test" | "test-omit-dmb")
}

fn golden_boot_key_from_digests(
    case_name: &str,
    target: &str,
    stage: &str,
    pixels_telemetry: bool,
    case_source: &str,
    target_source: &str,
    stdlib_source: &str,
    bench_source: &str,
    census: &str,
    compiler: &str,
    vmm: &str,
) -> String {
    key_of(&[
        ("contract", "golden-boot-v1".to_string()),
        ("case", case_name.to_string()),
        ("target", target.to_string()),
        ("stage", stage.to_string()),
        ("pixels-telemetry", pixels_telemetry.to_string()),
        ("case-source", case_source.to_string()),
        ("target-source", target_source.to_string()),
        ("stdlib-source", stdlib_source.to_string()),
        ("bench-source", bench_source.to_string()),
        ("census", census.to_string()),
        ("compiler", compiler.to_string()),
        ("vmm", vmm.to_string()),
    ])
}

fn golden_boot_image_key(
    case_name: &str,
    stage: &str,
    pixels_telemetry: bool,
    image_digest: &str,
    vmm_digest: &str,
) -> String {
    key_of(&[
        ("contract", "golden-boot-image-v1".to_string()),
        ("case", case_name.to_string()),
        ("stage", stage.to_string()),
        ("pixels-telemetry", pixels_telemetry.to_string()),
        ("image", image_digest.to_string()),
        ("vmm", vmm_digest.to_string()),
    ])
}

fn production_stdlib_digest(stdlib: &Path) -> Result<String, String> {
    Ok(key_of(&[
        ("core", tree_digest(&stdlib.join("core"), &["wr"])?),
        ("drivers", tree_digest(&stdlib.join("drivers"), &["wr"])?),
    ]))
}

struct GoldenBootKeyContext {
    stdlib_source: String,
    bench_source: String,
    census: String,
    compiler: String,
    vmm: String,
}

impl GoldenBootKeyContext {
    fn new(wrela: &Path, vmm: &Path) -> Result<Self, String> {
        Ok(Self {
            stdlib_source: production_stdlib_digest(&root().join("stdlib"))?,
            bench_source: tree_digest(&root().join("bench"), &["toml"])?,
            census: file_digest(&root().join("tests/census.toml")),
            compiler: file_digest(wrela),
            vmm: file_digest(vmm),
        })
    }
}

struct GoldenStageKeyContext {
    stdlib_source: String,
    bench_source: String,
    census: String,
    compiler: String,
    tool: String,
}

impl GoldenStageKeyContext {
    fn new(wrela: &Path) -> Result<Self, String> {
        // Keep the source-level producer boundary explicit. `main.rs` owns
        // bundle production, `golden.rs` owns stage selection and invocation,
        // and `pixels_cache.rs` owns artifact interpretation. Ordinary dump
        // text is covered by the separately fingerprinted compiler binary.
        let tool = golden_tool_digest_from_parts(
            &file_digest(&root().join("crates/xtask/src/main.rs")),
            &file_digest(&root().join("crates/xtask/src/golden.rs")),
            &file_digest(&root().join("crates/xtask/src/pixels_cache.rs")),
        );
        Ok(Self {
            stdlib_source: production_stdlib_digest(&root().join("stdlib"))?,
            bench_source: tree_digest(&root().join("bench"), &["toml"])?,
            census: file_digest(&root().join("tests/census.toml")),
            compiler: file_digest(wrela),
            tool,
        })
    }
}

fn golden_tool_digest_from_parts(main: &str, golden: &str, cache: &str) -> String {
    key_of(&[
        ("main", main.to_string()),
        ("golden", golden.to_string()),
        ("cache", cache.to_string()),
    ])
}

/// Digest the complete fixture input tree while deliberately excluding only
/// checked expectations. Cost stages consume frequency sidecars in addition
/// to Wrela source, and project fixtures may add other non-`.wr` inputs over
/// time. Over-covering those inputs costs a cache miss; under-covering them
/// could return a stale diagnostic or report.
fn golden_input_tree_digest(dir: &Path) -> Result<String, String> {
    fn collect(dir: &Path, out: &mut Vec<PathBuf>) -> Result<(), String> {
        let mut entries = std::fs::read_dir(dir)
            .map_err(|error| format!("read {}: {error}", dir.display()))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| format!("read {}: {error}", dir.display()))?;
        entries.sort_by_key(std::fs::DirEntry::file_name);
        for entry in entries {
            let path = entry.path();
            if path.is_dir() {
                if entry.file_name() != "expected" {
                    collect(&path, out)?;
                }
            } else if path.is_file() {
                out.push(path);
            }
        }
        Ok(())
    }

    let mut files = Vec::new();
    if dir.is_dir() {
        collect(dir, &mut files)?;
    }
    let mut identity = String::new();
    for file in files {
        let relative = file.strip_prefix(dir).unwrap_or(&file);
        let bytes =
            std::fs::read(&file).map_err(|error| format!("read {}: {error}", file.display()))?;
        identity.push_str(&format!(
            "{} {}\n",
            relative.display(),
            wrela_compiler::report::sha256_hex(&bytes)
        ));
    }
    Ok(wrela_compiler::report::sha256_hex(identity.as_bytes()))
}

fn golden_stage_key_from_digests(
    case_name: &str,
    target: &str,
    stage: &str,
    renderer: &str,
    case_source: &str,
    target_source: &str,
    stdlib_source: &str,
    bench_source: &str,
    census: &str,
    compiler: &str,
    tool: &str,
) -> String {
    key_of(&[
        ("contract", "golden-stage-v2".to_string()),
        ("case", case_name.to_string()),
        ("target", target.to_string()),
        ("stage", stage.to_string()),
        ("renderer", renderer.to_string()),
        ("case-source", case_source.to_string()),
        ("target-source", target_source.to_string()),
        ("stdlib-source", stdlib_source.to_string()),
        ("bench-source", bench_source.to_string()),
        ("census", census.to_string()),
        ("compiler", compiler.to_string()),
        ("tool", tool.to_string()),
    ])
}

fn golden_image_digest_key_from_digests(
    case_name: &str,
    target: &str,
    stage: &str,
    pixels_telemetry: bool,
    case_source: &str,
    target_source: &str,
    stdlib_source: &str,
    bench_source: &str,
    census: &str,
    compiler: &str,
) -> String {
    key_of(&[
        ("contract", "golden-image-digest-v1".to_string()),
        ("case", case_name.to_string()),
        ("target", target.to_string()),
        ("stage", stage.to_string()),
        ("pixels-telemetry", pixels_telemetry.to_string()),
        ("case-source", case_source.to_string()),
        ("target-source", target_source.to_string()),
        ("stdlib-source", stdlib_source.to_string()),
        ("bench-source", bench_source.to_string()),
        ("census", census.to_string()),
        ("compiler", compiler.to_string()),
    ])
}

struct GoldenSourceDigests {
    case: String,
    target: String,
}

impl GoldenSourceDigests {
    fn new(case: &Path, input: &Path) -> Result<Self, String> {
        let target_scope = if input.is_dir() {
            input
        } else {
            input.parent().unwrap_or(input)
        };
        let case_digest = golden_input_tree_digest(case)?;
        let target_digest = if target_scope == case {
            case_digest.clone()
        } else {
            golden_input_tree_digest(target_scope)?
        };
        Ok(Self {
            case: case_digest,
            target: target_digest,
        })
    }
}

fn golden_stage_cache_key(
    case: &Path,
    input: &Path,
    stage: &str,
    renderer: Option<usize>,
    sources: &GoldenSourceDigests,
    context: &GoldenStageKeyContext,
) -> Result<String, String> {
    let case_name = case
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| format!("bad golden case path: {}", case.display()))?;
    let target = input
        .strip_prefix(root())
        .unwrap_or(input)
        .to_string_lossy()
        .replace('\\', "/");
    Ok(golden_stage_key_from_digests(
        case_name,
        &target,
        stage,
        &renderer.map_or_else(|| "default".to_string(), |value| value.to_string()),
        &sources.case,
        &sources.target,
        &context.stdlib_source,
        &context.bench_source,
        &context.census,
        &context.compiler,
        &context.tool,
    ))
}

fn bundle_cache_stage(stage: &str) -> bool {
    matches!(
        stage,
        "check" | "typed" | "report" | "image" | "field-graph" | "frame-program" | "render-layout"
    )
}

/// Stages implemented by `wrela dump` have no filesystem side effects and
/// return their complete observation as text, so an exact cache hit can skip
/// the process. Build/img are intentionally excluded because later stages
/// consume their files; guest tests use the separately signed boot cache.
fn golden_dump_stage_cacheable(stage: &str) -> bool {
    !matches!(
        stage,
        "test" | "test-omit-dmb" | "build" | "build-err" | "img"
    )
}

fn golden_boot_cache_key(
    case: &Path,
    input: &Path,
    stage: &str,
    pixels_telemetry: bool,
    context: &GoldenBootKeyContext,
) -> Result<String, String> {
    let case_name = case
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| format!("bad golden case path: {}", case.display()))?;
    let target = input
        .strip_prefix(root())
        .unwrap_or(input)
        .to_string_lossy()
        .replace('\\', "/");
    let target_scope = if input.is_dir() {
        input
    } else {
        input.parent().unwrap_or(input)
    };
    Ok(golden_boot_key_from_digests(
        case_name,
        &target,
        stage,
        pixels_telemetry,
        &tree_digest(case, &["wr"])?,
        &tree_digest(target_scope, &["wr"])?,
        &context.stdlib_source,
        &context.bench_source,
        &context.census,
        &context.compiler,
        &context.vmm,
    ))
}

struct GoldenImageDigestLookup {
    digest: Option<String>,
}

fn golden_boot_image_digest_lookup(
    case: &Path,
    input: &Path,
    stage: &str,
    pixels_telemetry: bool,
    wrela: &Path,
    vmm: &Path,
    context: &GoldenBootKeyContext,
) -> Result<GoldenImageDigestLookup, String> {
    let case_name = case
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| format!("bad golden case path: {}", case.display()))?;
    let target = input
        .strip_prefix(root())
        .unwrap_or(input)
        .to_string_lossy()
        .replace('\\', "/");
    let target_scope = if input.is_dir() {
        input
    } else {
        input.parent().unwrap_or(input)
    };
    let digest_key = golden_image_digest_key_from_digests(
        case_name,
        &target,
        stage,
        pixels_telemetry,
        &tree_digest(case, &["wr"])?,
        &tree_digest(target_scope, &["wr"])?,
        &context.stdlib_source,
        &context.bench_source,
        &context.census,
        &context.compiler,
    );
    let compile_cache = Cache::compile_closure();
    let cached = compile_cache.get(&digest_key);
    let digest = if let Some(digest) = cached
        && is_sha256_hex(digest.trim())
    {
        Some(digest.trim().to_string())
    } else {
        let mut command = wrela_command(wrela);
        command
            .current_dir(root())
            .arg("test")
            .arg(input)
            .arg("--image-digest-only")
            .arg("--vmm")
            .arg(vmm);
        if stage == "test-omit-dmb" {
            command.arg("--omit-dmb");
        }
        if pixels_telemetry {
            command.arg("--pixels-telemetry");
        }
        let output = command
            .output()
            .map_err(|error| format!("{case_name} [{stage}]: derive image digest: {error}"))?;
        if !output.status.success() {
            None
        } else if let Some(digest) =
            parse_golden_image_digest(&String::from_utf8_lossy(&output.stdout))
        {
            compile_cache.put(&digest_key, &digest);
            Some(digest)
        } else {
            // A comptime-only test has no runnable image. It remains cheap and
            // deliberately ineligible for the image-keyed boot cache.
            None
        }
    };
    Ok(GoldenImageDigestLookup { digest })
}

fn parse_golden_image_digest(stdout: &str) -> Option<String> {
    stdout
        .lines()
        .find_map(|line| line.strip_prefix("p8-image-digest "))
        .filter(|digest| is_sha256_hex(digest))
        .map(str::to_string)
}

fn golden_boot_cache_publishable(
    eligible: bool,
    process_succeeded: bool,
    actual: &str,
    expectation_matched: bool,
) -> bool {
    eligible
        && process_succeeded
        && pixels_test_report_green(actual)
        && !actual.lines().any(|line| line.contains(": FAILED"))
        && expectation_matched
}

fn expected_files_need_pixels_bundle(expected_files: &[PathBuf], boot: BootSel) -> bool {
    expected_files.iter().any(|path| {
        let stage = path
            .file_stem()
            .and_then(|stage| stage.to_str())
            .unwrap_or("");
        let (base, _) = renderer_dump_stage(stage).unwrap_or((stage, None));
        matches!(
            base,
            "field-graph" | "frame-program" | "render-layout" | "report" | "image"
        ) && stage_selected(stage, boot)
    })
}

fn requested_bundle_observations(expected_files: &[PathBuf], boot: BootSel) -> (bool, bool) {
    let requested = |wanted: &str| {
        expected_files.iter().any(|path| {
            path.file_stem().and_then(|stage| stage.to_str()) == Some(wanted)
                && stage_selected(wanted, boot)
        })
    };
    (requested("check"), requested("typed"))
}

fn case_needs_pixels_bundle(case: &Path, boot: BootSel) -> Result<bool, String> {
    if !accepted_pixels_case(case) {
        return Ok(false);
    }
    let expected_dir = case.join("expected");
    let mut expected_files = read_dir_paths(&expected_dir)?;
    expected_files.sort();
    Ok(expected_files_need_pixels_bundle(&expected_files, boot))
}

fn accepted_pixels_bundle_cache_complete(
    case: &Path,
    boot: BootSel,
    context: &GoldenStageKeyContext,
) -> Result<bool, String> {
    if !case_needs_pixels_bundle(case, boot)? {
        return Ok(false);
    }
    let Some(input) = golden_case_target(case)? else {
        return Ok(false);
    };
    let mut expected_files = read_dir_paths(&case.join("expected"))?;
    expected_files.sort();
    let cache = Cache::golden_stage();
    let sources = GoldenSourceDigests::new(case, &input)?;
    let mut found = 0usize;
    for exp in expected_files {
        let stage = exp
            .file_stem()
            .and_then(|stage| stage.to_str())
            .ok_or_else(|| format!("bad expected file name: {}", exp.display()))?;
        let (command_stage, renderer) = renderer_dump_stage(stage)?;
        if !stage_selected(stage, boot) || !bundle_cache_stage(command_stage) {
            continue;
        }
        found += 1;
        let key = golden_stage_cache_key(case, &input, command_stage, renderer, &sources, context)?;
        if cache.get(&key).is_none() {
            return Ok(false);
        }
    }
    Ok(found > 0)
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
    pixels_telemetry: bool,
    boot_key_context: Option<&GoldenBootKeyContext>,
    stage_key_context: &GoldenStageKeyContext,
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
        let case_name = case
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("case")
            .to_string();
        let accepted_pixels = accepted_pixels_case(case);
        let source_digests = GoldenSourceDigests::new(case, &input)?;
        let bundle_needed =
            accepted_pixels && expected_files_need_pixels_bundle(&expected_files, boot);
        let (observe_check, observe_typed) = requested_bundle_observations(&expected_files, boot);
        let stage_cache = Cache::golden_stage();
        let mut bundle_cache_keys = BTreeMap::new();
        let mut cached_bundle_outputs = BTreeMap::new();
        if bundle_needed {
            for exp in &expected_files {
                let stage = exp
                    .file_stem()
                    .and_then(|stage| stage.to_str())
                    .ok_or_else(|| format!("bad expected file name: {}", exp.display()))?;
                let (command_stage, renderer) = renderer_dump_stage(stage)?;
                if !stage_selected(stage, boot) || !bundle_cache_stage(command_stage) {
                    continue;
                }
                let key = golden_stage_cache_key(
                    case,
                    &input,
                    command_stage,
                    renderer,
                    &source_digests,
                    stage_key_context,
                )?;
                if let Some(actual) = stage_cache.get(&key) {
                    cached_bundle_outputs.insert(exp.clone(), actual);
                }
                bundle_cache_keys.insert(exp.clone(), key);
            }
        }
        let bundle_cache_complete = !bundle_cache_keys.is_empty()
            && bundle_cache_keys
                .keys()
                .all(|exp| cached_bundle_outputs.contains_key(exp));
        let pixels_bundle = if bundle_needed && !bundle_cache_complete {
            let _heavy = heavy_gate().acquire();
            Some(crate::produce_image_artifacts_with_observations(
                &input,
                observe_check,
                observe_typed,
            )?)
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
            let bundled_cache_hit = cached_bundle_outputs.contains_key(&exp);
            let bundled_actual = cached_bundle_outputs.remove(&exp).or_else(|| {
                pixels_bundle
                    .as_ref()
                    .and_then(|bundle| match command_stage {
                        "check" => bundle.check.clone(),
                        "typed" => bundle.typed.clone(),
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
                    })
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
                    if !bundled_cache_hit && let Some(key) = bundle_cache_keys.get(&exp) {
                        stage_cache.put(key, &actual);
                    }
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
                } else if !bundled_cache_hit && let Some(key) = bundle_cache_keys.get(&exp) {
                    stage_cache.put(key, &actual);
                }
                continue;
            }
            let ordinary_stage_cache_key = if golden_dump_stage_cacheable(&stage) {
                Some(golden_stage_cache_key(
                    case,
                    &input,
                    command_stage,
                    renderer,
                    &source_digests,
                    stage_key_context,
                )?)
            } else {
                None
            };
            let ordinary_cached_actual = ordinary_stage_cache_key
                .as_ref()
                .and_then(|key| stage_cache.get(key));
            let ordinary_cache_hit = ordinary_cached_actual.is_some();
            let boot_cache_eligible = golden_boot_cache_eligible(&stage);
            let boot_cache = Cache::golden_boot();
            let mut boot_cache_key = None;
            let mut cached_actual = ordinary_cached_actual;
            let mut boot_cache_hit = false;
            if boot_cache_eligible {
                let context = boot_key_context.ok_or_else(|| {
                    format!("{} [{stage}]: boot cache context missing", case.display())
                })?;
                let lookup = golden_boot_image_digest_lookup(
                    case,
                    &input,
                    &stage,
                    pixels_telemetry,
                    wrela,
                    vmm.ok_or_else(|| {
                        format!("{} [{stage}]: boot cache VMM missing", case.display())
                    })?,
                    context,
                )?;
                if let Some(image_digest) = &lookup.digest {
                    let key = golden_boot_image_key(
                        &case_name,
                        &stage,
                        pixels_telemetry,
                        image_digest,
                        &context.vmm,
                    );
                    let valid = |actual: &String| {
                        pixels_test_report_green(actual)
                            && !actual.lines().any(|line| line.contains(": FAILED"))
                    };
                    let image_cached = boot_cache.get(&key).filter(&valid);
                    boot_cache_hit = image_cached.is_some();
                    // One-time migration from the source/compiler key used
                    // before emitted-image identity became authoritative. The
                    // migrated entry is published below only after the normal
                    // exact expectation comparison repeats successfully.
                    let legacy_key =
                        golden_boot_cache_key(case, &input, &stage, pixels_telemetry, context)?;
                    cached_actual =
                        image_cached.or_else(|| boot_cache.get(&legacy_key).filter(valid));
                    boot_cache_key = Some(key);
                }
            }
            let _heavy = if cached_actual.is_none() && heavy_pixels_stage(&case_name, command_stage)
            {
                Some(heavy_gate().acquire())
            } else {
                None
            };
            let out = if cached_actual.is_some() {
                None
            } else if stage == "test" || stage == "test-omit-dmb" {
                let vmm = vmm.ok_or_else(|| {
                    format!(
                        "{} [{stage}]: boot stage selected without a VMM",
                        case.display()
                    )
                })?;
                let mut cmd = wrela_command(wrela);
                cmd.current_dir(root()).arg("test").arg(rel_input);
                if let Some(slots) = vmm_slots {
                    cmd.env("WRELA_VMM_SLOT_DIR", slots);
                }
                if stage == "test-omit-dmb" {
                    cmd.arg("--omit-dmb");
                }
                if pixels_telemetry {
                    cmd.arg("--pixels-telemetry");
                }
                cmd.arg("--vmm")
                    .arg(&vmm)
                    .output()
                    .map(Some)
                    .map_err(|e| format!("run wrela: {e}"))?
            } else if stage == "build" || stage == "build-err" {
                wrela_command(wrela)
                    .current_dir(root())
                    .arg("build")
                    .arg(rel_input)
                    .arg("--out-dir")
                    .arg(&build_out_dir_rel)
                    .output()
                    .map(Some)
                    .map_err(|e| format!("run wrela: {e}"))?
            } else {
                let mut command = wrela_command(wrela);
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
                    .map(Some)
                    .map_err(|e| format!("run wrela: {e}"))?
            };
            let process_succeeded = out.as_ref().is_none_or(|out| out.status.success());
            if stage == "build-err" {
                if process_succeeded {
                    failures.push(format!(
                        "{} [build-err]: wrela build unexpectedly exited successfully",
                        case.display()
                    ));
                    continue;
                }
            } else if stage != "test" && stage != "test-omit-dmb" && !process_succeeded {
                let out = out.as_ref().expect("only command output can fail");
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
            let actual = match cached_actual {
                Some(actual) => actual,
                None => {
                    let out = out.as_ref().expect("a cache miss ran the command");
                    let mut actual = String::from_utf8_lossy(&out.stdout).into_owned();
                    actual.push_str(&String::from_utf8_lossy(&out.stderr));
                    actual
                }
            };
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
            // A Pixels acceptance fixture may never record a failing test as
            // its expectation: `--update` would silently bless the failure
            // and every later verify run would ratify it as a green golden.
            // Deliberate-failure fixtures live outside these prefixes
            // (boot-hello, check-tests-*), and rejection fixtures use `err-`.
            if matches!(stage.as_str(), "test" | "test-omit-dmb") {
                let case_name = case
                    .file_name()
                    .and_then(|name| name.to_str())
                    .unwrap_or_default();
                let pixels_acceptance =
                    case_name.starts_with("check-pixels-") || case_name.starts_with("boot-pixels-");
                // A green run is the only acceptable expectation. Checking for
                // a `FAILED` line alone is not enough: a build error, a VMM
                // wall-cap timeout, or an empty transcript carries no `FAILED`
                // at all, so `--update` would happily record the breakage and
                // every later verify would ratify it as a green golden. Require
                // the passing summary positively instead.
                let reports_green = pixels_test_report_green(&actual);
                let reports_failure = actual.lines().any(|line| line.contains(": FAILED"));
                if pixels_acceptance && (reports_failure || !reports_green) {
                    let reason = if reports_failure {
                        "reports a failing test"
                    } else {
                        "did not complete a passing run (build error, timeout, or empty transcript)"
                    };
                    failures.push(format!(
                        "{} [{stage}]: pixels acceptance fixture {reason}; \
                         refusing to {} this transcript:\n{actual}",
                        case.display(),
                        if update { "record" } else { "accept" },
                    ));
                    continue;
                }
            }
            let expectation_matched = if update {
                std::fs::write(&exp, &actual)
                    .map_err(|e| format!("write {}: {e}", exp.display()))?;
                true
            } else {
                let expected = std::fs::read_to_string(&exp)
                    .map_err(|e| format!("read {}: {e}", exp.display()))?;
                if actual != expected {
                    failures.push(format!(
                        "{} [{stage}]: output differs from expectation\n--- expected\n{expected}--- actual\n{actual}",
                        case.display()
                    ));
                    false
                } else {
                    true
                }
            };
            if let Some(key) = boot_cache_key
                && !boot_cache_hit
                && golden_boot_cache_publishable(
                    boot_cache_eligible,
                    process_succeeded,
                    &actual,
                    expectation_matched,
                )
            {
                boot_cache.put(&key, &actual);
            }
            // A diagnostic that exactly matches an intentional rejection
            // fixture is a successful golden observation even though the CLI
            // correctly exits non-zero. The cache makes no status inference:
            // a binary change invalidates the key, and every hit still repeats
            // the exact expectation comparison above.
            if let Some(key) = ordinary_stage_cache_key
                && !ordinary_cache_hit
                && expectation_matched
                && (process_succeeded
                    || actual
                        .lines()
                        .next()
                        .is_some_and(|line| line.starts_with("error[")))
            {
                stage_cache.put(&key, &actual);
            }
            if update {
                continue;
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
    pixels_telemetry: bool,
    boot_key_context: Option<&GoldenBootKeyContext>,
    stage_key_context: &GoldenStageKeyContext,
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
                    let started = std::time::Instant::now();
                    let result = run_case(
                        &cases[i],
                        wrela,
                        vmm,
                        vmm_slots,
                        update,
                        boot,
                        pixels_telemetry,
                        boot_key_context,
                        stage_key_context,
                    );
                    if std::env::var_os("WRELA_GOLDEN_TIMINGS").is_some() {
                        eprintln!(
                            "golden-timing: {:.3}s {}",
                            started.elapsed().as_secs_f64(),
                            cases[i].display(),
                        );
                    }
                    match result {
                        Ok((n, f)) => results.lock().expect("results lock").push((i, n, f)),
                        Err(e) => {
                            let mut slot = hard_error.lock().expect("error lock");
                            if slot.is_none() {
                                *slot = Some(format!("{}: {e}", cases[i].display()));
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

fn isolated_pixels_child_args(case_name: &str, update: bool) -> Vec<String> {
    let mut args = vec![
        "golden".to_string(),
        "--case".to_string(),
        case_name.to_string(),
        "--no-boot".to_string(),
        "--jobs".to_string(),
        "1".to_string(),
        "--assume-built".to_string(),
        "--isolated-child".to_string(),
    ];
    if update {
        args.push("--update".to_string());
    }
    args
}

fn isolated_golden_expectation_count(output: &str) -> Result<usize, String> {
    let counts: Vec<usize> = output
        .lines()
        .filter_map(|line| line.strip_prefix("golden: "))
        .filter_map(|rest| {
            let mut words = rest.split_whitespace();
            let first = words.next()?;
            if first == "updated" {
                words.next()?.parse::<usize>().ok()
            } else {
                first.parse::<usize>().ok()
            }
        })
        .collect();
    match counts.as_slice() {
        [count] if *count > 0 => Ok(*count),
        _ => Err(format!(
            "isolated golden child emitted no unique positive expectation count:\n{output}"
        )),
    }
}

fn run_isolated_pixels_cases(cases: &[PathBuf], update: bool) -> Result<usize, String> {
    if cases.is_empty() {
        return Ok(0);
    }
    let executable = std::env::current_exe()
        .map_err(|error| format!("locate current xtask executable: {error}"))?;
    let run_one = |case: &Path| -> Result<usize, String> {
        let name = case
            .file_name()
            .and_then(|name| name.to_str())
            .ok_or_else(|| format!("bad golden case path: {}", case.display()))?;
        // Each child compiles one renderer bundle in-process; a heavy permit
        // bounds how many such children run at once.
        let _heavy = heavy_gate().acquire();
        let output = Command::new(&executable)
            .current_dir(root())
            .args(isolated_pixels_child_args(name, update))
            .output()
            .map_err(|error| format!("run isolated golden case `{name}`: {error}"))?;
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        if !output.status.success() {
            return Err(format!(
                "isolated golden case `{name}` failed (status {}):\n{stdout}{stderr}",
                output
                    .status
                    .code()
                    .map_or_else(|| "signal".to_string(), |code| code.to_string()),
            ));
        }
        isolated_golden_expectation_count(&stdout)
            .map_err(|error| format!("isolated golden case `{name}`: {error}"))
    };
    let workers = heavy_pixels_jobs().min(cases.len());
    let cursor = std::sync::atomic::AtomicUsize::new(0);
    let results: Vec<std::sync::Mutex<Option<Result<usize, String>>>> =
        cases.iter().map(|_| std::sync::Mutex::new(None)).collect();
    std::thread::scope(|scope| {
        for _ in 0..workers {
            scope.spawn(|| {
                loop {
                    let index = cursor.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    let Some(case) = cases.get(index) else {
                        return;
                    };
                    let run = run_one(case);
                    *results[index].lock().unwrap_or_else(|e| e.into_inner()) = Some(run);
                }
            });
        }
    });
    let mut expectations = 0usize;
    for slot in results {
        expectations += slot
            .into_inner()
            .unwrap_or_else(|e| e.into_inner())
            .unwrap_or_else(|| Err("isolated golden case result missing".to_string()))?;
    }
    Ok(expectations)
}

fn should_isolate_pixels_bundles(opts: &GoldenOpts) -> bool {
    opts.isolate_pixels_bundles
        || (opts.boot == BootSel::None && !opts.isolated_child && !opts.pixels_telemetry)
}

pub(crate) fn golden(opts: &GoldenOpts) -> Result<(), String> {
    if opts.clear_boot_cache {
        Cache::golden_boot().clear()?;
        println!("golden: successful boot transcript cache cleared");
    }
    // `cargo xtask` has already built this executable, which embeds the exact
    // `wrela` CLI dispatcher above. Reusing it removes a redundant cold link
    // and, unlike a hard-coded `target/debug/wrela`, remains correct under
    // CARGO_TARGET_DIR. Isolated child invocations keep `--assume-built` for
    // CLI compatibility; there is simply no separate binary to preflight.
    let wrela = std::env::current_exe()
        .map_err(|error| format!("golden: locate current task-runner executable: {error}"))?;
    let stage_key_context = GoldenStageKeyContext::new(&wrela)?;
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
        if let Some(cases) = &opts.cases {
            if !cases.iter().any(|case| case == &name) {
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

    if let Some(cases) = &opts.cases {
        let missing: Vec<&String> = cases
            .iter()
            .filter(|case| !selected_names.iter().any(|name| &name == case))
            .collect();
        if !missing.is_empty() {
            return Err(format!(
                "golden: requested cases matched no selected expectation under tests/golden/: \
                 {missing:?}"
            ));
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
    let scan_cases: Vec<PathBuf> = dump_cases
        .iter()
        .chain(boot_cases.iter())
        .cloned()
        .collect();
    // Cross-core scheduler fixtures deliberately fail closed when host
    // contention prevents their secondary vCPUs from reaching quiescence.
    // P8's sustained multi-vCPU Pixels guests can otherwise overlap them for
    // minutes. Run this small group after the ordinary guest pool, one at a
    // time, without giving up parallelism for the rest of the boot corpus.
    let mut scheduling_sensitive_boot_cases = Vec::new();
    boot_cases.retain(|case| {
        if scheduling_sensitive_boot_case(case) {
            scheduling_sensitive_boot_cases.push(case.clone());
            false
        } else {
            true
        }
    });

    // Fresh renderer bundles are faster in short-lived children: each large
    // compiler arena returns to the OS immediately, cutting allocator and VM
    // pressure across the uncached corpus. Child invocations carry
    // the private recursion guard while `--assume-built` remains a harmless
    // compatibility flag for direct maintainer invocations.
    let isolate_pixels_bundles = should_isolate_pixels_bundles(opts);
    let isolated_dump_cases = if isolate_pixels_bundles {
        if opts.pixels_telemetry || opts.boot != BootSel::None {
            return Err(
                "golden: Pixels bundle isolation only supports non-telemetry, non-boot runs"
                    .to_string(),
            );
        }
        let mut isolated = Vec::new();
        let mut ordinary = Vec::new();
        for case in dump_cases {
            if case_needs_pixels_bundle(&case, opts.boot)?
                && !accepted_pixels_bundle_cache_complete(&case, opts.boot, &stage_key_context)?
            {
                isolated.push(case);
            } else {
                ordinary.push(case);
            }
        }
        dump_cases = ordinary;
        isolated
    } else {
        Vec::new()
    };

    let vmm = if boot_cases.is_empty() && scheduling_sensitive_boot_cases.is_empty() {
        None
    } else {
        Some(build_and_sign_vmm()?)
    };
    let boot_key_context = vmm
        .as_deref()
        .map(|vmm| GoldenBootKeyContext::new(&wrela, vmm))
        .transpose()?;
    let vmm_slot_dir = if boot_cases.is_empty() && scheduling_sensitive_boot_cases.is_empty() {
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
    // Isolated pixels children and the ordinary dump pool share the heavy
    // gate, so running them concurrently cannot exceed the memory envelope.
    let (isolated_result, dump_result) = std::thread::scope(|scope| {
        let isolated = scope.spawn(|| run_isolated_pixels_cases(&isolated_dump_cases, opts.update));
        let dump = run_cases_parallel(
            &dump_cases,
            &wrela,
            vmm.as_deref(),
            None,
            opts.update,
            opts.boot,
            opts.jobs,
            opts.pixels_telemetry,
            boot_key_context.as_ref(),
            &stage_key_context,
        );
        let isolated = isolated
            .join()
            .unwrap_or_else(|_| Err("golden: isolated pixels worker panicked".to_string()));
        (isolated, dump)
    });
    let isolated_expectations = isolated_result?;
    let (n1, mut failures) = dump_result?;
    let (n2, boot_failures) = run_cases_parallel(
        &boot_cases,
        &wrela,
        vmm.as_deref(),
        vmm_slot_dir.as_deref(),
        opts.update,
        opts.boot,
        opts.jobs,
        opts.pixels_telemetry,
        boot_key_context.as_ref(),
        &stage_key_context,
    )?;
    let (n3, scheduling_sensitive_failures) = run_cases_parallel(
        &scheduling_sensitive_boot_cases,
        &wrela,
        vmm.as_deref(),
        vmm_slot_dir.as_deref(),
        opts.update,
        opts.boot,
        1,
        opts.pixels_telemetry,
        boot_key_context.as_ref(),
        &stage_key_context,
    )?;
    if let Some(dir) = &vmm_slot_dir {
        std::fs::remove_dir_all(dir)
            .map_err(|error| format!("remove {}: {error}", dir.display()))?;
    }
    failures.extend(boot_failures);
    failures.extend(scheduling_sensitive_failures);
    let cases = isolated_expectations + n1 + n2 + n3;

    if !failures.is_empty() {
        for f in &failures {
            eprintln!("{f}\n");
        }
        return Err(format!("golden: {} failure(s)", failures.len()));
    }
    assert_no_internal_error_in_goldens(&scan_cases)?;
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

pub(crate) fn assert_no_internal_error_in_goldens(cases: &[PathBuf]) -> Result<(), String> {
    const PREFIX: &str = "internal error: ";
    let mut hits = Vec::new();
    for case in cases {
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
    fn accepted_pixels_bundles_exclude_placeholder_boot_cases() {
        let cases = root().join("tests/golden");
        assert!(accepted_pixels_case(&cases.join("check-pixels-plane")));
        assert!(accepted_pixels_case(&cases.join("boot-pixels-plane")));
        assert!(!accepted_pixels_case(&cases.join("boot-pixels-gi")));
        assert!(!accepted_pixels_case(&cases.join("check-placement")));
    }

    #[test]
    fn bundle_semantic_observations_are_demand_driven() {
        let paths = |names: &[&str]| {
            names
                .iter()
                .map(|name| PathBuf::from(format!("expected/{name}.txt")))
                .collect::<Vec<_>>()
        };
        assert_eq!(
            requested_bundle_observations(&paths(&["report"]), BootSel::None),
            (false, false)
        );
        assert_eq!(
            requested_bundle_observations(&paths(&["check", "report"]), BootSel::None),
            (true, false)
        );
        assert_eq!(
            requested_bundle_observations(&paths(&["typed", "report"]), BootSel::None),
            (false, true)
        );
        assert_eq!(
            requested_bundle_observations(&paths(&["check", "typed"]), BootSel::Only),
            (false, false)
        );
    }

    #[test]
    fn uncached_static_renderer_bundles_release_their_process_arenas() {
        let static_run = GoldenOpts {
            boot: BootSel::None,
            ..GoldenOpts::default()
        };
        assert!(should_isolate_pixels_bundles(&static_run));

        let child = GoldenOpts {
            boot: BootSel::None,
            isolated_child: true,
            ..GoldenOpts::default()
        };
        assert!(
            !should_isolate_pixels_bundles(&child),
            "isolated children must compile in-process instead of recursing"
        );

        let compatibility_flag = GoldenOpts {
            boot: BootSel::None,
            assume_built: true,
            ..GoldenOpts::default()
        };
        assert!(
            should_isolate_pixels_bundles(&compatibility_flag),
            "--assume-built must not disable the faster cold-static path"
        );
    }

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

    #[test]
    fn pixels_green_report_requires_at_least_one_executed_test() {
        assert!(pixels_test_report_green("ok\n1 passed, 0 failed\n"));
        assert!(pixels_test_report_green("12 passed, 0 failed"));
        assert!(!pixels_test_report_green("0 passed, 0 failed\n"));
        assert!(!pixels_test_report_green("1 passed, 1 failed\n"));
        assert!(!pixels_test_report_green("not passed, 0 failed\n"));
    }

    #[test]
    fn boot_cache_selects_only_guest_test_stages_and_rejects_failures() {
        assert!(golden_boot_cache_eligible("test"));
        assert!(golden_boot_cache_eligible("test-omit-dmb"));
        assert!(!golden_boot_cache_eligible("report"));

        let green = "case: ok\n1 passed, 0 failed\n";
        assert!(golden_boot_cache_publishable(true, true, green, true));
        assert!(!golden_boot_cache_publishable(false, true, green, true));
        assert!(!golden_boot_cache_publishable(true, false, green, true));
        assert!(!golden_boot_cache_publishable(
            true,
            true,
            "case: FAILED\n0 passed, 1 failed\n",
            true
        ));
        assert!(!golden_boot_cache_publishable(true, true, green, false));
    }

    #[test]
    fn accepted_pixels_boot_key_covers_every_behavioral_input() {
        let values = [
            "case",
            "target",
            "test",
            "false",
            "case-src",
            "target-src",
            "stdlib",
            "bench",
            "census",
            "compiler",
            "vmm",
        ];
        let key = |values: &[&str; 11]| {
            golden_boot_key_from_digests(
                values[0],
                values[1],
                values[2],
                values[3] == "true",
                values[4],
                values[5],
                values[6],
                values[7],
                values[8],
                values[9],
                values[10],
            )
        };
        let baseline = key(&values);
        for index in 0..values.len() {
            let mut changed = values;
            changed[index] = if index == 3 { "true" } else { "changed" };
            assert_ne!(baseline, key(&changed), "component {index} was not keyed");
        }
    }

    #[test]
    fn emitted_image_boot_key_covers_every_runtime_input() {
        let values = ["case", "test", "false", "image", "vmm"];
        let key = |values: &[&str; 5]| {
            golden_boot_image_key(
                values[0],
                values[1],
                values[2] == "true",
                values[3],
                values[4],
            )
        };
        let baseline = key(&values);
        for index in 0..values.len() {
            let mut changed = values;
            changed[index] = if index == 2 { "true" } else { "changed" };
            assert_ne!(baseline, key(&changed), "component {index} was not keyed");
        }
    }

    #[test]
    fn image_digest_probe_is_fail_closed_and_allows_comptime_only_output() {
        let digest = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
        assert_eq!(
            parse_golden_image_digest(&format!("diagnostic\np8-image-digest {digest}\n")),
            Some(digest.to_string())
        );
        assert_eq!(parse_golden_image_digest("1 passed, 0 failed\n"), None);
        assert_eq!(parse_golden_image_digest("p8-image-digest short\n"), None);
    }

    #[test]
    fn accepted_pixels_stage_key_covers_every_behavioral_input() {
        let values = [
            "case",
            "target",
            "report",
            "default",
            "case-src",
            "target-src",
            "stdlib",
            "bench",
            "census",
            "compiler",
            "tool",
        ];
        let key = |values: &[&str; 11]| {
            golden_stage_key_from_digests(
                values[0], values[1], values[2], values[3], values[4], values[5], values[6],
                values[7], values[8], values[9], values[10],
            )
        };
        let baseline = key(&values);
        for index in 0..values.len() {
            let mut changed = values;
            changed[index] = "changed";
            assert_ne!(baseline, key(&changed), "component {index} was not keyed");
        }
        for stage in [
            "report",
            "image",
            "field-graph",
            "frame-program",
            "render-layout",
        ] {
            assert!(bundle_cache_stage(stage));
        }
        assert!(!bundle_cache_stage("test"));
        assert!(!bundle_cache_stage("build"));
    }

    #[test]
    fn stage_producer_fingerprint_covers_every_source_owner() {
        let baseline = golden_tool_digest_from_parts("main", "golden", "cache");
        assert_ne!(
            baseline,
            golden_tool_digest_from_parts("changed", "golden", "cache")
        );
        assert_ne!(
            baseline,
            golden_tool_digest_from_parts("main", "changed", "cache")
        );
        assert_ne!(
            baseline,
            golden_tool_digest_from_parts("main", "golden", "changed")
        );
    }

    #[test]
    fn frequency_sidecars_are_load_bearing_stage_inputs() {
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let dir = std::env::temp_dir().join(format!(
            "wrela-golden-sidecars-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("expected")).expect("create fixture");
        std::fs::write(dir.join("input.wr"), "module sidecar_key\n").expect("write source");
        std::fs::write(dir.join("lane1-freq.txt"), "a = 1\n").expect("write lane1");
        std::fs::write(dir.join("lane2-freq.txt"), "b = 2\n").expect("write lane2");
        std::fs::write(dir.join("expected/cost.txt"), "old expectation\n")
            .expect("write expectation");

        let key = || {
            let source = golden_input_tree_digest(&dir).expect("digest fixture");
            golden_stage_key_from_digests(
                "case", "input.wr", "cost", "default", &source, &source, "stdlib", "bench",
                "census", "compiler", "tool",
            )
        };
        let baseline = key();

        std::fs::write(dir.join("expected/cost.txt"), "new expectation\n")
            .expect("mutate expectation");
        assert_eq!(baseline, key(), "expected output is not an input");

        std::fs::write(dir.join("lane1-freq.txt"), "a = 3\n").expect("mutate lane1");
        let lane1_changed = key();
        assert_ne!(
            baseline, lane1_changed,
            "Lane 1 frequencies must move the key"
        );

        std::fs::write(dir.join("lane2-freq.txt"), "b = 4\n").expect("mutate lane2");
        assert_ne!(lane1_changed, key(), "Lane 2 frequencies must move the key");
        std::fs::remove_dir_all(&dir).expect("remove fixture");
    }

    #[test]
    fn driver_source_changes_invalidate_every_golden_cache_layer() {
        let dir =
            std::env::temp_dir().join(format!("wrela-golden-stdlib-key-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("core")).expect("create core fixture");
        std::fs::create_dir_all(dir.join("drivers")).expect("create drivers fixture");
        std::fs::write(dir.join("core/base.wr"), "module core.base\n").expect("write core fixture");
        let driver = dir.join("drivers/display.wr");
        std::fs::write(&driver, "module drivers.display\nconst VERSION: u32 = 1\n")
            .expect("write driver fixture");

        let before = production_stdlib_digest(&dir).expect("digest fixture");
        let stage_before = golden_stage_key_from_digests(
            "case",
            "target",
            "report",
            "default",
            "case-src",
            "target-src",
            &before,
            "bench",
            "census",
            "compiler",
            "tool",
        );
        let probe_before = golden_image_digest_key_from_digests(
            "case",
            "target",
            "test",
            false,
            "case-src",
            "target-src",
            &before,
            "bench",
            "census",
            "compiler",
        );
        let legacy_boot_before = golden_boot_key_from_digests(
            "case",
            "target",
            "test",
            false,
            "case-src",
            "target-src",
            &before,
            "bench",
            "census",
            "compiler",
            "vmm",
        );

        std::fs::write(&driver, "module drivers.display\nconst VERSION: u32 = 2\n")
            .expect("mutate driver fixture");
        let after = production_stdlib_digest(&dir).expect("digest mutated fixture");
        assert_ne!(before, after, "driver content must affect stdlib identity");
        assert_ne!(
            stage_before,
            golden_stage_key_from_digests(
                "case",
                "target",
                "report",
                "default",
                "case-src",
                "target-src",
                &after,
                "bench",
                "census",
                "compiler",
                "tool",
            )
        );
        assert_ne!(
            probe_before,
            golden_image_digest_key_from_digests(
                "case",
                "target",
                "test",
                false,
                "case-src",
                "target-src",
                &after,
                "bench",
                "census",
                "compiler",
            )
        );
        assert_ne!(
            legacy_boot_before,
            golden_boot_key_from_digests(
                "case",
                "target",
                "test",
                false,
                "case-src",
                "target-src",
                &after,
                "bench",
                "census",
                "compiler",
                "vmm",
            )
        );
        std::fs::remove_dir_all(&dir).expect("remove stdlib fixture");
    }

    #[test]
    fn dump_cache_excludes_every_stage_with_external_side_effects() {
        for stage in ["test", "test-omit-dmb", "build", "build-err", "img"] {
            assert!(!golden_dump_stage_cacheable(stage), "{stage}");
        }
        for stage in ["report", "typed", "mwir", "cost", "frame-program"] {
            assert!(golden_dump_stage_cacheable(stage), "{stage}");
        }
    }

    #[test]
    fn embedded_wrela_subprocess_uses_the_internal_cli_prefix() {
        let command = wrela_command(Path::new("/tmp/wrela-xtask-fixture"));
        assert_eq!(command.get_program(), "/tmp/wrela-xtask-fixture");
        assert_eq!(
            command.get_args().collect::<Vec<_>>(),
            [std::ffi::OsStr::new("__wrela")]
        );
    }

    #[test]
    fn isolated_pixels_child_selects_one_exact_non_boot_case() {
        assert_eq!(
            isolated_pixels_child_args("check-pixels-repeat", false),
            [
                "golden",
                "--case",
                "check-pixels-repeat",
                "--no-boot",
                "--jobs",
                "1",
                "--assume-built",
                "--isolated-child",
            ]
        );
        assert_eq!(
            isolated_pixels_child_args("check-pixels-repeat", true).last(),
            Some(&"--update".to_string())
        );
    }

    #[test]
    fn isolated_pixels_child_count_fails_closed() {
        assert_eq!(
            isolated_golden_expectation_count(
                "golden: 7 expectation(s) ok (1 case(s), boot stages skipped)\n"
            ),
            Ok(7)
        );
        assert_eq!(
            isolated_golden_expectation_count(
                "golden: updated 7 expectation(s) — review the diff before committing\n"
            ),
            Ok(7)
        );
        assert!(isolated_golden_expectation_count("golden: 0 expectation(s) ok\n").is_err());
        assert!(isolated_golden_expectation_count("unrelated output\n").is_err());
        assert!(
            isolated_golden_expectation_count(
                "golden: 2 expectation(s) ok\ngolden: 3 expectation(s) ok\n"
            )
            .is_err()
        );
    }

    #[test]
    fn only_cross_core_boot_cases_use_the_scheduling_sensitive_lane() {
        assert!(scheduling_sensitive_boot_case(Path::new(
            "tests/golden/boot-cross-core-ring-full"
        )));
        assert!(!scheduling_sensitive_boot_case(Path::new(
            "tests/golden/check-pixels-hard-csg"
        )));
        assert!(!scheduling_sensitive_boot_case(Path::new(
            "tests/golden/boot-async"
        )));
    }
}
