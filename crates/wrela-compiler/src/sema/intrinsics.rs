pub const IMAGE_BUILDER_SURFACE: &[&str] = &[
    "Image",
    "Image.device",
    "Image.driver",
    "Image.actor",
    "Image.pool",
    "Image.dma_pool",
    "Image.on_failure",
    "Image.check_layout",
    "Image.seal",
    "ImageDecl.handle",
];

pub const EXCEPTIONS: &[(&str, &str)] = &[
    (
        "now",
        "05 §5: a sealed effect. It is forbidden in comptime and ISR context and is \
         recorded/replayed through the recorder's `ClockRead` choice entry — wrela source can \
         express neither the prohibition nor the choice-point binding.",
    ),
    (
        "entropy",
        "05 §5 / plans/M17.md: a sealed effect `entropy[N]() -> Bytes[N]`. Forbidden in \
         comptime and ISR like `now()`, and recorded/replayed through the recorder's \
         `EntropyRead` choice entry — wrela source can express neither the prohibition nor \
         the choice-point binding.",
    ),
    (
        "dmb.ishld",
        "plans/M15.md item H / 04 §3 acquire barrier: one inlined DMB ISHLD word. Same \
         runtime-wr-only gate as `dmb.ishst`.",
    ),
    (
        "dmb.ishst",
        "plans/M15.md item H / 04 §3 publish barrier: one inlined DMB ISHST word. Legal only \
         inside stdlib/core/runtime.wr — not an author-facing 05 §9 surface; wrela has no fence \
         expression form.",
    ),
    (
        "Array.map_take",
        "05 §7: sealed whole-array consumption on the builtin `[T; N]` — there is no \
         wrela type to attach the method to; the receiver is a language primitive.",
    ),
    (
        "Array.try_map_take",
        "05 §7: sealed fallible whole-array consumption with unwind-and-reclaim on Err — \
         same reason as `Array.map_take`.",
    ),
    (
        "Mmio.read",
        "03 §2: a volatile read of a declared register. Direction, width and endianness come \
         from the `@layout(mmio)` declaration and it lowers to one load that may not be \
         reordered or elided — wrela has no volatile expression form.",
    ),
    (
        "Mmio.write",
        "03 §2: the write half of the same access, with the same reasoning.",
    ),
    (
        "Untrusted.checked_le",
        "03 §8: the sealed wrapper's one narrowing. The seal is the point — wrela source may \
         neither construct nor unwrap an `Untrusted[T]`, so its narrowing cannot be wrela \
         either without breaking the seal.",
    ),
    (
        "Device.claim",
        "03 §1: consumes a `DeviceCap` and partitions its MMIO — capability consumption is a \
         build-time proof, not a call.",
    ),
    (
        "Device.map_partition",
        "03 §9: a bring-up state transition, proven at build time.",
    ),
    (
        "Device.negotiate",
        "03 §9: feature negotiation against `required_features` declared on the image graph.",
    ),
    (
        "Device.read_capacity_sectors",
        "03 §9: a transport read whose legality depends on the bring-up state proven above it.",
    ),
    (
        "Device.reset",
        "03 §9: the transport reset transition; it invalidates receipts, which is a compiler \
         fact about outstanding work, not a library one.",
    ),
    (
        "Device.start",
        "03 §9: the DRIVER_OK transition that makes queues usable.",
    ),
    (
        "Device.take_irq",
        "03 §1: splits an `IrqCap` out of a `DeviceCap` — capability creation, which by 03 §1 \
         no address, import or cast may do.",
    ),
    (
        "VirtQueue.claim",
        "03 §5: claims a completion against a receipt the compiler tracks.",
    ),
    (
        "VirtQueue.configure",
        "03 §4: binds the ring the compiler laid out into the device's queue registers.",
    ),
    (
        "VirtQueue.drain",
        "03 §5: drains the used ring; the number of outstanding receipts is a compiler fact.",
    ),
    (
        "VirtQueue.prepare_block",
        "03 §4: writes descriptors into a permit's reserved slots — the slots exist only \
         because `reserve` proved them.",
    ),
    (
        "VirtQueue.publish",
        "03 §4: the available-ring publication and its release barrier.",
    ),
    (
        "VirtQueue.reclaim",
        "03 §9: reclaims slots after a ring reset, which is a state-machine transition.",
    ),
    (
        "VirtQueue.recover",
        "03 §5/§9: resolves a receipt through the Recovery state into a `CompletionOutcome`.",
    ),
    (
        "VirtQueue.reject",
        "03 §5: consumes a permit without publishing — the unwind half of the reservation \
         protocol, and the reason the protocol can be proven at all.",
    ),
    (
        "VirtQueue.reserve",
        "03 §4 / plans/M13.md item M: proof-conditioned reservation — \
         `Result[QueuePermit, CapacityError]` collapses to `QueuePermit` where \
         `sema::reserve_proof` holds; the intrinsic is where the proof attaches.",
    ),
    (
        "VirtQueue.suppress_interrupts",
        "03 §4: flips the ring's no-interrupt flag with the ordering the device contract \
         requires.",
    ),
    (
        "IrqCap.bind",
        "03 §6: binds a plain `fn` to a vector at build time, and the binding is what makes \
         the compiler restrict that function's transitive effects to the ISR set. A graph \
         edge, not a call.",
    ),
    (
        "IrqCap.unmask",
        "03 §6: unmasks the bound vector; legal only once the binding above exists.",
    ),
    (
        "InterruptCell.new",
        "03 §6: the cell's one source-visible constructor. WEAKEST ENTRY IN THIS LIST — it \
         lowers to a plain `Copy` (`lower.rs`); it is intrinsic only because the type is \
         compiler-known. See the accretion note in plans/M9.md item AA.",
    ),
    (
        "InterruptCell.fetch_or_release",
        "03 §6: release-ordered fetch-or on the ISR/ordinary channel; lowers to a dedicated \
         ordered instruction. wrela has no atomic or ordering expression form.",
    ),
    (
        "InterruptCell.load_acquire",
        "03 §6: acquire-ordered load, same reasoning.",
    ),
    (
        "InterruptCell.store_release",
        "03 §6: release-ordered store, same reasoning.",
    ),
    (
        "InterruptCell.swap_acquire",
        "03 §6: acquire-ordered swap, same reasoning.",
    ),
    (
        "wake",
        "03 §6: a statically bound bottom-half wake. Its argument is a *method reference* \
         resolved at build time, and the language has no value form for one (03 §6, and \
         `check_field_expr`'s own \"cannot reference method without calling it\").",
    ),
    (
        "Group.join_all",
        "02 §9: `with group(...)`'s scoped join. It is a suspension point with compiler-\
         reserved child slots, which is a contract rather than a library call.",
    ),
    (
        "Group.start",
        "02 §9: starts a child into one of those reserved slots.",
    ),
];

pub const FORMAT_KEY_SITES: &[(&str, &[&str], &str)] = &[
    (
        "Image.{mname}",
        &["Image.device", "Image.pool", "Image.dma_pool"],
        r#"if mname == "device" || mname == "pool" || mname == "dma_pool" {"#,
    ),
    (
        "Mmio.{op}",
        &["Mmio.read", "Mmio.write"],
        r#"if !matches!(op, "read" | "write") {"#,
    ),
    (
        "InterruptCell.{method}",
        &[
            "InterruptCell.store_release",
            "InterruptCell.swap_acquire",
            "InterruptCell.fetch_or_release",
        ],
        r#""store_release" | "swap_acquire" | "fetch_or_release" => {"#,
    ),
    (
        "Image.{name}",
        &["Image.driver", "Image.actor"],
        r#""driver" | "actor" => {"#,
    ),
];

pub const UNPRODUCED_CONSUMER_KEYS: &[(&str, &str)] = &[
    (
        "VirtQueue.poll_sources",
        "`is_queue_op_deferred` names it for `lower`/`flowwir_lower`, but `check_virtqueue_method` \
         rejects the method with `unimplemented_at` first, so the key is unreachable.",
    ),
    (
        "VirtQueue.completions_pending",
        "Same as `poll_sources` — rejected in sema before any node is built.",
    ),
];

pub fn is_bare_resolvable(name: &str) -> bool {
    matches!(
        name,
        "Image" | "now" | "entropy" | "wake" | "group" | "pool"
    )
}

pub fn written_down_surface() -> Vec<&'static str> {
    let mut all: Vec<&'static str> = IMAGE_BUILDER_SURFACE.to_vec();
    all.extend(EXCEPTIONS.iter().map(|(k, _)| *k));
    all.sort_unstable();
    let before = all.len();
    all.dedup();
    assert_eq!(
        before,
        all.len(),
        "a name is listed twice across IMAGE_BUILDER_SURFACE / EXCEPTIONS"
    );
    all
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    const BODIES_SRC: &str = include_str!("bodies.rs");
    const TRANSPORT_SRC: &str = include_str!("transport.rs");
    const ACTOR_SRC: &str = include_str!("actor.rs");

    fn marker() -> String {
        format!("{}{}", "TypedExprKind::Intrinsic", " {")
    }

    #[derive(Debug)]
    struct KeySite {
        file: String,
        line: usize,
        form: KeyForm,
    }

    #[derive(Debug, PartialEq)]
    enum KeyForm {
        Literal(String),
        Template(String),
    }

    fn scan_key_sites(label: &str, src: &str) -> Vec<KeySite> {
        let marker = marker();
        let lines: Vec<&str> = src.lines().collect();
        let mut out = Vec::new();
        for (i, line) in lines.iter().enumerate() {
            if line.trim_start().starts_with("//") {
                continue;
            }
            let Some(col) = line.find(&marker) else {
                continue;
            };
            let region = brace_region(&lines, i, col + marker.len() - 1, label);
            let Some(value) = key_binding(&region) else {
                continue;
            };
            let form = if let Some(rest) = value.strip_prefix('"') {
                let end = rest.find('"').unwrap_or_else(|| {
                    panic!("{label}:{}: unterminated key literal `{value}`", i + 1)
                });
                KeyForm::Literal(rest[..end].to_string())
            } else if let Some(rest) = value.strip_prefix("format!(\"") {
                let end = rest.find('"').unwrap_or_else(|| {
                    panic!("{label}:{}: unterminated key template `{value}`", i + 1)
                });
                KeyForm::Template(rest[..end].to_string())
            } else {
                panic!(
                    "{label}:{}: an intrinsic is built with a `key` this census cannot resolve \
                     (`key: {}`). plans/M9.md item AA requires every intrinsic name to be \
                     statically knowable: spell it as a string literal, or as a `format!` whose \
                     variable is bounded by a guard recorded in FORMAT_KEY_SITES.",
                    i + 1,
                    value.chars().take(40).collect::<String>()
                );
            };
            out.push(KeySite {
                file: label.to_string(),
                line: i + 1,
                form,
            });
        }
        out
    }

    fn brace_region(lines: &[&str], start: usize, open: usize, label: &str) -> String {
        let mut depth = 0i32;
        let mut region = String::new();
        for (n, line) in lines.iter().enumerate().skip(start).take(60) {
            let from = if n == start { open } else { 0 };
            for ch in line[from..].chars() {
                match ch {
                    '{' => {
                        depth += 1;
                        if depth == 1 {
                            continue;
                        }
                    }
                    '}' => {
                        depth -= 1;
                        if depth == 0 {
                            return region;
                        }
                    }
                    _ => {}
                }
                region.push(ch);
            }
            region.push(' ');
        }
        panic!(
            "{label}:{}: an intrinsic marker whose braces do not balance within 60 lines",
            start + 1
        );
    }

    fn key_binding(region: &str) -> Option<&str> {
        let mut from = 0usize;
        while let Some(rel) = region[from..].find("key:") {
            let at = from + rel;
            let boundary = at == 0
                || !region[..at]
                    .chars()
                    .next_back()
                    .is_some_and(|c| c.is_alphanumeric() || c == '_');
            if boundary {
                return Some(region[at + "key:".len()..].trim_start());
            }
            from = at + 4;
        }
        None
    }

    fn crate_sources() -> Vec<(String, String)> {
        let crates = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .expect("crates/ dir")
            .to_path_buf();
        assert!(
            crates.ends_with("crates"),
            "expected the workspace `crates/` dir, got {}",
            crates.display()
        );
        let mut files = Vec::new();
        collect_rs(&crates, &mut files);
        files
            .into_iter()
            .map(|p| {
                let label = p
                    .strip_prefix(&crates)
                    .unwrap_or(&p)
                    .to_string_lossy()
                    .to_string();
                let src = std::fs::read_to_string(&p).expect("read crate source");
                (label, src)
            })
            .collect()
    }

    #[test]
    fn intrinsic_surface_equals_the_written_down_list() {
        let mut sites = scan_key_sites("sema/bodies.rs", BODIES_SRC);
        sites.extend(scan_key_sites("sema/transport.rs", TRANSPORT_SRC));
        sites.extend(scan_key_sites("sema/actor.rs", ACTOR_SRC));
        assert!(
            sites.len() >= 30,
            "the census scan found only {} key sites in bodies+transport+actor — the marker \
             no longer matches the source, so this test is proving nothing",
            sites.len()
        );
        let mut literals = BTreeSet::new();
        let mut templates = BTreeSet::new();
        for s in &sites {
            match &s.form {
                KeyForm::Literal(k) => {
                    literals.insert(k.clone());
                }
                KeyForm::Template(t) => {
                    templates.insert(t.clone());
                }
            }
        }

        let declared_templates: BTreeSet<String> = FORMAT_KEY_SITES
            .iter()
            .map(|(t, _, _)| (*t).to_string())
            .collect();
        assert_eq!(
            templates, declared_templates,
            "a `key: format!(...)` construction site in bodies.rs is not resolved in \
             FORMAT_KEY_SITES (or a resolved one is gone). A templated key's expansion cannot be \
             read off the construction site — resolve the variable's range by hand, record the \
             guard that bounds it, and add it there."
        );

        let mut live: BTreeSet<String> = literals;
        for (_, expansion, _) in FORMAT_KEY_SITES {
            for k in *expansion {
                assert!(
                    live.insert((*k).to_string()),
                    "`{k}` is produced both by a literal site and by a `format!` expansion"
                );
            }
        }

        let written: BTreeSet<String> = written_down_surface()
            .into_iter()
            .map(str::to_string)
            .collect();

        let added: Vec<&String> = live.difference(&written).collect();
        let removed: Vec<&String> = written.difference(&live).collect();
        assert!(
            added.is_empty() && removed.is_empty(),
            "the intrinsic surface moved without the written-down list moving with it \
             (plans/M9.md item AA).\n  in the compiler, not in the list: {added:?}\n  \
             in the list, not in the compiler: {removed:?}\n\
             Adding a name: add it to EXCEPTIONS in sema/intrinsics.rs with a one-line \
             justification saying why wrela source cannot express it. Removing one: delete its \
             line. Either way, same commit."
        );
    }

    #[test]
    fn format_key_guards_are_locked() {
        for (tmpl, expansion, guard) in FORMAT_KEY_SITES {
            let hits = BODIES_SRC.matches(guard).count();
            assert_eq!(
                hits, 1,
                "the guard bounding `{tmpl}` to {expansion:?} is no longer present exactly once \
                 in bodies.rs (found {hits} occurrences of `{guard}`). Re-resolve what the key's \
                 variable can range over and update FORMAT_KEY_SITES."
            );
        }
    }

    const ALLOWED_OFFSITE_KEY_SITES: &[(&str, &str)] = &[
        ("wrela-compiler/src/eval/legal.rs", "wake"),
        ("wrela-compiler/src/eval/legal.rs", "entropy"),
        ("wrela-compiler/src/sema/transport.rs", "Device.claim"),
        ("wrela-compiler/src/sema/transport.rs", "Device.take_irq"),
        ("wrela-compiler/src/sema/transport.rs", "Device.negotiate"),
        ("wrela-compiler/src/sema/transport.rs", "Device.start"),
        ("wrela-compiler/src/sema/transport.rs", "Device.reset"),
        (
            "wrela-compiler/src/sema/transport.rs",
            "Device.read_capacity_sectors",
        ),
        (
            "wrela-compiler/src/sema/transport.rs",
            "Device.map_partition",
        ),
        ("wrela-compiler/src/sema/transport.rs", "VirtQueue.reserve"),
        (
            "wrela-compiler/src/sema/transport.rs",
            "VirtQueue.prepare_block",
        ),
        ("wrela-compiler/src/sema/transport.rs", "VirtQueue.publish"),
        ("wrela-compiler/src/sema/transport.rs", "VirtQueue.reject"),
        ("wrela-compiler/src/sema/transport.rs", "VirtQueue.drain"),
        ("wrela-compiler/src/sema/transport.rs", "VirtQueue.claim"),
        ("wrela-compiler/src/sema/transport.rs", "VirtQueue.recover"),
        ("wrela-compiler/src/sema/transport.rs", "VirtQueue.reclaim"),
        (
            "wrela-compiler/src/sema/transport.rs",
            "VirtQueue.suppress_interrupts",
        ),
        (
            "wrela-compiler/src/sema/transport.rs",
            "VirtQueue.configure",
        ),
        ("wrela-compiler/src/sema/actor.rs", "Group.join_all"),
        ("wrela-compiler/src/sema/actor.rs", "Group.start"),
    ];

    #[test]
    fn bodies_rs_is_the_only_producer() {
        let files = crate_sources();
        assert!(
            files.len() > 20,
            "the workspace source walk found only {} files — this test would pass vacuously",
            files.len()
        );
        let mut saw_bodies = false;
        let mut offsite: Vec<(String, String)> = Vec::new();
        for (label, src) in &files {
            if label.ends_with("sema/bodies.rs") {
                saw_bodies = true;
                assert_eq!(
                    src, BODIES_SRC,
                    "the walked sema/bodies.rs differs from the `include_str!` embed"
                );
                continue;
            }
            for s in scan_key_sites(label, src) {
                let key = match s.form {
                    KeyForm::Literal(k) => k,
                    KeyForm::Template(t) => format!("format!({t})"),
                };
                offsite.push((format!("{}:{}", s.file, s.line), key));
            }
        }
        assert!(
            saw_bodies,
            "the walk never reached sema/bodies.rs, so it proves nothing"
        );
        let unexpected: Vec<&(String, String)> = offsite
            .iter()
            .filter(|(loc, key)| {
                !ALLOWED_OFFSITE_KEY_SITES
                    .iter()
                    .any(|(f, k)| loc.starts_with(f) && k == key)
            })
            .collect();
        assert!(
            unexpected.is_empty(),
            "an intrinsic key site outside sema/bodies.rs: {unexpected:?}. The census scans \
             bodies.rs only, so a node built anywhere else is surface it cannot see. Move the \
             site into bodies.rs, or add it to ALLOWED_OFFSITE_KEY_SITES with a reason."
        );
        assert_eq!(
            offsite.len(),
            ALLOWED_OFFSITE_KEY_SITES.len(),
            "an allowlisted off-site key site is gone ({offsite:?}); delete its \
             ALLOWED_OFFSITE_KEY_SITES entry"
        );
    }

    fn collect_rs(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
        for entry in std::fs::read_dir(dir).expect("read crate source dir") {
            let path = entry.expect("dir entry").path();
            if path.is_dir() {
                collect_rs(&path, out);
            } else if path.extension().is_some_and(|e| e == "rs") {
                out.push(path);
            }
        }
    }

    #[test]
    fn image_builder_surface_matches_the_restriction_predicate() {
        for k in IMAGE_BUILDER_SURFACE {
            assert!(
                crate::sema::typed::is_restricted_intrinsic(k),
                "`{k}` is 05 §9 builder surface but `is_restricted_intrinsic` does not \
                 restrict it — it would be callable outside `@image`"
            );
        }
        for (k, _) in EXCEPTIONS {
            assert!(
                !crate::sema::typed::is_restricted_intrinsic(k),
                "`{k}` is in the exception set but `is_restricted_intrinsic` treats it as \
                 05 §9 builder surface — move it to IMAGE_BUILDER_SURFACE or fix the predicate"
            );
        }
    }

    #[test]
    fn unproduced_consumer_keys_are_still_unproduced() {
        let written = written_down_surface();
        for (k, _) in UNPRODUCED_CONSUMER_KEYS {
            assert!(
                crate::sema::bodies::is_queue_op_deferred(k).is_some(),
                "`{k}` is recorded as a consumer-side key with no producer, but no consumer \
                 accepts it any more — delete its UNPRODUCED_CONSUMER_KEYS entry"
            );
            assert!(
                !written.contains(k),
                "`{k}` gained a producer; move it out of UNPRODUCED_CONSUMER_KEYS and into \
                 EXCEPTIONS with a justification"
            );
        }
    }

    #[test]
    fn every_exception_is_justified() {
        for (k, why) in EXCEPTIONS {
            assert!(
                why.len() > 30,
                "`{k}`'s justification is too short to be one: `{why}`"
            );
        }
        let _ = written_down_surface();
    }
}
