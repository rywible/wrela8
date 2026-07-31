//! The image report (plans/M4.md item D, decision 8): renders a sealed,
//! checked `eval::image::ImageGraph` plus its own build inputs into
//! `ImageReport`, one stable text artifact in the identical M1 dump
//! style every earlier stage already uses — `Kind key=value` lines,
//! two-space indent, a versioned header line (`ImageReport v0`), fixed
//! section order, facts only (a section with nothing to say is absent
//! entirely; a later milestone appends sections without reshuffling
//! these). This is the milestone's flagship artifact (04-compiler.md §7)
//! — the difference from item B/C's own `--stage=image` raw graph dump
//! (`eval::image::dump`) is exactly what this module adds: build
//! identity (compiler revision, target, the four quota constants, one
//! content digest per file in the build closure), declared mailboxes and
//! logical actor edges pulled out of the raw graph's own generic
//! argument lists into their own named facts, and the decision-10
//! report-boundary enforcement (`render`'s own first act, below).
//! Reusing rather than reshaping: every `Arg`/`Name`/`Target` line below
//! is rendered with `eval::image`'s own `render_value`/`push_line`
//! (bumped to `pub(crate)` for exactly this reuse, no logic changed) so
//! the report's own facts are byte-identical in style to the raw dump's.
//!
//! **Section order** (decision 8, fixed, never reshuffled — this is the
//! whole "review surface" the golden diff protects):
//!
//! 1. Build identity: `Compiler`, `Machine revision` (plans/M5.md decision
//!    6: `wrela_machine::MACHINE_REVISION_STR`, joining build identity —
//!    added at item D, unconditional, every report), `Target` (the
//!    build-affecting fact, 04-compiler.md §7/§8), `Quota` × 4
//!    (`eval::quota`'s own constants, cited by 02-language.md §2.1 as
//!    "language constants recorded in build identity"), `Input` × one per
//!    file in the closure.
//! 2. The image: `Name`, `Target` (the image's own identity from
//!    `Image(...)`, 02-language.md §12.1 — deliberately the same `Kind`
//!    name as build identity's own `Target` line above; both are real,
//!    independently-required facts, so both appear, decision-8 sub-note
//!    below explains why this is not a placeholder-duplicate).
//! 3. `Device` blocks (args verbatim — 05-library.md §9 never checks a
//!    device's own args against anything, so nothing is pulled out of
//!    them).
//! 4. `Driver`/`Actor` blocks: every non-handle, non-`mailbox`, non-`core`
//!    argument as an `Arg` line (05 §9's own "arguments ... match `A.init` ...
//!    after ... substituted" — decision 7's "reserved args are wiring
//!    metadata" already strips `device`/`core`/`mailbox` from ordinary
//!    matching, and this renderer goes one step further: it pulls every
//!    *handle* argument, reserved or not, out into its own `Edge` fact
//!    below instead of an `Arg` line, pulls `mailbox` out into its
//!    own `Mailbox` fact — "declared mailboxes (recorded as-declared, no
//!    derivation)" per decision 8 — and pulls `core=` into the Placement
//!    section (8a), never an `Arg`).
//! 5. `Edge from=<decl> to=<decl>` — every logical actor edge (decision-8
//!    sub-note on direction, below), one line each, in the same
//!    construction order the edges were discovered in (devices, then
//!    drivers, then actors; each declaration's own argument order).
//! 6. `Pool` blocks (args verbatim, same reasoning as `Device`).
//! 7. `OnFailure` blocks: `policy` pulled onto the header line itself;
//!    any other labeled argument falls back to an ordinary `Arg` line.
//! 8a. **Placement** (plans/M8.md item B, 04-compiler.md §3): one
//!    `Placement id=... type=... core=... source=... work=... ...` line
//!    per driver/actor, in declaration order. The table is computed by
//!    `placement::place` (needs a `LayoutCtx` this module does not own)
//!    and passed into `render`; a section with nothing to say is absent.
//!    `core=` is wiring metadata (like `mailbox`), never an `Arg` line.
//! 8b. **The exact-bytes section** (plans/M7.md item B, 03-hardware.md §3:
//!    "For every `@layout` type the compiler reports exact size, offsets,
//!    padding, and endianness"): one `Layout name=... kind=... endian=...
//!    size=... padding=...` block per `@layout` type, each carrying its own
//!    `Field`/`Padding` lines. Rendered by `render_exact_bytes_section`
//!    below rather than by `render` itself, for the same reason the M5
//!    memory-map section is (`layout::render_layout_section`): the facts
//!    come from the build closure's own ast, which a sealed `ImageGraph`
//!    does not carry. `build_report` (`bin/wrela.rs`) calls it between the
//!    two — the declaration facts before the emission facts.
//!
//!    **Population**: every `@layout` type declared in the build closure.
//!    03 §3 says "the image reaches", and today that cannot be narrowed
//!    further: no capability type exists yet (plans/M7.md item A mints
//!    them), so nothing in an image can *hold* a layout, and the honest
//!    over-approximation is "declared in the closure" rather than an
//!    invented reachability rule. Narrowing it belongs to the item that
//!    makes `Mmio[L]` bind a layout to a device.
//! 8c. **Cost summary** (plans/M18.md item R, 04 §6): after a successful
//!    layout, `append_cost_summary` adds version/digest/total/ghz plus
//!    three Owner lines and (when placement is non-empty) Core + Shared
//!    lines. Scored from the same `CodegenProgram` layout built; omitted
//!    when layout soft-fails (`Ok(None)`). Not Terms / Placeable.
//! 8d. **Convention** (plans/codegen-pareto.md item F): one line per
//!    function whose calling convention is not the default one — its
//!    frame size, its resident temps and their registers, the registers
//!    a caller must assume it destroys, and how many registers its own
//!    pool reached. Rendered by `append_convention_section`; absent
//!    entirely under `dev`, where every function has the same
//!    convention.
//! 9. Registered layout asserts: recorded in the raw `--stage=image`
//!    graph dump, then **run** after layout against a real stdlib
//!    `ImageReport` value (`eval::layout_assert`, plans/M9.md item H).
//!    A failing assert fails the build (never a second layout pass,
//!    04-compiler.md §8). Successful asserts leave the report text and
//!    emitted image untouched — they are not themselves report sections.
//!
//! **Decision-8 sub-notes** (recorded at item D execution, 2026-07-23 —
//! every choice the plan left open):
//!
//! - **Input paths are package-root-relative by construction, never a
//!   real filesystem path**: this module never touches a disk path at
//!   all. The caller (`bin/wrela.rs::run_report_stage`, `xtask`'s own
//!   determinism oracle) supplies one `BuildInput` per file, and the
//!   `path` field it carries is *derived from the module's own dotted
//!   closure address* (`address_to_relative_path`, below) — exactly the
//!   same address `loader::module_file_path` itself builds a real file
//!   path from (dotted segments joined by `/`, `.wr` appended,
//!   `core`-prefixed for a toolchain-stdlib file) — never the actual path
//!   used to invoke the compiler, which can be absolute or working-
//!   directory-relative and would silently bake a specific checkout's own
//!   location into a pinned golden the moment it differs from another's
//!   (the exact failure mode `loader.rs`'s own module doc already
//!   disclosed for the missing-module diagnostic). Reconstructing the
//!   path from the address is the dumbest deterministic rule available:
//!   it needs no new bookkeeping (the address is already the closure's
//!   own key everywhere) and is trivially portable.
//! - **Build identity's own `Target` line duplicates the image's own
//!   `Target` line** (both literally read `graph.target`): intentional,
//!   not a placeholder. 04-compiler.md §7/§8 requires the target to be a
//!   recorded *build-affecting input* (identity), while 02-language.md
//!   §12.1 separately requires it as part of the image's own declared
//!   identity (name + target come only from `Image(...)`) — two real
//!   normative facts that happen to share one value, so both lines stay.
//! - **Edge direction**: `from` is the declaration whose own argument
//!   holds the handle (the "wired" consumer — the driver/actor whose
//!   `init` this argument satisfies), `to` is the declaration the handle
//!   names. This reads the same direction a later real call edge will
//!   take once actor bodies are typed (M6): the holder is the one that
//!   can call through the handle it was given, so `Edge from=actor#1
//!   to=driver#0` means "actor#1 can reach driver#0," matching
//!   04-compiler.md §7's own "every logical actor edge" wording — this is
//!   this milestone's whole realization of that fact, recorded honestly
//!   as a wiring-derived approximation, not a real call-graph edge (no
//!   actor body is typed yet, M6 non-goal).
//! - **A device's own args are never scanned for edges beyond the
//!   generic rule below**: 05 §9 never wires a device to another
//!   declaration (a device is always a leaf — nothing takes a device's
//!   own handle as an argument), so this is moot in practice, but the
//!   renderer applies its edge rule uniformly across `Device`/`Driver`/
//!   `Actor`/`Pool` args rather than special-casing which kinds can carry
//!   one — "dumb and correct," not "correct because special-cased."
//!
//! **Decision 9 (report determinism)**: this module does no I/O, no
//! caching, no wall clock, and no random iteration order — every
//! collection it walks is already a `BTreeMap`/program-order `Vec`
//! (`ImageGraph`'s own construction), so `render` is a pure function of
//! its arguments; the digest, below, is a pure function of bytes.
//! `xtask`'s own determinism oracle (item D's own deliverable 6) exploits
//! exactly this: calling `render` twice over freshly-loaded input can
//! never disagree with itself.

use std::collections::BTreeMap;

use wrela_machine::report as machine_report;

use crate::codegen::CodegenProgram;
use crate::cost::dump as cost_dump;
use crate::cost::{self, CostReport};
use crate::eval::image::{self, ImageDeclRef, ImageGraph, TypedProgramEnums};
use crate::eval::quota;
use crate::eval::value::Value;
use crate::placement::{self, PlacementTable};
use crate::sema::types;

/// Structured `--stage=report` facts before text render. Overlapping
/// identity lines (`Machine revision`, `Input`) share spellings with
/// [`machine_report::ParsedReport`] via `machine_report::line_*` helpers;
/// graph / placement / exact-bytes sections remain compiler-owned.
#[derive(Debug, Clone)]
pub struct ImageReportDoc<'a> {
    pub inputs: &'a [BuildInput],
    pub enums: &'a BTreeMap<String, Vec<String>>,
    pub graph: &'a ImageGraph,
    pub placement: &'a PlacementTable,
}

/// This crate's own static version string (`wrela-compiler`'s own
/// `Cargo.toml` — `bin/wrela.rs`'s `version` subcommand reads the
/// identical constant, since the `wrela` binary is itself a target of
/// this same package). Never a git SHA, never a build timestamp: decision
/// 8's own "byte-stable across commits of this repo and across machines"
/// requirement rules both out categorically — this is the one fact that
/// satisfies "build identity" without violating it.
const COMPILER_VERSION: &str = env!("CARGO_PKG_VERSION");

/// One file in the build closure's own content digest, keyed by the
/// package-root-relative path `address_to_relative_path` derives from
/// its module address — never a real filesystem path (this module's own
/// doc comment, decision-8 sub-note). The caller (`bin/wrela.rs`,
/// `xtask`) reads the real file's bytes and hashes them with
/// `sha256_hex`; this module never touches a disk path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BuildInput {
    pub path: String,
    pub digest: String,
}

/// Converts one module's own dotted closure address (`programs`'s own
/// `BTreeMap<String, TypedProgram>` key everywhere else in this crate —
/// a plain module path like `"image"`/`"actor"`, or a toolchain-stdlib
/// one like `"core.bytes"`) into the package-root-relative path this
/// report records it under: dots become slashes, `.wr` is appended. This
/// is exactly the reverse of `loader::module_file_path`'s own forward
/// construction (root + dotted segments -> file path), so the result is
/// package-root-relative *by construction* — never the real filesystem
/// path used to invoke the compiler (this module's own doc comment,
/// decision-8 sub-note).
pub fn address_to_relative_path(address: &str) -> String {
    format!("{}.wr", address.replace('.', "/"))
}

/// One already-rendered `Arg`/`Mailbox`/`Edge` fact this module's own
/// per-declaration scan produces, before it is written to the output
/// buffer — kept as a small enum rather than writing directly, so the
/// caller (`render_decl_facts`) can hand `Edge` facts back to be emitted
/// later, in their own section (decision 8's own fixed order places
/// `Edge` after every `Device`/`Driver`/`Actor` block, not interleaved).
enum DeclFact {
    Arg { label: String, rendered: String },
    Mailbox { rendered: String },
    Edge { to: ImageDeclRef },
}

/// Scans one declaration's own recorded arguments and sorts each into
/// its own report fact (this module's own doc comment, section 4): a
/// handle-valued argument (`Value::ImageDecl`, with or without its own
/// `.handle()` — indistinguishable once evaluated, exactly like
/// `eval::image_checks`'s own construction-DAG note) becomes an `Edge`;
/// an argument labeled `mailbox` becomes a `Mailbox` fact; `core=` is
/// omitted here (it appears only in the Placement section, 8a); everything
/// else is an ordinary `Arg` fact, rendered with `eval::image`'s own
/// `render_value` verbatim.
fn decl_facts(program: &TypedProgramEnums, args: &[image::DeclArg]) -> Vec<DeclFact> {
    let mut out = Vec::with_capacity(args.len());
    for a in args {
        if a.label == "core" {
            continue;
        }
        if let Value::ImageDecl(r) = &a.value {
            out.push(DeclFact::Edge { to: r.clone() });
            continue;
        }
        let rendered = image::render_value(program, &a.ty, &a.value);
        if a.label == "mailbox" {
            out.push(DeclFact::Mailbox { rendered });
        } else {
            out.push(DeclFact::Arg {
                label: a.label.clone(),
                rendered,
            });
        }
    }
    out
}

/// Writes one declaration's own `Arg`/`Mailbox` facts (depth 2, under its
/// own `Device`/`Driver`/`Actor`/`Pool` header line already written by
/// the caller) and appends every `Edge` fact it found to `edges` — keyed
/// by `owner` (this declaration's own `ImageDeclRef`) so the dedicated
/// `Edge` section below can render `from=<owner> to=<target>` once every
/// `Device`/`Driver`/`Actor` block has been walked.
fn render_decl_block(
    program: &TypedProgramEnums,
    owner: &ImageDeclRef,
    args: &[image::DeclArg],
    out: &mut String,
    edges: &mut Vec<(ImageDeclRef, ImageDeclRef)>,
) {
    for fact in decl_facts(program, args) {
        match fact {
            DeclFact::Arg { label, rendered } => {
                image::push_line(out, 2, &format!("Arg label={label} value={rendered}"));
            }
            DeclFact::Mailbox { rendered } => {
                image::push_line(out, 2, &format!("Mailbox value={rendered}"));
            }
            DeclFact::Edge { to } => edges.push((owner.clone(), to)),
        }
    }
}

/// One pool declaration's own recorded arguments, at depth 2 under its
/// `Pool`/`DmaPool` header. A decl-reference argument (only
/// `img.dma_pool`'s own `device=`) renders as an ordinary `Arg` line
/// naming the declaration — see section 6's own note for why it is not an
/// `Edge`.
fn render_pool_args(program: &TypedProgramEnums, args: &[image::DeclArg], out: &mut String) {
    for a in args {
        let rendered = match &a.value {
            Value::ImageDecl(r) => r.render(),
            v => image::render_value(program, &a.ty, v),
        };
        image::push_line(out, 2, &format!("Arg label={} value={rendered}", a.label));
    }
}

/// Builds the structured report value, then renders it. `inputs` must
/// already carry one entry per file in the build closure — this function
/// does no I/O and trusts its caller (`bin/wrela.rs::run_report_stage`,
/// `xtask`'s own determinism oracle) to have read and hashed every real
/// file; it renders `inputs` in the order given (the caller supplies them
/// in `BTreeMap`-by-address order, so the report's own `Input` lines come
/// out deterministically path-sorted with no further sorting needed
/// here). A sealed, checked graph has nothing left for `render` itself to
/// reject — registered `@layout_assert` fns are run *after* layout by
/// `eval::layout_assert` (plans/M9.md item H), never refused here with a
/// fake or partial reflection value.
pub fn render(
    inputs: &[BuildInput],
    enums: &BTreeMap<String, Vec<String>>,
    graph: &ImageGraph,
    placement: &PlacementTable,
) -> Result<String, String> {
    Ok(render_doc(&ImageReportDoc {
        inputs,
        enums,
        graph,
        placement,
    }))
}

/// Renders a structured [`ImageReportDoc`] (plans/M4.md item D). Identity
/// lines that overlap the VMM schema go through `wrela_machine::report`
/// Kind spellings so emitter and parser share one source.
pub fn render_doc(doc: &ImageReportDoc<'_>) -> String {
    let program = TypedProgramEnums { enums: doc.enums };
    let mut out = String::new();
    out.push_str("ImageReport v0\n");

    // --- 1. build identity -------------------------------------------------
    image::push_line(&mut out, 1, &format!("Compiler version={COMPILER_VERSION}"));
    image::push_line(
        &mut out,
        1,
        &machine_report::line_machine_revision(wrela_machine::MACHINE_REVISION_STR),
    );
    if let Some(target) = &doc.graph.target {
        image::push_line(
            &mut out,
            1,
            &format!(
                "Target value={}",
                image::render_value(&program, &target.ty, &target.value)
            ),
        );
    }
    image::push_line(
        &mut out,
        1,
        &format!("Quota max_steps={}", quota::MAX_STEPS),
    );
    image::push_line(
        &mut out,
        1,
        &format!("Quota max_memory={}", quota::MAX_MEMORY),
    );
    image::push_line(
        &mut out,
        1,
        &format!("Quota max_call_depth={}", quota::MAX_CALL_DEPTH),
    );
    image::push_line(
        &mut out,
        1,
        &format!("Quota max_exhaustive_cases={}", quota::MAX_EXHAUSTIVE_CASES),
    );
    for inp in doc.inputs {
        image::push_line(
            &mut out,
            1,
            &machine_report::line_input(&inp.path, &inp.digest),
        );
    }

    // --- 2. the image --------------------------------------------------------
    if let Some(name) = &doc.graph.name {
        image::push_line(
            &mut out,
            1,
            &format!(
                "Name value={}",
                image::render_value(&program, &name.ty, &name.value)
            ),
        );
    }
    if let Some(target) = &doc.graph.target {
        image::push_line(
            &mut out,
            1,
            &format!(
                "Target value={}",
                image::render_value(&program, &target.ty, &target.value)
            ),
        );
    }

    // --- 3/4. devices, drivers, actors (+ mailboxes) — and the edges each
    // one's own args discover along the way, held until section 5. ---------
    let mut edges: Vec<(ImageDeclRef, ImageDeclRef)> = Vec::new();

    for (i, d) in doc.graph.devices.iter().enumerate() {
        image::push_line(
            &mut out,
            1,
            &format!(
                "Device index={i} type={}",
                types::render_type(&d.device_type)
            ),
        );
        render_decl_block(
            &program,
            &ImageDeclRef::Device(i),
            &d.args,
            &mut out,
            &mut edges,
        );
    }
    for (i, d) in doc.graph.drivers.iter().enumerate() {
        image::push_line(
            &mut out,
            1,
            &format!(
                "Driver index={i} type={}",
                types::render_type(&d.actor_type)
            ),
        );
        render_decl_block(
            &program,
            &ImageDeclRef::Driver(i),
            &d.args,
            &mut out,
            &mut edges,
        );
    }
    for (i, d) in doc.graph.actors.iter().enumerate() {
        image::push_line(
            &mut out,
            1,
            &format!("Actor index={i} type={}", types::render_type(&d.actor_type)),
        );
        render_decl_block(
            &program,
            &ImageDeclRef::Actor(i),
            &d.args,
            &mut out,
            &mut edges,
        );
    }

    // --- 5. every logical actor edge -----------------------------------------
    for (from, to) in &edges {
        image::push_line(
            &mut out,
            1,
            &format!("Edge from={} to={}", from.render(), to.render()),
        );
    }

    // --- 6. pools (capacities are just their own recorded args) -------------
    //
    // Both forms, in one block, plain-then-DMA — `Pool`/`DmaPool` headers
    // are distinct `Kind` words, exactly as `eval::image::dump`'s own raw
    // graph dump already spells them, so the two are never confusable and
    // neither needs the other's fields.
    //
    // Pool args never carry a decl-reference *except* `img.dma_pool`'s own
    // `device=` (05 §9 wires no other pool argument to another
    // declaration). `render_decl_block`'s edge-detecting rule still runs
    // uniformly here rather than special-casing pools out of it (this
    // module's own doc comment: "dumb and correct," not "correct because
    // special-cased") — so a DMA pool's `device=` becomes an `Edge` fact
    // discovered too late for section 5's already-written output. That is
    // why it is rendered here, explicitly, as the pool's own
    // `device=<decl>` field: the reachability fact 03-hardware.md §3
    // requires is a property *of the pool*, not a wiring edge between two
    // actors, and section 5's own `Edge` lines are documented as "every
    // logical actor edge". The emission-side `Pool ... device=device#N`
    // line (`layout::render_layout_section`) carries the *resolved* device
    // — this one carries the declaration exactly as source wrote it,
    // driver spelling included.
    for (name, d) in &doc.graph.pools {
        image::push_line(
            &mut out,
            1,
            &format!(
                "Pool name={name} type={}",
                types::render_type(&d.payload_type)
            ),
        );
        render_pool_args(&program, &d.args, &mut out);
    }
    for (name, d) in &doc.graph.dma_pools {
        image::push_line(
            &mut out,
            1,
            &format!(
                "DmaPool name={name} type={}",
                types::render_type(&d.payload_type)
            ),
        );
        render_pool_args(&program, &d.args, &mut out);
    }

    // --- 7. failure policy -----------------------------------------------------
    for (i, s) in doc.graph.on_failures.iter().enumerate() {
        let policy = s
            .args
            .iter()
            .find(|a| a.label == "policy")
            .map(|a| image::render_value(&program, &a.ty, &a.value));
        let mut header = format!("OnFailure index={i}");
        if let Some(v) = &policy {
            header.push_str(&format!(" policy={v}"));
        }
        image::push_line(&mut out, 1, &header);
        for a in &s.args {
            if a.label == "policy" {
                continue;
            }
            image::push_line(
                &mut out,
                2,
                &format!(
                    "Arg label={} value={}",
                    a.label,
                    image::render_value(&program, &a.ty, &a.value)
                ),
            );
        }
    }

    // --- 8a. placement (04 §3 / plans/M8.md item B) -----------------------
    placement::render_placement_section(&mut out, doc.placement);

    // --- 8b/9. the exact-bytes section (appended by the caller) and
    // registered layout asserts (never reached): see module doc ------------

    out
}

/// Appends the exact-bytes section (module doc, section 8b) to an
/// already-rendered report. `layouts` is every `@layout` type in the build
/// closure, already checked and laid out by `sema::types::check_layouts`,
/// in the caller's own deterministic order (a `BTreeMap` walk keyed by
/// dotted module address, then declaration order inside each module).
/// Nothing to say means nothing printed — the facts-only rule this whole
/// module follows: a closure with no `@layout` type leaves the report
/// byte-identical to what it was before this section existed, which is
/// why no existing golden moved when it landed.
///
/// Determinism (`image.report.deterministic`): a pure function of its
/// arguments, exactly like `render` — no I/O, no clock, no hashing, no map
/// iteration of its own. Its *input* is deterministic for the same reason:
/// `check_layouts` is a pure function of one specialized ast module, and
/// the caller walks the closure in `BTreeMap` order.
///
/// `Err` for a layout whose sizing is still deferred (plans/M10.md item A2b
/// requirement 4): the report is the machine's own configuration surface, so
/// a `@layout` type reaching it with no computed size is a fail-closed
/// rejection, not a `size=0` line. The caller completes the table
/// (`types::complete_layouts`) before calling this.
pub fn render_exact_bytes_section(
    out: &mut String,
    layouts: &[types::LayoutType],
) -> Result<(), crate::sema::SemaError> {
    for l in layouts {
        types::push_layout_lines(out, 1, l)?;
    }
    Ok(())
}

/// Append the short Cost summary (plans/M18.md item R / 04 §6): version,
/// table digest, program total, ghz, multi-W rows, per-owner schedule
/// totals, and (when placement is non-empty) Core + Shared lines. No
/// Fn/Term/Placeable lines (those live on `--stage=cost`).
///
/// Loads `bench/a76-pi5.toml` via [`cost::load_default`] and
/// `workloads.toml` (+ sibling `lane1-freq.txt` when `source` is set);
/// missing/malformed table or workloads → `Err` (fail closed). Caller
/// scores the same `CodegenProgram` layout already produced (no second
/// lower).
pub fn append_cost_summary(
    out: &mut String,
    program: &CodegenProgram,
    placement: &PlacementTable,
    ghz: f64,
    source: Option<&std::path::Path>,
) -> Result<(), String> {
    let table = cost::load_default()?;
    let mut report = cost::score_program(program, &table, placement)?;
    let attach = cost::WorkloadAttach::load_default_for(source, program, &table, placement)?;
    cost::attach_workloads(&mut report, &attach)?;
    out.push_str(&format_cost_summary(
        &report,
        placement,
        ghz,
        Some(&attach),
    )?);
    Ok(())
}

/// Format the Cost block (depth-1 under `ImageReport v0`): owners plus
/// optional Core/Shared attribution. No Placeable lines.
pub fn format_cost_summary(
    report: &CostReport,
    placement: &PlacementTable,
    ghz: f64,
    attach: Option<&cost::WorkloadAttach>,
) -> Result<String, String> {
    let app = report.owner_totals.get("app").copied().unwrap_or(0);
    let runtime = report.owner_totals.get("runtime").copied().unwrap_or(0);
    let driver = report.owner_totals.get("driver").copied().unwrap_or(0);
    let mut header = format!(
        "  Cost version={} digest={} total={} ghz={}",
        report.version,
        report.digest,
        report.total_proxy_cycles,
        cost::fmt_compact(ghz),
    );
    if let Some(wd) = &report.workloads_digest {
        header.push_str(&format!(" workloads_digest={wd}"));
    }
    header.push('\n');
    let mut out = header;
    cost_dump::append_workload_rows(&mut out, 2, report, attach);
    out.push_str(&format!(
        "    Owner name=app proxy_cycles={app}\n\
         \x20   Owner name=runtime proxy_cycles={runtime}\n\
         \x20   Owner name=driver proxy_cycles={driver}\n"
    ));
    cost_dump::append_core_block(&mut out, 2, report, placement, ghz, false, attach)?;
    Ok(out)
}

/// **Section 8d — the calling convention this build chose, per function**
/// (plans/codegen-pareto.md item F).
///
/// The plan's own words: *"the report should show each function's chosen
/// convention — otherwise the most consequential decision in the compiler
/// is invisible."* Item F computes a different convention for every
/// function in the image, from a whole-program analysis, and nothing
/// downstream of codegen could otherwise say what it chose.
///
/// **A function with nothing to say is absent**, exactly as every other
/// section of this report is. Under `dev`, and under item E's
/// per-function allocator, no function has a convention of its own, so
/// the whole section disappears and every `dev` report is unchanged. A
/// function that got no residents, no frame deletion and no tail call
/// contributes no line even under `release`: the section is the list of
/// functions whose convention is not the default one.
///
/// One line per such function:
///
/// ```text
///   Convention fns=3 frameless=1 tail_calls=1
///     Fn key=blend frame=0 residents=4 regs=x2-x5 clobbers=x0-x1,x9,x30-x31 pool=19
///     Fn key=spans frame=16 residents=3 regs=x2-x4 clobbers=all pool=14
/// ```
///
/// `clobbers=all` is the fail-closed answer — a function on a call-graph
/// cycle, or one that reaches a body this compiler does not hold — and it
/// is spelled by name rather than as a mask so it cannot be mistaken for
/// a function that happens to touch every register.
pub fn append_convention_section(out: &mut String, program: &CodegenProgram) {
    if program.conventions.is_empty() {
        return;
    }
    let mut rows: Vec<String> = Vec::new();
    let mut frameless = 0usize;
    for (key, conv) in &program.conventions {
        let frame = program.fns.get(key).map(|f| f.frame_size);
        let residents = conv.assignment.resident_count();
        let interesting = residents > 0 || frame == Some(0);
        if !interesting {
            continue;
        }
        if frame == Some(0) {
            frameless += 1;
        }
        let regs: u32 = conv
            .assignment
            .residents()
            .iter()
            .fold(0u32, |m, &(_, r)| m | crate::regalloc::reg_bit(r));
        rows.push(format!(
            "    Fn key={key} frame={} residents={residents} regs={} clobbers={} pool={}\n",
            frame.map_or_else(|| "?".to_string(), |f| f.to_string()),
            crate::regalloc::render_reg_set(regs),
            crate::regalloc::render_reg_set(conv.clobbers),
            conv.pool.len(),
        ));
    }
    if rows.is_empty() {
        return;
    }
    let tail_calls: usize = program
        .fns
        .values()
        .flat_map(|f| f.code.iter())
        .filter(|w| w.text.ends_with("; tail call"))
        .count();
    out.push_str(&format!(
        "  Convention fns={} frameless={frameless} tail_calls={tail_calls}\n",
        rows.len()
    ));
    for r in rows {
        out.push_str(&r);
    }
}

// --- the digest (plans/M4.md item D, decision 9): one hardcoded SHA-256
// shared with the VMM via `wrela_machine::sha256` (06 §3 / §8). ---------

/// Plain SHA-256 hex (FIPS 180-4). Re-export of the machine-crate
/// implementation so report rendering and the VMM cannot drift.
pub use wrela_machine::sha256::sha256_hex;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::eval::image::{DeclArg, TypedValue};
    use crate::sema::types::Type;

    // --- SHA-256 standard public test vectors --------------------------------

    #[test]
    fn sha256_of_empty_string() {
        assert_eq!(
            sha256_hex(b""),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
    }

    #[test]
    fn sha256_of_abc() {
        assert_eq!(
            sha256_hex(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    // --- address_to_relative_path -------------------------------------------

    #[test]
    fn address_to_relative_path_joins_dots_as_slashes() {
        assert_eq!(address_to_relative_path("image"), "image.wr");
        assert_eq!(address_to_relative_path("core.bytes"), "core/bytes.wr");
        assert_eq!(
            address_to_relative_path("examples.image_basic"),
            "examples/image_basic.wr"
        );
    }

    // --- section ordering on a hand-built graph ------------------------------

    fn tv(ty: Type, value: Value) -> TypedValue {
        TypedValue { ty, value }
    }

    fn decl_arg(label: &str, ty: Type, value: Value) -> DeclArg {
        DeclArg {
            label: label.to_string(),
            ty,
            value,
        }
    }

    fn image_decl_ty() -> Type {
        Type::Named("ImageDecl".to_string(), vec![])
    }

    /// A small, real-shaped graph — one driver (a plain arg), one actor
    /// (a handle edge + a mailbox + a plain arg), one pool, one failure
    /// policy — enough to exercise every section this module renders in
    /// one pass.
    fn sample_graph() -> (ImageGraph, BTreeMap<String, Vec<String>>) {
        let mut enums = BTreeMap::new();
        enums.insert("Target".to_string(), vec!["wrela_machine_v1".to_string()]);
        enums.insert(
            "Failure".to_string(),
            vec!["Reboot".to_string(), "Halt".to_string()],
        );

        let mut g = ImageGraph::new(
            tv(
                Type::Static(Box::new(Type::Str)),
                Value::Str(b"sample".to_vec()),
            ),
            tv(
                Type::Named("Target".to_string(), vec![]),
                Value::Enum(0, vec![]),
            ),
        );
        g.drivers.push(crate::eval::image::DriverDecl {
            actor_type: Type::Named("Blk".to_string(), vec![]),
            args: vec![decl_arg("queue_depth", Type::U32, Value::U32(8))],
        });
        g.actors.push(crate::eval::image::ActorDecl {
            actor_type: Type::Named("Store".to_string(), vec![]),
            args: vec![
                decl_arg(
                    "disk",
                    image_decl_ty(),
                    Value::ImageDecl(ImageDeclRef::Driver(0)),
                ),
                decl_arg("mailbox", Type::U32, Value::U32(16)),
            ],
        });
        g.pools.insert(
            "Buffers".to_string(),
            crate::eval::image::PoolDecl {
                payload_type: Type::U32,
                args: vec![decl_arg("slots", Type::U32, Value::U32(4))],
            },
        );
        g.declare_on_failure(vec![decl_arg(
            "policy",
            Type::Named("Failure".to_string(), vec![]),
            Value::Enum(1, vec![]), // Failure.Halt
        )]);
        g.sealed = true;
        (g, enums)
    }

    #[test]
    fn render_produces_every_section_in_fixed_order() {
        let (g, enums) = sample_graph();
        let inputs = vec![BuildInput {
            path: "image.wr".to_string(),
            digest: sha256_hex(b"placeholder"),
        }];
        let text = render(&inputs, &enums, &g, &PlacementTable::default())
            .expect("no layout asserts registered");

        let expected = format!(
            "ImageReport v0\n\
             \x20 Compiler version={COMPILER_VERSION}\n\
             \x20 Machine revision={}\n\
             \x20 Target value=Target.wrela_machine_v1\n\
             \x20 Quota max_steps={}\n\
             \x20 Quota max_memory={}\n\
             \x20 Quota max_call_depth={}\n\
             \x20 Quota max_exhaustive_cases={}\n\
             \x20 Input path=image.wr sha256={}\n\
             \x20 Name value=sample\n\
             \x20 Target value=Target.wrela_machine_v1\n\
             \x20 Driver index=0 type=Blk\n\
             \x20   Arg label=queue_depth value=8\n\
             \x20 Actor index=0 type=Store\n\
             \x20   Mailbox value=16\n\
             \x20 Edge from=actor#0 to=driver#0\n\
             \x20 Pool name=Buffers type=u32\n\
             \x20   Arg label=slots value=4\n\
             \x20 OnFailure index=0 policy=Failure.Halt\n",
            wrela_machine::MACHINE_REVISION_STR,
            quota::MAX_STEPS,
            quota::MAX_MEMORY,
            quota::MAX_CALL_DEPTH,
            quota::MAX_EXHAUSTIVE_CASES,
            sha256_hex(b"placeholder"),
        );
        assert_eq!(text, expected);
    }

    #[test]
    fn a_registered_layout_assert_no_longer_blocks_render() {
        // plans/M9.md item H: decision 10's report-boundary refusal is
        // retired. `render` produces the report; `eval::layout_assert`
        // runs the registered fns after layout. End-to-end pins:
        // golden/check-layout-assert-passes, golden/err-layout-assert-fails.
        let (mut g, enums) = sample_graph();
        g.declare_check_layout("check_limits".to_string());
        let text = render(&[], &enums, &g, &PlacementTable::default())
            .expect("registered layout asserts must not block render");
        assert!(text.starts_with("ImageReport v0\n"));
    }

    /// plans/codegen-pareto.md item F: **a function's chosen convention
    /// appears in `--stage=report`.** Built from a real compile rather
    /// than a hand-made `CodegenProgram`, because the claim is about what
    /// the compiler decides, not about a formatter.
    #[test]
    fn the_convention_section_publishes_what_the_whole_program_pass_chose() {
        use crate::opts::{CompileMode, apply_mode};
        const SRC: &str = r#"
module examples.report_convention

fn leaf(a: u64) -> u64:
    x: u64 = a +% 1
    return x +% x

pub fn caller(a: u64) -> u64:
    keep: u64 = a *% 3
    p: u64 = leaf(a)
    return (keep +% p) +% keep
"#;
        let build = |mode: CompileMode| -> String {
            apply_mode(mode);
            let tokens = crate::syntax::lexer::lex(SRC).expect("lex");
            let module = crate::syntax::parser::parse(tokens).expect("parse");
            let typed = crate::sema::check_typed(&module, "<t>").expect("check");
            let mwir = crate::lower::lower_program(&typed).expect("lower");
            let ctx = crate::mwir::build_layout_ctx(&module, &Default::default()).expect("ctx");
            let prog = crate::codegen::codegen_program(&mwir, &ctx).expect("codegen");
            let mut out = String::new();
            append_convention_section(&mut out, &prog);
            out
        };

        // Under `dev` every function has the same convention, so the
        // section has nothing to say and is absent — the report's own
        // rule, applied to the new section.
        assert_eq!(build(CompileMode::Dev), "", "dev must add no section");

        let text = build(CompileMode::Release);
        apply_mode(CompileMode::Release);
        assert!(
            text.starts_with("  Convention fns="),
            "the section must lead with its own counts:\n{text}"
        );
        assert!(
            text.contains("    Fn key=leaf frame=0 "),
            "the frameless leaf must be published as frameless:\n{text}"
        );
        for want in ["residents=", "regs=x", "clobbers=", "pool="] {
            assert!(text.contains(want), "missing `{want}`:\n{text}");
        }
        // The clobber set is the fact a caller reads, so it must be a
        // measured one for a leaf, never the fail-closed answer.
        let leaf_line = text
            .lines()
            .find(|l| l.contains("Fn key=leaf "))
            .expect("leaf line");
        assert!(
            !leaf_line.contains("clobbers=all"),
            "a leaf's clobber set must be measured: {leaf_line}"
        );
        // Same input, same bytes.
        assert_eq!(build(CompileMode::Release), text);
        apply_mode(CompileMode::Release);
    }

    /// plans/M7.md item B: the exact-bytes section is appended by the
    /// caller (`bin/wrela.rs::build_report`), so `render`'s own determinism
    /// test above does not reach it. This one does: same input, same bytes,
    /// appended to whatever the report already said and nothing else.
    /// `golden/check-layout-mmio/expected/report.txt` is the end-to-end
    /// pin; this is the property.
    #[test]
    fn the_exact_bytes_section_is_a_pure_appending_function() {
        let layout = types::LayoutType {
            name: "VirtioIrqMmio".to_string(),
            kind: types::LayoutKind::Mmio,
            endian: types::LayoutEndian::Little,
            size: Some(0x68),
            padding: 0x60,
            entries: vec![
                types::LayoutEntry::Padding {
                    offset: 0,
                    size: 0x60,
                },
                types::LayoutEntry::Field(types::LayoutField {
                    name: "interrupt_status".to_string(),
                    ty: "ReadOnly[u32]".to_string(),
                    offset: 0x60,
                    size: 4,
                }),
            ],
        };
        let mut a = String::from("ImageReport v0\n");
        let mut b = a.clone();
        render_exact_bytes_section(&mut a, std::slice::from_ref(&layout)).expect("complete");
        render_exact_bytes_section(&mut b, std::slice::from_ref(&layout)).expect("complete");
        assert_eq!(a, b);
        assert_eq!(
            a,
            "ImageReport v0\n\
             \x20 Layout name=VirtioIrqMmio kind=mmio endian=little size=104 padding=96\n\
             \x20   Padding offset=0x0 size=96\n\
             \x20   Field name=interrupt_status type=ReadOnly[u32] offset=0x60 size=4\n"
        );
        // Nothing to say means nothing printed — which is why no existing
        // report golden moved when this section landed.
        let mut empty = String::from("ImageReport v0\n");
        render_exact_bytes_section(&mut empty, &[]).expect("nothing to render");
        assert_eq!(empty, "ImageReport v0\n");
    }

    #[test]
    fn render_is_a_pure_function_of_its_arguments() {
        let (g, enums) = sample_graph();
        let inputs = vec![BuildInput {
            path: "image.wr".to_string(),
            digest: sha256_hex(b"x"),
        }];
        let a = render(&inputs, &enums, &g, &PlacementTable::default()).unwrap();
        let b = render(&inputs, &enums, &g, &PlacementTable::default()).unwrap();
        assert_eq!(a, b);
    }

    /// plans/M18.md item R: short Cost summary is owners-only and always
    /// names the three buckets (zeros ok).
    #[test]
    fn cost_summary_contains_version_and_owners() {
        use crate::codegen::CodegenFn;
        use crate::cost::rule::{CostRule, EmittedWord};
        use std::collections::BTreeMap;

        let mut fns = BTreeMap::new();
        fns.insert(
            "checked_add".to_string(),
            CodegenFn {
                frame_size: 0,
                code: vec![EmittedWord::new(
                    0,
                    String::new(),
                    CostRule::Alu,
                    Some(1),
                    &[0, 0],
                )],
                relocs: Vec::new(),
            },
        );
        let program = CodegenProgram {
            fns,
            rodata: Vec::new(),
            ..Default::default()
        };
        let mut out = String::from("ImageReport v0\n");
        append_cost_summary(
            &mut out,
            &program,
            &PlacementTable::default(),
            cost::DEFAULT_GHZ,
            None,
        )
        .expect("default cost table");
        assert!(
            out.contains("Cost version=3"),
            "missing Cost version line:\n{out}"
        );
        assert!(out.contains("ghz=2.4"), "missing ghz:\n{out}");
        assert!(out.contains("Workload name=flat proxy_cycles="));
        assert!(out.contains("Owner name=app proxy_cycles="));
        assert!(out.contains("Owner name=runtime proxy_cycles="));
        assert!(out.contains("Owner name=driver proxy_cycles="));
        assert!(!out.contains("Term rule="));
        assert!(!out.contains("Fn key="));
        assert!(!out.contains("Placeable "));
    }

    #[test]
    fn format_cost_summary_aggregates_owners() {
        use std::collections::BTreeMap;
        let report = CostReport {
            version: 3,
            digest: "deadbeef".to_string(),
            provenance: "test-prov".to_string(),
            provenance_summary: "T1=1 T2=0 T3=0 T4=0 T5=0 rows=1".to_string(),
            profile: "a76-pi5".to_string(),
            pipelines: 8,
            dispatch_mops: 4,
            dispatch_uops: 8,
            reorder_window: 128,
            total_proxy_cycles: 30,
            total_words: 30,
            owner_totals: BTreeMap::from([
                ("app".to_string(), 10u64),
                ("runtime".to_string(), 12u64),
                ("driver".to_string(), 8u64),
            ]),
            fns: vec![],
            workloads_digest: Some("wdigest".to_string()),
            workload_totals: BTreeMap::from([("flat".to_string(), 30u64)]),
            workload_coverage: BTreeMap::new(),
            // 04 §6 / plans/M20.md item F: one per-core text and translation
            // budget line per core, pinned here beside the `Core n=` line it
            // belongs to. The numbers are a fixture, not a scored program —
            // `cost::footprint`'s units own the model.
            footprint: vec![cost::CoreBudget {
                n: 0,
                hot_text_bytes: 1216,
                l1i_bytes: 65536,
                over_l1i_lines: 0,
                over_l2_lines: 0,
                text_pages: 1,
                itlb_entries: 48,
                over_itlb_pages: 0,
                tlb_l2_entries: 1280,
                over_tlb_l2_pages: 0,
                data_pages: 2,
                over_dtlb_pages: 0,
                over_data_tlb_l2_pages: 0,
                charge: 0,
            }],
        };
        // Default placement has cores=1 → Core + Budget + Shared lines appear.
        let text =
            format_cost_summary(&report, &PlacementTable::default(), cost::DEFAULT_GHZ, None)
                .expect("format");
        assert_eq!(
            text,
            "  Cost version=3 digest=deadbeef total=30 ghz=2.4 workloads_digest=wdigest\n\
               \x20   Workload name=flat proxy_cycles=30\n\
               \x20   Owner name=app proxy_cycles=10\n\
               \x20   Owner name=runtime proxy_cycles=12\n\
               \x20   Owner name=driver proxy_cycles=8\n\
               \x20   Core n=0 proxy_cycles=0 max_turn_proxy=0 turns_per_sec=n/a ms_per_turn_model=n/a\n\
               \x20   Budget n=0 hot_text_bytes=1216 l1i_bytes=65536 over_l1i_lines=0 over_l2_lines=0 text_pages=1 itlb_entries=48 over_itlb_pages=0 tlb_l2_entries=1280 over_tlb_l2_pages=0 data_pages=2 over_dtlb_pages=0 over_data_tlb_l2_pages=0 charge=0\n\
               \x20   Shared proxy_cycles=0\n"
        );
    }

    #[test]
    fn format_cost_summary_omits_cores_when_placement_empty() {
        use std::collections::BTreeMap;
        let report = CostReport {
            version: 3,
            digest: "deadbeef".to_string(),
            provenance: "test-prov".to_string(),
            provenance_summary: "T1=1 T2=0 T3=0 T4=0 T5=0 rows=1".to_string(),
            profile: "a76-pi5".to_string(),
            pipelines: 8,
            dispatch_mops: 4,
            dispatch_uops: 8,
            reorder_window: 128,
            total_proxy_cycles: 30,
            total_words: 30,
            owner_totals: BTreeMap::from([
                ("app".to_string(), 10u64),
                ("runtime".to_string(), 12u64),
                ("driver".to_string(), 8u64),
            ]),
            fns: vec![],
            workloads_digest: None,
            workload_totals: BTreeMap::new(),
            workload_coverage: BTreeMap::new(),
            footprint: Vec::new(),
        };
        let empty = PlacementTable {
            entries: Vec::new(),
            cores: 0,
        };
        let text = format_cost_summary(&report, &empty, cost::DEFAULT_GHZ, None).expect("format");
        assert_eq!(
            text,
            "  Cost version=3 digest=deadbeef total=30 ghz=2.4\n\
               \x20   Workload name=flat proxy_cycles=30\n\
               \x20   Owner name=app proxy_cycles=10\n\
               \x20   Owner name=runtime proxy_cycles=12\n\
               \x20   Owner name=driver proxy_cycles=8\n"
        );
    }
}
