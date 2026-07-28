//! **Census of `internal error:` producer sites** (plans/M9.md item II;
//! ledger `compiler.diagnostics.no-reachable-internal-error`).
//!
//! Allowlist lives in `ledger/census.toml` (`[internal_error]`); this
//! module owns the source-tree scan that locks against it.

use crate::census;

/// Per-file counts from [`census::data`].
pub fn sites_by_file() -> &'static std::collections::BTreeMap<String, usize> {
    &census::data().internal_error.sites_by_file
}

/// Total sites across [`sites_by_file`].
pub fn site_count() -> usize {
    census::data().internal_error.total
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;
    use std::path::PathBuf;

    /// The producer-bug prefix, assembled so this file's own source does
    /// not contain the contiguous substring the scan looks for.
    fn producer_bug_prefix() -> String {
        format!("{}{}", "internal", " error:")
    }

    fn src_root() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src")
    }

    fn count_in(text: &str, prefix: &str) -> usize {
        text.matches(prefix).count()
    }

    fn scan_tree(dir: &std::path::Path, prefix: &str, out: &mut BTreeMap<String, usize>) {
        let entries = std::fs::read_dir(dir).unwrap_or_else(|e| {
            panic!("read {}: {e}", dir.display());
        });
        for entry in entries {
            let entry = entry.expect("read_dir entry");
            let path = entry.path();
            if path.is_dir() {
                scan_tree(&path, prefix, out);
                continue;
            }
            if path.extension().and_then(|e| e.to_str()) != Some("rs") {
                continue;
            }
            let rel = path
                .strip_prefix(src_root())
                .expect("file under src/")
                .to_string_lossy()
                .replace('\\', "/");
            // Census modules document the prefix; they are not producers.
            if rel == "internal_error_census.rs" || rel == "census.rs" {
                continue;
            }
            let text = std::fs::read_to_string(&path).unwrap_or_else(|e| {
                panic!("read {}: {e}", path.display());
            });
            let n = count_in(&text, prefix);
            if n == 0 {
                continue;
            }
            out.insert(rel, n);
        }
    }

    #[test]
    fn internal_error_site_count_matches_the_written_down_census() {
        let prefix = producer_bug_prefix();
        let mut live: BTreeMap<String, usize> = BTreeMap::new();
        scan_tree(&src_root(), &prefix, &mut live);

        let expected = sites_by_file().clone();

        assert_eq!(
            live, expected,
            "producer-bug site census drifted.\n\
             Update [internal_error] in ledger/census.toml in the same \
             commit that adds or removes a site (plans/M9.md item II).\n\
             live={live:?}\n\
             expected={expected:?}"
        );

        let total: usize = live.values().sum();
        assert_eq!(
            total,
            site_count(),
            "internal_error.total ({}) != sum of per-file counts ({total})",
            site_count()
        );
        assert_eq!(
            site_count(),
            206,
            "the written-down total is part of the ratchet; bump it deliberately"
        );
    }
}
