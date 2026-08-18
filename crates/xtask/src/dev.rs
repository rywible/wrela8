use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::golden::{BootSel, GoldenOpts, golden};
use crate::{pixels_formal, pixels_plan_lint, pixels_vectors, root, run};

const DEV_STATE_FORMAT: &str = "wrela-dev-state-v1";

// This is a successful-check watermark, not a result cache: any changed
// content still runs the normal focused checks, and a new HEAD invalidates the
// complete snapshot.
#[derive(Debug, PartialEq, Eq)]
struct DevState {
    head: String,
    files: BTreeMap<String, String>,
}

fn git_stdout(args: &[&str]) -> Result<String, String> {
    let output = Command::new("git")
        .current_dir(root())
        .args(args)
        .output()
        .map_err(|error| format!("dev: run git: {error}"))?;
    if !output.status.success() {
        return Err(format!(
            "dev: git {} failed:\n{}",
            args.join(" "),
            String::from_utf8_lossy(&output.stderr)
        ));
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

fn git_paths(args: &[&str]) -> Result<Vec<PathBuf>, String> {
    Ok(git_stdout(args)?
        .lines()
        .filter(|line| !line.is_empty())
        .map(PathBuf::from)
        .collect())
}

fn dev_state_path() -> PathBuf {
    root().join(".wrela-cache/dev-state-v1.txt")
}

fn path_key(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}

fn path_digest(path: &Path) -> Result<String, String> {
    match std::fs::read(root().join(path)) {
        Ok(bytes) => Ok(wrela_compiler::report::sha256_hex(&bytes)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok("absent".to_string()),
        Err(error) => Err(format!("dev: read {}: {error}", path.display())),
    }
}

fn render_dev_state(state: &DevState) -> String {
    let mut text = format!("{DEV_STATE_FORMAT}\nhead={}\n", state.head);
    for (path, digest) in &state.files {
        text.push_str(digest);
        text.push('\t');
        text.push_str(path);
        text.push('\n');
    }
    text
}

fn parse_dev_state(text: &str) -> Result<DevState, String> {
    if !text.ends_with('\n') || text.contains('\r') {
        return Err("non-canonical dev state".into());
    }
    let mut lines = text.lines();
    if lines.next() != Some(DEV_STATE_FORMAT) {
        return Err("wrong dev state format".into());
    }
    let head = lines
        .next()
        .and_then(|line| line.strip_prefix("head="))
        .filter(|head| head.len() == 40 && head.bytes().all(|byte| byte.is_ascii_hexdigit()))
        .ok_or("invalid dev state HEAD")?
        .to_string();
    let mut files = BTreeMap::new();
    for line in lines {
        let (digest, path) = line.split_once('\t').ok_or("malformed dev state row")?;
        if path.is_empty()
            || digest.is_empty()
            || files.insert(path.to_string(), digest.to_string()).is_some()
        {
            return Err("invalid dev state row".into());
        }
    }
    Ok(DevState { head, files })
}

fn current_dev_state(paths: &BTreeSet<PathBuf>) -> Result<DevState, String> {
    let head = git_stdout(&["rev-parse", "--verify", "HEAD"])?
        .trim()
        .to_string();
    if head.len() != 40 || !head.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err("dev: git returned a non-canonical HEAD identity".into());
    }
    let files = paths
        .iter()
        .map(|path| Ok((path_key(path), path_digest(path)?)))
        .collect::<Result<_, String>>()?;
    Ok(DevState { head, files })
}

fn incremental_paths(current: &DevState, previous: Option<&DevState>) -> BTreeSet<PathBuf> {
    if previous.is_none_or(|previous| previous.head != current.head) {
        return current.files.keys().map(PathBuf::from).collect();
    }
    let previous = previous.unwrap();
    current
        .files
        .iter()
        .filter(|(path, digest)| previous.files.get(*path) != Some(*digest))
        .map(|(path, _)| PathBuf::from(path))
        .collect()
}

fn write_dev_state(state: &DevState) -> Result<(), String> {
    let path = dev_state_path();
    std::fs::create_dir_all(path.parent().unwrap())
        .map_err(|error| format!("dev: create state directory: {error}"))?;
    let temporary = path.with_extension(format!("tmp-{}", std::process::id()));
    std::fs::write(&temporary, render_dev_state(state))
        .map_err(|error| format!("dev: write {}: {error}", temporary.display()))?;
    std::fs::rename(&temporary, &path)
        .map_err(|error| format!("dev: publish {}: {error}", path.display()))
}

fn golden_case(path: &Path) -> Option<String> {
    let mut components = path.components();
    if components.next()?.as_os_str() != "tests" || components.next()?.as_os_str() != "golden" {
        return None;
    }
    components.next()?.as_os_str().to_str().map(str::to_string)
}

fn checked_golden_case_exists(case: &str) -> bool {
    std::fs::read_dir(root().join("tests/golden").join(case).join("expected"))
        .ok()
        .is_some_and(|entries| {
            entries
                .filter_map(Result::ok)
                .any(|entry| entry.path().is_file())
        })
}

/// Representative Pixels cases whose compile-time dumps `dev` checks.
///
/// Chosen to span the distinct generated-code shapes rather than to be a
/// sample: a baseline single-renderer scene, hard CSG, the multi-renderer
/// placement path, the largest image in the corpus (closest to the 2 MiB
/// branch-region limit), a multi-tile scanout mode, and one diagnostic path.
const DEV_PIXELS_DUMP_SLICE: [&str; 6] = [
    "check-pixels-plane",
    "check-pixels-hard-csg",
    "check-pixels-two-renderers",
    "check-pixels-field-ops",
    "boot-pixels-partial-mode",
    "err-pixels-capacity",
];

pub(crate) fn dev(args: &[String]) -> Result<(), String> {
    if !args.is_empty() {
        return Err("usage: cargo xtask dev".to_string());
    }
    let mut all_paths: BTreeSet<PathBuf> = git_paths(&["diff", "--name-only", "HEAD"])?
        .into_iter()
        .collect();
    all_paths.extend(git_paths(&["ls-files", "--others", "--exclude-standard"])?);
    if all_paths.is_empty() {
        println!("dev: no changed or untracked paths; nothing to run");
        return Ok(());
    }
    let current_state = current_dev_state(&all_paths)?;
    let previous_state = std::fs::read_to_string(dev_state_path())
        .ok()
        .and_then(|text| parse_dev_state(&text).ok());
    let paths = incremental_paths(&current_state, previous_state.as_ref());
    if paths.is_empty() {
        println!(
            "dev: no paths changed since the last successful focused run ({} dirty path(s) retained)",
            all_paths.len()
        );
        return Ok(());
    }

    let formal = paths.iter().any(|path| path.starts_with("formal/pixels"));
    let pixels = paths.iter().any(|path| {
        path.starts_with("crates/wrela-compiler/src/pixels")
            || path == Path::new("crates/wrela-machine/src/pixels.rs")
            || path == Path::new("crates/wrela-machine/src/pixels_contract.rs")
            || path.starts_with("stdlib/core/render")
            || path.starts_with("formal/pixels")
    });
    let rust = paths.iter().any(|path| {
        path.extension().and_then(|extension| extension.to_str()) == Some("rs")
            || path.file_name().and_then(|name| name.to_str()) == Some("Cargo.toml")
    });
    let vmm = paths
        .iter()
        .any(|path| path.starts_with("crates/wrela-vmm"));
    let docs = paths.iter().any(|path| path.starts_with("docs"));

    run(
        Command::new("cargo").args(["fmt", "--all", "--check"]),
        "dev format",
    )?;
    pixels_plan_lint()?;
    if formal {
        pixels_formal()?;
    }
    if pixels {
        pixels_vectors(false)?;
    }
    if rust {
        if pixels {
            run(
                Command::new("cargo").args([
                    "test",
                    "-p",
                    "wrela-compiler",
                    "--lib",
                    "pixels",
                    "--quiet",
                ]),
                "dev focused compiler Pixels tests",
            )?;
            run(
                Command::new("cargo").args(["test", "-p", "wrela-machine", "pixels", "--quiet"]),
                "dev focused machine Pixels tests",
            )?;
        } else {
            run(
                Command::new("cargo").args([
                    "test",
                    "--workspace",
                    "--exclude",
                    "wrela-vmm",
                    "--quiet",
                ]),
                "dev workspace tests",
            )?;
        }
    }
    if vmm {
        run(
            Command::new("cargo").args(["test", "-p", "wrela-vmm", "--lib", "--quiet"]),
            "dev portable VMM tests",
        )?;
    }

    let mut cases: BTreeSet<String> = paths.iter().filter_map(|path| golden_case(path)).collect();
    cases.retain(|case| checked_golden_case_exists(case));
    let edited_pixels_cases: Vec<String> = cases
        .iter()
        .filter(|case| case.contains("pixels"))
        .cloned()
        .collect();
    if pixels {
        cases.retain(|case| !case.contains("pixels"));
    }
    if !cases.is_empty() {
        golden(&GoldenOpts {
            cases: Some(cases.into_iter().collect()),
            boot: BootSel::All,
            ..GoldenOpts::default()
        })?;
    }
    if pixels {
        // Every compile-time Pixels expectation, but only the boots that cost
        // seconds. A `check-pixels-*` boot is a full certified sweep — minutes
        // apiece, and there are ten of them — so booting the whole family here
        // made this focused lane an order of magnitude slower than the gate it
        // is supposed to precede. The `boot-pixels-*` family still exercises
        // compile -> image -> guest -> scanout end to end, including the
        // multi-tile and partial-mode display paths, and any Pixels fixture the
        // developer actually edited is booted below. `cargo xtask verify` owns
        // the adversarial sweeps.
        // Compile-time expectations for a representative slice rather than all
        // ~44 Pixels cases. Every case here is a genuine full compile (~4.4s
        // each; measured `check`/`typed` are only ~0.54s of that, so there is
        // no meaningful duplicate work to remove), and a stdlib or compiler
        // edit — the common reason this lane runs — perturbs every case
        // near-identically, so a slice catches it. What a slice can miss is
        // drift specific to one scene's generated code; `cargo xtask verify`
        // checks the whole corpus and is the gate that must pass.
        let mut slice: BTreeSet<String> = DEV_PIXELS_DUMP_SLICE
            .iter()
            .map(|case| (*case).to_string())
            .collect();
        slice.extend(edited_pixels_cases.iter().cloned());
        golden(&GoldenOpts {
            cases: Some(slice.into_iter().collect()),
            boot: BootSel::None,
            ..GoldenOpts::default()
        })?;
        golden(&GoldenOpts {
            filter: Some("boot-pixels-".to_string()),
            boot: BootSel::Only,
            ..GoldenOpts::default()
        })?;
        if !edited_pixels_cases.is_empty() {
            golden(&GoldenOpts {
                cases: Some(edited_pixels_cases.clone()),
                boot: BootSel::Only,
                ..GoldenOpts::default()
            })?;
        }
        println!(
            "dev: Pixels dumps checked for {} representative case(s); boots covered \
             `boot-pixels-*`{} — the full corpus and the adversarial \
             `check-pixels-*` sweeps run in `cargo xtask verify`",
            DEV_PIXELS_DUMP_SLICE.len() + edited_pixels_cases.len(),
            if edited_pixels_cases.is_empty() {
                String::new()
            } else {
                format!(" plus edited {}", edited_pixels_cases.join(", "))
            }
        );
    }
    if docs {
        crate::corpus(&[])?;
    }
    println!(
        "dev: focused checks passed for {} newly changed path(s)",
        paths.len()
    );
    write_dev_state(&current_state)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn state(head: char, rows: &[(&str, &str)]) -> DevState {
        DevState {
            head: head.to_string().repeat(40),
            files: rows
                .iter()
                .map(|(path, digest)| ((*path).to_string(), (*digest).to_string()))
                .collect(),
        }
    }

    #[test]
    fn dev_state_roundtrips_canonically() {
        let original = state(
            'a',
            &[
                ("crates/a.rs", "digest-a"),
                ("tests/with space.wr", "absent"),
            ],
        );
        let text = render_dev_state(&original);
        assert_eq!(parse_dev_state(&text).unwrap(), original);
        assert!(parse_dev_state(text.trim_end()).is_err());
        assert!(parse_dev_state(&(text + "digest-a\tcrates/a.rs\n")).is_err());
    }

    #[test]
    fn incremental_dev_work_is_content_and_head_scoped() {
        let previous = state('a', &[("a.rs", "one"), ("b.rs", "two")]);
        let unchanged = state('a', &[("a.rs", "one"), ("b.rs", "two")]);
        assert!(incremental_paths(&unchanged, Some(&previous)).is_empty());

        let changed = state('a', &[("a.rs", "one"), ("b.rs", "three")]);
        assert_eq!(
            incremental_paths(&changed, Some(&previous)),
            BTreeSet::from([PathBuf::from("b.rs")])
        );

        let new_head = state('b', &[("a.rs", "one"), ("b.rs", "two")]);
        assert_eq!(
            incremental_paths(&new_head, Some(&previous)),
            BTreeSet::from([PathBuf::from("a.rs"), PathBuf::from("b.rs")])
        );
    }
}
