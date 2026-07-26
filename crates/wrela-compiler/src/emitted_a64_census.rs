//! **Census of hand-emitted A64 instruction words** (plans/M10.md item F0).
//!
//! The milestone's exit criteria demand "no hand-emitted A64 outside the
//! floor" and a test that "asserts this … so the floor cannot grow
//! silently." That test did not exist; decisions 613 and 620 satisfied D
//! and E3 with Rust emitters that push `encode::enc_*` word by word
//! (`emit_rt_enqueue`, `emit_rt_run_one`), relocating the Rust-shaped hole
//! from `layout.rs` / `layout/harness.rs` into `codegen.rs` rather than removing it. Those
//! decisions are frozen. This file locks the *inventory as it stands* —
//! every hand-shaped emitter, its measured word count under a pinned
//! reference configuration, and a category — so the next emitter is a
//! deliberate, reviewed bump.
//!
//! ## What this is, and what it is not
//!
//! It is a ratchet against silent growth of the hand-emitted surface. It
//! is **not** a claim that the exit criterion's "≤ 30 floor words / only
//! three functions" holds today — see [`FLOOR_WORDS`] and the table in
//! `plans/M10.md` item F0. Closing the milestone on a false claim is
//! exactly what this census exists to prevent.
//!
//! ## Scope
//!
//! **In:** functions in `layout.rs` / `codegen.rs` that construct fixed or
//! image-static A64 sequences by calling `encode::enc_*` / pushing raw
//! `u32` words outside the ordinary MWIR/FlowWir → asm path — the
//! runtime hand-asm inventory, the floor stubs, and the 613/620
//! specialization emitters.
//!
//! **Out (by design, not omitted quietly):** the ordinary codegen IR
//! lowering (`emit_one`, `emit_prologue`, `emit_arith_*`, queue/format/
//! group/await helpers, …). Those *do* emit instruction words; they are
//! the compiler backend that wrela source is supposed to compile through.
//! Counting them as "hand-emitted" would make the census a census of the
//! whole backend. Asm/`FnCtx` patch helpers, `layout_program` orchestration,
//! and thin wrappers that share another row's bytes are listed in the
//! closed-set allowlists below rather than silently dropped.
//!
//! ## Reference configuration
//!
//! Image-dependent emitters are measured under a fixed REF (same values in
//! `layout::emitted_a64_census_live_counts` /
//! `codegen::emitted_a64_census_specialization_live_counts`):
//!
//! - `capacity = 4`, `slot_size = 32` (one argument word past the 16-byte
//!   header)
//! - one select actor, no child polls, no drain, no xreply arms
//! - checkpoint: empty irq/wake (M6 path), no group arena
//! - deadline scan/poll: `arena_capacity = 1`, one turn area
//! - boot init: one actor, `state_size = 8`, no `init` calls
//! - entry driver: zero runtime tests, `cores = 1`, no boot_init, no
//!   `rt_run_one`
//! - drain: core 0, one request lane + one reply lane
//! - group child poll: one child at index 0
//! - secondary core entry: core 1
//!
//! A varying emitter still ratchets when its input is pinned. Counts are
//! recomputed by the measure functions every test run — not hand-typed
//! forever and forgotten.
//!
//! ## Categories
//!
//! Each entry is exactly one of:
//! - **floor** — justified against ROADMAP's three categories plus this
//!   plan's category 4 (decision 650): pre-SP; must-clobber-no-register;
//!   no expression form (`brk`/…); stored code address jumped to
//! - **image_static** — specialization sanctioned by decisions 613 / 620 /
//!   623 / 630 / 633 / 680 (`emit_rt_enqueue`, `emit_rt_run_one`,
//!   `emit_rt_child_poll`, `emit_rt_select_and_run`, cross-core quartet,
//!   `emit_boot_init`)
//! - **not_yet_migrated** — owned by a named remaining item (G, K;
//!   F/F2/H/I examined or migrated)
//! - **unclassified** — could not be confidently placed, **or** an
//!   examined stated residue (item I's `build_entry_driver`) still counted
//!
//! ## Two independent locks (neither replaces the other)
//!
//! 1. **Per-function word counts** ([`EMITTED_A64_ENTRIES`] vs the live
//!    measure helpers) — catches an existing emitter that *grows*,
//!    *shrinks*, or is *removed*.
//! 2. **Per-file `encode::enc_` site counts** ([`ENCODE_ENC_SITES_BY_FILE`]
//!    vs a source-tree scan) — catches a *brand-new* emission site, even
//!    when nobody added it to the measure lists. Modelled on
//!    [`crate::internal_error_census`]: the live number is read from the
//!    files on disk, so silence requires updating the lock.
//!
//! ### Test-module exclusion (site scan only)
//!
//! `layout.rs` / `codegen.rs` carry large `#[cfg(test)]` /
//! `#[cfg(all(test, …))]` modules (`tests`, `harness_jit`,
//! `rt_child_poll_tests`, …) that call `encode::enc_*` heavily for the
//! macOS JIT harness. Those regions are stripped before counting, by
//! brace-matching from a cfg-test attribute immediately followed (blank
//! lines allowed) by `mod NAME {`. How this can go wrong: a test module
//! attributed with a form we do not recognise (e.g. `#[cfg(any(test,
//! feature = "…"))]`, or `cfg(test)` on a parent `mod` that wraps
//! production code) would be counted or skipped incorrectly; an
//! always-compiled `mod` that only looks like a test would be counted.
//! Individual `#[cfg(test)]` *functions* outside those modules (e.g.
//! `push_abort_tail`) are **kept** — they are emission sites in the crate.
//!
//! ### What the site scan still cannot catch
//!
//! An emitter built entirely by calling existing helpers (`push_load_imm`,
//! `Asm::load_imm`, …) without a new `encode::enc_*` token in source adds
//! no site and does not trip this lock. The per-function word-count lock
//! still catches growth of a *named* inventory entry; a wholly new
//! helper-only emitter that is never registered in the measure lists is
//! the residual hole. Stated rather than papered over.

/// One locked hand-emitter. `words` is the live length under REF.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EmitterEntry {
    pub name: &'static str,
    pub file: &'static str,
    pub words: usize,
    pub category: Category,
    /// Owning plan item, floor subcategory, or decision id — for humans.
    pub note: &'static str,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Category {
    Floor,
    ImageStatic,
    NotYetMigrated,
    Unclassified,
}

/// The locked inventory. Adding / removing a row, or changing `words`,
/// without updating this table fails the unit test below.
pub const EMITTED_A64_ENTRIES: &[EmitterEntry] = &[
    // --- floor -----------------------------------------------------------
    // Halt sequence: 4×load_imm exit_code + 4×load_imm exit_addr + str +
    // 4×load_imm EXIT_MMIO + str + brk = 15. Category 1 materialization of
    // constants plus category 3 `brk #0`. Used by `build_entry_stub` and by
    // `layout_program`'s build-image abort landings (decision 655).
    EmitterEntry {
        name: "push_halt",
        file: "layout/harness.rs",
        words: 15,
        category: Category::Floor,
        note: "floor cat1+cat3 halt sequence",
    },
    // SP install (4×imm + mov sp = 5) then push_halt (15) = 20. Whole
    // function is floor for `layout_program`'s no-runtime placeholder.
    // Double-counts push_halt's 15 in FLOOR_SUM_OF_ROWS; FLOOR_WORDS
    // subtracts that overlap.
    EmitterEntry {
        name: "build_entry_stub",
        file: "layout/harness.rs",
        words: 20,
        category: Category::Floor,
        note: "floor cat1 SP install + halt",
    },
    // Decision 650 category 4: 4×imm of OFF_TEST_CONTINUATION + ldr + br.
    // Item C: overwrites the compiled `__wrela_abort_tail` stub.
    EmitterEntry {
        name: "build_abort_tail_codegen_fn",
        file: "layout/harness.rs",
        words: 6,
        category: Category::Floor,
        note: "floor cat4 abort long-jump",
    },
    // --- image-static specialization (613 / 620 / 623 / 630 / 633) -------
    // REF: capacity=4, slot_size=32 → same length as build_ring_enqueue.
    EmitterEntry {
        name: "emit_rt_enqueue",
        file: "codegen.rs",
        words: 55,
        category: Category::ImageStatic,
        note: "decision 613; per-actor specialized admission",
    },
    // REF: one select, no drain, no child polls. Grows with select/poll
    // counts (image-static); pin is the REF shape above.
    EmitterEntry {
        name: "emit_rt_run_one",
        file: "codegen.rs",
        words: 46,
        category: Category::ImageStatic,
        note: "decision 620; per-core specialized scheduler tick",
    },
    // REF: one child at index 0. E4 deleted hand-asm `build_group_child_poll`.
    EmitterEntry {
        name: "emit_rt_child_poll",
        file: "codegen.rs",
        words: 75,
        category: Category::ImageStatic,
        note: "decision 623; per-site specialized group-child poll",
    },
    // REF: one sync method, no xreply, frame = TURN_RECORD_SIZE (no lineage).
    EmitterEntry {
        name: "emit_rt_select_and_run",
        file: "codegen.rs",
        words: 124,
        category: Category::ImageStatic,
        note: "decision 630; per-actor specialized select/dispatch (item F)",
    },
    // REF: one request ring cap=4 slot=32 (inlined enqueue; decision 637).
    EmitterEntry {
        name: "emit_rt_xsend",
        file: "codegen.rs",
        words: 69,
        category: Category::ImageStatic,
        note: "decision 633; per-edge specialized cross-core send (item F2)",
    },
    // REF: one reply ring cap=4.
    EmitterEntry {
        name: "emit_rt_xreply",
        file: "codegen.rs",
        words: 58,
        category: Category::ImageStatic,
        note: "decision 633; per-edge specialized cross-core reply (item F2)",
    },
    // REF: core 0, one request lane + one reply lane.
    EmitterEntry {
        name: "emit_rt_drain",
        file: "codegen.rs",
        words: 126,
        category: Category::ImageStatic,
        note: "decision 633; per-core specialized inbound drain (item F2)",
    },
    // REF: core 1. Includes 5 floor-cat1 SP-install words inside the body.
    EmitterEntry {
        name: "emit_secondary_core_entry",
        file: "codegen.rs",
        words: 26,
        category: Category::ImageStatic,
        note: "decision 633; per-core secondary entry (item F2); 5 floor-cat1 SP",
    },
    // REF: empty group/irq/wake. Includes 5 floor-cat2 save/restore words
    // inside the body (decision 673) — counted here until a later floor
    // extraction, same carve-out shape as secondary-core SP (decision 636).
    EmitterEntry {
        name: "emit_checkpoint_and_vector_stub",
        file: "codegen.rs",
        words: 26,
        category: Category::ImageStatic,
        note: "decision 670; contains 5 floor-cat2 save/restore words",
    },
    EmitterEntry {
        name: "emit_deadline_scan_and_delivery",
        file: "codegen.rs",
        words: 57,
        category: Category::ImageStatic,
        note: "decision 670; REF arena_capacity=1, one turn area",
    },
    EmitterEntry {
        name: "emit_deadline_poll",
        file: "codegen.rs",
        words: 38,
        category: Category::ImageStatic,
        note: "decision 670; REF arena_capacity=1",
    },
    // REF: one actor, state_size=8, no init calls (memset loop).
    EmitterEntry {
        name: "emit_boot_init",
        file: "codegen.rs",
        words: 14, // measured; bump if live count differs
        category: Category::ImageStatic,
        note: "decision 680; memset zero-fill (681); was layout build_boot_init",
    },
    // --- not yet migrated ------------------------------------------------
    EmitterEntry {
        name: "push_turn_addr_from_id",
        file: "layout/harness.rs",
        words: 7,
        category: Category::NotYetMigrated,
        note: "shared index→address; remaining harness paths",
    },
    // --- unclassified ----------------------------------------------------
    // Micro-helper used by both floor and migratable paths; not itself a
    // floor category and not owned by one migration item.
    EmitterEntry {
        name: "push_load_imm",
        file: "layout/harness.rs",
        words: 4,
        category: Category::Unclassified,
        note: "micro-helper; 4 words always",
    },
    // Test-only residue of item C (`#[cfg(test)]`). Not in production
    // images; still emits A64 words in the crate.
    EmitterEntry {
        name: "push_abort_tail",
        file: "layout/harness.rs",
        words: 19,
        category: Category::Unclassified,
        note: "cfg(test) only; C left it for the latch probe",
    },
    // plans/M10.md item I / decisions 685–689: examined stated residue.
    // REF zero tests = 94. Floor cats embedded (not in FLOOR_WORDS yet):
    // 5 cat1 SP, 1 cat3 halt brk; cat4 landing-pad arms appear only when
    // tests/boot_init exist (9 words/site). Migratable BL targets already
    // go to existing `__wrela_*` / `rt_*` keys; no further migrate without
    // new keys or language surface (decision 685).
    EmitterEntry {
        name: "build_entry_driver",
        file: "layout/harness.rs",
        words: 94,
        category: Category::Unclassified,
        note: "I residue (685–689); REF zero tests; cat1 SP + cat3 brk embedded",
    },
];

/// Sum of `words` over floor rows (includes `push_halt`∩`build_entry_stub`
/// overlap). Prefer [`FLOOR_WORDS`] for the exit-criterion comparison.
pub const FLOOR_SUM_OF_ROWS: usize = 41; // 15 + 20 + 6

/// Floor total after removing the `push_halt` / `build_entry_stub` overlap:
/// `push_halt` (15) + entry-stub-only SP prefix (5) + abort tail (6) = 26.
///
/// Compared against the exit criterion's ≤ 30 and ROADMAP's "roughly twenty
/// instructions". This count is the pure-floor *functions* only — it does
/// **not** include floor-category instructions still embedded inside
/// not-yet-migrated functions (checkpoint's 5-word save/restore, secondary
/// core / entry-driver SP installs, remaining `brk`s in F/F2). Those sit
/// in the not-yet-migrated total until their owning item extracts them.
pub const FLOOR_WORDS: usize = 26;

pub const IMAGE_STATIC_SUM_OF_ROWS: usize = 714; // was 593; G +26+57+38

pub const NOT_YET_MIGRATED_SUM_OF_ROWS: usize = 7; // was 128; G −26−57−38

pub const UNCLASSIFIED_SUM_OF_ROWS: usize = 117; // 4 + 19 + 94 (I stated residue)

/// Sum of every locked row. Includes helper/wrapper overlap (e.g.
/// `build_rt_enqueue` == `build_ring_enqueue`, `build_entry_stub` embeds
/// `push_halt`). Useful as a ratchet total; not "unique words in one image".
pub const GRAND_TOTAL_SUM_OF_ROWS: usize = 879; // unchanged; G moves not-yet→image-static

/// Per-file counts of the contiguous `encode::enc_` substring under
/// `crates/wrela-compiler/src/`, excluding `#[cfg(test)]` /
/// `#[cfg(all(test, …))]` modules (see module docs) and excluding this
/// census file (it documents the needle). Measured 2026-07-26. Adding a
/// call site in a listed file without bumping its count — or introducing
/// the needle in a new file — fails the unit test below.
/// Item G: checkpoint/deadline moved layout→codegen (811 / 76).
pub const ENCODE_ENC_SITES_BY_FILE: &[(&str, usize)] = &[
    ("codegen.rs", 811),
    ("layout.rs", 8),
    ("layout/harness.rs", 68),
];

/// Total sites across [`ENCODE_ENC_SITES_BY_FILE`].
pub const ENCODE_ENC_SITE_COUNT: usize = {
    let mut n = 0;
    let mut i = 0;
    while i < ENCODE_ENC_SITES_BY_FILE.len() {
        n += ENCODE_ENC_SITES_BY_FILE[i].1;
        i += 1;
    }
    n
};

#[cfg(test)]
mod tests {
    use super::*;
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
            if rel == "emitted_a64_census.rs" {
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

        let expected: BTreeMap<String, usize> = ENCODE_ENC_SITES_BY_FILE
            .iter()
            .map(|(f, n)| ((*f).to_string(), *n))
            .collect();

        assert_eq!(
            live, expected,
            "encode::enc_ emission-site census drifted.\n\
             Update ENCODE_ENC_SITES_BY_FILE in emitted_a64_census.rs in the \
             same commit that adds or removes a call site \
             (plans/M10.md item F0 — addition lock).\n\
             live={live:?}\n\
             expected={expected:?}"
        );

        let total: usize = live.values().sum();
        assert_eq!(
            total, ENCODE_ENC_SITE_COUNT,
            "ENCODE_ENC_SITE_COUNT ({ENCODE_ENC_SITE_COUNT}) != sum of per-file counts ({total})"
        );
        assert_eq!(
            ENCODE_ENC_SITE_COUNT, 887,
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
    const BACKEND_EMITTERS: &[&str] = &[
        "codegen.rs::emit_one",
        "codegen.rs::emit_queue_prepare",
        "codegen.rs::emit_queue_publish",
        "codegen.rs::emit_queue_suppress_interrupts",
        "codegen.rs::emit_queue_drain",
        "codegen.rs::emit_device_reset",
        "codegen.rs::emit_doorbell_poll_park",
        "codegen.rs::emit_queue_claim",
        "codegen.rs::emit_queue_recover",
        "codegen.rs::emit_queue_reclaim",
        "codegen.rs::emit_desc_entry",
        "codegen.rs::emit_desc_entry_len_reg",
        "codegen.rs::emit_format_scalar",
        "codegen.rs::emit_string_concat",
        "codegen.rs::emit_index_addr",
        "codegen.rs::emit_bytes_index_addr",
        "codegen.rs::emit_placed_index_addr",
        "codegen.rs::emit_arith_checked",
        "codegen.rs::emit_arith_wrapping",
        "codegen.rs::emit_div_rem",
        "codegen.rs::emit_shift",
        "codegen.rs::emit_convert",
        "codegen.rs::emit_prologue",
        "codegen.rs::emit_epilogue",
        "codegen.rs::emit_interrupt_cell_addr",
        "codegen.rs::emit_interrupt_cell_rmw",
        "codegen.rs::emit_async_entry",
        "codegen.rs::emit_async_epilogue",
        "codegen.rs::emit_group_create",
        "codegen.rs::emit_group_start",
        "codegen.rs::emit_group_close",
        "codegen.rs::emit_flow_op",
        "codegen.rs::emit_group_cancelled_flags",
        "codegen.rs::emit_async_cancelled_tail",
        "codegen.rs::emit_compose_group_join_result",
        "codegen.rs::emit_group_addr_from_temp",
        "codegen.rs::emit_await_suspend",
        "codegen.rs::emit_await_resume",
        "codegen.rs::emit_checkpoint_cancellation_test",
        "codegen.rs::emit_copy_staged_reply",
        "codegen.rs::emit_recompose_staged_result",
        "codegen.rs::emit_compose_staged_reply",
        "codegen.rs::emit_compose_from_reply_tag",
        "codegen.rs::emit_transition",
        "codegen.rs::emit_flat_entry",
        "codegen.rs::emit_flowwir_fn",
        "codegen.rs::emit_fn",
        "codegen.rs::emit_marshal_and_call",
        "codegen.rs::emit_send",
        "codegen.rs::emit_self_path",
        "codegen.rs::emit_now",
        "codegen.rs::push_turn_addr_from_id",
    ];

    /// Touches encoders but is not a hand-emitted runtime routine (patch
    /// helpers, orchestration, thin wrappers whose bytes are counted on
    /// another row, measure fns themselves).
    const NON_INVENTORY: &[&str] = &[
        "layout.rs::patch_bl",
        "layout.rs::patch_load_imm_words",
        "layout.rs::patch_adrp_add",
        "layout.rs::layout_program",
        "layout/harness.rs::emitted_a64_census_live_counts",
        "codegen.rs::emitted_a64_census_specialization_live_counts",
        // M10 F: JIT-only materialize of `emit_rt_select_and_run` (patches
        // Call/MailboxAddr/Turns*); not a hand-asm emitter row.
        "layout/harness.rs::build_rt_select_and_run",
        "layout/harness.rs::build_rt_enqueue",
        "layout/harness.rs::build_checkpoint_and_vector_stub",
        // M10 G: thin materialize of `emit_checkpoint_and_vector_stub`.
        "layout/harness.rs::build_checkpoint_and_vector_stub_ex",
        "codegen.rs::emit_deadline_scan_and_delivery_into",
        "codegen.rs::emit_deadline_poll_into",
        "layout/harness.rs::install_abort_tail_floor",
    ];

    /// Census rows that are real emitters (measured word count) but whose
    /// body does not itself contain `encode::enc_` — thin wrappers that
    /// forward to a counted sibling. Still locked by the live-count test.
    const WRAPPER_EMITTERS: &[&str] = &[
        // Item G: public entry forwards to `*_into` (which holds the enc_).
        "codegen.rs::emit_deadline_scan_and_delivery",
    ];

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

        let expected: BTreeMap<&str, usize> = EMITTED_A64_ENTRIES
            .iter()
            .map(|e| (e.name, e.words))
            .collect();

        assert_eq!(
            live, expected,
            "hand-emitted A64 census drifted.\n\
             Update EMITTED_A64_ENTRIES in emitted_a64_census.rs in the same \
             commit that adds, removes, or grows an emitter \
             (plans/M10.md item F0).\n\
             live={live:?}\n\
             expected={expected:?}"
        );

        let floor_rows: usize = EMITTED_A64_ENTRIES
            .iter()
            .filter(|e| e.category == Category::Floor)
            .map(|e| e.words)
            .sum();
        let image_static: usize = EMITTED_A64_ENTRIES
            .iter()
            .filter(|e| e.category == Category::ImageStatic)
            .map(|e| e.words)
            .sum();
        let not_yet: usize = EMITTED_A64_ENTRIES
            .iter()
            .filter(|e| e.category == Category::NotYetMigrated)
            .map(|e| e.words)
            .sum();
        let unclassified: usize = EMITTED_A64_ENTRIES
            .iter()
            .filter(|e| e.category == Category::Unclassified)
            .map(|e| e.words)
            .sum();
        let grand: usize = EMITTED_A64_ENTRIES.iter().map(|e| e.words).sum();

        assert_eq!(floor_rows, FLOOR_SUM_OF_ROWS, "FLOOR_SUM_OF_ROWS");
        assert_eq!(
            image_static, IMAGE_STATIC_SUM_OF_ROWS,
            "IMAGE_STATIC_SUM_OF_ROWS"
        );
        assert_eq!(
            not_yet, NOT_YET_MIGRATED_SUM_OF_ROWS,
            "NOT_YET_MIGRATED_SUM_OF_ROWS"
        );
        assert_eq!(
            unclassified, UNCLASSIFIED_SUM_OF_ROWS,
            "UNCLASSIFIED_SUM_OF_ROWS"
        );
        assert_eq!(grand, GRAND_TOTAL_SUM_OF_ROWS, "GRAND_TOTAL_SUM_OF_ROWS");
        assert_eq!(
            FLOOR_WORDS, 26,
            "adjusted floor total is part of the ratchet"
        );
        // Sanity: FLOOR_WORDS == push_halt + SP prefix + abort tail.
        assert_eq!(15 + 5 + 6, FLOOR_WORDS);
    }

    #[test]
    fn emitted_a64_hand_emitter_set_is_closed() {
        let candidates = scan_enc_fns();
        let census_keys: BTreeMap<String, ()> = EMITTED_A64_ENTRIES
            .iter()
            .map(|e| (format!("{}::{}", e.file, e.name), ()))
            .collect();
        let backend: BTreeMap<&str, ()> = BACKEND_EMITTERS.iter().map(|n| (*n, ())).collect();
        let non_inv: BTreeMap<&str, ()> = NON_INVENTORY.iter().map(|n| (*n, ())).collect();
        let wrappers: BTreeMap<&str, ()> = WRAPPER_EMITTERS.iter().map(|n| (*n, ())).collect();

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
            "new encode::enc_ emitter(s) not in EMITTED_A64_ENTRIES, \
             BACKEND_EMITTERS, or NON_INVENTORY — classify deliberately \
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
                 emitter — remove it from EMITTED_A64_ENTRIES in the same \
                 commit that removes a hand-asm twin after its specialized \
                 replacement is proven (E4 deleted build_rt_run_one / \
                 build_group_child_poll)"
            );
        }
        for key in wrappers.keys() {
            assert!(
                census_keys.contains_key(*key),
                "WRAPPER_EMITTERS entry `{key}` missing from EMITTED_A64_ENTRIES"
            );
        }
    }
}
