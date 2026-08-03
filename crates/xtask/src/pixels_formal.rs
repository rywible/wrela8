use std::path::Path;
use std::process::Command;

use crate::root;

const FORBIDDEN: &[&str] = &["sorry", "admit", "axiom", "unsafe", "native_decide"];

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
    let output = Command::new("lake")
        .current_dir(&dir)
        .arg("build")
        .output()
        .map_err(|error| lake_error("build the project", &error))?;
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

fn lake_error(action: &str, error: &std::io::Error) -> String {
    if error.kind() == std::io::ErrorKind::NotFound {
        format!(
            "pixels formal: required `lake` executable was not found while trying to {action}. \
             Install the pinned Lean toolchain with \
             `elan toolchain install leanprover/lean4:v4.30.0` and ensure `lake` is on PATH"
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
        assert!(error.contains("elan toolchain install leanprover/lean4:v4.30.0"));
    }
}
