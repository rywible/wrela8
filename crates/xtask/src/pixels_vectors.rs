use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use wrela_compiler::pixels::test_vectors::{
    NumericVector, REQUIRED_KERNELS, VectorFamily, deterministic_vectors, guest_module_text,
    validate_vectors, vector_digest,
};

use crate::root;

const MANIFEST_KEYS: &[&str] = &[
    "kernel",
    "theorem",
    "rust",
    "wrela_scalar",
    "edge",
    "generated",
    "adapter_schema",
    "error_code",
];

const FINAL_THEOREMS: &[&str] = &[
    "Pixels.run_certificate_first_visible",
    "Pixels.complete_event_cell_preserves_structure",
    "Pixels.fixed_q_winner_matches_real_winner",
    "Pixels.coverage_composite_within_budget",
    "Pixels.transparent_summary_within_budget",
    "Pixels.display_singleton_is_exact",
    "Pixels.kinetic_slack_preserves_run",
];

pub(crate) fn pixels_vectors(update: bool) -> Result<(), String> {
    let vectors = deterministic_vectors();
    validate_vectors(&vectors)?;
    validate_guest_module(update)?;
    validate_manifest(&vectors)?;
    validate_numeric_boot_source()?;
    println!(
        "pixels-vectors: {} vectors, {} kernels, digest={}",
        vectors.len(),
        REQUIRED_KERNELS.len(),
        vector_digest()
    );
    Ok(())
}

fn validate_guest_module(update: bool) -> Result<(), String> {
    let path = root().join("stdlib/core/render_test_vectors.wr");
    let expected = guest_module_text();
    if update {
        std::fs::write(&path, &expected)
            .map_err(|error| format!("pixels vectors: write {}: {error}", path.display()))?;
    }
    let source = std::fs::read_to_string(&path)
        .map_err(|error| format!("pixels vectors: read {}: {error}", path.display()))?;
    if source != expected {
        return Err(format!(
            "pixels vectors: generated guest module drifted; run `cargo xtask pixels-vectors --update`"
        ));
    }
    Ok(())
}

fn validate_manifest(vectors: &[NumericVector]) -> Result<(), String> {
    let path = root().join("formal/pixels/KERNELS.txt");
    let source = std::fs::read_to_string(&path)
        .map_err(|error| format!("pixels vectors: read {}: {error}", path.display()))?;
    let mut rows = Vec::<BTreeMap<&str, &str>>::new();
    let mut final_theorems = Vec::new();
    for (line_index, line) in source.lines().enumerate() {
        if let Some(theorem) = line.strip_prefix("# final_theorem=") {
            final_theorems.push(theorem);
            continue;
        }
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if line.trim() != line || line.contains("  ") {
            return Err(format!(
                "pixels vectors: KERNELS.txt:{} has unstable whitespace",
                line_index + 1
            ));
        }
        let mut row = BTreeMap::new();
        for token in line.split(' ') {
            let (key, value) = token.split_once('=').ok_or_else(|| {
                format!(
                    "pixels vectors: KERNELS.txt:{} malformed token `{token}`",
                    line_index + 1
                )
            })?;
            if !MANIFEST_KEYS.contains(&key) {
                return Err(format!(
                    "pixels vectors: KERNELS.txt:{} unknown key `{key}`",
                    line_index + 1
                ));
            }
            if value.is_empty() || row.insert(key, value).is_some() {
                return Err(format!(
                    "pixels vectors: KERNELS.txt:{} empty or duplicate `{key}`",
                    line_index + 1
                ));
            }
        }
        if row.len() != MANIFEST_KEYS.len() {
            return Err(format!(
                "pixels vectors: KERNELS.txt:{} requires exactly {} fields",
                line_index + 1,
                MANIFEST_KEYS.len()
            ));
        }
        rows.push(row);
    }
    if final_theorems != FINAL_THEOREMS {
        return Err(format!(
            "pixels vectors: final trust-boundary theorem census mismatch\nexpected: {FINAL_THEOREMS:?}\nactual: {final_theorems:?}"
        ));
    }
    let kernels = rows.iter().map(|row| row["kernel"]).collect::<Vec<_>>();
    if kernels != REQUIRED_KERNELS {
        return Err(format!(
            "pixels vectors: manifest kernel order/coverage mismatch\nexpected: {REQUIRED_KERNELS:?}\nactual: {kernels:?}"
        ));
    }
    let mut unique_mappings = BTreeSet::new();
    for row in rows {
        let kernel = row["kernel"];
        if row["adapter_schema"] != "i32x4_to_tagged_i64" {
            return Err(format!(
                "pixels vectors: `{kernel}` must declare adapter_schema=i32x4_to_tagged_i64"
            ));
        }
        if row["error_code"] != "tagged" {
            return Err(format!(
                "pixels vectors: `{kernel}` must declare error_code=tagged"
            ));
        }
        if !unique_mappings.insert((kernel, row["theorem"])) {
            return Err(format!("pixels vectors: duplicate mapping for `{kernel}`"));
        }
        validate_symbol(row["rust"], "Rust")?;
        validate_symbol(row["wrela_scalar"], "Wrela")?;
        validate_theorem(row["theorem"])?;
        validate_correspondence(kernel, row["theorem"])?;
        let edge = parse_case(row["edge"], kernel, "edge")?;
        let generated = parse_case(row["generated"], kernel, "generated")?;
        for (case, family) in [
            (edge, VectorFamily::Edge),
            (generated, VectorFamily::Generated),
        ] {
            if !vectors.iter().any(|vector| {
                vector.kernel == kernel && vector.case_id == case && vector.family == family
            }) {
                return Err(format!(
                    "pixels vectors: `{kernel}` manifest names missing {family:?} case {case}"
                ));
            }
        }
    }
    Ok(())
}

fn validate_correspondence(kernel: &str, theorem: &str) -> Result<(), String> {
    let path = root().join("formal/pixels/Pixels/KernelCorrespondence.lean");
    let source = std::fs::read_to_string(&path)
        .map_err(|error| format!("pixels vectors: read {}: {error}", path.display()))?;
    let expected = format!("def {kernel}_correspondence := @{theorem}");
    let count = source.lines().filter(|line| *line == expected).count();
    if count != 1 {
        return Err(format!(
            "pixels vectors: `{kernel}` requires exactly one kernel-specific Lean proof alias `{expected}`"
        ));
    }
    Ok(())
}

fn parse_case(raw: &str, kernel: &str, family: &str) -> Result<u32, String> {
    raw.parse::<u32>()
        .map_err(|_| format!("pixels vectors: `{kernel}` has invalid {family} vector case `{raw}`"))
}

fn validate_symbol(mapping: &str, language: &str) -> Result<(), String> {
    let (relative, symbol) = mapping.rsplit_once("::").ok_or_else(|| {
        format!("pixels vectors: {language} mapping `{mapping}` must be path::symbol")
    })?;
    if relative.contains("..") || Path::new(relative).is_absolute() {
        return Err(format!(
            "pixels vectors: {language} mapping path is not repository-relative: `{relative}`"
        ));
    }
    let path = root().join(relative);
    let source = std::fs::read_to_string(&path).map_err(|error| {
        format!(
            "pixels vectors: read {language} mapping {}: {error}",
            path.display()
        )
    })?;
    if !source_declares_function(&source, symbol, language) {
        return Err(format!(
            "pixels vectors: {language} symbol `{symbol}` is absent from {}",
            path.display()
        ));
    }
    Ok(())
}

fn source_declares_function(source: &str, symbol: &str, language: &str) -> bool {
    let comment_marker = if language == "Rust" { "//" } else { "#" };
    source.lines().any(|line| {
        let code = line
            .split_once(comment_marker)
            .map_or(line, |(before, _)| before)
            .trim_start();
        let Some(function) = code.find("fn ") else {
            return false;
        };
        let prefix = &code[..function];
        if !prefix.is_empty()
            && !prefix
                .chars()
                .all(|character| character.is_ascii_alphabetic() || "()_ ".contains(character))
        {
            return false;
        }
        let declaration = &code[function + 3..];
        declaration
            .strip_prefix(symbol)
            .is_some_and(|suffix| suffix.starts_with('(') || suffix.starts_with('<'))
    })
}

fn validate_theorem(theorem: &str) -> Result<(), String> {
    if !theorem.starts_with("Pixels.")
        || theorem
            .split('.')
            .any(|component| component.is_empty() || !component.chars().all(valid_identifier_char))
    {
        return Err(format!("pixels vectors: invalid theorem name `{theorem}`"));
    }
    let symbol = theorem
        .strip_prefix("Pixels.")
        .ok_or_else(|| format!("pixels vectors: invalid theorem `{theorem}`"))?;
    let formal = root().join("formal/pixels/Pixels");
    let mut found = false;
    for entry in std::fs::read_dir(&formal)
        .map_err(|error| format!("pixels vectors: read {}: {error}", formal.display()))?
    {
        let path = entry
            .map_err(|error| format!("pixels vectors: read formal entry: {error}"))?
            .path();
        if path.extension().and_then(|extension| extension.to_str()) != Some("lean") {
            continue;
        }
        let source = std::fs::read_to_string(&path)
            .map_err(|error| format!("pixels vectors: read {}: {error}", path.display()))?;
        found |= source_declares_theorem(&source, symbol);
    }
    if !found {
        return Err(format!(
            "pixels vectors: theorem `{theorem}` is absent from formal source"
        ));
    }
    let entry = root().join("formal/pixels/Pixels.lean");
    let entry_source = std::fs::read_to_string(&entry)
        .map_err(|error| format!("pixels vectors: read {}: {error}", entry.display()))?;
    if entry_source
        .lines()
        .filter(|line| line.trim() == format!("#print axioms {theorem}"))
        .count()
        != 1
    {
        return Err(format!(
            "pixels vectors: theorem `{theorem}` is not pinned by `#print axioms`"
        ));
    }
    Ok(())
}

fn source_declares_theorem(source: &str, symbol: &str) -> bool {
    source.lines().any(|line| {
        let code = line
            .split_once("--")
            .map_or(line, |(before, _)| before)
            .trim();
        let Some(suffix) = code.strip_prefix("theorem ") else {
            return false;
        };
        suffix.strip_prefix(symbol).is_some_and(|rest| {
            rest.is_empty()
                || rest.chars().next().is_some_and(char::is_whitespace)
                || rest.starts_with('(')
                || rest.starts_with(':')
        })
    })
}

fn valid_identifier_char(character: char) -> bool {
    character.is_ascii_alphanumeric() || character == '_'
}

fn validate_numeric_boot_source() -> Result<(), String> {
    let path = root().join("tests/golden/boot-pixels-numeric/src/examples/boot_pixels_numeric.wr");
    let source = std::fs::read_to_string(&path)
        .map_err(|error| format!("pixels vectors: read {}: {error}", path.display()))?;
    for forbidden in [
        "Field",
        "Renderer",
        "@field",
        "@material",
        "sphere(",
        "plane(",
    ] {
        if source.contains(forbidden) {
            return Err(format!(
                "pixels vectors: numeric boot source contains renderer scene token `{forbidden}`"
            ));
        }
    }
    for required in [
        "VECTOR_COUNT",
        "VECTOR_DIGEST",
        "interval_add",
        "byte_singleton",
    ] {
        if !source.contains(required) {
            return Err(format!(
                "pixels vectors: numeric boot source does not exercise `{required}`"
            ));
        }
    }
    if source.contains("VECTOR_COUNT ==") {
        return Err(
            "pixels vectors: numeric boot source must not hard-code generated vector count"
                .to_string(),
        );
    }
    let expected_path = root().join("tests/golden/boot-pixels-numeric/expected/test.txt");
    let expected = std::fs::read_to_string(&expected_path)
        .map_err(|error| format!("pixels vectors: read {}: {error}", expected_path.display()))?;
    if expected.contains("error[")
        || expected.contains("FAILED")
        || !expected.contains("1 passed, 0 failed")
    {
        return Err(
            "pixels vectors: numeric boot expectation must record successful runtime execution"
                .to_string(),
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn manifest_parser_rejects_missing_symbols() {
        let error =
            validate_symbol("stdlib/core/render_interval.wr::does_not_exist", "Wrela").unwrap_err();
        assert!(error.contains("absent"));
    }

    #[test]
    fn declaration_matching_rejects_comments_and_prefix_collisions() {
        assert!(!source_declares_function(
            "# pub fn real_name(value: i32)\npub fn real_name_suffix(value: i32)",
            "real_name",
            "Wrela"
        ));
        assert!(source_declares_function(
            "pub fn real_name(value: i32)",
            "real_name",
            "Wrela"
        ));
        assert!(!source_declares_theorem(
            "theorem exact_name_suffix : True := by trivial",
            "exact_name"
        ));
        assert!(source_declares_theorem(
            "theorem exact_name : True := by trivial",
            "exact_name"
        ));
    }

    #[test]
    fn manifest_requires_the_kernel_specific_proof_alias() {
        let error = validate_correspondence("q_insertion_sort", "Pixels.adjacent_q_order_three")
            .unwrap_err();
        assert!(error.contains("kernel-specific Lean proof alias"));
    }
}
