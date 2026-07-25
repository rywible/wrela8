//! **The intrinsic surface, written down** (plans/M9.md item AA,
//! decisions 56–59; ledger `library.intrinsics.closed-surface`).
//!
//! An *intrinsic* here means exactly one thing: a name `sema::bodies`
//! recognizes by spelling and turns into a `TypedExprKind::Intrinsic`
//! node carrying a `key`, instead of an ordinary `Call` to a function
//! that exists in source. The compiler, not a library, supplies its
//! meaning. Every such key is listed below.
//!
//! ## Why this file exists
//!
//! ROADMAP M10 warns that migrating item-by-item while adding whatever
//! intrinsic each item needs "designs the permanent surface by
//! accretion, in the one place this project can least afford it." That
//! pressure is in **M9**, not M10: every stdlib item can quietly reach
//! for a compiler-recognized name instead of writing wrela.
//! plans/M9.md shape decision 1 forbids it in prose; this file plus its
//! test is what enforces it. Adding a name to the compiler without
//! adding it here fails `cargo test`, and vice versa — so growing the
//! intrinsic surface becomes a deliberate, reviewable act with a
//! justification attached.
//!
//! The signature this exists to catch is already on the record:
//! 05-library.md §5 calls the `Duration` constructors "ordinary
//! phase-neutral functions", yet `ms` and `seconds` are compiler
//! intrinsic arms — and the other four (`ns`, `us`, `minutes`, `hours`)
//! do not exist at all, because nobody needed them for a golden.
//!
//! ## What this covers, and what it does not
//!
//! Covered: intrinsic **keys** — *meaning*. Not covered:
//! `sema::prelude`'s bare **names** — *resolution*. They are different
//! surfaces and they are not in bijection. `prelude::is_builtin` decides
//! whether `Foo` resolves at all with no import (`Actor`, `u32`,
//! `Bytes`, `Validated` — most of which are types, and several of which
//! resolve only so that a fail-closed rejection can name them). This
//! file decides whether a *call* gets compiler-supplied meaning. A name
//! can be in one and not the other in both directions: `Target` is a
//! prelude name with no intrinsic key; `Mmio.read` is an intrinsic key
//! that is not a prelude name (it is only ever reached through a typed
//! receiver). plans/M9.md item I owns shrinking `prelude.rs`; this file
//! owns holding the intrinsic surface still while it happens.
//!
//! ## How the list is checked (see the test module below)
//!
//! `sema/bodies.rs` is the sole producer of intrinsic nodes. It is read
//! as text (`include_str!`, so a missing file is a compile error rather
//! than a silently-empty scan) and every occurrence of the node's type
//! name followed by `{` — **any prefix, any path qualification** — is
//! located. Each occurrence's brace-balanced field region is then asked
//! whether it binds a `key`: a string literal or a `format!` is a census
//! entry, no `key:` at all is an ordinary pattern match and is ignored,
//! and a `key:` bound to anything else fails closed rather than being
//! quietly missed. The same scan runs over the whole workspace, where
//! the only permitted key sites are the enumerated ones in
//! `ALLOWED_OFFSITE_KEY_SITES`.
//!
//! Four sites build the key with `format!`; each one's expansion is
//! written down in `FORMAT_KEY_SITES` **together with the source line of
//! the guard that bounds its variable**, and that guard line is itself
//! asserted to be present verbatim — so widening a guard (adding a fifth
//! `InterruptCell` method to its match arm, say) fails the test too,
//! which hand-resolving the set alone would not catch.

/// **05-library.md §9's image-builder surface.** This is the part of the
/// intrinsic surface that is *not* an exception and is not expected to
/// shrink: it is comptime-only, emits no code, and its effects land on
/// the compiler's own image graph. Making it ordinary wrela would mean
/// exposing the image IR as a language type (plans/M9.md item E states
/// this as settled). §9 names them one by one; this list is that
/// paragraph, in key form.
///
/// It is also exactly `sema::typed::is_restricted_intrinsic`'s set — the
/// predicate `eval::legal` uses to reject a builder call anywhere but the
/// one reachable `@image` fn — and the test below asserts that equality
/// in both directions.
pub const IMAGE_BUILDER_SURFACE: &[&str] = &[
    // `Image(name, target)` — the one builder intrinsic called by bare
    // name; produces the resource builder.
    "Image",
    // `img.device[D](transport=..., required_features=...)`
    "Image.device",
    // `img.driver(A[...], device=d, mailbox=n?, ...)`
    "Image.driver",
    // `img.actor(A, mailbox=n, ...)`
    "Image.actor",
    // `img.pool[T](name=P, slots=N, max_payload=B)`
    "Image.pool",
    // `img.dma_pool[T](name=P, device=d, count=N)`
    "Image.dma_pool",
    // `img.supervise(children=..., strategy=..., intensity=...)`
    "Image.supervise",
    // `img.check_layout(f)` — registers a `@layout_assert` (04 §2).
    "Image.check_layout",
    // `img.seal()` — consumes the builder.
    "Image.seal",
    // `decl.handle()` — installs an `Actor[A]` identity as another
    // actor's `init` dependency.
    "ImageDecl.handle",
];

/// **The exception set**: every intrinsic key that is *not* 05 §9's
/// builder surface, each with the one line that justifies it being
/// compiler-supplied rather than wrela source.
///
/// A new entry here is the thing this file exists to make visible. The
/// bar plans/M9.md sets: an intrinsic is justified when wrela source
/// *cannot* express the thing (no expression form, a build-time graph
/// edge, a sealed wrapper, a recorded effect) — not when writing it in
/// wrela would merely be inconvenient.
///
/// Sorted by the surface each name belongs to, then alphabetically
/// within it.
pub const EXCEPTIONS: &[(&str, &str)] = &[
    // --- 05 §5: time -------------------------------------------------
    //
    // The two entries below are the accretion case study plans/M9.md
    // item AA was written around, and item E deletes both. When it does,
    // this comment block and its two lines go with them: that is the
    // "one-line change with an obvious diff" AA owes E.
    (
        "ms",
        "ACCRETION — 05 §5 calls the `Duration` constructors ordinary phase-neutral functions; \
         plans/M9.md item E deletes this arm for `stdlib/core/time.wr`.",
    ),
    (
        "seconds",
        "ACCRETION — same as `ms`: 05 §5 says ordinary function, the compiler says intrinsic. \
         plans/M9.md item E deletes this arm.",
    ),
    (
        "now",
        "05 §5: a sealed effect. It is forbidden in comptime and ISR context and is \
         recorded/replayed through the recorder's `ClockRead` choice entry — wrela source can \
         express neither the prohibition nor the choice-point binding.",
    ),
    // --- 05 §9 adjacent: builder vocabulary with no parameter list ----
    (
        "RestartIntensity",
        "02 §11 / 05 §9: `img.supervise(intensity=...)`'s argument. Comptime-only builder \
         vocabulary read straight off the image graph; it shares the intrinsic node purely \
         because it has no declared parameter list to align labeled arguments against.",
    ),
    // --- 03 §2: typed MMIO -------------------------------------------
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
    // --- 03 §8 / 05 §6: marked values --------------------------------
    (
        "Untrusted.checked_le",
        "03 §8: the sealed wrapper's one narrowing. The seal is the point — wrela source may \
         neither construct nor unwrap an `Untrusted[T]`, so its narrowing cannot be wrela \
         either without breaking the seal.",
    ),
    // --- 03 §1 / §9: device bring-up ---------------------------------
    //
    // Each of these consumes or advances a capability along 03 §9's
    // bring-up chain: the transition is a build-time-proven fact about
    // the image graph *and* a device-transport effect. Neither half has
    // a wrela spelling.
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
    // --- 03 §4 / §5: queues and receipts ------------------------------
    //
    // Every one of these is a reservation/publication step whose safety
    // is a *build-time* proof (`sema::reserve_proof`) over a ring the
    // compiler laid out. A wrela function could perform the stores; it
    // could not carry the proof.
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
         because `reserve_proven` proved them.",
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
        "VirtQueue.reserve_proven",
        "03 §4 / 05 §10's `*_proven` naming rule: the statically-proved reservation. \
         `sema::reserve_proof` is the proof; the intrinsic is where it attaches.",
    ),
    (
        "VirtQueue.suppress_interrupts",
        "03 §4: flips the ring's no-interrupt flag with the ordering the device contract \
         requires.",
    ),
    // --- 03 §6: interrupts and bottom halves --------------------------
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
    // --- 02 §9 / 05 §4: scoped concurrency ----------------------------
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

/// The four `sema/bodies.rs` sites that build a key with `format!`, and
/// therefore the four reasons the surface cannot be counted by grepping
/// for string literals.
///
/// Each entry is `(template, the concrete set the variable ranges over,
/// the source line that bounds it)`. **All four are statically knowable**
/// — each variable is constrained by a guard a few lines above or around
/// its construction site, and that guard's exact source text is the third
/// field so the test can assert it has not been widened.
pub const FORMAT_KEY_SITES: &[(&str, &[&str], &str)] = &[
    (
        // `check_image_bracket_intrinsic`, dispatched from
        // `check_call_index`.
        "Image.{mname}",
        &["Image.device", "Image.pool", "Image.dma_pool"],
        r#"if mname == "device" || mname == "pool" || mname == "dma_pool" {"#,
    ),
    (
        // `check_mmio_access` — anything else is a type error before the
        // node is built.
        "Mmio.{op}",
        &["Mmio.read", "Mmio.write"],
        r#"if !matches!(op, "read" | "write") {"#,
    ),
    (
        // `check_interrupt_cell_call` — the construction site is *inside*
        // this match arm; `load_acquire` has its own literal site and
        // every other method falls to the `other =>` rejection.
        "InterruptCell.{method}",
        &[
            "InterruptCell.store_release",
            "InterruptCell.swap_acquire",
            "InterruptCell.fetch_or_release",
        ],
        r#""store_release" | "swap_acquire" | "fetch_or_release" => {"#,
    ),
    (
        // `check_image_method_intrinsic` — the construction site is
        // inside this match arm; `supervise`/`check_layout`/`seal` have
        // their own literal sites and anything else is "no builder
        // method".
        "Image.{name}",
        &["Image.driver", "Image.actor"],
        r#""driver" | "actor" => {"#,
    ),
];

/// Keys a **consumer-side** predicate accepts that no construction site
/// in `sema/bodies.rs` ever produces — dead boundary, recorded rather
/// than removed (AA measures and locks; it does not remove).
///
/// If a producer for one of these ever appears, the closure test fails
/// with the name in the "in the compiler, not in the list" column, which
/// is the correct outcome: adding it is then a deliberate act.
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

/// The whole written-down surface: 05 §9's builder names plus the
/// exception set, sorted, deduplicated by assertion rather than silently.
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

    /// `sema/bodies.rs`, embedded at compile time. A missing or renamed
    /// file is a build failure, never an empty scan that passes.
    const BODIES_SRC: &str = include_str!("bodies.rs");

    /// The marker the census keys on (decision 60), assembled at runtime
    /// from two halves so that *this file's own code* never contains it
    /// verbatim and therefore needs no exclusion from the walk.
    ///
    /// Deliberately the **weak** form: no `kind:` prefix and no path
    /// qualification, so `crate::sema::typed::TypedExprKind::Intrinsic {`
    /// and a bare `TypedExprKind::Intrinsic { key: ... }` both match. The
    /// first version of this test keyed on the whole line
    /// `kind: TypedExprKind::Intrinsic {` and was evaded three separate
    /// ways by spelling the construction differently; see decision 60.
    fn marker() -> String {
        format!("{}{}", "TypedExprKind::Intrinsic", " {")
    }

    /// One occurrence of the marker that *produces* a key, with the key
    /// as written.
    #[derive(Debug)]
    struct KeySite {
        file: String,
        line: usize,
        form: KeyForm,
    }

    #[derive(Debug, PartialEq)]
    enum KeyForm {
        /// `key: "Image.seal".to_string()` / `.into()`.
        Literal(String),
        /// `key: format!("Mmio.{op}")`.
        Template(String),
    }

    /// Every marker occurrence in `src` that assigns a `key`, classified.
    ///
    /// The classification is structural rather than positional, which is
    /// what makes it hard to evade: from each marker's `{`, the
    /// brace-balanced field region is collected (across lines), and the
    /// region is then asked whether it binds `key:` to a value.
    ///
    /// - No `key:` in the region → a **pattern** (`{ key, .. }`,
    ///   `{ receiver, args, .. }`, `{ .. }`). Ignored, and deliberately
    ///   *not* counted: pattern matches on this node are added and
    ///   removed by ordinary downstream work, and a census that churned
    ///   on those would teach everyone to bump a number without reading.
    /// - `key:` bound to a string literal or a `format!` → a **key site**.
    /// - `key:` bound to anything else → **panic**. A computed key is
    ///   surface this census cannot resolve, so it fails closed and says
    ///   so rather than silently missing a name.
    ///
    /// Comment lines are skipped (they do not compile); this is why the
    /// worked example inside `bodies.rs`'s own `check_mmio_access` prose
    /// is not a site.
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
                continue; // a pattern match, not a construction
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

    /// The brace-balanced text between the `{` at `lines[start][open]`
    /// and its matching `}`. Fails loudly rather than returning a
    /// truncated region.
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
                            continue; // skip the opening brace itself
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

    /// The value bound to a `key:` field in `region`, if any. Matches
    /// `key` only at a token boundary, so no other field name can be
    /// mistaken for it.
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

    /// Every `.rs` file in the workspace, with a repo-relative label.
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

    /// **The ratchet** (plans/M9.md item AA, ledger
    /// `library.intrinsics.closed-surface`): the set of intrinsic keys
    /// `sema::bodies` can construct equals the set written down in this
    /// file. Fails when a name is added to the compiler without being
    /// added here, and when a name is removed from the compiler without
    /// being removed here.
    #[test]
    fn intrinsic_surface_equals_the_written_down_list() {
        let sites = scan_key_sites("sema/bodies.rs", BODIES_SRC);
        assert!(
            sites.len() >= 30,
            "the census scan found only {} key sites in bodies.rs — the marker no longer \
             matches the source, so this test is proving nothing",
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

        // Every `format!` template found must be one we have resolved.
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

    /// The `format!` expansions above are hand-resolved, so the closure
    /// test alone would not notice a *widened* guard (a fifth
    /// `InterruptCell` method added to its match arm produces a fifth
    /// key from an unchanged construction site). Locking the guard's
    /// source text closes that hole.
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

    /// The only intrinsic key sites outside `sema/bodies.rs`: `(file,
    /// key)`, enumerated rather than counted, so an *added* site fails
    /// even in a file that already has one.
    const ALLOWED_OFFSITE_KEY_SITES: &[(&str, &str)] = &[
        // A `#[cfg(test)]` fixture asserting `wake` is ISR-legal. A test
        // fixture is not surface.
        ("wrela-compiler/src/eval/legal.rs", "wake"),
    ];

    /// `sema/bodies.rs` is the sole producer of intrinsic nodes, which is
    /// what makes scanning that one file a complete census. Walks the
    /// whole workspace so a key site added anywhere else fails here.
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
                // The walked file and the compile-time embed must be the
                // same text, or the census is reading a different tree
                // from the one this test polices.
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

    /// 05 §9's surface and `sema::typed::is_restricted_intrinsic` are the
    /// same set — the predicate is what confines a builder call to the one
    /// reachable `@image` fn, so a §9 name missing from it would be a
    /// builder intrinsic callable from ordinary code.
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

    /// The recorded dead consumer boundary stays dead: `is_queue_op_deferred`
    /// still names both keys, and neither is produced (which the closure
    /// test above independently enforces by their absence from the list).
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

    /// Every exception carries a justification, and no name is listed twice.
    #[test]
    fn every_exception_is_justified() {
        for (k, why) in EXCEPTIONS {
            assert!(
                why.len() > 30,
                "`{k}`'s justification is too short to be one: `{why}`"
            );
        }
        let _ = written_down_surface(); // asserts no duplicates
    }
}
