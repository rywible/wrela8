//! `golden` subcommand and helpers (extracted from main.rs).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

use wrela_compiler::codegen;
use wrela_compiler::eval;
use wrela_compiler::flowwir;
use wrela_compiler::flowwir_lower;
use wrela_compiler::layout;
use wrela_compiler::loader;
use wrela_compiler::lower;
use wrela_compiler::mwir;
use wrela_compiler::placement;
use wrela_compiler::report;
use wrela_compiler::sema;
use wrela_compiler::sema::typed::TestKind;
use wrela_compiler::syntax::ast::Module;
use wrela_compiler::syntax::lexer::{self, Token, TokenKind};
use wrela_compiler::syntax::parser::{self, Parsed};
use wrela_compiler::syntax::printer;

use crate::{fail_closed, golden_case_dirs, root, run};

// --- golden ---------------------------------------------------------------
//
// Layout: tests/golden/<case>/input.wr + expected/<stage>.txt. Each
// expected file pins `wrela dump --stage=<stage> input.wr` byte-for-byte.
//
// plans/M4.md item A adds one more case shape, a *project*: a case dir
// containing `src/` (a package tree of `.wr` files), a file `root` (one
// line: the root file's path relative to the case dir, e.g.
// `src/app/main.wr`), and `expected/` exactly like today. The runner
// tells the two shapes apart by `root`'s presence — `root` names the
// file `wrela dump`/`wrela test` is actually invoked on in place of the
// flat shape's `input.wr`; everything else (stage dispatch, `test.txt`'s
// touch convention, `--update`) is identical. The loader (`wrela dump
// --stage=check`, plans/M4.md item A) anchors the package root from
// that file's own path alone — no new flag.
/// plans/M5.md item D: the review-visible form a golden pins for raw
/// image bytes (decision: a hexdump, not raw binary in git — "more
/// reviewable," the task's own instruction) — 16 bytes per line,
/// `%08x: ` offset prefix, space-separated lowercase hex pairs, no ASCII
/// column (kept minimal; the offset+hex pairs alone are enough to spot a
/// one-byte diff in a review, and `--stage=asm`'s own dump already gives
/// every word a mnemonic elsewhere). Deterministic, a pure function of
/// `bytes`.
pub(crate) fn hex_dump(bytes: &[u8]) -> String {
    let mut out = String::new();
    for (i, chunk) in bytes.chunks(16).enumerate() {
        let offset = i * 16;
        let hex: Vec<String> = chunk.iter().map(|b| format!("{b:02x}")).collect();
        out.push_str(&format!("{offset:08x}: {}\n", hex.join(" ")));
    }
    out
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

/// plans/M5.md item E: builds the `wrela-vmm` binary and codesigns it
/// against `crates/wrela-vmm/entitlements.plist` — the smoke probe's own
/// exact recipe (plans/M5.md's "item zero"), re-run on every `cargo xtask
/// check`/`golden`. `golden`'s own `test.txt` cases pass the resulting
/// path via `wrela test --vmm <path>` so a case with an `@test(runtime)`
/// fn boots on this exact, freshly rebuilt-and-signed binary rather than
/// whatever `find_vmm_binary`'s own next-to-the-executable fallback might
/// happen to find on disk. `codesign` itself only exists on macOS
/// (CLAUDE.md's own target host for this milestone's flagship dev loop);
/// building still runs everywhere, so a `test.txt` case with no runtime
/// tests never depends on this step at all, and the boot golden's own
/// failure on a non-macOS host is an honest, expected gap, not a silent
/// skip.
pub(crate) fn build_and_sign_vmm() -> Result<PathBuf, String> {
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

/// `cargo test --workspace` never codesigns anything — the test binaries
/// `cargo test` builds under `target/debug/deps/` are ordinary,
/// unsigned executables, exactly like any other `cargo build` output
/// (`build_and_sign_vmm`'s own doc comment: codesigning is this xtask's
/// own, deliberate, post-build step, never something Cargo does for
/// free). That was harmless before this item — `wrela-vmm`'s own
/// pre-item-F unit tests (the ESR decoder, `parse_report`) are pure
/// functions, no HVF calls anywhere — but item F's own hand-built
/// clock-reading test (`lib.rs`) is the first `#[test]` in this
/// workspace that calls `hv_vm_create` for real, so its test binary
/// needs the identical entitlement `build_and_sign_vmm` already gives
/// the ordinary `wrela-vmm` binary. `check`'s own plain `cargo test
/// --workspace` step therefore excludes `wrela-vmm` (above); this
/// function is `wrela-vmm`'s own replacement: build its test binaries
/// with `--no-run`, codesign each one (parsed off cargo's own
/// `Executable ... (path)` stderr lines — no `--message-format=json`/
/// `serde_json` dependency needed for two lines of known shape), then
/// run each directly, single-threaded (`--test-threads=1`) — the same
/// "no two calls into HVF's one process-wide VM context are ever
/// concurrent" reasoning the hand-built test's own doc comment gives,
/// now enforced at the process level too, not just within one `#[test]`
/// fn.
pub(crate) fn test_wrela_vmm_signed() -> Result<(), String> {
    // Deliberately no `--quiet` here (unlike every other `cargo`
    // invocation in this file): cargo only ever prints its own
    // `Executable ... (path)` lines — this function's one source of
    // truth for which binaries to sign — under the default verbosity;
    // `--quiet` suppresses them even on a rebuild, and a cached (already
    // up to date) build prints nothing else either way, so there is no
    // extra noise being traded away here.
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

pub(crate) fn golden(update: bool) -> Result<(), String> {
    run(
        Command::new("cargo").args(["build", "--quiet", "-p", "wrela-compiler", "--bin", "wrela"]),
        "cargo build wrela",
    )?;
    let wrela = root().join("target/debug/wrela");
    let vmm = build_and_sign_vmm()?;
    let golden_dir = root().join("tests/golden");
    let mut cases = 0usize;
    let mut failures = Vec::new();
    for case in golden_case_dirs(&golden_dir)? {
        let expected_dir = case.join("expected");
        let input = match golden_case_target(&case)? {
            Some(target) if target.exists() && expected_dir.is_dir() => target,
            _ => {
                failures.push(format!(
                    "{}: missing input.wr (or `root`'s target) or expected/",
                    case.display()
                ));
                continue;
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
            // Invoke with a root-relative path from the repo root: the
            // path is rendered into some diagnostics (the generic
            // requirement chain's `instantiated at <path>:<line>` lines),
            // and an absolute path would bake this checkout's location
            // into the pinned expectation — failing in any worktree or
            // clone.
            let rel_input = input.strip_prefix(root()).unwrap_or(&input);
            // plans/M4.md item E: `build.txt`/`build-err.txt` mean "run
            // `wrela build <input.wr> --out-dir <fixed-repo-relative-dir>`,
            // compare its stdout" — a third meaning alongside `test.txt`'s,
            // the golden runner's own "one new case shape" (decision 11).
            // The out-dir is a fixed, deterministic, repo-relative path
            // derived from the case's own name (never a random temp name:
            // its own literal text is what `wrela build`'s stdout prints
            // back, decision 8's own "print the path exactly as derived
            // from the argument", so it must be stable across runs to stay
            // golden-pinnable) — removed and recreated fresh immediately
            // before every invocation, then removed again after this
            // expectation's own checks finish, so no build artifact is
            // ever left behind for git to notice.
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
            // plans/M5.md item D: `img.hex` means "hexdump whatever
            // `<name>.img` the case's own `build.txt` stage already wrote
            // into `build_out_dir_abs` and compare/update against that" —
            // a fourth expectation-file meaning, alongside `test.txt`'s/
            // `build.txt`'s/`build-err.txt`'s, never its own `wrela`
            // invocation (a case carrying `img.hex` must also carry
            // `build.txt`, sorted before it alphabetically — `b` < `i` —
            // so the image already exists on disk by the time this stage
            // runs; the build-output directory's own removal is deferred
            // to the end of this case's loop, below, specifically so this
            // stage can still see it).
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
            // `test.txt` means "run `wrela test <input.wr>`, compare its
            // stdout" (plans/M3.md item E) rather than the ordinary
            // `wrela dump --stage=<stage>` every other expectation file
            // pins — the touch convention extended one file name beyond
            // `--stage=<name>`, the dumbest integration (M3.md item E's
            // own wording). `wrela test` exits nonzero exactly when any
            // `@test` failed (decision 9) — a normal, expected outcome
            // some goldens deliberately pin (a mixed pass/fail report),
            // not a runner malfunction, so unlike every `dump` stage
            // below, a nonzero exit here is not itself a failure; only
            // stdout is ever compared, exactly like every other stage.
            //
            // plans/M15.md item K / decision 1098: `test-omit-dmb.txt`
            // is the mutated arm of boot-cross-core-publish-acquire —
            // same `wrela test` path with `--omit-dmb` (barriers
            // stripped). Timeout / pass under that flag is an oracle
            // failure (pinned expectation is a FAILED checksum).
            let out = if stage == "test" || stage == "test-omit-dmb" {
                let mut cmd = Command::new(&wrela);
                cmd.current_dir(root()).arg("test").arg(rel_input);
                if stage == "test-omit-dmb" {
                    cmd.arg("--omit-dmb");
                }
                cmd.arg("--vmm").arg(&vmm).output().map_err(|e| format!("run wrela: {e}"))?
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
            // `build-err.txt` requires the opposite of every other
            // expectation file: `wrela build` exits nonzero exactly when
            // it printed a diagnostic (decision 11). `dump` also exits
            // nonzero on a diagnostic (plans/M9.md item NN); when the
            // pinned expectation is itself an `error[…]` line, that
            // nonzero exit is the expected companion — only a failure
            // exit with empty/mismatched stdout is a harness problem.
            if stage == "build-err" {
                if out.status.success() {
                    failures.push(format!(
                        "{} [build-err]: wrela build unexpectedly exited successfully",
                        case.display()
                    ));
                    continue;
                }
            } else if stage != "test" && stage != "test-omit-dmb" && !out.status.success() {
                // plans/M9.md item NN: dump exits nonzero on a diagnostic.
                // Accept that when the pinned expectation (or, under
                // `--update`, the fresh stdout) is itself an `error[…]`
                // line — otherwise a success-case dump that crashed is
                // still a harness failure.
                // Item RR: the fresh text is stdout then stderr, same as
                // the expectation built below — a dump that failed before
                // printing anything puts its whole diagnostic on stderr,
                // so reading stdout alone would no longer see it.
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
            // plans/M9.md item RR: diagnostics moved from stdout to
            // stderr (`bin/wrela.rs`'s `print_line_diagnostic` doc has
            // the why), so an expectation is "everything the tool wrote",
            // stdout first. Every stage that succeeds writes nothing to
            // stderr, so the ~565 non-diagnostic goldens are unchanged
            // byte for byte; a diagnostic case's text simply arrives on
            // the other stream and concatenates into the same expectation
            // it always had. `--timings` is the one thing that would
            // interleave here, and no golden passes it.
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
            // decision 11: a `build.txt` case additionally proves the
            // *written* report file — not just `wrela build`'s own stdout
            // summary — matches the pinned `expected/report.txt`, if the
            // case carries one (every project-shaped `build.txt` case
            // does; `err-image-*`'s own `build-err.txt` cases never write
            // a report at all, so this block never runs for those).
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
        // plans/M5.md item D: the build-output directory (if any expected
        // file in this case used one) is removed once, here, after every
        // expectation file for this case has had its own chance to read
        // it — `img.hex` (above) needs `build.txt`'s own written `.img`
        // still on disk, so removal can no longer happen immediately
        // after the `build`/`build-err` stage itself finishes.
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
    if update {
        println!("golden: updated {cases} expectation(s) — review the diff before committing");
        return Ok(());
    }
    // plans/M9.md item II: pinning `internal error:` as an expected
    // diagnostic is itself a bug (house rule: each is a compiler bug, not
    // an outcome). Catch it at the golden surface so a `--update` that
    // baked one in fails the next gate run.
    assert_no_internal_error_in_goldens(&golden_dir)?;
    if failures.is_empty() {
        println!("golden: {cases} expectation(s) ok");
        Ok(())
    } else {
        for f in &failures {
            eprintln!("{f}\n");
        }
        Err(format!("golden: {} failure(s)", failures.len()))
    }
}

/// plans/M9.md item II: no golden expectation may contain the
/// `"internal error: "` prefix. Walks every `expected/*.txt` under
/// `tests/golden/`.
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
