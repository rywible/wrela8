use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::golden::{BootSel, GoldenOpts, golden};
use crate::{pixels_formal, pixels_plan_lint, pixels_vectors, root, run};

fn git_paths(args: &[&str]) -> Result<Vec<PathBuf>, String> {
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
    Ok(String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter(|line| !line.is_empty())
        .map(PathBuf::from)
        .collect())
}

fn golden_case(path: &Path) -> Option<String> {
    let mut components = path.components();
    if components.next()?.as_os_str() != "tests" || components.next()?.as_os_str() != "golden" {
        return None;
    }
    components.next()?.as_os_str().to_str().map(str::to_string)
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
    let mut paths: BTreeSet<PathBuf> = git_paths(&["diff", "--name-only", "HEAD"])?
        .into_iter()
        .collect();
    paths.extend(git_paths(&["ls-files", "--others", "--exclude-standard"])?);
    if paths.is_empty() {
        println!("dev: no changed or untracked paths; nothing to run");
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
    let edited_pixels_cases: Vec<String> = cases
        .iter()
        .filter(|case| case.contains("pixels"))
        .cloned()
        .collect();
    if pixels {
        cases.retain(|case| !case.contains("pixels"));
    }
    for filter in cases {
        golden(&GoldenOpts {
            filter: Some(filter),
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
        "dev: focused checks passed for {} changed path(s)",
        paths.len()
    );
    Ok(())
}
