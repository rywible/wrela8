//! Local development harness. There is no CI: `cargo xtask check` IS the
//! definition of "the tree is good", run locally before calling anything
//! done. Subcommands:
//!
//!   check      fmt + tests + golden + corpus + fuzz(smoke) + ledger (the gate)
//!   golden     run golden tests; `--update` rewrites expectations
//!   corpus     extract every ```wrela block from docs/ and lex it
//!   fuzz       cargo xtask fuzz [lexer|parser|sema|eval] [--iters N]
//!              [--seed S]; deterministic in-tree fuzzer (plans/M1.md
//!              items B/E, plans/M2.md item I, plans/M3.md item F). All
//!              four targets are live (bare `fuzz` runs `lexer` at the
//!              deep default budget); each has its own smoke budget wired
//!              into `check`. `sema` runs lex -> parse -> `sema::check`
//!              over corpus/golden-input mutations and token-soup, same
//!              shape as `parser`, plus (on every iteration whose input
//!              parses, ledger clause sema.check.roundtrip-stable) two
//!              more invariants: sema roundtrip stability (pretty-print,
//!              reparse, recheck — the two sema outcomes must agree) and
//!              item-rotation acceptance invariance (rotating the
//!              module's top-level items by one must not flip Ok/Err
//!              either way). `eval` runs lex -> parse ->
//!              `sema::check_typed` (which already evaluates every const
//!              initializer and `comptime assert`) -> on success,
//!              `eval::run_tests` over every comptime-legal `@test`, same
//!              corpus/token-soup shape again; invariants: never panics,
//!              deterministic across two runs, and every outcome is a
//!              well-formed diagnostic or test report (ledger clause
//!              comptime.eval.no-panics).
//!   roundtrip  pretty-print every parseable corpus entry and golden input,
//!              reparse it, and compare the two AST dumps (spans stripped)
//!              — the parser's `diff-eval` (plans/M1.md item E). Also runs
//!              the same sema-roundtrip oracle as `fuzz sema` above,
//!              whenever the entry parses as a whole `Module` (ledger
//!              clause sema.check.roundtrip-stable). Wired into `check`,
//!              after `corpus`.
//!   ledger     verify spec-coverage ledger (ledger/ledger.toml)
//!   repro      build an image twice, compare bytes   (fails closed today)
//!   diff-eval  evaluator-vs-backend differential      (fails closed today)
//!   profile    replay a recorded workload under counters (fails closed today)
//!   bench      cargo xtask bench compiler|guest; the compiler lane is
//!              live (plans/M1.md, ROADMAP.md "cleverness budget"): lex +
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
//!              Wired into `check`, after roundtrip. `bench guest` and
//!              bare `bench` still fail closed — the guest lane needs the
//!              VMM and record/replay, which land at M5.
//!
//! The cleverness budget (ROADMAP.md): optimizations land only with a
//! profile, a before/after on the same recording, and a lock. `bench
//! compiler` is that lock for the compiler's own speed; the guest lane
//! (`bench guest`) and `profile` still refuse to fake results until M5
//! gives them a machine to measure.
//!
//! Golden discipline: an expectation file changes only together with a
//! ledger clause that justifies it. The golden diff is the review surface.

use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};
use std::time::{Duration, Instant};

use wrela_compiler::eval;
use wrela_compiler::sema;
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
        Some("ledger") => ledger(),
        Some("repro") => fail_closed("repro", "requires image emission (backend not implemented)"),
        Some("diff-eval") => fail_closed(
            "diff-eval",
            "requires the evaluator and backend (not implemented)",
        ),
        Some("profile") => fail_closed(
            "profile",
            "requires record/replay on the VMM (lands at M5); no profile may be faked",
        ),
        Some("fuzz") => fuzz(&args[1..]),
        Some("bench") => bench(&args[1..]),
        _ => {
            eprintln!(
                "usage: cargo xtask <check|golden [--update]|corpus|fuzz [lexer|parser|sema|eval] [--iters N] [--seed S]|roundtrip|ledger|repro|diff-eval|profile|bench <compiler|guest>>"
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
        Command::new("cargo").args(["test", "--workspace", "--quiet"]),
        "cargo test",
    )?;
    golden(false)?;
    corpus()?;
    fuzz_lexer_smoke()?;
    fuzz_parser_smoke()?;
    fuzz_sema_smoke()?;
    fuzz_eval_smoke()?;
    roundtrip()?;
    bench_compiler()?;
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
        Some(a) if a == "lexer" || a == "parser" || a == "sema" || a == "eval" => {
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
        other => Err(format!(
            "fuzz: unknown target `{other}` (expected `lexer`, `parser`, `sema`, or `eval`)"
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

/// Every input `xtask corpus` already lexes: doc blocks plus golden
/// `input.wr` files, in deterministic (sorted) order. This is the corpus
/// half of the fuzzer's mutation strategy and reuses `extract_doc_blocks`
/// rather than re-walking the docs.
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
/// successful typecheck) `eval::run_tests`. Exactly one of these four
/// shapes comes back — never a panic, per `check_eval_invariants`'s
/// `catch_unwind`.
enum EvalPipelineOutcome {
    /// A successful typecheck, reduced to `run_tests`'s own report text
    /// (determinism means the *same* input reproduces a byte-identical
    /// report too — including which comptime-legal `@test`s passed,
    /// failed, or hit their quota).
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
        extra_lines: Vec<String>,
        omit_location: bool,
    },
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
                    EvalPipelineOutcome::Ok(report)
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

/// Every invariant the eval fuzzer checks, once per iteration, on one
/// input. Runs the whole lex-then-parse-then-check_typed-then-(run_tests)
/// pipeline twice under `catch_unwind`, mirroring `check_sema_invariants`'s
/// shape, plus the well-formedness check (invariant (d)) on a successful
/// outcome and the fixed-category check (also (d)) on a `SemaErr`
/// outcome.
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
    if let EvalPipelineOutcome::Ok(report) = &first {
        report_is_well_formed(report)?;
    }

    match (&first, &second) {
        (EvalPipelineOutcome::Ok(r1), EvalPipelineOutcome::Ok(r2)) => {
            if r1 != r2 {
                return Err(
                    "eval is not deterministic: two runs produced different test reports".into(),
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

fn golden(update: bool) -> Result<(), String> {
    run(
        Command::new("cargo").args(["build", "--quiet", "-p", "wrela-compiler", "--bin", "wrela"]),
        "cargo build wrela",
    )?;
    let wrela = root().join("target/debug/wrela");
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
            if stage != "test" && !out.status.success() {
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
fn bench_threshold_us(key: &str) -> Result<u128, String> {
    let path = root().join("bench/thresholds.toml");
    let text =
        std::fs::read_to_string(&path).map_err(|e| format!("read {}: {e}", path.display()))?;
    let value: toml::Value = text
        .parse()
        .map_err(|e| format!("parse {}: {e}", path.display()))?;
    value
        .get("compiler")
        .and_then(|c| c.get(key))
        .and_then(|v| v.as_integer())
        .map(|v| v as u128)
        .ok_or_else(|| format!("{}: missing [compiler] {key}", path.display()))
}

fn compiler_bench_threshold_us() -> Result<u128, String> {
    bench_threshold_us("full_corpus_median_us")
}

fn check_bench_threshold_us() -> Result<u128, String> {
    bench_threshold_us("check_golden_median_us")
}

fn eval_bench_threshold_us() -> Result<u128, String> {
    bench_threshold_us("eval_tests_median_us")
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

fn bench(args: &[String]) -> Result<(), String> {
    match args.first().map(String::as_str) {
        Some("compiler") => bench_compiler(),
        Some("guest") => fail_closed(
            "bench guest",
            "the guest lane needs the VMM and record/replay and lands at M5; a threshold \
             without a measurement is a lie",
        ),
        None => fail_closed(
            "bench",
            "bare `bench` fails closed; run `bench compiler` (live) or `bench guest` (M5)",
        ),
        Some(other) => Err(format!(
            "bench: unknown lane `{other}` (expected `compiler` or `guest`)"
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
