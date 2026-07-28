//! `fuzz` subcommand and helpers (extracted from main.rs).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;
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

use crate::corpus::{extract_doc_blocks, extract_example_files};
use crate::{fail_closed, golden_case_dirs, root};

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

pub(crate) const FUZZ_LEXER_DEEP_ITERS: u64 = 200_000;
pub(crate) const FUZZ_LEXER_DEEP_SEED: u64 = 1;
// Wired into `check` (after corpus, before ledger): two fixed seeds, 1_000
// iterations each, so the gate stays well under a second and fully
// deterministic — no seed ever comes from the clock or the environment.
pub(crate) const FUZZ_LEXER_SMOKE_SEEDS: &[u64] = &[1, 2];
pub(crate) const FUZZ_LEXER_SMOKE_ITERS_PER_SEED: u64 = 1_000;

/// splitmix64: the entire PRNG. No external crate — a fuzzer this dumb
/// does not need one, and determinism-by-construction (ROADMAP.md) means
/// the generator itself must never change behavior across platforms.
pub(crate) struct Rng(u64);

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

pub(crate) fn fuzz(args: &[String]) -> Result<(), String> {
    let (target, rest) = match args.first() {
        Some(a)
            if a == "lexer"
                || a == "parser"
                || a == "sema"
                || a == "eval"
                || a == "lower"
                || a == "async"
                || a == "imports" =>
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
        "imports" => {
            let iters = parse_flag_u64(rest, "--iters")?.unwrap_or(FUZZ_IMPORTS_DEEP_ITERS);
            let seed = parse_flag_u64(rest, "--seed")?.unwrap_or(FUZZ_IMPORTS_DEEP_SEED);
            fuzz_imports(iters, seed)
        }
        other => Err(format!(
            "fuzz: unknown target `{other}` (expected `lexer`, `parser`, `sema`, `eval`, \
             `lower`, `async`, or `imports`)"
        )),
    }
}

pub(crate) fn parse_flag_u64(args: &[String], flag: &str) -> Result<Option<u64>, String> {
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
pub(crate) const PROJECT_SEED_CASES: &[&str] = &[
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
pub(crate) fn collect_wr_files(dir: &Path, out: &mut Vec<PathBuf>) -> Result<(), String> {
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
pub(crate) fn project_seed_inputs() -> Result<Vec<String>, String> {
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
pub(crate) fn corpus_seed_inputs() -> Result<Vec<String>, String> {
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
pub(crate) fn random_byte(rng: &mut Rng) -> u8 {
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
pub(crate) fn random_input(rng: &mut Rng) -> Vec<u8> {
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
pub(crate) fn mutate_seed_input(rng: &mut Rng, seed_inputs: &[String]) -> Vec<u8> {
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
pub(crate) fn mutate_seed_input_from(
    rng: &mut Rng,
    bases: &[String],
    donors: &[String],
) -> Vec<u8> {
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

/// How far one lexer-fuzz input got — every iteration reaches the lexer
/// itself (that is this lane's whole surface); the split is Ok vs Err so
/// a future collapse into "all Err, never a real token stream" is visible.
#[derive(Debug, Clone, Default)]
pub(crate) struct LexerReach {
    lex_ok: bool,
}

#[derive(Default)]
pub(crate) struct LexerReachTotals {
    lex_ok: u64,
    lex_err: u64,
}

impl LexerReachTotals {
    fn add(&mut self, r: &LexerReach) {
        if r.lex_ok {
            self.lex_ok += 1;
        } else {
            self.lex_err += 1;
        }
    }
}

/// Every invariant the fuzzer checks, once per iteration, on one input.
/// Lexes twice under `catch_unwind` (a panic is a finding, not a crash) so
/// the determinism invariant and the no-panic invariant share one call
/// shape. Returns measured reach on success (plans/M9.md item PP).
pub(crate) fn check_lex_invariants(input: &str) -> Result<LexerReach, String> {
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
            check_ok_invariants(t1)?;
            Ok(LexerReach { lex_ok: true })
        }
        (Err(e1), Err(e2)) => {
            if e1.message != e2.message || e1.line != e2.line || e1.col != e2.col {
                return Err(
                    "lexing is not deterministic: two runs produced different errors".into(),
                );
            }
            Ok(LexerReach { lex_ok: false })
        }
        _ => Err("lexing is not deterministic: one run errored and the other did not".into()),
    }
}

pub(crate) fn panic_message(payload: &(dyn std::any::Any + Send)) -> String {
    if let Some(s) = payload.downcast_ref::<&str>() {
        s.to_string()
    } else if let Some(s) = payload.downcast_ref::<String>() {
        s.clone()
    } else if let Some(s) = payload.downcast_ref::<Box<str>>() {
        s.to_string()
    } else {
        format!("non-string panic payload (type_id={:?})", payload.type_id())
    }
}

pub(crate) fn tokens_equal(a: &[Token], b: &[Token]) -> bool {
    a.len() == b.len()
        && a.iter().zip(b).all(|(x, y)| {
            x.kind == y.kind && x.text == y.text && x.line == y.line && x.col == y.col
        })
}

pub(crate) fn check_ok_invariants(tokens: &[Token]) -> Result<(), String> {
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

pub(crate) fn run_lexer_fuzz(iters: u64, seed: u64, seed_inputs: &[String]) -> Result<(), String> {
    let mut rng = Rng::new(seed);
    let mut totals = LexerReachTotals::default();
    for i in 0..iters {
        let bytes = if i % 2 == 0 {
            random_input(&mut rng)
        } else {
            mutate_seed_input(&mut rng, seed_inputs)
        };
        let input = String::from_utf8_lossy(&bytes).into_owned();
        match check_lex_invariants(&input) {
            Ok(reach) => totals.add(&reach),
            Err(reason) => {
                return report_fuzz_failure("lexer", "crash-", seed, i, &input, &reason);
            }
        }
    }
    println!(
        "fuzz lexer: {iters} iteration(s) clean (seed={seed}); reached lex Ok {}, lex Err {}",
        totals.lex_ok, totals.lex_err,
    );
    Ok(())
}

pub(crate) fn report_fuzz_failure(
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
pub(crate) fn with_silenced_panic_hook<F: FnOnce() -> Result<(), String>>(
    f: F,
) -> Result<(), String> {
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(|_| {}));
    let result = f();
    std::panic::set_hook(previous);
    result
}

pub(crate) fn fuzz_lexer(iters: u64, seed: u64) -> Result<(), String> {
    let seed_inputs = corpus_seed_inputs()?;
    with_silenced_panic_hook(|| run_lexer_fuzz(iters, seed, &seed_inputs))
}

pub(crate) fn fuzz_lexer_smoke() -> Result<(), String> {
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

pub(crate) const FUZZ_PARSER_DEEP_ITERS: u64 = 100_000;
pub(crate) const FUZZ_PARSER_DEEP_SEED: u64 = 1;
pub(crate) const FUZZ_PARSER_SMOKE_SEEDS: &[u64] = &[1, 2];
pub(crate) const FUZZ_PARSER_SMOKE_ITERS_PER_SEED: u64 = 1_000;

/// One full run of the pipeline the parser fuzzer exercises: lex, then (on
/// success) parse via `parse_any`. Exactly one of these four shapes comes
/// back — never a panic, per `check_parse_invariants`'s `catch_unwind`.
pub(crate) enum PipelineOutcome {
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

pub(crate) fn run_pipeline_once(input: &str) -> PipelineOutcome {
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

/// Measured reach for the parser lane (plans/M9.md item PP): how many
/// inputs got past lex into `parse_any`, and how many died at lex.
#[derive(Debug, Clone, Default)]
pub(crate) struct ParserReach {
    /// `parser::parse_any` ran (lex succeeded).
    parsed: bool,
    /// That parse accepted the input.
    parse_ok: bool,
}

#[derive(Default)]
pub(crate) struct ParserReachTotals {
    parse_ok: u64,
    parse_err: u64,
    died_lex: u64,
}

impl ParserReachTotals {
    fn add(&mut self, r: &ParserReach) {
        if !r.parsed {
            self.died_lex += 1;
        } else if r.parse_ok {
            self.parse_ok += 1;
        } else {
            self.parse_err += 1;
        }
    }
}

pub(crate) fn parser_reach_of(o: &PipelineOutcome) -> ParserReach {
    match o {
        PipelineOutcome::Ok(_) => ParserReach {
            parsed: true,
            parse_ok: true,
        },
        PipelineOutcome::ParseErr { .. } => ParserReach {
            parsed: true,
            parse_ok: false,
        },
        PipelineOutcome::LexErr { .. } => ParserReach {
            parsed: false,
            parse_ok: false,
        },
    }
}

/// Every invariant the parser fuzzer checks, once per iteration, on one
/// input. Runs the whole lex-then-parse pipeline twice under
/// `catch_unwind` (a panic in either stage is a finding), mirroring
/// `check_lex_invariants`'s shape. Returns measured reach on success
/// (plans/M9.md item PP).
pub(crate) fn check_parse_invariants(input: &str) -> Result<ParserReach, String> {
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
            Ok(parser_reach_of(&first))
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
            Ok(parser_reach_of(&first))
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
            Ok(parser_reach_of(&first))
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
pub(crate) fn token_soup(rng: &mut Rng) -> String {
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

pub(crate) fn run_parser_fuzz(iters: u64, seed: u64, seed_inputs: &[String]) -> Result<(), String> {
    let mut rng = Rng::new(seed);
    let mut totals = ParserReachTotals::default();
    for i in 0..iters {
        let input = if i % 2 == 0 {
            String::from_utf8_lossy(&mutate_seed_input(&mut rng, seed_inputs)).into_owned()
        } else {
            token_soup(&mut rng)
        };
        match check_parse_invariants(&input) {
            Ok(reach) => totals.add(&reach),
            Err(reason) => {
                return report_fuzz_failure("parser", "parse-crash-", seed, i, &input, &reason);
            }
        }
    }
    println!(
        "fuzz parser: {iters} iteration(s) clean (seed={seed}); reached parse Ok {}, parse Err {}, \
         died at lex {}",
        totals.parse_ok, totals.parse_err, totals.died_lex,
    );
    Ok(())
}

pub(crate) fn fuzz_parser(iters: u64, seed: u64) -> Result<(), String> {
    let seed_inputs = corpus_seed_inputs()?;
    with_silenced_panic_hook(|| run_parser_fuzz(iters, seed, &seed_inputs))
}

pub(crate) fn fuzz_parser_smoke() -> Result<(), String> {
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
pub(crate) const FUZZ_SEMA_DEEP_ITERS: u64 = 2_000_000;
pub(crate) const FUZZ_SEMA_DEEP_SEED: u64 = 1;
pub(crate) const FUZZ_SEMA_SMOKE_SEEDS: &[u64] = &[1, 2];
pub(crate) const FUZZ_SEMA_SMOKE_ITERS_PER_SEED: u64 = 1_000;

/// The fixed diagnostic-category set plans/M2.md decision 1 names, plus
/// `comptime` (plans/M3.md item B: the evaluator's own abandonment/quota
/// build errors, surfaced through `sema::check` since it now runs const
/// initializers through the real evaluator). Any `SemaError` whose
/// category is not in this list is itself an invariant violation, not a
/// legitimate rejection.
pub(crate) const SEMA_CATEGORIES: &[&str] = &[
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
    // plans/M11.md item A / decision 721: sync-loop `@budget(bound=N)`
    // discharge — `error[sema]` when a synchronous for/while lacks the
    // attribute (02-language.md §8.1).
    "sema",
];

/// One full run of the pipeline the sema fuzzer exercises: lex, then (on
/// success) parse a whole module, then (on success) `sema::check`.
/// Exactly one of these four shapes comes back — never a panic, per
/// `check_sema_invariants`'s `catch_unwind`.
pub(crate) enum SemaPipelineOutcome {
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

pub(crate) fn run_sema_pipeline_once(input: &str) -> SemaPipelineOutcome {
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
            Ok(module) => match sema::check_dump(&module, "<fuzz>") {
                Ok(dump) => SemaPipelineOutcome::Ok(dump),
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

/// Measured reach for the sema lane (plans/M9.md item PP): how many
/// inputs reached `sema::check_dump`, and where the rest died.
#[derive(Debug, Clone, Default)]
pub(crate) struct SemaReach {
    /// `sema::check_dump` ran (lex+parse succeeded).
    checked: bool,
    /// That check accepted the input.
    check_ok: bool,
    died_lex: bool,
    died_parse: bool,
}

#[derive(Default)]
pub(crate) struct SemaReachTotals {
    check_ok: u64,
    check_err: u64,
    died_lex: u64,
    died_parse: u64,
}

impl SemaReachTotals {
    fn add(&mut self, r: &SemaReach) {
        if r.died_lex {
            self.died_lex += 1;
        } else if r.died_parse {
            self.died_parse += 1;
        } else if r.check_ok {
            self.check_ok += 1;
        } else if r.checked {
            self.check_err += 1;
        }
    }
}

pub(crate) fn sema_reach_of(o: &SemaPipelineOutcome) -> SemaReach {
    match o {
        SemaPipelineOutcome::Ok(_) => SemaReach {
            checked: true,
            check_ok: true,
            ..SemaReach::default()
        },
        SemaPipelineOutcome::SemaErr { .. } => SemaReach {
            checked: true,
            check_ok: false,
            ..SemaReach::default()
        },
        SemaPipelineOutcome::LexErr { .. } => SemaReach {
            died_lex: true,
            ..SemaReach::default()
        },
        SemaPipelineOutcome::ParseErr { .. } => SemaReach {
            died_parse: true,
            ..SemaReach::default()
        },
    }
}

/// Every invariant the sema fuzzer checks, once per iteration, on one
/// input. Runs the whole lex-then-parse-then-check pipeline twice under
/// `catch_unwind`, mirroring `check_parse_invariants`'s shape, plus a
/// direct check that a successful `SemaError` category (when the outcome
/// is instead an error) is one of the fixed set, and that `sema::dump` is
/// itself panic-free and repeat-call-identical on a successful outcome.
/// Returns measured reach on success (plans/M9.md item PP).
pub(crate) fn check_sema_invariants(input: &str) -> Result<SemaReach, String> {
    let first = std::panic::catch_unwind(|| run_sema_pipeline_once(input))
        .map_err(|p| format!("sema panicked: {}", panic_message(&p)))?;
    let second = std::panic::catch_unwind(|| run_sema_pipeline_once(input))
        .map_err(|p| format!("sema panicked on a repeat call: {}", panic_message(&p)))?;

    if let SemaPipelineOutcome::SemaErr {
        category, message, ..
    } = &first
    {
        if !SEMA_CATEGORIES.contains(category) {
            return Err(format!(
                "sema produced an unknown diagnostic category `{category}` (not in the fixed set)"
            ));
        }
        // plans/M9.md item NN / CLAUDE.md: an `internal error:` is a bug,
        // not an outcome. Sema was the one live lane that only checked the
        // category set and would have shrugged at the comptime-assert
        // unbound-local find; close that hole here too.
        if message.starts_with("internal error: ") {
            return Err(format!("sema: check_dump reported {message}"));
        }
    }

    match (&first, &second) {
        (SemaPipelineOutcome::Ok(d1), SemaPipelineOutcome::Ok(d2)) => {
            if d1 != d2 {
                return Err("sema is not deterministic: two runs produced different dumps".into());
            }
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
        }
        _ => {
            return Err(
                "sema is not deterministic: the two runs disagreed on success/failure or which \
             stage failed"
                    .into(),
            );
        }
    }
    Ok(sema_reach_of(&first))
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
pub(crate) enum SemaOutcomeSummary {
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
pub(crate) fn sema_outcome_summary(module: &Module, path: &str) -> SemaOutcomeSummary {
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
pub(crate) fn strip_position_tails(lines: &[String], path: &str) -> Vec<String> {
    let marker = format!(" at {path}:");
    lines
        .iter()
        .map(|l| match l.find(&marker) {
            Some(idx) => l[..idx].to_string(),
            None => l.clone(),
        })
        .collect()
}

pub(crate) fn describe_sema_outcome(o: &SemaOutcomeSummary) -> String {
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
pub(crate) fn sema_outcomes_agree(
    a: &SemaOutcomeSummary,
    b: &SemaOutcomeSummary,
) -> Result<(), String> {
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
pub(crate) fn rotate_first_item_to_end(module: &Module) -> Option<Module> {
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
pub(crate) fn check_sema_roundtrip_and_rotation(input: &str) -> Result<(), String> {
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
pub(crate) fn check_sema_roundtrip_and_rotation_guarded(input: &str) -> Result<(), String> {
    match std::panic::catch_unwind(|| check_sema_roundtrip_and_rotation(input)) {
        Ok(result) => result,
        Err(p) => Err(format!(
            "sema panicked (roundtrip/rotation invariants): {}",
            panic_message(&p)
        )),
    }
}

pub(crate) fn run_sema_fuzz(iters: u64, seed: u64, seed_inputs: &[String]) -> Result<(), String> {
    let mut rng = Rng::new(seed);
    let mut totals = SemaReachTotals::default();
    for i in 0..iters {
        let input = fuzz_input_with_comptime_assert_shapes(&mut rng, seed_inputs, i);
        match check_sema_invariants(&input) {
            Ok(reach) => totals.add(&reach),
            Err(reason) => {
                return report_fuzz_failure("sema", "sema-crash-", seed, i, &input, &reason);
            }
        }
        if let Err(reason) = check_sema_roundtrip_and_rotation_guarded(&input) {
            return report_fuzz_failure("sema", "sema-crash-", seed, i, &input, &reason);
        }
    }
    println!(
        "fuzz sema: {iters} iteration(s) clean (seed={seed}); reached check Ok {}, check Err {}, \
         died at lex {}, parse {}",
        totals.check_ok, totals.check_err, totals.died_lex, totals.died_parse,
    );
    Ok(())
}

pub(crate) fn fuzz_sema(iters: u64, seed: u64) -> Result<(), String> {
    let seed_inputs = corpus_seed_inputs()?;
    with_silenced_panic_hook(|| run_sema_fuzz(iters, seed, &seed_inputs))
}

pub(crate) fn fuzz_sema_smoke() -> Result<(), String> {
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
pub(crate) const FUZZ_EVAL_DEEP_ITERS: u64 = 2_000_000;
pub(crate) const FUZZ_EVAL_DEEP_SEED: u64 = 1;
pub(crate) const FUZZ_EVAL_SMOKE_SEEDS: &[u64] = &[1, 2];
pub(crate) const FUZZ_EVAL_SMOKE_ITERS_PER_SEED: u64 = 1_000;

/// One full run of the pipeline the eval fuzzer exercises: lex, then (on
/// success) parse a whole module, then `sema::check_typed`, then (on a
/// successful typecheck) `eval::run_tests`, then — plans/M4.md item E —
/// whenever the typechecked module declares exactly one reachable
/// `@image` fn, the image pipeline too (`run_image_pipeline_once`, below).
/// Exactly one of these four shapes comes back — never a panic, per
/// `check_eval_invariants`'s `catch_unwind`.
pub(crate) enum EvalPipelineOutcome {
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
pub(crate) fn run_image_pipeline_once(
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
                    &program
                        .enums
                        .iter()
                        .map(|(k, e)| (k.clone(), e.variants.clone()))
                        .collect(),
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
pub(crate) fn render_sema_error_diag(e: &sema::SemaError) -> String {
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

pub(crate) fn run_eval_pipeline_once(input: &str) -> EvalPipelineOutcome {
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
pub(crate) fn report_is_well_formed(report: &str) -> Result<(), String> {
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
pub(crate) fn summary_line_well_formed(line: &str) -> bool {
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
pub(crate) fn test_line_well_formed(line: &str) -> bool {
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
pub(crate) fn image_outcome_is_well_formed(text: &str) -> Result<(), String> {
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

/// Invariant (e), plans/M9.md item BB (decision 62): an
/// `"internal error: "`-prefixed message is a **bug**, never an outcome
/// (CLAUDE.md's own wording for what every lane checks). The `lower` and
/// `async` lanes have applied exactly this rule to every stage they touch
/// since M7 — `async_sema_outcome`'s own doc comment even names
/// `eval/interp.rs`'s ~50 such guards as the class it exists to falsify —
/// but this lane, the one that actually *evaluates* comptime code, never
/// applied it: a `check_typed` `Err` was accepted as long as its category
/// was in the fixed set, and an `internal error:` inside a `@test`'s own
/// `FAILED` verdict was accepted as long as the line was well-formed.
/// Item BB is what made that visible (`?`'s missing `from` conversion
/// abandoned with an `internal error:` from ordinary source, and this
/// lane would have shrugged at it). Three surfaces, because a comptime
/// abandonment can surface as any of them: a whole-program diagnostic, a
/// per-`@test` verdict, or the image pipeline's own one-line diagnostic.
pub(crate) fn eval_outcome_carries_no_internal_error(
    outcome: &EvalPipelineOutcome,
) -> Result<(), String> {
    const PREFIX: &str = "internal error: ";
    match outcome {
        EvalPipelineOutcome::SemaErr { message, .. } => {
            if message.starts_with(PREFIX) {
                return Err(format!("eval: sema::check_typed reported {message}"));
            }
        }
        EvalPipelineOutcome::Ok(report, image_outcome) => {
            for line in report.lines() {
                if let Some((_, verdict)) = line.split_once(": FAILED ") {
                    if verdict.starts_with(PREFIX) {
                        return Err(format!("eval: run_tests reported {verdict}"));
                    }
                }
            }
            // `error[<cat>]: internal error: ...` — the one-line
            // diagnostic shape `image_outcome_is_well_formed` already
            // parses, split at the same `]: ` boundary it uses.
            if let Some(first_line) = image_outcome.as_ref().and_then(|t| t.lines().next()) {
                if let Some((_, rest)) = first_line.split_once("]: ") {
                    if rest.starts_with(PREFIX) {
                        return Err(format!("eval: the image pipeline reported {rest}"));
                    }
                }
            }
        }
        _ => {}
    }
    Ok(())
}

/// Measured reach for the eval lane (plans/M9.md item PP): the surface
/// this lane exists for is `check_typed` then `run_tests` (and optionally
/// the image pipeline). Inputs that die at lex/parse/sema never touch it.
#[derive(Debug, Clone, Default)]
pub(crate) struct EvalReach {
    /// `sema::check_typed` accepted — `run_tests` therefore ran.
    check_typed: bool,
    died_lex: bool,
    died_parse: bool,
    /// Parsed but `check_typed` rejected.
    died_sema: bool,
}

#[derive(Default)]
pub(crate) struct EvalReachTotals {
    check_typed: u64,
    died_lex: u64,
    died_parse: u64,
    died_sema: u64,
}

impl EvalReachTotals {
    fn add(&mut self, r: &EvalReach) {
        if r.check_typed {
            self.check_typed += 1;
        } else if r.died_lex {
            self.died_lex += 1;
        } else if r.died_parse {
            self.died_parse += 1;
        } else if r.died_sema {
            self.died_sema += 1;
        }
    }
}

pub(crate) fn eval_reach_of(o: &EvalPipelineOutcome) -> EvalReach {
    match o {
        EvalPipelineOutcome::Ok(_, _) => EvalReach {
            check_typed: true,
            ..EvalReach::default()
        },
        EvalPipelineOutcome::LexErr { .. } => EvalReach {
            died_lex: true,
            ..EvalReach::default()
        },
        EvalPipelineOutcome::ParseErr { .. } => EvalReach {
            died_parse: true,
            ..EvalReach::default()
        },
        EvalPipelineOutcome::SemaErr { .. } => EvalReach {
            died_sema: true,
            ..EvalReach::default()
        },
    }
}

/// Every invariant the eval fuzzer checks, once per iteration, on one
/// input. Runs the whole lex-then-parse-then-check_typed-then-(run_tests,
/// then — plans/M4.md item E — the image pipeline when exactly one
/// `@image` fn is declared) pipeline twice under `catch_unwind`, mirroring
/// `check_sema_invariants`'s shape, plus the well-formedness check
/// (invariant (d)) on a successful outcome (both `run_tests`'s own report
/// and, when present, the image pipeline's own outcome) and the
/// fixed-category check (also (d)) on a `SemaErr` outcome. Returns
/// measured reach on success (plans/M9.md item PP).
pub(crate) fn check_eval_invariants(input: &str) -> Result<EvalReach, String> {
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
    eval_outcome_carries_no_internal_error(&first)?;

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
        }
        _ => {
            return Err(
                "eval is not deterministic: the two runs disagreed on success/failure or which \
             stage failed"
                    .into(),
            );
        }
    }
    Ok(eval_reach_of(&first))
}

/// plans/M9.md item NN: fixed shapes that put `comptime assert` over a
/// runtime-visible name (parameter, local, loop-accumulated local,
/// for-loop variable, field of a parameter, `self`, `@test` local).
/// Mutation and token soup never spelled this class, so the eval lane's
/// `internal error:` check had nothing to catch — the fifth reachable
/// producer-bug after II's multi-module four. Numerics vary from the
/// seeded RNG; the shape set is fixed (same discipline as `fuzz imports`).
///
/// Indent is written as `\n    ` on one line — never a `\` line-
/// continuation before indented text, which would eat the spaces
/// (see `import_test_fn`'s own comment for the same trap).
pub(crate) fn generate_comptime_assert_runtime_shape(rng: &mut Rng) -> String {
    let n = (rng.gen_range(40) as i64) + 1;
    let k = (rng.gen_range(40) as i64) + 1;
    match rng.gen_range(7) {
        0 => format!(
            "module fuzz.ca_param\nfn f(n: i64) -> i64:\n    comptime assert n > 0, \"param\"\n    return n\n@test pub fn go(): assert f({n}) == {n}, \"ok\"\n"
        ),
        1 => format!(
            "module fuzz.ca_local\nfn compute() -> i64:\n    t = {n}\n    comptime assert t * 2 == {twice}, \"doubling\"\n    return t\n@test pub fn go(): assert compute() == {n}, \"ok\"\n",
            twice = n * 2,
        ),
        2 => format!(
            "module fuzz.ca_loop\nfn f() -> i64:\n    total = 0\n    for i in 0..{n}:\n        total = total + i\n    comptime assert total == {k}, \"loop\"\n    return total\n@test pub fn go(): assert f() >= 0, \"ok\"\n"
        ),
        3 => format!(
            "module fuzz.ca_for_var\nfn f() -> i64:\n    for i in 0..{n}:\n        comptime assert i >= 0, \"i\"\n    return 0\n@test pub fn go(): assert f() == 0, \"ok\"\n"
        ),
        4 => format!(
            "module fuzz.ca_field\nstruct Point:\n    x: i64\n    y: i64\nfn g(p: Point) -> i64:\n    comptime assert p.x > 0, \"x\"\n    return p.x\n@test pub fn go(): assert g(Point(x={n}, y={k})) == {n}, \"ok\"\n"
        ),
        5 => format!(
            "module fuzz.ca_self\nstruct Box:\n    n: i64\n    fn check(self) -> i64:\n        comptime assert self.n > 0, \"n\"\n        return self.n\n@test pub fn go(): assert Box(n={n}).check() == {n}, \"ok\"\n"
        ),
        _ => format!(
            "module fuzz.ca_test_local\n@test pub fn go():\n    x = {n}\n    comptime assert x == {n}, \"x\"\n"
        ),
    }
}

/// Corpus mutation / token soup / comptime-assert-over-runtime-name
/// shapes (plans/M9.md item NN). Every fourth iteration is a shape so
/// the class cannot regress silently under the existing
/// `internal error:` invariant.
pub(crate) fn fuzz_input_with_comptime_assert_shapes(
    rng: &mut Rng,
    seed_inputs: &[String],
    i: u64,
) -> String {
    if i % 4 == 3 {
        return generate_comptime_assert_runtime_shape(rng);
    }
    if i % 2 == 0 {
        String::from_utf8_lossy(&mutate_seed_input(rng, seed_inputs)).into_owned()
    } else {
        token_soup(rng)
    }
}

pub(crate) fn run_eval_fuzz(iters: u64, seed: u64, seed_inputs: &[String]) -> Result<(), String> {
    let mut rng = Rng::new(seed);
    let mut totals = EvalReachTotals::default();
    for i in 0..iters {
        let input = fuzz_input_with_comptime_assert_shapes(&mut rng, seed_inputs, i);
        match check_eval_invariants(&input) {
            Ok(reach) => totals.add(&reach),
            Err(reason) => {
                return report_fuzz_failure("eval", "eval-crash-", seed, i, &input, &reason);
            }
        }
    }
    println!(
        "fuzz eval: {iters} iteration(s) clean (seed={seed}); reached check_typed {}, \
         died at lex {}, parse {}, check {}",
        totals.check_typed, totals.died_lex, totals.died_parse, totals.died_sema,
    );
    Ok(())
}

pub(crate) fn fuzz_eval(iters: u64, seed: u64) -> Result<(), String> {
    let seed_inputs = corpus_seed_inputs()?;
    with_silenced_panic_hook(|| run_eval_fuzz(iters, seed, &seed_inputs))
}

pub(crate) fn fuzz_eval_smoke() -> Result<(), String> {
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
pub(crate) const FUZZ_LOWER_DEEP_ITERS: u64 = 2_000_000;
pub(crate) const FUZZ_LOWER_DEEP_SEED: u64 = 1;
// `#[allow(dead_code)]`: not yet read by `check()` (`fuzz_lower_smoke`'s own
// doc comment explains why) — deliberately kept, not deleted, so wiring
// the smoke call back in once the blocking sema fix lands is a one-line
// change with its own budget already named here.
#[allow(dead_code)]
pub(crate) const FUZZ_LOWER_SMOKE_SEEDS: &[u64] = &[1, 2];
#[allow(dead_code)]
pub(crate) const FUZZ_LOWER_SMOKE_ITERS_PER_SEED: u64 = 1_000;

/// What a successful `layout::layout_test_image` attempt contributes to
/// `LowerFuzzOutcome::Ok`'s own determinism compare — `ImageLayout`'s own
/// three fields, copied out field-by-field rather than storing `ImageLayout`
/// itself (which derives no `PartialEq`/`Clone` this crate could reuse
/// without adding one to `wrela-compiler` for a fuzz-only need).
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum LayoutOutcome {
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
pub(crate) enum LowerFuzzOutcome {
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
pub(crate) fn runtime_test_names(program: &sema::typed::TypedProgram) -> Vec<String> {
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
pub(crate) fn concat_code_words(program: &codegen::CodegenProgram) -> Vec<u32> {
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
pub(crate) fn attempt_layout(
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

/// Measured reach for the lower lane (plans/M9.md item PP): the surface
/// this lane exists for is `lower`/`codegen` after `check_typed`. The
/// `time_layout_rejected` counter names NN's carry-out 2 explicitly — a
/// `build_layout_ctx` failure on a time-mentioning module that already
/// passed `check_typed` — so the before/after of teaching that inject is
/// visible in the printed line rather than inferred.
#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct LowerReach {
    check_typed: bool,
    lower_ok: bool,
    lower_rejected: bool,
    /// NN carry-out 2: `build_layout_ctx` failed after check_typed on a
    /// time-mentioning module (classified as LowerRejected until PP fixes
    /// the inject). Separate from ordinary lower rejections.
    time_layout_rejected: bool,
    codegen_ok: bool,
    codegen_rejected: bool,
    layout_built: bool,
    died_lex: bool,
    died_parse: bool,
    died_sema: bool,
}

#[derive(Default)]
pub(crate) struct LowerReachTotals {
    check_typed: u64,
    lower_ok: u64,
    lower_rejected: u64,
    time_layout_rejected: u64,
    codegen_ok: u64,
    codegen_rejected: u64,
    layout_built: u64,
    died_lex: u64,
    died_parse: u64,
    died_sema: u64,
}

impl LowerReachTotals {
    fn add(&mut self, r: &LowerReach) {
        self.check_typed += u64::from(r.check_typed);
        self.lower_ok += u64::from(r.lower_ok);
        self.lower_rejected += u64::from(r.lower_rejected);
        self.time_layout_rejected += u64::from(r.time_layout_rejected);
        self.codegen_ok += u64::from(r.codegen_ok);
        self.codegen_rejected += u64::from(r.codegen_rejected);
        self.layout_built += u64::from(r.layout_built);
        self.died_lex += u64::from(r.died_lex);
        self.died_parse += u64::from(r.died_parse);
        self.died_sema += u64::from(r.died_sema);
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
pub(crate) fn run_lower_pipeline_once(input: &str) -> (LowerFuzzOutcome, LowerReach) {
    let mut reach = LowerReach::default();
    let module = match lexer::lex(input) {
        Err(e) => {
            reach.died_lex = true;
            return (
                LowerFuzzOutcome::LexErr {
                    message: e.message,
                    line: e.line,
                    col: e.col,
                },
                reach,
            );
        }
        Ok(tokens) => match parser::parse(tokens) {
            Err(e) => {
                reach.died_parse = true;
                return (
                    LowerFuzzOutcome::ParseErr {
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
    let program = match sema::check_typed(&module, "<fuzz-lower>") {
        Err(e) => {
            reach.died_sema = true;
            return (
                LowerFuzzOutcome::SemaErr {
                    category: e.category,
                    message: e.message,
                    line: e.line,
                    col: e.col,
                    extra_lines: e.extra_lines,
                    omit_location: e.omit_location,
                },
                reach,
            );
        }
        Ok(p) => p,
    };
    reach.check_typed = true;
    let mwir_program = match lower::lower_program(&program) {
        Err(e) => {
            return if e.message.starts_with("internal error: ") {
                (
                    LowerFuzzOutcome::Bug(format!("lower::lower_program: {}", e.message)),
                    reach,
                )
            } else {
                reach.lower_rejected = true;
                (
                    LowerFuzzOutcome::LowerRejected { message: e.message },
                    reach,
                )
            };
        }
        Ok(p) => p,
    };
    reach.lower_ok = true;
    let mwir_dump = mwir::dump(&mwir_program);
    let layout_ctx = match mwir::build_layout_ctx(&module, &Default::default()) {
        Err(e) => {
            // After item PP, `build_layout_ctx` injects the same
            // Duration/Instant arity `check_typed` does, so a failure
            // here on a `check_typed`-accepted program is a genuine bug
            // again — including on time-mentioning modules.
            return (
                LowerFuzzOutcome::Bug(format!(
                    "mwir::build_layout_ctx failed after check_typed already accepted this program: \
                 {e:?}"
                )),
                reach,
            );
        }
        Ok(c) => c,
    };
    let codegen_program = match codegen::codegen_program(&mwir_program, &layout_ctx) {
        Err(e) => {
            return if e.message.starts_with("internal error: ") {
                (
                    LowerFuzzOutcome::Bug(format!("codegen::codegen_program: {}", e.message)),
                    reach,
                )
            } else {
                reach.codegen_rejected = true;
                (
                    LowerFuzzOutcome::CodegenRejected { message: e.message },
                    reach,
                )
            };
        }
        Ok(p) => p,
    };
    if let Err(reason) = codegen::validate(&codegen_program) {
        return (
            LowerFuzzOutcome::Bug(format!("codegen::validate: {reason}")),
            reach,
        );
    }
    reach.codegen_ok = true;
    let code_words = concat_code_words(&codegen_program);
    let layout = match attempt_layout(&program, &codegen_program) {
        Ok(l) => l,
        Err(bug) => return (LowerFuzzOutcome::Bug(bug), reach),
    };
    if matches!(layout, LayoutOutcome::Built { .. }) {
        reach.layout_built = true;
    }
    (
        LowerFuzzOutcome::Ok {
            mwir_dump,
            code_words,
            layout,
        },
        reach,
    )
}

/// Every invariant the lower fuzzer checks, once per iteration, on one
/// input. Runs the whole pipeline twice under `catch_unwind`, mirroring
/// `check_eval_invariants`'s shape exactly: invariant (c)'s category check
/// on a `SemaErr`/`LowerRejected`/`CodegenRejected` outcome, invariant (a)'s
/// "never a `Bug`" check, then invariant (b)'s determinism compare,
/// matched per-shape (rather than one blanket `!=`) so a divergence names
/// exactly which stage disagreed, mirroring every other lane's own
/// diagnostic style in this file. Returns measured reach on success
/// (plans/M9.md item PP).
pub(crate) fn check_lower_invariants(input: &str) -> Result<LowerReach, String> {
    let (first, reach) = std::panic::catch_unwind(|| run_lower_pipeline_once(input))
        .map_err(|p| format!("lower/codegen panicked: {}", panic_message(&p)))?;
    let (second, reach2) =
        std::panic::catch_unwind(|| run_lower_pipeline_once(input)).map_err(|p| {
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
        }
        (LowerFuzzOutcome::Ok { .. }, LowerFuzzOutcome::Ok { .. }) => {
            if first != second {
                return Err(
                    "lower is not deterministic: two runs produced a different mwir dump, \
                     codegen'd words, or laid-out test image for the same input"
                        .into(),
                );
            }
        }
        _ => {
            return Err(
                "lower is not deterministic: the two runs disagreed on success/failure or which \
             stage failed"
                    .into(),
            );
        }
    }
    if reach != reach2 {
        return Err("lower is not deterministic: two runs reached different stages".into());
    }
    Ok(reach)
}

pub(crate) fn run_lower_fuzz(iters: u64, seed: u64, seed_inputs: &[String]) -> Result<(), String> {
    let mut rng = Rng::new(seed);
    let mut totals = LowerReachTotals::default();
    for i in 0..iters {
        // plans/M9.md item NN carry-out 3 / item PP: same
        // comptime-assert-over-runtime-name shapes as `fuzz sema`/`eval`,
        // now that `build_layout_ctx` injects the time prelude (carry-out
        // 2) so wiring them no longer shifts the schedule onto that hole.
        let input = fuzz_input_with_comptime_assert_shapes(&mut rng, seed_inputs, i);
        match check_lower_invariants(&input) {
            Ok(reach) => totals.add(&reach),
            Err(reason) => {
                return report_fuzz_failure("lower", "lower-crash-", seed, i, &input, &reason);
            }
        }
    }
    println!(
        "fuzz lower: {iters} iteration(s) clean (seed={seed}); reached check_typed {}, \
         lower Ok {}, lower rejected {} ({} time-prelude layout-ctx), codegen Ok {}, \
         codegen rejected {}, layout built {}, died at lex {}, parse {}, check {}",
        totals.check_typed,
        totals.lower_ok,
        totals.lower_rejected,
        totals.time_layout_rejected,
        totals.codegen_ok,
        totals.codegen_rejected,
        totals.layout_built,
        totals.died_lex,
        totals.died_parse,
        totals.died_sema,
    );
    Ok(())
}

pub(crate) fn fuzz_lower(iters: u64, seed: u64) -> Result<(), String> {
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
pub(crate) fn fuzz_lower_smoke() -> Result<(), String> {
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
pub(crate) const FUZZ_ASYNC_DEEP_ITERS: u64 = 400_000;
pub(crate) const FUZZ_ASYNC_DEEP_SEED: u64 = 1;
// Wired into `check` alongside every other live lane's smoke: two fixed
// seeds, 1_000 iterations each (~0.5s total at the cost measured above),
// no seed ever from the clock or the environment.
pub(crate) const FUZZ_ASYNC_SMOKE_SEEDS: &[u64] = &[1, 2];
pub(crate) const FUZZ_ASYNC_SMOKE_ITERS_PER_SEED: u64 = 1_000;

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
pub(crate) const ASYNC_SEED_CASES: &[&str] = &[
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
    "boot-await-rejected",
    "boot-cancel-cleanup",
    "boot-deadline-cancel",
    "boot-deadline-inherit",
    "boot-group-join",
    "boot-group-four-children",
    "boot-send",
    // Accept-shaped sema cases over the same surface — no runtime test, so
    // they mutate toward "async fns that lower and codegen but never lay
    // out an image", which is `asm-async-*`'s shape with more variety.
    "check-actor-methods",
    "check-actor-private-handle-helper",
    "check-actor-send",
    "check-await-self-path",
    "check-await-result-path",
    "check-await-question-mark",
    "check-deadline",
    "check-group",
    "check-send-proven",
];

/// The `ASYNC_SEED_CASES` inputs, in the listed order. Fails closed on a
/// missing case (see that constant's own doc comment).
pub(crate) fn async_seed_inputs() -> Result<Vec<String>, String> {
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
pub(crate) struct AsyncReach {
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
pub(crate) struct AsyncReachTotals {
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
pub(crate) enum AsyncFuzzOutcome {
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
pub(crate) fn async_sema_outcome(stage: &'static str, e: sema::SemaError) -> AsyncFuzzOutcome {
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
pub(crate) fn async_stage_err(
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
pub(crate) fn run_async_pipeline_once(input: &str) -> (AsyncFuzzOutcome, AsyncReach) {
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
    let enqueue_specs = match layout::mailbox_enqueue_specs(&graph, &modules, &layout_ctx) {
        Err(msg) => {
            return (
                async_stage_err("layout::mailbox_enqueue_specs", "build", msg),
                reach,
            );
        }
        Ok(s) => s,
    };
    let codegen_program = match codegen::codegen_program_with_async(
        &mwir_program,
        &flow_program,
        &layout_ctx,
        &method_index,
        group_arena_capacity,
        &enqueue_specs,
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

    // `test_cmd`'s runtime tier only ever lays out an image when the file
    // declares at least one `@test(runtime)` fn — mirrored exactly, so a
    // `Skipped` here means "production would not have laid one out either",
    // not "this lane looked away" (which is precisely what `attempt_layout`
    // in the `lower` lane had to say about every async test).
    // Image layout goes through `lower_and_codegen_image` (force-roots) —
    // the stub-checked codegen above stays for FlowWir/reach measurement.
    let layout_outcome = if runtime_tests.is_empty() {
        LayoutOutcome::Skipped
    } else {
        let async_tests: std::collections::BTreeSet<String> = runtime_tests
            .iter()
            .filter(|name| program.fns.get(*name).is_some_and(|f| f.is_async))
            .cloned()
            .collect();
        let is_async_image = !async_tests.is_empty();
        let mut programs: BTreeMap<String, sema::typed::TypedProgram> = BTreeMap::new();
        programs.insert(module.path.join("."), program.clone());
        // Same one-check → one-lower path as `wrela test`: force-root the
        // live runtime before layout so enqueue/secondary trampolines exist.
        let compiled = match layout::lower_and_codegen_image(
            &modules,
            &programs,
            &layout_ctx,
            &graph,
            &runtime_tests,
            &async_tests,
        ) {
            Ok(c) => c,
            Err(e) => {
                if e.starts_with("internal error: ") {
                    return (
                        AsyncFuzzOutcome::Bug(format!("layout::lower_and_codegen_image: {e}")),
                        reach,
                    );
                }
                return (
                    async_stage_err("layout::lower_and_codegen_image", "unimplemented", e),
                    reach,
                );
            }
        };
        let boot = layout::BootCtx {
            graph: &graph,
            modules: &compiled.modules,
            programs: &compiled.programs,
            layout_ctx: &compiled.layout_ctx,
            async_frames: &compiled.async_frames,
            group_child_index: &compiled.group_child_index,
            flow: &compiled.flow,
        };
        match layout::layout_test_image(
            &compiled.program,
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
pub(crate) fn check_async_invariants(input: &str) -> Result<AsyncReach, String> {
    let (first, reach) = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        run_async_pipeline_once(input)
    }))
    .map_err(|p| format!("the async pipeline panicked: {}", panic_message(p.as_ref())))?;
    let (second, reach2) = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        run_async_pipeline_once(input)
    }))
    .map_err(|p| {
        format!(
            "the async pipeline panicked on a repeat call: {}",
            panic_message(p.as_ref())
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
pub(crate) fn async_fuzz_input(
    rng: &mut Rng,
    async_seeds: &[String],
    corpus_seeds: &[String],
) -> String {
    match rng.gen_range(8) {
        0 => token_soup(rng),
        1 => String::from_utf8_lossy(&mutate_seed_input_from(rng, async_seeds, corpus_seeds))
            .into_owned(),
        _ => String::from_utf8_lossy(&mutate_seed_input(rng, async_seeds)).into_owned(),
    }
}

pub(crate) fn run_async_fuzz(
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

pub(crate) fn fuzz_async(iters: u64, seed: u64) -> Result<(), String> {
    let async_seeds = async_seed_inputs()?;
    let corpus_seeds = corpus_seed_inputs()?;
    with_silenced_panic_hook(|| run_async_fuzz(iters, seed, &async_seeds, &corpus_seeds))
}

pub(crate) fn fuzz_async_smoke() -> Result<(), String> {
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

// --- fuzz: imports (plans/M9.md item II) ---------------------------------
//
// Every other fuzz lane is single-file. Four reachable `internal error:`
// finds this milestone (A1b, EE, HH, HH#1) all needed a multi-module
// closure, so every lane's per-iteration `internal error:` check has
// never once seen an import. This lane is the deferred multi-module
// generator: a small fixed set of module shapes (exporter + importer,
// aliasing importer, two-deep chain, aliased peer + reachable generic)
// filled from the seeded RNG, run through `check_program_typed` +
// `eval::run_tests` + `lower::lower_program`. An `"internal error: "`
// anywhere is a bug.
//
// Not a general program generator. The shapes are the ones that would
// have caught the four finds; numeric field values vary so "accepted"
// and "correct" stay distinct under mutation of the constants.

pub(crate) const FUZZ_IMPORTS_DEEP_ITERS: u64 = 200_000;
pub(crate) const FUZZ_IMPORTS_DEEP_SEED: u64 = 1;
pub(crate) const FUZZ_IMPORTS_SMOKE_SEEDS: &[u64] = &[1, 2];
pub(crate) const FUZZ_IMPORTS_SMOKE_ITERS_PER_SEED: u64 = 1_000;

/// One closed multi-module program: module address -> source text, plus
/// the root module's address (the importer that runs `@test`s / lowers).
pub(crate) struct ImportClosure {
    modules: Vec<(Vec<String>, String)>,
    root: Vec<String>,
}

/// `@test` body with a four-space indent. Built without `\` line
/// continuation so Rust's "eat leading whitespace on the next line"
/// rule cannot strip the indent.
pub(crate) fn import_test_fn(expect: u32, msg: &str) -> String {
    format!("@test\npub fn t():\n    assert D == {expect}, \"{msg}\"\n")
}

pub(crate) fn import_shape_comptime_construct(n: u32, k: u32) -> ImportClosure {
    // A1b: comptime construction of a struct declared in another module.
    let expect = n.wrapping_add(k);
    let app = format!(
        "module app.main\n\nfrom lib.g import Cell\n\nconst D: u32 = Cell(n={n}).n + {k}\n\n{}",
        import_test_fn(expect, "imported comptime construct")
    );
    ImportClosure {
        modules: vec![
            (
                vec!["lib".into(), "g".into()],
                "module lib.g\n\npub struct Cell:\n    n: u32\n".into(),
            ),
            (vec!["app".into(), "main".into()], app),
        ],
        root: vec!["app".into(), "main".into()],
    }
}

pub(crate) fn import_shape_fields_and_method(a: u32, b: u32) -> ImportClosure {
    // EE: imported struct fields + method, exercised at comptime and lower.
    let expect = a.wrapping_add(b);
    let app = format!(
        "module app.main\n\nfrom lib.g import Pair\n\nfn drive() -> u32:\n    return Pair(a={a}, b={b}).sum()\n\nconst D: u32 = drive()\n\n{}",
        import_test_fn(expect, "imported fields and method")
    );
    ImportClosure {
        modules: vec![
            (
                vec!["lib".into(), "g".into()],
                "module lib.g\n\npub struct Pair:\n    a: u32\n    b: u32\n\n    pub fn sum(read self) -> u32:\n        return self.a + self.b\n"
                    .into(),
            ),
            (vec!["app".into(), "main".into()], app),
        ],
        root: vec!["app".into(), "main".into()],
    }
}

pub(crate) fn import_shape_reachable_unimported(seed: u32, add: u32) -> ImportClosure {
    // HH: import only Maker; field-access a reachable-but-unimported Box.
    let expect = seed.wrapping_add(1).wrapping_add(add);
    let app = format!(
        "module app.main\n\nfrom lib.g import Maker\n\nfn drive() -> u32:\n    m = Maker(seed={seed})\n    b = m.build()\n    return b.n + {add}\n\nconst D: u32 = drive()\n\n{}",
        import_test_fn(expect, "reachable unimported Box")
    );
    ImportClosure {
        modules: vec![
            (
                vec!["lib".into(), "g".into()],
                "module lib.g\n\npub struct Box:\n    n: u32\n\npub struct Maker:\n    seed: u32\n\n    pub fn build(read self) -> Box:\n        return Box(n=self.seed + 1)\n"
                    .into(),
            ),
            (vec!["app".into(), "main".into()], app),
        ],
        root: vec!["app".into(), "main".into()],
    }
}

pub(crate) fn import_shape_alias_peer_generic(n: u32, add: u32) -> ImportClosure {
    // HH#1: alias a peer type; import wrap/peel of a generic; do not
    // import the generic itself. Instantiation keys must re-key.
    let expect = n.wrapping_add(add);
    let app = format!(
        "module app.main\n\nfrom lib.g import Src as Item\nfrom lib.g import wrap_box\nfrom lib.g import peel_box\n\nfn drive() -> u32:\n    s = Item(n={n})\n    b = wrap_box(take s)\n    i: Item = peel_box(take b)\n    return i.n + {add}\n\nconst D: u32 = drive()\n\n{}",
        import_test_fn(expect, "aliased peer + reachable generic")
    );
    ImportClosure {
        modules: vec![
            (
                vec!["lib".into(), "g".into()],
                "module lib.g\n\npub struct Src:\n    n: u32\n\npub struct Box[T]:\n    v: T\n\npub fn peel_box(take b: Box[Src]) -> Src:\n    return b.v\n\npub fn wrap_box(take s: Src) -> Box[Src]:\n    return Box[Src](v=s)\n"
                    .into(),
            ),
            (vec!["app".into(), "main".into()], app),
        ],
        root: vec!["app".into(), "main".into()],
    }
}

pub(crate) fn import_shape_chain(seed: u32, add: u32) -> ImportClosure {
    // HH chain: A→B→C, only A imported.
    let expect = seed.wrapping_add(1).wrapping_add(add);
    let app = format!(
        "module app.main\n\nfrom lib.a import A\n\nfn drive() -> u32:\n    a = A(seed={seed})\n    b = a.make()\n    c = b.get()\n    return c.n + {add}\n\nconst D: u32 = drive()\n\n{}",
        import_test_fn(expect, "two-deep reachable chain")
    );
    ImportClosure {
        modules: vec![
            (
                vec!["lib".into(), "c".into()],
                "module lib.c\n\npub struct C:\n    n: u32\n".into(),
            ),
            (
                vec!["lib".into(), "b".into()],
                "module lib.b\n\nfrom lib.c import C\n\npub struct B:\n    inner: C\n\n    pub fn get(read self) -> C:\n        return self.inner\n"
                    .into(),
            ),
            (
                vec!["lib".into(), "a".into()],
                "module lib.a\n\nfrom lib.b import B\nfrom lib.c import C\n\npub struct A:\n    seed: u32\n\n    pub fn make(read self) -> B:\n        return B(inner=C(n=self.seed + 1))\n"
                    .into(),
            ),
            (vec!["app".into(), "main".into()], app),
        ],
        root: vec!["app".into(), "main".into()],
    }
}

pub(crate) fn import_shape_alias_owner(seed: u32, add: u32) -> ImportClosure {
    // HH alias-owner: `Maker as Builder`; reachable Box keeps exporter spelling.
    let expect = seed.wrapping_add(1).wrapping_add(add);
    let app = format!(
        "module app.main\n\nfrom lib.g import Maker as Builder\n\nfn drive() -> u32:\n    m = Builder(seed={seed})\n    b = m.build()\n    return b.n + {add}\n\nconst D: u32 = drive()\n\n{}",
        import_test_fn(expect, "reachable under aliased owner")
    );
    ImportClosure {
        modules: vec![
            (
                vec!["lib".into(), "g".into()],
                "module lib.g\n\npub struct Box:\n    n: u32\n\npub struct Maker:\n    seed: u32\n\n    pub fn build(read self) -> Box:\n        return Box(n=self.seed + 1)\n"
                    .into(),
            ),
            (vec!["app".into(), "main".into()], app),
        ],
        root: vec!["app".into(), "main".into()],
    }
}

pub(crate) fn import_shape_enum_payload(n: u32, add: u32) -> ImportClosure {
    // JJ: aliased enum; payload struct never imported; match binds it.
    let expect = n.wrapping_add(add);
    let app = format!(
        "module app.main\n\nfrom lib.g import Res as R\nfrom lib.g import make\n\nfn drive() -> u32:\n    match make(n={n}):\n        case .Good(p):\n            return p.n + {add}\n        case .Bad:\n            return 0\n\nconst D: u32 = drive()\n\n{}",
        import_test_fn(expect, "aliased enum unimported payload")
    );
    ImportClosure {
        modules: vec![
            (
                vec!["lib".into(), "g".into()],
                "module lib.g\n\npub struct Payload:\n    n: u32\n\npub enum Res:\n    Good(Payload)\n    Bad\n\npub fn make(n: u32) -> Res:\n    return Res.Good(Payload(n=n))\n"
                    .into(),
            ),
            (vec!["app".into(), "main".into()], app),
        ],
        root: vec!["app".into(), "main".into()],
    }
}

pub(crate) fn import_shape_enum_payload_generic(n: u32, add: u32) -> ImportClosure {
    // JJ neighbour: variant payload is itself generic (`Good(Box[Payload])`).
    let expect = n.wrapping_add(add);
    let app = format!(
        "module app.main\n\nfrom lib.g import Res as R\nfrom lib.g import make\n\nfn drive() -> u32:\n    match make(n={n}):\n        case .Good(b):\n            return b.v.n + {add}\n        case .Bad:\n            return 0\n\nconst D: u32 = drive()\n\n{}",
        import_test_fn(expect, "enum payload generic Box")
    );
    ImportClosure {
        modules: vec![
            (
                vec!["lib".into(), "g".into()],
                "module lib.g\n\npub struct Payload:\n    n: u32\n\npub struct Box[T]:\n    v: T\n\npub enum Res:\n    Good(Box[Payload])\n    Bad\n\npub fn make(n: u32) -> Res:\n    return Res.Good(Box[Payload](v=Payload(n=n)))\n"
                    .into(),
            ),
            (vec!["app".into(), "main".into()], app),
        ],
        root: vec!["app".into(), "main".into()],
    }
}

pub(crate) fn generate_import_closure(rng: &mut Rng) -> ImportClosure {
    // Keep values in a small range so wrapping addition stays obvious and
    // assert messages stay short.
    let n = (rng.gen_range(50) as u32) + 1;
    let k = (rng.gen_range(50) as u32) + 1;
    match rng.gen_range(8) {
        0 => import_shape_comptime_construct(n, k),
        1 => import_shape_fields_and_method(n, k),
        2 => import_shape_reachable_unimported(n, k),
        3 => import_shape_alias_peer_generic(n, k),
        4 => import_shape_chain(n, k),
        5 => import_shape_alias_owner(n, k),
        6 => import_shape_enum_payload(n, k),
        _ => import_shape_enum_payload_generic(n, k),
    }
}

/// Measured reach for the imports lane (plans/M9.md item PP). Shapes are
/// hand-built multi-module programs, so most iterations should reach
/// `check_program_typed`; the printed line makes a silent generator
/// collapse visible the same way the async lane's does.
#[derive(Debug, Clone, Default)]
pub(crate) struct ImportsReach {
    check_accepted: bool,
    check_rejected: bool,
    run_tests: bool,
    lower_ok: bool,
    lower_rejected: bool,
}

#[derive(Default)]
pub(crate) struct ImportsReachTotals {
    check_accepted: u64,
    check_rejected: u64,
    run_tests: u64,
    lower_ok: u64,
    lower_rejected: u64,
}

impl ImportsReachTotals {
    fn add(&mut self, r: &ImportsReach) {
        self.check_accepted += u64::from(r.check_accepted);
        self.check_rejected += u64::from(r.check_rejected);
        self.run_tests += u64::from(r.run_tests);
        self.lower_ok += u64::from(r.lower_ok);
        self.lower_rejected += u64::from(r.lower_rejected);
    }
}

pub(crate) fn parse_module_source(src: &str) -> Result<Module, String> {
    let tokens = lexer::lex(src).map_err(|e| format!("lex: {}", e.message))?;
    match parser::parse_any(tokens).map_err(|e| format!("parse: {}", e.message))? {
        Parsed::Module(m) => Ok(m),
        Parsed::Fragment(_) => Err("parse: expected a whole module, got a fragment".into()),
    }
}

pub(crate) fn message_has_internal_error(msg: &str) -> bool {
    msg.contains("internal error: ")
}

/// One iteration of the imports lane: build a closed multi-module
/// program, typecheck the whole closure, run comptime tests on the root,
/// and lower the root. Any `"internal error: "` is a finding. Returns
/// measured reach on success (plans/M9.md item PP).
pub(crate) fn check_imports_invariants(closure: &ImportClosure) -> Result<ImportsReach, String> {
    let mut reach = ImportsReach::default();
    let mut modules: BTreeMap<Vec<String>, Module> = BTreeMap::new();
    let mut paths: BTreeMap<Vec<String>, String> = BTreeMap::new();
    for (addr, src) in &closure.modules {
        let module = parse_module_source(src)?;
        let path = format!("{}.wr", addr.join("/"));
        paths.insert(addr.clone(), path);
        modules.insert(addr.clone(), module);
    }

    let programs = match sema::check_program_typed(&modules, &paths) {
        Ok(p) => p,
        Err(e) => {
            if message_has_internal_error(&e.message) {
                return Err(format!(
                    "imports: check_program_typed reported internal error: {}",
                    e.message
                ));
            }
            // Named rejection is fine — shapes are intentionally narrow
            // and a future language change may refuse one of them by name.
            reach.check_rejected = true;
            return Ok(reach);
        }
    };
    reach.check_accepted = true;

    let root = programs
        .get(&closure.root)
        .ok_or_else(|| "imports: root module missing from checked programs".to_string())?;

    let (report, _all_ok) = eval::run_tests(root);
    reach.run_tests = true;
    for line in report.lines() {
        if message_has_internal_error(line) {
            return Err(format!("imports: run_tests reported {line}"));
        }
        if let Some((_, verdict)) = line.split_once(": FAILED ") {
            if message_has_internal_error(verdict) {
                return Err(format!("imports: run_tests FAILED with {verdict}"));
            }
        }
    }

    match lower::lower_program(root) {
        Ok(_) => {
            reach.lower_ok = true;
        }
        Err(e) => {
            if message_has_internal_error(&e.message) {
                return Err(format!(
                    "imports: lower_program reported internal error: {}",
                    e.message
                ));
            }
            reach.lower_rejected = true;
        }
    }

    Ok(reach)
}

pub(crate) fn format_import_closure(closure: &ImportClosure) -> String {
    closure
        .modules
        .iter()
        .map(|(addr, src)| format!("// {}.wr\n{src}", addr.join("/")))
        .collect::<Vec<_>>()
        .join("\n")
}

pub(crate) fn run_imports_fuzz(iters: u64, seed: u64) -> Result<(), String> {
    let mut rng = Rng::new(seed);
    let mut totals = ImportsReachTotals::default();
    for i in 0..iters {
        let closure = generate_import_closure(&mut rng);
        let first = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            check_imports_invariants(&closure)
        }))
        .unwrap_or_else(|_| Err("imports: panic in check_program_typed/run_tests/lower".into()));
        let second = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            check_imports_invariants(&closure)
        }))
        .unwrap_or_else(|_| Err("imports: panic in check_program_typed/run_tests/lower".into()));
        match (&first, &second) {
            (Ok(r1), Ok(r2)) => {
                if r1.check_accepted != r2.check_accepted
                    || r1.check_rejected != r2.check_rejected
                    || r1.run_tests != r2.run_tests
                    || r1.lower_ok != r2.lower_ok
                    || r1.lower_rejected != r2.lower_rejected
                {
                    return Err(format!(
                        "imports fuzz nondeterminism at iteration {i} (seed={seed}): reach disagreed"
                    ));
                }
                totals.add(r1);
            }
            (Err(a), Err(b)) if a == b => {
                return Err(format!(
                    "imports fuzz failure at iteration {i} (seed={seed}): {a}\n--- modules ---\n{}",
                    format_import_closure(&closure)
                ));
            }
            (Ok(_), Err(b)) | (Err(b), Ok(_)) => {
                return Err(format!(
                    "imports fuzz nondeterminism at iteration {i} (seed={seed}): one run Ok, \
                     other Err ({b})"
                ));
            }
            (Err(a), Err(b)) => {
                return Err(format!(
                    "imports fuzz nondeterminism at iteration {i} (seed={seed}):\n  {a}\n  {b}"
                ));
            }
        }
    }
    println!(
        "fuzz imports: {iters} iteration(s) clean (seed={seed}); reached check_program_typed \
         accepted {}, rejected {}, run_tests {}, lower Ok {}, lower rejected {}",
        totals.check_accepted,
        totals.check_rejected,
        totals.run_tests,
        totals.lower_ok,
        totals.lower_rejected,
    );
    Ok(())
}

pub(crate) fn fuzz_imports(iters: u64, seed: u64) -> Result<(), String> {
    with_silenced_panic_hook(|| run_imports_fuzz(iters, seed))
}

pub(crate) fn fuzz_imports_smoke() -> Result<(), String> {
    with_silenced_panic_hook(|| {
        for &seed in FUZZ_IMPORTS_SMOKE_SEEDS {
            run_imports_fuzz(FUZZ_IMPORTS_SMOKE_ITERS_PER_SEED, seed)?;
        }
        Ok(())
    })
}
