//! Local development harness. There is no CI: `cargo xtask check` IS the
//! definition of "the tree is good", run locally before calling anything
//! done. Subcommands:
//!
//!   check      fmt + tests + golden + corpus + fuzz(smoke) + ledger (the gate)
//!   golden     run golden tests; `--update` rewrites expectations
//!   corpus     extract every ```wrela block from docs/ and lex it
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

fn root() -> PathBuf {
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
fn golden_case_dirs(golden_dir: &Path) -> Result<Vec<PathBuf>, String> {
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
        Some("corpus") => corpus(),
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
                "usage: cargo xtask <check|golden [--update]|corpus|fuzz [lexer|parser|sema|eval|lower|async] [--iters N] [--seed S]|roundtrip|report-determinism|ledger|repro|diff-eval|diff-blk|profile|bench <compiler|build|guest>>"
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

fn fail_closed(name: &str, why: &str) -> Result<(), String> {
    Err(format!(
        "`{name}` fails closed: {why}. It must never fake a pass."
    ))
}

fn run(cmd: &mut Command, what: &str) -> Result<(), String> {
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
    corpus()?;
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

// --- corpus ---------------------------------------------------------------
//
// The docs are test inputs: every ```wrela fenced block in docs/language/
// must lex cleanly (from M1 on, also parse — fragments containing `...`
// stay lex-only). Blocks are materialized under target/corpus/ for
// debugging; the check itself runs in-process.

/// One ```wrela fenced block extracted from a docs/language/*.md file.
struct DocBlock {
    doc: PathBuf,
    start_line: usize,
    name: String,
    body: String,
}

/// Walks every docs/language/*.md file and pulls out its ```wrela blocks,
/// in deterministic (sorted-by-filename, source-order-within-file) order.
/// Shared by `corpus` (which lexes every block) and `fuzz` (which mutates
/// them) — the walk exists exactly once.
fn extract_doc_blocks() -> Result<(Vec<DocBlock>, Vec<String>), String> {
    let docs_dir = root().join("docs/language");
    let mut doc_files: Vec<_> = std::fs::read_dir(&docs_dir)
        .map_err(|e| format!("read {}: {e}", docs_dir.display()))?
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == "md"))
        .collect();
    doc_files.sort();
    let mut blocks = Vec::new();
    let mut failures = Vec::new();
    for doc in doc_files {
        let stem = doc
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("doc")
            .to_string();
        let text =
            std::fs::read_to_string(&doc).map_err(|e| format!("read {}: {e}", doc.display()))?;
        let mut in_block = false;
        let mut start_line = 0usize;
        let mut body = String::new();
        for (i, line) in text.lines().enumerate() {
            if !in_block {
                if line.trim_end() == "```wrela" {
                    in_block = true;
                    start_line = i + 2; // first line of the block body
                    body.clear();
                }
            } else if line.trim_end() == "```" {
                in_block = false;
                let name = format!("{stem}-{start_line}.wr");
                blocks.push(DocBlock {
                    doc: doc.clone(),
                    start_line,
                    name,
                    body: body.clone(),
                });
            } else {
                body.push_str(line);
                body.push('\n');
            }
        }
        if in_block {
            failures.push(format!("{}: unterminated ```wrela block", doc.display()));
        }
    }
    Ok((blocks, failures))
}

/// Every `.wr` file under docs/language/examples/, whole-file, in
/// deterministic (sorted) order — additional corpus entries alongside the
/// ```wrela doc blocks (plans/M1.md item D, step 5).
fn extract_example_files() -> Result<Vec<DocBlock>, String> {
    let dir = root().join("docs/language/examples");
    let mut files: Vec<_> = std::fs::read_dir(&dir)
        .map_err(|e| format!("read {}: {e}", dir.display()))?
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == "wr"))
        .collect();
    files.sort();
    let mut entries = Vec::new();
    for path in files {
        let body =
            std::fs::read_to_string(&path).map_err(|e| format!("read {}: {e}", path.display()))?;
        let name = path
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("example.wr")
            .to_string();
        entries.push(DocBlock {
            doc: path,
            start_line: 1,
            name,
            body,
        });
    }
    Ok(entries)
}

/// Every ```wrela doc block and every docs/language/examples/*.wr file must
/// lex; from M1 item D on, every one of them must also *parse* — except a
/// block whose body contains the literal substring `...`, which is a doc
/// fragment (an illustrative snippet, not a complete construct) and stays
/// lex-only. `docs.examples.wrela-blocks-parse` is the ledger clause for
/// the parse half; `docs.examples.wrela-blocks-lex` already covered lexing.
fn corpus() -> Result<(), String> {
    let out_dir = root().join("target/corpus");
    std::fs::create_dir_all(&out_dir).map_err(|e| format!("create {}: {e}", out_dir.display()))?;
    let (blocks, mut failures) = extract_doc_blocks()?;
    let examples = extract_example_files()?;
    let mut lexed = 0usize;
    let mut parsed = 0usize;
    let mut fragments = 0usize;
    for b in blocks.into_iter().chain(examples) {
        std::fs::write(out_dir.join(&b.name), &b.body)
            .map_err(|e| format!("write corpus {}: {e}", b.name))?;
        let tokens = match lexer::lex(&b.body) {
            Ok(tokens) => tokens,
            Err(e) => {
                failures.push(format!(
                    "{}:{}: lex error at block line {}:{}: {}",
                    b.doc.display(),
                    b.start_line,
                    e.line,
                    e.col,
                    e.message
                ));
                continue;
            }
        };
        lexed += 1;
        if b.body.contains("...") {
            fragments += 1;
            continue;
        }
        match wrela_compiler::syntax::parser::parse_any(tokens) {
            Ok(_) => parsed += 1,
            Err(e) => failures.push(format!(
                "{}:{}: parse error at block line {}:{}: {}",
                b.doc.display(),
                b.start_line,
                e.line,
                e.col,
                e.message
            )),
        }
    }
    if failures.is_empty() {
        println!("corpus: lexed {lexed}, parsed {parsed}, fragments skipped {fragments}");
        Ok(())
    } else {
        for f in &failures {
            eprintln!("{f}");
        }
        Err(format!("corpus: {} failure(s)", failures.len()))
    }
}

// --- fuzz -------------------------------------------------------------
//
// Deterministic in-tree fuzzing (plans/M1.md, shape decision 6): a seeded
// splitmix64 generator drives two strategies — arbitrary byte strings
// biased toward the bytes that actually drive the lexer's branches (ASCII
// printables, newlines, quotes, braces, backslashes, digits, and 4-space
// runs so indentation paths are not left to chance, plus occasional raw
// non-ASCII bytes), and byte-level mutations of the same corpus `xtask
// corpus` already lexes (every ```wrela doc block, plus every golden
// `input.wr`). No external fuzzing engine (cargo-fuzz/libFuzzer): nightly
// plus an external engine is a liability this project does not need while
// the dumb fuzzer keeps finding bugs.
//
// Every candidate is sanitized with `String::from_utf8_lossy` before it
// reaches the lexer (`lex` takes `&str`; a stray invalid byte becomes
// U+FFFD, which is itself a non-ASCII byte sequence, so the "raw
// non-ASCII byte" path still gets exercised deterministically without
// ever handing the lexer a string it was never contracted to accept).
//
// Invariants checked every iteration: never panics; the result is
// `Ok(tokens)` or one `LexError`; on `Ok`, the last token is `Eof` and no
// earlier token is; INDENT count equals DEDENT count; token lines are
// monotonically non-decreasing; and lexing the same input twice gives
// identical output. A find writes the exact input to
// `target/fuzz/crash-<n>.wr` and reports the seed + iteration so it
// reproduces; every find must be minimized by hand into a
// `tests/golden/lex-fuzz-*` case before the underlying bug is fixed.

const FUZZ_LEXER_DEEP_ITERS: u64 = 200_000;
const FUZZ_LEXER_DEEP_SEED: u64 = 1;
// Wired into `check` (after corpus, before ledger): two fixed seeds, 1_000
// iterations each, so the gate stays well under a second and fully
// deterministic — no seed ever comes from the clock or the environment.
const FUZZ_LEXER_SMOKE_SEEDS: &[u64] = &[1, 2];
const FUZZ_LEXER_SMOKE_ITERS_PER_SEED: u64 = 1_000;

/// splitmix64: the entire PRNG. No external crate — a fuzzer this dumb
/// does not need one, and determinism-by-construction (ROADMAP.md) means
/// the generator itself must never change behavior across platforms.
struct Rng(u64);

impl Rng {
    fn new(seed: u64) -> Self {
        Rng(seed)
    }

    fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// Uniform in `[0, n)`. `n` must be nonzero.
    fn gen_range(&mut self, n: usize) -> usize {
        (self.next_u64() % n as u64) as usize
    }
}

fn fuzz(args: &[String]) -> Result<(), String> {
    let (target, rest) = match args.first() {
        Some(a)
            if a == "lexer"
                || a == "parser"
                || a == "sema"
                || a == "eval"
                || a == "lower"
                || a == "async" =>
        {
            (a.as_str(), &args[1..])
        }
        _ => ("lexer", args),
    };
    match target {
        "lexer" => {
            let iters = parse_flag_u64(rest, "--iters")?.unwrap_or(FUZZ_LEXER_DEEP_ITERS);
            let seed = parse_flag_u64(rest, "--seed")?.unwrap_or(FUZZ_LEXER_DEEP_SEED);
            fuzz_lexer(iters, seed)
        }
        "parser" => {
            let iters = parse_flag_u64(rest, "--iters")?.unwrap_or(FUZZ_PARSER_DEEP_ITERS);
            let seed = parse_flag_u64(rest, "--seed")?.unwrap_or(FUZZ_PARSER_DEEP_SEED);
            fuzz_parser(iters, seed)
        }
        "sema" => {
            let iters = parse_flag_u64(rest, "--iters")?.unwrap_or(FUZZ_SEMA_DEEP_ITERS);
            let seed = parse_flag_u64(rest, "--seed")?.unwrap_or(FUZZ_SEMA_DEEP_SEED);
            fuzz_sema(iters, seed)
        }
        "eval" => {
            let iters = parse_flag_u64(rest, "--iters")?.unwrap_or(FUZZ_EVAL_DEEP_ITERS);
            let seed = parse_flag_u64(rest, "--seed")?.unwrap_or(FUZZ_EVAL_DEEP_SEED);
            fuzz_eval(iters, seed)
        }
        "lower" => {
            let iters = parse_flag_u64(rest, "--iters")?.unwrap_or(FUZZ_LOWER_DEEP_ITERS);
            let seed = parse_flag_u64(rest, "--seed")?.unwrap_or(FUZZ_LOWER_DEEP_SEED);
            fuzz_lower(iters, seed)
        }
        "async" => {
            let iters = parse_flag_u64(rest, "--iters")?.unwrap_or(FUZZ_ASYNC_DEEP_ITERS);
            let seed = parse_flag_u64(rest, "--seed")?.unwrap_or(FUZZ_ASYNC_DEEP_SEED);
            fuzz_async(iters, seed)
        }
        other => Err(format!(
            "fuzz: unknown target `{other}` (expected `lexer`, `parser`, `sema`, `eval`, \
             `lower`, or `async`)"
        )),
    }
}

fn parse_flag_u64(args: &[String], flag: &str) -> Result<Option<u64>, String> {
    let with_eq = format!("{flag}=");
    for (i, a) in args.iter().enumerate() {
        if let Some(v) = a.strip_prefix(&with_eq) {
            return v
                .parse::<u64>()
                .map(Some)
                .map_err(|e| format!("{flag}: {e}"));
        }
        if a == flag {
            let v = args
                .get(i + 1)
                .ok_or_else(|| format!("{flag}: missing value"))?;
            return v
                .parse::<u64>()
                .map(Some)
                .map_err(|e| format!("{flag}: {e}"));
        }
    }
    Ok(None)
}

/// plans/M4.md item E: the four project-shaped golden cases whose own
/// module files are worth the extra fuzz-seed weight — `appliance`/
/// `image-project` for the builder-intrinsic/`@image` shapes the
/// single-file `image-basic`/`image-helper-accept` seeds (already walked
/// via their own `input.wr`, in `corpus_seed_inputs` below) don't fully
/// cover on their own, `multi-module-accept`/`import-cycle-accept` for the
/// loader's own multi-module import machinery. A fixed, named list rather
/// than a second blanket directory walk: most `tests/golden/*` project
/// fixtures are `err-import-*`/`err-image-*` cases whose whole point is
/// one specific rejection, not extra mutation-worthy surface.
const PROJECT_SEED_CASES: &[&str] = &[
    "appliance",
    "image-project",
    "multi-module-accept",
    "import-cycle-accept",
];

/// Every `.wr` file under `dir`, walked recursively (`multi-module-accept`'s
/// own `src/app/lib/constants.wr` needs this — the other three project
/// seed cases happen to be flat, but the walk does not assume that),
/// appended to `out` in whatever order `read_dir` gives — sorted by the
/// caller, not here, so this stays a pure collector.
fn collect_wr_files(dir: &Path, out: &mut Vec<PathBuf>) -> Result<(), String> {
    for entry in std::fs::read_dir(dir).map_err(|e| format!("read {}: {e}", dir.display()))? {
        let entry = entry.map_err(|e| format!("read {}: {e}", dir.display()))?;
        let path = entry.path();
        if path.is_dir() {
            collect_wr_files(&path, out)?;
        } else if path.extension().and_then(|e| e.to_str()) == Some("wr") {
            out.push(path);
        }
    }
    Ok(())
}

/// plans/M4.md item E: every `src/*.wr` file (recursively) belonging to
/// `PROJECT_SEED_CASES`, in deterministic (sorted-by-path, then
/// sorted-by-case-name) order — each fed to the fuzzer as its own
/// standalone seed input, one *module* at a time, never assembled back
/// into a whole closure (the plan's own "do not wire multi-module
/// closures into the fuzzer itself — that is future work" line). A
/// mutation of one of these files that carries a `from ... import ...`
/// line fails closed at `sema::check_typed` exactly like any other
/// unresolvable import would (an honest, already-covered `SemaErr`
/// outcome, in the fixed `SEMA_CATEGORIES` set) — not a bug this lane
/// needs to work around; the real mutation value here is each file's own
/// `@image`/builder-intrinsic-bearing *shape*.
fn project_seed_inputs() -> Result<Vec<String>, String> {
    let golden_dir = root().join("tests/golden");
    let mut inputs = Vec::new();
    for case in PROJECT_SEED_CASES {
        let src_dir = golden_dir.join(case).join("src");
        let mut files = Vec::new();
        collect_wr_files(&src_dir, &mut files)?;
        files.sort();
        for f in files {
            inputs.push(
                std::fs::read_to_string(&f).map_err(|e| format!("read {}: {e}", f.display()))?,
            );
        }
    }
    Ok(inputs)
}

/// Every input `xtask corpus` already lexes: doc blocks plus golden
/// `input.wr` files, in deterministic (sorted) order — plus, plans/M4.md
/// item E, every project-shaped seed module `project_seed_inputs` above
/// collects. This is the corpus half of the fuzzer's mutation strategy
/// and reuses `extract_doc_blocks` rather than re-walking the docs.
fn corpus_seed_inputs() -> Result<Vec<String>, String> {
    let (blocks, failures) = extract_doc_blocks()?;
    if let Some(f) = failures.first() {
        return Err(format!("fuzz: corpus is broken, fix it first: {f}"));
    }
    let mut inputs: Vec<String> = blocks.into_iter().map(|b| b.body).collect();
    let golden_dir = root().join("tests/golden");
    for dir in golden_case_dirs(&golden_dir)? {
        let input = dir.join("input.wr");
        if input.exists() {
            inputs.push(
                std::fs::read_to_string(&input)
                    .map_err(|e| format!("read {}: {e}", input.display()))?,
            );
        }
    }
    inputs.extend(project_seed_inputs()?);
    if inputs.is_empty() {
        return Err("fuzz: no seed inputs (doc corpus and golden inputs are both empty)".into());
    }
    Ok(inputs)
}

/// One byte, weighted toward what actually drives the lexer's branches:
/// ASCII identifier/digit characters, source punctuation and operators,
/// newline, space, quotes, backslash, `#`, tab (the lexer's own reject
/// path), and occasionally a raw non-ASCII byte (0x80..=0xFF) — invalid
/// alone, but sanitized to a still-non-ASCII replacement char before it
/// ever reaches `lex` (see the module doc above).
fn random_byte(rng: &mut Rng) -> u8 {
    const WORD: &[u8] = b"abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789_";
    const PUNCT: &[u8] = b"+-*/%&|^~<>=(),.:?@!$;[]{}";
    const QUOTES: &[u8] = b"\"'";
    match rng.gen_range(100) {
        0..=39 => WORD[rng.gen_range(WORD.len())],
        40..=54 => PUNCT[rng.gen_range(PUNCT.len())],
        55..=64 => b'\n',
        65..=74 => b' ',
        75..=80 => QUOTES[rng.gen_range(QUOTES.len())],
        81..=85 => b'\\',
        86..=90 => b'#',
        91..=94 => b'\t',
        _ => (0x80 + rng.gen_range(0x80)) as u8,
    }
}

/// Arbitrary byte string, strategy 1: mostly single bytes from
/// `random_byte`, with a 15% chance per step of emitting a whole 4-space
/// run instead, so INDENT/DEDENT paths are not left to chance.
fn random_input(rng: &mut Rng) -> Vec<u8> {
    let target_len = rng.gen_range(400);
    let mut buf = Vec::with_capacity(target_len);
    while buf.len() < target_len {
        if rng.gen_range(100) < 15 {
            buf.extend_from_slice(b"    ");
        } else {
            buf.push(random_byte(rng));
        }
    }
    buf
}

/// Corpus mutation, strategy 2: 1-4 random edits (flip, insert, delete,
/// truncate, splice-in-a-slice-from-another-seed) on a real doc/golden
/// input, so the fuzzer spends most of its budget near inputs the lexer is
/// supposed to accept rather than only in the wholly-random tail.
fn mutate_seed_input(rng: &mut Rng, seed_inputs: &[String]) -> Vec<u8> {
    mutate_seed_input_from(rng, seed_inputs, seed_inputs)
}

/// `mutate_seed_input` with the *base* population and the *splice-donor*
/// population named separately (plans/M7.md item Y): the async lane wants
/// every base to be an async/actor-shaped golden while still occasionally
/// splicing in a slice of the wider corpus, so a mutation can carry a
/// generic fn, a `defer`, or a `for ... take` into an actor program. Every
/// existing caller passes the same slice twice (`mutate_seed_input` above),
/// which consumes the RNG in exactly the order it always did — no existing
/// lane's seed changes meaning.
fn mutate_seed_input_from(rng: &mut Rng, bases: &[String], donors: &[String]) -> Vec<u8> {
    let mut bytes = bases[rng.gen_range(bases.len())].as_bytes().to_vec();
    let ops = 1 + rng.gen_range(4);
    for _ in 0..ops {
        if bytes.is_empty() {
            bytes.push(random_byte(rng));
            continue;
        }
        match rng.gen_range(5) {
            0 => {
                let i = rng.gen_range(bytes.len());
                bytes[i] = random_byte(rng);
            }
            1 => {
                let i = rng.gen_range(bytes.len() + 1);
                bytes.insert(i, random_byte(rng));
            }
            2 => {
                let i = rng.gen_range(bytes.len());
                bytes.remove(i);
            }
            3 => {
                let i = 1 + rng.gen_range(bytes.len());
                bytes.truncate(i);
            }
            _ => {
                let other = donors[rng.gen_range(donors.len())].as_bytes();
                if !other.is_empty() {
                    let start = rng.gen_range(other.len());
                    let end = start + rng.gen_range(other.len() - start + 1);
                    let i = rng.gen_range(bytes.len() + 1);
                    bytes.splice(i..i, other[start..end].iter().copied());
                }
            }
        }
    }
    bytes
}

/// Every invariant the fuzzer checks, once per iteration, on one input.
/// Lexes twice under `catch_unwind` (a panic is a finding, not a crash) so
/// the determinism invariant and the no-panic invariant share one call
/// shape.
fn check_lex_invariants(input: &str) -> Result<(), String> {
    let first = std::panic::catch_unwind(|| lexer::lex(input))
        .map_err(|p| format!("lexer panicked: {}", panic_message(&p)))?;
    let second = std::panic::catch_unwind(|| lexer::lex(input))
        .map_err(|p| format!("lexer panicked on a repeat call: {}", panic_message(&p)))?;
    match (&first, &second) {
        (Ok(t1), Ok(t2)) => {
            if !tokens_equal(t1, t2) {
                return Err(
                    "lexing is not deterministic: two runs produced different tokens".into(),
                );
            }
            check_ok_invariants(t1)
        }
        (Err(e1), Err(e2)) => {
            if e1.message != e2.message || e1.line != e2.line || e1.col != e2.col {
                return Err(
                    "lexing is not deterministic: two runs produced different errors".into(),
                );
            }
            Ok(())
        }
        _ => Err("lexing is not deterministic: one run errored and the other did not".into()),
    }
}

fn panic_message(payload: &(dyn std::any::Any + Send)) -> String {
    if let Some(s) = payload.downcast_ref::<&str>() {
        s.to_string()
    } else if let Some(s) = payload.downcast_ref::<String>() {
        s.clone()
    } else {
        "non-string panic payload".to_string()
    }
}

fn tokens_equal(a: &[Token], b: &[Token]) -> bool {
    a.len() == b.len()
        && a.iter().zip(b).all(|(x, y)| {
            x.kind == y.kind && x.text == y.text && x.line == y.line && x.col == y.col
        })
}

fn check_ok_invariants(tokens: &[Token]) -> Result<(), String> {
    if !matches!(tokens.last(), Some(t) if t.kind == TokenKind::Eof) {
        return Err("last token is not Eof".into());
    }
    if tokens[..tokens.len() - 1]
        .iter()
        .any(|t| t.kind == TokenKind::Eof)
    {
        return Err("Eof token appears before the end of the stream".into());
    }
    let indents = tokens
        .iter()
        .filter(|t| t.kind == TokenKind::Indent)
        .count();
    let dedents = tokens
        .iter()
        .filter(|t| t.kind == TokenKind::Dedent)
        .count();
    if indents != dedents {
        return Err(format!(
            "INDENT/DEDENT imbalance: {indents} indent(s), {dedents} dedent(s)"
        ));
    }
    let mut last_line = 0u32;
    for t in tokens {
        if t.line < last_line {
            return Err(format!(
                "token line went backwards: {}:{} after line {last_line}",
                t.line, t.col
            ));
        }
        last_line = t.line;
    }
    Ok(())
}

fn run_lexer_fuzz(iters: u64, seed: u64, seed_inputs: &[String]) -> Result<(), String> {
    let mut rng = Rng::new(seed);
    for i in 0..iters {
        let bytes = if i % 2 == 0 {
            random_input(&mut rng)
        } else {
            mutate_seed_input(&mut rng, seed_inputs)
        };
        let input = String::from_utf8_lossy(&bytes).into_owned();
        if let Err(reason) = check_lex_invariants(&input) {
            return report_fuzz_failure("lexer", "crash-", seed, i, &input, &reason);
        }
    }
    println!("fuzz lexer: {iters} iteration(s) clean (seed={seed})");
    Ok(())
}

fn report_fuzz_failure(
    label: &str,
    prefix: &str,
    seed: u64,
    iter: u64,
    input: &str,
    reason: &str,
) -> Result<(), String> {
    let dir = root().join("target/fuzz");
    std::fs::create_dir_all(&dir).map_err(|e| format!("create {}: {e}", dir.display()))?;
    let mut n = 0usize;
    let path = loop {
        let p = dir.join(format!("{prefix}{n}.wr"));
        if !p.exists() {
            break p;
        }
        n += 1;
    };
    std::fs::write(&path, input).map_err(|e| format!("write {}: {e}", path.display()))?;
    Err(format!(
        "fuzz {label}: seed={seed} iteration={iter}: {reason}\n  input written to {}",
        path.display()
    ))
}

/// Silences the default panic hook (which would otherwise print a full
/// "thread panicked at ..." backtrace to stderr for every finding) for the
/// duration of a fuzz run; a panic is still caught and reported explicitly
/// by `check_lex_invariants`/`report_fuzz_failure`, just without the
/// noise. Always restores the previous hook, even when the run fails.
fn with_silenced_panic_hook<F: FnOnce() -> Result<(), String>>(f: F) -> Result<(), String> {
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(|_| {}));
    let result = f();
    std::panic::set_hook(previous);
    result
}

fn fuzz_lexer(iters: u64, seed: u64) -> Result<(), String> {
    let seed_inputs = corpus_seed_inputs()?;
    with_silenced_panic_hook(|| run_lexer_fuzz(iters, seed, &seed_inputs))
}

fn fuzz_lexer_smoke() -> Result<(), String> {
    let seed_inputs = corpus_seed_inputs()?;
    with_silenced_panic_hook(|| {
        for &seed in FUZZ_LEXER_SMOKE_SEEDS {
            run_lexer_fuzz(FUZZ_LEXER_SMOKE_ITERS_PER_SEED, seed, &seed_inputs)?;
        }
        Ok(())
    })
}

// --- fuzz: parser -----------------------------------------------------
//
// plans/M1.md item E ("parser hardening"). Two strategies, mirroring the
// lexer fuzzer's shape exactly (same `Rng`, same corpus seed inputs):
//
//  1. corpus mutation (`mutate_seed_input`, already shared with the lexer
//     fuzzer) fed through the same lex-then-parse pipeline `xtask corpus`
//     itself uses (`parser::parse_any`, which picks the fragment entry
//     point when the input has no `module` header);
//  2. token-soup (`token_soup` below): builds random-but-lexable *text* by
//     sampling a vocabulary of keywords, identifiers, literals, operators,
//     newlines, and 4-space indent units — never `Token` structs directly,
//     so the real lexer stays in the loop.
//
// Invariants checked every iteration, on the whole lex-then-parse
// pipeline: never panics; the result is a successful parse (module or
// fragment) or exactly one error (from either stage); running the same
// input through the pipeline twice gives an identical outcome (same AST
// dump, or the same error stage/message/line/col). A find writes the input
// to `target/fuzz/parse-crash-<n>.wr` and reports the seed + iteration so
// it reproduces; every find is minimized by hand into a
// `tests/golden/parse-fuzz-*` case before the underlying bug is fixed.

const FUZZ_PARSER_DEEP_ITERS: u64 = 100_000;
const FUZZ_PARSER_DEEP_SEED: u64 = 1;
const FUZZ_PARSER_SMOKE_SEEDS: &[u64] = &[1, 2];
const FUZZ_PARSER_SMOKE_ITERS_PER_SEED: u64 = 1_000;

/// One full run of the pipeline the parser fuzzer exercises: lex, then (on
/// success) parse via `parse_any`. Exactly one of these four shapes comes
/// back — never a panic, per `check_parse_invariants`'s `catch_unwind`.
enum PipelineOutcome {
    /// A successful parse (module or fragment), reduced to its dump (with
    /// spans — determinism means the *same* input reproduces byte-
    /// identical spans too, not just the same tree shape).
    Ok(String),
    LexErr {
        message: String,
        line: u32,
        col: u32,
    },
    ParseErr {
        message: String,
        line: u32,
        col: u32,
    },
}

fn run_pipeline_once(input: &str) -> PipelineOutcome {
    match lexer::lex(input) {
        Err(e) => PipelineOutcome::LexErr {
            message: e.message,
            line: e.line,
            col: e.col,
        },
        Ok(tokens) => match parser::parse_any(tokens) {
            Ok(Parsed::Module(m)) => PipelineOutcome::Ok(parser::dump(&m)),
            Ok(Parsed::Fragment(entries)) => PipelineOutcome::Ok(parser::dump_fragment(&entries)),
            Err(e) => PipelineOutcome::ParseErr {
                message: e.message,
                line: e.line,
                col: e.col,
            },
        },
    }
}

/// Every invariant the parser fuzzer checks, once per iteration, on one
/// input. Runs the whole lex-then-parse pipeline twice under
/// `catch_unwind` (a panic in either stage is a finding), mirroring
/// `check_lex_invariants`'s shape.
fn check_parse_invariants(input: &str) -> Result<(), String> {
    let first = std::panic::catch_unwind(|| run_pipeline_once(input))
        .map_err(|p| format!("parser panicked: {}", panic_message(&p)))?;
    let second = std::panic::catch_unwind(|| run_pipeline_once(input))
        .map_err(|p| format!("parser panicked on a repeat call: {}", panic_message(&p)))?;
    match (&first, &second) {
        (PipelineOutcome::Ok(d1), PipelineOutcome::Ok(d2)) => {
            if d1 != d2 {
                return Err(
                    "parsing is not deterministic: two runs produced different ASTs".into(),
                );
            }
            Ok(())
        }
        (
            PipelineOutcome::LexErr {
                message: m1,
                line: l1,
                col: c1,
            },
            PipelineOutcome::LexErr {
                message: m2,
                line: l2,
                col: c2,
            },
        ) => {
            if m1 != m2 || l1 != l2 || c1 != c2 {
                return Err(
                    "parsing is not deterministic: two runs produced different lex errors".into(),
                );
            }
            Ok(())
        }
        (
            PipelineOutcome::ParseErr {
                message: m1,
                line: l1,
                col: c1,
            },
            PipelineOutcome::ParseErr {
                message: m2,
                line: l2,
                col: c2,
            },
        ) => {
            if m1 != m2 || l1 != l2 || c1 != c2 {
                return Err(
                    "parsing is not deterministic: two runs produced different parse errors".into(),
                );
            }
            Ok(())
        }
        _ => Err(
            "parsing is not deterministic: the two runs disagreed on success/failure or which \
             stage failed"
                .into(),
        ),
    }
}

/// Token-soup, strategy 2: builds random-but-lexable *text* (never `Token`
/// structs) by sampling a vocabulary of real wrela tokens. At the start of
/// a line, occasionally emits 0-3 four-space indent units so INDENT/DEDENT
/// paths are exercised; otherwise samples one token (keyword, identifier,
/// int/float/string literal, or operator) and separates pieces with a
/// single space so tokens never accidentally glue together (`1` next to
/// `0` must stay two tokens unless the fuzzer means to test `10`).
fn token_soup(rng: &mut Rng) -> String {
    const IDENTS: &[&str] = &[
        "x", "y", "foo", "bar", "self", "counter", "Widget", "T", "_",
    ];
    const INT_LITS: &[&str] = &["0", "1", "42", "0x1000_0000", "0b101", "0o17", "1_000"];
    const FLOAT_LITS: &[&str] = &["1.0", "0.5e10", "3.14", "2e-3"];
    const STR_LITS: &[&str] = &["\"hi\"", "\"\"", "f\"{x}\"", "b\"\\x00\""];
    const OPERATORS: &[&str] = &[
        "+", "-", "*", "/", "%", "&", "|", "^", "~", "<", ">", "=", "(", ")", "[", "]", "{", "}",
        ",", ":", ".", "?", "@", ";", "->", "..", "..=", "<<", ">>", "<=", ">=", "==", "!=", "+=",
        "-=", "*=", "/=", "%=", "&=", "|=", "^=", "+%", "-%", "*%", "<<=", ">>=",
    ];

    let piece_count = 1 + rng.gen_range(80);
    let mut out = String::new();
    let mut at_line_start = true;
    for _ in 0..piece_count {
        if at_line_start && rng.gen_range(100) < 40 {
            let levels = rng.gen_range(4);
            for _ in 0..levels {
                out.push_str("    ");
            }
            at_line_start = false;
            continue;
        }
        match rng.gen_range(100) {
            0..=24 => out.push_str(lexer::KEYWORDS[rng.gen_range(lexer::KEYWORDS.len())]),
            25..=44 => out.push_str(IDENTS[rng.gen_range(IDENTS.len())]),
            45..=54 => out.push_str(INT_LITS[rng.gen_range(INT_LITS.len())]),
            55..=59 => out.push_str(FLOAT_LITS[rng.gen_range(FLOAT_LITS.len())]),
            60..=64 => out.push_str(STR_LITS[rng.gen_range(STR_LITS.len())]),
            65..=89 => out.push_str(OPERATORS[rng.gen_range(OPERATORS.len())]),
            90..=97 => {
                out.push('\n');
                at_line_start = true;
                continue;
            }
            _ => {
                out.push(' ');
                continue;
            }
        }
        out.push(' ');
        at_line_start = false;
    }
    out
}

fn run_parser_fuzz(iters: u64, seed: u64, seed_inputs: &[String]) -> Result<(), String> {
    let mut rng = Rng::new(seed);
    for i in 0..iters {
        let input = if i % 2 == 0 {
            String::from_utf8_lossy(&mutate_seed_input(&mut rng, seed_inputs)).into_owned()
        } else {
            token_soup(&mut rng)
        };
        if let Err(reason) = check_parse_invariants(&input) {
            return report_fuzz_failure("parser", "parse-crash-", seed, i, &input, &reason);
        }
    }
    println!("fuzz parser: {iters} iteration(s) clean (seed={seed})");
    Ok(())
}

fn fuzz_parser(iters: u64, seed: u64) -> Result<(), String> {
    let seed_inputs = corpus_seed_inputs()?;
    with_silenced_panic_hook(|| run_parser_fuzz(iters, seed, &seed_inputs))
}

fn fuzz_parser_smoke() -> Result<(), String> {
    let seed_inputs = corpus_seed_inputs()?;
    with_silenced_panic_hook(|| {
        for &seed in FUZZ_PARSER_SMOKE_SEEDS {
            run_parser_fuzz(FUZZ_PARSER_SMOKE_ITERS_PER_SEED, seed, &seed_inputs)?;
        }
        Ok(())
    })
}

// --- fuzz: sema ---------------------------------------------------------
//
// plans/M2.md item I ("hardening + measurement"). Exactly the parser
// fuzzer's two strategies and the same corpus seed inputs
// (`corpus_seed_inputs`, `mutate_seed_input`, `token_soup`) — that seed
// set already includes every `tests/golden/*/input.wr`, which is what
// makes it the interesting one here: it includes the sema-*valid*
// `check-*` programs, not just syntax the lexer/parser alone would
// generate. One more stage is added to the pipeline: lex, then (on
// success) parse via `parser::parse` (sema operates on a whole `Module`;
// there is no fragment entry point for it, unlike the parser fuzzer's
// `parse_any`), then (on success) `sema::check`.
//
// Invariants checked every iteration, under `catch_unwind` (a panic in
// any stage is a finding): sema never panics; the outcome is a successful
// dump or exactly one `SemaError` whose `category` is one of the fixed
// set plans/M2.md decision 1 names (`name`, `type`, `access`, `move`,
// `init`, `overlap`, `match`, `generic`, `unimplemented`) — any other
// category string is itself a bug; running the whole pipeline twice on
// the same input gives an identical outcome (same dump, or the same
// error stage/category/message/line/col); and on success, `sema::dump`
// itself does not panic and is byte-identical across two separate calls
// (checked in addition to, not instead of, the two full-pipeline runs
// above, since `check`'s dumb re-run-declare-inside-dump shape means dump
// has its own chance to misbehave independently of `check`). A find
// writes to `target/fuzz/sema-crash-<n>.wr` and reports the seed +
// iteration so it reproduces; every find is minimized by hand into a
// `tests/golden/sema-fuzz-*` case before the underlying bug is fixed.
//
// Two more invariants (ledger clause sema.check.roundtrip-stable,
// `check_sema_roundtrip_and_rotation`/`_guarded`, defined further down
// this file next to the shared `sema_outcome_summary`/
// `sema_outcomes_agree` comparison machinery `xtask roundtrip` also
// uses), checked once more per iteration whenever the input lexes and
// parses (regardless of whether sema then accepts or rejects it): sema
// roundtrip stability (pretty-print the parsed module, reparse it, recheck
// — the two sema outcomes must agree, per that machinery's comparison
// rule) and item-rotation acceptance invariance (module-scope
// declarations are order-independent by construction — collect-then-
// resolve — so rotating the top-level items by one and rechecking must
// not flip Ok to Err or vice versa, even though the dump/diagnostic
// content is allowed to change). Findings from either reuse the same
// `target/fuzz/sema-crash-<n>.wr` reporting path.

// Measured on the authoring machine (debug build), before the roundtrip +
// item-rotation invariants below existed: ~38-39us/iteration (500_000
// iters in ~19.4s, 2_000_000 in ~78s) — sema's extra lex+parse+three-pass-
// pipeline+dump work per iteration over the parser fuzzer's lex+parse is
// real but not dramatic, since most mutated/soup inputs fail out at lex or
// parse and never reach a pass.
//
// Re-measured after adding `check_sema_roundtrip_and_rotation_guarded`
// (ledger clause sema.check.roundtrip-stable — a second lex+parse+check
// pass to recover the parsed module, a pretty-print+reparse+recheck for
// the roundtrip oracle, and a clone+recheck for the item-rotation oracle,
// on every iteration whose input parses): ~61us/iteration (500_000 iters
// in ~31.0s, 2_000_000 in ~125.3s) — roughly 1.6x the old per-iteration
// cost, not the full 2x a naive doubling would predict, again because
// most iterations never reach a parseable module at all. 2_000_000 still
// lands a bare `cargo xtask fuzz sema` at a bit over two minutes, inside
// the "roughly a minute or two"/1-3 minute band plans/M2.md item I and
// this ledger clause both target, so the deep default is unchanged.
const FUZZ_SEMA_DEEP_ITERS: u64 = 2_000_000;
const FUZZ_SEMA_DEEP_SEED: u64 = 1;
const FUZZ_SEMA_SMOKE_SEEDS: &[u64] = &[1, 2];
const FUZZ_SEMA_SMOKE_ITERS_PER_SEED: u64 = 1_000;

/// The fixed diagnostic-category set plans/M2.md decision 1 names, plus
/// `comptime` (plans/M3.md item B: the evaluator's own abandonment/quota
/// build errors, surfaced through `sema::check` since it now runs const
/// initializers through the real evaluator). Any `SemaError` whose
/// category is not in this list is itself an invariant violation, not a
/// legitimate rejection.
const SEMA_CATEGORIES: &[&str] = &[
    "name",
    "type",
    "access",
    "move",
    "init",
    "overlap",
    "match",
    "generic",
    "unimplemented",
    "comptime",
    // plans/M4.md item A: the loader's own diagnostics (root-file/
    // module-path disagreement, a missing file for an imported module
    // path) — deliberately added, not discovered by the fuzzer (the
    // fuzzer never drives the loader; it exercises `sema::check`
    // directly on a single fuzzed file).
    "build",
    // plans/M6.md item A: the actor surface's own diagnostics (message-
    // value restrictions, the bare-`send`-statement floor, ...) —
    // 02-language.md §9's own vocabulary, deliberately added like `build`
    // above.
    "actor",
];

/// One full run of the pipeline the sema fuzzer exercises: lex, then (on
/// success) parse a whole module, then (on success) `sema::check`.
/// Exactly one of these four shapes comes back — never a panic, per
/// `check_sema_invariants`'s `catch_unwind`.
enum SemaPipelineOutcome {
    /// A successful `check`, reduced to its dump (determinism means the
    /// *same* input reproduces a byte-identical dump too).
    Ok(String),
    LexErr {
        message: String,
        line: u32,
        col: u32,
    },
    ParseErr {
        message: String,
        line: u32,
        col: u32,
    },
    SemaErr {
        category: &'static str,
        message: String,
        line: u32,
        col: u32,
        /// Item H's one multi-line exception (decision 2): empty/`false`
        /// for every ordinary diagnostic, so this adds no new invariant
        /// shape, only extends the existing determinism check to also
        /// cover the generic-instantiation chain's extra lines.
        extra_lines: Vec<String>,
        omit_location: bool,
    },
}

fn run_sema_pipeline_once(input: &str) -> SemaPipelineOutcome {
    match lexer::lex(input) {
        Err(e) => SemaPipelineOutcome::LexErr {
            message: e.message,
            line: e.line,
            col: e.col,
        },
        Ok(tokens) => match parser::parse(tokens) {
            Err(e) => SemaPipelineOutcome::ParseErr {
                message: e.message,
                line: e.line,
                col: e.col,
            },
            // "<fuzz>" is not a real file path: item H's chain diagnostic
            // cites the path verbatim (decision 2), but the fuzzer's
            // determinism check only compares two runs of the *same*
            // input against each other, so any fixed placeholder works.
            Ok(module) => match sema::check(&module, "<fuzz>") {
                Ok(()) => SemaPipelineOutcome::Ok(sema::dump(&module)),
                Err(e) => SemaPipelineOutcome::SemaErr {
                    category: e.category,
                    message: e.message,
                    line: e.line,
                    col: e.col,
                    extra_lines: e.extra_lines,
                    omit_location: e.omit_location,
                },
            },
        },
    }
}

/// Every invariant the sema fuzzer checks, once per iteration, on one
/// input. Runs the whole lex-then-parse-then-check pipeline twice under
/// `catch_unwind`, mirroring `check_parse_invariants`'s shape, plus a
/// direct check that a successful `SemaError` category (when the outcome
/// is instead an error) is one of the fixed set, and that `sema::dump` is
/// itself panic-free and repeat-call-identical on a successful outcome.
fn check_sema_invariants(input: &str) -> Result<(), String> {
    let first = std::panic::catch_unwind(|| run_sema_pipeline_once(input))
        .map_err(|p| format!("sema panicked: {}", panic_message(&p)))?;
    let second = std::panic::catch_unwind(|| run_sema_pipeline_once(input))
        .map_err(|p| format!("sema panicked on a repeat call: {}", panic_message(&p)))?;

    if let SemaPipelineOutcome::SemaErr { category, .. } = &first {
        if !SEMA_CATEGORIES.contains(category) {
            return Err(format!(
                "sema produced an unknown diagnostic category `{category}` (not in the fixed set)"
            ));
        }
    }

    match (&first, &second) {
        (SemaPipelineOutcome::Ok(d1), SemaPipelineOutcome::Ok(d2)) => {
            if d1 != d2 {
                return Err("sema is not deterministic: two runs produced different dumps".into());
            }
            Ok(())
        }
        (
            SemaPipelineOutcome::LexErr {
                message: m1,
                line: l1,
                col: c1,
            },
            SemaPipelineOutcome::LexErr {
                message: m2,
                line: l2,
                col: c2,
            },
        ) => {
            if m1 != m2 || l1 != l2 || c1 != c2 {
                return Err(
                    "sema is not deterministic: two runs produced different lex errors".into(),
                );
            }
            Ok(())
        }
        (
            SemaPipelineOutcome::ParseErr {
                message: m1,
                line: l1,
                col: c1,
            },
            SemaPipelineOutcome::ParseErr {
                message: m2,
                line: l2,
                col: c2,
            },
        ) => {
            if m1 != m2 || l1 != l2 || c1 != c2 {
                return Err(
                    "sema is not deterministic: two runs produced different parse errors".into(),
                );
            }
            Ok(())
        }
        (
            SemaPipelineOutcome::SemaErr {
                category: cat1,
                message: m1,
                line: l1,
                col: c1,
                extra_lines: e1,
                omit_location: o1,
            },
            SemaPipelineOutcome::SemaErr {
                category: cat2,
                message: m2,
                line: l2,
                col: c2,
                extra_lines: e2,
                omit_location: o2,
            },
        ) => {
            if cat1 != cat2 || m1 != m2 || l1 != l2 || c1 != c2 || e1 != e2 || o1 != o2 {
                return Err(
                    "sema is not deterministic: two runs produced different diagnostics".into(),
                );
            }
            Ok(())
        }
        _ => Err(
            "sema is not deterministic: the two runs disagreed on success/failure or which \
             stage failed"
                .into(),
        ),
    }
}

// --- fuzz: sema roundtrip stability + item-rotation invariance ----------
//
// ledger clause sema.check.roundtrip-stable. Two more invariants, checked
// whenever an input parses (regardless of whether sema accepts or rejects
// it) — shared by `fuzz sema` (below) and `xtask roundtrip`
// (`sema_roundtrip_check`, further down this file), since both boil down
// to the same question: do two sema outcomes on what should be an
// equivalent module agree?
//
//  1. Sema roundtrip: A = sema outcome of the parsed module, B = sema
//     outcome of parse(pretty(A's module)). A and B must agree.
//  2. Item-rotation acceptance invariance: module-scope declarations are
//     order-independent (collect-then-resolve). Rotating the module's
//     top-level items by one (first item moved to the end; `imports` is a
//     separate `Module` field and is left untouched) and re-checking must
//     preserve *acceptance* (Ok stays Ok, Err stays Err) even though the
//     dump/diagnostic content may differ.
//
// Comparison rule for "agree" (both here and in the roundtrip oracle): two
// `Ok` outcomes must produce a byte-identical `sema::dump` (decision 8: the
// check dump carries no spans, so this is exact, not approximate); two
// `Err` outcomes must carry the same `category` and `message`. `line`/`col`
// are deliberately never compared — the printer relayouts source
// positions, so they are expected to move. `extra_lines` (item H's
// `required by`/`instantiated at` chain) legitimately carries a position
// too (`" at <path>:<line>"`); since both sides of every comparison here
// are checked with the *same* fixed placeholder path, that suffix is
// stripped before comparing (honestly position-independent: only the
// trailing line number is dropped, everything else in the chain — the
// requirement, the expression rendered, the file path itself — still has
// to match). This is the stricter of the two options the plan allows
// (strip-then-compare vs. drop `extra_lines` from the comparison
// entirely).
enum SemaOutcomeSummary {
    /// A successful check, reduced to its dump.
    Ok(String),
    Err {
        category: &'static str,
        message: String,
        /// `extra_lines` with each line's trailing `" at <path>:<line>"`
        /// stripped (see the module comment above).
        extra_lines: Vec<String>,
        omit_location: bool,
    },
}

/// Runs `sema::check_typed`/`sema::dump_typed` on `module` and reduces the
/// result to the fields `sema_outcomes_agree` compares. plans/M3.md
/// decision 3: the typed-roundtrip oracle (`typed(x) == typed(pretty(parse(x)))`
/// byte-for-byte) replaces the check-dump comparison this used to run —
/// strictly stronger, same machinery (`check_typed` runs the identical
/// pass pipeline `check` does, in the same order, so any `Err` it
/// produces is byte-identical to what plain `check` would have; the `Ok`
/// case now compares the full typed program instead of just the
/// declaration-signature dump).
fn sema_outcome_summary(module: &Module, path: &str) -> SemaOutcomeSummary {
    match sema::check_typed(module, path) {
        Ok(program) => SemaOutcomeSummary::Ok(sema::dump_typed(&program)),
        Err(e) => SemaOutcomeSummary::Err {
            category: e.category,
            message: e.message,
            extra_lines: strip_position_tails(&e.extra_lines, path),
            omit_location: e.omit_location,
        },
    }
}

/// Strips each line's trailing `" at <path>:<line>"` (item H's chain
/// format, sema/generics.rs), leaving everything before it untouched. A
/// line without that exact marker (any ordinary diagnostic — `extra_lines`
/// is empty for those) is returned unchanged.
fn strip_position_tails(lines: &[String], path: &str) -> Vec<String> {
    let marker = format!(" at {path}:");
    lines
        .iter()
        .map(|l| match l.find(&marker) {
            Some(idx) => l[..idx].to_string(),
            None => l.clone(),
        })
        .collect()
}

fn describe_sema_outcome(o: &SemaOutcomeSummary) -> String {
    match o {
        SemaOutcomeSummary::Ok(d) => format!("accepted\n{d}"),
        SemaOutcomeSummary::Err {
            category,
            message,
            extra_lines,
            omit_location,
        } => format!(
            "rejected: [{category}] {message} (extra_lines={extra_lines:?}, omit_location={omit_location})"
        ),
    }
}

/// The comparison rule described in the module comment above. `Ok(())` on
/// agreement; `Err(reason)` describing the disagreement otherwise.
fn sema_outcomes_agree(a: &SemaOutcomeSummary, b: &SemaOutcomeSummary) -> Result<(), String> {
    match (a, b) {
        (SemaOutcomeSummary::Ok(d1), SemaOutcomeSummary::Ok(d2)) => {
            if d1 == d2 {
                Ok(())
            } else {
                Err(format!(
                    "both accept but produced different dumps\n--- a ---\n{d1}\n--- b ---\n{d2}"
                ))
            }
        }
        (
            SemaOutcomeSummary::Err {
                category: c1,
                message: m1,
                extra_lines: e1,
                omit_location: o1,
            },
            SemaOutcomeSummary::Err {
                category: c2,
                message: m2,
                extra_lines: e2,
                omit_location: o2,
            },
        ) => {
            if c1 == c2 && m1 == m2 && e1 == e2 && o1 == o2 {
                Ok(())
            } else {
                Err(format!(
                    "both reject but disagree\n  a: [{c1}] {m1} extra_lines={e1:?} omit_location={o1}\n  b: [{c2}] {m2} extra_lines={e2:?} omit_location={o2}"
                ))
            }
        }
        _ => Err(format!(
            "one run accepted, the other rejected\n  a: {}\n  b: {}",
            describe_sema_outcome(a),
            describe_sema_outcome(b)
        )),
    }
}

/// Item 2 (item-rotation acceptance invariance): clones `module` and moves
/// its first top-level item to the end (`imports` untouched). `None` when
/// there are fewer than two items — rotation is a no-op, so there is
/// nothing to check.
fn rotate_first_item_to_end(module: &Module) -> Option<Module> {
    if module.items.len() < 2 {
        return None;
    }
    let mut rotated = module.clone();
    rotated.items.rotate_left(1);
    Some(rotated)
}

/// The two invariants above, run once per fuzz iteration on an input that
/// lexed and parsed (a lex/parse failure means there is no module to
/// re-check — `Ok(())`, nothing to do; `check_sema_invariants` already
/// covers the lex/parse-error determinism invariants). Uses the same fixed
/// placeholder path both `run_sema_pipeline_once` uses, for the same
/// reason (see its own doc comment): the comparison is between two runs of
/// this fuzzer's own pipeline, never against a golden file, so any fixed
/// string works, and using the same one on every call is what makes
/// `strip_position_tails` honest.
fn check_sema_roundtrip_and_rotation(input: &str) -> Result<(), String> {
    const PATH: &str = "<fuzz>";
    let tokens = match lexer::lex(input) {
        Ok(t) => t,
        Err(_) => return Ok(()),
    };
    let module = match parser::parse(tokens) {
        Ok(m) => m,
        Err(_) => return Ok(()),
    };

    // 1. Sema roundtrip.
    let original = sema_outcome_summary(&module, PATH);
    let pretty = printer::pretty(&module);
    let tokens2 = match lexer::lex(&pretty) {
        Ok(t) => t,
        Err(e) => {
            return Err(format!(
                "sema-roundtrip: pretty-printed output failed to lex: {} at {}:{}\n--- pretty ---\n{pretty}",
                e.message, e.line, e.col
            ));
        }
    };
    let reprinted = match parser::parse(tokens2) {
        Ok(m) => m,
        Err(e) => {
            return Err(format!(
                "sema-roundtrip: pretty-printed output failed to reparse: {} at {}:{}\n--- pretty ---\n{pretty}",
                e.message, e.line, e.col
            ));
        }
    };
    let roundtripped = sema_outcome_summary(&reprinted, PATH);
    sema_outcomes_agree(&original, &roundtripped)
        .map_err(|reason| format!("sema-roundtrip: {reason}"))?;

    // 2. Item-rotation acceptance invariance.
    if let Some(rotated) = rotate_first_item_to_end(&module) {
        let orig_ok = matches!(original, SemaOutcomeSummary::Ok(_));
        let rotated_ok = sema::check(&rotated, PATH).is_ok();
        if orig_ok != rotated_ok {
            return Err(format!(
                "item-rotation: sema {} the original but {} it after rotating module items \
                 by one (order-dependence bug)",
                if orig_ok { "accepted" } else { "rejected" },
                if rotated_ok { "accepted" } else { "rejected" },
            ));
        }
    }

    Ok(())
}

/// `check_sema_roundtrip_and_rotation` under `catch_unwind`, mirroring
/// every other fuzz invariant in this file: a panic here is a finding, not
/// a crash.
fn check_sema_roundtrip_and_rotation_guarded(input: &str) -> Result<(), String> {
    match std::panic::catch_unwind(|| check_sema_roundtrip_and_rotation(input)) {
        Ok(result) => result,
        Err(p) => Err(format!(
            "sema panicked (roundtrip/rotation invariants): {}",
            panic_message(&p)
        )),
    }
}

fn run_sema_fuzz(iters: u64, seed: u64, seed_inputs: &[String]) -> Result<(), String> {
    let mut rng = Rng::new(seed);
    for i in 0..iters {
        let input = if i % 2 == 0 {
            String::from_utf8_lossy(&mutate_seed_input(&mut rng, seed_inputs)).into_owned()
        } else {
            token_soup(&mut rng)
        };
        if let Err(reason) = check_sema_invariants(&input) {
            return report_fuzz_failure("sema", "sema-crash-", seed, i, &input, &reason);
        }
        if let Err(reason) = check_sema_roundtrip_and_rotation_guarded(&input) {
            return report_fuzz_failure("sema", "sema-crash-", seed, i, &input, &reason);
        }
    }
    println!("fuzz sema: {iters} iteration(s) clean (seed={seed})");
    Ok(())
}

fn fuzz_sema(iters: u64, seed: u64) -> Result<(), String> {
    let seed_inputs = corpus_seed_inputs()?;
    with_silenced_panic_hook(|| run_sema_fuzz(iters, seed, &seed_inputs))
}

fn fuzz_sema_smoke() -> Result<(), String> {
    let seed_inputs = corpus_seed_inputs()?;
    with_silenced_panic_hook(|| {
        for &seed in FUZZ_SEMA_SMOKE_SEEDS {
            run_sema_fuzz(FUZZ_SEMA_SMOKE_ITERS_PER_SEED, seed, &seed_inputs)?;
        }
        Ok(())
    })
}

// --- fuzz: eval -----------------------------------------------------------
//
// plans/M3.md item F ("hardening + measurement"). Exactly the sema
// fuzzer's two strategies and the same corpus seed inputs
// (`corpus_seed_inputs`, `mutate_seed_input`, `token_soup`) — the seed set
// already includes every `tests/golden/*/input.wr`, which is what makes
// this lane interesting: the `check-tests-*` goldens are real `@test`-
// bearing programs, so mutating them exercises the evaluator on inputs
// that actually run, not only ones the lexer/parser/sema alone would
// generate. One more stage is added on top of the sema pipeline: lex,
// then (on success) parse a whole module (`parser::parse` — like `fuzz
// sema`, there is no fragment entry point), then `sema::check_typed`
// (which, per `sema::mod::check_typed`'s own doc comment, already runs
// `eval::check_comptime` — every module-level `const` initializer and
// every `comptime assert` — as its own final step; a mutated input that
// merely typechecks has therefore already had its consts/asserts
// evaluated by the time this lane ever sees it), then, on a successful
// typecheck, `eval::run_tests` — every comptime-legal `@test` fn, each
// under its own small fixed quota (`eval::quota::Quota::new()`,
// `MAX_STEPS = 20_000` — already "small" by design, per
// `comptime.eval.quotas`'s own note: kept deliberately far below the
// plan's own "e.g. 1,000,000" suggestion so a single quota-exhausting
// program stays cheap; reused as-is here rather than threading a second,
// fuzz-only quota constant through `run_tests`/`eval_test`, since CLAUDE.md
// rules out a knob nothing else needs).
//
// Invariants checked every iteration, under `catch_unwind` (a panic
// anywhere in `check_typed` or `run_tests` is a finding — this is the
// "never panics anywhere in eval" invariant (a), detected exactly the way
// every other lane in this file detects a panic: `catch_unwind` around
// the call, since the harness runs in-process and a real panic would
// otherwise unwind straight out of `main`):
//
//  (a) never panics (`catch_unwind`, as above);
//  (b) deterministic: the whole pipeline (lex-parse-check_typed, and, on
//      Ok, run_tests) is run twice and the two outcomes are byte-compared
//      — same shape as `check_sema_invariants`/`check_parse_invariants`;
//  (c) always terminates within quota: this is not a separate runtime
//      check (there is no wall clock anywhere in this file or in
//      `eval::quota`, by doctrine) but a structural guarantee — every
//      evaluator loop iteration and call ticks `Quota::tick_step`
//      (`comptime.eval.quotas`), so a diverging comptime program always
//      *returns* (`Ok` or an `EvalError`) once its step budget is spent,
//      rather than looping forever; invariants (a)+(b) are what actually
//      observe this on every iteration, since a hang would simply never
//      report success or failure at all (the fuzz loop itself would
//      stall) — there is nothing further to assert without a wall clock
//      this project's determinism doctrine already rules out;
//  (d) abandonment is always a well-formed diagnostic, never an internal
//      panic message leaking through: on a `check_typed` `Err`, the
//      `SemaError`'s `category` must be one of the fixed
//      `SEMA_CATEGORIES` set (identical check to `fuzz sema`'s own,
//      reused verbatim — `comptime` abandonment from `eval::check_comptime`
//      is already one of that fixed set); on a successful typecheck,
//      `run_tests`'s own report text must match its one pinned shape
//      (`comptime.tests.build-tier`) line for line — `test <name>: ok` or
//      `test <name>: FAILED <message>`, then one `<N> passed, <M> failed`
//      summary line — checked by `report_is_well_formed` below.
//
// A find writes the input to `target/fuzz/eval-crash-<n>.wr` (same
// `report_fuzz_failure` numbering convention every other lane uses — the
// seed and iteration are already in the printed message, so the file name
// itself does not need to embed them) and reports the seed + iteration so
// it reproduces; every find is minimized by hand into a
// `tests/golden/eval-fuzz-*` case before the underlying bug is fixed.

// Measured on the authoring machine (debug build): ~59us/iteration
// (100_000 iters in ~5.9s), essentially identical to `fuzz sema`'s own
// per-iteration cost (~61us, see that lane's own measurement comment
// above) — `check_typed` already pays for everything `sema::check` does
// (it *is* what `sema::check` delegates to, plus it keeps the typed
// program instead of discarding it), and `run_tests` only adds real cost
// on the rare mutation that both fully typechecks *and* still carries a
// `@test` fn, which a fixed, small quota (`comptime.eval.quotas`) bounds
// tightly. 2_000_000 iterations therefore lands in the same "roughly a
// minute or two" band `fuzz sema`'s own deep default targets (plans/M2.md
// item I), so the deep default matches it exactly rather than picking a
// new number for its own sake.
const FUZZ_EVAL_DEEP_ITERS: u64 = 2_000_000;
const FUZZ_EVAL_DEEP_SEED: u64 = 1;
const FUZZ_EVAL_SMOKE_SEEDS: &[u64] = &[1, 2];
const FUZZ_EVAL_SMOKE_ITERS_PER_SEED: u64 = 1_000;

/// One full run of the pipeline the eval fuzzer exercises: lex, then (on
/// success) parse a whole module, then `sema::check_typed`, then (on a
/// successful typecheck) `eval::run_tests`, then — plans/M4.md item E —
/// whenever the typechecked module declares exactly one reachable
/// `@image` fn, the image pipeline too (`run_image_pipeline_once`, below).
/// Exactly one of these four shapes comes back — never a panic, per
/// `check_eval_invariants`'s `catch_unwind`.
enum EvalPipelineOutcome {
    /// A successful typecheck, reduced to `run_tests`'s own report text
    /// (determinism means the *same* input reproduces a byte-identical
    /// report too — including which comptime-legal `@test`s passed,
    /// failed, or hit their quota), plus — plans/M4.md item E — the image
    /// pipeline's own outcome text whenever the module has exactly one
    /// reachable `@image` fn (`None` otherwise: zero or more than one
    /// `@image` fn is not this extension's concern — decision 6's own
    /// diagnostic for "more than one" already renders through the
    /// ordinary `SemaErr`/well-formed-report machinery on a *real*
    /// multi-`@image` build; a single fuzzed file simply never reaches
    /// that shape without a second module, which this fuzzer never
    /// drives).
    Ok(String, Option<String>),
    LexErr {
        message: String,
        line: u32,
        col: u32,
    },
    ParseErr {
        message: String,
        line: u32,
        col: u32,
    },
    SemaErr {
        category: &'static str,
        message: String,
        line: u32,
        col: u32,
        extra_lines: Vec<String>,
        omit_location: bool,
    },
}

/// plans/M4.md item E: whenever `program` (the just-typechecked, single
/// fuzzed module) declares exactly one reachable `@image` fn
/// (`TypedProgram::image_fn`), runs the identical image pipeline `wrela
/// build`/`wrela dump --stage=report` do — `eval::interp::eval_image` ->
/// `eval::image_checks::check_sealed` -> `report::render` — and returns
/// its outcome as one already fully rendered string: either the rendered
/// report itself (`"ImageReport v0\n..."`) or a one-line diagnostic in the
/// exact `error[cat]: message` house style every other stage in this file
/// already prints (`image_outcome_is_well_formed`, below, checks exactly
/// this shape). Returns `None` when the module has no `@image` fn at all
/// — overwhelmingly the common case for both corpus-mutation and
/// token-soup input, so this stays a rare-cost addition exactly like
/// `run_tests` itself already is. Runs under the identical quota
/// discipline every other comptime entry point here already gets
/// (`eval_image` builds its own fresh `Quota::new()` internally, same as
/// `run_call`/`eval_test`) and the identical `catch_unwind`/run-twice
/// coverage `check_eval_invariants` already wraps the whole pipeline in —
/// no second mechanism, this fn is just one more step inside
/// `run_eval_pipeline_once`.
///
/// There is no real file backing a fuzzed input (`mutate_seed_input`/
/// `token_soup` only ever produce in-memory bytes), so the one
/// `report::BuildInput` a report render needs is built from `input`'s own
/// raw bytes directly (`report::sha256_hex(input.as_bytes())`) rather than
/// a real file read — a real hash of the real bytes being evaluated, just
/// not read a second time off disk. `programs` (the map `check_sealed`
/// needs for cross-module init-arg matching) is built with exactly one
/// entry, `program` itself, under its own declared module address — this
/// fuzzer only ever drives a single module, never a whole closure (the
/// plan's own "do not wire multi-module closures into the fuzzer" line),
/// so a real cross-module reference inside `program` simply cannot exist
/// here; `program.clone()` is cheap relative to the rest of this rare
/// path (`TypedProgram` already derives `Clone`).
fn run_image_pipeline_once(
    program: &sema::typed::TypedProgram,
    module_addr: &str,
    input: &str,
) -> Option<String> {
    let fn_name = program.image_fn.clone()?;
    let mut programs = BTreeMap::new();
    programs.insert(module_addr.to_string(), program.clone());
    let text = match eval::interp::eval_image(program, &fn_name) {
        Ok(graph) => match eval::image_checks::check_sealed(&graph, program, &programs) {
            Ok(()) => {
                let build_input = report::BuildInput {
                    path: report::address_to_relative_path(module_addr),
                    digest: report::sha256_hex(input.as_bytes()),
                };
                match report::render(
                    &[build_input],
                    &program.enums,
                    &graph,
                    &wrela_compiler::placement::PlacementTable::default(),
                ) {
                    Ok(text) => text,
                    Err(e) => format!("error[build]: {e}\n"),
                }
            }
            Err(e) => render_sema_error_diag(&e),
        },
        Err(e) => render_sema_error_diag(&eval::to_sema_error(e)),
    };
    Some(text)
}

/// Renders one `sema::SemaError` as an owned, already-`\n`-terminated
/// string in the exact one-line `error[cat]: message [at L:C]` house
/// style `bin/wrela.rs::print_sema_error` prints — this crate's own small
/// duplicate of that renderer (`produce_report_text`'s own nested
/// `render_sema_error` is the identical shape, kept local to that
/// function; this top-level copy is shared by `run_image_pipeline_once`
/// above, the only other place in this file that needs one).
fn render_sema_error_diag(e: &sema::SemaError) -> String {
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

fn run_eval_pipeline_once(input: &str) -> EvalPipelineOutcome {
    match lexer::lex(input) {
        Err(e) => EvalPipelineOutcome::LexErr {
            message: e.message,
            line: e.line,
            col: e.col,
        },
        Ok(tokens) => match parser::parse(tokens) {
            Err(e) => EvalPipelineOutcome::ParseErr {
                message: e.message,
                line: e.line,
                col: e.col,
            },
            // "<fuzz-eval>" is not a real file path — same reasoning as
            // `run_sema_pipeline_once`'s own placeholder: the determinism
            // check only ever compares two runs of the *same* input
            // against each other, so any fixed placeholder works.
            Ok(module) => match sema::check_typed(&module, "<fuzz-eval>") {
                Ok(program) => {
                    let (report, _any_failed) = eval::run_tests(&program);
                    let module_addr = module.path.join(".");
                    let image_outcome = run_image_pipeline_once(&program, &module_addr, input);
                    EvalPipelineOutcome::Ok(report, image_outcome)
                }
                Err(e) => EvalPipelineOutcome::SemaErr {
                    category: e.category,
                    message: e.message,
                    line: e.line,
                    col: e.col,
                    extra_lines: e.extra_lines,
                    omit_location: e.omit_location,
                },
            },
        },
    }
}

/// Invariant (d)'s own check on a successful outcome: `run_tests`'s
/// report (`comptime.tests.build-tier`'s pinned shape) is a sequence of
/// `test <name>: ok` / `test <name>: FAILED <message>` lines followed by
/// exactly one `<N> passed, <M> failed` summary line — never empty (a
/// file with zero `@test` fns still prints `0 passed, 0 failed` alone)
/// and never anything that looks like a leaked internal panic string.
fn report_is_well_formed(report: &str) -> Result<(), String> {
    let lines: Vec<&str> = report.lines().collect();
    let Some((summary, test_lines)) = lines.split_last() else {
        return Err("eval: run_tests report is empty (missing summary line)".into());
    };
    if !summary_line_well_formed(summary) {
        return Err(format!(
            "eval: run_tests report's summary line is malformed: {summary:?}"
        ));
    }
    for line in test_lines {
        if !test_line_well_formed(line) {
            return Err(format!(
                "eval: run_tests report contains a malformed test line: {line:?}"
            ));
        }
    }
    Ok(())
}

/// `"<N> passed, <M> failed"` — both `N` and `M` plain decimal integers,
/// nothing else on the line.
fn summary_line_well_formed(line: &str) -> bool {
    let Some((n, rest)) = line.split_once(" passed, ") else {
        return false;
    };
    let Some(m) = rest.strip_suffix(" failed") else {
        return false;
    };
    n.parse::<u64>().is_ok() && m.parse::<u64>().is_ok()
}

/// `"test <name>: ok"`, `"test <name>: ok (<N> cases)"` (an exhaustive
/// test's own success line), or `"test <name>: FAILED <message>"` — the
/// only shapes `run_tests` ever emits per test (`eval/mod.rs`'s own doc
/// comment on `run_tests`; an exhaustive counterexample's
/// `[param=value, ...]` prefix lives inside the FAILED message).
fn test_line_well_formed(line: &str) -> bool {
    let Some(rest) = line.strip_prefix("test ") else {
        return false;
    };
    match rest.split_once(": ") {
        Some((_name, "ok")) => true,
        Some((_name, verdict)) => {
            if verdict.starts_with("FAILED ") {
                return true;
            }
            let Some(n) = verdict
                .strip_prefix("ok (")
                .and_then(|v| v.strip_suffix(" cases)"))
            else {
                return false;
            };
            n.parse::<u64>().is_ok()
        }
        None => false,
    }
}

/// plans/M4.md item E's own well-formedness half: `run_image_pipeline_once`'s
/// returned text is well-formed exactly when it is the versioned report
/// header (`report::render`'s own `"ImageReport v0"` first line — the rest
/// is not re-validated line-by-line here, since `report.rs`'s own
/// `push_line`/`render_value` are already what every report-bearing golden
/// pins byte-for-byte; this fuzz lane's own job is "never a leaked panic
/// string", not re-proving the renderer's own shape) or a single
/// well-formed one-line diagnostic in the fixed `error[cat]: message` house
/// style, `cat` one of the fixed `SEMA_CATEGORIES` (the identical category
/// set every `SemaErr` outcome is already checked against above) — a
/// multi-line diagnostic (the generic-instantiation chain's own
/// `extra_lines`) is legal too, exactly like an ordinary `SemaErr`, so only
/// the first line's own shape is checked.
fn image_outcome_is_well_formed(text: &str) -> Result<(), String> {
    if text.starts_with("ImageReport v0") {
        return Ok(());
    }
    let Some(first_line) = text.lines().next() else {
        return Err("eval: image pipeline outcome is empty".to_string());
    };
    let Some(rest) = first_line.strip_prefix("error[") else {
        return Err(format!(
            "eval: image pipeline outcome is neither a report nor a diagnostic: {first_line:?}"
        ));
    };
    let Some((category, _)) = rest.split_once(']') else {
        return Err(format!(
            "eval: image pipeline outcome's diagnostic line is malformed: {first_line:?}"
        ));
    };
    if !SEMA_CATEGORIES.contains(&category) {
        return Err(format!(
            "eval: image pipeline outcome produced an unknown diagnostic category `{category}` \
             (not in the fixed set)"
        ));
    }
    Ok(())
}

/// Every invariant the eval fuzzer checks, once per iteration, on one
/// input. Runs the whole lex-then-parse-then-check_typed-then-(run_tests,
/// then — plans/M4.md item E — the image pipeline when exactly one
/// `@image` fn is declared) pipeline twice under `catch_unwind`, mirroring
/// `check_sema_invariants`'s shape, plus the well-formedness check
/// (invariant (d)) on a successful outcome (both `run_tests`'s own report
/// and, when present, the image pipeline's own outcome) and the
/// fixed-category check (also (d)) on a `SemaErr` outcome.
fn check_eval_invariants(input: &str) -> Result<(), String> {
    let first = std::panic::catch_unwind(|| run_eval_pipeline_once(input))
        .map_err(|p| format!("eval panicked: {}", panic_message(&p)))?;
    let second = std::panic::catch_unwind(|| run_eval_pipeline_once(input))
        .map_err(|p| format!("eval panicked on a repeat call: {}", panic_message(&p)))?;

    if let EvalPipelineOutcome::SemaErr { category, .. } = &first {
        if !SEMA_CATEGORIES.contains(category) {
            return Err(format!(
                "eval produced an unknown diagnostic category `{category}` (not in the fixed set)"
            ));
        }
    }
    if let EvalPipelineOutcome::Ok(report, image_outcome) = &first {
        report_is_well_formed(report)?;
        if let Some(text) = image_outcome {
            image_outcome_is_well_formed(text)?;
        }
    }

    match (&first, &second) {
        (EvalPipelineOutcome::Ok(r1, image1), EvalPipelineOutcome::Ok(r2, image2)) => {
            if r1 != r2 {
                return Err(
                    "eval is not deterministic: two runs produced different test reports".into(),
                );
            }
            if image1 != image2 {
                return Err(
                    "eval is not deterministic: two runs produced different image pipeline \
                     outcomes"
                        .into(),
                );
            }
            Ok(())
        }
        (
            EvalPipelineOutcome::LexErr {
                message: m1,
                line: l1,
                col: c1,
            },
            EvalPipelineOutcome::LexErr {
                message: m2,
                line: l2,
                col: c2,
            },
        ) => {
            if m1 != m2 || l1 != l2 || c1 != c2 {
                return Err(
                    "eval is not deterministic: two runs produced different lex errors".into(),
                );
            }
            Ok(())
        }
        (
            EvalPipelineOutcome::ParseErr {
                message: m1,
                line: l1,
                col: c1,
            },
            EvalPipelineOutcome::ParseErr {
                message: m2,
                line: l2,
                col: c2,
            },
        ) => {
            if m1 != m2 || l1 != l2 || c1 != c2 {
                return Err(
                    "eval is not deterministic: two runs produced different parse errors".into(),
                );
            }
            Ok(())
        }
        (
            EvalPipelineOutcome::SemaErr {
                category: cat1,
                message: m1,
                line: l1,
                col: c1,
                extra_lines: e1,
                omit_location: o1,
            },
            EvalPipelineOutcome::SemaErr {
                category: cat2,
                message: m2,
                line: l2,
                col: c2,
                extra_lines: e2,
                omit_location: o2,
            },
        ) => {
            if cat1 != cat2 || m1 != m2 || l1 != l2 || c1 != c2 || e1 != e2 || o1 != o2 {
                return Err(
                    "eval is not deterministic: two runs produced different sema diagnostics"
                        .into(),
                );
            }
            Ok(())
        }
        _ => Err(
            "eval is not deterministic: the two runs disagreed on success/failure or which \
             stage failed"
                .into(),
        ),
    }
}

fn run_eval_fuzz(iters: u64, seed: u64, seed_inputs: &[String]) -> Result<(), String> {
    let mut rng = Rng::new(seed);
    for i in 0..iters {
        let input = if i % 2 == 0 {
            String::from_utf8_lossy(&mutate_seed_input(&mut rng, seed_inputs)).into_owned()
        } else {
            token_soup(&mut rng)
        };
        if let Err(reason) = check_eval_invariants(&input) {
            return report_fuzz_failure("eval", "eval-crash-", seed, i, &input, &reason);
        }
    }
    println!("fuzz eval: {iters} iteration(s) clean (seed={seed})");
    Ok(())
}

fn fuzz_eval(iters: u64, seed: u64) -> Result<(), String> {
    let seed_inputs = corpus_seed_inputs()?;
    with_silenced_panic_hook(|| run_eval_fuzz(iters, seed, &seed_inputs))
}

fn fuzz_eval_smoke() -> Result<(), String> {
    let seed_inputs = corpus_seed_inputs()?;
    with_silenced_panic_hook(|| {
        for &seed in FUZZ_EVAL_SMOKE_SEEDS {
            run_eval_fuzz(FUZZ_EVAL_SMOKE_ITERS_PER_SEED, seed, &seed_inputs)?;
        }
        Ok(())
    })
}

// --- fuzz: lower (plans/M5.md item G) ---------------------------------
//
// The lowering/codegen fuzz lane: exactly `fuzz eval`'s own two strategies
// and the same corpus seed inputs (`corpus_seed_inputs`, `mutate_seed_input`,
// `token_soup`) — that seed set already includes every `tests/golden/*/
// input.wr`, which since item B/C/E now includes the `mwir-*`/`asm-*`/
// `boot-hello` golden inputs too (ordinary golden case dirs,
// `golden_case_dirs` walks them exactly like every other case — confirmed
// directly, not assumed: `ls tests/golden | grep -E 'mwir-|asm-|boot-hello'`
// lists all fifteen, each with its own `input.wr` `corpus_seed_inputs`
// already reads). One more stage beyond `fuzz eval`'s own lex -> parse ->
// `sema::check_typed`: on a successful typecheck, `lower::lower_program`,
// then, on success, `codegen::codegen_program` — never `eval::run_tests`
// or the image pipeline (this lane's whole point is the backend, not the
// evaluator, which `fuzz eval` already covers).
//
// Invariants checked every iteration, under `catch_unwind` (a panic
// anywhere in `lower`/`codegen` is a finding — this is invariant (a), "no
// panics anywhere in lower/codegen", detected exactly the way every other
// lane in this file detects a panic):
//
//  (a) never panics;
//  (b) deterministic: the whole pipeline (lex-parse-check_typed, and, on
//      Ok, lower+codegen, and, whenever the program declares an
//      `@test(runtime)` fn, the test-image layout too) is run twice and the
//      two outcomes are byte-compared — the mwir dump text
//      (`mwir::dump`), the concatenated codegen'd words (every `CodegenFn`'s
//      own `(u32, String)` pairs' `u32` half, in `BTreeMap` key order,
//      *not* the `--stage=asm` dump text a second time — a deliberately
//      separate, word-level compare so a hypothetical dump-rendering bug
//      could never mask a real byte-level divergence), and, on a built test
//      image, the laid-out blob/entry/sections (`layout::layout_test_image`'s
//      own `ImageLayout`);
//  (c) a lowering or codegen rejection is always the fixed `unimplemented`
//      diagnostic category (`bin/wrela.rs`'s own house style prints every
//      `lower::LowerError`/`codegen::CodegenError` as
//      `error[unimplemented]: <message>` — neither error type carries a
//      `category` field at all, unlike `sema::SemaError`, so this lane
//      checks the one fixed literal is still a member of `SEMA_CATEGORIES`
//      rather than re-deriving a category from the message text); **except**
//      a message that starts with `"internal error: "` — both `LowerError`
//      and `CodegenError` (and `LayoutError`) reserve that exact prefix for
//      their own "should be unreachable for any `check_typed`-accepted
//      program" producer-bug guards (`lower.rs`/`codegen.rs`'s own doc
//      comments on their respective `internal(...)` constructors), so
//      hitting one is itself an invariant violation, not a legitimate
//      fail-closed outcome — folded into `LowerFuzzOutcome::Bug` below,
//      exactly like an unknown `SemaError` category is for `fuzz eval`;
//  (d) every successfully codegen'd program passes `codegen::validate`'s own
//      structural checks (that module's own doc comment carries the full
//      list — non-empty code per fn, every `Reloc` in range and, for
//      `Reloc::Call`, targeting a fn this same program actually codegen'd);
//  (e) whenever the typechecked program declares one or more
//      `@test(runtime)` fns, `layout::layout_test_image` (the exact path
//      `bin/wrela.rs::test_cmd`'s own runtime tier calls — reused directly,
//      not reimplemented) is attempted; its own internal
//      `verify_section_sizes` call already re-derives the section table
//      from scratch and turns any mismatch into an `Err` before this lane
//      ever sees a `Ok(ImageLayout)` back, so a successful `Built` outcome
//      *is* the section-size-verified proof this invariant asks for — no
//      second, redundant re-verification is needed here. A program with no
//      `@test(runtime)` fn skips layout entirely (`LayoutOutcome::Skipped`,
//      counted, never attempted) — booting is diff-eval's/the guest bench
//      lane's own job, never this in-process loop's (a boot is ~50ms; this
//      lane's own budget is ~100us/iteration).
//
// A find writes the input to `target/fuzz/lower-crash-<n>.wr` (the same
// `report_fuzz_failure` numbering convention every other lane uses) and
// reports the seed + iteration so it reproduces; every find is minimized by
// hand into a `tests/golden/err-mwir-*` case before the underlying bug is
// fixed, per house rule — fixed here only when the root cause is genuinely
// in `lower.rs`/`codegen.rs`; a root cause in `sema`/`eval` is pinned and
// reported instead (out of this lane's own scope).
//
// Live finding, disclosed rather than routed around (plans/M5.md item G's
// own "pin as golden per house rule BEFORE fixing... if the root cause is
// in sema/eval, pin + report, don't fix out-of-scope"): this lane's very
// first real exercise found a genuine, reproducible `sema::bodies` over-
// acceptance bug, pinned at `golden/err-mwir-if-else-scope-leak` — an
// `if`/`else` whose two branches each declare their own explicitly-typed
// local under the identical name (`value: u64 = 1` / `value: u64 = 2`,
// each syntactically its own fresh, block-scoped declaration, not 02
// §8.1's own documented bare-assignment "definite-init merge" idiom) is
// wrongly accepted by `check_typed` — its own `--stage=typed` dump shows
// the `else` branch's declaration demoted to a plain `Assign` onto the
// `then` branch's local, and the name survives, wrongly, past the end of
// the whole `if`/`else` for the trailing `return value` to read. This is a
// real defect in `sema::bodies`'s own scope handling, not a lowering gap:
// `lower.rs`'s own per-block `LEnv` push/pop is what actually behaves
// correctly here (its own "should be unreachable for a `check_typed`-
// accepted program" internal guard is exactly what surfaces the upstream
// bug). Root cause confirmed out of this session's own permitted scope
// (`sema/`/`eval/` logic — CLAUDE.md/task rules), so it is pinned and
// reported, not fixed here.
//
// Severity, measured directly rather than assumed: this shape is common
// enough in the existing corpus (`value` is an extremely frequent local
// name across the real `tests/golden/*/input.wr` seed set) that a single
// 1-4-op mutation reaches it almost immediately at *every* seed tried —
// 1 through 20, inclusive, every one crashed within 3000 iterations (seed
// 7 crashed at iteration *0*, i.e. the very first mutated input already
// triggered it). There is consequently no seed/iteration budget, however
// small, that currently gives an honest "clean" smoke or deep run — every
// seed reproduces the *same* already-pinned, already-reported bug, not a
// spread of distinct ones a bigger corpus or a different seed could dodge.
// Per plans/M5.md item G's own explicit "fix or report finds first"
// instruction (before the required 3-fresh-seed deep-clean check), this
// session reports rather than fakes a clean run: `fuzz_lower_smoke` is
// therefore NOT called from `check()` (see that call site's own comment),
// and `FUZZ_LOWER_DEEP_ITERS`/`FUZZ_LOWER_DEEP_SEED` below describe the
// budget this lane is *sized* for once the sema fix lands, not a budget it
// currently completes.
//
// Per-iteration cost, measured anyway (aggregated across seeds 1-30's own
// pre-crash prefixes, `target/fuzz/xtask` debug build, authoring machine —
// no seed's own prefix is long enough alone to amortize process/corpus-load
// startup, so this sums 31_126 total pre-crash iterations across 30 short
// runs against their own combined 1.69s wall time, then subtracts each
// invocation's own ~5ms fixed corpus-load/startup cost, measured directly
// via the seed=7/iteration=0 case's own near-instant `real 0.00`s runs):
// roughly 50-60us/iteration — close to, and consistent with, `fuzz eval`'s
// own ~59us/iteration (this pipeline shares the identical lex/parse/
// check_typed prefix `fuzz eval` already pays for; `lower`+`codegen`
// replace `run_tests`'s own rare-cost tail with a cost of the same rough
// order). `FUZZ_LOWER_DEEP_ITERS = 2_000_000` is picked to match `fuzz
// sema`/`fuzz eval`'s own deep default exactly, landing in the identical
// "roughly a minute or two" band those two lanes' own comments already
// target, rather than picking a new number for its own sake — the number
// this session would run at seeds 21/22/23 once the blocking finding above
// is fixed.
const FUZZ_LOWER_DEEP_ITERS: u64 = 2_000_000;
const FUZZ_LOWER_DEEP_SEED: u64 = 1;
// `#[allow(dead_code)]`: not yet read by `check()` (`fuzz_lower_smoke`'s own
// doc comment explains why) — deliberately kept, not deleted, so wiring
// the smoke call back in once the blocking sema fix lands is a one-line
// change with its own budget already named here.
#[allow(dead_code)]
const FUZZ_LOWER_SMOKE_SEEDS: &[u64] = &[1, 2];
#[allow(dead_code)]
const FUZZ_LOWER_SMOKE_ITERS_PER_SEED: u64 = 1_000;

/// What a successful `layout::layout_test_image` attempt contributes to
/// `LowerFuzzOutcome::Ok`'s own determinism compare — `ImageLayout`'s own
/// three fields, copied out field-by-field rather than storing `ImageLayout`
/// itself (which derives no `PartialEq`/`Clone` this crate could reuse
/// without adding one to `wrela-compiler` for a fuzz-only need).
#[derive(Debug, Clone, PartialEq)]
enum LayoutOutcome {
    /// No `@test(runtime)` fn declared — invariant (e)'s own "skip layout
    /// entirely, counted" rule; the overwhelmingly common case for both
    /// corpus-mutation and token-soup input.
    Skipped,
    /// `layout::layout_test_image` rejected this program with a legitimate
    /// (non-`"internal error: "`) `LayoutError` — the only real example this
    /// module has is "relocation out of range", structurally unreachable at
    /// this fuzzer's own tiny image sizes but not disclaimed as impossible.
    Rejected(String),
    /// `layout::layout_test_image` succeeded; `verify_section_sizes` already
    /// ran internally (invariant (e)'s own note above).
    Built {
        blob: Vec<u8>,
        entry: u64,
        sections: Vec<(&'static str, u64, u64)>,
    },
}

/// Exactly one of these shapes comes back from one pipeline run — never a
/// panic, per `check_lower_invariants`'s `catch_unwind`.
#[derive(Debug, Clone, PartialEq)]
enum LowerFuzzOutcome {
    LexErr {
        message: String,
        line: u32,
        col: u32,
    },
    ParseErr {
        message: String,
        line: u32,
        col: u32,
    },
    SemaErr {
        category: &'static str,
        message: String,
        line: u32,
        col: u32,
        extra_lines: Vec<String>,
        omit_location: bool,
    },
    /// `lower::lower_program` rejected this program with its own fixed
    /// `error[unimplemented]` diagnostic (decision 2's fail-closed set) —
    /// never an `"internal error: "`-prefixed message, which is folded into
    /// `Bug` instead (see that variant).
    LowerRejected { message: String },
    /// `codegen::codegen_program` rejected this program the same way
    /// (frame-size overflow, a floating-point value, more than 8 call
    /// arguments, ...) — same `"internal error: "` carve-out as above.
    CodegenRejected { message: String },
    /// Lowered and codegen'd cleanly.
    Ok {
        mwir_dump: String,
        code_words: Vec<u32>,
        layout: LayoutOutcome,
    },
    /// A genuine bug this lane found — never a legitimate outcome,
    /// `check_lower_invariants` always rejects this variant as a finding,
    /// the same way `check_eval_invariants` rejects an unknown `SemaError`
    /// category. Covers every "should be unreachable for a `check_typed`-
    /// accepted program" shape this pipeline can hit: an
    /// `"internal error: "`-prefixed `LowerError`/`CodegenError`/
    /// `LayoutError`, a `mwir::build_layout_ctx` failure on a module that
    /// already passed `check_typed` (the identical unreachable-in-theory
    /// shape one layer up — `build_layout_ctx` only re-runs `specialize`/
    /// `declare`, both strict subsets of what `check_typed` itself already
    /// ran clean), or a `codegen::validate` structural-invariant failure.
    Bug(String),
}

/// `program.tests`' own `TestKind::Runtime` names, in declaration order
/// (`program.tests` is a plain `Vec`, never reordered) — exactly the list
/// `bin/wrela.rs::test_cmd`'s own runtime tier builds before calling
/// `layout::layout_test_image`.
fn runtime_test_names(program: &sema::typed::TypedProgram) -> Vec<String> {
    program
        .tests
        .iter()
        .filter(|t| t.kind == TestKind::Runtime)
        .map(|t| t.name.clone())
        .collect()
}

/// Every emitted `u32` word across every fn in `program`, `BTreeMap`-key
/// order (deterministic) — invariant (b)'s own separate, word-level
/// determinism population, kept apart from `codegen::dump`'s text so a
/// hypothetical dump-rendering bug could never mask a real byte-level
/// divergence between two runs.
fn concat_code_words(program: &codegen::CodegenProgram) -> Vec<u32> {
    let mut words = Vec::new();
    for f in program.fns.values() {
        for (w, _text) in &f.code {
            words.push(*w);
        }
    }
    words
}

/// Invariant (e): attempts `layout::layout_test_image` exactly the way
/// `bin/wrela.rs::test_cmd`'s own runtime tier does, whenever `program`
/// declares one or more `@test(runtime)` fns; `Err` here always means a
/// genuine bug (folded into `LowerFuzzOutcome::Bug` by the caller), never a
/// legitimate rejection path this fn itself decides — the one legitimate
/// `LayoutError` shape (`"relocation out of range"`) is instead carried as
/// `Ok(LayoutOutcome::Rejected(..))`.
fn attempt_layout(
    program: &sema::typed::TypedProgram,
    codegen_program: &codegen::CodegenProgram,
) -> Result<LayoutOutcome, String> {
    let runtime_tests = runtime_test_names(program);
    if runtime_tests.is_empty() {
        return Ok(LayoutOutcome::Skipped);
    }
    // plans/M6.md item D: `codegen_program` (this fn's own parameter) is
    // always built from the *sync-only* `lower::lower_program` path
    // (`run_lower_pipeline_once`'s own doc comment) — this lane never
    // calls `flowwir_lower::lower_program`/`codegen::codegen_program_with_async`
    // at all yet, so a program declaring an *async* `@test(runtime)` fn
    // has no compiled entry for `layout_test_image` to find (an honest
    // `Skipped`, not the `"was never codegen'd"` internal-error guard
    // firing for a real reason this fn's own doc already names as
    // out of scope — surfaced for the first time by this item's own new
    // async-test-bearing goldens joining the fuzz corpus). Extending this
    // lane to the async pipeline is real, further work, named here.
    if program
        .tests
        .iter()
        .any(|t| t.kind == TestKind::Runtime && program.fns.get(&t.name).is_none_or(|f| f.is_async))
    {
        return Ok(LayoutOutcome::Skipped);
    }
    // plans/M6.md item D: no `BootCtx` — this lane's own fuzzed corpus
    // never synthesizes a well-formed actor image from scratch (a real
    // `mailbox=` capacity, a matching `@image`, ...), so `None` is exactly
    // as scoped as this lane already was pre-item-D; a real actor-bearing
    // fuzz case is named, future work, not silently claimed here.
    match layout::layout_test_image(
        codegen_program,
        &runtime_tests,
        &std::collections::BTreeSet::new(),
        None,
        &BTreeMap::new(),
    ) {
        Ok(l) => Ok(LayoutOutcome::Built {
            blob: l.blob,
            entry: l.entry,
            sections: l
                .sections
                .iter()
                .map(|s| (s.name, s.base, s.size))
                .collect(),
        }),
        Err(e) => {
            if e.message.starts_with("internal error: ") {
                Err(format!("layout::layout_test_image: {}", e.message))
            } else {
                Ok(LayoutOutcome::Rejected(e.message))
            }
        }
    }
}

/// One full run of the pipeline the lower fuzzer exercises: lex, then (on
/// success) parse a whole module, then `sema::check_typed`, then (on a
/// successful typecheck) `lower::lower_program`, then (on success)
/// `codegen::codegen_program`, then (invariant (e)) `attempt_layout`.
/// "<fuzz-lower>" is not a real file path — same placeholder reasoning as
/// `run_eval_pipeline_once`'s own `"<fuzz-eval>"`: the determinism check
/// only ever compares two runs of the *same* input against each other, so
/// any fixed placeholder works.
fn run_lower_pipeline_once(input: &str) -> LowerFuzzOutcome {
    let module = match lexer::lex(input) {
        Err(e) => {
            return LowerFuzzOutcome::LexErr {
                message: e.message,
                line: e.line,
                col: e.col,
            };
        }
        Ok(tokens) => match parser::parse(tokens) {
            Err(e) => {
                return LowerFuzzOutcome::ParseErr {
                    message: e.message,
                    line: e.line,
                    col: e.col,
                };
            }
            Ok(module) => module,
        },
    };
    let program = match sema::check_typed(&module, "<fuzz-lower>") {
        Err(e) => {
            return LowerFuzzOutcome::SemaErr {
                category: e.category,
                message: e.message,
                line: e.line,
                col: e.col,
                extra_lines: e.extra_lines,
                omit_location: e.omit_location,
            };
        }
        Ok(p) => p,
    };
    let mwir_program = match lower::lower_program(&program) {
        Err(e) => {
            return if e.message.starts_with("internal error: ") {
                LowerFuzzOutcome::Bug(format!("lower::lower_program: {}", e.message))
            } else {
                LowerFuzzOutcome::LowerRejected { message: e.message }
            };
        }
        Ok(p) => p,
    };
    let mwir_dump = mwir::dump(&mwir_program);
    let layout_ctx = match mwir::build_layout_ctx(&module) {
        Err(e) => {
            return LowerFuzzOutcome::Bug(format!(
                "mwir::build_layout_ctx failed after check_typed already accepted this program: \
                 {e:?}"
            ));
        }
        Ok(c) => c,
    };
    let codegen_program = match codegen::codegen_program(&mwir_program, &layout_ctx) {
        Err(e) => {
            return if e.message.starts_with("internal error: ") {
                LowerFuzzOutcome::Bug(format!("codegen::codegen_program: {}", e.message))
            } else {
                LowerFuzzOutcome::CodegenRejected { message: e.message }
            };
        }
        Ok(p) => p,
    };
    if let Err(reason) = codegen::validate(&codegen_program) {
        return LowerFuzzOutcome::Bug(format!("codegen::validate: {reason}"));
    }
    let code_words = concat_code_words(&codegen_program);
    let layout = match attempt_layout(&program, &codegen_program) {
        Ok(l) => l,
        Err(bug) => return LowerFuzzOutcome::Bug(bug),
    };
    LowerFuzzOutcome::Ok {
        mwir_dump,
        code_words,
        layout,
    }
}

/// Every invariant the lower fuzzer checks, once per iteration, on one
/// input. Runs the whole pipeline twice under `catch_unwind`, mirroring
/// `check_eval_invariants`'s shape exactly: invariant (c)'s category check
/// on a `SemaErr`/`LowerRejected`/`CodegenRejected` outcome, invariant (a)'s
/// "never a `Bug`" check, then invariant (b)'s determinism compare,
/// matched per-shape (rather than one blanket `!=`) so a divergence names
/// exactly which stage disagreed, mirroring every other lane's own
/// diagnostic style in this file.
fn check_lower_invariants(input: &str) -> Result<(), String> {
    let first = std::panic::catch_unwind(|| run_lower_pipeline_once(input))
        .map_err(|p| format!("lower/codegen panicked: {}", panic_message(&p)))?;
    let second = std::panic::catch_unwind(|| run_lower_pipeline_once(input)).map_err(|p| {
        format!(
            "lower/codegen panicked on a repeat call: {}",
            panic_message(&p)
        )
    })?;

    if let LowerFuzzOutcome::Bug(msg) = &first {
        return Err(format!("lower/codegen fuzz found a bug: {msg}"));
    }
    if let LowerFuzzOutcome::SemaErr { category, .. } = &first {
        if !SEMA_CATEGORIES.contains(category) {
            return Err(format!(
                "lower: unknown sema diagnostic category `{category}` (not in the fixed set)"
            ));
        }
    }
    if matches!(
        &first,
        LowerFuzzOutcome::LowerRejected { .. } | LowerFuzzOutcome::CodegenRejected { .. }
    ) && !SEMA_CATEGORIES.contains(&"unimplemented")
    {
        return Err(
            "lower/codegen: the fixed `unimplemented` diagnostic category is missing from \
             SEMA_CATEGORIES"
                .into(),
        );
    }

    match (&first, &second) {
        (
            LowerFuzzOutcome::LexErr {
                message: m1,
                line: l1,
                col: c1,
            },
            LowerFuzzOutcome::LexErr {
                message: m2,
                line: l2,
                col: c2,
            },
        ) => {
            if m1 != m2 || l1 != l2 || c1 != c2 {
                return Err(
                    "lower is not deterministic: two runs produced different lex errors".into(),
                );
            }
            Ok(())
        }
        (
            LowerFuzzOutcome::ParseErr {
                message: m1,
                line: l1,
                col: c1,
            },
            LowerFuzzOutcome::ParseErr {
                message: m2,
                line: l2,
                col: c2,
            },
        ) => {
            if m1 != m2 || l1 != l2 || c1 != c2 {
                return Err(
                    "lower is not deterministic: two runs produced different parse errors".into(),
                );
            }
            Ok(())
        }
        (
            LowerFuzzOutcome::SemaErr {
                category: cat1,
                message: m1,
                line: l1,
                col: c1,
                extra_lines: e1,
                omit_location: o1,
            },
            LowerFuzzOutcome::SemaErr {
                category: cat2,
                message: m2,
                line: l2,
                col: c2,
                extra_lines: e2,
                omit_location: o2,
            },
        ) => {
            if cat1 != cat2 || m1 != m2 || l1 != l2 || c1 != c2 || e1 != e2 || o1 != o2 {
                return Err(
                    "lower is not deterministic: two runs produced different sema diagnostics"
                        .into(),
                );
            }
            Ok(())
        }
        (
            LowerFuzzOutcome::LowerRejected { message: m1 },
            LowerFuzzOutcome::LowerRejected { message: m2 },
        ) => {
            if m1 != m2 {
                return Err(
                    "lower is not deterministic: two runs produced different lowering rejections"
                        .into(),
                );
            }
            Ok(())
        }
        (
            LowerFuzzOutcome::CodegenRejected { message: m1 },
            LowerFuzzOutcome::CodegenRejected { message: m2 },
        ) => {
            if m1 != m2 {
                return Err(
                    "lower is not deterministic: two runs produced different codegen rejections"
                        .into(),
                );
            }
            Ok(())
        }
        (LowerFuzzOutcome::Ok { .. }, LowerFuzzOutcome::Ok { .. }) => {
            if first != second {
                return Err(
                    "lower is not deterministic: two runs produced a different mwir dump, \
                     codegen'd words, or laid-out test image for the same input"
                        .into(),
                );
            }
            Ok(())
        }
        _ => Err(
            "lower is not deterministic: the two runs disagreed on success/failure or which \
             stage failed"
                .into(),
        ),
    }
}

fn run_lower_fuzz(iters: u64, seed: u64, seed_inputs: &[String]) -> Result<(), String> {
    let mut rng = Rng::new(seed);
    for i in 0..iters {
        let input = if i % 2 == 0 {
            String::from_utf8_lossy(&mutate_seed_input(&mut rng, seed_inputs)).into_owned()
        } else {
            token_soup(&mut rng)
        };
        if let Err(reason) = check_lower_invariants(&input) {
            return report_fuzz_failure("lower", "lower-crash-", seed, i, &input, &reason);
        }
    }
    println!("fuzz lower: {iters} iteration(s) clean (seed={seed})");
    Ok(())
}

fn fuzz_lower(iters: u64, seed: u64) -> Result<(), String> {
    let seed_inputs = corpus_seed_inputs()?;
    with_silenced_panic_hook(|| run_lower_fuzz(iters, seed, &seed_inputs))
}

/// Same shape as every other lane's own `_smoke` fn (2 fixed seeds, 1_000
/// iterations apiece) — but, unlike every other lane's, **not** called from
/// `check()` yet (hence `#[allow(dead_code)]` below). `FUZZ_LOWER_DEEP_ITERS`'s
/// own doc comment (above) records why: this lane's very first real
/// exercise found a genuine, pinned, out-of-scope `sema::bodies` bug
/// (`golden/err-mwir-if-else-scope-leak`) that the first of these two fixed
/// smoke seeds already reproduces well inside 1_000 iterations (seed=1 at
/// iteration 708, on the corpus as of this commit — `run_lower_fuzz`
/// returns on that `Err` before `fuzz_lower_smoke`'s own loop ever reaches
/// seed=2, whose own first reproduction of the identical bug happens to
/// land at iteration 2134, past this particular smoke budget, on this same
/// corpus) — and every seed 1 through 20 tried this session reproduces it
/// within 3_000 iterations regardless, so there is no seed choice here that
/// would make this call honest today, only ones that happen to delay it
/// past whatever budget is picked. Callable directly (`cargo xtask fuzz
/// lower --iters 1000 --seed 1`) for verification; wire it into `check()`
/// (one line, right where every other `fuzz_*_smoke` call already sits)
/// the moment the sema fix lands.
#[allow(dead_code)]
fn fuzz_lower_smoke() -> Result<(), String> {
    let seed_inputs = corpus_seed_inputs()?;
    with_silenced_panic_hook(|| {
        for &seed in FUZZ_LOWER_SMOKE_SEEDS {
            run_lower_fuzz(FUZZ_LOWER_SMOKE_ITERS_PER_SEED, seed, &seed_inputs)?;
        }
        Ok(())
    })
}

// --- fuzz: async --------------------------------------------------------
//
// plans/M7.md item Y. `attempt_layout`'s own doc comment has disclosed
// since M6-D that the `lower` lane never calls `flowwir_lower::lower_program`
// or `codegen::codegen_program_with_async` at all — an async `@test(runtime)`
// fn is an honest `LayoutOutcome::Skipped` there — so the whole async
// pipeline (FlowWir lowering, `emit_flowwir_fn`, async frame sizing, the
// group child-index map, and `layout_test_image` with a real `BootCtx`)
// had **no fuzz coverage whatsoever**. This lane is that coverage: the same
// mechanism as every lane above it (seeded splitmix64, corpus mutation, no
// external engine), pointed at the pipeline `bin/wrela.rs::test_cmd`'s own
// runtime tier actually runs.
//
// Generation (the one thing this lane does differently, and it has to):
// the async surface is not reachable by chance. A `token_soup` string will
// never spell `@actor` + `pub fn` + `async fn` + `await` + `@image`, and a
// mutation of an arbitrary corpus entry lands on an async program only as
// often as async entries appear in the corpus. So the *base* population is
// the fixed, named `ASYNC_SEED_CASES` list below — every accept-shaped
// async/actor golden in the tree — while splice donors come from the same
// list most of the time and from the whole corpus occasionally (an
// `f`-string, a generic fn, a `defer`, a `for ... take` carried into an
// actor program is exactly the cross-shape a hand-written golden never
// covers). `token_soup` keeps one iteration in eight anyway, so the
// arbitrary-garbage tail every other lane checks is not silently dropped
// here. `run_async_fuzz` prints the measured reach every run (how many
// iterations type-checked, how many actually lowered >=1 async fn, how many
// reached async codegen, how many laid out a real async test image) — a
// lane that never reaches the surface it claims to cover is worthless, and
// the number is printed rather than assumed.
//
// Invariants, identical to the `lower` lane's: (a) nothing in the pipeline
// ever panics; (b) two runs of the same input agree — the FlowWir dump, the
// concatenated codegen'd words, and the laid-out image blob/entry/sections
// all byte-compared, plus the measured reach itself; (c) every rejection is
// a legitimate fail-closed diagnostic in the fixed category set
// (`SEMA_CATEGORIES` — a `SemaError`'s own `category`, and for the stages
// that carry no category of their own the fixed literal `bin/wrela.rs`
// prints for that stage, recorded per stage in `AsyncFuzzOutcome::Rejected`);
// (d) an `"internal error: "`-prefixed message anywhere, a
// `codegen::validate` failure, or a `mwir`/`layout` context failure on a
// program `check_typed` already accepted is a **bug**, reported as a
// finding, never tolerated.
//
// A find writes the exact input to `target/fuzz/async-crash-<n>.wr` and
// reports the seed + iteration so it reproduces exactly.

// Measured on the authoring machine, the same way every other lane's
// budget was (the `cargo xtask` alias' own debug build — `run -q -p xtask`,
// never a release one): 20_000 iterations in 5.0s of user time, 60_000 in
// 15.1s, i.e. ~250us/iteration. That is roughly 4-5x the `lower` lane's own
// ~50-60us, for two named reasons rather than a mysterious one: this lane's
// mutation bases are *all* real programs, so ~14% of its iterations reach
// `check_typed` where the corpus-wide lanes reach it far more rarely; and
// each of those then pays a full sync+async lowering, an async codegen and
// (for ~4.6% of all iterations) a real `BootCtx` image layout, instead of
// falling out at lex. 400_000 therefore lands a bare `cargo xtask fuzz
// async` at roughly 100 seconds — inside the same "roughly a minute or
// two" band `fuzz sema`/`fuzz eval`/`fuzz lower` already target. The band
// is what is being matched here, not their iteration count.
const FUZZ_ASYNC_DEEP_ITERS: u64 = 400_000;
const FUZZ_ASYNC_DEEP_SEED: u64 = 1;
// Wired into `check` alongside every other live lane's smoke: two fixed
// seeds, 1_000 iterations each (~0.5s total at the cost measured above),
// no seed ever from the clock or the environment.
const FUZZ_ASYNC_SMOKE_SEEDS: &[u64] = &[1, 2];
const FUZZ_ASYNC_SMOKE_ITERS_PER_SEED: u64 = 1_000;

/// Every accept-shaped async/actor golden in the tree — this lane's own
/// mutation bases. A fixed, named list rather than a directory scan with a
/// `grep`-shaped heuristic, for the same reason `PROJECT_SEED_CASES` is one:
/// which cases carry async surface is a decision, and a decision belongs in
/// source where a reviewer can see it move. `async_seed_inputs` fails
/// closed if any name here no longer exists, so a renamed golden breaks the
/// lane loudly instead of silently shrinking its base population.
///
/// Deliberately excluded: the `err-actor-*`/`err-await-*`/`err-send-*`/
/// `err-group-*` cases, whose whole point is a rejection sema reaches long
/// before FlowWir does. They are still reachable *as splice donors* through
/// `corpus_seed_inputs` (every golden `input.wr` is in there), which is the
/// role they can actually play here.
const ASYNC_SEED_CASES: &[&str] = &[
    // The FlowWir stage's own goldens — await in a branch, in a loop, in a
    // chain, under `defer`.
    "flowwir-basic",
    "flowwir-branch-await",
    "flowwir-chain",
    "flowwir-defer",
    "flowwir-loop-await",
    // The async machine-code goldens.
    "asm-async-basic",
    "asm-async-loop-checkpoint",
    // Every real actor/async boot image: the full BootCtx path (rtdata,
    // boot sequence, dispatch tables, group arena, deadlines).
    "boot-actor-chain",
    "boot-actor-reply-struct",
    "boot-actor-smoke",
    "boot-actors",
    "boot-await-mailbox-full",
    "boot-cancel-cleanup",
    "boot-deadline-cancel",
    "boot-deadline-inherit",
    "boot-group-join",
    "boot-send",
    // Accept-shaped sema cases over the same surface — no runtime test, so
    // they mutate toward "async fns that lower and codegen but never lay
    // out an image", which is `asm-async-*`'s shape with more variety.
    "check-actor-methods",
    "check-actor-private-handle-helper",
    "check-actor-send",
    "check-await-self-path",
    "check-deadline",
    "check-group",
    "check-send-proven",
];

/// The `ASYNC_SEED_CASES` inputs, in the listed order. Fails closed on a
/// missing case (see that constant's own doc comment).
fn async_seed_inputs() -> Result<Vec<String>, String> {
    let golden_dir = root().join("tests/golden");
    let mut inputs = Vec::with_capacity(ASYNC_SEED_CASES.len());
    for case in ASYNC_SEED_CASES {
        let path = golden_dir.join(case).join("input.wr");
        inputs.push(std::fs::read_to_string(&path).map_err(|e| {
            format!(
                "fuzz async: ASYNC_SEED_CASES names `{case}`, but {} is unreadable: {e} \
                 (a renamed/removed golden must be fixed in ASYNC_SEED_CASES, not ignored)",
                path.display()
            )
        })?);
    }
    Ok(inputs)
}

/// How far one fuzzed input actually got down the async pipeline — the
/// measured hit rate this lane reports, and (since it is fully derived from
/// the run) part of the determinism compare for free.
#[derive(Debug, Clone, Default, PartialEq)]
struct AsyncReach {
    /// `sema::check_typed` accepted it.
    typechecked: bool,
    /// `flowwir_lower::lower_program` returned `Ok` — i.e. the async
    /// lowering ran to completion (over zero or more async fns).
    flow_lowered: bool,
    /// How many async fns/methods that FlowWir program actually contains.
    /// Zero means "this input reached `flowwir_lower` but gave it nothing
    /// to do" — counted separately, because counting it as coverage would
    /// be the exact dishonesty this lane exists to end.
    async_fns: usize,
    /// `codegen::codegen_program_with_async` returned `Ok` (and
    /// `codegen::validate` passed).
    codegen_ok: bool,
    /// `layout::layout_test_image` built an image with a real `BootCtx`.
    image_built: bool,
    /// ...and at least one of that image's runtime tests was async, so the
    /// entry driver's own scheduler loop, turn areas and dispatch tables
    /// were laid out for real.
    async_image: bool,
}

/// Running totals across one `run_async_fuzz` invocation.
#[derive(Default)]
struct AsyncReachTotals {
    typechecked: u64,
    flow_lowered: u64,
    with_async_fns: u64,
    async_fns_seen: u64,
    codegen_ok: u64,
    image_built: u64,
    async_image: u64,
}

impl AsyncReachTotals {
    fn add(&mut self, r: &AsyncReach) {
        self.typechecked += u64::from(r.typechecked);
        self.flow_lowered += u64::from(r.flow_lowered);
        self.with_async_fns += u64::from(r.async_fns > 0);
        self.async_fns_seen += r.async_fns as u64;
        self.codegen_ok += u64::from(r.codegen_ok);
        self.image_built += u64::from(r.image_built);
        self.async_image += u64::from(r.async_image);
    }
}

/// Exactly one of these shapes comes back from one pipeline run — never a
/// panic, per `check_async_invariants`'s `catch_unwind`.
#[derive(Debug, Clone, PartialEq)]
enum AsyncFuzzOutcome {
    LexErr {
        message: String,
        line: u32,
        col: u32,
    },
    ParseErr {
        message: String,
        line: u32,
        col: u32,
    },
    /// Anything that produces a real `SemaError`: `sema::check_typed`,
    /// `layout::merge_layout_ctx`, and the image evaluator (via
    /// `eval::to_sema_error`). Its own `category` is checked against
    /// `SEMA_CATEGORIES`.
    SemaErr {
        category: &'static str,
        message: String,
        line: u32,
        col: u32,
        extra_lines: Vec<String>,
        omit_location: bool,
    },
    /// A fail-closed rejection from a stage whose error type carries no
    /// category of its own (`LowerError`, `FlowError`, `CodegenError`,
    /// `LayoutError`, `resolve_runtime_test_args`' bare `String`).
    /// `category` is the fixed literal `bin/wrela.rs::test_cmd` prints for
    /// that exact stage, so invariant (c) checks the same set a user would
    /// actually see; `stage` names the call site so a determinism
    /// divergence or a category miss is diagnosable without a rerun.
    Rejected {
        stage: &'static str,
        category: &'static str,
        message: String,
    },
    /// The whole async pipeline ran.
    Ok {
        flow_dump: String,
        code_words: Vec<u32>,
        layout: LayoutOutcome,
    },
    /// A genuine bug this lane found — `check_async_invariants` always
    /// rejects this variant as a finding. Same population as the `lower`
    /// lane's own `Bug`: an `"internal error: "`-prefixed message from any
    /// stage, or a structural failure on a program `check_typed` already
    /// accepted.
    Bug(String),
}

impl AsyncFuzzOutcome {
    /// Which stage this outcome came from — the determinism compare's own
    /// first check, so "the two runs disagreed" always names where.
    fn stage(&self) -> &'static str {
        match self {
            AsyncFuzzOutcome::LexErr { .. } => "lex",
            AsyncFuzzOutcome::ParseErr { .. } => "parse",
            AsyncFuzzOutcome::SemaErr { .. } => "sema",
            AsyncFuzzOutcome::Rejected { stage, .. } => stage,
            AsyncFuzzOutcome::Ok { .. } => "ok",
            AsyncFuzzOutcome::Bug(_) => "bug",
        }
    }
}

/// Every `SemaError` this lane can see — `sema::check_typed`'s own, and
/// the two later stages that report through the same type
/// (`layout::merge_layout_ctx`, and the image evaluator via
/// `eval::to_sema_error`). `stage` names which, for the determinism
/// compare's own diagnostics.
///
/// The `"internal error: "` carve-out applies here exactly as it does to
/// every other stage in this lane, and it is not decoration: `eval/interp.rs`
/// alone carries ~50 of those guards (`await`/`send`/`with group` reaching
/// the comptime evaluator, an unbound local in place position, a missing
/// builder argument, ...), every one of them a "should be unreachable for a
/// `check_typed`-accepted program" claim that this lane is in a position to
/// falsify. Classifying them as ordinary `comptime` diagnostics would have
/// silently swallowed exactly the class of find this item exists to make
/// visible.
fn async_sema_outcome(stage: &'static str, e: sema::SemaError) -> AsyncFuzzOutcome {
    if e.message.starts_with("internal error: ") {
        return AsyncFuzzOutcome::Bug(format!("{stage}: {}", e.message));
    }
    AsyncFuzzOutcome::SemaErr {
        category: e.category,
        message: e.message,
        line: e.line,
        col: e.col,
        extra_lines: e.extra_lines,
        omit_location: e.omit_location,
    }
}

/// One stage's `Err`, split the one way that matters: an
/// `"internal error: "` prefix is a bug (invariant (d)), anything else is a
/// legitimate fail-closed rejection carrying the category that stage prints
/// (invariant (c)).
fn async_stage_err(
    stage: &'static str,
    category: &'static str,
    message: String,
) -> AsyncFuzzOutcome {
    if message.starts_with("internal error: ") {
        AsyncFuzzOutcome::Bug(format!("{stage}: {message}"))
    } else {
        AsyncFuzzOutcome::Rejected {
            stage,
            category,
            message,
        }
    }
}

/// One full run of the async pipeline, mirroring `bin/wrela.rs::test_cmd`'s
/// own runtime tier stage for stage (and `build_runtime_test_image`'s own
/// "deliberately parallel copy" reasoning — those driver internals are not
/// a library surface this crate can call into). "<fuzz-async>" is not a
/// real path: the determinism check only ever compares two runs of the
/// *same* input, so any fixed placeholder works.
fn run_async_pipeline_once(input: &str) -> (AsyncFuzzOutcome, AsyncReach) {
    let mut reach = AsyncReach::default();
    let module = match lexer::lex(input) {
        Err(e) => {
            return (
                AsyncFuzzOutcome::LexErr {
                    message: e.message,
                    line: e.line,
                    col: e.col,
                },
                reach,
            );
        }
        Ok(tokens) => match parser::parse(tokens) {
            Err(e) => {
                return (
                    AsyncFuzzOutcome::ParseErr {
                        message: e.message,
                        line: e.line,
                        col: e.col,
                    },
                    reach,
                );
            }
            Ok(module) => module,
        },
    };
    let program = match sema::check_typed(&module, "<fuzz-async>") {
        Err(e) => return (async_sema_outcome("sema::check_typed", e), reach),
        Ok(p) => p,
    };
    reach.typechecked = true;

    let mut modules: BTreeMap<String, Module> = BTreeMap::new();
    modules.insert(module.path.join("."), module.clone());
    // A failure here is a **bug**, not a rejection — the identical judgement
    // the `lower` lane already makes about `mwir::build_layout_ctx`, which
    // is exactly what this fn calls for a single-module build: it re-runs
    // `specialize`/`declare`, both strict subsets of what `check_typed`
    // itself just ran clean on this same module.
    let layout_ctx = match layout::merge_layout_ctx(&modules) {
        Err(e) => {
            return (
                AsyncFuzzOutcome::Bug(format!(
                    "layout::merge_layout_ctx failed after check_typed already accepted this \
                     program: [{}] {}",
                    e.category, e.message
                )),
                reach,
            );
        }
        Ok(c) => c,
    };
    // The sync half, exactly as `test_cmd` runs it: `codegen_program_with_async`
    // needs both halves, and `flowwir_lower` never touches a sync fn.
    let mwir_program = match lower::lower_program(&program) {
        Err(e) => {
            return (
                async_stage_err("lower::lower_program", "unimplemented", e.message),
                reach,
            );
        }
        Ok(p) => p,
    };
    // THE stage this whole lane exists for.
    let flow_program = match flowwir_lower::lower_program(&program) {
        Err(e) => {
            return (
                async_stage_err("flowwir_lower::lower_program", "unimplemented", e.message),
                reach,
            );
        }
        Ok(p) => p,
    };
    reach.flow_lowered = true;
    reach.async_fns = flow_program.fns.len();
    let flow_dump = flowwir::dump(&flow_program);

    let graph = match &program.image_fn {
        Some(fn_name) => match eval::interp::eval_image(&program, fn_name) {
            Err(e) => {
                return (
                    async_sema_outcome("eval::interp::eval_image", eval::to_sema_error(e)),
                    reach,
                );
            }
            Ok(g) => g,
        },
        None => eval::image::ImageGraph::default(),
    };
    let method_index = match layout::actor_method_index_tables(&modules, &layout_ctx) {
        Err(e) => {
            return (
                async_stage_err(
                    "layout::actor_method_index_tables",
                    "unimplemented",
                    e.message,
                ),
                reach,
            );
        }
        Ok(m) => m,
    };
    let runtime_tests = runtime_test_names(&program);
    let test_args = match layout::resolve_runtime_test_args(&program, &runtime_tests, &graph) {
        Err(msg) => {
            return (
                async_stage_err("layout::resolve_runtime_test_args", "build", msg),
                reach,
            );
        }
        Ok(a) => a,
    };
    let group_arena_capacity = layout::count_with_group_sites(&modules);
    let codegen_program = match codegen::codegen_program_with_async(
        &mwir_program,
        &flow_program,
        &layout_ctx,
        &method_index,
        group_arena_capacity,
    ) {
        Err(e) => {
            return (
                async_stage_err(
                    "codegen::codegen_program_with_async",
                    "unimplemented",
                    e.message,
                ),
                reach,
            );
        }
        Ok(p) => p,
    };
    if let Err(reason) = codegen::validate(&codegen_program) {
        return (
            AsyncFuzzOutcome::Bug(format!("codegen::validate (async-aware): {reason}")),
            reach,
        );
    }
    reach.codegen_ok = true;
    let code_words = concat_code_words(&codegen_program);

    let async_frames = match codegen::async_frame_sizes(&flow_program, &layout_ctx) {
        Err(e) => {
            return (
                async_stage_err("codegen::async_frame_sizes", "unimplemented", e.message),
                reach,
            );
        }
        Ok(m) => m,
    };
    let group_child_index = match codegen::compute_group_child_indices(&flow_program) {
        Err(e) => {
            return (
                async_stage_err(
                    "codegen::compute_group_child_indices",
                    "unimplemented",
                    e.message,
                ),
                reach,
            );
        }
        Ok(m) => m,
    };

    // `test_cmd`'s runtime tier only ever lays out an image when the file
    // declares at least one `@test(runtime)` fn — mirrored exactly, so a
    // `Skipped` here means "production would not have laid one out either",
    // not "this lane looked away" (which is precisely what `attempt_layout`
    // in the `lower` lane had to say about every async test).
    let layout_outcome = if runtime_tests.is_empty() {
        LayoutOutcome::Skipped
    } else {
        let async_tests: std::collections::BTreeSet<String> = runtime_tests
            .iter()
            .filter(|name| program.fns.get(*name).is_some_and(|f| f.is_async))
            .cloned()
            .collect();
        let is_async_image = !async_tests.is_empty();
        let boot = layout::BootCtx {
            graph: &graph,
            modules: &modules,
            layout_ctx: &layout_ctx,
            async_frames: &async_frames,
            group_child_index: &group_child_index,
        };
        match layout::layout_test_image(
            &codegen_program,
            &runtime_tests,
            &async_tests,
            Some(boot),
            &test_args,
        ) {
            Ok(l) => {
                reach.image_built = true;
                reach.async_image = is_async_image;
                LayoutOutcome::Built {
                    blob: l.blob,
                    entry: l.entry,
                    sections: l
                        .sections
                        .iter()
                        .map(|s| (s.name, s.base, s.size))
                        .collect(),
                }
            }
            Err(e) => {
                if e.message.starts_with("internal error: ") {
                    return (
                        AsyncFuzzOutcome::Bug(format!("layout::layout_test_image: {}", e.message)),
                        reach,
                    );
                }
                LayoutOutcome::Rejected(e.message)
            }
        }
    };

    (
        AsyncFuzzOutcome::Ok {
            flow_dump,
            code_words,
            layout: layout_outcome,
        },
        reach,
    )
}

/// Every invariant the async fuzzer checks, once per iteration, on one
/// input. Runs the whole pipeline twice under `catch_unwind` (invariant
/// (a)), rejects a `Bug` (invariant (d)), category-checks a rejection
/// (invariant (c)), then compares the two runs (invariant (b)) — stage
/// first, so a divergence names where, then the full value, which for an
/// `Ok` is the FlowWir dump, the codegen'd words and the image bytes.
/// Returns the first run's reach so the caller can total it.
fn check_async_invariants(input: &str) -> Result<AsyncReach, String> {
    let (first, reach) = std::panic::catch_unwind(|| run_async_pipeline_once(input))
        .map_err(|p| format!("the async pipeline panicked: {}", panic_message(&p)))?;
    let (second, reach2) =
        std::panic::catch_unwind(|| run_async_pipeline_once(input)).map_err(|p| {
            format!(
                "the async pipeline panicked on a repeat call: {}",
                panic_message(&p)
            )
        })?;

    if let AsyncFuzzOutcome::Bug(msg) = &first {
        return Err(format!("async fuzz found a bug: {msg}"));
    }
    match &first {
        AsyncFuzzOutcome::SemaErr { category, .. } => {
            if !SEMA_CATEGORIES.contains(category) {
                return Err(format!(
                    "async: unknown sema diagnostic category `{category}` (not in the fixed set)"
                ));
            }
        }
        AsyncFuzzOutcome::Rejected {
            stage, category, ..
        } => {
            if !SEMA_CATEGORIES.contains(category) {
                return Err(format!(
                    "async: {stage} rejected with category `{category}`, which is not in the \
                     fixed set"
                ));
            }
        }
        _ => {}
    }

    if first.stage() != second.stage() {
        return Err(format!(
            "the async pipeline is not deterministic: one run stopped at `{}`, the other at `{}`",
            first.stage(),
            second.stage()
        ));
    }
    if first != second {
        return Err(format!(
            "the async pipeline is not deterministic: two runs of the same input produced \
             different `{}` results",
            first.stage()
        ));
    }
    if reach != reach2 {
        return Err(
            "the async pipeline is not deterministic: two runs reached different stages".into(),
        );
    }
    Ok(reach)
}

/// One iteration's input: mostly a mutated async/actor golden, sometimes
/// one with a splice donor drawn from the whole corpus, occasionally plain
/// token soup — see the section comment above for why the mix is weighted
/// this way rather than the 50/50 the other lanes use.
fn async_fuzz_input(rng: &mut Rng, async_seeds: &[String], corpus_seeds: &[String]) -> String {
    match rng.gen_range(8) {
        0 => token_soup(rng),
        1 => String::from_utf8_lossy(&mutate_seed_input_from(rng, async_seeds, corpus_seeds))
            .into_owned(),
        _ => String::from_utf8_lossy(&mutate_seed_input(rng, async_seeds)).into_owned(),
    }
}

fn run_async_fuzz(
    iters: u64,
    seed: u64,
    async_seeds: &[String],
    corpus_seeds: &[String],
) -> Result<(), String> {
    let mut rng = Rng::new(seed);
    let mut totals = AsyncReachTotals::default();
    for i in 0..iters {
        let input = async_fuzz_input(&mut rng, async_seeds, corpus_seeds);
        match check_async_invariants(&input) {
            Ok(reach) => totals.add(&reach),
            Err(reason) => {
                return report_fuzz_failure("async", "async-crash-", seed, i, &input, &reason);
            }
        }
    }
    println!(
        "fuzz async: {iters} iteration(s) clean (seed={seed}); reached check_typed {}, \
         flowwir_lower {} ({} with >=1 async fn, {} async fns lowered), async codegen {}, \
         test image laid out {} ({} of them async)",
        totals.typechecked,
        totals.flow_lowered,
        totals.with_async_fns,
        totals.async_fns_seen,
        totals.codegen_ok,
        totals.image_built,
        totals.async_image,
    );
    Ok(())
}

fn fuzz_async(iters: u64, seed: u64) -> Result<(), String> {
    let async_seeds = async_seed_inputs()?;
    let corpus_seeds = corpus_seed_inputs()?;
    with_silenced_panic_hook(|| run_async_fuzz(iters, seed, &async_seeds, &corpus_seeds))
}

fn fuzz_async_smoke() -> Result<(), String> {
    let async_seeds = async_seed_inputs()?;
    let corpus_seeds = corpus_seed_inputs()?;
    with_silenced_panic_hook(|| {
        for &seed in FUZZ_ASYNC_SMOKE_SEEDS {
            run_async_fuzz(
                FUZZ_ASYNC_SMOKE_ITERS_PER_SEED,
                seed,
                &async_seeds,
                &corpus_seeds,
            )?;
        }
        Ok(())
    })
}

// --- golden ---------------------------------------------------------------
//
// Layout: tests/golden/<case>/input.wr + expected/<stage>.txt. Each
// expected file pins `wrela dump --stage=<stage> input.wr` byte-for-byte.
//
// plans/M4.md item A adds one more case shape, a *project*: a case dir
// containing `src/` (a package tree of `.wr` files), a file `root` (one
// line: the root file's path relative to the case dir, e.g.
// `src/app/main.wr`), and `expected/` exactly like today. The runner
// tells the two shapes apart by `root`'s presence — `root` names the
// file `wrela dump`/`wrela test` is actually invoked on in place of the
// flat shape's `input.wr`; everything else (stage dispatch, `test.txt`'s
// touch convention, `--update`) is identical. The loader (`wrela dump
// --stage=check`, plans/M4.md item A) anchors the package root from
// that file's own path alone — no new flag.
/// plans/M5.md item D: the review-visible form a golden pins for raw
/// image bytes (decision: a hexdump, not raw binary in git — "more
/// reviewable," the task's own instruction) — 16 bytes per line,
/// `%08x: ` offset prefix, space-separated lowercase hex pairs, no ASCII
/// column (kept minimal; the offset+hex pairs alone are enough to spot a
/// one-byte diff in a review, and `--stage=asm`'s own dump already gives
/// every word a mnemonic elsewhere). Deterministic, a pure function of
/// `bytes`.
fn hex_dump(bytes: &[u8]) -> String {
    let mut out = String::new();
    for (i, chunk) in bytes.chunks(16).enumerate() {
        let offset = i * 16;
        let hex: Vec<String> = chunk.iter().map(|b| format!("{b:02x}")).collect();
        out.push_str(&format!("{offset:08x}: {}\n", hex.join(" ")));
    }
    out
}

fn golden_case_target(case: &Path) -> Result<Option<PathBuf>, String> {
    let root_marker = case.join("root");
    if root_marker.is_file() {
        let rel = std::fs::read_to_string(&root_marker)
            .map_err(|e| format!("read {}: {e}", root_marker.display()))?;
        let rel = rel.trim();
        if rel.is_empty() {
            return Err(format!("{}: `root` file is empty", root_marker.display()));
        }
        return Ok(Some(case.join(rel)));
    }
    let input = case.join("input.wr");
    if input.exists() {
        return Ok(Some(input));
    }
    Ok(None)
}

/// plans/M5.md item E: builds the `wrela-vmm` binary and codesigns it
/// against `crates/wrela-vmm/entitlements.plist` — the smoke probe's own
/// exact recipe (plans/M5.md's "item zero"), re-run on every `cargo xtask
/// check`/`golden`. `golden`'s own `test.txt` cases pass the resulting
/// path via `wrela test --vmm <path>` so a case with an `@test(runtime)`
/// fn boots on this exact, freshly rebuilt-and-signed binary rather than
/// whatever `find_vmm_binary`'s own next-to-the-executable fallback might
/// happen to find on disk. `codesign` itself only exists on macOS
/// (CLAUDE.md's own target host for this milestone's flagship dev loop);
/// building still runs everywhere, so a `test.txt` case with no runtime
/// tests never depends on this step at all, and the boot golden's own
/// failure on a non-macOS host is an honest, expected gap, not a silent
/// skip.
fn build_and_sign_vmm() -> Result<PathBuf, String> {
    run(
        Command::new("cargo").args(["build", "--quiet", "-p", "wrela-vmm", "--bin", "wrela-vmm"]),
        "cargo build wrela-vmm",
    )?;
    let bin = root().join("target/debug/wrela-vmm");
    if cfg!(target_os = "macos") {
        let mut cmd = Command::new("codesign");
        cmd.args(["--force", "--sign", "-", "--entitlements"]);
        cmd.arg(root().join("crates/wrela-vmm/entitlements.plist"));
        cmd.arg(&bin);
        run(&mut cmd, "codesign wrela-vmm")?;
    }
    Ok(bin)
}

/// `cargo test --workspace` never codesigns anything — the test binaries
/// `cargo test` builds under `target/debug/deps/` are ordinary,
/// unsigned executables, exactly like any other `cargo build` output
/// (`build_and_sign_vmm`'s own doc comment: codesigning is this xtask's
/// own, deliberate, post-build step, never something Cargo does for
/// free). That was harmless before this item — `wrela-vmm`'s own
/// pre-item-F unit tests (the ESR decoder, `parse_report`) are pure
/// functions, no HVF calls anywhere — but item F's own hand-built
/// clock-reading test (`lib.rs`) is the first `#[test]` in this
/// workspace that calls `hv_vm_create` for real, so its test binary
/// needs the identical entitlement `build_and_sign_vmm` already gives
/// the ordinary `wrela-vmm` binary. `check`'s own plain `cargo test
/// --workspace` step therefore excludes `wrela-vmm` (above); this
/// function is `wrela-vmm`'s own replacement: build its test binaries
/// with `--no-run`, codesign each one (parsed off cargo's own
/// `Executable ... (path)` stderr lines — no `--message-format=json`/
/// `serde_json` dependency needed for two lines of known shape), then
/// run each directly, single-threaded (`--test-threads=1`) — the same
/// "no two calls into HVF's one process-wide VM context are ever
/// concurrent" reasoning the hand-built test's own doc comment gives,
/// now enforced at the process level too, not just within one `#[test]`
/// fn.
fn test_wrela_vmm_signed() -> Result<(), String> {
    // Deliberately no `--quiet` here (unlike every other `cargo`
    // invocation in this file): cargo only ever prints its own
    // `Executable ... (path)` lines — this function's one source of
    // truth for which binaries to sign — under the default verbosity;
    // `--quiet` suppresses them even on a rebuild, and a cached (already
    // up to date) build prints nothing else either way, so there is no
    // extra noise being traded away here.
    let output = Command::new("cargo")
        .current_dir(root())
        .args(["test", "-p", "wrela-vmm", "--no-run"])
        .output()
        .map_err(|e| format!("cargo test -p wrela-vmm --no-run: {e}"))?;
    if !output.status.success() {
        return Err(format!(
            "cargo test -p wrela-vmm --no-run failed:\n{}",
            String::from_utf8_lossy(&output.stderr)
        ));
    }
    let stderr = String::from_utf8_lossy(&output.stderr);
    let mut executables: Vec<PathBuf> = Vec::new();
    for line in stderr.lines() {
        let line = line.trim();
        let Some(rest) = line.strip_prefix("Executable ") else {
            continue;
        };
        let (Some(open), Some(close)) = (rest.rfind('('), rest.rfind(')')) else {
            continue;
        };
        if close > open {
            executables.push(root().join(&rest[open + 1..close]));
        }
    }
    if executables.is_empty() {
        return Err(
            "cargo test -p wrela-vmm --no-run: found no test executable(s) to sign".to_string(),
        );
    }
    for exe in &executables {
        if cfg!(target_os = "macos") {
            let mut cmd = Command::new("codesign");
            cmd.args(["--force", "--sign", "-", "--entitlements"]);
            cmd.arg(root().join("crates/wrela-vmm/entitlements.plist"));
            cmd.arg(exe);
            run(&mut cmd, "codesign wrela-vmm test binary")?;
        }
        run(
            Command::new(exe).arg("--test-threads=1"),
            &format!("run {}", exe.display()),
        )?;
    }
    Ok(())
}

fn golden(update: bool) -> Result<(), String> {
    run(
        Command::new("cargo").args(["build", "--quiet", "-p", "wrela-compiler", "--bin", "wrela"]),
        "cargo build wrela",
    )?;
    let wrela = root().join("target/debug/wrela");
    let vmm = build_and_sign_vmm()?;
    let golden_dir = root().join("tests/golden");
    let mut cases = 0usize;
    let mut failures = Vec::new();
    for case in golden_case_dirs(&golden_dir)? {
        let expected_dir = case.join("expected");
        let input = match golden_case_target(&case)? {
            Some(target) if target.exists() && expected_dir.is_dir() => target,
            _ => {
                failures.push(format!(
                    "{}: missing input.wr (or `root`'s target) or expected/",
                    case.display()
                ));
                continue;
            }
        };
        let mut expected_files: Vec<_> = std::fs::read_dir(&expected_dir)
            .map_err(|e| format!("read {}: {e}", expected_dir.display()))?
            .filter_map(Result::ok)
            .map(|e| e.path())
            .collect();
        expected_files.sort();
        for exp in expected_files {
            let stage = exp
                .file_stem()
                .and_then(|s| s.to_str())
                .ok_or_else(|| format!("bad expected file name: {}", exp.display()))?
                .to_string();
            // Invoke with a root-relative path from the repo root: the
            // path is rendered into some diagnostics (the generic
            // requirement chain's `instantiated at <path>:<line>` lines),
            // and an absolute path would bake this checkout's location
            // into the pinned expectation — failing in any worktree or
            // clone.
            let rel_input = input.strip_prefix(root()).unwrap_or(&input);
            // plans/M4.md item E: `build.txt`/`build-err.txt` mean "run
            // `wrela build <input.wr> --out-dir <fixed-repo-relative-dir>`,
            // compare its stdout" — a third meaning alongside `test.txt`'s,
            // the golden runner's own "one new case shape" (decision 11).
            // The out-dir is a fixed, deterministic, repo-relative path
            // derived from the case's own name (never a random temp name:
            // its own literal text is what `wrela build`'s stdout prints
            // back, decision 8's own "print the path exactly as derived
            // from the argument", so it must be stable across runs to stay
            // golden-pinnable) — removed and recreated fresh immediately
            // before every invocation, then removed again after this
            // expectation's own checks finish, so no build artifact is
            // ever left behind for git to notice.
            let case_name = case
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("case")
                .to_string();
            let build_out_dir_rel = format!("target/golden-build-tmp/{case_name}");
            let build_out_dir_abs = root().join(&build_out_dir_rel);
            if stage == "build" || stage == "build-err" {
                if build_out_dir_abs.exists() {
                    std::fs::remove_dir_all(&build_out_dir_abs)
                        .map_err(|e| format!("remove {}: {e}", build_out_dir_abs.display()))?;
                }
                std::fs::create_dir_all(&build_out_dir_abs)
                    .map_err(|e| format!("create {}: {e}", build_out_dir_abs.display()))?;
            }
            // plans/M5.md item D: `img.hex` means "hexdump whatever
            // `<name>.img` the case's own `build.txt` stage already wrote
            // into `build_out_dir_abs` and compare/update against that" —
            // a fourth expectation-file meaning, alongside `test.txt`'s/
            // `build.txt`'s/`build-err.txt`'s, never its own `wrela`
            // invocation (a case carrying `img.hex` must also carry
            // `build.txt`, sorted before it alphabetically — `b` < `i` —
            // so the image already exists on disk by the time this stage
            // runs; the build-output directory's own removal is deferred
            // to the end of this case's loop, below, specifically so this
            // stage can still see it).
            if stage == "img" {
                let written: Vec<_> = std::fs::read_dir(&build_out_dir_abs)
                    .map_err(|e| format!("read {}: {e}", build_out_dir_abs.display()))?
                    .filter_map(Result::ok)
                    .map(|e| e.path())
                    .filter(|p| p.extension().and_then(|e| e.to_str()) == Some("img"))
                    .collect();
                let img_bytes = match written.as_slice() {
                    [one] => {
                        std::fs::read(one).map_err(|e| format!("read {}: {e}", one.display()))?
                    }
                    other => {
                        failures.push(format!(
                            "{} [img]: expected exactly one `*.img` written to {}, found {} \
                             (does this case also carry a `build.txt` expectation, sorted \
                             before `img.hex`?)",
                            case.display(),
                            build_out_dir_abs.display(),
                            other.len()
                        ));
                        continue;
                    }
                };
                let actual = hex_dump(&img_bytes);
                cases += 1;
                if update {
                    std::fs::write(&exp, &actual)
                        .map_err(|e| format!("write {}: {e}", exp.display()))?;
                    continue;
                }
                let expected = std::fs::read_to_string(&exp)
                    .map_err(|e| format!("read {}: {e}", exp.display()))?;
                if actual != expected {
                    failures.push(format!(
                        "{} [img]: image bytes differ from expectation\n--- expected\n{expected}--- actual\n{actual}",
                        case.display()
                    ));
                }
                continue;
            }
            // `test.txt` means "run `wrela test <input.wr>`, compare its
            // stdout" (plans/M3.md item E) rather than the ordinary
            // `wrela dump --stage=<stage>` every other expectation file
            // pins — the touch convention extended one file name beyond
            // `--stage=<name>`, the dumbest integration (M3.md item E's
            // own wording). `wrela test` exits nonzero exactly when any
            // `@test` failed (decision 9) — a normal, expected outcome
            // some goldens deliberately pin (a mixed pass/fail report),
            // not a runner malfunction, so unlike every `dump` stage
            // below, a nonzero exit here is not itself a failure; only
            // stdout is ever compared, exactly like every other stage.
            let out = if stage == "test" {
                Command::new(&wrela)
                    .current_dir(root())
                    .arg("test")
                    .arg(rel_input)
                    .arg("--vmm")
                    .arg(&vmm)
                    .output()
                    .map_err(|e| format!("run wrela: {e}"))?
            } else if stage == "build" || stage == "build-err" {
                Command::new(&wrela)
                    .current_dir(root())
                    .arg("build")
                    .arg(rel_input)
                    .arg("--out-dir")
                    .arg(&build_out_dir_rel)
                    .output()
                    .map_err(|e| format!("run wrela: {e}"))?
            } else {
                Command::new(&wrela)
                    .current_dir(root())
                    .arg("dump")
                    .arg(format!("--stage={stage}"))
                    .arg(rel_input)
                    .output()
                    .map_err(|e| format!("run wrela: {e}"))?
            };
            // `build-err.txt` requires the opposite of every other
            // expectation file: `wrela build` exits nonzero exactly when
            // it printed a diagnostic (decision 11 — unlike `dump`, which
            // stays exit-0-by-convention), so a *successful* exit here is
            // itself the failure.
            if stage == "build-err" {
                if out.status.success() {
                    failures.push(format!(
                        "{} [build-err]: wrela build unexpectedly exited successfully",
                        case.display()
                    ));
                    continue;
                }
            } else if stage != "test" && !out.status.success() {
                failures.push(format!(
                    "{} [{stage}]: wrela exited with failure:\n{}",
                    case.display(),
                    String::from_utf8_lossy(&out.stderr)
                ));
                continue;
            }
            let actual = String::from_utf8_lossy(&out.stdout).into_owned();
            cases += 1;
            if update {
                std::fs::write(&exp, &actual)
                    .map_err(|e| format!("write {}: {e}", exp.display()))?;
                continue;
            }
            let expected = std::fs::read_to_string(&exp)
                .map_err(|e| format!("read {}: {e}", exp.display()))?;
            if actual != expected {
                failures.push(format!(
                    "{} [{stage}]: output differs from expectation\n--- expected\n{expected}--- actual\n{actual}",
                    case.display()
                ));
            }
            // decision 11: a `build.txt` case additionally proves the
            // *written* report file — not just `wrela build`'s own stdout
            // summary — matches the pinned `expected/report.txt`, if the
            // case carries one (every project-shaped `build.txt` case
            // does; `err-image-*`'s own `build-err.txt` cases never write
            // a report at all, so this block never runs for those).
            if stage == "build" {
                let report_expected = expected_dir.join("report.txt");
                if report_expected.is_file() {
                    let written: Vec<_> = std::fs::read_dir(&build_out_dir_abs)
                        .map_err(|e| format!("read {}: {e}", build_out_dir_abs.display()))?
                        .filter_map(Result::ok)
                        .map(|e| e.path())
                        .filter(|p| p.extension().and_then(|e| e.to_str()) == Some("txt"))
                        .filter(|p| {
                            p.file_name()
                                .and_then(|n| n.to_str())
                                .is_some_and(|n| n.ends_with(".report.txt"))
                        })
                        .collect();
                    match written.as_slice() {
                        [one] => {
                            let written_text = std::fs::read_to_string(one)
                                .map_err(|e| format!("read {}: {e}", one.display()))?;
                            let expected_text = std::fs::read_to_string(&report_expected)
                                .map_err(|e| format!("read {}: {e}", report_expected.display()))?;
                            if written_text != expected_text {
                                failures.push(format!(
                                    "{} [build]: the report file `wrela build` wrote differs from expected/report.txt\n--- expected\n{expected_text}--- actual\n{written_text}",
                                    case.display()
                                ));
                            }
                        }
                        other => failures.push(format!(
                            "{} [build]: expected exactly one `*.report.txt` written to {}, found {}",
                            case.display(),
                            build_out_dir_abs.display(),
                            other.len()
                        )),
                    }
                }
            }
        }
        // plans/M5.md item D: the build-output directory (if any expected
        // file in this case used one) is removed once, here, after every
        // expectation file for this case has had its own chance to read
        // it — `img.hex` (above) needs `build.txt`'s own written `.img`
        // still on disk, so removal can no longer happen immediately
        // after the `build`/`build-err` stage itself finishes.
        let case_name = case
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("case")
            .to_string();
        let build_out_dir_abs = root().join(format!("target/golden-build-tmp/{case_name}"));
        if build_out_dir_abs.exists() {
            std::fs::remove_dir_all(&build_out_dir_abs)
                .map_err(|e| format!("remove {}: {e}", build_out_dir_abs.display()))?;
        }
    }
    if update {
        println!("golden: updated {cases} expectation(s) — review the diff before committing");
        return Ok(());
    }
    if failures.is_empty() {
        println!("golden: {cases} expectation(s) ok");
        Ok(())
    } else {
        for f in &failures {
            eprintln!("{f}\n");
        }
        Err(format!("golden: {} failure(s)", failures.len()))
    }
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
fn produce_report_and_image(target: &Path) -> Result<(String, Option<Vec<u8>>), String> {
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
                        let placement =
                            match placement::place(&graph, &modules_by_addr, &layout_ctx) {
                                Ok(p) => p,
                                Err(e) => return Ok((format!("error[build]: {e}\n"), None)),
                            };
                        match report::render(&inputs, &program.enums, &graph, &placement) {
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
                                let mut layout_types = Vec::new();
                                for module in modules_by_addr.values() {
                                    let specialized = sema::specialize::specialize(module)
                                        .map_err(|e| render_sema_error(&e))?;
                                    layout_types.extend(
                                        sema::types::check_layouts(&specialized)
                                            .map_err(|e| render_sema_error(&e))?,
                                    );
                                }
                                report::render_exact_bytes_section(&mut text, &layout_types);
                                let img = match layout::try_layout_program(
                                    &programs,
                                    &layout_ctx,
                                    &graph,
                                    &modules_by_addr,
                                ) {
                                    Ok(Some(image_layout)) => {
                                        layout::render_layout_section(&mut text, &image_layout);
                                        Some(image_layout.blob)
                                    }
                                    Ok(None) => None,
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
/// destination, must each exit `EXIT_VMM_FAILURE` (2). The two constants
/// are duplicated here (not imported — `xtask` deliberately never links
/// the `wrela-vmm` *crate*, the same established boundary
/// `parse_guest_record`'s own doc comment already explains) rather than
/// invented independently: both mirror `main.rs`'s own `EXIT_VMM_FAILURE`/
/// `EXIT_REPLAY_DIVERGENCE` values exactly, cited by name in the comments
/// below so a future edit to either side is easy to keep in sync.
fn repro_replay_exit_code_contract(vmm: &Path) -> Result<(), String> {
    const EXIT_VMM_FAILURE: i32 = 2; // mirrors wrela_vmm::main::EXIT_VMM_FAILURE
    const EXIT_REPLAY_DIVERGENCE: i32 = 3; // mirrors wrela_vmm::main::EXIT_REPLAY_DIVERGENCE

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
    const EXIT_REPLAY_DIVERGENCE: i32 = 3; // mirrors wrela_vmm::main::EXIT_REPLAY_DIVERGENCE
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
             diverge"
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
    repro_replay_exit_code_contract(&vmm)
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
    const EXIT_REPLAY_DIVERGENCE: i32 = 3; // mirrors wrela_vmm::main::EXIT_REPLAY_DIVERGENCE
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

    // (1) The recording carries exactly the admissions this workload's own
    // source says it must, in order — the oracle on the *witness* itself,
    // checked before any replay is attempted, so a recorder that miscounted
    // a drain, watched the wrong ring, or emitted nothing at all fails here
    // rather than replaying its own mistake back to itself:
    //
    //   - the root turn on core 0 messages `Far` on core 2;
    //   - `Near`'s turn on core 0 messages `Sink` on core 1;
    //   - `Far`'s turn on core 2 messages the same `Sink`, and is admitted
    //     *after* core 0's even though it was published before it (the
    //     consuming core's drain walks its lanes in ring order — the
    //     workload's own header has the whole argument, and its assertion
    //     `n == 1` is the transcript half of this same claim).
    //
    // `sender=` is a core, not an actor: decision 28 settled that the
    // producer of a cross-core ring is a core.
    let admissions: Vec<&str> = record_text
        .lines()
        .filter_map(|l| l.split_once("]=").map(|(_, rhs)| rhs))
        .filter(|rhs| rhs.starts_with("Admission "))
        .collect();
    let expected = [
        "Admission mailbox=Far sender=core0",
        "Admission mailbox=Sink sender=core0",
        "Admission mailbox=Sink sender=core2",
    ];
    if admissions != expected {
        let _ = std::fs::remove_dir_all(&tmp_dir);
        return Err(format!(
            "repro: {CASE} recorded {:?}, expected {:?}",
            admissions, expected
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

    // (3) The tamper: **invert the admission order at `Sink`** — swap the
    // producing cores of its two entries and leave every other byte of the
    // log exactly as recorded. Both tampered lines are individually
    // plausible (each names a core that really does produce into `Sink`),
    // so the only thing wrong with the log is the *order*, which is the
    // fact this whole item exists to record. A tamper that broke a line's
    // shape, or named a core that produces nothing, would be caught by
    // something weaker.
    let sink_prefix = "Admission mailbox=Sink sender=";
    let swapped_sender = |line: &str| -> Option<String> {
        let (head, rhs) = line.split_once("]=")?;
        let sender = rhs.strip_prefix(sink_prefix)?;
        let other = match sender {
            "core0" => "core2",
            "core2" => "core0",
            _ => return None,
        };
        Some(format!("{head}]={sink_prefix}{other}"))
    };
    let mut tampered = String::new();
    let mut swapped = 0usize;
    for line in record_text.lines() {
        match swapped_sender(line) {
            Some(rewritten) => {
                swapped += 1;
                tampered.push_str(&rewritten);
            }
            None => tampered.push_str(line),
        }
        tampered.push('\n');
    }
    if swapped != 2 {
        let _ = std::fs::remove_dir_all(&tmp_dir);
        return Err(format!(
            "repro: expected 2 `Sink` admission lines to invert, found {swapped}"
        ));
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
    let stderr = String::from_utf8_lossy(&tampered_out.stderr).to_string();
    let _ = std::fs::remove_dir_all(&tmp_dir);
    if tampered_exit != EXIT_REPLAY_DIVERGENCE {
        return Err(format!(
            "repro: a tampered admission choice replayed with exit {tampered_exit}, expected \
             {EXIT_REPLAY_DIVERGENCE} (a determinism finding must never be mistaken for a \
             successful replay):\n{stderr}"
        ));
    }
    if !stderr.contains("admission mismatch") {
        return Err(format!(
            "repro: the tampered {CASE} replay diverged, but not by name — stderr must say which \
             admission disagreed:\n{stderr}"
        ));
    }
    println!(
        "repro: tests/golden/{CASE}'s {} cross-core admission(s) — two producing cores into one \
         mailbox — record and replay byte-stable, and an inverted admission order is caught by \
         name (witness-only: the guest performs the admission, the recorder witnesses it, replay \
         checks it)",
        admissions.len()
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
    const EXIT_REPLAY_DIVERGENCE: i32 = 3; // mirrors wrela_vmm::main::EXIT_REPLAY_DIVERGENCE
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
    let sp_top = machine_layout::core_stack_base(0) + machine_layout::CORE_STACK_SIZE;

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
    // measuring pass fixes the data region's base.
    let entry_len = build_entry(0).len();
    let data_base = {
        let after_code = machine_layout::IMAGE_BASE + (entry_len as u64) * 4;
        after_code.div_ceil(16) * 16
    };
    let words = build_entry(data_base);
    assert_eq!(words.len(), entry_len, "entry length must not move");

    let mut img: Vec<u8> = words.iter().flat_map(|w| w.to_le_bytes()).collect();
    img.resize(
        (data_base - machine_layout::IMAGE_BASE + DATA_REGION_SIZE) as usize,
        0,
    );
    let data_off = (data_base - machine_layout::IMAGE_BASE) as usize;
    fill_blk_ring(&mut img, data_off, data_base);

    let report_text = format!(
        "Machine revision={}\n\
         Input path=<xtask blk conformance> digest=deadbeef\n\
         Section name=entry base={:#x} size={}\n\
         Entry base={:#x}\n\
         BlkDevice device=device#0 capacity_sectors=16 features={:#x} vector={BLK_VECTOR}\n\
         BlkQueue index=0 size={QUEUE_SIZE} desc={:#x} avail={:#x} used={:#x} doorbell={:#x}\n\
         BlkPool name=BlockControl device=device#0 base={:#x} size={:#x}\n",
        wrela_machine::MACHINE_REVISION_STR,
        machine_layout::IMAGE_BASE,
        img.len(),
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
/// lowering-skips, <S2> exhaustive-skips`).
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
}

/// Lex+parse+typecheck a single-module program — mirrors `wrela test`'s
/// own scope exactly (`sema::check_typed`'s own "imports through the
/// single-module entry ... are [unimplemented]" refusal, `bin/
/// wrela.rs::test_cmd` never attempts `loader::load_closure` either).
/// `None` for anything out of this oracle's scope: a lex/parse/sema
/// failure (an `err-*` golden — an expected rejection, never a bug this
/// oracle should report on) or a program with imports (multi-module, out
/// of `wrela test`'s own scope) — never treated as a plumbing `Err`,
/// since both are ordinary, expected exclusions.
fn typecheck_single_module(
    source: &str,
    path: &str,
) -> Option<(Module, sema::typed::TypedProgram)> {
    let tokens = lexer::lex(source).ok()?;
    let module = parser::parse(tokens).ok()?;
    if !module.imports.is_empty() {
        return None;
    }
    let program = sema::check_typed(&module, path).ok()?;
    Some((module, program))
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
fn build_runtime_test_image(
    module: &Module,
    program: &sema::typed::TypedProgram,
    source: &str,
    path: &str,
    test_names: &[String],
) -> Result<(Vec<u8>, String), String> {
    let mut modules: BTreeMap<String, Module> = BTreeMap::new();
    modules.insert(module.path.join("."), module.clone());
    let layout_ctx = layout::merge_layout_ctx(&modules).map_err(|e| e.message)?;
    let mwir_program = lower::lower_program(program).map_err(|e| e.message)?;
    // plans/M6.md item F: the same full pipeline `bin/wrela.rs::test_cmd`
    // runs, not the sync-only shortcut this fn used through item E — the
    // determinism/replay lanes now build a real actor+group image
    // (`boot-deadline-cancel`), which needs FlowWir, the async codegen
    // entry point, and the real `BootCtx`. A case with no actors and no
    // async test flows through it byte-identically (empty graph, empty
    // flow program), which is what keeps `boot-hello`'s own recorded
    // choice log and `bench guest`'s exact counts unmoved.
    let flow_program =
        wrela_compiler::flowwir_lower::lower_program(program).map_err(|e| e.message)?;
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
    let method_index =
        layout::actor_method_index_tables(&modules, &layout_ctx).map_err(|e| e.message)?;
    let test_args =
        layout::resolve_runtime_test_args(program, test_names, &graph).map_err(|e| e)?;
    let group_arena_capacity = layout::count_with_group_sites(&modules);
    let codegen_program = codegen::codegen_program_with_async(
        &mwir_program,
        &flow_program,
        &layout_ctx,
        &method_index,
        group_arena_capacity,
    )
    .map_err(|e| e.message)?;
    let async_frames =
        codegen::async_frame_sizes(&flow_program, &layout_ctx).map_err(|e| e.message)?;
    let async_tests: std::collections::BTreeSet<String> = test_names
        .iter()
        .filter(|name| program.fns.get(*name).is_some_and(|f| f.is_async))
        .cloned()
        .collect();
    let group_child_index =
        codegen::compute_group_child_indices(&flow_program).map_err(|e| e.message)?;
    let boot = layout::BootCtx {
        graph: &graph,
        modules: &modules,
        layout_ctx: &layout_ctx,
        async_frames: &async_frames,
        group_child_index: &group_child_index,
    };
    let image_layout = layout::layout_test_image(
        &codegen_program,
        test_names,
        &async_tests,
        Some(boot),
        &test_args,
    )
    .map_err(|e| e.message)?;
    let source_digest = report::sha256_hex(source.as_bytes());
    let mut report_text = format!(
        "Machine revision={}\nInput path={path} digest={source_digest}\n",
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
struct GuestRecord {
    exit_code: u64,
    exits: u64,
    choice_count: usize,
}

fn parse_guest_record(text: &str) -> Result<GuestRecord, String> {
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
        let Some((module, program)) = typecheck_single_module(&source, &path_display) else {
            continue; // out of scope: lex/parse/sema error, or multi-module
        };
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
        let (eval_report, _) = eval::run_tests(&program);
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
            &module,
            &program,
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
         {} quota-skips",
        tally.agree,
        tally.cases_agreed,
        tally.lowering_skips,
        tally.exhaustive_skips,
        tally.quota_skips
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
         exhaustive-skips, {} quota-skips",
        tally.agree,
        tally.cases_agreed,
        tally.lowering_skips,
        tally.exhaustive_skips,
        tally.quota_skips
    );
    Ok(())
}

// --- bench: guest lane + profile (plans/M5.md item F, decision 14) ---------
//
// `cargo xtask bench guest`: boots `tests/golden/boot-hello`'s own test
// image (built once, outside the timed loop — only the *boot* itself is
// the measured workload, not compilation) via the codesigned `wrela-vmm`
// binary, `--record`ed every time so the exact per-boot counts
// (`RecordFile`'s own `exits`/`choice_count`/`exit_code`, plus the
// transcript bytes captured on stdout) are available for the "exact
// replay counts" half of decision 14 without a second, separately-timed
// invocation. Same warmup+timed shape as every other bench lane, its own
// locked median in `bench/thresholds.toml`'s `[guest]` table.

const BENCH_GUEST_WARMUP_ITERS: usize = 2;
const BENCH_GUEST_TIMED_ITERS: usize = 5;

fn guest_bench_threshold_us() -> Result<u128, String> {
    bench_threshold_us("guest", "boot_hello_median_us")
}

/// Builds `tests/golden/boot-hello`'s own `@test(runtime)` image once —
/// shared by `bench_guest_lane` and `profile`, below, so the "which
/// program, which tests" decision lives in exactly one place.
fn boot_hello_test_image() -> Result<(Vec<u8>, String), String> {
    golden_test_image("boot-hello")
}

/// The same builder, over any single-module `tests/golden/<case>` that
/// declares `@test(runtime)` fns — plans/M6.md item F needs a second one
/// (`boot-deadline-cancel`, the first genuinely clock-driven boot) and
/// there is nothing `boot-hello`-specific about the recipe.
fn golden_test_image(case_name: &str) -> Result<(Vec<u8>, String), String> {
    let case = root().join("tests/golden").join(case_name);
    let target = golden_case_target(&case)?
        .ok_or_else(|| format!("tests/golden/{case_name} has no input.wr"))?;
    let source =
        std::fs::read_to_string(&target).map_err(|e| format!("read {}: {e}", target.display()))?;
    let path_display = target.display().to_string();
    let (module, program) = typecheck_single_module(&source, &path_display)
        .ok_or_else(|| format!("tests/golden/{case_name} failed to typecheck"))?;
    let runtime_names: Vec<String> = program
        .tests
        .iter()
        .filter(|t| t.kind == TestKind::Runtime)
        .map(|t| t.name.clone())
        .collect();
    if runtime_names.is_empty() {
        return Err(format!(
            "tests/golden/{case_name} declares no @test(runtime) fns"
        ));
    }
    build_runtime_test_image(&module, &program, &source, &path_display, &runtime_names)
}

fn bench_guest_lane() -> Result<(), String> {
    let vmm = build_and_sign_vmm()?;
    let (img_bytes, report_text) = boot_hello_test_image()?;

    let tmp_dir = root().join("target/bench-guest-tmp");
    if tmp_dir.exists() {
        std::fs::remove_dir_all(&tmp_dir)
            .map_err(|e| format!("remove {}: {e}", tmp_dir.display()))?;
    }
    std::fs::create_dir_all(&tmp_dir).map_err(|e| format!("create {}: {e}", tmp_dir.display()))?;
    let img_path = tmp_dir.join("boot.img");
    let report_path = tmp_dir.join("boot.report.txt");
    std::fs::write(&img_path, &img_bytes)
        .map_err(|e| format!("write {}: {e}", img_path.display()))?;
    std::fs::write(&report_path, &report_text)
        .map_err(|e| format!("write {}: {e}", report_path.display()))?;
    let record_path = tmp_dir.join("boot.record.txt");

    let boot_one = || -> Result<(Duration, String, i32, GuestRecord), String> {
        let start = Instant::now();
        let out = Command::new(&vmm)
            .arg(&report_path)
            .arg(&img_path)
            .arg("--record")
            .arg(&record_path)
            .output()
            .map_err(|e| format!("run wrela-vmm: {e}"))?;
        let elapsed = start.elapsed();
        let exit_code_class = out.status.code().unwrap_or(-1);
        let transcript = String::from_utf8_lossy(&out.stdout).into_owned();
        let record_text = std::fs::read_to_string(&record_path)
            .map_err(|e| format!("read {}: {e}", record_path.display()))?;
        let record = parse_guest_record(&record_text)?;
        Ok((elapsed, transcript, exit_code_class, record))
    };

    for _ in 0..BENCH_GUEST_WARMUP_ITERS {
        boot_one()?;
    }

    let mut totals = Vec::with_capacity(BENCH_GUEST_TIMED_ITERS);
    let mut transcripts = Vec::with_capacity(BENCH_GUEST_TIMED_ITERS);
    let mut exit_codes = Vec::with_capacity(BENCH_GUEST_TIMED_ITERS);
    let mut exits_counts = Vec::with_capacity(BENCH_GUEST_TIMED_ITERS);
    let mut choice_counts = Vec::with_capacity(BENCH_GUEST_TIMED_ITERS);
    for _ in 0..BENCH_GUEST_TIMED_ITERS {
        let (elapsed, transcript, exit_code_class, record) = boot_one()?;
        if exit_code_class != 0 && exit_code_class != 1 {
            let _ = std::fs::remove_dir_all(&tmp_dir);
            return Err(format!(
                "bench guest: the wrela VMM did not boot the test image (exit {exit_code_class})"
            ));
        }
        totals.push(elapsed);
        transcripts.push(transcript);
        exit_codes.push(record.exit_code);
        exits_counts.push(record.exits);
        choice_counts.push(record.choice_count);
    }
    let _ = std::fs::remove_dir_all(&tmp_dir);

    // decision 14's own "exact counts" half: every timed boot of the
    // identical image must produce byte-identical transcripts and
    // identical exit codes/exit counts — anything else is a real
    // nondeterminism bug (the generated runtime or the VMM), never
    // ordinary machine noise. plans/M6.md item E: the choice-sequence
    // recorder's own count joins this exact-count set (`boot-hello` has
    // no actors/deadlines, so its own choice count is just its clock-read
    // count today — the identical fact `clock_log_len` used to name,
    // generalized).
    let first_transcript = transcripts[0].clone();
    for (i, t) in transcripts.iter().enumerate() {
        if *t != first_transcript {
            return Err(format!(
                "bench guest: transcript differs on timed iteration {i} (expected byte-identical \
                 transcripts across every boot of the same image)"
            ));
        }
    }
    let first_exit_code = exit_codes[0];
    if exit_codes.iter().any(|&e| e != first_exit_code) {
        return Err("bench guest: exit code differs across timed boots".to_string());
    }
    let first_exits = exits_counts[0];
    if exits_counts.iter().any(|&e| e != first_exits) {
        return Err("bench guest: vCPU exit count differs across timed boots".to_string());
    }
    let first_choice_count = choice_counts[0];
    if choice_counts.iter().any(|&c| c != first_choice_count) {
        return Err("bench guest: choice-sequence count differs across timed boots".to_string());
    }

    totals.sort();
    let min = totals[0];
    let max = totals[totals.len() - 1];
    let med = median(&totals);
    let median_us = med.as_micros();

    println!(
        "bench guest: {BENCH_GUEST_WARMUP_ITERS} warmup + {BENCH_GUEST_TIMED_ITERS} timed boot(s) \
         of tests/golden/boot-hello"
    );
    println!(
        "bench guest: wall time: min={}us median={}us max={}us",
        min.as_micros(),
        median_us,
        max.as_micros()
    );
    println!(
        "bench guest: exact counts across every timed boot: transcript={} byte(s), \
         exit_code={first_exit_code}, exits={first_exits}, choices={first_choice_count}",
        first_transcript.len()
    );

    let threshold_us = guest_bench_threshold_us()?;
    if median_us > threshold_us {
        return Err(format!(
            "bench guest: FAIL: measured median {median_us}us exceeds locked threshold \
             {threshold_us}us (bench/thresholds.toml) — an algorithmic/HVF-path regression, not \
             machine noise, is what this lock exists to catch"
        ));
    }
    println!(
        "bench guest: median {median_us}us within locked threshold {threshold_us}us (bench/thresholds.toml)"
    );
    Ok(())
}

/// `cargo xtask profile` (plans/M5.md item F, decision 14): the compiler's
/// own per-phase wall time building `tests/golden/boot-hello` (the "in-
/// process equivalent" of `--timings` — that flag lives on `wrela dump`,
/// which never builds a runtime-test image, so this fn instruments the
/// same phases by hand around the identical build call chain
/// `build_runtime_test_image` makes) plus the guest counts from one real
/// boot of the resulting image. No PMU, no flamegraphs — wall time and
/// exact counts, the plan's own "dumb sufficient version".
fn profile() -> Result<(), String> {
    let vmm = build_and_sign_vmm()?;
    let case = root().join("tests/golden/boot-hello");
    let target = golden_case_target(&case)?
        .ok_or_else(|| "profile: tests/golden/boot-hello has no input.wr".to_string())?;

    let total_start = Instant::now();

    let read_start = Instant::now();
    let source =
        std::fs::read_to_string(&target).map_err(|e| format!("read {}: {e}", target.display()))?;
    let read_time = read_start.elapsed();

    let lex_start = Instant::now();
    let tokens = lexer::lex(&source).map_err(|e| format!("lex error: {}", e.message))?;
    let lex_time = lex_start.elapsed();

    let parse_start = Instant::now();
    let module = parser::parse(tokens).map_err(|e| format!("parse error: {}", e.message))?;
    let parse_time = parse_start.elapsed();

    let path_display = target.display().to_string();
    let check_start = Instant::now();
    let program = sema::check_typed(&module, &path_display)
        .map_err(|e| format!("sema error: {}", e.message))?;
    let check_time = check_start.elapsed();

    let runtime_names: Vec<String> = program
        .tests
        .iter()
        .filter(|t| t.kind == TestKind::Runtime)
        .map(|t| t.name.clone())
        .collect();

    let mut modules: BTreeMap<String, Module> = BTreeMap::new();
    modules.insert(module.path.join("."), module.clone());
    let layout_ctx_start = Instant::now();
    let layout_ctx = layout::merge_layout_ctx(&modules).map_err(|e| e.message)?;
    let layout_ctx_time = layout_ctx_start.elapsed();

    let lower_start = Instant::now();
    let mwir_program = lower::lower_program(&program).map_err(|e| e.message)?;
    let lower_time = lower_start.elapsed();

    let codegen_start = Instant::now();
    let codegen_program =
        codegen::codegen_program(&mwir_program, &layout_ctx).map_err(|e| e.message)?;
    let codegen_time = codegen_start.elapsed();

    let image_start = Instant::now();
    // plans/M6.md item D: `None` — `bench guest` always times `boot-hello`
    // (no actors); a real actor-bearing guest bench case is named, future
    // work.
    let image_layout = layout::layout_test_image(
        &codegen_program,
        &runtime_names,
        &std::collections::BTreeSet::new(),
        None,
        &BTreeMap::new(),
    )
    .map_err(|e| e.message)?;
    let image_time = image_start.elapsed();

    let total_time = total_start.elapsed();

    println!(
        "profile: compiler (tests/golden/boot-hello) read={}us lex={}us parse={}us check={}us \
         layout_ctx={}us lower={}us codegen={}us image={}us total={}us",
        read_time.as_micros(),
        lex_time.as_micros(),
        parse_time.as_micros(),
        check_time.as_micros(),
        layout_ctx_time.as_micros(),
        lower_time.as_micros(),
        codegen_time.as_micros(),
        image_time.as_micros(),
        total_time.as_micros()
    );

    let source_digest = report::sha256_hex(source.as_bytes());
    let mut report_text = format!(
        "Machine revision={}\nInput path={path_display} digest={source_digest}\n",
        wrela_machine::MACHINE_REVISION_STR
    );
    for s in &image_layout.sections {
        report_text.push_str(&format!(
            "Section name={} base={:#x} size={}\n",
            s.name, s.base, s.size
        ));
    }
    report_text.push_str(&format!("Entry base={:#x}\n", image_layout.entry));
    layout::append_vmm_runtime_lines(&mut report_text, &image_layout);

    let tmp_dir = root().join("target/profile-tmp");
    if tmp_dir.exists() {
        std::fs::remove_dir_all(&tmp_dir)
            .map_err(|e| format!("remove {}: {e}", tmp_dir.display()))?;
    }
    std::fs::create_dir_all(&tmp_dir).map_err(|e| format!("create {}: {e}", tmp_dir.display()))?;
    let img_path = tmp_dir.join("boot.img");
    let report_path = tmp_dir.join("boot.report.txt");
    std::fs::write(&img_path, &image_layout.blob)
        .map_err(|e| format!("write {}: {e}", img_path.display()))?;
    std::fs::write(&report_path, &report_text)
        .map_err(|e| format!("write {}: {e}", report_path.display()))?;
    let record_path = tmp_dir.join("boot.record.txt");

    let guest_start = Instant::now();
    let out = Command::new(&vmm)
        .arg(&report_path)
        .arg(&img_path)
        .arg("--record")
        .arg(&record_path)
        .output()
        .map_err(|e| format!("run wrela-vmm: {e}"))?;
    let guest_wall = guest_start.elapsed();
    let exit_code_class = out.status.code().unwrap_or(-1);
    if exit_code_class != 0 && exit_code_class != 1 {
        let _ = std::fs::remove_dir_all(&tmp_dir);
        return Err(format!(
            "profile: the wrela VMM did not boot the test image (exit {exit_code_class})"
        ));
    }
    let transcript_len = out.stdout.len();
    let record_text = std::fs::read_to_string(&record_path)
        .map_err(|e| format!("read {}: {e}", record_path.display()))?;
    let record = parse_guest_record(&record_text)?;
    let _ = std::fs::remove_dir_all(&tmp_dir);

    println!(
        "profile: guest (tests/golden/boot-hello) wall={}us exits={} transcript_bytes={} choices={}",
        guest_wall.as_micros(),
        record.exits,
        transcript_len,
        record.choice_count
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

// --- bench ------------------------------------------------------------
//
// ROADMAP.md's "cleverness budget": the compiler lane lands at M1 because
// it needs nothing but the compiler and a clock (the guest lane needs the
// VMM and record/replay, so it waits for M5 — `bench guest` and bare
// `bench` still fail closed below). `cargo xtask bench compiler` times the
// whole front end — lex, then (per the same "..." fragment rule `corpus`
// uses) parse — over every corpus entry `xtask corpus` already covers
// (doc blocks + docs/language/examples/*.wr) plus every
// tests/golden/*/input.wr, in-process (no subprocess per iteration: the
// lexer/parser are called directly as library functions). One workload
// iteration is one pass over every entry; 3 untimed warmup iterations
// settle caches/allocator state, then 15 timed iterations are measured.
// Reported: min/median/max of the full-corpus total, plus the median for
// the single largest entry by source length (today, the virtio worked
// example) so a regression in the one input most likely to expose
// quadratic behavior is visible on its own. The median is then compared
// against the locked threshold in `bench/thresholds.toml` — exceeding it
// is a bench failure with both numbers printed. Wired into `check` (after
// roundtrip, before ledger): the corpus is small enough today that 18
// full passes run in milliseconds, well under `check`'s budget, and the
// median prints on every gate run so a creeping trend is visible long
// before it trips the (deliberately loose, 10x) lock.

const BENCH_WARMUP_ITERS: usize = 3;
const BENCH_TIMED_ITERS: usize = 15;

/// One corpus entry for the compiler bench: a name (for reporting) and its
/// full source text. Reuses exactly the entries `xtask corpus` walks
/// (`extract_doc_blocks` + `extract_example_files`), plus every golden
/// `input.wr` (the error-case corpus, which `corpus`/`roundtrip` don't
/// otherwise exercise as a lex+parse workload).
struct BenchEntry {
    name: String,
    body: String,
    /// Whether `body` contains the `...`-fragment marker (entry-invariant,
    /// computed once here rather than re-scanned by `run_bench_workload`
    /// on every timed iteration).
    has_dots: bool,
}

fn bench_corpus_entries() -> Result<Vec<BenchEntry>, String> {
    let (blocks, failures) = extract_doc_blocks()?;
    if let Some(f) = failures.first() {
        return Err(format!("bench: corpus is broken, fix it first: {f}"));
    }
    let examples = extract_example_files()?;
    let mut entries: Vec<BenchEntry> = blocks
        .into_iter()
        .chain(examples)
        .map(|b| BenchEntry {
            has_dots: b.body.contains("..."),
            name: b.name,
            body: b.body,
        })
        .collect();

    let golden_dir = root().join("tests/golden");
    for dir in golden_case_dirs(&golden_dir)? {
        let input = dir.join("input.wr");
        if !input.exists() {
            continue;
        }
        let case = dir
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("golden")
            .to_string();
        let body = std::fs::read_to_string(&input)
            .map_err(|e| format!("read {}: {e}", input.display()))?;
        entries.push(BenchEntry {
            has_dots: body.contains("..."),
            name: format!("golden/{case}/input.wr"),
            body,
        });
    }
    if entries.is_empty() {
        return Err("bench compiler: no corpus entries found".into());
    }
    Ok(entries)
}

/// One full workload iteration: lex every entry, and — following the same
/// `...`-fragment rule `corpus` uses — parse the ones that aren't doc
/// fragments. Outcomes (lex/parse errors, e.g. the syntax-error goldens)
/// are discarded; only wall time is measured. Also returns how long the
/// entry at `track_index` took on its own, so the caller can track one
/// entry's time across iterations without a second pass over the corpus.
fn run_bench_workload(entries: &[BenchEntry], track_index: usize) -> (Duration, Duration) {
    let mut tracked = Duration::ZERO;
    let start = Instant::now();
    for (i, e) in entries.iter().enumerate() {
        let entry_start = Instant::now();
        if let Ok(tokens) = lexer::lex(&e.body) {
            if !e.has_dots {
                let _ = parser::parse_any(tokens);
            }
        }
        if i == track_index {
            tracked = entry_start.elapsed();
        }
    }
    (start.elapsed(), tracked)
}

/// One golden entry for the bench's check lane: its full source text,
/// restricted (see `bench_check_entries`) to inputs that lex and parse
/// cleanly, so every timed iteration reaches `sema::check`.
struct CheckBenchEntry {
    body: String,
}

/// Every `tests/golden/*/input.wr` whose lex+parse succeeds — plans/M2.md
/// item I: "every input whose check currently succeeds AND every one that
/// fails sema (an error result is still a timed, valid outcome — but
/// exclude lex/parse-error inputs)". Filtering is done by actually
/// running lex+parse here (not by name pattern: `err-type-*` etc. are
/// exactly the sema-error-but-parses cases this lane must include, while
/// `err-bad-dedent`/`err-unterminated-string`/etc. must not).
fn bench_check_entries() -> Result<Vec<CheckBenchEntry>, String> {
    let golden_dir = root().join("tests/golden");
    let mut entries = Vec::new();
    for dir in golden_case_dirs(&golden_dir)? {
        let input = dir.join("input.wr");
        if !input.exists() {
            continue;
        }
        let body = std::fs::read_to_string(&input)
            .map_err(|e| format!("read {}: {e}", input.display()))?;
        let parses = match lexer::lex(&body) {
            Ok(tokens) => parser::parse(tokens).is_ok(),
            Err(_) => false,
        };
        if !parses {
            continue;
        }
        entries.push(CheckBenchEntry { body });
    }
    if entries.is_empty() {
        return Err("bench compiler (check lane): no lexing/parsing golden inputs found".into());
    }
    Ok(entries)
}

/// One full check-lane workload iteration: lex, parse, then `sema::check`
/// every entry (all three always succeed at lex/parse by construction of
/// `bench_check_entries`; `check`'s own Ok/Err outcome is discarded —
/// only wall time is measured, and an error result is as valid a timed
/// outcome as success).
fn run_check_bench_workload(entries: &[CheckBenchEntry]) -> Duration {
    let start = Instant::now();
    for e in entries {
        if let Ok(tokens) = lexer::lex(&e.body) {
            if let Ok(module) = parser::parse(tokens) {
                let _ = sema::check(&module, "<bench>");
            }
        }
    }
    start.elapsed()
}

/// One test-bearing golden entry for the bench's eval lane: its full
/// source text. Restricted (see `bench_eval_entries`) to the goldens that
/// actually pin a `test.txt` expectation — the ones `@test` fns of their
/// own, so every timed iteration reaches `eval::run_tests` and actually
/// evaluates something, not just `check_typed`'s own const/assert tail.
struct EvalBenchEntry {
    body: String,
}

/// Every `tests/golden/*/expected/test.txt`-bearing case's `input.wr`
/// (plans/M3.md item F: "evaluate the test-bearing goldens" — today
/// `check-tests-arith`/`check-tests-mixed`/`check-tests-program`, the
/// `comptime.tests.build-tier` cases with real `@test` fns to run;
/// `err-test-params` is deliberately excluded — it fails sema before any
/// test ever runs, so it has no `test.txt` at all). Detected by the same
/// expectation-file presence the golden runner itself uses, not by name
/// pattern, so a future `check-tests-*` golden is picked up automatically.
fn bench_eval_entries() -> Result<Vec<EvalBenchEntry>, String> {
    let golden_dir = root().join("tests/golden");
    let mut entries = Vec::new();
    for dir in golden_case_dirs(&golden_dir)? {
        let input = dir.join("input.wr");
        if !dir.join("expected/test.txt").exists() || !input.exists() {
            continue;
        }
        let body = std::fs::read_to_string(&input)
            .map_err(|e| format!("read {}: {e}", input.display()))?;
        entries.push(EvalBenchEntry { body });
    }
    if entries.is_empty() {
        return Err("bench compiler (eval lane): no test-bearing golden inputs found".into());
    }
    Ok(entries)
}

/// One full eval-lane workload iteration: lex, parse, `sema::check_typed`,
/// then (discarding the `Ok`/`Err` outcome of each stage exactly like
/// `run_check_bench_workload` — only wall time is measured) `run_tests`
/// on a successful typecheck. Every entry here is known-good (the golden
/// suite's own `check_typed`/`wrela test` runs already pin these as
/// accepting and producing a real report), so in practice every timed
/// iteration reaches `run_tests` — but the match still fails closed rather
/// than assuming that, exactly like the check lane above.
fn run_eval_bench_workload(entries: &[EvalBenchEntry]) -> Duration {
    let start = Instant::now();
    for e in entries {
        if let Ok(tokens) = lexer::lex(&e.body) {
            if let Ok(module) = parser::parse(tokens) {
                if let Ok(program) = sema::check_typed(&module, "<bench>") {
                    let _ = eval::run_tests(&program);
                }
            }
        }
    }
    start.elapsed()
}

/// A locked threshold from `bench/thresholds.toml`, in microseconds, read
/// from `[compiler]`'s `key`. Committed, not generated: it exists to
/// catch algorithmic blowups, not to track machine noise, so it is set
/// deliberately (see the file's own comment) rather than recomputed on
/// every run.
fn bench_threshold_us(section: &str, key: &str) -> Result<u128, String> {
    let path = root().join("bench/thresholds.toml");
    let text =
        std::fs::read_to_string(&path).map_err(|e| format!("read {}: {e}", path.display()))?;
    let value: toml::Value = text
        .parse()
        .map_err(|e| format!("parse {}: {e}", path.display()))?;
    value
        .get(section)
        .and_then(|c| c.get(key))
        .and_then(|v| v.as_integer())
        .map(|v| v as u128)
        .ok_or_else(|| format!("{}: missing [{section}] {key}", path.display()))
}

fn compiler_bench_threshold_us() -> Result<u128, String> {
    bench_threshold_us("compiler", "full_corpus_per_entry_us")
}

fn check_bench_threshold_us() -> Result<u128, String> {
    bench_threshold_us("compiler", "check_golden_per_entry_us")
}

fn eval_bench_threshold_us() -> Result<u128, String> {
    bench_threshold_us("compiler", "eval_tests_per_entry_us")
}

/// The three corpus-sized compiler lanes lock **microseconds per entry**,
/// not a whole-corpus absolute (GOAL.md, "the compiler-lane locks are
/// losing resolution"): `check_golden_median_us` had already been re-locked
/// 40000 -> 400000 as the corpus grew 25 -> 154 entries, and an absolute
/// lock dilutes on every added golden, so a real per-entry regression can
/// hide inside corpus growth forever. A per-entry lock is corpus-size
/// invariant: adding goldens moves the measured number not at all, and a
/// lane that gets 2x slower per entry trips it whatever the corpus size.
///
/// The comparison never divides the measurement. `median_ns` is checked
/// against `threshold_per_entry_us * 1000 * entries`, so the lock keeps
/// full timer resolution however small a per-entry median becomes; the
/// divided number is computed for the printout only. The `[build]` and
/// `[guest]` lanes keep absolute locks on purpose — each times one fixed
/// workload (the example appliance, one `boot-hello` boot), so there is no
/// corpus to divide by and nothing to dilute.
fn enforce_per_entry_lock(
    lane: &str,
    median: Duration,
    entries: usize,
    threshold_per_entry_us: u128,
) -> Result<(), String> {
    if entries == 0 {
        return Err(format!(
            "{lane}: FAIL: zero entries — a per-entry lock over an empty corpus measures \
             nothing, and a lane that silently benchmarks nothing is the failure this \
             check exists to make loud"
        ));
    }
    let per_entry_us = median.as_nanos() / 1000 / entries as u128;
    let budget_ns = threshold_per_entry_us * 1000 * entries as u128;
    if median.as_nanos() > budget_ns {
        return Err(format!(
            "{lane}: FAIL: measured {per_entry_us}us/entry over {entries} entries exceeds \
             locked threshold {threshold_per_entry_us}us/entry (bench/thresholds.toml) — an \
             algorithmic blowup, not machine noise, is what this lock exists to catch"
        ));
    }
    println!(
        "{lane}: {per_entry_us}us/entry over {entries} entries, within locked \
         {threshold_per_entry_us}us/entry (bench/thresholds.toml)"
    );
    Ok(())
}

/// plans/M4.md item E: `xtask bench build`'s own locked median, kept in its
/// own `[build]` table (not `[compiler]`) since this lane times a
/// different pipeline tail (loader -> sema -> `eval_image` -> graph checks
/// -> report render) over a single project, not lex/parse/`check`/
/// `run_tests` over the whole corpus — a distinct measurement deserves its
/// own section, not a fourth same-named key crowding `[compiler]`.
fn build_bench_threshold_us() -> Result<u128, String> {
    bench_threshold_us("build", "build_appliance_median_us")
}

fn median(sorted: &[Duration]) -> Duration {
    sorted[sorted.len() / 2]
}

fn bench_compiler() -> Result<(), String> {
    let entries = bench_corpus_entries()?;
    let (track_index, largest_name, largest_len) = entries
        .iter()
        .enumerate()
        .max_by_key(|(_, e)| e.body.len())
        .map(|(i, e)| (i, e.name.clone(), e.body.len()))
        .expect("bench_corpus_entries never returns empty");

    for _ in 0..BENCH_WARMUP_ITERS {
        run_bench_workload(&entries, track_index);
    }

    let mut totals = Vec::with_capacity(BENCH_TIMED_ITERS);
    let mut tracked = Vec::with_capacity(BENCH_TIMED_ITERS);
    for _ in 0..BENCH_TIMED_ITERS {
        let (total, entry) = run_bench_workload(&entries, track_index);
        totals.push(total);
        tracked.push(entry);
    }
    totals.sort();
    tracked.sort();

    let min = totals[0];
    let max = totals[totals.len() - 1];
    let med = median(&totals);
    let tracked_med = median(&tracked);
    let median_us = med.as_micros();

    println!(
        "bench compiler: {} corpus entries, {BENCH_WARMUP_ITERS} warmup + {BENCH_TIMED_ITERS} timed iteration(s)",
        entries.len()
    );
    println!(
        "bench compiler: full corpus total: min={}us median={}us max={}us",
        min.as_micros(),
        median_us,
        max.as_micros()
    );
    println!(
        "bench compiler: largest entry `{largest_name}` ({largest_len} bytes): median={}us",
        tracked_med.as_micros()
    );

    enforce_per_entry_lock(
        "bench compiler",
        med,
        entries.len(),
        compiler_bench_threshold_us()?,
    )?;

    bench_check_lane()
}

/// The check lane (plans/M2.md item I): lex+parse+`sema::check` over
/// every golden input that lexes and parses (both sema-ok and
/// sema-error outcomes are timed; lex/parse-error golden inputs are
/// excluded — see `bench_check_entries`). Same 3 warmup + 15 timed shape
/// as the lex+parse lane above, its own locked median
/// (`check_golden_per_entry_us`, kept separate from `full_corpus_per_entry_us`
/// so the two lanes never mask one another).
fn bench_check_lane() -> Result<(), String> {
    let entries = bench_check_entries()?;

    for _ in 0..BENCH_WARMUP_ITERS {
        run_check_bench_workload(&entries);
    }

    let mut totals = Vec::with_capacity(BENCH_TIMED_ITERS);
    for _ in 0..BENCH_TIMED_ITERS {
        totals.push(run_check_bench_workload(&entries));
    }
    totals.sort();

    let min = totals[0];
    let max = totals[totals.len() - 1];
    let med = median(&totals);
    let median_us = med.as_micros();

    println!(
        "bench compiler (check lane): {} golden entries, {BENCH_WARMUP_ITERS} warmup + \
         {BENCH_TIMED_ITERS} timed iteration(s)",
        entries.len()
    );
    println!(
        "bench compiler (check lane): total: min={}us median={}us max={}us",
        min.as_micros(),
        median_us,
        max.as_micros()
    );

    enforce_per_entry_lock(
        "bench compiler (check lane)",
        med,
        entries.len(),
        check_bench_threshold_us()?,
    )?;
    bench_eval_lane()
}

/// The eval lane (plans/M3.md item F): full pipeline + `eval::run_tests`
/// over every test-bearing golden (`bench_eval_entries`). Same 3 warmup +
/// 15 timed shape as the other two lanes, its own locked median
/// (`eval_tests_per_entry_us`, kept separate from the other two thresholds
/// for the same reason `check_golden_per_entry_us` is kept separate from
/// `full_corpus_per_entry_us` — one lane's regression must never mask
/// another's).
fn bench_eval_lane() -> Result<(), String> {
    let entries = bench_eval_entries()?;

    for _ in 0..BENCH_WARMUP_ITERS {
        run_eval_bench_workload(&entries);
    }

    let mut totals = Vec::with_capacity(BENCH_TIMED_ITERS);
    for _ in 0..BENCH_TIMED_ITERS {
        totals.push(run_eval_bench_workload(&entries));
    }
    totals.sort();

    let min = totals[0];
    let max = totals[totals.len() - 1];
    let med = median(&totals);
    let median_us = med.as_micros();

    println!(
        "bench compiler (eval lane): {} test-bearing golden entries, {BENCH_WARMUP_ITERS} warmup + \
         {BENCH_TIMED_ITERS} timed iteration(s)",
        entries.len()
    );
    println!(
        "bench compiler (eval lane): total: min={}us median={}us max={}us",
        min.as_micros(),
        median_us,
        max.as_micros()
    );

    enforce_per_entry_lock(
        "bench compiler (eval lane)",
        med,
        entries.len(),
        eval_bench_threshold_us()?,
    )?;
    Ok(())
}

// --- bench: build lane (plans/M4.md item E) --------------------------------
//
// `xtask bench build`: in-process, times the *whole build pipeline* —
// loader (or the single-file fork) -> `sema::check_program_typed` (or
// `check_typed`) -> `eval::interp::eval_image` -> `eval::image_checks::
// check_sealed` -> `report::render` — over the M4 example appliance
// (`golden/appliance`, the milestone's own flagship project), the same
// 3-warmup + 15-timed shape every other bench lane already uses. No file
// writes: this reuses `produce_report_text` verbatim (the same in-process
// pipeline `report-determinism` already calls twice per case to prove
// determinism — here it's timed instead, once per iteration), which only
// ever reads source bytes off disk to hash them, never writes anything —
// exactly the "loader -> sema -> eval -> checks -> report render, no file
// writes" the plan asks this lane to measure. A distinct locked median
// (`build_appliance_median_us`, its own `[build]` table in
// `bench/thresholds.toml`) rather than folding into any `[compiler]` key,
// for the same reason the check/eval lanes above keep their own thresholds
// separate from each other and from the lex+parse lane: one lane's
// regression must never mask another's.

/// The appliance golden case's own root target
/// (`tests/golden/appliance/src/image.wr`) — resolved through the same
/// `golden_case_target` the golden runner and `report-determinism` both
/// use, rather than a second hardcoded path, so this lane can never point
/// at a file that has silently stopped existing.
fn bench_build_target() -> Result<PathBuf, String> {
    let case = root().join("tests/golden/appliance");
    golden_case_target(&case)?
        .ok_or_else(|| "bench build: tests/golden/appliance has no `root`-named target".to_string())
}

/// One full build-lane workload iteration: `produce_report_and_image` over
/// the appliance's root target, discarding the outcome (a rendered report,
/// or a well-formed diagnostic — either is as valid a timed outcome as the
/// other, exactly like the compiler bench's check/eval lanes above) —
/// only wall time is measured.
fn run_build_bench_workload(target: &Path) -> Duration {
    let start = Instant::now();
    let _ = produce_report_and_image(target);
    start.elapsed()
}

fn bench_build_lane() -> Result<(), String> {
    let target = bench_build_target()?;

    for _ in 0..BENCH_WARMUP_ITERS {
        run_build_bench_workload(&target);
    }

    let mut totals = Vec::with_capacity(BENCH_TIMED_ITERS);
    for _ in 0..BENCH_TIMED_ITERS {
        totals.push(run_build_bench_workload(&target));
    }
    totals.sort();

    let min = totals[0];
    let max = totals[totals.len() - 1];
    let med = median(&totals);
    let median_us = med.as_micros();

    println!(
        "bench build: {BENCH_WARMUP_ITERS} warmup + {BENCH_TIMED_ITERS} timed iteration(s) over \
         the example appliance"
    );
    println!(
        "bench build: total: min={}us median={}us max={}us",
        min.as_micros(),
        median_us,
        max.as_micros()
    );

    let threshold_us = build_bench_threshold_us()?;
    if median_us > threshold_us {
        return Err(format!(
            "bench build: FAIL: measured median {median_us}us exceeds locked threshold \
             {threshold_us}us (bench/thresholds.toml) — an algorithmic blowup, not machine \
             noise, is what this lock exists to catch"
        ));
    }
    println!(
        "bench build: median {median_us}us within locked threshold {threshold_us}us (bench/thresholds.toml)"
    );
    Ok(())
}

fn bench(args: &[String]) -> Result<(), String> {
    match args.first().map(String::as_str) {
        Some("compiler") => bench_compiler(),
        Some("build") => bench_build_lane(),
        Some("guest") => bench_guest_lane(),
        None => fail_closed(
            "bench",
            "bare `bench` fails closed; run `bench compiler`, `bench build`, or `bench guest` \
             (all live)",
        ),
        Some(other) => Err(format!(
            "bench: unknown lane `{other}` (expected `compiler`, `build`, or `guest`)"
        )),
    }
}

// --- ledger ---------------------------------------------------------------
//
// ledger/ledger.toml maps normative clauses in docs/language/ to the tests
// that enforce them. Every clause has status "test" (with existing test
// paths) or "gap" (explicit, visible debt). This measures coverage of the
// SPEC, not of the code.

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
