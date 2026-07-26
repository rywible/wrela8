//! **Census of required `Fn key=` spellings in backend golden dumps**
//! (plans/M9.md item KK; closes decision 324's coverage-loss audit).
//!
//! After H3's reachability-based lowering, a golden that only reaches its
//! feature from comptime `@test` / `const` initializers silently collapses
//! to `Store.init` while still looking green. This ratchet locks the
//! *specific* fn keys each `expected/mwir.txt` / `expected/asm.txt` must
//! contain — a count is gameable; a missing `Cell.value` / `Pt.format` /
//! `Pair.sum` is not.
//!
//! Patterned on [`internal_error_census`]: every backend dump is
//! registered (required keys, or the zero-fn allowlist). Adding a dump
//! without updating the table — or dropping a required key — fails
//! `cargo test`.

/// Backend dumps that legitimately contain zero `Fn key=` lines, with why.
pub const ZERO_FN_DUMPS: &[(&str, &str)] = &[
    (
        "err-boot-dma-region-handoff-group/expected/asm.txt",
        "boot refuses before codegen emits fns (DMA handoff group diagnostic)",
    ),
    (
        "err-import-lower-body-helper/expected/asm.txt",
        "lower refuses body-only helper before emitting fns (decision 321)",
    ),
    (
        "err-mmio-offset-out-of-reach/expected/asm.txt",
        "layout/codegen refuses before emitting fns",
    ),
    (
        "err-mmio-signed-register/expected/asm.txt",
        "layout/codegen refuses before emitting fns",
    ),
    (
        "err-mwir-dynamic-panic/expected/mwir.txt",
        "lower refuses dynamic panic before emitting fns",
    ),
    (
        "err-mwir-if-else-scope-leak/expected/mwir.txt",
        "sema/lower refuse scope leak; mwir dump is empty",
    ),
];

/// Required `Fn key=` spellings per dump path under `tests/golden/`.
/// Each dump must contain every listed key (extra keys are fine).
pub const REQUIRED_FN_KEYS: &[(&str, &[&str])] = &[
    (
        "asm-arith/expected/asm.txt",
        &["checked_add", "checked_div", "mixed", "wrapping_add"],
    ),
    (
        "asm-async-basic/expected/asm.txt",
        &["Caller.run", "Counter.get"],
    ),
    (
        "asm-async-loop-checkpoint/expected/asm.txt",
        &["Store.load", "poll_until"],
    ),
    (
        "asm-calls/expected/asm.txt",
        &["add_one", "combo", "double"],
    ),
    ("asm-enum-match/expected/asm.txt", &["area"]),
    (
        "asm-generic/expected/asm.txt",
        &["double_identity", "fn:identity[u64]"],
    ),
    ("asm-loop/expected/asm.txt", &["sum_array", "sum_to"]),
    (
        "asm-struct/expected/asm.txt",
        &["Point.init", "Point.shift", "Point.sum", "make_and_sum"],
    ),
    (
        "asm-take/expected/asm.txt",
        &["consume", "consume_box", "use_takes"],
    ),
    (
        "boot-dma-reclaim/expected/mwir.txt",
        &[
            "BlkDriver.drain",
            "BlkDriver.init",
            "BlkDriver.on_queue_irq",
            "Ledger.mark",
            "Ledger.read_marks",
        ],
    ),
    (
        "check-completion-outcome/expected/mwir.txt",
        &["BlkDriver.abandon", "BlkDriver.init"],
    ),
    (
        "check-deriving-from-struct/expected/mwir.txt",
        &[
            "WrapError.from",
            "convert_wrapped",
            "direct_call",
            "parse",
            "settle",
            "guest_from",
        ],
    ),
    (
        "check-deriving-from/expected/mwir.txt",
        &[
            "WrapError.from",
            "convert_wrapped",
            "direct_call",
            "parse",
            "settle",
            "guest_from",
        ],
    ),
    (
        "check-device-claim/expected/asm.txt",
        &[
            "BlkDriver.ack_low",
            "BlkDriver.init",
            "BlkDriver.last_handled",
            "BlkDriver.on_queue_irq",
        ],
    ),
    (
        "check-device-claim/expected/mwir.txt",
        &[
            "BlkDriver.ack_low",
            "BlkDriver.init",
            "BlkDriver.last_handled",
            "BlkDriver.on_queue_irq",
        ],
    ),
    ("check-device-reset/expected/mwir.txt", &["BlkDriver.init"]),
    (
        "check-driver-mode-irq/expected/mwir.txt",
        &[
            "struct:BlkDriver[DriverMode.Irq].drain",
            "struct:BlkDriver[DriverMode.Irq].init",
            "struct:BlkDriver[DriverMode.Irq].on_queue_irq",
        ],
    ),
    (
        "check-driver-mode-poll/expected/mwir.txt",
        &[
            "struct:BlkDriver[DriverMode.Poll].drain",
            "struct:BlkDriver[DriverMode.Poll].init",
        ],
    ),
    (
        "check-enum-method-properties/expected/asm.txt",
        &[
            "State.consume",
            "State.double_score",
            "State.finish",
            "State.fresh",
            "State.score",
            "State.start",
            "bump_state",
            "p_assoc",
            "p_chain",
            "p_mut_param",
            "p_no_clobber",
            "p_sibling",
            "p_take",
            "p_variant_change",
        ],
    ),
    (
        "check-enum-methods/expected/asm.txt",
        &[
            "Cell.empty",
            "Cell.fill",
            "Cell.into_n",
            "Cell.value",
            "mut_path",
            "take_path",
            "guest_methods",
        ],
    ),
    (
        "check-enum-methods/expected/mwir.txt",
        &[
            "Cell.empty",
            "Cell.fill",
            "Cell.into_n",
            "Cell.value",
            "mut_path",
            "take_path",
            "guest_methods",
        ],
    ),
    (
        "check-format-bound/expected/mwir.txt",
        &[
            "Point.format",
            "Hue.format",
            "Point.max_formatted_len",
            "digest",
            "digest_small",
            "digest_hue",
            "digest_concat",
            "bool_digest",
            "as_u64",
            "inferred_format",
            "guest_format",
        ],
    ),
    (
        "check-format-fstring-guest/expected/asm.txt",
        &["Pt.format", "Store.fmt_len", "guest_fmt"],
    ),
    (
        "check-format-fstring-guest/expected/mwir.txt",
        &["Pt.format", "Store.fmt_len"],
    ),
    (
        "check-from-explicit/expected/mwir.txt",
        &[
            "WrapError.from",
            "convert_wrapped",
            "parse",
            "settle",
            "guest_from",
        ],
    ),
    ("check-fstring-bounds/expected/mwir.txt", &["Store.init"]),
    (
        "check-if-else-scoping/expected/mwir.txt",
        &["conditional_use"],
    ),
    (
        "check-import-alias-deriving-format/expected/asm.txt",
        &["Label.format", "digest", "guest_alias_format"],
    ),
    (
        "check-import-alias-deriving-from/expected/asm.txt",
        &["Box.from", "direct", "guest_alias_from"],
    ),
    (
        "check-import-alias-enum-method/expected/asm.txt",
        &[
            "Hue.code",
            "Hue.flip",
            "Hue.into_code",
            "Hue.red",
            "mut_path",
            "take_path",
            "guest_alias_enum_method",
        ],
    ),
    (
        "check-import-alias-method/expected/asm.txt",
        &[
            "Cnt.bump",
            "Cnt.get",
            "Cnt.into_n",
            "Cnt.zero",
            "mut_path",
            "take_path",
            "guest_alias_method",
        ],
    ),
    (
        "check-import-alias-pattern/expected/asm.txt",
        &["pick_annotated", "pick_constructed", "guest_alias_pattern"],
    ),
    (
        "check-import-alias-sig-dual/expected/asm.txt",
        &[
            "Grip.make",
            "Out.peel",
            "param_path",
            "return_path",
            "guest_sig_dual",
        ],
    ),
    (
        "check-import-alias-sig-param/expected/asm.txt",
        &["Grip.make", "drive", "guest_sig_param"],
    ),
    (
        "check-import-deriving-format/expected/asm.txt",
        &["Tag.format", "digest", "guest_import_format"],
    ),
    (
        "check-import-enum-method/expected/asm.txt",
        &["Color.code", "Color.red", "guest_enum_method"],
    ),
    (
        "check-import-lower/expected/asm.txt",
        &[
            "Pair.sum",
            "twice",
            "drive",
            "field_sum",
            "method_sum",
            "guest_import_lower",
        ],
    ),
    (
        "check-import-lower/expected/mwir.txt",
        &[
            "Pair.sum",
            "twice",
            "drive",
            "field_sum",
            "method_sum",
            "guest_import_lower",
        ],
    ),
    (
        "check-import-reachable-alias-generic/expected/asm.txt",
        &["wrap_box", "peel_box", "drive", "Store.guest_reachable"],
    ),
    (
        "check-import-reachable-alias/expected/asm.txt",
        &["Builder.build", "drive", "guest_reachable"],
    ),
    (
        "check-import-reachable-chain/expected/asm.txt",
        &["A.make", "B.get", "drive", "guest_reachable"],
    ),
    (
        "check-import-reachable-enum-payload/expected/asm.txt",
        &["make", "drive", "guest_reachable"],
    ),
    (
        "check-import-reachable-field-generic/expected/asm.txt",
        &["Maker.hold", "Maker.wrap", "drive", "Store.guest_reachable"],
    ),
    (
        "check-import-reachable-only/expected/asm.txt",
        &["Store.bump", "Store.init", "used"],
    ),
    (
        "check-import-reachable-only/expected/mwir.txt",
        &["Store.bump", "Store.init", "used"],
    ),
    (
        "check-import-reachable-private/expected/asm.txt",
        &["Maker.build", "drive", "guest_reachable"],
    ),
    (
        "check-import-reachable-type/expected/asm.txt",
        &["Maker.build", "drive", "guest_reachable"],
    ),
    (
        "check-interrupt-cell/expected/mwir.txt",
        &[
            "BlkDriver.drain",
            "BlkDriver.init",
            "BlkDriver.on_queue_irq",
        ],
    ),
    (
        "check-receipt-handoff/expected/mwir.txt",
        &["BlkDriver.init", "BlkDriver.submit_read"],
    ),
    ("check-stdlib-loaded/expected/mwir.txt", &["sample"]),
    (
        "check-string-bound/expected/mwir.txt",
        &["occupied", "byte0", "byte1", "probe", "guest_string"],
    ),
    ("check-untrusted-narrow-index/expected/asm.txt", &["good"]),
    ("check-untrusted-narrow-index/expected/mwir.txt", &["good"]),
    (
        "check-untrusted-narrowing/expected/asm.txt",
        &["narrow", "use_ok"],
    ),
    (
        "check-untrusted-narrowing/expected/mwir.txt",
        &["narrow", "use_ok"],
    ),
    ("check-untrusted-try/expected/asm.txt", &["narrow"]),
    ("check-untrusted-try/expected/mwir.txt", &["narrow"]),
    (
        "mwir-arith/expected/mwir.txt",
        &["checked_add", "checked_div", "mixed", "wrapping_add"],
    ),
    (
        "mwir-calls/expected/mwir.txt",
        &["add_one", "combo", "double"],
    ),
    ("mwir-enum-match/expected/mwir.txt", &["area"]),
    (
        "mwir-generic/expected/mwir.txt",
        &["double_identity", "fn:identity[u64]"],
    ),
    ("mwir-loop/expected/mwir.txt", &["sum_array", "sum_to"]),
    (
        "mwir-struct/expected/mwir.txt",
        &["Point.init", "Point.shift", "Point.sum", "make_and_sum"],
    ),
    (
        "mwir-take/expected/mwir.txt",
        &["consume", "consume_box", "use_takes"],
    ),
];

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
                let key = rest.split_whitespace().next().unwrap_or("");
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
        let zero: BTreeMap<&str, &str> = ZERO_FN_DUMPS.iter().copied().collect();
        let required: BTreeMap<&str, &[&str]> = REQUIRED_FN_KEYS.iter().copied().collect();

        let mut failures: Vec<String> = Vec::new();

        for (rel, keys) in &live {
            if keys.is_empty() {
                if !zero.contains_key(rel.as_str()) {
                    failures.push(format!(
                        "{rel}: dump has zero Fn keys but is not in ZERO_FN_DUMPS.\n\
                         Allowlist it deliberately (with a reason) or restore a guest \
                         path so the feature is reachable (plans/M9.md item KK)."
                    ));
                }
                continue;
            }
            if zero.contains_key(rel.as_str()) {
                failures.push(format!(
                    "{rel}: listed in ZERO_FN_DUMPS but dump now has keys {keys:?}.\n\
                     Remove it from the allowlist in guest_fn_key_census.rs."
                ));
                continue;
            }
            let Some(need) = required.get(rel.as_str()) else {
                failures.push(format!(
                    "{rel}: backend dump is not in REQUIRED_FN_KEYS.\n\
                     Add its required Fn keys to guest_fn_key_census.rs \
                     (plans/M9.md item KK)."
                ));
                continue;
            };
            let missing: Vec<&str> = need
                .iter()
                .copied()
                .filter(|k| !keys.contains(*k))
                .collect();
            if !missing.is_empty() {
                failures.push(format!(
                    "{rel}: missing required Fn keys {missing:?}.\n\
                     live keys: {keys:?}.\n\
                     Restore a guest-reachable path (`@test(runtime)`, actor `pub`, \
                     or `@task`) that calls the feature, or update REQUIRED_FN_KEYS \
                     deliberately (plans/M9.md item KK)."
                ));
            }
        }

        for rel in zero.keys() {
            if !live.contains_key(*rel) {
                failures.push(format!(
                    "{rel}: in ZERO_FN_DUMPS but no such golden dump exists."
                ));
            }
        }
        for rel in required.keys() {
            if !live.contains_key(*rel) {
                failures.push(format!(
                    "{rel}: in REQUIRED_FN_KEYS but no such golden dump exists."
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
