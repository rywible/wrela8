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
//! 4. `Driver`/`Actor` blocks: every non-handle, non-`mailbox` argument
//!    as an `Arg` line (05 §9's own "arguments ... match `A.init` ...
//!    after ... substituted" — decision 7's "reserved args are wiring
//!    metadata" already strips `device`/`core`/`mailbox` from ordinary
//!    matching, and this renderer goes one step further: it pulls every
//!    *handle* argument, reserved or not, out into its own `Edge` fact
//!    below instead of an `Arg` line, and pulls `mailbox` out into its
//!    own `Mailbox` fact — "declared mailboxes (recorded as-declared, no
//!    derivation)" per decision 8).
//! 5. `Edge from=<decl> to=<decl>` — every logical actor edge (decision-8
//!    sub-note on direction, below), one line each, in the same
//!    construction order the edges were discovered in (devices, then
//!    drivers, then actors; each declaration's own argument order).
//! 6. `Pool` blocks (args verbatim, same reasoning as `Device`).
//! 7. `Supervise` blocks: `strategy`/`intensity` pulled onto the header
//!    line itself, `children` expanded into one `Child` line per member;
//!    any other labeled argument (there is no other in 05 §9's own
//!    surface, but the renderer does not silently drop a real fact it
//!    cannot name) falls back to an ordinary `Arg` line.
//! 8. Registered layout asserts: **never actually rendered** — decision
//!    10's own report-boundary enforcement (`render`'s first act) fails
//!    the whole report closed, citing the gap `image.report.layout-asserts`
//!    (plans/M5.md item D's own reword of this diagnostic — the stdlib
//!    `ImageReport` reflection type a real `@layout_assert` run needs
//!    still does not exist), the instant even one `@layout_assert` is
//!    registered, so a *successful* report can never
//!    reach this section with anything to print. No dead rendering code
//!    exists for it here for exactly that reason (unlike
//!    `eval::image_checks::check_construction_dag`'s own unrepresentable-
//!    but-real-code cycle case): there is no "empty" shape this section
//!    could ever take other than total absence, and total absence needs
//!    no code to produce.
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

use crate::eval::image::{self, ImageDeclRef, ImageGraph, TypedProgramEnums};
use crate::eval::quota;
use crate::eval::value::Value;
use crate::sema::types;

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
/// an argument labeled `mailbox` becomes a `Mailbox` fact; everything
/// else is an ordinary `Arg` fact, rendered with `eval::image`'s own
/// `render_value` verbatim.
fn decl_facts(program: &TypedProgramEnums, args: &[image::DeclArg]) -> Vec<DeclFact> {
    let mut out = Vec::with_capacity(args.len());
    for a in args {
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

/// Renders the whole `ImageReport` (plans/M4.md item D). `inputs` must
/// already carry one entry per file in the build closure — this function
/// does no I/O and trusts its caller (`bin/wrela.rs::run_report_stage`,
/// `xtask`'s own determinism oracle) to have read and hashed every real
/// file; it renders `inputs` in the order given (the caller supplies them
/// in `BTreeMap`-by-address order, so the report's own `Input` lines come
/// out deterministically path-sorted with no further sorting needed
/// here). `Err` is decision 10's own report-boundary enforcement: a
/// registered `@layout_assert` fails the whole report closed, citing the
/// gap `image.report.layout-asserts` (plans/M5.md item D's own reword,
/// replacing the earlier "M5" milestone-number wording with the actual
/// missing capability it names — the stdlib `ImageReport` reflection
/// type) — this is the *only* way `render` ever fails; a sealed, checked graph
/// has nothing else left to reject (every other rejection already
/// happened earlier in the pipeline, at `check_sealed` or before).
pub fn render(
    inputs: &[BuildInput],
    enums: &BTreeMap<String, Vec<String>>,
    graph: &ImageGraph,
) -> Result<String, String> {
    if !graph.layout_asserts.is_empty() {
        let names: Vec<String> = graph
            .layout_asserts
            .iter()
            .map(|a| a.fn_key.clone())
            .collect();
        return Err(format!(
            "the image report cannot be produced: {} registered `@layout_assert` fn(s) ({}) — \
             registered @layout_assert fns cannot run yet: the stdlib ImageReport reflection \
             type does not exist (gap image.report.layout-asserts); this compiler never \
             asserts against nothing (decision 10); `--stage=image` still dumps the raw \
             graph for such an image, only the report itself refuses",
            graph.layout_asserts.len(),
            names.join(", ")
        ));
    }

    let program = TypedProgramEnums { enums };
    let mut out = String::new();
    out.push_str("ImageReport v0\n");

    // --- 1. build identity -------------------------------------------------
    image::push_line(&mut out, 1, &format!("Compiler version={COMPILER_VERSION}"));
    image::push_line(
        &mut out,
        1,
        &format!("Machine revision={}", wrela_machine::MACHINE_REVISION_STR),
    );
    if let Some(target) = &graph.target {
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
    for inp in inputs {
        image::push_line(
            &mut out,
            1,
            &format!("Input path={} sha256={}", inp.path, inp.digest),
        );
    }

    // --- 2. the image --------------------------------------------------------
    if let Some(name) = &graph.name {
        image::push_line(
            &mut out,
            1,
            &format!(
                "Name value={}",
                image::render_value(&program, &name.ty, &name.value)
            ),
        );
    }
    if let Some(target) = &graph.target {
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

    for (i, d) in graph.devices.iter().enumerate() {
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
    for (i, d) in graph.drivers.iter().enumerate() {
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
    for (i, d) in graph.actors.iter().enumerate() {
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
    for (name, d) in &graph.pools {
        image::push_line(
            &mut out,
            1,
            &format!(
                "Pool name={name} type={}",
                types::render_type(&d.payload_type)
            ),
        );
        // Pool args never carry a decl-reference in practice (05 §9 never
        // wires a pool to another declaration — `img.dma_pool`'s own
        // `device=` argument is the one exception, and `dma_pools` is
        // always empty by the time a report can ever be rendered, decision
        // 10) — `render_decl_block`'s edge-detecting rule still runs
        // uniformly here rather than special-casing pools out of it (this
        // module's own doc comment: "dumb and correct," not "correct
        // because special-cased"). Any such edge would simply be appended
        // to `edges` too late for section 5's own already-written output —
        // acceptable because it cannot happen today; not silently wrong
        // because it is real code that would need to change, not a gap
        // hidden behind a check.
        let mut unused_edges = Vec::new();
        render_decl_block(
            &program,
            &ImageDeclRef::Pool(name.clone()),
            &d.args,
            &mut out,
            &mut unused_edges,
        );
    }

    // --- 7. supervision tree ---------------------------------------------------
    for (i, s) in graph.supervisions.iter().enumerate() {
        let strategy = s
            .args
            .iter()
            .find(|a| a.label == "strategy")
            .map(|a| image::render_value(&program, &a.ty, &a.value));
        let intensity = s
            .args
            .iter()
            .find(|a| a.label == "intensity")
            .map(|a| image::render_value(&program, &a.ty, &a.value));
        let mut header = format!("Supervise index={i}");
        if let Some(v) = &strategy {
            header.push_str(&format!(" strategy={v}"));
        }
        if let Some(v) = &intensity {
            header.push_str(&format!(" intensity={v}"));
        }
        image::push_line(&mut out, 1, &header);
        for a in &s.args {
            match a.label.as_str() {
                "strategy" | "intensity" => {}
                "children" => {
                    if let Value::Array(items) = &a.value {
                        for item in items {
                            let rendered = if let Value::ImageDecl(r) = item {
                                r.render()
                            } else {
                                image::render_bare_value(item)
                            };
                            image::push_line(&mut out, 2, &format!("Child value={rendered}"));
                        }
                    } else {
                        image::push_line(
                            &mut out,
                            2,
                            &format!(
                                "Arg label=children value={}",
                                image::render_value(&program, &a.ty, &a.value)
                            ),
                        );
                    }
                }
                _ => {
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
        }
    }

    // --- 8. registered layout asserts: never reached, see module doc --------

    Ok(out)
}

// --- the digest (plans/M4.md item D, decision 9): one hardcoded,
// hand-rolled SHA-256, plain u32 ops, no new dependency. Standard public
// test vectors (empty string, "abc") below. ---------------------------------

const SHA256_K: [u32; 64] = [
    0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4, 0xab1c5ed5,
    0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe, 0x9bdc06a7, 0xc19bf174,
    0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f, 0x4a7484aa, 0x5cb0a9dc, 0x76f988da,
    0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7, 0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967,
    0x27b70a85, 0x2e1b2138, 0x4d2c6dfc, 0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85,
    0xa2bfe8a1, 0xa81a664b, 0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070,
    0x19a4c116, 0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
    0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7, 0xc67178f2,
];

const SHA256_H0: [u32; 8] = [
    0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab, 0x5be0cd19,
];

/// Plain, textbook SHA-256 (FIPS 180-4) over `data`, hex-lowercase output
/// — the whole of decision 9's "one hardcoded hash implemented in-tree;
/// no new dependency." No streaming API, no incremental hasher state: the
/// digest is a pure function of a byte slice, called once per build-
/// closure file (`bin/wrela.rs::run_report_stage`, `xtask`'s own
/// determinism oracle), which is all this milestone ever needs.
pub fn sha256_hex(data: &[u8]) -> String {
    let mut h = SHA256_H0;

    let bit_len = (data.len() as u64).wrapping_mul(8);
    let mut msg = data.to_vec();
    msg.push(0x80);
    while msg.len() % 64 != 56 {
        msg.push(0);
    }
    msg.extend_from_slice(&bit_len.to_be_bytes());

    for chunk in msg.chunks(64) {
        let mut w = [0u32; 64];
        for (i, word) in w.iter_mut().take(16).enumerate() {
            *word = u32::from_be_bytes([
                chunk[4 * i],
                chunk[4 * i + 1],
                chunk[4 * i + 2],
                chunk[4 * i + 3],
            ]);
        }
        for i in 16..64 {
            let s0 = w[i - 15].rotate_right(7) ^ w[i - 15].rotate_right(18) ^ (w[i - 15] >> 3);
            let s1 = w[i - 2].rotate_right(17) ^ w[i - 2].rotate_right(19) ^ (w[i - 2] >> 10);
            w[i] = w[i - 16]
                .wrapping_add(s0)
                .wrapping_add(w[i - 7])
                .wrapping_add(s1);
        }

        let (mut a, mut b, mut c, mut d, mut e, mut f, mut g, mut hh) =
            (h[0], h[1], h[2], h[3], h[4], h[5], h[6], h[7]);

        for i in 0..64 {
            let s1 = e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25);
            let ch = (e & f) ^ ((!e) & g);
            let temp1 = hh
                .wrapping_add(s1)
                .wrapping_add(ch)
                .wrapping_add(SHA256_K[i])
                .wrapping_add(w[i]);
            let s0 = a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22);
            let maj = (a & b) ^ (a & c) ^ (b & c);
            let temp2 = s0.wrapping_add(maj);

            hh = g;
            g = f;
            f = e;
            e = d.wrapping_add(temp1);
            d = c;
            c = b;
            b = a;
            a = temp1.wrapping_add(temp2);
        }

        h[0] = h[0].wrapping_add(a);
        h[1] = h[1].wrapping_add(b);
        h[2] = h[2].wrapping_add(c);
        h[3] = h[3].wrapping_add(d);
        h[4] = h[4].wrapping_add(e);
        h[5] = h[5].wrapping_add(f);
        h[6] = h[6].wrapping_add(g);
        h[7] = h[7].wrapping_add(hh);
    }

    h.iter().map(|word| format!("{word:08x}")).collect()
}

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
    /// (a handle edge + a mailbox + a plain arg), one pool, one supervise
    /// group over both — enough to exercise every section this module
    /// renders in one pass.
    fn sample_graph() -> (ImageGraph, BTreeMap<String, Vec<String>>) {
        let mut enums = BTreeMap::new();
        enums.insert("Target".to_string(), vec!["wrela_machine_v1".to_string()]);
        enums.insert(
            "Restart".to_string(),
            vec!["OneForOne".to_string(), "OneForAll".to_string()],
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
        g.declare_supervise(vec![
            decl_arg(
                "children",
                Type::Named("ImageDeclArray".to_string(), vec![]),
                Value::Array(vec![
                    Value::ImageDecl(ImageDeclRef::Driver(0)),
                    Value::ImageDecl(ImageDeclRef::Actor(0)),
                ]),
            ),
            decl_arg(
                "strategy",
                Type::Named("Restart".to_string(), vec![]),
                Value::Enum(0, vec![]),
            ),
        ]);
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
        let text = render(&inputs, &enums, &g).expect("no layout asserts registered");

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
             \x20 Supervise index=0 strategy=Restart.OneForOne\n\
             \x20   Child value=driver#0\n\
             \x20   Child value=actor#0\n",
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
    fn a_registered_layout_assert_fails_the_report_closed() {
        let (mut g, enums) = sample_graph();
        g.declare_check_layout("check_limits".to_string());
        let err = render(&[], &enums, &g).expect_err("decision 10: never asserted against nothing");
        assert!(err.contains("check_limits"));
        assert!(err.contains("image.report.layout-asserts"));
    }

    #[test]
    fn render_is_a_pure_function_of_its_arguments() {
        let (g, enums) = sample_graph();
        let inputs = vec![BuildInput {
            path: "image.wr".to_string(),
            digest: sha256_hex(b"x"),
        }];
        let a = render(&inputs, &enums, &g).unwrap();
        let b = render(&inputs, &enums, &g).unwrap();
        assert_eq!(a, b);
    }
}
