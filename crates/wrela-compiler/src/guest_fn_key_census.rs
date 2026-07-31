//! **Census of required `Fn key=` spellings in backend golden dumps**
//! (plans/M9.md item KK; closes decision 324's coverage-loss audit).
//!
//! Allowlists live in `tests/census.toml` (`[guest_fn_key]`); this module
//! owns the golden-dump scan that locks against them.

use crate::census;

pub fn zero_fn_dumps() -> &'static [(String, String)] {
    &census::data().guest_fn_key.zero_fn_dumps
}

pub fn required_fn_keys() -> &'static [(String, Vec<String>)] {
    &census::data().guest_fn_key.required
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::{BTreeMap, BTreeSet};
    use std::path::PathBuf;

    fn golden_root() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tests/golden")
    }

    fn parse_fn_keys(text: &str) -> BTreeSet<String> {
        let mut out = BTreeSet::new();
        for line in text.lines() {
            if let Some(rest) = line.trim_start().strip_prefix("Fn key=") {
                let key = if let Some(i) = rest.find(" ret=") {
                    &rest[..i]
                } else if let Some(i) = rest.find(" frame=") {
                    &rest[..i]
                } else {
                    rest.split_whitespace().next().unwrap_or("")
                };
                if !key.is_empty() {
                    out.insert(key.to_string());
                }
            }
        }
        out
    }

    fn scan_backend_dumps() -> BTreeMap<String, BTreeSet<String>> {
        let root = golden_root();
        let mut live: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
        let entries = std::fs::read_dir(&root).unwrap_or_else(|e| {
            panic!("read {}: {e}", root.display());
        });
        for entry in entries {
            let entry = entry.expect("read_dir entry");
            let case = entry.path();
            if !case.is_dir() {
                continue;
            }
            let exp = case.join("expected");
            if !exp.is_dir() {
                continue;
            }
            for dump in ["mwir.txt", "asm.txt"] {
                let path = exp.join(dump);
                if !path.exists() {
                    continue;
                }
                let rel = format!(
                    "{}/expected/{dump}",
                    case.file_name().unwrap().to_string_lossy()
                );
                let text = std::fs::read_to_string(&path).unwrap_or_else(|e| {
                    panic!("read {}: {e}", path.display());
                });
                live.insert(rel, parse_fn_keys(&text));
            }
        }
        live
    }

    #[test]
    fn backend_fn_key_census_matches_the_written_down_table() {
        let live = scan_backend_dumps();
        let zero: BTreeMap<&str, &str> = zero_fn_dumps()
            .iter()
            .map(|(p, r)| (p.as_str(), r.as_str()))
            .collect();
        let required: BTreeMap<&str, &[String]> = required_fn_keys()
            .iter()
            .map(|(p, keys)| (p.as_str(), keys.as_slice()))
            .collect();

        let mut failures: Vec<String> = Vec::new();

        for (rel, keys) in &live {
            if keys.is_empty() {
                if !zero.contains_key(rel.as_str()) {
                    failures.push(format!(
                        "{rel}: dump has zero Fn keys but is not in zero_fn_dumps.\n\
                         Allowlist it deliberately in tests/census.toml or restore a guest \
                         path so the feature is reachable (plans/M9.md item KK)."
                    ));
                }
                continue;
            }
            if zero.contains_key(rel.as_str()) {
                failures.push(format!(
                    "{rel}: listed in zero_fn_dumps but dump now has keys {keys:?}.\n\
                     Remove it from tests/census.toml [guest_fn_key]."
                ));
                continue;
            }
            let Some(need) = required.get(rel.as_str()) else {
                failures.push(format!(
                    "{rel}: backend dump is not in required_fn_keys.\n\
                     Add its required Fn keys to tests/census.toml \
                     (plans/M9.md item KK)."
                ));
                continue;
            };
            let missing: Vec<&str> = need
                .iter()
                .map(|s| s.as_str())
                .filter(|k| !keys.contains(*k))
                .collect();
            if !missing.is_empty() {
                failures.push(format!(
                    "{rel}: missing required Fn keys {missing:?}.\n\
                     live keys: {keys:?}.\n\
                     Restore a guest-reachable path (`@test(runtime)`, actor `pub`, \
                     or `@task`) that calls the feature, or update tests/census.toml \
                     deliberately (plans/M9.md item KK)."
                ));
            }
        }

        for rel in zero.keys() {
            if !live.contains_key(*rel) {
                failures.push(format!(
                    "{rel}: in zero_fn_dumps but no such golden dump exists."
                ));
            }
        }
        for rel in required.keys() {
            if !live.contains_key(*rel) {
                failures.push(format!(
                    "{rel}: in required_fn_keys but no such golden dump exists."
                ));
            }
        }

        assert!(
            failures.is_empty(),
            "guest fn-key census drifted:\n{}",
            failures.join("\n\n")
        );
    }
}
