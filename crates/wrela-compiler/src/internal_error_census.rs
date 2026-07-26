//! **Census of `internal error:` producer sites** (plans/M9.md item II;
//! ledger `compiler.diagnostics.no-reachable-internal-error`).
//!
//! CLAUDE.md: every fuzz lane checks each iteration for messages that
//! start with the producer-bug prefix, and each such message is a bug,
//! not an outcome. This file locks the *count and file distribution* of
//! those sites so adding one without updating the census fails
//! `cargo test`.
//!
//! ## What this is, and what it is not
//!
//! It is a ratchet against silent growth of the producer-bug surface.
//! It is **not** a claim that every site is unreachable from ordinary
//! source — four sites this milestone alone were reachable through
//! multi-module shapes no fuzz lane constructed, and a fifth
//! (`unbound local` via `comptime assert` over a runtime name) was
//! reachable through a single-file shape the generators never spelled
//! (plans/M9.md item NN). Reachability is policed by generator shapes
//! (`cargo xtask fuzz imports`; the comptime-assert-over-runtime-name
//! shapes in `fuzz sema`/`eval`/`lower`) and by the golden assertion
//! that no `expected/*.txt` contains the prefix. Per-site "why
//! unreachable" annotations are deliberately not written here: a false
//! "unreachable" claim is worse than none (item II decision 171; item
//! NN decision 414 reaffirms — the durable half is a shape the fuzzer
//! can reach, not a census column).

/// Per-file counts of the producer-bug prefix under
/// `crates/wrela-compiler/src/`, measured 2026-07-25. Adding a site in
/// a listed file without bumping its count — or adding a site in a new
/// file — fails the unit test below. The census file itself is excluded
/// from the scan (it talks about the prefix without emitting one).
pub const INTERNAL_ERROR_SITES_BY_FILE: &[(&str, usize)] = &[
    // 75 -> 77, plans/M10.md item A2c: two sites in `collect_placed_statics`
    // (non-named type / missing completed size).
    ("layout.rs", 77),
    // 78 -> 79, plans/M10.md item A2c: placed static has no comptime value.
    ("eval/interp.rs", 79),
    ("lower.rs", 5),
    ("codegen.rs", 4),
    ("eval/mod.rs", 2),
    ("placement.rs", 2),
    ("sema/mod.rs", 3),
    ("sema/specialize.rs", 2),
    ("sema/types.rs", 2),
    ("flowwir_lower.rs", 1),
    ("sema/typed.rs", 1),
];

/// Total sites across [`INTERNAL_ERROR_SITES_BY_FILE`].
pub const INTERNAL_ERROR_SITE_COUNT: usize = {
    let mut n = 0;
    let mut i = 0;
    while i < INTERNAL_ERROR_SITES_BY_FILE.len() {
        n += INTERNAL_ERROR_SITES_BY_FILE[i].1;
        i += 1;
    }
    n
};

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

    /// Count occurrences of the prefix in one file's text. Overlapping
    /// matches are not a concern — the prefix never overlaps itself.
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
            // This census file documents the prefix; it is not a producer.
            if rel == "internal_error_census.rs" {
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

        let expected: BTreeMap<String, usize> = INTERNAL_ERROR_SITES_BY_FILE
            .iter()
            .map(|(f, n)| ((*f).to_string(), *n))
            .collect();

        assert_eq!(
            live, expected,
            "producer-bug site census drifted.\n\
             Update INTERNAL_ERROR_SITES_BY_FILE in internal_error_census.rs \
             in the same commit that adds or removes a site \
             (plans/M9.md item II).\n\
             live={live:?}\n\
             expected={expected:?}"
        );

        let total: usize = live.values().sum();
        assert_eq!(
            total, INTERNAL_ERROR_SITE_COUNT,
            "INTERNAL_ERROR_SITE_COUNT ({INTERNAL_ERROR_SITE_COUNT}) != sum of per-file counts ({total})"
        );
        assert_eq!(
            INTERNAL_ERROR_SITE_COUNT, 178,
            "the written-down total is part of the ratchet; bump it deliberately"
        );
    }
}
