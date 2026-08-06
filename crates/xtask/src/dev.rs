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
    if pixels {
        cases.retain(|case| !case.contains("pixels"));
        cases.insert("pixels".to_string());
    }
    for filter in cases {
        golden(&GoldenOpts {
            filter: Some(filter),
            boot: BootSel::All,
            ..GoldenOpts::default()
        })?;
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
