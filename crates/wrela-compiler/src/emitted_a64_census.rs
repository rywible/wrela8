//! **Census of hand-emitted A64 instruction words** (plans/M10.md item F0).
//!
//! Allowlist / inventory live in `ledger/census.toml` (`[emitted_a64]`);
//! this module owns the live measurement and source-site scan locks.

use crate::census;

pub use crate::census::EmittedCategory as Category;
pub use census::EmitterEntry;

pub fn entries() -> &'static [EmitterEntry] {
    &census::data().emitted_a64.entries
}

pub fn encode_enc_sites_by_file() -> &'static std::collections::BTreeMap<String, usize> {
    &census::data().emitted_a64.encode_enc_sites_by_file
}

pub fn encode_enc_site_count() -> usize {
    census::data().emitted_a64.encode_enc_site_total
}

pub fn floor_sum_of_rows() -> usize {
    census::data().emitted_a64.floor_sum_of_rows
}

pub fn floor_words() -> usize {
    census::data().emitted_a64.floor_words
}

pub fn image_static_sum_of_rows() -> usize {
    census::data().emitted_a64.image_static_sum_of_rows
}

pub fn not_yet_migrated_sum_of_rows() -> usize {
    census::data().emitted_a64.not_yet_migrated_sum_of_rows
}

pub fn unclassified_sum_of_rows() -> usize {
    census::data().emitted_a64.unclassified_sum_of_rows
}

pub fn grand_total_sum_of_rows() -> usize {
    census::data().emitted_a64.grand_total_sum_of_rows
}

#[cfg(test)]
fn backend_emitters() -> &'static [String] {
    &census::data().emitted_a64.backend_emitters
}

#[cfg(test)]
fn non_inventory() -> &'static [String] {
    &census::data().emitted_a64.non_inventory
}

#[cfg(test)]
fn wrapper_emitters() -> &'static [String] {
    &census::data().emitted_a64.wrapper_emitters
}

#[cfg(test)]
mod tests {
    use super::*;

    fn backend_emitters() -> &'static [String] {
        super::backend_emitters()
    }
    fn non_inventory() -> &'static [String] {
        super::non_inventory()
    }
    fn wrapper_emitters() -> &'static [String] {
        super::wrapper_emitters()
    }
    use std::collections::BTreeMap;
    use std::path::PathBuf;

    fn src_root() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src")
    }

    /// The emission-site needle, assembled so this file's own docs that
    /// spell it as two halves are not the only exclusion — we also skip
    /// this file by name below (it still contains contiguous forms in
    /// older comments and in `scan_enc_fns`).
    fn encode_enc_needle() -> String {
        format!("{}{}", "encode::", "enc_")
    }

    /// Drop `#[cfg(test)]` / `#[cfg(all(test, …))]` module bodies so the
    /// JIT harness and unit-test stand-ins do not own the lock.
    fn strip_cfg_test_modules(text: &str) -> String {
        let lines: Vec<&str> = text.lines().collect();
        let mut out: Vec<&str> = Vec::new();
        let mut i = 0;
        while i < lines.len() {
            let stripped = lines[i].trim_start();
            let is_cfg_test =
                stripped.starts_with("#[cfg(test)]") || stripped.starts_with("#[cfg(all(test");
            if is_cfg_test {
                let mut j = i;
                while j < lines.len() && lines[j].trim_start().starts_with("#[") {
                    j += 1;
                }
                while j < lines.len() && lines[j].trim().is_empty() {
                    j += 1;
                }
                let is_mod = j < lines.len()
                    && lines[j]
                        .trim_start()
                        .trim_start_matches("pub ")
                        .starts_with("mod ");
                if is_mod {
                    let mut k = j;
                    let mut depth: isize = 0;
                    let mut started = false;
                    while k < lines.len() {
                        depth += lines[k].chars().filter(|&c| c == '{').count() as isize;
                        depth -= lines[k].chars().filter(|&c| c == '}').count() as isize;
                        if lines[k].contains('{') {
                            started = true;
                        }
                        k += 1;
                        if started && depth == 0 {
                            break;
                        }
                    }
                    i = k;
                    continue;
                }
            }
            out.push(lines[i]);
            i += 1;
        }
        let mut s = out.join("\n");
        if text.ends_with('\n') {
            s.push('\n');
        }
        s
    }

    fn scan_encode_enc_sites(
        dir: &std::path::Path,
        needle: &str,
        out: &mut BTreeMap<String, usize>,
    ) {
        let entries = std::fs::read_dir(dir).unwrap_or_else(|e| {
            panic!("read {}: {e}", dir.display());
        });
        for entry in entries {
            let entry = entry.expect("read_dir entry");
            let path = entry.path();
            if path.is_dir() {
                scan_encode_enc_sites(&path, needle, out);
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
            // This census file documents the needle; it is not a producer.
            if rel == "emitted_a64_census.rs" || rel == "census.rs" {
                continue;
            }
            let text = std::fs::read_to_string(&path).unwrap_or_else(|e| {
                panic!("read {}: {e}", path.display());
            });
            let production = strip_cfg_test_modules(&text);
            let n = production.matches(needle).count();
            if n == 0 {
                continue;
            }
            out.insert(rel, n);
        }
    }

    #[test]
    fn encode_enc_site_count_matches_the_written_down_census() {
        let needle = encode_enc_needle();
        let mut live: BTreeMap<String, usize> = BTreeMap::new();
        scan_encode_enc_sites(&src_root(), &needle, &mut live);

        let expected: BTreeMap<String, usize> = encode_enc_sites_by_file().clone();

        assert_eq!(
            live, expected,
            "encode::enc_ emission-site census drifted.\n\
             Update ledger/census.toml [emitted_a64.encode_enc_sites_by_file] in the \
             same commit that adds or removes a call site \
             (plans/M10.md item F0 — addition lock).\n\
             live={live:?}\n\
             expected={expected:?}"
        );

        let total: usize = live.values().sum();
        assert_eq!(
            total,
            encode_enc_site_count(),
            "encode_enc_site_total ({}) != sum of per-file counts ({total})",
            encode_enc_site_count()
        );
        // 453 -> 347, deliberately downward, in three steps and with no
        // change to a single emitted word (every golden dump verified
        // byte-for-byte identical against the pre-change binary):
        //   * the five nested `load_imm`/`push` copies in the stub emitters
        //     were lifted to one module-level pair;
        //   * the dead `patch_cond`/`patch_cbz`/`patch_cbnz_w`/`bl_to`/
        //     `b_to`/`load_rodata_addr_at`/`bl_console_append_*` `Asm`
        //     helpers were deleted;
        //   * 58 three-register ALU sites and 21 `cmp` sites collapsed onto
        //     `FnCtx::{add_reg,mul_reg,orr_reg,and_reg,cmp_reg}`, and the
        //     three identical park-and-return tails onto
        //     `emit_park_and_return`.
        assert_eq!(
            encode_enc_site_count(),
            347,
            "the written-down total is part of the ratchet; bump it deliberately"
        );
    }

    /// Module-level (`^fn` / `^pub fn` / `^pub(crate) fn`) names whose body
    /// contains `encode::enc_`, keyed as `file::name`.
    fn scan_enc_fns() -> BTreeMap<String, ()> {
        let mut out = BTreeMap::new();
        for file in ["layout.rs", "layout/harness.rs", "codegen.rs"] {
            let path = src_root().join(file);
            let text = std::fs::read_to_string(&path).unwrap_or_else(|e| {
                panic!("read {}: {e}", path.display());
            });
            let lines: Vec<&str> = text.lines().collect();
            let mut i = 0;
            while i < lines.len() {
                let line = lines[i];
                let name = if let Some(rest) = line.strip_prefix("fn ") {
                    Some(rest.split('(').next().unwrap_or("").trim())
                } else if let Some(rest) = line.strip_prefix("pub fn ") {
                    Some(rest.split('(').next().unwrap_or("").trim())
                } else if let Some(rest) = line.strip_prefix("pub(crate) fn ") {
                    Some(rest.split('(').next().unwrap_or("").trim())
                } else if let Some(rest) = line.strip_prefix("pub(super) fn ") {
                    // plans/M10.md item K: harness submodule emits with
                    // parent-only visibility.
                    Some(rest.split('(').next().unwrap_or("").trim())
                } else {
                    None
                };
                if let Some(name) = name {
                    if !name.is_empty() {
                        let mut j = i;
                        let mut body_start = None;
                        while j < lines.len() {
                            if lines[j].contains('{') {
                                body_start = Some(j);
                                break;
                            }
                            if lines[j].trim_end().ends_with(';') {
                                break;
                            }
                            j += 1;
                        }
                        if let Some(bs) = body_start {
                            let mut depth = 0isize;
                            let mut k = bs;
                            while k < lines.len() {
                                depth += lines[k].matches('{').count() as isize;
                                depth -= lines[k].matches('}').count() as isize;
                                if depth == 0 {
                                    let body = lines[i..=k].join("\n");
                                    if body.contains("encode::enc_")
                                        || body.contains("crate::encode::enc_")
                                    {
                                        out.insert(format!("{file}::{name}"), ());
                                    }
                                    break;
                                }
                                k += 1;
                            }
                        }
                    }
                }
                i += 1;
            }
        }
        out
    }

    /// Ordinary codegen IR→A64 path — emits, but is not the hand-emitted
    /// inventory this census locks.

    /// Touches encoders but is not a hand-emitted runtime routine (patch
    /// helpers, orchestration, thin wrappers whose bytes are counted on
    /// another row, measure fns themselves).

    /// Census rows that are real emitters (measured word count) but whose
    /// body does not itself contain `encode::enc_` — thin wrappers that
    /// forward to a counted sibling. Still locked by the live-count test.

    #[test]
    fn emitted_a64_census_matches_live_measurements() {
        let mut live: BTreeMap<&'static str, usize> = BTreeMap::new();
        for (k, v) in crate::layout::emitted_a64_census_live_counts() {
            live.insert(k, v);
        }
        for (k, v) in crate::codegen::emitted_a64_census_specialization_live_counts() {
            assert!(
                live.insert(k, v).is_none(),
                "specialization key {k} collided with a layout key"
            );
        }

        let expected: BTreeMap<&str, usize> = entries()
            .iter()
            .map(|e| (e.name.as_str(), e.words))
            .collect();

        assert_eq!(
            live, expected,
            "hand-emitted A64 census drifted.\n\
             Update ledger/census.toml [[emitted_a64.entries]] in the same \
             commit that adds, removes, or grows an emitter \
             (plans/M10.md item F0).\n\
             live={live:?}\n\
             expected={expected:?}"
        );

        let floor_rows: usize = entries()
            .iter()
            .filter(|e| e.category == Category::Floor)
            .map(|e| e.words)
            .sum();
        let image_static: usize = entries()
            .iter()
            .filter(|e| e.category == Category::ImageStatic)
            .map(|e| e.words)
            .sum();
        let not_yet: usize = entries()
            .iter()
            .filter(|e| e.category == Category::NotYetMigrated)
            .map(|e| e.words)
            .sum();
        let unclassified: usize = entries()
            .iter()
            .filter(|e| e.category == Category::Unclassified)
            .map(|e| e.words)
            .sum();
        let grand: usize = entries().iter().map(|e| e.words).sum();

        assert_eq!(floor_rows, floor_sum_of_rows(), "floor_sum_of_rows()");
        assert_eq!(
            image_static,
            image_static_sum_of_rows(),
            "image_static_sum_of_rows()"
        );
        assert_eq!(
            not_yet,
            not_yet_migrated_sum_of_rows(),
            "not_yet_migrated_sum_of_rows()"
        );
        assert_eq!(
            unclassified,
            unclassified_sum_of_rows(),
            "unclassified_sum_of_rows()"
        );
        assert_eq!(
            grand,
            grand_total_sum_of_rows(),
            "grand_total_sum_of_rows()"
        );
        assert_eq!(
            floor_words(),
            52,
            "adjusted floor total is part of the ratchet"
        );
        // Sanity: floor_words() == push_halt + SP prefix + abort tail + secondary SP
        // + checkpoint LR + primary trampoline.
        assert_eq!(15 + 5 + 6 + 5 + 5 + 16, floor_words());
    }

    #[test]
    fn emitted_a64_hand_emitter_set_is_closed() {
        let candidates = scan_enc_fns();
        let census_keys: BTreeMap<String, ()> = entries()
            .iter()
            .map(|e| (format!("{}::{}", e.file, e.name), ()))
            .collect();
        let backend: BTreeMap<&str, ()> = backend_emitters()
            .iter()
            .map(|n| (n.as_str(), ()))
            .collect();
        let non_inv: BTreeMap<&str, ()> =
            non_inventory().iter().map(|n| (n.as_str(), ())).collect();
        let wrappers: BTreeMap<&str, ()> = wrapper_emitters()
            .iter()
            .map(|n| (n.as_str(), ()))
            .collect();

        let mut unexpected = Vec::new();
        for key in candidates.keys() {
            if census_keys.contains_key(key) {
                continue;
            }
            if backend.contains_key(key.as_str()) {
                continue;
            }
            if non_inv.contains_key(key.as_str()) {
                continue;
            }
            unexpected.push(key.clone());
        }
        assert!(
            unexpected.is_empty(),
            "new encode::enc_ emitter(s) not in ledger/census.toml entries, \
             backend_emitters, or non_inventory — classify deliberately \
             (plans/M10.md item F0):\n  {}",
            unexpected.join("\n  ")
        );

        for key in census_keys.keys() {
            if wrappers.contains_key(key.as_str()) {
                continue;
            }
            assert!(
                candidates.contains_key(key),
                "census entry `{key}` no longer scans as an encode::enc_ \
                 emitter — remove it from ledger/census.toml in the same \
                 commit that removes a hand-asm twin after its specialized \
                 replacement is proven (E4 deleted build_rt_run_one / \
                 build_group_child_poll)"
            );
        }
        for key in wrappers.keys() {
            assert!(
                census_keys.contains_key(*key),
                "wrapper_emitters entry `{key}` missing from ledger/census.toml"
            );
        }
    }
}
