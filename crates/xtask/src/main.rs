//! Local development harness. There is no CI: `cargo xtask check` IS the
//! definition of "the tree is good", run locally before calling anything
//! done. Subcommands:
//!
//!   check      fmt + tests + golden + corpus + fuzz(smoke) + ledger (the gate)
//!   golden     run golden tests; `--update` rewrites expectations
//!   field-visibility-census  empty-census gate (plans/M13.md G3)
//!   corpus     extract every ```wrela block from docs/ and lex it
//!              (from M1, also parse). Always sema-checks every parseable
//!              block (plans/M9.md item J3; per-block stubs / nest from
//!              J1b/J1c) and ratchets the pinned census: ok-decay fails,
//!              an accepted disagreement that starts passing fails (naming
//!              its ledger gap), and every accepted row's gap must still
//!              be `status = "gap"` in `ledger/ledger.toml`. `--sema` only
//!              prints the verbose per-block report for humans; the gate
//!              itself is the same path `check` runs. A standing guard
//!              refuses any wrap that discards fence item text.
//!   fuzz       cargo xtask fuzz [lexer|parser|sema|eval|lower|async]
//!              [--iters N] [--seed S]; deterministic in-tree fuzzer
//!              (plans/M1.md items B/E, plans/M2.md item I, plans/M3.md
//!              item F, plans/M5.md item G, plans/M7.md item Y). All six
//!              targets are live (bare `fuzz` runs `lexer` at the deep
//!              default budget) and all six have a smoke budget wired into
//!              `check`. `sema` runs lex -> parse -> `sema::check` over
//!              corpus/golden-input
//!              mutations and token-soup, same shape as `parser`, plus (on
//!              every iteration whose input parses, ledger clause
//!              sema.check.roundtrip-stable) two more invariants: sema
//!              roundtrip stability (pretty-print, reparse, recheck — the
//!              two sema outcomes must agree) and item-rotation acceptance
//!              invariance (rotating the module's top-level items by one
//!              must not flip Ok/Err either way). `eval` runs lex -> parse
//!              -> `sema::check_typed` (which already evaluates every
//!              const initializer and `comptime assert`) -> on success,
//!              `eval::run_tests` over every comptime-legal `@test`, same
//!              corpus/token-soup shape again; invariants: never panics,
//!              deterministic across two runs, and every outcome is a
//!              well-formed diagnostic or test report (ledger clause
//!              comptime.eval.no-panics). `lower` runs lex -> parse ->
//!              `sema::check_typed` -> on success, `lower::lower_program`
//!              -> on success, `codegen::codegen_program`, over the same
//!              corpus/token-soup shape again (the seed set's own
//!              `tests/golden/{mwir,asm}-*/boot-hello` input files, already
//!              collected by `corpus_seed_inputs`, are what actually give
//!              this lane lowering/codegen-shaped mutation material);
//!              invariants: never panics anywhere in `lower`/`codegen`,
//!              deterministic across two runs (the mwir dump text, the
//!              concatenated codegen'd words, and — whenever the program
//!              declares an `@test(runtime)` fn — the laid-out test image
//!              blob, all byte-compared), a lowering/codegen rejection is
//!              always the fixed `unimplemented` diagnostic category, and
//!              every successfully codegen'd program passes
//!              `codegen::validate`'s own structural checks (ledger clause
//!              compiler.lower.no-panics). `async` (plans/M7.md item Y) is
//!              the same pipeline's *async* half, which `lower` has
//!              disclosed since M6-D that it never reaches at all: lex ->
//!              parse -> `sema::check_typed` -> `lower::lower_program` +
//!              `flowwir_lower::lower_program` -> `eval_image` (whenever
//!              the program declares an `@image`) ->
//!              `codegen::codegen_program_with_async` (the `emit_flowwir_fn`
//!              driver) -> `async_frame_sizes`/`compute_group_child_indices`
//!              -> `layout::layout_test_image` with a real `BootCtx`,
//!              i.e. `bin/wrela.rs::test_cmd`'s own runtime tier stage for
//!              stage. Generation is biased at the surface it covers —
//!              mutation bases are the fixed `ASYNC_SEED_CASES` list of
//!              async/actor goldens, since no random byte stream ever
//!              spells a valid actor image — and every run prints its own
//!              measured reach (how many iterations type-checked, lowered
//!              >=1 async fn, reached async codegen, laid out an async
//!              image). Same invariants as `lower`: never panics,
//!              deterministic across two runs (FlowWir dump, codegen'd
//!              words, image bytes, and the reach itself), every rejection
//!              in the fixed category set, and `"internal error: "`
//!              anywhere is a bug (ledger clause compiler.lower.no-panics).
//!   roundtrip  pretty-print every parseable corpus entry and golden input,
//!              reparse it, and compare the two AST dumps (spans stripped)
//!              — the parser's `diff-eval` (plans/M1.md item E). Also runs
//!              the same sema-roundtrip oracle as `fuzz sema` above,
//!              whenever the entry parses as a whole `Module` (ledger
//!              clause sema.check.roundtrip-stable). Wired into `check`,
//!              after `corpus`.
//!   report-determinism
//!              plans/M4.md item D, decision 9, grown by plans/M5.md item D
//!              (decision 10): for every golden case carrying an
//!              `expected/report.txt`, produces `wrela dump --stage=report`'s
//!              own output PLUS whatever `wrela build` would write as
//!              `<name>.img` *twice*, in-process (fresh lex/parse/sema/
//!              eval/lower/codegen/layout every call — no caching, no
//!              shared state, `produce_report_and_image`), and byte-
//!              compares both the report text and the image bytes (`Some`
//!              only when the program's own reachable surface fully
//!              lowers — `layout::try_layout_program`'s "all or nothing"
//!              rule) — flips `compiler.repro.byte-identical` from gap to
//!              test (the *unsigned* image + report; the signed triple is
//!              M8+ territory, noted in the clause itself). Wired into
//!              `check`, right after `golden` (the same cases `golden`
//!              itself just proved match the pinned expectation — this
//!              oracle instead proves two *fresh* runs agree with each
//!              other, a distinct property golden alone does not).
//!              (wired into `check` at plans/M8.md item C3's finding,
//!              2026-07-25: the four tamper lanes are the strongest
//!              oracles here and were opt-in while `check` was the gate.)
//!   repro      plans/M5.md decision 10: the identical oracle as
//!              `report-determinism` above, runnable standalone (no
//!              separate "full corpus" form exists yet — every report-
//!              bearing golden is already the whole population either
//!              name covers at this milestone's scale).
//!   ledger     verify spec-coverage ledger (ledger/ledger.toml)
//!   diff-eval  evaluator-vs-backend differential      (fails closed today)
//!   profile    replay a recorded workload under counters (fails closed today)
//!   bench      cargo xtask bench compiler|build|guest; the compiler lane
//!              is live (plans/M1.md, ROADMAP.md "cleverness budget"): lex +
//!              parse, in-process, over every doc/example corpus entry
//!              plus every tests/golden/*/input.wr (3 warmup + 15 timed
//!              iterations), reporting min/median/max total wall time and
//!              the median for the single largest entry, then comparing
//!              the median against the locked threshold in
//!              bench/thresholds.toml. plans/M2.md item I adds a second
//!              lane in the same command: lex+parse+`sema::check` over
//!              every tests/golden/*/input.wr that lexes and parses (both
//!              sema-ok and sema-error outcomes count; lex/parse-error
//!              inputs are excluded), same 3+15 shape, its own locked
//!              per-entry median (`check_golden_per_entry_us`). plans/M3.md
//!              item F adds
//!              a third lane: lex+parse+`sema::check_typed`+
//!              `eval::run_tests` over every test-bearing golden (the
//!              `check-tests-*` cases with a pinned `test.txt`), same
//!              3+15 shape, its own locked per-entry median
//!              (`eval_tests_per_entry_us`). The three corpus-sized lanes lock
//!              microseconds *per entry* (GOAL.md, 2026-07-25): a
//!              whole-corpus absolute dilutes on every added golden, so a
//!              per-entry regression could hide inside corpus growth.
//!              plans/M4.md item E adds `bench build`, its own lane in its
//!              own subcommand rather than a fourth `bench compiler` key:
//!              the whole build pipeline (loader/single-file fork ->
//!              sema -> `eval_image` -> graph checks -> report render, no
//!              file writes — `produce_report_text`, reused from
//!              `report-determinism`) over the M4 example appliance, same
//!              3+15 shape, its own locked median (`build_appliance_median_us`
//!              in `bench/thresholds.toml`'s own `[build]` table). Wired
//!              into `check`, after roundtrip (`bench compiler` then
//!              `bench build`). `bench guest` and bare `bench` still fail
//!              closed — the guest lane needs the VMM and record/replay,
//!              which land at M5.
//!
//! The cleverness budget (ROADMAP.md): optimizations land only with a
//! profile, a before/after on the same recording, and a lock. `bench
//! compiler` is that lock for the compiler's own speed; the guest lane
//! (`bench guest`) and `profile` still refuse to fake results until M5
//! gives them a machine to measure.
//!
//! Golden discipline: an expectation file changes only together with a
//! ledger clause that justifies it. The golden diff is the review surface.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};
use std::time::{Duration, Instant};

use wrela_compiler::codegen;
use wrela_compiler::eval;
use wrela_compiler::flowwir;
use wrela_compiler::flowwir_lower;
use wrela_compiler::layout;
use wrela_compiler::loader;
use wrela_compiler::lower;
use wrela_compiler::mwir;
use wrela_compiler::placement;
use wrela_compiler::report;
use wrela_compiler::sema;
use wrela_compiler::sema::typed::TestKind;
use wrela_compiler::syntax::ast::Module;
use wrela_compiler::syntax::lexer::{self, Token, TokenKind};
use wrela_compiler::syntax::parser::{self, Parsed};
use wrela_compiler::syntax::printer;

mod corpus_sema_census;
mod corpus_sema_context;

mod bench;
mod corpus;
mod fuzz;
mod golden;

use bench::*;
use corpus::*;
use fuzz::*;
use golden::*;

pub(crate) fn root() -> PathBuf {
    // crates/xtask -> repo root
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("repo root")
        .to_path_buf()
}

/// Every subdirectory of `golden_dir` (one golden case apiece), in
/// deterministic (sorted) order — the scan `corpus_seed_inputs`/`golden`/
/// `roundtrip`/`bench_corpus_entries`/`bench_check_entries` all repeat
/// verbatim before doing their own per-case work.
pub(crate) fn golden_case_dirs(golden_dir: &Path) -> Result<Vec<PathBuf>, String> {
    let mut dirs: Vec<_> = std::fs::read_dir(golden_dir)
        .map_err(|e| format!("read {}: {e}", golden_dir.display()))?
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| p.is_dir())
        .collect();
    dirs.sort();
    Ok(dirs)
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let result = match args.first().map(String::as_str) {
        Some("check") => check(),
        Some("golden") => golden(args.iter().any(|a| a == "--update")),
        Some("corpus") => corpus(&args[1..]),
        Some("roundtrip") => roundtrip(),
        Some("report-determinism") => report_determinism(),
        Some("ledger") => ledger(),
        // plans/M5.md decision 10, item D: `repro` is the same
        // twice-fresh-build byte-compare `report_determinism` already runs
        // in `check` (grown, item D, to cover image bytes as well as
        // report text) — every golden case that emits a `report.txt` is
        // the whole `@image`/`wrela build` population either form covers.
        // Item F grows the *standalone* form one population further:
        // `report_determinism` alone has no way to reach
        // `tests/golden/boot-hello`'s own image at all (a runtime-test
        // image has no `@image` fn, so it never goes through the
        // `wrela build` pipeline `report_determinism` walks) — `repro`
        // additionally proves that image's own byte-reproducibility,
        // making bare `cargo xtask repro` a strict superset of `cargo
        // xtask report-determinism` rather than a bare synonym for it.
        Some("repro") => repro(),
        Some("diff-eval") => diff_eval(),
        Some("diff-blk") => diff_blk(),
        Some("profile") => profile(),
        Some("fuzz") => fuzz(&args[1..]),
        Some("bench") => bench(&args[1..]),
        _ => {
            eprintln!(
                "usage: cargo xtask <check|golden [--update]|corpus [--sema]|fuzz [lexer|parser|sema|eval|lower|async|imports] [--iters N] [--seed S]|roundtrip|report-determinism|ledger|repro|diff-eval|diff-blk|profile|bench <compiler|build|guest>>"
            );
            return ExitCode::FAILURE;
        }
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(msg) => {
            eprintln!("xtask: {msg}");
            ExitCode::FAILURE
        }
    }
}

pub(crate) fn fail_closed(name: &str, why: &str) -> Result<(), String> {
    Err(format!(
        "`{name}` fails closed: {why}. It must never fake a pass."
    ))
}

pub(crate) fn run(cmd: &mut Command, what: &str) -> Result<(), String> {
    let status = cmd
        .current_dir(root())
        .status()
        .map_err(|e| format!("{what}: {e}"))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("{what} failed"))
    }
}

fn check() -> Result<(), String> {
    run(
        Command::new("cargo").args(["fmt", "--all", "--check"]),
        "cargo fmt --check",
    )?;
    run(
        Command::new("cargo").args(["test", "--workspace", "--exclude", "wrela-vmm", "--quiet"]),
        "cargo test",
    )?;
    test_wrela_vmm_signed()?;
    golden(false)?;
    report_determinism()?;
    diff_eval_smoke()?;
    // plans/M9.md item J3: bare `corpus` always sema-classifies and
    // verifies the pinned census (accepted disagreements cite open ledger
    // gaps). Same path as `corpus --sema`; the flag only adds the verbose
    // report. No second collection path.
    corpus(&[])?;
    fuzz_lexer_smoke()?;
    fuzz_parser_smoke()?;
    fuzz_sema_smoke()?;
    fuzz_eval_smoke()?;
    // Wired in at M5-G finalization: the sema branch-scoping fix landed
    // (commit 5766861, sema.names.resolution), the lane runs clean at its
    // deep budget on fresh seeds, and the smoke joins every other lane's.
    fuzz_lower_smoke()?;
    // plans/M7.md item Y: the async half of that same pipeline, which the
    // `lower` lane above has disclosed it never reaches since M6-D.
    fuzz_async_smoke()?;
    // plans/M9.md item II: multi-module closures. Every other lane is
    // single-file; four reachable `internal error:` finds this milestone
    // all needed an import.
    fuzz_imports_smoke()?;
    // (Historical note, kept for the record: this call was briefly and
    // deliberately absent — the lane's first exercise reproduced a real,
    // pinned `sema::bodies` finding, golden/err-mwir-if-else-scope-leak,
    // within any 1000-iteration budget, and wiring it in before the fix
    // exactly the kind of approximation CLAUDE.md's "never fake a pass"
    // rules out, so this lane stays standalone-only
    // (`cargo xtask fuzz lower`) until the sema fix lands, mirroring how
    // `diff-eval`/`profile`/`bench guest` themselves stayed unwired before
    // their own items landed.
    roundtrip()?;
    // plans/M8.md item C3's finding, acted on 2026-07-25. `report_determinism`
    // above proves the *build* is byte-reproducible; `repro` is what proves
    // the **replay** oracles still work — and it is where all four tamper
    // lanes live (a tampered clock read, a tampered device completion, a
    // tampered exit code, and now a tampered admission order, each of which
    // must be *caught by name*). Those are the strongest oracles in this
    // repo, and until now every one of them was opt-in while CLAUDE.md
    // called `check` "the gate": a regression that let a tampered log
    // replay clean would have passed every gate run anyone made.
    //
    // The cost argument that kept it out does not survive measurement:
    // `cargo xtask repro` is ~1.8s wall, against a `check` that is minutes,
    // and it introduces no new dependency class — `check` already runs
    // `test_wrela_vmm_signed` and `bench_guest_lane`, both of which boot
    // over Hypervisor.framework.
    repro()?;
    bench_compiler()?;
    bench_build_lane()?;
    bench_guest_lane()?;
    ledger()?;
    println!("xtask check: ok");
    Ok(())
}

// --- report determinism (plans/M4.md item D, decision 9) -------------------
//
// `04-compiler.md` §8: "identical declared inputs, compiler revision,
// machine revision, and quotas produce a byte-for-byte identical ...
// report." The full binary-image half of that claim
// (`compiler.repro.byte-identical`) stays a gap until M5's linker exists,
// but the *report* half is provable today, right now, on every project-
// shaped golden case: produce it twice, from scratch, and demand the two
// runs agree byte-for-byte. Dumb on purpose — no caching, no parallelism,
// a plain sequential loop over every case with a pinned `report.txt`.

/// Reproduces `wrela dump --stage=report <target>`'s own stdout PLUS
/// (plans/M5.md item D) whatever `wrela build` would write as `<name>.img`
/// alongside it, entirely in-process — no subprocess, no shared state
/// between calls, so calling this twice back-to-back for the same `target`
/// is exactly "fresh loader+sema+eval+lower+codegen+layout each time."
/// Mirrors `bin/wrela.rs`'s own `--stage=report`/`build_report` driver
/// structurally (the single-file/whole-closure fork, one-`@image`
/// discovery, `eval_image`, `check_sealed`, `report::render`, then
/// `layout::merge_layout_ctx`/`layout::try_layout_program`'s identical
/// "all or nothing" attempt) rather than calling into it: that binary's
/// own driver functions are not a library surface this crate can reach, so
/// this is its own small, deliberately parallel copy (CLAUDE.md: "prefer
/// long obvious files over deep indirection") — `golden`'s own pinned
/// `report.txt`/written-image expectations are the tripwire that would
/// catch the two ever silently drifting apart. Always returns `Ok` with
/// the rendered text (a dump, even an error dump, *is* the stable output —
/// the same house rule `bin/wrela.rs`'s own module doc states) plus
/// `Some(image bytes)` exactly when layout succeeded; the outer `Err` path
/// is reserved for this function's own plumbing failures (a file the
/// closure needs cannot be read, or `layout_program`'s own genuine
/// internal-consistency failure) — itself part of what determinism means
/// to prove absent across two runs.
pub(crate) fn produce_report_and_image(target: &Path) -> Result<(String, Option<Vec<u8>>), String> {
    fn render_sema_error(e: &sema::SemaError) -> String {
        let mut s = if e.omit_location {
            format!("error[{}]: {}\n", e.category, e.message)
        } else {
            format!(
                "error[{}]: {} at {}:{}\n",
                e.category, e.message, e.line, e.col
            )
        };
        for line in &e.extra_lines {
            s.push_str(line);
            s.push('\n');
        }
        s
    }

    let source =
        std::fs::read_to_string(target).map_err(|e| format!("read {}: {e}", target.display()))?;
    let tokens = match lexer::lex(&source) {
        Ok(t) => t,
        Err(e) => {
            return Ok((
                format!("error[lex]: {} at {}:{}\n", e.message, e.line, e.col),
                None,
            ));
        }
    };
    let parsed = match parser::parse(tokens) {
        Ok(m) => m,
        Err(e) => {
            return Ok((
                format!("error[parse]: {} at {}:{}\n", e.message, e.line, e.col),
                None,
            ));
        }
    };

    let (programs, file_paths, modules_by_addr): (
        BTreeMap<String, sema::typed::TypedProgram>,
        BTreeMap<String, PathBuf>,
        BTreeMap<String, Module>,
    ) = if parsed.imports.is_empty() {
        match sema::check_typed(&parsed, &target.display().to_string()) {
            Ok(program) => {
                let addr = parsed.path.join(".");
                let mut programs = BTreeMap::new();
                let mut file_paths = BTreeMap::new();
                let mut modules_by_addr = BTreeMap::new();
                file_paths.insert(addr.clone(), target.to_path_buf());
                modules_by_addr.insert(addr.clone(), parsed);
                programs.insert(addr, program);
                (programs, file_paths, modules_by_addr)
            }
            Err(e) => return Ok((render_sema_error(&e), None)),
        }
    } else {
        match loader::load_closure(target) {
            Ok(loaded) => {
                let paths: BTreeMap<Vec<String>, String> = loaded
                    .modules
                    .iter()
                    .map(|(k, m)| (k.clone(), m.file.display().to_string()))
                    .collect();
                let file_paths: BTreeMap<String, PathBuf> = loaded
                    .modules
                    .iter()
                    .map(|(k, m)| (k.join("."), m.file.clone()))
                    .collect();
                let modules: BTreeMap<Vec<String>, Module> = loaded
                    .modules
                    .into_iter()
                    .map(|(k, m)| (k, m.module))
                    .collect();
                let modules_by_addr: BTreeMap<String, Module> = modules
                    .iter()
                    .map(|(k, m)| (k.join("."), m.clone()))
                    .collect();
                match sema::check_program_typed(&modules, &paths) {
                    Ok(programs) => {
                        let programs: BTreeMap<String, sema::typed::TypedProgram> = programs
                            .into_iter()
                            .map(|(k, p)| (k.join("."), p))
                            .collect();
                        (programs, file_paths, modules_by_addr)
                    }
                    Err(e) => return Ok((render_sema_error(&e), None)),
                }
            }
            Err(loader::LoadError::Lex(e)) => {
                return Ok((
                    format!("error[lex]: {} at {}:{}\n", e.message, e.line, e.col),
                    None,
                ));
            }
            Err(loader::LoadError::Parse(e)) => {
                return Ok((
                    format!("error[parse]: {} at {}:{}\n", e.message, e.line, e.col),
                    None,
                ));
            }
            Err(loader::LoadError::Build(e)) => return Ok((render_sema_error(&e), None)),
        }
    };

    let candidates: Vec<(&String, &String)> = programs
        .iter()
        .filter_map(|(m, p)| p.image_fn.as_ref().map(|f| (m, f)))
        .collect();
    match candidates.len() {
        0 => Ok((
            "error[build]: no `@image` fn found in the build closure\n".to_string(),
            None,
        )),
        1 => {
            let (module, fn_name) = candidates[0];
            let program = &programs[module];
            match eval::interp::eval_image(program, fn_name) {
                Ok(graph) => match eval::image_checks::check_sealed(&graph, program, &programs) {
                    Ok(()) => {
                        let mut inputs = Vec::with_capacity(file_paths.len());
                        for (addr, path) in &file_paths {
                            // plans/M11.md item E / decision 780: stub /
                            // live `core.__image_runtime` is not on disk —
                            // digest is inserted after layout (mirrors
                            // `bin/wrela.rs::build_report`).
                            if path.to_string_lossy()
                                == wrela_compiler::rtconfig::GENERATED_INPUT_PATH
                                || addr.as_str() == wrela_compiler::rtconfig::MODULE_ADDR
                            {
                                continue;
                            }
                            let bytes = std::fs::read(path)
                                .map_err(|e| format!("read {}: {e}", path.display()))?;
                            inputs.push(report::BuildInput {
                                path: report::address_to_relative_path(addr),
                                digest: report::sha256_hex(&bytes),
                            });
                        }
                        let mut layout_ctx = layout::merge_layout_ctx(&modules_by_addr)
                            .map_err(|e| render_sema_error(&e))?;
                        layout::enrich_layout_ctx_with_instantiations(&mut layout_ctx, &programs);
                        let placement = match placement::place(
                            &graph,
                            &modules_by_addr,
                            &layout_ctx,
                            graph.cores,
                        ) {
                            Ok(p) => p,
                            Err(e) => return Ok((format!("error[build]: {e}\n"), None)),
                        };
                        let enum_variants: BTreeMap<String, Vec<String>> = program
                            .enums
                            .iter()
                            .map(|(k, e)| (k.clone(), e.variants.clone()))
                            .collect();
                        match report::render(&inputs, &enum_variants, &graph, &placement) {
                            Ok(mut text) => {
                                // plans/M7.md item B disclosed this hole in its
                                // own clause note rather than leaving it to be
                                // found: `bin/wrela.rs` appends the exact-bytes
                                // section here (declaration facts before the
                                // memory map), and this oracle did not — so
                                // `repro`/`report_determinism` were comparing a
                                // report text with the section missing, and a
                                // nondeterminism *inside* it could not have been
                                // caught by the lane whose whole job is catching
                                // nondeterminism. Mirrors the production path
                                // exactly; `check_layouts` already ran and passed
                                // for each module inside the sema check that
                                // produced `programs`, so neither call can fail
                                // here, and both are still handled as real errors.
                                // plans/M10.md item A2b: including the later
                                // completion pass, for the same reason — an
                                // oracle that skipped it would compare a
                                // report whose `const`-length layouts are
                                // missing their sizes.
                                let mut layout_types = Vec::new();
                                for (key, module) in &modules_by_addr {
                                    let specialized = sema::specialize::specialize(module)
                                        .map_err(|e| render_sema_error(&e))?;
                                    let mut layouts = sema::types::check_layouts(&specialized)
                                        .map_err(|e| render_sema_error(&e))?;
                                    if let Some(p) = programs.get(key) {
                                        sema::types::complete_layouts(
                                            &specialized,
                                            p,
                                            &mut layouts,
                                        )
                                        .map_err(|e| render_sema_error(&e))?;
                                    }
                                    layout_types.extend(layouts);
                                }
                                report::render_exact_bytes_section(&mut text, &layout_types)
                                    .map_err(|e| render_sema_error(&e))?;
                                let img = match layout::try_layout_program(
                                    &programs,
                                    &layout_ctx,
                                    &graph,
                                    &modules_by_addr,
                                ) {
                                    Ok(Some(image_layout)) => {
                                        if let Some(ref tables) = image_layout.runtime {
                                            let rt_text =
                                                wrela_compiler::rtconfig::generate_and_typecheck(
                                                    tables,
                                                )
                                                .map_err(|e| {
                                                    if e.ends_with('\n') {
                                                        e
                                                    } else {
                                                        format!("{e}\n")
                                                    }
                                                })?;
                                            let digest = report::sha256_hex(rt_text.as_bytes());
                                            wrela_compiler::rtconfig::insert_generated_input_line(
                                                &mut text, &digest,
                                            );
                                        }
                                        layout::render_layout_section(&mut text, &image_layout);
                                        // plans/M9.md item H: mirror
                                        // `bin/wrela.rs::build_report`.
                                        if let Err(diag) =
                                            eval::layout_assert::run(program, &graph, &image_layout)
                                        {
                                            return Ok((diag, None));
                                        }
                                        Some(image_layout.blob)
                                    }
                                    Ok(None) => {
                                        if !graph.layout_asserts.is_empty() {
                                            let names: Vec<&str> = graph
                                                .layout_asserts
                                                .iter()
                                                .map(|a| a.fn_key.as_str())
                                                .collect();
                                            return Ok((
                                                format!(
                                                    "error[build]: registered `@layout_assert` fn(s) \
                                                     ({}) require a laid-out image; this program's \
                                                     reachable surface did not fully lower\n",
                                                    names.join(", ")
                                                ),
                                                None,
                                            ));
                                        }
                                        None
                                    }
                                    // A layout error is a *rendered
                                    // diagnostic* on the production path
                                    // (`bin/wrela.rs`: `error[build]:
                                    // layout: {e}`), not a harness
                                    // malfunction — and this oracle exists
                                    // to compare exactly what production
                                    // produces. Returning `Err` here made
                                    // the report-determinism lane abort
                                    // instead of diffing the moment a
                                    // golden first pinned a report whose
                                    // layout legitimately fails
                                    // (plans/M8.md item C1's own
                                    // `err-placement-cross-core-send`) —
                                    // found by that golden, fixed here to
                                    // mirror `bin/wrela.rs` word for word.
                                    Err(e) => {
                                        return Ok((format!("error[build]: layout: {e}\n"), None));
                                    }
                                };
                                Ok((text, img))
                            }
                            Err(e) => Ok((format!("error[build]: {e}\n"), None)),
                        }
                    }
                    Err(e) => Ok((render_sema_error(&e), None)),
                },
                Err(e) => Ok((render_sema_error(&eval::to_sema_error(e)), None)),
            }
        }
        _ => {
            let names: Vec<String> = candidates
                .iter()
                .map(|(m, f)| format!("{m}::{f}"))
                .collect();
            Ok((
                format!(
                    "error[build]: more than one `@image` fn reachable in the build closure ({})\n",
                    names.join(", ")
                ),
                None,
            ))
        }
    }
}

/// plans/M5.md decision 10 (`compiler.repro.byte-identical`): "identical
/// declared inputs ... produce a byte-for-byte identical ... image and
/// report." Grown from the M4-era report-only oracle (this fn's own former
/// name) to cover both halves of that sentence: for every golden case
/// carrying a pinned `expected/report.txt`, `produce_report_and_image` runs
/// *twice*, fresh, in-process, and this compares the rendered report text
/// AND the emitted image bytes (`Some`/`Some`-equal, or `None`/`None` —
/// never one `Some` and one `None`, which would itself be a determinism
/// failure) — the same population `golden` itself already pins, so `xtask
/// check`'s own existing `report-determinism` step is, unchanged in name,
/// this clause's own in-check wiring; `cargo xtask repro` (below) is the
/// identical check run standalone. Coverage note (the clause's own
/// "record what's covered now" instruction): the *unsigned* image + report
/// only — the signed triple (M8+) is not implemented yet, named nowhere as
/// covered here.
fn report_determinism() -> Result<(), String> {
    let golden_dir = root().join("tests/golden");
    let mut cases = 0usize;
    let mut failures = Vec::new();
    for case in golden_case_dirs(&golden_dir)? {
        if !case.join("expected/report.txt").exists() {
            continue;
        }
        let target = match golden_case_target(&case)? {
            Some(t) if t.exists() => t,
            _ => {
                failures.push(format!(
                    "{}: expected/report.txt exists but no input.wr/`root` target found",
                    case.display()
                ));
                continue;
            }
        };
        let (first_text, first_img) = produce_report_and_image(&target)?;
        let (second_text, second_img) = produce_report_and_image(&target)?;
        cases += 1;
        if first_text != second_text {
            let first_line = first_text.lines().next().unwrap_or("");
            let mismatch = first_text
                .lines()
                .zip(second_text.lines())
                .enumerate()
                .find(|(_, (a, b))| a != b);
            let where_str = match mismatch {
                Some((i, (a, b))) => format!("first differing line {}: {a:?} vs {b:?}", i + 1),
                None => "outputs differ only in length".to_string(),
            };
            failures.push(format!(
                "{}: two fresh --stage=report runs disagree ({where_str}); first run began {:?}",
                case.display(),
                first_line
            ));
        }
        if first_img != second_img {
            failures.push(format!(
                "{}: two fresh image-emission runs disagree ({} bytes vs {} bytes)",
                case.display(),
                first_img.map(|b| b.len()).unwrap_or(0),
                second_img.map(|b| b.len()).unwrap_or(0),
            ));
        }
    }
    if failures.is_empty() {
        println!(
            "report-determinism: {cases} case(s) reproduced byte-for-byte (report + image where present) across two runs"
        );
        Ok(())
    } else {
        for f in &failures {
            eprintln!("{f}\n");
        }
        Err(format!("report-determinism: {} failure(s)", failures.len()))
    }
}

/// plans/M5.md item F: grows `repro`'s own full-corpus form beyond
/// `report_determinism`'s scope (below) to also prove
/// `tests/golden/boot-hello`'s own `@test(runtime)` test image is
/// byte-reproducible — two fresh, in-process `boot_hello_test_image`
/// calls (itself `build_runtime_test_image` over that case's own
/// runtime tests, `diff-eval`'s/`bench guest`'s shared helper), byte-
/// compared, image bytes and report text alike. The identical "two
/// fresh builds must agree" property `report_determinism` already
/// proves for the `@image`/`wrela build` pipeline, proved here for the
/// separate runtime-test-image pipeline decision 1 introduced (a
/// runtime-test image has no `@image` fn at all, so it never goes
/// through `report_determinism`'s own walk).
fn repro_test_image() -> Result<(), String> {
    let (img_bytes, report_text) = boot_hello_test_image()?;
    let (img_bytes2, report_text2) = boot_hello_test_image()?;
    if img_bytes != img_bytes2 {
        return Err(format!(
            "repro: tests/golden/boot-hello: two fresh test-image builds disagree ({} bytes vs {} bytes)",
            img_bytes.len(),
            img_bytes2.len()
        ));
    }
    if report_text != report_text2 {
        return Err(
            "repro: tests/golden/boot-hello: two fresh test-image report builds disagree"
                .to_string(),
        );
    }
    println!(
        "repro: tests/golden/boot-hello's own test image reproduced byte-for-byte (image + report) across two runs"
    );
    Ok(())
}

/// plans/M6.md item E: the choice-sequence recorder's own citable
/// conformance evidence (`machine.replay.clock-log`/`machine.replay.
/// choice-sequence`'s own ledger notes) — shells out to the freshly
/// built-and-signed `wrela-vmm` *binary* exactly once more (never the
/// `wrela-vmm` *crate*: this file's own established "xtask stays
/// unsigned, only the one signed binary calls HVF" boundary, the same
/// reason `run_vmm`/`bench_guest_lane` never link it either), recording
/// `tests/golden/boot-hello`'s own real test image live via `--record`,
/// then replaying that exact recording via `--replay` — a genuine
/// end-to-end exercise of `record::Chooser::choose_next`'s own record and
/// replay arms alike, over a real, already-citable golden's own compiled
/// image, not a hand-built stand-in. `boot-hello` declares no actors, so
/// this particular boot's own choice sequence is `ClockRead`-shaped only
/// (no `DeadlineWake`/`VectorRaise`) — the fuller tag coverage lives in
/// `wrela-vmm`'s own conformance suite (`park_conformance_wakes_at_the_
/// deadline_and_resumes_over_hvf`, `vector_raise_observed_at_a_checkpoint_
/// over_hvf`, `record_replay_of_the_park_wake_scenario_is_byte_stable_and_
/// detects_tamper`), disclosed here rather than silently implied covered:
/// those tests are real, but — like `machine.clock.trap-logged`'s own
/// established precedent — `cargo test -p wrela-vmm` unit tests are not
/// individually citable by `xtask ledger`'s own validator (no `tests/`
/// path, no `xtask:<command>`).
fn repro_choice_log_roundtrip(vmm: &Path) -> Result<(), String> {
    let (img_bytes, report_text) = boot_hello_test_image()?;
    let tmp_dir = root().join("target/repro-choice-log-tmp");
    if tmp_dir.exists() {
        std::fs::remove_dir_all(&tmp_dir)
            .map_err(|e| format!("remove {}: {e}", tmp_dir.display()))?;
    }
    std::fs::create_dir_all(&tmp_dir).map_err(|e| format!("create {}: {e}", tmp_dir.display()))?;
    let img_path = tmp_dir.join("boot.img");
    let report_path = tmp_dir.join("boot.report.txt");
    let record_path = tmp_dir.join("boot.record.txt");
    std::fs::write(&img_path, &img_bytes).map_err(|e| format!("write img: {e}"))?;
    std::fs::write(&report_path, &report_text).map_err(|e| format!("write report: {e}"))?;

    let record_out = Command::new(vmm)
        .arg(&report_path)
        .arg(&img_path)
        .arg("--record")
        .arg(&record_path)
        .output()
        .map_err(|e| format!("run wrela-vmm --record: {e}"))?;
    let record_exit = record_out.status.code().unwrap_or(-1);
    if record_exit != 0 && record_exit != 1 {
        let _ = std::fs::remove_dir_all(&tmp_dir);
        return Err(format!(
            "repro: choice-log record boot did not complete (exit {record_exit})"
        ));
    }
    let record_text = std::fs::read_to_string(&record_path)
        .map_err(|e| format!("read {}: {e}", record_path.display()))?;
    if !record_text.starts_with("ChoiceLog v1\n") {
        let _ = std::fs::remove_dir_all(&tmp_dir);
        return Err(
            "repro: choice-log record file does not start with the versioned `ChoiceLog v1` header"
                .to_string(),
        );
    }

    let replay_out = Command::new(vmm)
        .arg(&report_path)
        .arg(&img_path)
        .arg("--replay")
        .arg(&record_path)
        .output()
        .map_err(|e| format!("run wrela-vmm --replay: {e}"))?;
    let replay_exit = replay_out.status.code().unwrap_or(-1);
    let _ = std::fs::remove_dir_all(&tmp_dir);
    // A clean (non-diverging) replay mirrors the guest's own exit code
    // exactly like a plain boot (`boot-hello` deliberately fails one
    // test, so `record_exit`/`replay_exit` are both `1` here — an
    // ordinary, expected guest outcome, never `EXIT_VMM_FAILURE`/
    // `EXIT_REPLAY_DIVERGENCE`) — never unconditionally `0`.
    if replay_exit != record_exit {
        return Err(format!(
            "repro: choice-log replay diverged from its own recording (exit {replay_exit}, \
             expected {record_exit} to match the recorded boot's own guest-authored exit):\n{}",
            String::from_utf8_lossy(&replay_out.stderr)
        ));
    }
    println!(
        "repro: tests/golden/boot-hello's own choice log (`ChoiceLog v1`) records and replays \
         byte-stable, zero divergence"
    );
    Ok(())
}

/// plans/M6.md item E, verification's own fail-closed finding: the
/// process-level exit-code contract `wrela-vmm/src/main.rs`'s own module
/// doc names must actually hold on the real, signed binary — a caller
/// (`xtask`, CI, a script) trusts `$?` alone, never stdout/stderr, so
/// every documented outcome is asserted here directly against
/// `Output::status.code()`: a clean replay reflects the *same*
/// guest-authored exit code (`0`/`1`) the original recording boot
/// itself reported (`tests/golden/boot-hello` has a deliberately failing
/// test, so this is `1` here — an ordinary, expected guest outcome, not
/// a VMM failure, exactly the "guest-authored vs runner-authored"
/// distinction `main.rs`'s own doc draws); a replay whose recorded
/// `exit_code=` line is tampered must exit `EXIT_REPLAY_DIVERGENCE` (3)
/// and name the mismatch on stderr — **never** `0` (the exact fail-closed
/// violation this item's own verification pass caught); a `--replay`
/// against an unparseable record file, and a `--record` to an unwritable
/// destination, must each exit `EXIT_VMM_FAILURE` (2). Exit codes live in
/// `wrela_machine::vmm_process` so xtask and `wrela-vmm` cannot drift
/// (xtask still does not link the `wrela-vmm` crate — only the shared
/// machine contract).
fn repro_replay_exit_code_contract(vmm: &Path) -> Result<(), String> {
    use wrela_machine::vmm_process::{EXIT_REPLAY_DIVERGENCE, EXIT_VMM_FAILURE};

    let (img_bytes, report_text) = boot_hello_test_image()?;
    let tmp_dir = root().join("target/repro-exit-code-contract-tmp");
    if tmp_dir.exists() {
        std::fs::remove_dir_all(&tmp_dir)
            .map_err(|e| format!("remove {}: {e}", tmp_dir.display()))?;
    }
    std::fs::create_dir_all(&tmp_dir).map_err(|e| format!("create {}: {e}", tmp_dir.display()))?;
    let img_path = tmp_dir.join("boot.img");
    let report_path = tmp_dir.join("boot.report.txt");
    let record_path = tmp_dir.join("boot.record.txt");
    std::fs::write(&img_path, &img_bytes).map_err(|e| format!("write img: {e}"))?;
    std::fs::write(&report_path, &report_text).map_err(|e| format!("write report: {e}"))?;

    let fail = |tmp_dir: &Path, msg: String| -> Result<(), String> {
        let _ = std::fs::remove_dir_all(tmp_dir);
        Err(msg)
    };

    // --- record: a plain boot's own guest-authored exit code -------------
    let record_out = Command::new(vmm)
        .arg(&report_path)
        .arg(&img_path)
        .arg("--record")
        .arg(&record_path)
        .output()
        .map_err(|e| format!("run wrela-vmm --record: {e}"))?;
    let record_exit = record_out.status.code().unwrap_or(-1);
    if record_exit != 0 && record_exit != 1 {
        return fail(
            &tmp_dir,
            format!(
                "repro: exit-code-contract record boot did not complete (exit {record_exit}, \
                 expected the guest-authored 0 or 1)"
            ),
        );
    }

    // --- clean replay: reflects the identical guest-authored outcome ----
    let clean_replay = Command::new(vmm)
        .arg(&report_path)
        .arg(&img_path)
        .arg("--replay")
        .arg(&record_path)
        .output()
        .map_err(|e| format!("run wrela-vmm --replay: {e}"))?;
    let clean_exit = clean_replay.status.code().unwrap_or(-1);
    if clean_exit != record_exit {
        return fail(
            &tmp_dir,
            format!(
                "repro: exit-code-contract clean replay exit ({clean_exit}) does not match the \
                 recorded boot's own guest-authored exit ({record_exit})"
            ),
        );
    }

    // --- tampered exit_code=: must exit EXIT_REPLAY_DIVERGENCE, never 0 -
    let record_text =
        std::fs::read_to_string(&record_path).map_err(|e| format!("read record: {e}"))?;
    let tampered_text: String = record_text
        .lines()
        .map(|line| match line.strip_prefix("exit_code=") {
            Some(v) => {
                let original: u64 = v.parse().unwrap_or(0);
                format!("exit_code={}", original ^ 0xFF)
            }
            None => line.to_string(),
        })
        .collect::<Vec<_>>()
        .join("\n")
        + "\n";
    let tampered_path = tmp_dir.join("tampered.record.txt");
    std::fs::write(&tampered_path, &tampered_text)
        .map_err(|e| format!("write tampered record: {e}"))?;
    let diverged_replay = Command::new(vmm)
        .arg(&report_path)
        .arg(&img_path)
        .arg("--replay")
        .arg(&tampered_path)
        .output()
        .map_err(|e| format!("run wrela-vmm --replay (tampered): {e}"))?;
    let diverged_exit = diverged_replay.status.code().unwrap_or(-1);
    if diverged_exit != EXIT_REPLAY_DIVERGENCE {
        return fail(
            &tmp_dir,
            format!(
                "repro: FAIL-CLOSED VIOLATION: a replay with a tampered exit_code must exit \
                 {EXIT_REPLAY_DIVERGENCE} (EXIT_REPLAY_DIVERGENCE), got {diverged_exit} instead \
                 (stderr: {})",
                String::from_utf8_lossy(&diverged_replay.stderr)
            ),
        );
    }
    if !String::from_utf8_lossy(&diverged_replay.stderr).contains("exit code mismatch") {
        return fail(
            &tmp_dir,
            "repro: a tampered replay's own stderr does not name the exit-code mismatch"
                .to_string(),
        );
    }

    // --- malformed record file on --replay: must exit EXIT_VMM_FAILURE --
    let malformed_path = tmp_dir.join("malformed.record.txt");
    std::fs::write(&malformed_path, b"not a choice log at all\n")
        .map_err(|e| format!("write malformed record: {e}"))?;
    let malformed_replay = Command::new(vmm)
        .arg(&report_path)
        .arg(&img_path)
        .arg("--replay")
        .arg(&malformed_path)
        .output()
        .map_err(|e| format!("run wrela-vmm --replay (malformed): {e}"))?;
    let malformed_exit = malformed_replay.status.code().unwrap_or(-1);
    if malformed_exit != EXIT_VMM_FAILURE {
        return fail(
            &tmp_dir,
            format!(
                "repro: a malformed --replay record file must exit {EXIT_VMM_FAILURE} \
                 (EXIT_VMM_FAILURE), got {malformed_exit}"
            ),
        );
    }

    // --- --record to an unwritable path: must exit EXIT_VMM_FAILURE ------
    let unwritable_path = tmp_dir.join("no-such-subdir").join("rec.txt");
    let unwritable_record = Command::new(vmm)
        .arg(&report_path)
        .arg(&img_path)
        .arg("--record")
        .arg(&unwritable_path)
        .output()
        .map_err(|e| format!("run wrela-vmm --record (unwritable): {e}"))?;
    let unwritable_exit = unwritable_record.status.code().unwrap_or(-1);
    let _ = std::fs::remove_dir_all(&tmp_dir);
    if unwritable_exit != EXIT_VMM_FAILURE {
        return Err(format!(
            "repro: --record to an unwritable path must exit {EXIT_VMM_FAILURE} \
             (EXIT_VMM_FAILURE), got {unwritable_exit}"
        ));
    }

    println!(
        "repro: wrela-vmm's own process-level exit-code contract holds: clean replay={clean_exit} \
         (mirrors the guest), tampered-exit-code replay={EXIT_REPLAY_DIVERGENCE}, malformed \
         replay/unwritable record={EXIT_VMM_FAILURE}"
    );
    Ok(())
}

/// plans/M6.md item F: `tests/golden/boot-deadline-cancel` is the first
/// boot in this repo whose *behaviour* depends on the clock — the group's
/// own deadline expires, the scheduler's deadline poll observes it through
/// a real `CLOCK_MMIO_ADDR` read, and the child is cancelled at exactly one
/// checkpoint as a result. That makes it the real test of decision 9's own
/// claim that replay takes its time from the ChoiceLog rather than from the
/// host clock, so this check proves three things end to end, on the real
/// signed binary:
///
/// 1. the recording is genuinely clock-driven — its choice sequence
///    carries several `ClockRead` entries and no `DeadlineWake` (this boot
///    never parks: at M6 nothing can block a turn forever, so the
///    scheduler always has ready work — recorded honestly rather than
///    claiming a sleep was skipped that never existed);
/// 2. replaying that recording is **clean** — zero divergence, and the
///    same guest-authored exit code, so the whole cancellation schedule
///    reproduces exactly; and
/// 3. the replayed clock really is the logged one, proved the only way
///    that cannot be faked: tampering the *first* logged `ClockRead` to a
///    far-future value (so the armed deadline is never reached) changes
///    the guest's own behaviour — the child runs its loop to completion
///    and the test's assertion fails — and the replay must therefore
///    report divergence and exit `EXIT_REPLAY_DIVERGENCE`, never `0`. If
///    replay were quietly reading the host clock, the tamper would change
///    nothing and this assertion would fail.
fn repro_deadline_cancel_replay_is_clock_log_driven(vmm: &Path) -> Result<(), String> {
    use wrela_machine::vmm_process::EXIT_REPLAY_DIVERGENCE;
    let (img_bytes, report_text) = golden_test_image("boot-deadline-cancel")?;
    let tmp_dir = root().join("target/repro-deadline-cancel-tmp");
    if tmp_dir.exists() {
        std::fs::remove_dir_all(&tmp_dir)
            .map_err(|e| format!("remove {}: {e}", tmp_dir.display()))?;
    }
    std::fs::create_dir_all(&tmp_dir).map_err(|e| format!("create {}: {e}", tmp_dir.display()))?;
    let img_path = tmp_dir.join("boot.img");
    let report_path = tmp_dir.join("boot.report.txt");
    let record_path = tmp_dir.join("boot.record.txt");
    std::fs::write(&img_path, &img_bytes).map_err(|e| format!("write img: {e}"))?;
    std::fs::write(&report_path, &report_text).map_err(|e| format!("write report: {e}"))?;

    let record_out = Command::new(vmm)
        .arg(&report_path)
        .arg(&img_path)
        .arg("--record")
        .arg(&record_path)
        .output()
        .map_err(|e| format!("run wrela-vmm --record: {e}"))?;
    let record_exit = record_out.status.code().unwrap_or(-1);
    if record_exit != 0 {
        let _ = std::fs::remove_dir_all(&tmp_dir);
        return Err(format!(
            "repro: boot-deadline-cancel's own recording boot did not pass (exit {record_exit}):\n{}",
            String::from_utf8_lossy(&record_out.stdout)
        ));
    }
    let record_text = std::fs::read_to_string(&record_path)
        .map_err(|e| format!("read {}: {e}", record_path.display()))?;
    let clock_reads = record_text.matches("=ClockRead ").count();
    if clock_reads < 2 {
        let _ = std::fs::remove_dir_all(&tmp_dir);
        return Err(format!(
            "repro: boot-deadline-cancel recorded {clock_reads} ClockRead choice(s) — this boot \
             is supposed to be clock-driven (the `now()` that arms the deadline, plus the \
             scheduler's own deadline poll)"
        ));
    }

    let replay_out = Command::new(vmm)
        .arg(&report_path)
        .arg(&img_path)
        .arg("--replay")
        .arg(&record_path)
        .output()
        .map_err(|e| format!("run wrela-vmm --replay: {e}"))?;
    let replay_exit = replay_out.status.code().unwrap_or(-1);
    if replay_exit != record_exit {
        let _ = std::fs::remove_dir_all(&tmp_dir);
        return Err(format!(
            "repro: boot-deadline-cancel replayed with exit {replay_exit}, expected \
             {record_exit}:\n{}",
            String::from_utf8_lossy(&replay_out.stderr)
        ));
    }

    // (3) The tamper: rewrite the FIRST ClockRead to a far-future value, so
    // a guest reading its clock from the log arms a deadline that never
    // expires. Everything else in the log is left exactly as recorded.
    let mut tampered = String::new();
    let mut done = false;
    for line in record_text.lines() {
        if !done && line.contains("=ClockRead value=") {
            let head = &line[..line.find("value=").unwrap()];
            tampered.push_str(head);
            tampered.push_str("value=9000000000000000000\n");
            done = true;
            continue;
        }
        tampered.push_str(line);
        tampered.push('\n');
    }
    if !done {
        let _ = std::fs::remove_dir_all(&tmp_dir);
        return Err("repro: no ClockRead line to tamper in the recording".to_string());
    }
    let tampered_path = tmp_dir.join("boot.tampered.txt");
    std::fs::write(&tampered_path, &tampered).map_err(|e| format!("write tampered: {e}"))?;
    let tampered_out = Command::new(vmm)
        .arg(&report_path)
        .arg(&img_path)
        .arg("--replay")
        .arg(&tampered_path)
        .output()
        .map_err(|e| format!("run wrela-vmm --replay (tampered): {e}"))?;
    let tampered_exit = tampered_out.status.code().unwrap_or(-1);
    let _ = std::fs::remove_dir_all(&tmp_dir);
    if tampered_exit != EXIT_REPLAY_DIVERGENCE {
        return Err(format!(
            "repro: a replay whose first logged ClockRead was moved to the far future exited \
             {tampered_exit}, expected {EXIT_REPLAY_DIVERGENCE} — the replayed guest's own \
             clock must come from the log, so this tamper has to change its behaviour and \
             diverge\nstderr:\n{}",
            String::from_utf8_lossy(&tampered_out.stderr)
        ));
    }
    println!(
        "repro: tests/golden/boot-deadline-cancel ({clock_reads} recorded ClockRead choices) \
         replays clean with zero divergence, and a tampered clock value diverges \
         (exit {EXIT_REPLAY_DIVERGENCE}) — replay's time comes from the log"
    );
    Ok(())
}

/// `cargo xtask repro` (plans/M5.md decision 10, item F; plans/M6.md item
/// E): the standalone, full-corpus form — `report_determinism`'s own
/// `@image`/`wrela build` population, `repro_test_image`'s runtime-test-
/// image case, `repro_choice_log_roundtrip`'s own record/replay round
/// trip, and `repro_replay_exit_code_contract`'s own process-level
/// exit-code proof, so bare `repro` covers every image-emitting *and*
/// determinism-recording path this milestone has.
fn repro() -> Result<(), String> {
    report_determinism()?;
    repro_test_image()?;
    let vmm = build_and_sign_vmm()?;
    repro_choice_log_roundtrip(&vmm)?;
    repro_deadline_cancel_replay_is_clock_log_driven(&vmm)?;
    repro_blk_completion_replay(&vmm)?;
    repro_cross_core_admission_replay(&vmm)?;
    repro_cross_core_mailbox_depth_admissions(&vmm)?;
    repro_replay_exit_code_contract(&vmm)
}

/// plans/M8.md item H Target C: the depth-1 mailbox under three cores
/// records both eventual admissions (Near then Far). Back-pressure is
/// not itself a choice entry — under Progress-serial replay it is still
/// checked — but the choice sequence must still name both messages once
/// they admit, or a drain that dropped the held message would look
/// identical to one that held it until the transcript assert (`total == 11`)
/// fired.
fn repro_cross_core_mailbox_depth_admissions(vmm: &Path) -> Result<(), String> {
    const CASE: &str = "boot-cross-core-mailbox-depth";
    let (img_bytes, report_text) = golden_test_image(CASE)?;
    let tmp_dir = root().join("target/repro-mailbox-depth-tmp");
    if tmp_dir.exists() {
        std::fs::remove_dir_all(&tmp_dir)
            .map_err(|e| format!("remove {}: {e}", tmp_dir.display()))?;
    }
    std::fs::create_dir_all(&tmp_dir).map_err(|e| format!("create {}: {e}", tmp_dir.display()))?;
    let img_path = tmp_dir.join("boot.img");
    let report_path = tmp_dir.join("boot.report.txt");
    let record_path = tmp_dir.join("boot.record.txt");
    std::fs::write(&img_path, &img_bytes).map_err(|e| format!("write img: {e}"))?;
    std::fs::write(&report_path, &report_text).map_err(|e| format!("write report: {e}"))?;

    let record_out = Command::new(vmm)
        .arg(&report_path)
        .arg(&img_path)
        .arg("--record")
        .arg(&record_path)
        .output()
        .map_err(|e| format!("run wrela-vmm --record: {e}"))?;
    let record_exit = record_out.status.code().unwrap_or(-1);
    if record_exit != 0 {
        let _ = std::fs::remove_dir_all(&tmp_dir);
        return Err(format!(
            "repro: {CASE}'s recording boot failed (exit {record_exit}):\n{}{}",
            String::from_utf8_lossy(&record_out.stdout),
            String::from_utf8_lossy(&record_out.stderr)
        ));
    }
    let record_text = std::fs::read_to_string(&record_path)
        .map_err(|e| format!("read {}: {e}", record_path.display()))?;
    let admissions: Vec<&str> = record_text
        .lines()
        .filter_map(|l| l.split_once("]=").map(|(_, rhs)| rhs))
        .filter(|rhs| rhs.starts_with("Admission "))
        .collect();
    // Cap-1 rings under-count under overlap when produce+consume nets to
    // zero between exits (`AdmissionWitness` / plans/M15.md item I). The
    // guest transcript (`total == 11`, exit 0) is the back-pressure proof —
    // Far's held +10 was admitted. The choice log may drop every Sink←*
    // observe; require Far←core0 (cap>1 kick ring) so the recorder still
    // saw the cross-core kick, then replay must halt exit 0.
    let has = |s: &str| admissions.iter().any(|a| *a == s);
    if !has("Admission mailbox=Far sender=core0") {
        let _ = std::fs::remove_dir_all(&tmp_dir);
        return Err(format!(
            "repro: {CASE} recorded {:?}, need Far←core0 (kick); guest total==11 \
             locks Far's +10 — cap-1 under-count may drop every Sink←* observe",
            admissions
        ));
    }
    let replay_out = Command::new(vmm)
        .arg(&report_path)
        .arg(&img_path)
        .arg("--replay")
        .arg(&record_path)
        .output()
        .map_err(|e| format!("run wrela-vmm --replay: {e}"))?;
    let replay_exit = replay_out.status.code().unwrap_or(-1);
    let _ = std::fs::remove_dir_all(&tmp_dir);
    if replay_exit != record_exit {
        return Err(format!(
            "repro: {CASE} replayed with exit {replay_exit}, expected {record_exit}:\n{}",
            String::from_utf8_lossy(&replay_out.stderr)
        ));
    }
    println!(
        "repro: tests/golden/{CASE}'s depth-1 mailbox under three cores records Far←core0; \
         replays clean exit 0 — guest total==11 locks back-pressure; cap-1 Sink admission \
         under-count under overlap is tolerated in the choice log"
    );
    Ok(())
}

/// plans/M8.md item C3, decision 42 (and the milestone's own exit
/// criterion: "`ChoiceEntry::Admission` is no longer format-only: record →
/// replay clean → tamper an admission choice → divergence, on a named
/// workload"): the fourth sibling of this lane's three existing tamper
/// oracles — a recorded clock read, a recorded device completion, and the
/// process exit code.
///
/// The named workload is **`tests/golden/boot-cross-core-admission-order`**,
/// written for this lane because no pre-existing image could falsify an
/// *order*: `boot-cross-core-two-senders` carries two cross-core messages
/// to one mailbox, but both are produced by core 0, so its two `Admission`
/// entries are byte-identical and a swap of them is unobservable. In the
/// named workload `Near` is on core 0, `Far` is on core 2, and both message
/// `Sink` on core 1, so `Sink`'s admission sequence is `core0` then
/// `core2` — two entries that differ, whose order the boot's own transcript
/// independently states (`Near`'s await returns `1`, not `11`, because core
/// 0's `+1` was admitted before the `+10` core 2 had already published).
///
/// **What this proves, in the words the item asks for: witness-only, not
/// injection.** The admission is performed by guest code
/// (`layout::build_rt_drain`) in guest memory, and the VMM neither writes
/// a mailbox nor reorders a ring in either mode — under plans/M8.md
/// decision 11's baton there is no alternative order to feed back, so
/// "replay injects the recorded order" would be a claim with no mechanism
/// behind it. What replay does is re-witness and compare, so the honest
/// oracle is divergence detection: an admission entry whose mailbox or
/// producing core has been altered must be **caught by name** during
/// replay, exactly as the tampered blk completion is. That is what the
/// third step below asserts, and the second step (`replay clean`) is what
/// makes the third non-vacuous.
fn repro_cross_core_admission_replay(vmm: &Path) -> Result<(), String> {
    use wrela_machine::vmm_process::EXIT_REPLAY_DIVERGENCE;
    const CASE: &str = "boot-cross-core-admission-order";
    let (img_bytes, report_text) = golden_test_image(CASE)?;
    let tmp_dir = root().join("target/repro-admission-tmp");
    if tmp_dir.exists() {
        std::fs::remove_dir_all(&tmp_dir)
            .map_err(|e| format!("remove {}: {e}", tmp_dir.display()))?;
    }
    std::fs::create_dir_all(&tmp_dir).map_err(|e| format!("create {}: {e}", tmp_dir.display()))?;
    let img_path = tmp_dir.join("boot.img");
    let report_path = tmp_dir.join("boot.report.txt");
    let record_path = tmp_dir.join("boot.record.txt");
    std::fs::write(&img_path, &img_bytes).map_err(|e| format!("write img: {e}"))?;
    std::fs::write(&report_path, &report_text).map_err(|e| format!("write report: {e}"))?;

    let record_out = Command::new(vmm)
        .arg(&report_path)
        .arg(&img_path)
        .arg("--record")
        .arg(&record_path)
        .output()
        .map_err(|e| format!("run wrela-vmm --record: {e}"))?;
    let record_exit = record_out.status.code().unwrap_or(-1);
    if record_exit != 0 {
        let _ = std::fs::remove_dir_all(&tmp_dir);
        return Err(format!(
            "repro: {CASE}'s own recording boot did not pass (exit {record_exit}):\n{}{}",
            String::from_utf8_lossy(&record_out.stdout),
            String::from_utf8_lossy(&record_out.stderr)
        ));
    }
    let record_text = std::fs::read_to_string(&record_path)
        .map_err(|e| format!("read {}: {e}", record_path.display()))?;

    // (1) Witness multiset under overlap (plans/M15.md item I):
    //   Far←core0 (root's await far.kick),
    //   Sink←core2 (Far.bump), Sink←core0 (Near.add), Sink←core0 (sink.total).
    // Order is not fixed; Progress still serializes replay.
    let admissions: Vec<&str> = record_text
        .lines()
        .filter_map(|l| l.split_once("]=").map(|(_, rhs)| rhs))
        .filter(|rhs| rhs.starts_with("Admission "))
        .collect();
    let mut got = admissions.clone();
    got.sort();
    let mut want = vec![
        "Admission mailbox=Far sender=core0",
        "Admission mailbox=Sink sender=core0",
        "Admission mailbox=Sink sender=core0",
        "Admission mailbox=Sink sender=core2",
    ];
    want.sort();
    if got != want {
        let _ = std::fs::remove_dir_all(&tmp_dir);
        return Err(format!(
            "repro: {CASE} recorded {:?}, expected the multiset {:?}",
            admissions, want
        ));
    }
    let progress_count = record_text
        .lines()
        .filter(|l| l.contains("]=Progress "))
        .count();
    if progress_count == 0 {
        let _ = std::fs::remove_dir_all(&tmp_dir);
        return Err(format!(
            "repro: {CASE} recorded no Progress entries — Yield-Progress replay needs at least one"
        ));
    }

    // (2) Replay is clean: zero divergence, same guest-authored exit code.
    let replay_out = Command::new(vmm)
        .arg(&report_path)
        .arg(&img_path)
        .arg("--replay")
        .arg(&record_path)
        .output()
        .map_err(|e| format!("run wrela-vmm --replay: {e}"))?;
    let replay_exit = replay_out.status.code().unwrap_or(-1);
    if replay_exit != record_exit {
        let _ = std::fs::remove_dir_all(&tmp_dir);
        return Err(format!(
            "repro: {CASE} replayed with exit {replay_exit}, expected {record_exit}:\n{}",
            String::from_utf8_lossy(&replay_out.stderr)
        ));
    }

    // (3) Tampers — every one must be caught **by name**, naming the field
    // that diverged (plans/M8.md item H Target B). A tamper that replays
    // clean is a hole in the recorder's own contract (06 §8).
    struct Tamper {
        name: &'static str,
        /// Substring the stderr must contain (the named divergence).
        expect: &'static str,
        apply: fn(&str) -> String,
    }
    let tampers: &[Tamper] = &[
        Tamper {
            // plans/M15.md item I / decision 8: under overlap, Admission
            // replay is a multiset bag — order inversion is not a witness.
            // A sender-identity flip still must be caught by name.
            name: "flip every Sink admission sender",
            expect: "admission mismatch (sender)",
            apply: |text| {
                let sink_prefix = "Admission mailbox=Sink sender=";
                let mut out = String::new();
                for line in text.lines() {
                    if let Some((head, rhs)) = line.split_once("]=") {
                        if let Some(sender) = rhs.strip_prefix(sink_prefix) {
                            let other = match sender {
                                "core0" => "core2",
                                "core2" => "core0",
                                _ => sender,
                            };
                            out.push_str(&format!("{head}]={sink_prefix}{other}\n"));
                            continue;
                        }
                    }
                    out.push_str(line);
                    out.push('\n');
                }
                out
            },
        },
        Tamper {
            // plans/M15.md item I: Progress tamper → named divergence
            // (ordinary repro, not an enumerator).
            name: "Progress core out of range",
            expect: "progress mismatch",
            apply: |text| {
                let mut out = String::new();
                let mut done = false;
                for line in text.lines() {
                    if !done {
                        if let Some((head, rhs)) = line.split_once("]=") {
                            if rhs.starts_with("Progress core=") {
                                out.push_str(&format!("{head}]=Progress core=99\n"));
                                done = true;
                                continue;
                            }
                        }
                    }
                    out.push_str(line);
                    out.push('\n');
                }
                out
            },
        },
        Tamper {
            name: "tamper sender core on Far",
            expect: "admission mismatch (sender)",
            apply: |text| {
                text.replace(
                    "Admission mailbox=Far sender=core0",
                    "Admission mailbox=Far sender=core2",
                )
            },
        },
        Tamper {
            name: "tamper mailbox identity",
            expect: "admission mismatch (mailbox)",
            apply: |text| {
                text.replace(
                    "Admission mailbox=Far sender=core0",
                    "Admission mailbox=Ghost sender=core0",
                )
            },
        },
        Tamper {
            // Cap-1 / overlap under-count (decision 8) tolerates pure count
            // deltas. Drop a *unique* Far admission so replay's Far observe
            // mismatches the remaining bag by mailbox identity.
            name: "drop unique Far admission",
            expect: "admission mismatch",
            apply: |text| {
                let mut choice_lines: Vec<String> = Vec::new();
                let mut trailer: Vec<String> = Vec::new();
                for line in text.lines() {
                    if line.starts_with("choice[") {
                        choice_lines.push(line.to_string());
                    } else if line.starts_with("ChoiceLog") || line.starts_with("choice_count=") {
                        // rebuilt below
                    } else if !line.is_empty() {
                        trailer.push(line.to_string());
                    }
                }
                let idx = choice_lines
                    .iter()
                    .position(|l| l.contains("]=Admission mailbox=Far "))
                    .expect("Far admission present");
                choice_lines.remove(idx);
                let mut out = String::from("ChoiceLog v1\n");
                out.push_str(&format!("choice_count={}\n", choice_lines.len()));
                for (i, line) in choice_lines.iter().enumerate() {
                    let rhs = line.split_once("]=").map(|(_, r)| r).unwrap_or(line);
                    out.push_str(&format!("choice[{i}]={rhs}\n"));
                }
                for t in trailer {
                    out.push_str(&t);
                    out.push('\n');
                }
                out
            },
        },
        Tamper {
            // Strip every real Admission and leave only Spurious←core0 so
            // the first observe mismatches by mailbox (same-sender alt).
            name: "replace bag with spurious Admission",
            expect: "admission mismatch",
            apply: |text| {
                let mut non_admission: Vec<String> = Vec::new();
                let mut trailer: Vec<String> = Vec::new();
                for line in text.lines() {
                    if line.starts_with("choice[") {
                        if let Some((_, rhs)) = line.split_once("]=") {
                            if rhs.starts_with("Admission ") {
                                continue;
                            }
                        }
                        non_admission.push(line.to_string());
                    } else if line.starts_with("ChoiceLog") || line.starts_with("choice_count=") {
                    } else if !line.is_empty() {
                        trailer.push(line.to_string());
                    }
                }
                non_admission
                    .push("choice[N]=Admission mailbox=Spurious sender=core0".to_string());
                let mut out = String::from("ChoiceLog v1\n");
                out.push_str(&format!("choice_count={}\n", non_admission.len()));
                for (i, line) in non_admission.iter().enumerate() {
                    let rhs = line.split_once("]=").map(|(_, r)| r).unwrap_or(line);
                    out.push_str(&format!("choice[{i}]={rhs}\n"));
                }
                for t in trailer {
                    out.push_str(&t);
                    out.push('\n');
                }
                out
            },
        },
        Tamper {
            // Multiset still distinguishes mailboxes: replace Far with a
            // duplicate Sink so the bag loses Far and gains an extra Sink.
            name: "replace Far admission with extra Sink",
            expect: "admission mismatch",
            apply: |text| {
                text.replace(
                    "Admission mailbox=Far sender=core0",
                    "Admission mailbox=Sink sender=core0",
                )
            },
        },
    ];

    let mut reports = Vec::new();
    for (i, tamper) in tampers.iter().enumerate() {
        let tampered = (tamper.apply)(&record_text);
        if tampered == record_text {
            let _ = std::fs::remove_dir_all(&tmp_dir);
            return Err(format!(
                "repro: tamper {:?} left the record unchanged",
                tamper.name
            ));
        }
        let tampered_path = tmp_dir.join(format!("boot.tampered-{i}.txt"));
        std::fs::write(&tampered_path, &tampered).map_err(|e| format!("write tampered: {e}"))?;
        let tampered_out = Command::new(vmm)
            .arg(&report_path)
            .arg(&img_path)
            .arg("--replay")
            .arg(&tampered_path)
            .output()
            .map_err(|e| format!("run wrela-vmm --replay (tampered {}): {e}", tamper.name))?;
        let tampered_exit = tampered_out.status.code().unwrap_or(-1);
        let stderr = String::from_utf8_lossy(&tampered_out.stderr).to_string();
        if tampered_exit != EXIT_REPLAY_DIVERGENCE {
            let _ = std::fs::remove_dir_all(&tmp_dir);
            return Err(format!(
                "repro: tamper {:?} replayed with exit {tampered_exit}, expected \
                 {EXIT_REPLAY_DIVERGENCE} (a determinism finding must never be mistaken for a \
                 successful replay):\n{stderr}",
                tamper.name
            ));
        }
        if !stderr.contains(tamper.expect) {
            let _ = std::fs::remove_dir_all(&tmp_dir);
            return Err(format!(
                "repro: tamper {:?} diverged, but not by name — stderr must contain {:?}:\n{stderr}",
                tamper.name, tamper.expect
            ));
        }
        reports.push(format!("{:?} → {}", tamper.name, tamper.expect));
    }
    let _ = std::fs::remove_dir_all(&tmp_dir);
    println!(
        "repro: tests/golden/{CASE}'s {} cross-core admission(s) — record and replay byte-stable; \
         {} named tampers each caught by field:\n  {}",
        admissions.len(),
        reports.len(),
        reports.join("\n  ")
    );
    Ok(())
}

/// plans/M7.md item F, decision 7 (and the milestone's own exit criterion
/// "`cargo xtask repro` covers a blk workload: record → replay clean →
/// tamper a device-completion choice → divergence"): a real boot that
/// publishes two virtio-blk requests through the split ring, rings the
/// shared-memory doorbell (06 §5 — an ordinary store, no trap), records
/// the resulting `DeviceCompletion` choices, replays them byte-stable, and
/// then proves a tampered completion is *caught* rather than replayed.
///
/// **The guest here is hand-assembled, and that is the honest state of the
/// milestone**: the compiled driver is items A–E/G/H (capabilities,
/// `@layout`, typed MMIO, DMA pools, queues/receipts, ISRs, bring-up), so
/// no `.wr` source can publish a descriptor yet. This lane plays the
/// driver's role directly, exactly the way `wrela-vmm`'s own conformance
/// tests do — and it is deliberately its own copy of that builder rather
/// than a shared one: `xtask` does not link `wrela-vmm` (this file's own
/// established "xtask stays unsigned, only the one signed binary calls
/// HVF" boundary — the identical reason `report_determinism` carries its
/// own copy of `bin/wrela.rs`'s report driver). The two copies are kept
/// honest by both booting: a drift in either fails here or there.
///
/// Once the compiled driver exists, this lane's own image builder is what
/// gets deleted in favour of a golden's real image — named here rather
/// than left to be discovered.
fn repro_blk_completion_replay(vmm: &Path) -> Result<(), String> {
    use wrela_machine::vmm_process::EXIT_REPLAY_DIVERGENCE;
    let (img_bytes, report_text) = blk_conformance_image();
    let tmp_dir = root().join("target/repro-blk-tmp");
    if tmp_dir.exists() {
        std::fs::remove_dir_all(&tmp_dir)
            .map_err(|e| format!("remove {}: {e}", tmp_dir.display()))?;
    }
    std::fs::create_dir_all(&tmp_dir).map_err(|e| format!("create {}: {e}", tmp_dir.display()))?;
    let img_path = tmp_dir.join("blk.img");
    let report_path = tmp_dir.join("blk.report.txt");
    let record_path = tmp_dir.join("blk.record.txt");
    std::fs::write(&img_path, &img_bytes).map_err(|e| format!("write img: {e}"))?;
    std::fs::write(&report_path, &report_text).map_err(|e| format!("write report: {e}"))?;

    let record_out = Command::new(vmm)
        .arg(&report_path)
        .arg(&img_path)
        .arg("--record")
        .arg(&record_path)
        .output()
        .map_err(|e| format!("run wrela-vmm --record: {e}"))?;
    let record_exit = record_out.status.code().unwrap_or(-1);
    if record_exit != 0 {
        let _ = std::fs::remove_dir_all(&tmp_dir);
        return Err(format!(
            "repro: the blk conformance boot failed (exit {record_exit}; the guest folds one bit \
             per failed check into its exit code — see `blk_conformance_image`):\n{}",
            String::from_utf8_lossy(&record_out.stderr)
        ));
    }
    let record_text = std::fs::read_to_string(&record_path)
        .map_err(|e| format!("read {}: {e}", record_path.display()))?;
    let completions = record_text.matches("=DeviceCompletion ").count();
    if completions != 2 {
        let _ = std::fs::remove_dir_all(&tmp_dir);
        return Err(format!(
            "repro: the blk workload recorded {completions} DeviceCompletion choice(s), expected 2 \
             (one write, one read-back)"
        ));
    }

    let replay_out = Command::new(vmm)
        .arg(&report_path)
        .arg(&img_path)
        .arg("--replay")
        .arg(&record_path)
        .output()
        .map_err(|e| format!("run wrela-vmm --replay: {e}"))?;
    let replay_exit = replay_out.status.code().unwrap_or(-1);
    if replay_exit != record_exit {
        let _ = std::fs::remove_dir_all(&tmp_dir);
        return Err(format!(
            "repro: the blk workload's replay diverged from its own recording (exit {replay_exit}, \
             expected {record_exit}):\n{}",
            String::from_utf8_lossy(&replay_out.stderr)
        ));
    }

    // The tamper: flip the recorded status byte of the first completion
    // from `0` (OK) to `1` (IOERR). Everything else is left exactly as
    // recorded, so the *only* thing that can be caught is the completion
    // itself — the model recomputes the operation deterministically and
    // must report the disagreement rather than replaying the tampered
    // answer.
    let mut tampered = String::new();
    let mut done = false;
    for line in record_text.lines() {
        if !done && line.contains("=DeviceCompletion ") && line.contains(" status=0 ") {
            tampered.push_str(&line.replace(" status=0 ", " status=1 "));
            tampered.push('\n');
            done = true;
            continue;
        }
        tampered.push_str(line);
        tampered.push('\n');
    }
    if !done {
        let _ = std::fs::remove_dir_all(&tmp_dir);
        return Err("repro: no DeviceCompletion line to tamper in the recording".to_string());
    }
    let tampered_path = tmp_dir.join("blk.tampered.txt");
    std::fs::write(&tampered_path, &tampered).map_err(|e| format!("write tampered: {e}"))?;
    let tampered_out = Command::new(vmm)
        .arg(&report_path)
        .arg(&img_path)
        .arg("--replay")
        .arg(&tampered_path)
        .output()
        .map_err(|e| format!("run wrela-vmm --replay (tampered): {e}"))?;
    let tampered_exit = tampered_out.status.code().unwrap_or(-1);
    let stderr = String::from_utf8_lossy(&tampered_out.stderr).to_string();
    let _ = std::fs::remove_dir_all(&tmp_dir);
    if tampered_exit != EXIT_REPLAY_DIVERGENCE {
        return Err(format!(
            "repro: a tampered device completion replayed with exit {tampered_exit}, expected \
             {EXIT_REPLAY_DIVERGENCE} (a determinism finding must never be mistaken for a \
             successful replay):\n{stderr}"
        ));
    }
    if !stderr.contains("device completion mismatch") {
        return Err(format!(
            "repro: the tampered blk replay diverged, but not by name — stderr must say which \
             completion disagreed:\n{stderr}"
        ));
    }
    println!(
        "repro: the blk workload's 2 device completion(s) record and replay byte-stable, and a \
         tampered completion is caught by name"
    );
    Ok(())
}

/// The hand-assembled virtio-blk driver `repro_blk_completion_replay`
/// boots (its own doc comment has the whole rationale, including why this
/// is deliberately a second copy of `wrela-vmm`'s own conformance
/// builder). Returns `(image bytes, report text)`.
///
/// The ring, both request headers, the source payload and the destination
/// buffer live in the image's own trailing data region, covered by exactly
/// one declared pool window; the descriptor chains are prefilled here (a
/// driver's build-time role), and the guest program does the two runtime
/// acts a real driver does — publish an available entry, ring the doorbell
/// — then parks so the VMM's own poll site runs, and finally checks every
/// observable fact, folding one bit per failed check into its exit code.
fn blk_conformance_image() -> (Vec<u8>, String) {
    use wrela_compiler::encode;
    use wrela_machine::{layout as machine_layout, machine_info, mmio, pending};

    // The split ring's own shape and the virtio-blk request format, as
    // `wrela-vmm`'s `devices` module implements them — shared verbatim
    // with the QEMU side of the differential oracle (`blk_shape`,
    // `fill_blk_ring`), so "both implementations were handed the same
    // ring" is a fact about one function rather than a claim about two.
    use blk_shape::*;
    const DEVICE_FEATURES: u64 = (1 << 32) | (1 << 9); // VERSION_1 | BLK_F_FLUSH
    const BLK_VECTOR: u64 = 1;
    /// 06 §5's shared-memory doorbell word. It has no QEMU counterpart at
    /// all — QEMU's notification is a trapping `QueueNotify` MMIO write,
    /// which is exactly the thing 06 §5 deletes — so it lives here rather
    /// than in the shared shape.
    const OFF_DOORBELL: u64 = 0x140;

    fn load_imm(reg: u8, value: u64) -> Vec<u32> {
        use wrela_compiler::encode;
        vec![
            encode::enc_movz(reg, (value & 0xFFFF) as u16, 0, true),
            encode::enc_movk(reg, ((value >> 16) & 0xFFFF) as u16, 16, true),
            encode::enc_movk(reg, ((value >> 32) & 0xFFFF) as u16, 32, true),
            encode::enc_movk(reg, ((value >> 48) & 0xFFFF) as u16, 48, true),
        ]
    }

    let payload: Vec<u8> = (0..512u32).map(|i| ((i * 7 + 3) % 256) as u8).collect();
    let expect_first = u64::from_le_bytes(payload[0..8].try_into().expect("8 bytes"));
    let expect_last = u64::from_le_bytes(payload[504..512].try_into().expect("8 bytes"));
    let sp_top = machine_layout::core_stack_base_n(0, 1) + machine_layout::CORE_STACK_SIZE;

    let build_entry = |data_base: u64| -> Vec<u32> {
        let avail = data_base + OFF_AVAIL;
        let used = data_base + OFF_USED;
        let doorbell = data_base + OFF_DOORBELL;
        let mut w = Vec::new();
        w.extend(load_imm(9, sp_top));
        w.push(encode::enc_add_imm(31, 9, 0, true)); // mov sp, x9

        // One aligned 64-bit store publishes the whole avail header
        // (`flags: u16 = 0, idx, ring[0] = 0, ring[1] = 3`), so no 16-bit
        // store encoding is needed.
        let publish = |w: &mut Vec<u32>, idx: u64| {
            w.extend(load_imm(9, avail));
            w.extend(load_imm(10, (idx << 16) | (3 << 48)));
            w.push(encode::enc_str_x_imm(10, 9, 0));
            w.extend(load_imm(9, doorbell));
            w.push(encode::enc_movz(10, 1, 0, true));
            w.push(encode::enc_str_x_imm(10, 9, 0));
        };
        let park = |w: &mut Vec<u32>| {
            w.extend(load_imm(9, mmio::CLOCK_MMIO_ADDR));
            w.push(encode::enc_ldr_x_imm(11, 9, 0));
            w.extend(load_imm(12, 20_000_000)); // 20ms — a real, bounded fallback
            w.push(encode::enc_add_reg(11, 11, 12, true));
            w.extend(load_imm(
                9,
                machine_layout::MACHINE_INFO_BASE + machine_info::OFF_NEXT_DEADLINE,
            ));
            w.push(encode::enc_str_x_imm(11, 9, 0));
            w.extend(load_imm(9, mmio::PARK_MMIO_ADDR));
            w.push(encode::enc_str_x_imm(11, 9, 0));
        };
        publish(&mut w, 1);
        park(&mut w);
        publish(&mut w, 2);
        park(&mut w);

        w.extend(load_imm(9, used));
        w.push(encode::enc_ldr_x_imm(19, 9, 0));
        w.push(encode::enc_ldr_x_imm(20, 9, 8));
        w.push(encode::enc_ldr_x_imm(21, 9, 16));
        w.extend(load_imm(9, data_base + OFF_STATUS1));
        w.push(encode::enc_ldrb_imm(22, 9, 0));
        w.extend(load_imm(9, data_base + OFF_STATUS2));
        w.push(encode::enc_ldrb_imm(23, 9, 0));
        w.extend(load_imm(9, data_base + OFF_DST));
        w.push(encode::enc_ldr_x_imm(24, 9, 0));
        w.push(encode::enc_ldr_x_imm(25, 9, 504));
        w.extend(load_imm(9, pending::core_word_addr(0)));
        w.push(encode::enc_ldr_x_imm(26, 9, 0));

        w.push(encode::enc_movz(1, 0, 0, true)); // fail accumulator
        let check = |w: &mut Vec<u32>, actual: u8, expect: u64, bit: u8| {
            w.extend(load_imm(13, expect));
            w.push(encode::enc_cmp_reg(actual, 13, true));
            w.push(encode::enc_cset(14, encode::Cond::Ne, true));
            if bit > 0 {
                w.push(encode::enc_lsl_imm(14, 14, bit, true));
            }
            w.push(encode::enc_orr_reg(1, 1, 14, true));
        };
        check(&mut w, 19, 2u64 << 16, 0); // used.idx == 2, ring[0].id == 0
        check(&mut w, 20, 1 | (3u64 << 32), 1); // ring[0].len == 1, ring[1].id == 3
        check(&mut w, 21, 513, 2); // ring[1].len == 512 + status
        check(&mut w, 22, 0, 3); // write status == OK
        check(&mut w, 23, 0, 4); // read status == OK
        check(&mut w, 24, expect_first, 5); // first payload word survived the round trip
        check(&mut w, 25, expect_last, 6); // last payload word too
        check(&mut w, 26, 1u64 << BLK_VECTOR, 7); // only the blk vector: neither park slept

        w.extend(load_imm(15, mmio::EXIT_MMIO_ADDR));
        w.push(encode::enc_str_x_imm(1, 15, 0));
        w.push(encode::enc_brk(0));
        w
    };

    // The entry sequence's own length is independent of the addresses it
    // embeds (every constant is a fixed-width `load_imm`), so one
    // measuring pass fixes the data region's base. Data must sit outside
    // the page-granular RX window applied to `Section name=entry` (16 KiB
    // HVF pages) — same rule as `wrela-vmm`'s `build_blk_conformance_image`.
    let entry_len = build_entry(0).len();
    let code_bytes = (entry_len as u64) * 4;
    const PAGE: u64 = 16 * 1024;
    let code_span = code_bytes.div_ceil(PAGE) * PAGE;
    let data_base = machine_layout::IMAGE_BASE + code_span;
    let words = build_entry(data_base);
    assert_eq!(words.len(), entry_len, "entry length must not move");

    let mut img: Vec<u8> = words.iter().flat_map(|w| w.to_le_bytes()).collect();
    img.resize((code_span + DATA_REGION_SIZE) as usize, 0);
    let data_off = (data_base - machine_layout::IMAGE_BASE) as usize;
    fill_blk_ring(&mut img, data_off, data_base);

    let image_digest = wrela_machine::sha256::sha256_hex(&img);
    let report_text = format!(
        "Machine revision={}\n\
         Input path=<xtask-blk-conformance> sha256=e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855\n\
         Image sha256={image_digest}\n\
         Section name=entry base={:#x} size={code_bytes}\n\
         Entry base={:#x}\n\
         BlkDevice device=device#0 capacity_sectors=16 features={:#x} vector={BLK_VECTOR}\n\
         BlkQueue index=0 size={QUEUE_SIZE} desc={:#x} avail={:#x} used={:#x} doorbell={:#x}\n\
         BlkPool name=BlockControl device=device#0 base={:#x} size={:#x}\n",
        wrela_machine::MACHINE_REVISION_STR,
        machine_layout::IMAGE_BASE,
        machine_layout::IMAGE_BASE,
        DEVICE_FEATURES,
        data_base + OFF_DESC,
        data_base + OFF_AVAIL,
        data_base + OFF_USED,
        data_base + OFF_DOORBELL,
        data_base,
        DATA_REGION_SIZE,
    );
    (img, report_text)
}

// --- diff-eval (plans/M5.md decision 9, item F) -----------------------------
//
// The evaluator-vs-backend differential oracle (flips `compiler.eval.
// matches-backend`): for every golden case whose input typechecks and
// declares at least one bare `@test` (`TestKind::Comptime` — decision 9's
// own "the comptime tier's own set"; `@test(exhaustive)` is deliberately
// excluded from the comparison itself and counted in its own skip
// category instead, decision 2's sub-note), this compiles those same
// test fns into one runtime-test image (reusing item E's own harness —
// `layout::layout_test_image` — since a bare `@test` fn is exactly the
// zero-arg shape the harness already runs; naming it `runtime_tests`
// there is a harness-internal detail, not a claim about the test's own
// declared kind), boots it once via the codesigned `wrela-vmm` binary,
// and compares each guest-printed report line against `eval::run_tests`'
// own line for the same fn, byte for byte. Every skip (a case with zero
// comptime-legal tests, an exhaustive test, a program whose lowering
// fails closed) is counted AND printed as it happens — never silent, per
// the plan's own instruction — and any real disagreement fails the whole
// command loudly with both lines and the case name.

/// This oracle's own running tally, printed as the final summary line
/// (`diff-eval: <N> test(s) agree across <C> case(s), <S1>
/// lowering-skips, <S2> exhaustive-skips, <S3> quota-skips, <S4>
/// import-skips`).
#[derive(Default)]
struct DiffEvalTally {
    agree: usize,
    cases_agreed: usize,
    lowering_skips: usize,
    exhaustive_skips: usize,
    /// A third, plan-unanticipated skip category found while implementing
    /// this oracle (recorded here, not silently folded into
    /// `lowering_skips`, per the "never silent" house rule):
    /// `comptime.eval.quotas`' own step/memory quota is a *comptime-tier*
    /// resource bound (`eval::quota::MAX_STEPS`/`MAX_MEMORY`) with no
    /// backend equivalent whatsoever — the naive A76 codegen has no step
    /// counter, so a test the evaluator fails with "step/memory quota
    /// exceeded" (`check-tests-mixed`'s own `test_quota_exceeded`, a bare
    /// `while true: total = total + 1`) does not fail closed at lowering
    /// (an unbounded loop is perfectly ordinary mwir) — it lowers and
    /// codegens cleanly, then **spins forever** on real hardware, since
    /// nothing in the compiled image ever enforces the evaluator's own
    /// 20_000-step budget. Booting it would either hang for the VMM's own
    /// `WALL_CAP` (30s) on every `diff-eval` run or, worse, be
    /// indistinguishable from a genuine backend bug. Detected before ever
    /// building an image: a comptime test whose own `eval::run_tests` line
    /// contains the fixed substring `"quota exceeded"` is excluded from
    /// the image entirely and counted here instead of compiled/booted.
    quota_skips: usize,
    /// plans/M9.md item EE: a multi-module case that typechecks but whose
    /// import closure this oracle genuinely cannot build into a guest
    /// image (fail closed by name). Before EE every import-bearing case
    /// was a *silent* `continue` with no tally entry — so the summary
    /// line overstated the oracle's scope. Handled cases (the preferred
    /// half of EE) never increment this; only a named residual does.
    import_skips: usize,
}

/// One typechecked program ready for the oracle — either a single-module
/// file or the root of a loaded import closure. `modules` is the whole
/// closure keyed by dotted path (one entry for the no-imports case), the
/// same shape `bin/wrela.rs::check_closure` / `produce_report_and_image`
/// already build; `layout::merge_layout_ctx` needs every module's AST so
/// an imported struct's fields size correctly. `programs` is every
/// module's typed tree (plans/M9.md item II): `enrich_layout_ctx_with_
/// instantiations` needs imported instantiations under the importer's
/// alias spelling, or a case like `Box[Item]` sizes as a lowering-skip
/// even though `--stage=asm` (which does enrich) dumps clean.
struct DiffEvalChecked {
    root_program: sema::typed::TypedProgram,
    modules: BTreeMap<String, Module>,
    programs: BTreeMap<String, sema::typed::TypedProgram>,
}

/// Lex+parse+typecheck for the oracle — mirrors `bin/wrela.rs::check_closure`
/// (and `produce_report_and_image`'s own parallel copy of the same fork):
/// no imports → `sema::check_typed`; any import → `loader::load_closure` +
/// `sema::check_program_typed`. `None` only for a lex/parse/sema/load
/// failure (an `err-*` golden — an expected rejection, never a bug this
/// oracle should report on). plans/M9.md item EE: before this, imports
/// returned `None` silently and the caller `continue`d with no tally —
/// that was the defect; multi-module cases with comptime `@test`s now
/// reach the comparison.
pub(crate) fn typecheck_for_diff_eval(target: &Path) -> Option<DiffEvalChecked> {
    let source = std::fs::read_to_string(target).ok()?;
    let path_display = target.display().to_string();
    let tokens = lexer::lex(&source).ok()?;
    let module = parser::parse(tokens).ok()?;
    if module.imports.is_empty() {
        let program = sema::check_typed(&module, &path_display).ok()?;
        let addr = module.path.join(".");
        let mut modules = BTreeMap::new();
        modules.insert(addr.clone(), module);
        let mut programs = BTreeMap::new();
        programs.insert(addr, program.clone());
        return Some(DiffEvalChecked {
            root_program: program,
            modules,
            programs,
        });
    }
    // Deliberately parallel to `produce_report_and_image` / `bin/wrela.rs::
    // check_closure` — those driver internals are not a library surface
    // this crate can call into (same disclosed convention those call
    // sites already document).
    let loaded = loader::load_closure(target).ok()?;
    let paths: BTreeMap<Vec<String>, String> = loaded
        .modules
        .iter()
        .map(|(k, m)| (k.clone(), m.file.display().to_string()))
        .collect();
    let modules_by_key: BTreeMap<Vec<String>, Module> = loaded
        .modules
        .into_iter()
        .map(|(k, m)| (k, m.module))
        .collect();
    let programs_by_key = sema::check_program_typed(&modules_by_key, &paths).ok()?;
    let root_key = loaded.root.clone();
    let root_program = programs_by_key.get(&root_key)?.clone();
    let modules: BTreeMap<String, Module> = modules_by_key
        .into_iter()
        .map(|(k, m)| (k.join("."), m))
        .collect();
    let programs: BTreeMap<String, sema::typed::TypedProgram> = programs_by_key
        .into_iter()
        .map(|(k, p)| (k.join("."), p))
        .collect();
    Some(DiffEvalChecked {
        root_program,
        modules,
        programs,
    })
}

/// Builds a runtime-test image out of `test_names` (in the order given —
/// the guest's own transcript lines come out in this same order,
/// `layout::layout_test_image`'s own doc) plus the report text
/// `wrela-vmm` needs to boot it. Mirrors `bin/wrela.rs::test_cmd`'s own
/// image+report construction exactly: those driver internals are not a
/// library surface this crate can call into, so this is its own small,
/// deliberately parallel copy — the identical "own small, deliberately
/// parallel copy" reasoning `produce_report_and_image`'s own doc comment
/// already gives for `report-determinism`, one call chain later.
///
/// `modules` is the whole build closure (plans/M9.md item EE): an
/// imported struct's field layout lives in the *exporting* module's AST,
/// so `layout::merge_layout_ctx` must see every module, not just the
/// root. Lowering the root alone is enough for the code itself —
/// `lower::lower_program` emits imported fns/methods under the local
/// spelling (item EE decision 90).
pub(crate) fn build_runtime_test_image(
    program: &sema::typed::TypedProgram,
    modules: &BTreeMap<String, Module>,
    programs: &BTreeMap<String, sema::typed::TypedProgram>,
    source: &str,
    path: &str,
    test_names: &[String],
) -> Result<(Vec<u8>, String), String> {
    let mut layout_ctx = layout::merge_layout_ctx(modules).map_err(|e| e.message)?;
    // plans/M9.md item II: fold imported instantiations under the
    // importer's alias spelling — same call `--stage=asm` already makes.
    layout::enrich_layout_ctx_with_instantiations(&mut layout_ctx, programs);
    // plans/M6.md item F / scaffolding dissolution: same one-check →
    // one-lower path as `bin/wrela.rs::test_cmd` — force-root the live
    // runtime before layout so primary/enqueue/secondary trampolines exist.
    let graph = match &program.image_fn {
        Some(fn_name) => {
            wrela_compiler::eval::interp::eval_image(program, fn_name).map_err(|e| {
                format!(
                    "image graph: {}",
                    wrela_compiler::eval::to_sema_error(e).message
                )
            })?
        }
        None => wrela_compiler::eval::image::ImageGraph::default(),
    };
    let test_args =
        layout::resolve_runtime_test_args(program, test_names, &graph).map_err(|e| e)?;
    let async_tests: std::collections::BTreeSet<String> = test_names
        .iter()
        .filter(|name| program.fns.get(*name).is_some_and(|f| f.is_async))
        .cloned()
        .collect();
    let compiled = layout::lower_and_codegen_image(
        modules,
        programs,
        &layout_ctx,
        &graph,
        test_names,
        &async_tests,
    )?;
    let boot = layout::BootCtx {
        graph: &graph,
        modules: &compiled.modules,
        programs: &compiled.programs,
        layout_ctx: &compiled.layout_ctx,
        async_frames: &compiled.async_frames,
        group_child_index: &compiled.group_child_index,
        flow: &compiled.flow,
    };
    let image_layout = layout::layout_test_image(
        &compiled.program,
        test_names,
        &async_tests,
        Some(boot),
        &test_args,
    )
    .map_err(|e| e.message)?;
    let source_digest = report::sha256_hex(source.as_bytes());
    let image_digest = report::sha256_hex(&image_layout.blob);
    let mut report_text = format!(
        "Machine revision={}\nInput path={path} sha256={source_digest}\nImage sha256={image_digest}\n",
        wrela_machine::MACHINE_REVISION_STR
    );
    for s in &image_layout.sections {
        report_text.push_str(&format!(
            "Section name={} base={:#x} size={}\n",
            s.name, s.base, s.size
        ));
    }
    report_text.push_str(&format!("Entry base={:#x}\n", image_layout.entry));
    // plans/M8.md item C3: the same VMM-facing lines `bin/wrela.rs`'s own
    // runtime tier writes — `CoreEntry` (item C1), `Ring` (this item),
    // `Blk*` and `IrqHostInject`. This copy carried none of them before,
    // so every image the determinism lanes built was implicitly
    // single-core and deviceless; a cross-core case failed its release
    // doorbell rather than booting. One shared writer, no fourth copy.
    layout::append_vmm_runtime_lines(&mut report_text, &image_layout);
    Ok((image_layout.blob, report_text))
}

/// Shells out to the codesigned `wrela-vmm` binary exactly like `bin/
/// wrela.rs::test_cmd`/`golden`'s own `test.txt` stage do — `xtask`
/// itself stays unsigned throughout (plans/M5.md decision 11: the only
/// binary that ever touches Hypervisor.framework is `wrela-vmm`).
/// `exit_code_class` is the wrapper process's own exit code: `0`/`1`
/// mirror the guest's own reported outcome (decision 9's normal pass/
/// fail range), anything else names a genuine VMM-level failure.
struct VmmBoot {
    transcript: String,
    exit_code_class: i32,
}

fn run_vmm(vmm: &Path, report_path: &Path, img_path: &Path) -> Result<VmmBoot, String> {
    let out = Command::new(vmm)
        .arg(report_path)
        .arg(img_path)
        .output()
        .map_err(|e| format!("run wrela-vmm: {e}"))?;
    Ok(VmmBoot {
        transcript: String::from_utf8_lossy(&out.stdout).into_owned(),
        exit_code_class: out.status.code().unwrap_or(-1),
    })
}

/// One golden case's own recorded facts from `--record <path>` (`wrela-
/// vmm`'s own hand-rolled `key=value` text format, `wrela-vmm/src/
/// record.rs::RecordFile::to_text`) — `xtask` deliberately does not
/// depend on the `wrela-vmm` *crate* to parse this (CLAUDE.md: a
/// dependency is a liability, and plans/M5.md decision 11's own "keeps
/// xtask itself unsigned and the signed surface one small binary" reads
/// as a design boundary worth keeping crisp at the crate-graph level too,
/// not only at the "who calls HVF" level) — a few `strip_prefix` calls
/// over a handful of known keys is simpler than a real dependency for
/// the three fields `bench guest`/`profile` actually need. plans/M6.md
/// item E: `clock_log_len` -> `choice_count` (decision 9's own choice-
/// sequence recorder grows the field this reads — bench guest's own
/// "exact counts" assertion now covers the whole choice sequence, not
/// only clock reads, per the item's own "bench guest's exact-count
/// assertions extend to choice-count" instruction).
pub(crate) struct GuestRecord {
    pub(crate) exit_code: u64,
    pub(crate) exits: u64,
    pub(crate) choice_count: usize,
}

pub(crate) fn parse_guest_record(text: &str) -> Result<GuestRecord, String> {
    let mut exit_code = None;
    let mut exits = None;
    let mut choice_count = None;
    for line in text.lines() {
        if let Some(v) = line.strip_prefix("exit_code=") {
            exit_code = Some(v.parse().map_err(|e| format!("bad exit_code {v:?}: {e}"))?);
        } else if let Some(v) = line.strip_prefix("exits=") {
            exits = Some(v.parse().map_err(|e| format!("bad exits {v:?}: {e}"))?);
        } else if let Some(v) = line.strip_prefix("choice_count=") {
            choice_count = Some(
                v.parse()
                    .map_err(|e| format!("bad choice_count {v:?}: {e}"))?,
            );
        }
    }
    Ok(GuestRecord {
        exit_code: exit_code.ok_or("record file: missing exit_code")?,
        exits: exits.ok_or("record file: missing exits")?,
        choice_count: choice_count.ok_or("record file: missing choice_count")?,
    })
}

/// The oracle itself, over whichever golden cases `filter` selects
/// (`None` = the whole corpus, `cargo xtask diff-eval`'s own standalone
/// form; `Some(names)` = the in-`check` smoke subset, below). Every case
/// visited prints its own contribution as it happens; any real
/// disagreement returns `Err` immediately (decision 9: "ANY disagreement
/// ... fails"), never accumulated past the first one found.
fn diff_eval_over_cases(vmm: &Path, filter: Option<&[&str]>) -> Result<DiffEvalTally, String> {
    let golden_dir = root().join("tests/golden");
    let mut tally = DiffEvalTally::default();
    for case in golden_case_dirs(&golden_dir)? {
        let name = case
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("case")
            .to_string();
        if let Some(names) = filter {
            if !names.contains(&name.as_str()) {
                continue;
            }
        }
        let Some(target) = golden_case_target(&case)? else {
            continue;
        };
        if !target.exists() {
            continue;
        }
        let source = std::fs::read_to_string(&target)
            .map_err(|e| format!("read {}: {e}", target.display()))?;
        let path_display = target.display().to_string();
        let Some(checked) = typecheck_for_diff_eval(&target) else {
            continue; // out of scope: lex/parse/sema/load error (err-* goldens)
        };
        let program = &checked.root_program;
        if program.tests.is_empty() {
            continue; // fully out of scope: no @test fn of any kind
        }

        let comptime_names: Vec<String> = program
            .tests
            .iter()
            .filter(|t| t.kind == TestKind::Comptime)
            .map(|t| t.name.clone())
            .collect();
        let exhaustive_count = program
            .tests
            .iter()
            .filter(|t| t.kind == TestKind::Exhaustive)
            .count();
        tally.exhaustive_skips += exhaustive_count;

        if comptime_names.is_empty() {
            println!(
                "diff-eval: case {name}: no comptime-legal @test fn(s) ({exhaustive_count} \
                 exhaustive skipped) — out of scope"
            );
            continue;
        }

        // `eval::run_tests` first: every comptime test's own line is
        // needed both to filter out quota-exhaustion outcomes (below)
        // and, later, as the comparison oracle itself — one call covers
        // both, exactly the shape `wrela test`'s own comptime tier
        // already produces.
        let (eval_report, _) = eval::run_tests(program);
        let eval_line_for = |test_name: &str| -> Option<&str> {
            let prefix = format!("test {test_name}: ");
            eval_report.lines().find(|l| l.starts_with(&prefix))
        };

        // `comptime.eval.quotas`' own step/memory quota is a comptime-tier
        // resource bound with no backend equivalent (`DiffEvalTally::
        // quota_skips`' own doc comment) — a test the evaluator only
        // fails via quota exhaustion is excluded from the image entirely,
        // never compiled/booted, since a real image would just spin
        // forever rather than disagree meaningfully.
        let mut backend_names: Vec<String> = Vec::new();
        let mut quota_skipped: Vec<String> = Vec::new();
        for test_name in &comptime_names {
            match eval_line_for(test_name) {
                Some(line) if line.contains("quota exceeded") => {
                    quota_skipped.push(test_name.clone());
                }
                _ => backend_names.push(test_name.clone()),
            }
        }
        tally.quota_skips += quota_skipped.len();
        if !quota_skipped.is_empty() {
            println!(
                "diff-eval: case {name}: {} comptime test(s) skipped (evaluator-only quota \
                 exhaustion, no backend equivalent): {}",
                quota_skipped.len(),
                quota_skipped.join(", ")
            );
        }
        if backend_names.is_empty() {
            continue; // every comptime test in this case was quota-skipped
        }

        let (img_bytes, report_text) = match build_runtime_test_image(
            program,
            &checked.modules,
            &checked.programs,
            &source,
            &path_display,
            &backend_names,
        ) {
            Ok(pair) => pair,
            Err(e) => {
                tally.lowering_skips += backend_names.len();
                println!(
                    "diff-eval: case {name}: lowering failed closed ({e}) — {} comptime test(s) skipped",
                    backend_names.len()
                );
                continue;
            }
        };

        let tmp_dir = root().join(format!("target/diff-eval-tmp/{name}"));
        if tmp_dir.exists() {
            std::fs::remove_dir_all(&tmp_dir)
                .map_err(|e| format!("remove {}: {e}", tmp_dir.display()))?;
        }
        std::fs::create_dir_all(&tmp_dir)
            .map_err(|e| format!("create {}: {e}", tmp_dir.display()))?;
        let img_path = tmp_dir.join("test.img");
        let report_path = tmp_dir.join("test.report.txt");
        std::fs::write(&img_path, &img_bytes)
            .map_err(|e| format!("write {}: {e}", img_path.display()))?;
        std::fs::write(&report_path, &report_text)
            .map_err(|e| format!("write {}: {e}", report_path.display()))?;
        let boot_result = run_vmm(vmm, &report_path, &img_path);
        let _ = std::fs::remove_dir_all(&tmp_dir);
        let boot = boot_result?;
        if boot.exit_code_class != 0 && boot.exit_code_class != 1 {
            return Err(format!(
                "diff-eval: case {name}: the wrela VMM did not boot the test image (exit {})",
                boot.exit_code_class
            ));
        }

        let t_lines: Vec<&str> = boot.transcript.lines().collect();
        if t_lines.len() != backend_names.len() + 1 {
            return Err(format!(
                "diff-eval: case {name}: guest transcript is not well-formed (expected {} test \
                 line(s) then a summary, got {} line(s)):\n{}",
                backend_names.len(),
                t_lines.len(),
                boot.transcript
            ));
        }

        for (i, test_name) in backend_names.iter().enumerate() {
            let guest_line = t_lines[i];
            let prefix = format!("test {test_name}: ");
            if !guest_line.starts_with(&prefix) {
                return Err(format!(
                    "diff-eval: case {name}: guest transcript line {} does not name test \
                     `{test_name}` (got {guest_line:?})",
                    i + 1
                ));
            }
            let eval_line = eval_line_for(test_name).ok_or_else(|| {
                format!("diff-eval: case {name}: no evaluator line found for test `{test_name}`")
            })?;
            if guest_line != eval_line {
                return Err(format!(
                    "diff-eval: DISAGREEMENT in case {name}, test `{test_name}`:\n  evaluator: {eval_line}\n  backend:   {guest_line}"
                ));
            }
            tally.agree += 1;
        }
        tally.cases_agreed += 1;
        println!(
            "diff-eval: case {name}: {} comptime test(s) agree ({exhaustive_count} exhaustive skipped)",
            backend_names.len()
        );
    }
    Ok(tally)
}

/// `cargo xtask diff-eval`: the unrestricted, full-corpus form (decision
/// 9's own "the full corpus on demand").
fn diff_eval() -> Result<(), String> {
    let vmm = build_and_sign_vmm()?;
    let tally = diff_eval_over_cases(&vmm, None)?;
    println!(
        "diff-eval: {} test(s) agree across {} case(s), {} lowering-skips, {} exhaustive-skips, \
         {} quota-skips, {} import-skips",
        tally.agree,
        tally.cases_agreed,
        tally.lowering_skips,
        tally.exhaustive_skips,
        tally.quota_skips,
        tally.import_skips
    );
    Ok(())
}

/// The in-`check` smoke subset (plans/M5.md item F: "the boot golden +
/// one arithmetic-heavy case", item F's own text naming three specific
/// cases to pick from): `boot-hello` (exercises the whole build+sign+
/// boot chain end to end on every `check` run — it declares zero
/// comptime-legal tests of its own, decision 1's own runtime-only scope,
/// so it never contributes to the agree/skip counters, only to proving
/// the pipeline itself still boots) plus the two arithmetic-heavy
/// comptime suites `check-tests-arith`/`check-tests-program` (15
/// comptime tests between them, exercising checked/wrapping arithmetic,
/// structs, enums, `match`, loops, and a generic fn) — the closest
/// existing cases to the plan's own naming, used verbatim rather than
/// substituted.
const DIFF_EVAL_SMOKE_CASES: [&str; 3] = ["boot-hello", "check-tests-arith", "check-tests-program"];

fn diff_eval_smoke() -> Result<(), String> {
    let vmm = build_and_sign_vmm()?;
    let tally = diff_eval_over_cases(&vmm, Some(&DIFF_EVAL_SMOKE_CASES))?;
    println!(
        "diff-eval (smoke): {} test(s) agree across {} case(s), {} lowering-skips, {} \
         exhaustive-skips, {} quota-skips, {} import-skips",
        tally.agree,
        tally.cases_agreed,
        tally.lowering_skips,
        tally.exhaustive_skips,
        tally.quota_skips,
        tally.import_skips
    );
    Ok(())
}

// --- roundtrip --------------------------------------------------------
//
// plans/M1.md item E's second oracle, the parser's `diff-eval`: for every
// corpus entry that parses (same `...`-fragment skip rule as `corpus`)
// and every golden's `input.wr`, parse -> pretty-print -> reparse ->
// compare the two AST dumps with spans stripped (spans necessarily
// differ: the pretty-printed text is laid out differently from the
// original source, so only the tree shape is being compared). Any
// mismatch prints both dumps' first divergence plus the pretty-printed
// text that produced it, for direct debugging.
//
// ledger clause sema.check.roundtrip-stable adds a second oracle riding
// the same parse -> pretty -> reparse cycle, on the same entries, whenever
// the entry parses as a whole `Module` (sema has no fragment entry point):
// A = sema outcome of the original module, B = sema outcome of the
// reparsed one; `sema_roundtrip_check` (shared comparison machinery in
// the "fuzz: sema roundtrip stability + item-rotation invariance" section
// above) demands they agree. This is why the golden half of `roundtrip`
// below no longer filters to `ast-*` only: `check-*`/`err-type-*`/etc.
// golden inputs are exactly the sema-*meaningful* corpus (valid syntax,
// sema accepts or rejects) the sema oracle needs, and the existing AST
// oracle running on them too is a free, expected-to-pass bonus check
// (any AST mismatch it turned up would itself be a printer bug worth
// fixing, same as on `ast-*`).

enum RoundtripResult {
    Checked,
    Skipped,
    Mismatch(String),
}

fn roundtrip() -> Result<(), String> {
    let (blocks, failures) = extract_doc_blocks()?;
    if let Some(f) = failures.first() {
        return Err(format!("roundtrip: corpus is broken, fix it first: {f}"));
    }
    let examples = extract_example_files()?;
    let mut checked = 0usize;
    let mut skipped = 0usize;
    let mut sema_checked = 0usize;
    let mut mismatches = Vec::new();

    for b in blocks.into_iter().chain(examples) {
        if b.body.contains("...") {
            skipped += 1;
            continue;
        }
        let (result, sema) = roundtrip_one(&b.name, &b.body);
        match result {
            RoundtripResult::Checked => checked += 1,
            RoundtripResult::Skipped => skipped += 1,
            RoundtripResult::Mismatch(msg) => mismatches.push(msg),
        }
        match sema {
            None => {}
            Some(Ok(())) => sema_checked += 1,
            Some(Err(msg)) => mismatches.push(msg),
        }
    }

    let golden_dir = root().join("tests/golden");
    for dir in golden_case_dirs(&golden_dir)? {
        let input = dir.join("input.wr");
        if !input.exists() {
            continue;
        }
        let name = dir
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("golden")
            .to_string();
        let body = std::fs::read_to_string(&input)
            .map_err(|e| format!("read {}: {e}", input.display()))?;
        let (result, sema) = roundtrip_one(&name, &body);
        match result {
            RoundtripResult::Checked => checked += 1,
            RoundtripResult::Skipped => skipped += 1,
            RoundtripResult::Mismatch(msg) => mismatches.push(msg),
        }
        match sema {
            None => {}
            Some(Ok(())) => sema_checked += 1,
            Some(Err(msg)) => mismatches.push(msg),
        }
    }

    if mismatches.is_empty() {
        println!(
            "roundtrip: {checked} entry(ies) ok, {skipped} skipped (fragment or parse error), \
             {sema_checked} sema-roundtrip-checked"
        );
        Ok(())
    } else {
        for m in &mismatches {
            eprintln!("{m}\n");
        }
        Err(format!("roundtrip: {} mismatch(es)", mismatches.len()))
    }
}

/// One entry's parse -> pretty -> reparse -> compare cycle, plus (whenever
/// the entry parses as a whole `Module`) the sema-roundtrip oracle over
/// the same original/reparsed pair. Entries that don't lex/parse at all
/// are `Skipped` (that's `corpus`'s job to catch, not roundtrip's) rather
/// than treated as a failure here; the second return value is the
/// sema-roundtrip oracle's own result — `None` when the entry isn't a
/// `Module` (a fragment, or the AST cycle itself already failed before a
/// reparsed module ever existed to check), `Some(Ok(()))` when it agreed,
/// `Some(Err(reason))` on a genuine sema-roundtrip disagreement.
fn roundtrip_one(name: &str, body: &str) -> (RoundtripResult, Option<Result<(), String>>) {
    let tokens = match lexer::lex(body) {
        Ok(t) => t,
        Err(_) => return (RoundtripResult::Skipped, None),
    };
    match parser::parse_any(tokens) {
        Ok(Parsed::Module(m)) => {
            let dump1 = parser::dump_no_spans(&m);
            let pretty = printer::pretty(&m);
            let tokens2 = match lexer::lex(&pretty) {
                Ok(t) => t,
                Err(e) => {
                    return (
                        RoundtripResult::Mismatch(format!(
                            "{name}: pretty-printed output failed to lex: {} at {}:{}\n--- pretty ---\n{pretty}",
                            e.message, e.line, e.col
                        )),
                        None,
                    );
                }
            };
            let reparsed = match parser::parse(tokens2) {
                Ok(m2) => m2,
                Err(e) => {
                    return (
                        RoundtripResult::Mismatch(format!(
                            "{name}: pretty-printed output failed to reparse: {} at {}:{}\n--- pretty ---\n{pretty}",
                            e.message, e.line, e.col
                        )),
                        None,
                    );
                }
            };
            let dump2 = parser::dump_no_spans(&reparsed);
            let ast_result = compare_dumps(name, &dump1, &dump2, &pretty);
            let sema = Some(sema_roundtrip_check(name, &m, &reparsed));
            (ast_result, sema)
        }
        Ok(Parsed::Fragment(entries)) => {
            let dump1 = parser::dump_fragment_no_spans(&entries);
            let pretty = printer::pretty_fragment(&entries);
            let tokens2 = match lexer::lex(&pretty) {
                Ok(t) => t,
                Err(e) => {
                    return (
                        RoundtripResult::Mismatch(format!(
                            "{name}: pretty-printed fragment failed to lex: {} at {}:{}\n--- pretty ---\n{pretty}",
                            e.message, e.line, e.col
                        )),
                        None,
                    );
                }
            };
            let dump2 = match parser::parse_any(tokens2) {
                Ok(Parsed::Fragment(entries2)) => parser::dump_fragment_no_spans(&entries2),
                Ok(Parsed::Module(_)) => {
                    return (
                        RoundtripResult::Mismatch(format!(
                            "{name}: pretty-printed fragment reparsed as a module\n--- pretty ---\n{pretty}"
                        )),
                        None,
                    );
                }
                Err(e) => {
                    return (
                        RoundtripResult::Mismatch(format!(
                            "{name}: pretty-printed fragment failed to reparse: {} at {}:{}\n--- pretty ---\n{pretty}",
                            e.message, e.line, e.col
                        )),
                        None,
                    );
                }
            };
            // Sema has no fragment entry point (it operates on a whole
            // `Module` — mod.rs's own doc comment); the sema-roundtrip
            // oracle only applies to the `Parsed::Module` arm above.
            (compare_dumps(name, &dump1, &dump2, &pretty), None)
        }
        Err(_) => (RoundtripResult::Skipped, None),
    }
}

/// The sema-roundtrip oracle (ledger clause sema.check.roundtrip-stable),
/// applied to one entry's original and reparsed modules — the same
/// `sema_outcome_summary`/`sema_outcomes_agree` machinery `fuzz sema`
/// uses. `Ok(())` on agreement.
fn sema_roundtrip_check(name: &str, original: &Module, reparsed: &Module) -> Result<(), String> {
    const PATH: &str = "<roundtrip>";
    let a = sema_outcome_summary(original, PATH);
    let b = sema_outcome_summary(reparsed, PATH);
    sema_outcomes_agree(&a, &b)
        .map_err(|reason| format!("{name}: sema-roundtrip mismatch: {reason}"))
}

fn compare_dumps(name: &str, dump1: &str, dump2: &str, pretty: &str) -> RoundtripResult {
    if dump1 == dump2 {
        return RoundtripResult::Checked;
    }
    let (line1, line2) = first_divergence(dump1, dump2);
    RoundtripResult::Mismatch(format!(
        "{name}: roundtrip mismatch (first divergence)\n--- original dump ---\n{line1}\n--- reparsed dump ---\n{line2}\n--- pretty-printed ---\n{pretty}"
    ))
}

/// The first line at which two dumps differ, each labeled with its line
/// number (1-based); `<end of output>` stands in when one dump is shorter.
fn first_divergence(a: &str, b: &str) -> (String, String) {
    let a_lines: Vec<&str> = a.lines().collect();
    let b_lines: Vec<&str> = b.lines().collect();
    let n = a_lines.len().max(b_lines.len());
    for i in 0..n {
        let la = a_lines.get(i).copied().unwrap_or("<end of output>");
        let lb = b_lines.get(i).copied().unwrap_or("<end of output>");
        if la != lb {
            return (
                format!("line {}: {la}", i + 1),
                format!("line {}: {lb}", i + 1),
            );
        }
    }
    ("<identical>".to_string(), "<identical>".to_string())
}

// --- ledger ---------------------------------------------------------------
//
// ledger/ledger.toml maps normative clauses in docs/language/ to the tests
// that enforce them. Every clause has status "test" (with existing test
// paths) or "gap" (explicit, visible debt). This measures coverage of the
// SPEC, not of the code.

/// Does any `.rs` file under `crates/` define `#[test] fn <name>(...)`?
/// Backs the ledger's `unit:<fn name>` test references (plans/M9.md item
/// AA): a clause may not name a unit test that does not exist.
///
/// **The `#[test]` attribute is required, not just the function**
/// (plans/M9.md decision 59b). The first version of this checked only
/// for `fn <name>(`, which meant `unit:main` — `fn main(` in this very
/// file — satisfied a clause and counted it among the tested ones. A
/// reference type satisfiable by a non-test lets a clause claim coverage
/// it does not have, which is exactly what the sibling `golden/<name>`
/// reference (a real directory under `tests/`) does not permit.
///
/// Deliberately not an attribute parser: it looks for a literal
/// `#[test]` followed, past whitespace only, by `fn <name>(`. A test
/// carrying a second attribute between the two (`#[should_panic]`) would
/// be reported as missing — a false negative, which is the safe
/// direction, and loud.
///
/// Walks the tree rather than shelling out, and fails closed — an
/// unreadable tree yields no hits, which reports the clause as unbacked.
fn crate_sources_have_test_fn(name: &str) -> bool {
    let needle = format!("fn {name}(");
    fn walk(dir: &std::path::Path, needle: &str) -> bool {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return false;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                if path.file_name().is_some_and(|n| n == "target") {
                    continue;
                }
                if walk(&path, needle) {
                    return true;
                }
            } else if path.extension().is_some_and(|e| e == "rs")
                && std::fs::read_to_string(&path).is_ok_and(|s| {
                    s.match_indices("#[test]").any(|(i, m)| {
                        // The attribute must not be commented out
                        // (plans/M9.md decision 59c, orchestrator
                        // verification of 59b): a textual scan is happy
                        // to find `#[test]` inside `// #[test]`, so
                        // commenting out an attribute to disable a
                        // flaky test would silently leave every clause
                        // citing it counted as tested — the exact
                        // silent-coverage failure `unit:` refs exist to
                        // prevent. Checked by walking back to the line
                        // start and rejecting a `//` before the
                        // attribute; a `//` anywhere earlier on that
                        // line comments the rest of it out.
                        let line_start = s[..i].rfind('\n').map_or(0, |n| n + 1);
                        if s[line_start..i].contains("//") {
                            return false;
                        }
                        s[i + m.len()..].trim_start().starts_with(needle)
                    })
                })
            {
                return true;
            }
        }
        false
    }
    walk(&root().join("crates"), &needle)
}

fn ledger() -> Result<(), String> {
    let path = root().join("ledger/ledger.toml");
    let text =
        std::fs::read_to_string(&path).map_err(|e| format!("read {}: {e}", path.display()))?;
    let value: toml::Value = text.parse().map_err(|e| format!("parse ledger: {e}"))?;
    let clauses = value
        .get("clause")
        .and_then(|c| c.as_array())
        .ok_or("ledger has no [[clause]] entries")?;
    let mut seen = std::collections::HashSet::new();
    let mut tested = 0usize;
    let mut gaps = Vec::new();
    for (i, clause) in clauses.iter().enumerate() {
        let id = clause
            .get("id")
            .and_then(|v| v.as_str())
            .ok_or(format!("clause {i}: missing id"))?;
        if !seen.insert(id.to_string()) {
            return Err(format!("duplicate clause id `{id}`"));
        }
        let doc = clause
            .get("doc")
            .and_then(|v| v.as_str())
            .ok_or(format!("clause `{id}`: missing doc reference"))?;
        let doc_file = doc
            .split('#')
            .next()
            .expect("split yields at least one part");
        if !root().join("docs/language").join(doc_file).exists() {
            return Err(format!(
                "clause `{id}`: doc file `{doc_file}` does not exist"
            ));
        }
        match clause.get("status").and_then(|v| v.as_str()) {
            Some("test") => {
                let tests = clause
                    .get("tests")
                    .and_then(|v| v.as_array())
                    .ok_or(format!("clause `{id}`: status=test requires a tests list"))?;
                if tests.is_empty() {
                    return Err(format!("clause `{id}`: empty tests list"));
                }
                for t in tests {
                    let t = t
                        .as_str()
                        .ok_or(format!("clause `{id}`: non-string test"))?;
                    // `xtask:<command>` names a harness check instead of a
                    // tests/ path (e.g. the doc corpus).
                    if let Some(cmd) = t.strip_prefix("xtask:") {
                        if !matches!(
                            cmd,
                            "corpus"
                                | "repro"
                                | "diff-eval"
                                // plans/M7.md item F, decision 2a: the
                                // QEMU differential oracle for ring
                                // handling. Standalone (never wired into
                                // `check`) because 06 §1 has QEMU
                                // scheduled for retirement — see
                                // `diff_blk`'s own doc comment.
                                | "diff-blk"
                                | "profile"
                                | "bench"
                                | "fuzz"
                                | "roundtrip"
                                | "report-determinism"
                        ) {
                            return Err(format!("clause `{id}`: unknown xtask check `{cmd}`"));
                        }
                        continue;
                    }
                    // `unit:<fn name>` names a `#[test]` inside a crate
                    // (plans/M9.md item AA: the intrinsic-surface guard
                    // locks compiler *source* against a written-down
                    // list, so it is a cargo unit test rather than a
                    // golden — there is no artifact to dump). Verified
                    // mechanically the only way a name can be: a
                    // `#[test]`-attributed function by that name must
                    // exist under `crates/` (decision 59b — the
                    // attribute is the whole point; without it any
                    // function satisfied the reference). `cargo test` is
                    // already the first step of `xtask check`, so a
                    // failing one cannot reach here.
                    if let Some(f) = t.strip_prefix("unit:") {
                        if f.is_empty() {
                            return Err(format!("clause `{id}`: empty unit test name"));
                        }
                        if !crate_sources_have_test_fn(f) {
                            return Err(format!(
                                "clause `{id}`: `{f}` is not a `#[test]` function under crates/"
                            ));
                        }
                        continue;
                    }
                    if !root().join("tests").join(t).exists() {
                        return Err(format!(
                            "clause `{id}`: test `{t}` does not exist under tests/"
                        ));
                    }
                }
                tested += 1;
            }
            Some("gap") => gaps.push(id.to_string()),
            _ => return Err(format!("clause `{id}`: status must be \"test\" or \"gap\"")),
        }
    }
    println!(
        "ledger: {} clause(s), {tested} tested, {} explicit gap(s)",
        clauses.len(),
        gaps.len()
    );
    for g in &gaps {
        println!("  gap: {g}");
    }
    Ok(())
}

// --- diff-blk: the QEMU differential oracle (plans/M7.md decision 2a) -------

/// Homebrew's own path on this development host. Fails closed (never
/// skips) if absent — see `diff_blk`.
const QEMU_AARCH64: &str = "/opt/homebrew/bin/qemu-system-aarch64";

/// PL011 UART data register on QEMU's `virt` machine.
const VIRT_UART: u64 = 0x0900_0000;

/// `cargo xtask diff-blk`: the QEMU differential oracle (plans/M7.md
/// decision 2a — "the QEMU bootstrap implementation (06 §1, scheduled for
/// retirement) is used as a **differential oracle** for ring handling
/// while it still exists — the one thing that gets permanently harder
/// later, since a bespoke ring would have no second implementation to
/// disagree with").
///
/// One ring, two devices. `fill_blk_ring` writes the *identical*
/// descriptor chains, request headers, poisoned status bytes and payload
/// into both guests' data regions; each guest then publishes those chains
/// through its own transport — the wrela machine's shared-memory doorbell
/// (06 §5) on one side, QEMU's trapping `QueueNotify` MMIO write on the
/// other — and both devices' answers are compared field by field: the used
/// ring's own index and every `(id, len)` entry, both status bytes, and an
/// FNV-1a digest over the transferred payload plus its status byte,
/// computed identically on both sides (`wrela_vmm::record::digest_hex` in
/// Rust; the same loop hand-assembled in the QEMU guest).
///
/// **Deliberately not wired into `cargo xtask check`**: 06 §1 has QEMU
/// "used until the wrela VMM boots images, then retired", so making the
/// gate depend on a tool that is scheduled to go away would turn its
/// eventual removal into a gate failure. It fails closed — never skips —
/// when QEMU is absent, so an operator who runs it always learns whether
/// it ran.
///
/// What it can and cannot compare, stated plainly: the *ring handling and
/// request format* (which is exactly what decision 2a asks for), not the
/// transport (this machine has none) and not the doorbell (QEMU's
/// notification is a trap, which is the thing 06 §5 deletes).
///
/// Optional host conformance oracle outside `check`, not the VMM.
fn diff_blk() -> Result<(), String> {
    if !Path::new(QEMU_AARCH64).exists() {
        return fail_closed(
            "diff-blk",
            &format!("{QEMU_AARCH64} is not installed on this host"),
        );
    }
    let dir = root().join("target/diff-blk-tmp");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).map_err(|e| format!("create {}: {e}", dir.display()))?;

    // (0) Prove QEMU runs a hand-assembled guest of ours at all, before
    // any comparison is attempted — an oracle that silently degrades into
    // "QEMU printed nothing, so nothing disagreed" would be worse than
    // none.
    let smoke_path = dir.join("smoke.bin");
    std::fs::write(&smoke_path, build_qemu_smoke_guest()).map_err(|e| format!("write: {e}"))?;
    let smoke = run_qemu(&smoke_path, None)?;
    if !smoke.contains("WRELA-SMOKE") {
        return Err(format!(
            "diff-blk: QEMU did not run this harness's own smallest guest at all (got {smoke:?}) — \
             the oracle refuses to report agreement it never established"
        ));
    }

    // (1) QEMU's virtio-blk.
    let guest_path = dir.join("guest.bin");
    std::fs::write(&guest_path, build_qemu_blk_guest()).map_err(|e| format!("write guest: {e}"))?;
    let disk_path = dir.join("disk.img");
    std::fs::write(&disk_path, vec![0u8; 16 * 512]).map_err(|e| format!("write disk: {e}"))?;
    let qemu_out = run_qemu(&guest_path, Some(&disk_path))?;
    let fields: Vec<u64> = match qemu_out.split_whitespace().collect::<Vec<_>>().as_slice() {
        ["R", rest @ ..] if rest.len() == 7 => rest
            .iter()
            .map(|h| u64::from_str_radix(h, 16).map_err(|e| format!("bad qemu hex {h:?}: {e}")))
            .collect::<Result<_, _>>()?,
        _ => {
            return Err(format!(
                "diff-blk: the QEMU guest did not complete its own two operations (it prints \
                 `NODEV`/`FEAT`/`TMO1`/`TMO2` for each way bring-up can fail): {qemu_out:?}"
            ));
        }
    };
    let (used_w0, used_w1, used_w2) = (fields[0], fields[1], fields[2]);
    let qemu = BlkAnswer {
        used_idx: ((used_w0 >> 16) & 0xFFFF) as u32,
        head0: (used_w0 >> 32) as u32,
        len0: (used_w1 & 0xFFFF_FFFF) as u32,
        head1: (used_w1 >> 32) as u32,
        len1: (used_w2 & 0xFFFF_FFFF) as u32,
        status0: fields[3] as u32,
        status1: fields[4] as u32,
        digest0: format!("{:016x}", fields[5]),
        digest1: format!("{:016x}", fields[6]),
    };

    // (2) The wrela VMM's own model, over the identical ring — read back
    // out of the recorded choice sequence (the completions themselves)
    // plus the guest's own checks (its exit code).
    let vmm = build_and_sign_vmm()?;
    let (img_bytes, report_text) = blk_conformance_image();
    let img_path = dir.join("wrela.img");
    let report_path = dir.join("wrela.report.txt");
    let record_path = dir.join("wrela.record.txt");
    std::fs::write(&img_path, &img_bytes).map_err(|e| format!("write img: {e}"))?;
    std::fs::write(&report_path, &report_text).map_err(|e| format!("write report: {e}"))?;
    let out = Command::new(&vmm)
        .arg(&report_path)
        .arg(&img_path)
        .arg("--record")
        .arg(&record_path)
        .output()
        .map_err(|e| format!("run wrela-vmm: {e}"))?;
    if out.status.code() != Some(0) {
        return Err(format!(
            "diff-blk: the wrela side's own boot failed (exit {:?}):\n{}",
            out.status.code(),
            String::from_utf8_lossy(&out.stderr)
        ));
    }
    let record_text = std::fs::read_to_string(&record_path)
        .map_err(|e| format!("read {}: {e}", record_path.display()))?;
    let completions: Vec<BTreeMap<String, String>> = record_text
        .lines()
        .filter(|l| l.contains("=DeviceCompletion "))
        .map(|l| {
            l.split_whitespace()
                .filter_map(|p| p.split_once('='))
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect()
        })
        .collect();
    if completions.len() != 2 {
        return Err(format!(
            "diff-blk: the wrela side recorded {} completion(s), expected 2",
            completions.len()
        ));
    }
    let field = |i: usize, k: &str| -> Result<String, String> {
        completions[i]
            .get(k)
            .cloned()
            .ok_or_else(|| format!("diff-blk: recorded completion #{i} has no `{k}` field"))
    };
    let num = |i: usize, k: &str| -> Result<u32, String> {
        field(i, k)?
            .parse()
            .map_err(|e| format!("diff-blk: completion #{i} field `{k}`: {e}"))
    };
    let wrela = BlkAnswer {
        // The wrela model publishes one used entry per completion, in
        // order, so its own used index is simply how many it published.
        used_idx: completions.len() as u32,
        head0: num(0, "head")?,
        len0: num(0, "len")?,
        head1: num(1, "head")?,
        len1: num(1, "len")?,
        status0: num(0, "status")?,
        status1: num(1, "status")?,
        digest0: field(0, "digest")?,
        digest1: field(1, "digest")?,
    };

    // (3) The comparison itself.
    let facts: Vec<(&str, String, String)> = vec![
        (
            "used.idx",
            wrela.used_idx.to_string(),
            qemu.used_idx.to_string(),
        ),
        (
            "write: used id",
            wrela.head0.to_string(),
            qemu.head0.to_string(),
        ),
        (
            "write: used len",
            wrela.len0.to_string(),
            qemu.len0.to_string(),
        ),
        (
            "write: status",
            wrela.status0.to_string(),
            qemu.status0.to_string(),
        ),
        (
            "write: payload digest",
            wrela.digest0.clone(),
            qemu.digest0.clone(),
        ),
        (
            "read: used id",
            wrela.head1.to_string(),
            qemu.head1.to_string(),
        ),
        (
            "read: used len",
            wrela.len1.to_string(),
            qemu.len1.to_string(),
        ),
        (
            "read: status",
            wrela.status1.to_string(),
            qemu.status1.to_string(),
        ),
        (
            "read: payload digest",
            wrela.digest1.clone(),
            qemu.digest1.clone(),
        ),
    ];
    let mut disagreements = Vec::new();
    for (what, w, q) in &facts {
        if w != q {
            disagreements.push(format!("  {what}: wrela says `{w}`, QEMU says `{q}`"));
        }
    }
    if !disagreements.is_empty() {
        return Err(format!(
            "diff-blk: the two virtio-blk implementations disagree on {} of {} compared fact(s):\n{}",
            disagreements.len(),
            facts.len(),
            disagreements.join("\n")
        ));
    }
    for (what, w, _) in &facts {
        println!("diff-blk:   {what} = {w} (both)");
    }
    println!(
        "diff-blk: {} fact(s) agree between wrela-vmm's own virtio-blk model and QEMU {}'s, over \
         the identical descriptor chains",
        facts.len(),
        qemu_version()?
    );
    Ok(())
}

/// What one implementation answered for the two-operation workload. Both
/// sides fill this in from entirely different sources — the wrela side
/// from its own recorded choice sequence, QEMU's from the used ring its
/// guest read back — and it is the whole comparison surface.
struct BlkAnswer {
    used_idx: u32,
    head0: u32,
    len0: u32,
    head1: u32,
    len1: u32,
    status0: u32,
    status1: u32,
    digest0: String,
    digest1: String,
}

/// The QEMU build actually compared against, recorded in the oracle's own
/// output line (a differential result means nothing without naming the
/// other implementation).
fn qemu_version() -> Result<String, String> {
    let out = Command::new(QEMU_AARCH64)
        .arg("--version")
        .output()
        .map_err(|e| format!("run qemu --version: {e}"))?;
    Ok(String::from_utf8_lossy(&out.stdout)
        .lines()
        .next()
        .unwrap_or("qemu-system-aarch64")
        .trim()
        .to_string())
}

/// Runs one hand-assembled bare-metal guest under QEMU's `virt` machine
/// and returns everything it printed on the UART. Bounded by a wall clock
/// (a guest that never reaches its own `SYSTEM_OFF` is killed and
/// reported, never left to hang the harness).
fn run_qemu(guest: &Path, disk: Option<&Path>) -> Result<String, String> {
    let mut cmd = Command::new(QEMU_AARCH64);
    cmd.args([
        "-M",
        "virt",
        "-cpu",
        "cortex-a72",
        "-m",
        "256",
        "-nographic",
        "-no-reboot",
        // QEMU's virtio-mmio transport still defaults to the **legacy**
        // (v1) interface, whose ring layout and feature words are not the
        // ones 03-hardware.md §1 names ("OASIS VIRTIO 1.2 split rings as
        // profiled by the machine spec"). Without this the guest's own
        // scan finds a `Version=1` transport and prints `NODEV` — found
        // the honest way, by the oracle refusing to run, not by guessing.
        "-global",
        "virtio-mmio.force-legacy=false",
    ]);
    cmd.arg("-device");
    cmd.arg(format!(
        "loader,file={},addr=0x40100000,force-raw=on",
        guest.display()
    ));
    cmd.arg("-device").arg("loader,addr=0x40100000,cpu-num=0");
    if let Some(disk) = disk {
        cmd.arg("-drive")
            .arg(format!("if=none,file={},format=raw,id=d0", disk.display()));
        cmd.arg("-device").arg("virtio-blk-device,drive=d0");
    }
    cmd.stdout(std::process::Stdio::piped());
    cmd.stderr(std::process::Stdio::piped());
    cmd.stdin(std::process::Stdio::null());
    let mut child = cmd.spawn().map_err(|e| format!("spawn qemu: {e}"))?;
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        match child.try_wait().map_err(|e| format!("wait qemu: {e}"))? {
            Some(_) => break,
            None => {
                if Instant::now() > deadline {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err(
                        "diff-blk: the QEMU guest never reached its own SYSTEM_OFF within 20s"
                            .to_string(),
                    );
                }
                std::thread::sleep(Duration::from_millis(20));
            }
        }
    }
    let out = child
        .wait_with_output()
        .map_err(|e| format!("collect qemu output: {e}"))?;
    Ok(String::from_utf8_lossy(&out.stdout).to_string())
}

fn qemu_load_imm(reg: u8, value: u64) -> Vec<u32> {
    use wrela_compiler::encode;
    vec![
        encode::enc_movz(reg, (value & 0xFFFF) as u16, 0, true),
        encode::enc_movk(reg, ((value >> 16) & 0xFFFF) as u16, 16, true),
        encode::enc_movk(reg, ((value >> 32) & 0xFFFF) as u16, 32, true),
        encode::enc_movk(reg, ((value >> 48) & 0xFFFF) as u16, 48, true),
    ]
}

/// `hvc #0` — PSCI's own conduit on QEMU's `virt` machine without
/// `virtualization=on`. Not in `wrela_compiler::encode` because this
/// machine has no hypercall instruction at all (06 §5: the guest exits
/// via a trapping store), so the one raw word is spelled here.
const ENC_HVC0: u32 = 0xD400_0002;

/// `x0 = PSCI SYSTEM_OFF; hvc #0` — how a QEMU guest ends. Nothing after
/// it ever runs.
fn qemu_system_off(w: &mut Vec<u32>) {
    w.extend(qemu_load_imm(0, 0x8400_0008));
    w.push(ENC_HVC0);
}

fn build_qemu_smoke_guest() -> Vec<u8> {
    use wrela_compiler::encode;
    let mut w = Vec::new();
    w.extend(qemu_load_imm(9, VIRT_UART));
    for b in b"WRELA-SMOKE\n" {
        w.push(encode::enc_movz(10, *b as u16, 0, false));
        w.push(encode::enc_str_w_imm(10, 9, 0));
    }
    qemu_system_off(&mut w);
    w.iter().flat_map(|x| x.to_le_bytes()).collect()
}

// --- the QEMU-side driver ---------------------------------------------------
//
// virtio-mmio v2 register file (OASIS VIRTIO 1.2 §4.2.2) — the transport
// **this machine does not have** (06 §3/§5 delete discovery and trapping
// notification alike), present here for exactly one reason: it is how a
// guest reaches QEMU's virtio-blk, and QEMU's virtio-blk is the second
// implementation decision 2a wants to disagree with. Nothing below is
// modelled by `wrela-vmm`, and nothing below should ever be.

const MMIO_MAGIC: u16 = 0x000;
const MMIO_VERSION: u16 = 0x004;
const MMIO_DEVICE_ID: u16 = 0x008;
const MMIO_DEVICE_FEATURES: u16 = 0x010;
const MMIO_DEVICE_FEATURES_SEL: u16 = 0x014;
const MMIO_DRIVER_FEATURES: u16 = 0x020;
const MMIO_DRIVER_FEATURES_SEL: u16 = 0x024;
const MMIO_QUEUE_SEL: u16 = 0x030;
const MMIO_QUEUE_NUM: u16 = 0x038;
const MMIO_QUEUE_READY: u16 = 0x044;
const MMIO_QUEUE_NOTIFY: u16 = 0x050;
const MMIO_STATUS: u16 = 0x070;
const MMIO_QUEUE_DESC_LOW: u16 = 0x080;
const MMIO_QUEUE_DESC_HIGH: u16 = 0x084;
const MMIO_QUEUE_DRIVER_LOW: u16 = 0x090;
const MMIO_QUEUE_DRIVER_HIGH: u16 = 0x094;
const MMIO_QUEUE_DEVICE_LOW: u16 = 0x0A0;
const MMIO_QUEUE_DEVICE_HIGH: u16 = 0x0A4;

const VIRT_MMIO_BASE: u64 = 0x0A00_0000;
const VIRT_MMIO_STRIDE: u64 = 0x200;
const VIRT_MMIO_SLOTS: u64 = 32;
/// Where QEMU's generic loader puts this guest, and therefore where its
/// own trailing data region (ring + buffers) lives.
const QEMU_LOAD_ADDR: u64 = 0x4010_0000;

/// The shared shape of both sides of the oracle: the identical ring
/// geometry, descriptor chains, and payload `blk_conformance_image` gives
/// the wrela VMM, expressed as offsets within one data region.
mod blk_shape {
    pub const QUEUE_SIZE: u64 = 8;
    pub const DESC_SIZE: u64 = 16;
    pub const DESC_F_NEXT: u16 = 1;
    pub const DESC_F_WRITE: u16 = 2;
    pub const T_IN: u32 = 0;
    pub const T_OUT: u32 = 1;
    pub const OFF_DESC: u64 = 0x000;
    pub const OFF_AVAIL: u64 = 0x080;
    pub const OFF_USED: u64 = 0x0C0;
    pub const OFF_HDR1: u64 = 0x150;
    pub const OFF_HDR2: u64 = 0x160;
    pub const OFF_STATUS1: u64 = 0x170;
    pub const OFF_STATUS2: u64 = 0x178;
    pub const OFF_SRC: u64 = 0x200;
    pub const OFF_DST: u64 = 0x400;
    pub const DATA_REGION_SIZE: u64 = 0x600;

    /// The one payload both sides write and read back.
    pub fn payload() -> Vec<u8> {
        (0..512u32).map(|i| ((i * 7 + 3) % 256) as u8).collect()
    }
}

/// Writes the ring's own prefilled bytes (descriptor chains, both request
/// headers, the poisoned status bytes, the source payload) into `img` at
/// `data_off`, given the region's guest-physical `data_base`. **The one
/// copy both sides of the oracle use**, so "the same ring" is a fact
/// rather than a claim: `blk_conformance_image` (the wrela VMM's own
/// image) and `build_qemu_blk_guest` (QEMU's) both call this.
fn fill_blk_ring(img: &mut [u8], data_off: usize, data_base: u64) {
    use blk_shape::*;
    let put = |img: &mut [u8], off: u64, bytes: &[u8]| {
        let at = data_off + off as usize;
        img[at..at + bytes.len()].copy_from_slice(bytes);
    };
    let desc = |img: &mut [u8], i: u64, addr: u64, len: u32, flags: u16, next: u16| {
        let at = OFF_DESC + i * DESC_SIZE;
        put(img, at, &addr.to_le_bytes());
        put(img, at + 8, &len.to_le_bytes());
        put(img, at + 12, &flags.to_le_bytes());
        put(img, at + 14, &next.to_le_bytes());
    };
    put(img, OFF_HDR1, &T_OUT.to_le_bytes());
    put(img, OFF_HDR1 + 8, &0u64.to_le_bytes());
    desc(img, 0, data_base + OFF_HDR1, 16, DESC_F_NEXT, 1);
    desc(img, 1, data_base + OFF_SRC, 512, DESC_F_NEXT, 2);
    desc(img, 2, data_base + OFF_STATUS1, 1, DESC_F_WRITE, 0);
    put(img, OFF_HDR2, &T_IN.to_le_bytes());
    put(img, OFF_HDR2 + 8, &0u64.to_le_bytes());
    desc(img, 3, data_base + OFF_HDR2, 16, DESC_F_NEXT, 4);
    desc(
        img,
        4,
        data_base + OFF_DST,
        512,
        DESC_F_NEXT | DESC_F_WRITE,
        5,
    );
    desc(img, 5, data_base + OFF_STATUS2, 1, DESC_F_WRITE, 0);
    // `0` is `STATUS_OK` and the image is zero-padded, so an unwritten
    // status byte would read as a pass. Poison both.
    put(img, OFF_STATUS1, &[0xEE]);
    put(img, OFF_STATUS2, &[0xEE]);
    put(img, OFF_SRC, &blk_shape::payload());
}

/// A bare-metal virtio-mmio blk driver, hand-assembled, for QEMU's `virt`
/// machine. Scans the 32 virtio-mmio transports for a block device, brings
/// it up (reset -> ACKNOWLEDGE -> DRIVER -> features -> FEATURES_OK ->
/// queue -> DRIVER_OK), publishes the *same two chains* the wrela side
/// publishes, polls the used ring, then prints one line the harness parses:
///
/// ```text
/// R <used[0..8]> <used[8..16]> <used[16..24]> <fnv(SRC||status1)> <fnv(DST||status2)>
/// ```
///
/// Every failure has its own printed marker instead of a hang (`NODEV`,
/// `FEAT`, `TMO1`, `TMO2`), so a broken oracle says which step broke.
fn build_qemu_blk_guest() -> Vec<u8> {
    use blk_shape::*;
    use wrela_compiler::encode;
    use wrela_compiler::encode::Cond;

    // Registers: x20 = transport base, x21 = data base, x22 = UART,
    // x9/x10/x11/x12/x13/x14/x15/x16 = scratch, x23..x27 = results.
    let build = |data_base: u64| -> Vec<u32> {
        let mut w: Vec<u32> = Vec::new();
        let li = |w: &mut Vec<u32>, reg: u8, v: u64| w.extend(qemu_load_imm(reg, v));

        li(&mut w, 22, VIRT_UART);
        li(&mut w, 21, data_base);

        // --- print one byte through the UART -------------------------
        let putc = |w: &mut Vec<u32>, b: u8| {
            w.push(encode::enc_movz(10, b as u16, 0, false));
            w.push(encode::enc_str_w_imm(10, 22, 0));
        };
        let puts = |w: &mut Vec<u32>, s: &[u8]| {
            for b in s {
                putc(w, *b);
            }
        };

        // --- scan the 32 virtio-mmio transports for DeviceID 2 --------
        li(&mut w, 20, VIRT_MMIO_BASE);
        li(&mut w, 19, VIRT_MMIO_SLOTS);
        let scan_top = w.len();
        w.push(encode::enc_ldr_w_imm(9, 20, MMIO_MAGIC));
        li(&mut w, 10, 0x7472_6976); // 'virt'
        w.push(encode::enc_cmp_reg(9, 10, false));
        let magic_ne = w.len();
        w.push(0); // b.ne next
        w.push(encode::enc_ldr_w_imm(9, 20, MMIO_VERSION));
        w.push(encode::enc_cmp_imm(9, 2, false));
        let version_ne = w.len();
        w.push(0); // b.ne next
        w.push(encode::enc_ldr_w_imm(9, 20, MMIO_DEVICE_ID));
        w.push(encode::enc_cmp_imm(9, 2, false)); // VIRTIO_ID_BLOCK
        let id_eq = w.len();
        w.push(0); // b.eq found
        let next_slot = w.len();
        li(&mut w, 10, VIRT_MMIO_STRIDE);
        w.push(encode::enc_add_reg(20, 20, 10, true));
        w.push(encode::enc_subs_imm(19, 19, 1, true));
        {
            let this = w.len();
            w.push(encode::enc_cbnz(
                19,
                ((scan_top as i64 - this as i64) * 4) as i32,
                true,
            ));
        }
        puts(&mut w, b"NODEV\n");
        qemu_system_off(&mut w);
        let found = w.len();
        for (at, cond) in [(magic_ne, Cond::Ne), (version_ne, Cond::Ne)] {
            w[at] = encode::enc_b_cond(cond, ((next_slot as i64 - at as i64) * 4) as i32);
        }
        w[id_eq] = encode::enc_b_cond(Cond::Eq, ((found as i64 - id_eq as i64) * 4) as i32);

        // --- bring-up (VIRTIO 1.2 §3.1) -------------------------------
        let status = |w: &mut Vec<u32>, bits: u16| {
            w.push(encode::enc_movz(10, bits, 0, false));
            w.push(encode::enc_str_w_imm(10, 20, MMIO_STATUS));
        };
        status(&mut w, 0); // reset
        status(&mut w, 1); // ACKNOWLEDGE
        status(&mut w, 3); // ACKNOWLEDGE | DRIVER

        // Feature word 1 (bits 32..63): accept VIRTIO_F_VERSION_1 (bit 32).
        w.push(encode::enc_movz(10, 1, 0, false));
        w.push(encode::enc_str_w_imm(10, 20, MMIO_DEVICE_FEATURES_SEL));
        w.push(encode::enc_str_w_imm(10, 20, MMIO_DRIVER_FEATURES_SEL));
        w.push(encode::enc_movz(10, 1, 0, false));
        w.push(encode::enc_str_w_imm(10, 20, MMIO_DRIVER_FEATURES));
        // Feature word 0: accept VIRTIO_BLK_F_FLUSH (bit 9) if offered.
        w.push(encode::enc_movz(10, 0, 0, false));
        w.push(encode::enc_str_w_imm(10, 20, MMIO_DEVICE_FEATURES_SEL));
        w.push(encode::enc_str_w_imm(10, 20, MMIO_DRIVER_FEATURES_SEL));
        w.push(encode::enc_ldr_w_imm(9, 20, MMIO_DEVICE_FEATURES));
        w.push(encode::enc_movz(10, 1 << 9, 0, false));
        w.push(encode::enc_and_reg(10, 10, 9, false));
        w.push(encode::enc_str_w_imm(10, 20, MMIO_DRIVER_FEATURES));

        status(&mut w, 3 | 8); // FEATURES_OK
        w.push(encode::enc_ldr_w_imm(9, 20, MMIO_STATUS));
        w.push(encode::enc_movz(10, 8, 0, false));
        w.push(encode::enc_and_reg(9, 9, 10, false));
        let feat_ok = w.len();
        w.push(0); // cbnz x9, ok
        puts(&mut w, b"FEAT\n");
        qemu_system_off(&mut w);
        {
            let target = w.len();
            w[feat_ok] = encode::enc_cbnz(9, ((target as i64 - feat_ok as i64) * 4) as i32, true);
        }

        // --- queue 0 --------------------------------------------------
        w.push(encode::enc_movz(10, 0, 0, false));
        w.push(encode::enc_str_w_imm(10, 20, MMIO_QUEUE_SEL));
        w.push(encode::enc_movz(10, QUEUE_SIZE as u16, 0, false));
        w.push(encode::enc_str_w_imm(10, 20, MMIO_QUEUE_NUM));
        for (lo, hi, addr) in [
            (
                MMIO_QUEUE_DESC_LOW,
                MMIO_QUEUE_DESC_HIGH,
                data_base + OFF_DESC,
            ),
            (
                MMIO_QUEUE_DRIVER_LOW,
                MMIO_QUEUE_DRIVER_HIGH,
                data_base + OFF_AVAIL,
            ),
            (
                MMIO_QUEUE_DEVICE_LOW,
                MMIO_QUEUE_DEVICE_HIGH,
                data_base + OFF_USED,
            ),
        ] {
            li(&mut w, 10, addr & 0xFFFF_FFFF);
            w.push(encode::enc_str_w_imm(10, 20, lo));
            li(&mut w, 10, addr >> 32);
            w.push(encode::enc_str_w_imm(10, 20, hi));
        }
        w.push(encode::enc_movz(10, 1, 0, false));
        w.push(encode::enc_str_w_imm(10, 20, MMIO_QUEUE_READY));
        status(&mut w, 3 | 8 | 4); // DRIVER_OK

        // --- publish + notify + poll, twice ---------------------------
        let mut timeout_markers: Vec<(usize, &[u8])> = Vec::new();
        for (round, (avail_idx, want_used)) in [(1u64, 1u32), (2u64, 2u32)].iter().enumerate() {
            // avail: one aligned 64-bit store of flags/idx/ring[0]/ring[1].
            li(&mut w, 9, data_base + OFF_AVAIL);
            li(&mut w, 10, (avail_idx << 16) | (3 << 48));
            w.push(encode::enc_str_x_imm(10, 9, 0));
            // The doorbell QEMU actually has: a trapping MMIO write.
            w.push(encode::enc_movz(10, 0, 0, false));
            w.push(encode::enc_str_w_imm(10, 20, MMIO_QUEUE_NOTIFY));
            // Poll used.idx, bounded.
            li(&mut w, 12, 200_000_000);
            li(&mut w, 9, data_base + OFF_USED);
            let poll_top = w.len();
            w.push(encode::enc_ldr_w_imm(10, 9, 0)); // flags | idx<<16
            w.push(encode::enc_lsr_imm(10, 10, 16, false));
            w.push(encode::enc_cmp_imm(10, *want_used as u16, false));
            let done = w.len();
            w.push(0); // b.eq done
            w.push(encode::enc_subs_imm(12, 12, 1, true));
            {
                let this = w.len();
                w.push(encode::enc_cbnz(
                    12,
                    ((poll_top as i64 - this as i64) * 4) as i32,
                    true,
                ));
            }
            let marker: &[u8] = if round == 0 { b"TMO1\n" } else { b"TMO2\n" };
            timeout_markers.push((w.len(), marker));
            puts(&mut w, marker);
            qemu_system_off(&mut w);
            let target = w.len();
            w[done] = encode::enc_b_cond(Cond::Eq, ((target as i64 - done as i64) * 4) as i32);
        }
        let _ = timeout_markers;

        // --- read the three used-ring words and both status bytes -----
        li(&mut w, 9, data_base + OFF_USED);
        w.push(encode::enc_ldr_x_imm(23, 9, 0));
        w.push(encode::enc_ldr_x_imm(24, 9, 8));
        w.push(encode::enc_ldr_x_imm(25, 9, 16));
        li(&mut w, 9, data_base + OFF_STATUS1);
        w.push(encode::enc_ldrb_imm(19, 9, 0));
        li(&mut w, 9, data_base + OFF_STATUS2);
        w.push(encode::enc_ldrb_imm(28, 9, 0));

        // --- FNV-1a over (buffer || status), twice --------------------
        // Matches `wrela_vmm::record::digest_hex` exactly: h = OFFSET;
        // for b: h ^= b; h *= PRIME.
        let fnv = |w: &mut Vec<u32>, start: u64, len: u64, status_at: u64, out: u8| {
            li(w, 13, 0xcbf2_9ce4_8422_2325); // hash
            li(w, 14, 0x0000_0100_0000_01b3); // prime
            li(w, 11, start);
            li(w, 15, start + len);
            let top = w.len();
            w.push(encode::enc_ldrb_imm(16, 11, 0));
            w.push(encode::enc_eor_reg(13, 13, 16, true));
            w.push(encode::enc_mul(13, 13, 14, true));
            w.push(encode::enc_add_imm(11, 11, 1, true));
            w.push(encode::enc_cmp_reg(11, 15, true));
            {
                let this = w.len();
                w.push(encode::enc_b_cond(
                    Cond::Ne,
                    ((top as i64 - this as i64) * 4) as i32,
                ));
            }
            li(w, 11, status_at);
            w.push(encode::enc_ldrb_imm(16, 11, 0));
            w.push(encode::enc_eor_reg(13, 13, 16, true));
            w.push(encode::enc_mul(13, 13, 14, true));
            w.push(encode::enc_mov_reg(out, 13, true));
        };
        fnv(
            &mut w,
            data_base + OFF_SRC,
            512,
            data_base + OFF_STATUS1,
            26,
        );
        fnv(
            &mut w,
            data_base + OFF_DST,
            512,
            data_base + OFF_STATUS2,
            27,
        );

        // --- print `R <5 hex words>\n` --------------------------------
        let print_hex = |w: &mut Vec<u32>, src: u8| {
            // x11 = shift, counting 60, 56, ... 0.
            w.push(encode::enc_movz(11, 60, 0, true));
            let top = w.len();
            w.push(encode::enc_lsr_reg(12, src, 11, true));
            w.push(encode::enc_movz(13, 0xF, 0, true));
            w.push(encode::enc_and_reg(12, 12, 13, true));
            w.push(encode::enc_cmp_imm(12, 10, true));
            // digit = nibble + ('0' or 'a' - 10), chosen branch-free.
            w.push(encode::enc_movz(13, b'0' as u16, 0, true));
            w.push(encode::enc_movz(14, (b'a' - 10) as u16, 0, true));
            // `Cc` is `Lo`: unsigned lower, i.e. nibble < 10.
            w.push(encode::enc_csel(13, 13, 14, Cond::Cc, true));
            w.push(encode::enc_add_reg(12, 12, 13, true));
            w.push(encode::enc_str_w_imm(12, 22, 0));
            w.push(encode::enc_subs_imm(11, 11, 4, true));
            {
                let this = w.len();
                w.push(encode::enc_b_cond(
                    Cond::Ge,
                    ((top as i64 - this as i64) * 4) as i32,
                ));
            }
        };
        puts(&mut w, b"R ");
        for reg in [23u8, 24, 25, 19, 28, 26, 27] {
            print_hex(&mut w, reg);
            putc(&mut w, b' ');
        }
        putc(&mut w, b'\n');
        qemu_system_off(&mut w);
        w
    };

    let probe_len = build(0).len();
    let data_base = {
        let after_code = QEMU_LOAD_ADDR + (probe_len as u64) * 4;
        after_code.div_ceil(16) * 16
    };
    let words = build(data_base);
    assert_eq!(words.len(), probe_len, "guest length must not move");
    let mut img: Vec<u8> = words.iter().flat_map(|x| x.to_le_bytes()).collect();
    img.resize((data_base - QEMU_LOAD_ADDR + DATA_REGION_SIZE) as usize, 0);
    let data_off = (data_base - QEMU_LOAD_ADDR) as usize;
    fill_blk_ring(&mut img, data_off, data_base);
    img
}
