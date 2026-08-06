use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::process::Command;

use crate::root;

const FORBIDDEN: &[&str] = &["sorry", "admit", "axiom", "unsafe", "native_decide"];
const PINNED_LEAN_TOOLCHAIN: &str = "leanprover/lean4:v4.32.2";

fn strip_lean(source: &str) -> Result<String, String> {
    let bytes = source.as_bytes();
    let mut out = String::with_capacity(bytes.len());
    let mut i = 0;
    let mut block_depth = 0usize;
    let mut line_comment = false;
    let mut string = false;
    let mut escaped = false;
    while i < bytes.len() {
        let b = bytes[i];
        let next = bytes.get(i + 1).copied();
        if line_comment {
            if b == b'\n' {
                line_comment = false;
                out.push('\n');
            } else {
                out.push(' ');
            }
            i += 1;
            continue;
        }
        if block_depth != 0 {
            if b == b'/' && next == Some(b'-') {
                block_depth += 1;
                out.push_str("  ");
                i += 2;
            } else if b == b'-' && next == Some(b'/') {
                block_depth -= 1;
                out.push_str("  ");
                i += 2;
            } else {
                out.push(if b == b'\n' { '\n' } else { ' ' });
                i += 1;
            }
            continue;
        }
        if string {
            if escaped {
                escaped = false;
            } else if b == b'\\' {
                escaped = true;
            } else if b == b'"' {
                string = false;
            }
            out.push(if b == b'\n' { '\n' } else { ' ' });
            i += 1;
            continue;
        }
        if b == b'-' && next == Some(b'-') {
            line_comment = true;
            out.push_str("  ");
            i += 2;
        } else if b == b'/' && next == Some(b'-') {
            block_depth = 1;
            out.push_str("  ");
            i += 2;
        } else if b == b'!' && next == Some(b'"') {
            return Err(
                "interpolated or macro string literals are unsupported by the admission scanner"
                    .to_string(),
            );
        } else if b == b'"' {
            string = true;
            out.push(' ');
            i += 1;
        } else {
            out.push(b as char);
            i += 1;
        }
    }
    if block_depth != 0 {
        return Err("unterminated block comment".to_string());
    }
    if string {
        return Err("unterminated string literal".to_string());
    }
    Ok(out)
}

fn scan_text(source: &str, name: &str) -> Result<(), String> {
    let stripped =
        strip_lean(source).map_err(|message| format!("pixels formal scan: {message} in {name}"))?;
    for (line_index, line) in stripped.lines().enumerate() {
        for token in line.split(|c: char| !(c.is_ascii_alphanumeric() || c == '_')) {
            if FORBIDDEN.contains(&token) {
                return Err(format!(
                    "pixels formal scan: forbidden token `{token}` in {name}:{}",
                    line_index + 1
                ));
            }
        }
    }
    Ok(())
}

fn scan_dir(dir: &Path) -> Result<(), String> {
    let mut pending = vec![dir.to_path_buf()];
    while let Some(path) = pending.pop() {
        let mut entries = std::fs::read_dir(&path)
            .map_err(|e| format!("read {}: {e}", path.display()))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| format!("read {} entry: {e}", path.display()))?;
        entries.sort_by_key(|entry| entry.path());
        for entry in entries {
            let path = entry.path();
            if path.file_name().and_then(|s| s.to_str()) == Some(".lake") {
                continue;
            }
            if path.is_dir() {
                pending.push(path);
            } else if path.extension().and_then(|s| s.to_str()) == Some("lean") {
                let source = std::fs::read_to_string(&path)
                    .map_err(|e| format!("read {}: {e}", path.display()))?;
                scan_text(&source, &path.display().to_string())?;
            }
        }
    }
    Ok(())
}

pub(crate) fn pixels_formal_scan() -> Result<(), String> {
    scan_dir(&root().join("formal/pixels"))
}

fn normalize_axioms(output: &str) -> String {
    let mut rows = Vec::new();
    for line in output.lines() {
        let (left, axioms) = if let Some((left, right)) = line.split_once(" depends on axioms: [") {
            (left, right.trim().trim_end_matches(']').replace(", ", ","))
        } else if let Some((left, _)) = line.split_once(" does not depend on any axioms") {
            (left, "none".to_string())
        } else {
            continue;
        };
        let theorem = left
            .trim()
            .trim_start_matches("info:")
            .trim()
            .split_whitespace()
            .last()
            .unwrap_or("")
            .trim_matches('\'');
        rows.push(format!("{theorem} = {axioms}"));
    }
    rows.sort();
    rows.dedup();
    let mut normalized = rows.join("\n");
    normalized.push('\n');
    normalized
}

pub(crate) fn pixels_formal() -> Result<(), String> {
    pixels_formal_scan()?;
    let dir = root().join("formal/pixels");
    let output = build_pixels(&dir)?;
    if !output.status.success() {
        return Err(format!(
            "pixels formal: `lake build` failed:\n{}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        ));
    }
    let combined = format!(
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let actual = normalize_axioms(&combined);
    let expected = std::fs::read_to_string(dir.join("EXPECTED_AXIOMS.txt"))
        .map_err(|e| format!("read EXPECTED_AXIOMS.txt: {e}"))?;
    if actual != expected {
        return Err(format!(
            "pixels formal: axiom manifest drift\nexpected:\n{expected}actual:\n{actual}"
        ));
    }
    println!(
        "pixels-formal: {} theorem axiom rows match",
        actual.lines().count()
    );
    Ok(())
}

fn build_pixels(dir: &Path) -> Result<std::process::Output, String> {
    let root_olean = dir.join(".lake/build/lib/lean/Pixels.olean");
    if !root_olean.is_file() {
        for chunk in formal_module_order(dir)?.chunks(4) {
            let mut command = Command::new("lake");
            command.current_dir(dir).arg("build");
            for module in chunk {
                command.arg(format!("+{module}"));
            }
            let output = command
                .output()
                .map_err(|error| lake_error("build a cold four-module batch", &error))?;
            if !output.status.success() {
                return Err(format!(
                    "pixels formal: cold module batch `{}` failed:\n{}{}",
                    chunk.join(" "),
                    String::from_utf8_lossy(&output.stdout),
                    String::from_utf8_lossy(&output.stderr)
                ));
            }
        }
    }
    Command::new("lake")
        .current_dir(dir)
        .arg("build")
        .output()
        .map_err(|error| lake_error("build the project", &error))
}

fn formal_module_order(dir: &Path) -> Result<Vec<String>, String> {
    let source_dir = dir.join("Pixels");
    let mut dependencies = BTreeMap::<String, BTreeSet<String>>::new();
    for entry in std::fs::read_dir(&source_dir)
        .map_err(|error| format!("pixels formal: read {}: {error}", source_dir.display()))?
    {
        let path = entry
            .map_err(|error| format!("pixels formal: read module entry: {error}"))?
            .path();
        if path.extension().and_then(|extension| extension.to_str()) != Some("lean") {
            continue;
        }
        let stem = path
            .file_stem()
            .and_then(|stem| stem.to_str())
            .ok_or_else(|| format!("pixels formal: non-UTF-8 module path {}", path.display()))?;
        let module = format!("Pixels.{stem}");
        let source = std::fs::read_to_string(&path)
            .map_err(|error| format!("pixels formal: read {}: {error}", path.display()))?;
        let imports = source
            .lines()
            .filter_map(|line| line.strip_prefix("import Pixels."))
            .map(|suffix| format!("Pixels.{}", suffix.trim()))
            .collect();
        dependencies.insert(module, imports);
    }
    let names: BTreeSet<String> = dependencies.keys().cloned().collect();
    for (module, imports) in &dependencies {
        for import in imports {
            if !names.contains(import) {
                return Err(format!(
                    "pixels formal: {module} imports unknown local module {import}"
                ));
            }
        }
    }
    let mut built = BTreeSet::new();
    let mut order = Vec::with_capacity(names.len());
    while order.len() != names.len() {
        let ready = names
            .iter()
            .filter(|name| !built.contains(*name))
            .find(|name| {
                dependencies
                    .get(*name)
                    .is_some_and(|imports| imports.is_subset(&built))
            })
            .cloned();
        let Some(module) = ready else {
            return Err("pixels formal: local import graph contains a cycle".to_string());
        };
        built.insert(module.clone());
        order.push(module);
    }
    Ok(order)
}

fn lake_error(action: &str, error: &std::io::Error) -> String {
    if error.kind() == std::io::ErrorKind::NotFound {
        format!(
            "pixels formal: required `lake` executable was not found while trying to {action}. \
             Install the pinned Lean toolchain with \
             `elan toolchain install {PINNED_LEAN_TOOLCHAIN}` and ensure `lake` is on PATH"
        )
    } else {
        format!("pixels formal: could not {action} with `lake`: {error}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scanner_ignores_comments_and_strings() {
        scan_text(
            r#"
              -- sorry
              /- outer axiom /- nested unsafe -/ -/
              def safe := "admit"
            "#,
            "fixture",
        )
        .unwrap();
    }

    #[test]
    fn scanner_rejects_unterminated_comments_and_strings() {
        for source in ["/- no close", "\"no close"] {
            assert!(
                scan_text(source, "fixture")
                    .unwrap_err()
                    .contains("unterminated")
            );
        }
    }

    #[test]
    fn scanner_fails_closed_on_executable_interpolated_strings() {
        for source in [
            r#"def hidden := s!"{(by sorry : Nat)}""#,
            r#"def hidden := m!"{(by admit : Nat)}""#,
        ] {
            let error = scan_text(source, "fixture").unwrap_err();
            assert!(error.contains("interpolated or macro string"));
        }
    }

    #[test]
    fn scanner_rejects_each_escape_token() {
        for token in FORBIDDEN {
            let error =
                scan_text(&format!("theorem bad : True := by {token}"), "fixture").unwrap_err();
            assert!(error.contains(token));
        }
    }

    #[test]
    fn axiom_output_is_normalized_and_sorted() {
        let got = normalize_axioms(
            "'B' depends on axioms: [Quot.sound]\n\
             info: X:1:0: 'A' depends on axioms: [propext, Classical.choice]\n\
             'C' does not depend on any axioms\n",
        );
        assert_eq!(
            got,
            "A = propext,Classical.choice\nB = Quot.sound\nC = none\n"
        );
    }

    #[test]
    fn missing_lake_error_has_pinned_install_instructions() {
        let error = lake_error(
            "build the project",
            &std::io::Error::from(std::io::ErrorKind::NotFound),
        );
        assert!(error.contains(&format!("elan toolchain install {PINNED_LEAN_TOOLCHAIN}")));
    }
}
