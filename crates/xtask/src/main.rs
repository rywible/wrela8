//! Local development harness. There is no CI: `cargo xtask check` IS the
//! definition of "the tree is good", run locally before calling anything
//! done. Subcommands:
//!
//!   check      fmt + tests + golden + corpus + fuzz(smoke) + ledger (the gate)
//!   golden     run golden tests; `--update` rewrites expectations
//!   corpus     extract every ```wrela block from docs/ and lex it
//!   fuzz       cargo xtask fuzz [lexer|parser|sema|eval|lower] [--iters N]
//!              [--seed S]; deterministic in-tree fuzzer (plans/M1.md
//!              items B/E, plans/M2.md item I, plans/M3.md item F,
//!              plans/M5.md item G). All five targets are live (bare
//!              `fuzz` runs `lexer` at the deep default budget); every
//!              target but `lower` has its own smoke budget wired into
//!              `check` (`lower`'s own smoke fn exists and runs standalone
//!              — `fuzz_lower_smoke`'s own doc comment explains exactly why
//!              it is not yet called from `check()`: it currently
//!              reproduces a real, pinned, out-of-scope `sema::bodies`
//!              finding at essentially every seed, well inside any smoke
//!              budget). `sema` runs lex -> parse -> `sema::check` over
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
//!              compiler.lower.no-panics).
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
//!              median (`check_golden_median_us`). plans/M3.md item F adds
//!              a third lane: lex+parse+`sema::check_typed`+
//!              `eval::run_tests` over every test-bearing golden (the
//!              `check-tests-*` cases with a pinned `test.txt`), same
//!              3+15 shape, its own locked median (`eval_tests_median_us`).
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
use wrela_compiler::layout;
use wrela_compiler::loader;
use wrela_compiler::lower;
use wrela_compiler::mwir;
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
        Some("profile") => profile(),
        Some("fuzz") => fuzz(&args[1..]),
        Some("bench") => bench(&args[1..]),
        _ => {
            eprintln!(
                "usage: cargo xtask <check|golden [--update]|corpus|fuzz [lexer|parser|sema|eval|lower] [--iters N] [--seed S]|roundtrip|report-determinism|ledger|repro|diff-eval|profile|bench <compiler|build|guest>>"
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
        Some(a) if a == "lexer" || a == "parser" || a == "sema" || a == "eval" || a == "lower" => {
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
        other => Err(format!(
            "fuzz: unknown target `{other}` (expected `lexer`, `parser`, `sema`, `eval`, or `lower`)"
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
    let mut bytes = seed_inputs[rng.gen_range(seed_inputs.len())]
        .as_bytes()
        .to_vec();
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
                let other = seed_inputs[rng.gen_range(seed_inputs.len())].as_bytes();
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
                match report::render(&[build_input], &program.enums, &graph) {
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
    match layout::layout_test_image(codegen_program, &runtime_tests) {
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
                        match report::render(&inputs, &program.enums, &graph) {
                            Ok(mut text) => {
                                let layout_ctx = layout::merge_layout_ctx(&modules_by_addr)
                                    .map_err(|e| render_sema_error(&e))?;
                                let img = match layout::try_layout_program(&programs, &layout_ctx) {
                                    Ok(Some(image_layout)) => {
                                        layout::render_layout_section(&mut text, &image_layout);
                                        Some(image_layout.blob)
                                    }
                                    Ok(None) => None,
                                    Err(e) => return Err(format!("layout: {e}")),
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

/// `cargo xtask repro` (plans/M5.md decision 10, item F): the standalone,
/// full-corpus form — `report_determinism`'s own `@image`/`wrela build`
/// population plus `repro_test_image`'s one runtime-test-image case,
/// so bare `repro` covers every image-emitting path this milestone has,
/// not only the one `report_determinism` alone can reach.
fn repro() -> Result<(), String> {
    report_determinism()?;
    repro_test_image()
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
    let codegen_program =
        codegen::codegen_program(&mwir_program, &layout_ctx).map_err(|e| e.message)?;
    let image_layout =
        layout::layout_test_image(&codegen_program, test_names).map_err(|e| e.message)?;
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
/// the three fields `bench guest`/`profile` actually need.
struct GuestRecord {
    exit_code: u64,
    exits: u64,
    clock_log_len: usize,
}

fn parse_guest_record(text: &str) -> Result<GuestRecord, String> {
    let mut exit_code = None;
    let mut exits = None;
    let mut clock_log_len = None;
    for line in text.lines() {
        if let Some(v) = line.strip_prefix("exit_code=") {
            exit_code = Some(v.parse().map_err(|e| format!("bad exit_code {v:?}: {e}"))?);
        } else if let Some(v) = line.strip_prefix("exits=") {
            exits = Some(v.parse().map_err(|e| format!("bad exits {v:?}: {e}"))?);
        } else if let Some(v) = line.strip_prefix("clock_log_len=") {
            clock_log_len = Some(
                v.parse()
                    .map_err(|e| format!("bad clock_log_len {v:?}: {e}"))?,
            );
        }
    }
    Ok(GuestRecord {
        exit_code: exit_code.ok_or("record file: missing exit_code")?,
        exits: exits.ok_or("record file: missing exits")?,
        clock_log_len: clock_log_len.ok_or("record file: missing clock_log_len")?,
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
// (`RecordFile`'s own `exits`/`clock_log_len`/`exit_code`, plus the
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
    let case = root().join("tests/golden/boot-hello");
    let target = golden_case_target(&case)?
        .ok_or_else(|| "tests/golden/boot-hello has no input.wr".to_string())?;
    let source =
        std::fs::read_to_string(&target).map_err(|e| format!("read {}: {e}", target.display()))?;
    let path_display = target.display().to_string();
    let (module, program) = typecheck_single_module(&source, &path_display)
        .ok_or_else(|| "tests/golden/boot-hello failed to typecheck".to_string())?;
    let runtime_names: Vec<String> = program
        .tests
        .iter()
        .filter(|t| t.kind == TestKind::Runtime)
        .map(|t| t.name.clone())
        .collect();
    if runtime_names.is_empty() {
        return Err("tests/golden/boot-hello declares no @test(runtime) fns".to_string());
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
    }
    let _ = std::fs::remove_dir_all(&tmp_dir);

    // decision 14's own "exact counts" half: every timed boot of the
    // identical image must produce byte-identical transcripts and
    // identical exit codes/exit counts — anything else is a real
    // nondeterminism bug (the generated runtime or the VMM), never
    // ordinary machine noise.
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
         exit_code={first_exit_code}, exits={first_exits}",
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
    let image_layout =
        layout::layout_test_image(&codegen_program, &runtime_names).map_err(|e| e.message)?;
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
        "profile: guest (tests/golden/boot-hello) wall={}us exits={} transcript_bytes={} clock_reads={}",
        guest_wall.as_micros(),
        record.exits,
        transcript_len,
        record.clock_log_len
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
    bench_threshold_us("compiler", "full_corpus_median_us")
}

fn check_bench_threshold_us() -> Result<u128, String> {
    bench_threshold_us("compiler", "check_golden_median_us")
}

fn eval_bench_threshold_us() -> Result<u128, String> {
    bench_threshold_us("compiler", "eval_tests_median_us")
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

    let threshold_us = compiler_bench_threshold_us()?;
    if median_us > threshold_us {
        return Err(format!(
            "bench compiler: FAIL: measured median {median_us}us exceeds locked threshold \
             {threshold_us}us (bench/thresholds.toml) — an algorithmic blowup, not machine \
             noise, is what this lock exists to catch"
        ));
    }
    println!(
        "bench compiler: median {median_us}us within locked threshold {threshold_us}us (bench/thresholds.toml)"
    );

    bench_check_lane()
}

/// The check lane (plans/M2.md item I): lex+parse+`sema::check` over
/// every golden input that lexes and parses (both sema-ok and
/// sema-error outcomes are timed; lex/parse-error golden inputs are
/// excluded — see `bench_check_entries`). Same 3 warmup + 15 timed shape
/// as the lex+parse lane above, its own locked median
/// (`check_golden_median_us`, kept separate from `full_corpus_median_us`
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

    let threshold_us = check_bench_threshold_us()?;
    if median_us > threshold_us {
        return Err(format!(
            "bench compiler (check lane): FAIL: measured median {median_us}us exceeds locked \
             threshold {threshold_us}us (bench/thresholds.toml) — an algorithmic blowup, not \
             machine noise, is what this lock exists to catch"
        ));
    }
    println!(
        "bench compiler (check lane): median {median_us}us within locked threshold {threshold_us}us (bench/thresholds.toml)"
    );
    bench_eval_lane()
}

/// The eval lane (plans/M3.md item F): full pipeline + `eval::run_tests`
/// over every test-bearing golden (`bench_eval_entries`). Same 3 warmup +
/// 15 timed shape as the other two lanes, its own locked median
/// (`eval_tests_median_us`, kept separate from the other two thresholds
/// for the same reason `check_golden_median_us` is kept separate from
/// `full_corpus_median_us` — one lane's regression must never mask
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

    let threshold_us = eval_bench_threshold_us()?;
    if median_us > threshold_us {
        return Err(format!(
            "bench compiler (eval lane): FAIL: measured median {median_us}us exceeds locked \
             threshold {threshold_us}us (bench/thresholds.toml) — an algorithmic blowup, not \
             machine noise, is what this lock exists to catch"
        ));
    }
    println!(
        "bench compiler (eval lane): median {median_us}us within locked threshold {threshold_us}us (bench/thresholds.toml)"
    );
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
