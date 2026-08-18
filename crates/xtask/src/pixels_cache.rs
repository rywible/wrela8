//! Versioned, fail-open-to-a-miss caches for the Pixels conformance loop.
//!
//! Content-addressed caches live here, all structurally unable
//! to change a result:
//!
//! - the **compile-closure cache** maps a source closure to the instrumented
//!   image digest, skipping the `wrela test --image-digest-only` compilation
//!   that precedes the boot-cache decision;
//! - the **score cache** maps a case's complete recorded evidence to the
//!   `FrameScore` the scorer derived from it;
//! - the **golden boot cache** stores only positive, successful guest-test
//!   transcripts already proven green and byte-identical to their expectation;
//! - the **golden stage cache** stores side-effect-free compiler dump text
//!   already matched byte-for-byte (including intentional rejection
//!   diagnostics, which are successful golden observations rather than cached
//!   compiler success claims).
//! - the **census cache** stores hot-path reports only after two independent
//!   build directories produced byte-identical text.
//!
//! The generic artifacts obey this contract, and the reason each clause exists:
//!
//! - **versioned artifact with a schema header.** A format change that
//!   silently reused old bytes would be a wrong result, so the header is
//!   checked before the body is parsed and a mismatch is a miss.
//! - **atomic staged publication.** The same-directory temporary is linked
//!   into place atomically (the no-replace equivalent of tmp+rename), so a
//!   reader never observes a half-written artifact and a crash costs a miss.
//! - **concurrent writers race safely.** Two workers computing the same key
//!   write identical bytes; the loser's rename is harmless. A *different*
//!   artifact under the same name fails its embedded key or body digest on
//!   read and is a miss.
//! - **corruption or schema mismatch is a miss, never an error.** A cache is
//!   an optimization; it may never be the reason a run fails.
//! - **failures are never cached.** Only a successfully derived value is
//!   stored, so a transient failure cannot become permanent.
//! - **`WRELA_P8_*_CACHE=0` disables each cache**, and
//!   `cargo xtask pixels-conformance --clear-caches` removes them.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use crate::root;

/// Artifact format version. Bump on any change to the header or body shape.
pub(crate) const CACHE_SCHEMA: u32 = 1;

const MAGIC: &str = "wrela-p8-cache";

/// One named content-addressed cache.
#[derive(Debug, Clone)]
pub(crate) struct Cache {
    name: &'static str,
    env: &'static str,
    dir: PathBuf,
    legacy_dir: Option<PathBuf>,
}

impl Cache {
    fn root_dir() -> PathBuf {
        // Semantic result caches are not Cargo build products. Keeping them
        // outside `target/` means `cargo clean` produces a genuinely cold
        // Rust build without also throwing away independently keyed compiler
        // results; every cache still has an explicit disable and clear path.
        root().join(".wrela-cache")
    }

    fn named(name: &'static str, env: &'static str, leaf: &str) -> Cache {
        Cache {
            name,
            env,
            dir: Self::root_dir().join(leaf),
            // Read-through migration for caches written before semantic
            // results moved out of Cargo's build directory. Parsing still
            // verifies schema, cache name, key, and body digest before a hit
            // is promoted to the persistent location.
            legacy_dir: Some(root().join("target").join(leaf)),
        }
    }

    pub(crate) fn compile_closure() -> Cache {
        Self::named(
            "compile-closure",
            "WRELA_P8_COMPILE_CACHE",
            "p8-compile-cache",
        )
    }

    pub(crate) fn score() -> Cache {
        Self::named("score", "WRELA_P8_SCORE_CACHE", "p8-score-cache")
    }

    /// The pre-existing boot cache: raw guest artifacts keyed by image and
    /// VMM digest. It predates this module and keeps its own read/write path
    /// in `pixels_conformance`; what lives here is the one definition of its
    /// directory and switch, so `--clear-caches` and the conformance loop
    /// cannot drift apart.
    pub(crate) fn boot() -> Cache {
        Self::named("boot", "WRELA_P8_BOOT_CACHE", "p8-boot-cache")
    }

    /// Successful positive golden boot transcripts. Unlike the
    /// conformance boot cache, this stores the final exact text that the
    /// golden runner has already proven green and matched against its checked
    /// expectation. A hit still repeats both validations.
    pub(crate) fn golden_boot() -> Cache {
        Self::named(
            "golden-boot",
            "WRELA_P8_GOLDEN_BOOT_CACHE",
            "p8-golden-boot-cache",
        )
    }

    /// Exact side-effect-free compiler dump text, one stage per entry.
    pub(crate) fn golden_stage() -> Cache {
        Self::named(
            "golden-stage",
            "WRELA_P8_GOLDEN_STAGE_CACHE",
            "p8-golden-stage-cache",
        )
    }

    /// Parsed Wrela import-closure identities. This keeps closure-granular
    /// golden keys cheap on repeated runs without broadening the result key:
    /// a stdlib/compiler change invalidates discovery, then unaffected output
    /// stages can still reuse their narrower closure identity.
    pub(crate) fn golden_closure() -> Cache {
        Self::named(
            "golden-closure",
            "WRELA_P8_GOLDEN_CLOSURE_CACHE",
            "p8-golden-closure-cache",
        )
    }

    /// Deterministic hot-path census reports. A miss still performs both
    /// required builds; only their byte-identical result may be published.
    pub(crate) fn census() -> Cache {
        Self::named("census", "WRELA_P8_CENSUS_CACHE", "p8-census-cache")
    }

    pub(crate) fn dir(&self) -> &Path {
        &self.dir
    }

    pub(crate) fn legacy_dir(&self) -> Option<&Path> {
        self.legacy_dir.as_deref()
    }

    /// Every cache this milestone owns, including the boot cache so
    /// `--clear-caches` clears the whole loop rather than part of it.
    pub(crate) fn all() -> Vec<Cache> {
        vec![
            Cache::compile_closure(),
            Cache::score(),
            Cache::boot(),
            Cache::golden_boot(),
            Cache::golden_stage(),
            Cache::golden_closure(),
            Cache::census(),
        ]
    }

    /// The two derived conformance caches introduced by P8R.6. Cache-parity
    /// deliberately toggles only these: the raw signed-guest cache is verified
    /// independently and clearing it would add two unrelated full guest sweeps
    /// to a compile/score cache benchmark.
    fn p8r6_derived() -> [Cache; 2] {
        [Cache::compile_closure(), Cache::score()]
    }

    pub(crate) fn enabled(&self) -> bool {
        std::env::var(self.env)
            .map(|value| value != "0")
            .unwrap_or(true)
    }

    fn path(&self, key: &str) -> PathBuf {
        self.dir.join(format!("{key}.txt"))
    }

    /// Read a cached value, or `None` for any reason at all.
    ///
    /// Every failure mode — absent, unreadable, wrong schema, wrong name,
    /// wrong key, or a body that does not match its recorded digest — is a
    /// miss. A corrupt artifact warns once and is treated as absent.
    pub(crate) fn get(&self, key: &str) -> Option<String> {
        self.get_with_warning(key, |warning| println!("{warning}"))
    }

    fn get_with_warning(&self, key: &str, mut warn: impl FnMut(String)) -> Option<String> {
        if !self.enabled() {
            return None;
        }
        let primary = self.path(key);
        let legacy = self
            .legacy_dir
            .as_ref()
            .map(|dir| dir.join(format!("{key}.txt")));
        for (path, promote) in
            std::iter::once((primary, false)).chain(legacy.into_iter().map(|path| (path, true)))
        {
            let text = match std::fs::read_to_string(&path) {
                Ok(text) => text,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
                Err(error) => {
                    warn(format!(
                        "pixels-cache: {} artifact {} ignored (read failed: {error})",
                        self.name,
                        path.display()
                    ));
                    continue;
                }
            };
            match parse_artifact(self.name, key, &text) {
                Ok(body) => {
                    if promote {
                        self.put(key, &body);
                    }
                    return Some(body);
                }
                Err(why) => warn(format!(
                    "pixels-cache: {} artifact {} ignored ({why})",
                    self.name,
                    path.display()
                )),
            }
        }
        None
    }

    /// Store a value. A write failure is reported but is never fatal: the
    /// caller already holds the correct value.
    pub(crate) fn put(&self, key: &str, body: &str) {
        if !self.enabled() {
            return;
        }
        if let Err(why) = self.try_put(key, body) {
            println!("pixels-cache: {} write skipped ({why})", self.name);
        }
    }

    fn try_put(&self, key: &str, body: &str) -> Result<(), String> {
        std::fs::create_dir_all(&self.dir)
            .map_err(|error| format!("create {}: {error}", self.dir.display()))?;
        let path = self.path(key);
        let staged = self.dir.join(format!(
            "{key}.{}.{:?}.tmp",
            std::process::id(),
            std::thread::current().id()
        ));
        let artifact = render_artifact(self.name, key, body);
        std::fs::write(&staged, &artifact)
            .map_err(|error| format!("write {}: {error}", staged.display()))?;
        // Publish without replacing an existing winner. A hard link in the
        // same directory is an atomic create: exactly one concurrent writer
        // wins, and every reader sees a complete artifact.
        match std::fs::hard_link(&staged, &path) {
            Ok(()) => {
                let _ = std::fs::remove_file(&staged);
                Ok(())
            }
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                let existing = std::fs::read_to_string(&path).ok();
                let _ = std::fs::remove_file(&staged);
                if existing.as_deref() == Some(artifact.as_str()) {
                    return Ok(());
                }
                // Two successful computations for one key must be identical.
                // Remove the disputed entry so the next reader recomputes;
                // the current caller already owns its independently derived
                // correct value.
                let _ = std::fs::remove_file(&path);
                Err(format!(
                    "concurrent value mismatch for {} key {key}; disputed artifact removed",
                    self.name
                ))
            }
            Err(error) => {
                let _ = std::fs::remove_file(&staged);
                Err(format!("publish {}: {error}", path.display()))
            }
        }
    }

    pub(crate) fn clear(&self) -> Result<(), String> {
        for dir in std::iter::once(&self.dir).chain(self.legacy_dir.iter()) {
            match std::fs::remove_dir_all(dir) {
                Ok(()) => {}
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => return Err(format!("clear {}: {error}", dir.display())),
            }
        }
        Ok(())
    }
}

fn render_artifact(name: &str, key: &str, body: &str) -> String {
    format!(
        "{MAGIC} {CACHE_SCHEMA}\nname {name}\nkey {key}\nbody {}\n--\n{body}",
        wrela_compiler::report::sha256_hex(body.as_bytes())
    )
}

fn parse_artifact(name: &str, key: &str, text: &str) -> Result<String, String> {
    let (header, body) = text
        .split_once("\n--\n")
        .ok_or_else(|| "truncated artifact".to_string())?;
    let mut lines = header.lines();
    let first = lines.next().ok_or_else(|| "empty header".to_string())?;
    let (magic, schema) = first
        .split_once(' ')
        .ok_or_else(|| "malformed magic line".to_string())?;
    if magic != MAGIC {
        return Err(format!("foreign artifact `{magic}`"));
    }
    if schema.parse::<u32>().ok() != Some(CACHE_SCHEMA) {
        return Err(format!("schema `{schema}` is not {CACHE_SCHEMA}"));
    }
    let field = |want: &str, line: Option<&str>| -> Result<String, String> {
        line.and_then(|line| line.strip_prefix(want))
            .map(|value| value.trim().to_string())
            .ok_or_else(|| format!("missing `{}` field", want.trim()))
    };
    if field("name ", lines.next())? != name {
        return Err("artifact belongs to another cache".to_string());
    }
    if field("key ", lines.next())? != key {
        return Err("artifact key does not match its file name".to_string());
    }
    let digest = field("body ", lines.next())?;
    if digest != wrela_compiler::report::sha256_hex(body.as_bytes()) {
        return Err("body digest mismatch".to_string());
    }
    Ok(body.to_string())
}

/// Build a cache key from named components.
///
/// Every component is named, so a key derivation that forgets one is visible
/// in the debug listing rather than silently colliding, and the digest covers
/// the names as well as the values.
pub(crate) fn key_of(components: &[(&str, String)]) -> String {
    let mut text = String::new();
    for (name, value) in components {
        text.push_str(name);
        text.push('=');
        text.push_str(value);
        text.push('\n');
    }
    wrela_compiler::report::sha256_hex(text.as_bytes())
}

fn check_pinned_cache_parity(pinned: &str, artifact_sha256: &str) -> Result<(), String> {
    let stable = [
        "# pixels conformance cache parity".to_string(),
        format!("schema = {CACHE_SCHEMA}"),
        "cases = full-corpus".to_string(),
        "cache_scope = compile-closure,score".to_string(),
        "boot_inputs = verified-content-cache".to_string(),
        "artifacts = tests/pixels_truth/p8-visibility.txt".to_string(),
        format!("artifact_sha256 = {artifact_sha256}"),
        "identical = true".to_string(),
    ];
    for line in stable {
        if !pinned.lines().any(|candidate| candidate == line) {
            return Err(format!(
                "pixels-cache-parity: pinned report omits stable result `{line}`; review and rerun with --update"
            ));
        }
    }
    Ok(())
}

pub(crate) fn is_sha256_hex(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

/// Digest every file under `dir` whose extension is in `extensions`.
///
/// Deliberately a superset of any true dependency closure: over-covering
/// costs a miss, under-covering would serve a stale value.
pub(crate) fn tree_digest(dir: &Path, extensions: &[&str]) -> Result<String, String> {
    fn collect(dir: &Path, extensions: &[&str], out: &mut Vec<PathBuf>) -> Result<(), String> {
        let mut entries = std::fs::read_dir(dir)
            .map_err(|error| format!("read {}: {error}", dir.display()))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| format!("read {}: {error}", dir.display()))?;
        entries.sort_by_key(std::fs::DirEntry::file_name);
        for entry in entries {
            let path = entry.path();
            if path.is_dir() {
                collect(&path, extensions, out)?;
            } else if path
                .extension()
                .is_some_and(|found| extensions.iter().any(|want| found == *want))
            {
                out.push(path);
            }
        }
        Ok(())
    }

    let mut files = Vec::new();
    if dir.is_dir() {
        collect(dir, extensions, &mut files)?;
    }
    let mut text = String::new();
    for file in files {
        let relative = file.strip_prefix(root()).unwrap_or(&file);
        let bytes =
            std::fs::read(&file).map_err(|error| format!("read {}: {error}", file.display()))?;
        text.push_str(&format!(
            "{} {}\n",
            relative.display(),
            wrela_compiler::report::sha256_hex(&bytes)
        ));
    }
    Ok(wrela_compiler::report::sha256_hex(text.as_bytes()))
}

/// Digest one file, or a stable marker when it is absent.
pub(crate) fn file_digest(path: &Path) -> String {
    match std::fs::read(path) {
        Ok(bytes) => wrela_compiler::report::sha256_hex(&bytes),
        Err(_) => "absent".to_string(),
    }
}

/// Cold / warm / cache-disabled parity, with the wall time each took.
///
/// Read every emitted conformance artifact under one isolated output root.
/// Paths and bytes are both part of the comparison so a missing, extra, or
/// changed report fails parity.
fn output_snapshot(dir: &Path) -> Result<BTreeMap<PathBuf, Vec<u8>>, String> {
    fn collect(
        root: &Path,
        dir: &Path,
        out: &mut BTreeMap<PathBuf, Vec<u8>>,
    ) -> Result<(), String> {
        let mut entries = std::fs::read_dir(dir)
            .map_err(|error| format!("pixels-cache-parity: read {}: {error}", dir.display()))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| format!("pixels-cache-parity: read {}: {error}", dir.display()))?;
        entries.sort_by_key(std::fs::DirEntry::file_name);
        for entry in entries {
            let path = entry.path();
            if path.is_dir() {
                collect(root, &path, out)?;
            } else {
                let relative = path
                    .strip_prefix(root)
                    .map_err(|_| {
                        format!(
                            "pixels-cache-parity: {} escaped output root",
                            path.display()
                        )
                    })?
                    .to_path_buf();
                let bytes = std::fs::read(&path).map_err(|error| {
                    format!("pixels-cache-parity: read {}: {error}", path.display())
                })?;
                out.insert(relative, bytes);
            }
        }
        Ok(())
    }

    let mut snapshot = BTreeMap::new();
    collect(dir, dir, &mut snapshot)?;
    Ok(snapshot)
}

fn require_same_snapshot(
    baseline_label: &str,
    baseline: &BTreeMap<PathBuf, Vec<u8>>,
    candidate_label: &str,
    candidate: &BTreeMap<PathBuf, Vec<u8>>,
) -> Result<(), String> {
    if baseline == candidate {
        return Ok(());
    }
    let paths = baseline
        .keys()
        .chain(candidate.keys())
        .cloned()
        .collect::<std::collections::BTreeSet<_>>();
    let differences = paths
        .into_iter()
        .filter_map(|path| match (baseline.get(&path), candidate.get(&path)) {
            (Some(_), None) => Some(format!("{} missing from {candidate_label}", path.display())),
            (None, Some(_)) => Some(format!("{} extra in {candidate_label}", path.display())),
            (Some(left), Some(right)) if left != right => Some(format!(
                "{} bytes differ ({}={}; {}={})",
                path.display(),
                baseline_label,
                wrela_compiler::report::sha256_hex(left),
                candidate_label,
                wrela_compiler::report::sha256_hex(right),
            )),
            _ => None,
        })
        .collect::<Vec<_>>();
    Err(format!(
        "pixels-cache-parity: {candidate_label} artifacts differ from {baseline_label}: {}",
        differences.join("; ")
    ))
}

/// Run the complete conformance corpus cold, warm, and cache-disabled, then
/// compare the actual emitted truth/report artifact bytes from isolated
/// directories. The repository gate separately verifies the entire guest
/// golden corpus, so this timing lane skips that redundant prelude and measures
/// the P8R.6 compile/score caches themselves. Process stdout is
/// diagnostic only and is never the authority.
pub(crate) fn cache_parity(args: &[String]) -> Result<(), String> {
    const PHASE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(600);

    fn wait_for_phase(
        child: &mut std::process::Child,
        label: &str,
        timeout: std::time::Duration,
    ) -> Result<std::process::ExitStatus, String> {
        let started = std::time::Instant::now();
        loop {
            if let Some(status) = child
                .try_wait()
                .map_err(|error| format!("pixels-cache-parity: poll {label} run: {error}"))?
            {
                return Ok(status);
            }
            if started.elapsed() >= timeout {
                child.kill().map_err(|error| {
                    format!("pixels-cache-parity: kill timed-out {label}: {error}")
                })?;
                let _ = child.wait();
                return Err(format!(
                    "pixels-cache-parity: {label} run exceeded {}s",
                    timeout.as_secs()
                ));
            }
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
    }

    fn run(
        label: &str,
        disabled: bool,
        output_dir: &Path,
    ) -> Result<(BTreeMap<PathBuf, Vec<u8>>, f64), String> {
        // Subprocesses rather than in-process calls: the cache switches are
        // environment variables, and a child process owns its own environment
        // without racing this one.
        let mut command = std::process::Command::new(env!("CARGO"));
        command.current_dir(root()).args([
            "run",
            "--quiet",
            "-p",
            "xtask",
            "--",
            "pixels-conformance",
            "--assume-guest-fixtures-verified",
        ]);
        command.env("WRELA_P8_CONFORMANCE_OUTPUT_DIR", output_dir);
        for cache in Cache::p8r6_derived() {
            if disabled {
                command.env(cache.env, "0");
            } else {
                command.env_remove(cache.env);
            }
        }
        command
            .stdout(std::process::Stdio::inherit())
            .stderr(std::process::Stdio::inherit());
        println!("pixels-cache-parity: starting {label} run");
        let started = std::time::Instant::now();
        let mut child = command
            .spawn()
            .map_err(|error| format!("pixels-cache-parity: {label} run: {error}"))?;
        let status = wait_for_phase(&mut child, label, PHASE_TIMEOUT)?;
        let elapsed = started.elapsed().as_secs_f64();
        if !status.success() {
            return Err(format!(
                "pixels-cache-parity: {label} run failed with {status}"
            ));
        }
        let snapshot = output_snapshot(output_dir)?;
        if snapshot.keys().collect::<Vec<_>>() != vec![&PathBuf::from("p8-visibility.txt")] {
            return Err(format!(
                "pixels-cache-parity: {label} emitted unexpected artifact set: {:?}",
                snapshot.keys().collect::<Vec<_>>()
            ));
        }
        println!("pixels-cache-parity: {label} run in {elapsed:.1}s");
        Ok((snapshot, elapsed))
    }

    let update = match args {
        [full] if full == "--full" => false,
        [full, update] if full == "--full" && update == "--update" => true,
        _ => {
            return Err("usage: cargo xtask pixels-cache-parity --full [--update]".to_string());
        }
    };
    for cache in Cache::p8r6_derived() {
        cache.clear()?;
    }
    let scratch =
        std::env::temp_dir().join(format!("wrela-pixels-cache-parity-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&scratch);
    let result = (|| {
        let cold_dir = scratch.join("cold");
        let warm_dir = scratch.join("warm");
        let off_dir = scratch.join("disabled");
        let (cold, cold_secs) = run("cold", false, &cold_dir)?;
        let (warm, warm_secs) = run("warm", false, &warm_dir)?;
        let (off, off_secs) = run("disabled", true, &off_dir)?;
        require_same_snapshot("cold", &cold, "warm", &warm)?;
        require_same_snapshot("cold", &cold, "disabled", &off)?;
        let truth = cold
            .get(Path::new("p8-visibility.txt"))
            .expect("validated artifact set");
        let artifact_sha256 = wrela_compiler::report::sha256_hex(truth);
        let report = format!(
            "# pixels conformance cache parity\nschema = {CACHE_SCHEMA}\ncases = full-corpus\n\
         cache_scope = compile-closure,score\nboot_inputs = verified-content-cache\n\
         artifacts = tests/pixels_truth/p8-visibility.txt\n\
         artifact_sha256 = {}\n\
         \n## [M] measured wall time (seconds)\ncold = {cold_secs:.1}\nwarm = {warm_secs:.1}\n\
         disabled = {off_secs:.1}\n\n## verdict\nidentical = true\n",
            artifact_sha256
        );
        let path = root().join("tests/census/p8-baseline/cache-parity.txt");
        if update {
            std::fs::create_dir_all(path.parent().expect("artifact parent"))
                .and_then(|()| std::fs::write(&path, &report))
                .map_err(|error| format!("write {}: {error}", path.display()))?;
            println!(
                "pixels-cache-parity: cold/warm/disabled artifact bytes identical; wrote {}",
                path.display()
            );
        } else {
            let pinned = std::fs::read_to_string(&path)
                .map_err(|error| format!("read {}: {error}", path.display()))?;
            check_pinned_cache_parity(&pinned, &artifact_sha256)?;
            println!(
                "pixels-cache-parity: cold/warm/disabled artifact bytes identical; stable fields match {} (timings left untouched)",
                path.display()
            );
        }
        Ok(())
    })();
    let cleanup = std::fs::remove_dir_all(&scratch)
        .map_err(|error| format!("pixels-cache-parity: remove {}: {error}", scratch.display()));
    match (result, cleanup) {
        (Ok(()), Ok(())) => Ok(()),
        (Err(error), _) | (Ok(()), Err(error)) => Err(error),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &'static str) -> Cache {
        let dir = std::env::temp_dir().join(format!(
            "wrela-p8-cache-test-{}-{name}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        Cache {
            name: "score",
            env: "WRELA_P8_CACHE_TEST_UNSET",
            dir,
            legacy_dir: None,
        }
    }

    #[test]
    fn a_round_trip_returns_exactly_what_was_stored() {
        let cache = scratch("roundtrip");
        assert_eq!(cache.get("k"), None, "an absent key is a miss");
        cache.put("k", "body\nwith lines\n");
        assert_eq!(cache.get("k").as_deref(), Some("body\nwith lines\n"));
        cache.clear().expect("clear");
        assert_eq!(cache.get("k"), None, "clearing removes the artifact");
    }

    #[test]
    fn identical_writers_share_a_hit_but_a_mismatched_writer_forces_a_miss() {
        let cache = scratch("writers");
        cache.try_put("k", "same").expect("first writer");
        cache.try_put("k", "same").expect("identical loser");
        assert_eq!(cache.get("k").as_deref(), Some("same"));

        let error = cache
            .try_put("k", "different")
            .expect_err("one key cannot publish two values");
        assert!(error.contains("value mismatch"));
        assert_eq!(cache.get("k"), None, "a disputed key must be recomputed");
        cache.clear().expect("clear");
    }

    #[test]
    fn a_verified_legacy_entry_is_promoted_and_clear_cannot_resurrect_it() {
        let mut cache = scratch("migration-primary");
        let legacy_dir = cache.dir.with_extension("legacy");
        let legacy = Cache {
            name: cache.name,
            env: cache.env,
            dir: legacy_dir.clone(),
            legacy_dir: None,
        };
        legacy.put("k", "verified old value");
        cache.legacy_dir = Some(legacy_dir.clone());

        assert_eq!(cache.get("k").as_deref(), Some("verified old value"));
        assert!(cache.path("k").is_file(), "a verified hit is promoted");
        cache.clear().expect("clear both cache generations");
        assert!(!cache.dir.exists());
        assert!(!legacy_dir.exists());
        assert_eq!(cache.get("k"), None, "clear cannot revive the fallback");
    }

    #[test]
    fn clearing_an_absent_cache_is_not_an_error() {
        let cache = scratch("absent");
        cache.clear().expect("clearing a cache that never existed");
    }

    #[test]
    fn a_truncated_or_bit_flipped_artifact_degrades_to_a_miss() {
        let cache = scratch("corrupt");
        cache.put("k", "the real value");
        let path = cache.path("k");
        let mut warnings = Vec::new();

        let good = std::fs::read_to_string(&path).expect("artifact");
        // Truncation: the body separator disappears.
        std::fs::write(&path, &good[..good.len() / 2]).expect("truncate");
        assert_eq!(
            cache.get_with_warning("k", |warning| warnings.push(warning)),
            None,
            "a truncated artifact is a miss"
        );
        assert_eq!(warnings.len(), 1, "truncation warns exactly once");
        assert!(warnings[0].contains("ignored"), "{}", warnings[0]);

        // Bit flip in the body: the recorded digest no longer matches.
        let flipped = good.replace("the real value", "the fake value");
        assert_ne!(flipped, good, "the fixture must actually change the body");
        std::fs::write(&path, &flipped).expect("flip");
        warnings.clear();
        assert_eq!(
            cache.get_with_warning("k", |warning| warnings.push(warning)),
            None,
            "a bit-flipped body is a miss"
        );
        assert_eq!(warnings.len(), 1, "a bit flip warns exactly once");
        assert!(warnings[0].contains("digest"), "{}", warnings[0]);

        // Invalid UTF-8 is a read failure, not an absent entry, and therefore
        // must produce the same visible corrupt-artifact warning.
        std::fs::write(&path, [0xff, 0xfe]).expect("invalid utf-8");
        warnings.clear();
        assert_eq!(
            cache.get_with_warning("k", |warning| warnings.push(warning)),
            None,
            "invalid UTF-8 is a miss"
        );
        assert_eq!(warnings.len(), 1, "invalid UTF-8 warns exactly once");
        assert!(warnings[0].contains("read failed"), "{}", warnings[0]);

        // Schema drift.
        let stale = good.replacen(
            &format!("{MAGIC} {CACHE_SCHEMA}"),
            &format!("{MAGIC} {}", CACHE_SCHEMA + 1),
            1,
        );
        std::fs::write(&path, &stale).expect("stale schema");
        assert_eq!(cache.get("k"), None, "a future schema is a miss");

        // An artifact belonging to another cache, filed under this key.
        let foreign = good.replacen("name score", "name compile-closure", 1);
        std::fs::write(&path, &foreign).expect("foreign");
        assert_eq!(cache.get("k"), None, "a foreign artifact is a miss");

        // A concurrent writer's artifact for a different key, renamed here.
        let other_key = good.replacen("key k", "key other", 1);
        std::fs::write(&path, &other_key).expect("other key");
        assert_eq!(cache.get("k"), None, "a mismatched key is a miss");

        // Restoring the good bytes restores the hit.
        std::fs::write(&path, &good).expect("restore");
        assert_eq!(cache.get("k").as_deref(), Some("the real value"));
        cache.clear().expect("clear");
    }

    #[test]
    fn a_disabled_cache_never_reads_or_writes() {
        let mut cache = scratch("disabled");
        cache.env = "WRELA_P8_SCORE_CACHE";
        // SAFETY: single-threaded within this test, and the variable is
        // restored before it returns.
        unsafe { std::env::set_var("WRELA_P8_SCORE_CACHE", "0") };
        cache.put("k", "value");
        assert_eq!(cache.get("k"), None);
        assert!(!cache.path("k").exists(), "a disabled cache writes nothing");
        unsafe { std::env::remove_var("WRELA_P8_SCORE_CACHE") };
        cache.put("k", "value");
        assert_eq!(cache.get("k").as_deref(), Some("value"));
        cache.clear().expect("clear");
    }

    #[test]
    fn every_key_component_is_load_bearing() {
        let base: Vec<(&str, String)> = vec![
            ("sources", "a".to_string()),
            ("compiler", "b".to_string()),
            ("options", "c".to_string()),
        ];
        let reference = key_of(&base);
        for index in 0..base.len() {
            let mut perturbed = base.clone();
            perturbed[index].1.push('!');
            assert_ne!(
                key_of(&perturbed),
                reference,
                "perturbing `{}` must change the key",
                base[index].0
            );
        }
        // A value moving between components must change the key too, which a
        // naive concatenation would not catch.
        let swapped: Vec<(&str, String)> = vec![
            ("sources", "ab".to_string()),
            ("compiler", String::new()),
            ("options", "c".to_string()),
        ];
        assert_ne!(key_of(&swapped), reference);
    }

    #[test]
    fn cached_digest_values_use_the_canonical_lowercase_shape() {
        assert!(is_sha256_hex(&"0".repeat(64)));
        assert!(is_sha256_hex(&"abcdef0123456789".repeat(4)));
        assert!(!is_sha256_hex(&"0".repeat(63)));
        assert!(!is_sha256_hex(&"A".repeat(64)));
        assert!(!is_sha256_hex(&"g".repeat(64)));
    }

    #[test]
    fn a_tree_digest_follows_its_files() {
        let dir = std::env::temp_dir().join(format!(
            "wrela-p8-tree-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("nested")).expect("mkdir");
        std::fs::write(dir.join("a.wr"), "one").expect("write");
        std::fs::write(dir.join("nested/b.wr"), "two").expect("write");
        std::fs::write(dir.join("ignored.txt"), "three").expect("write");
        let first = tree_digest(&dir, &["wr"]).expect("digest");

        std::fs::write(dir.join("ignored.txt"), "four").expect("write");
        assert_eq!(
            tree_digest(&dir, &["wr"]).expect("digest"),
            first,
            "an unselected extension does not move the digest"
        );

        std::fs::write(dir.join("nested/b.wr"), "changed").expect("write");
        assert_ne!(
            tree_digest(&dir, &["wr"]).expect("digest"),
            first,
            "a nested source change moves the digest"
        );

        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(
            tree_digest(&dir, &["wr"]).expect("digest"),
            wrela_compiler::report::sha256_hex(b""),
            "an absent tree digests as empty rather than failing"
        );
    }

    #[test]
    fn the_clearable_set_names_every_cache_once() {
        let mut names: Vec<&str> = Cache::all().iter().map(|cache| cache.name).collect();
        names.sort_unstable();
        names.dedup();
        assert_eq!(
            names,
            vec![
                "boot",
                "census",
                "compile-closure",
                "golden-boot",
                "golden-closure",
                "golden-stage",
                "score",
            ]
        );
        let mut dirs: Vec<PathBuf> = Cache::all().iter().map(|cache| cache.dir.clone()).collect();
        dirs.sort();
        dirs.dedup();
        assert_eq!(dirs.len(), 7, "each cache owns a distinct directory");
        assert!(
            dirs.iter()
                .all(|dir| dir.starts_with(root().join(".wrela-cache")))
        );
    }

    #[test]
    fn parity_toggles_only_the_two_p8r6_derived_caches() {
        assert_eq!(
            Cache::p8r6_derived().map(|cache| cache.name),
            ["compile-closure", "score"]
        );
    }

    #[test]
    fn parity_compares_artifact_paths_and_raw_bytes() {
        let dir = std::env::temp_dir().join(format!(
            "wrela-p8-parity-snapshot-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let left = dir.join("left");
        let right = dir.join("right");
        std::fs::create_dir_all(&left).expect("left dir");
        std::fs::create_dir_all(&right).expect("right dir");
        std::fs::write(left.join("p8-visibility.txt"), b"truth\n").expect("left report");
        std::fs::write(right.join("p8-visibility.txt"), b"truth\n").expect("right report");
        let baseline = output_snapshot(&left).expect("left snapshot");
        let same = output_snapshot(&right).expect("right snapshot");
        require_same_snapshot("left", &baseline, "right", &same).expect("identical bytes");

        std::fs::write(right.join("p8-visibility.txt"), b"changed\n").expect("changed report");
        let changed = output_snapshot(&right).expect("changed snapshot");
        assert!(
            require_same_snapshot("left", &baseline, "right", &changed)
                .expect_err("changed bytes must fail")
                .contains("bytes differ")
        );

        std::fs::remove_file(right.join("p8-visibility.txt")).expect("remove report");
        std::fs::write(right.join("extra.txt"), b"truth\n").expect("extra report");
        let wrong_set = output_snapshot(&right).expect("wrong-set snapshot");
        let error = require_same_snapshot("left", &baseline, "right", &wrong_set)
            .expect_err("missing and extra paths must fail");
        assert!(error.contains("missing from right") && error.contains("extra in right"));
        std::fs::remove_dir_all(&dir).expect("cleanup");
    }

    #[test]
    fn parity_verification_ignores_timings_but_checks_stable_identity() {
        let digest = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
        let pinned = format!(
            "# pixels conformance cache parity\nschema = {CACHE_SCHEMA}\ncases = full-corpus\n\
             cache_scope = compile-closure,score\nboot_inputs = verified-content-cache\n\
             artifacts = tests/pixels_truth/p8-visibility.txt\nartifact_sha256 = {digest}\n\n\
             ## [M] measured wall time (seconds)\ncold = 999.9\nwarm = 0.1\ndisabled = 777.7\n\n\
             ## verdict\nidentical = true\n"
        );
        check_pinned_cache_parity(&pinned, digest).expect("timings are informational");
        let stale = pinned.replace(digest, &"f".repeat(64));
        assert!(
            check_pinned_cache_parity(&stale, digest)
                .unwrap_err()
                .contains("artifact_sha256")
        );
    }
}
