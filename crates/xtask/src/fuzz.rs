use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use wrela_compiler::codegen;
use wrela_compiler::eval;
use wrela_compiler::flowwir;
use wrela_compiler::flowwir_lower;
use wrela_compiler::layout;
use wrela_compiler::lower;
use wrela_compiler::mwir;
use wrela_compiler::report;
use wrela_compiler::sema;
use wrela_compiler::sema::typed::TestKind;
use wrela_compiler::syntax::ast::Module;
use wrela_compiler::syntax::lexer::{self, Token, TokenKind};
use wrela_compiler::syntax::parser::{self, Parsed};
use wrela_compiler::syntax::printer;

use crate::corpus::extract_doc_blocks;
use crate::{golden_case_dirs, root};

pub(crate) const FUZZ_LEXER_DEEP_ITERS: u64 = 200_000;
pub(crate) const FUZZ_LEXER_DEEP_SEED: u64 = 1;
pub(crate) const FUZZ_LEXER_SMOKE_SEEDS: &[u64] = &[1, 2];
pub(crate) const FUZZ_LEXER_SMOKE_ITERS_PER_SEED: u64 = 1_000;

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

    fn gen_range(&mut self, n: usize) -> usize {
        (self.next_u64() % n as u64) as usize
    }
}

pub(crate) fn fuzz(args: &[String]) -> Result<(), String> {
    const TARGETS: &[&str] = &[
        "smoke", "all", "lexer", "parser", "sema", "eval", "lower", "async", "imports", "report",
    ];
    let _mode = crate::CompileOptsGuard::mode(wrela_compiler::opts::CompileMode::Dev);
    let (target, mut i) = match args.first() {
        Some(a) if !a.starts_with('-') => {
            if !TARGETS.contains(&a.as_str()) {
                return Err(format!(
                    "fuzz: unknown target `{a}` (expected {})",
                    TARGETS.join(", ")
                ));
            }
            (a.as_str(), 1usize)
        }
        Some(a) => {
            return Err(format!(
                "fuzz: expected a target before `{a}` (expected {})",
                TARGETS.join(", ")
            ));
        }
        None => {
            return Err(format!(
                "fuzz: missing target (expected {})",
                TARGETS.join(", ")
            ));
        }
    };
    let mut iters = None;
    let mut seed = None;
    while i < args.len() {
        let arg = &args[i];
        let (slot, name, inline) = if let Some(v) = arg.strip_prefix("--iters=") {
            (&mut iters, "--iters", Some(v))
        } else if let Some(v) = arg.strip_prefix("--seed=") {
            (&mut seed, "--seed", Some(v))
        } else if arg == "--iters" {
            (&mut iters, "--iters", None)
        } else if arg == "--seed" {
            (&mut seed, "--seed", None)
        } else {
            return Err(format!("fuzz: unknown argument `{arg}`"));
        };
        if slot.is_some() {
            return Err(format!("fuzz: `{name}` specified more than once"));
        }
        let value = match inline {
            Some(v) if !v.is_empty() => v,
            Some(_) => return Err(format!("fuzz: `{name}` needs a value")),
            None => {
                i += 1;
                args.get(i)
                    .map(String::as_str)
                    .ok_or_else(|| format!("fuzz: `{name}` needs a value"))?
            }
        };
        *slot = Some(
            value
                .parse::<u64>()
                .map_err(|e| format!("fuzz: {name}: {e}"))?,
        );
        i += 1;
    }
    match target {
        "smoke" => {
            if iters.is_some() || seed.is_some() {
                return Err("fuzz smoke: --iters/--seed are fixed by the suite".to_string());
            }
            fuzz_smoke_all()
        }
        "all" => {
            if iters.is_some() || seed.is_some() {
                return Err("fuzz all: --iters/--seed are fixed per target".to_string());
            }
            fuzz_deep_all()
        }
        "lexer" => fuzz_lexer(
            iters.unwrap_or(FUZZ_LEXER_DEEP_ITERS),
            seed.unwrap_or(FUZZ_LEXER_DEEP_SEED),
        ),
        "parser" => fuzz_parser(
            iters.unwrap_or(FUZZ_PARSER_DEEP_ITERS),
            seed.unwrap_or(FUZZ_PARSER_DEEP_SEED),
        ),
        "sema" => fuzz_sema(
            iters.unwrap_or(FUZZ_SEMA_DEEP_ITERS),
            seed.unwrap_or(FUZZ_SEMA_DEEP_SEED),
        ),
        "eval" => fuzz_eval(
            iters.unwrap_or(FUZZ_EVAL_DEEP_ITERS),
            seed.unwrap_or(FUZZ_EVAL_DEEP_SEED),
        ),
        "lower" => fuzz_lower(
            iters.unwrap_or(FUZZ_LOWER_DEEP_ITERS),
            seed.unwrap_or(FUZZ_LOWER_DEEP_SEED),
        ),
        "async" => fuzz_async(
            iters.unwrap_or(FUZZ_ASYNC_DEEP_ITERS),
            seed.unwrap_or(FUZZ_ASYNC_DEEP_SEED),
        ),
        "imports" => fuzz_imports(
            iters.unwrap_or(FUZZ_IMPORTS_DEEP_ITERS),
            seed.unwrap_or(FUZZ_IMPORTS_DEEP_SEED),
        ),
        "report" => fuzz_report(
            iters.unwrap_or(FUZZ_REPORT_DEEP_ITERS),
            seed.unwrap_or(FUZZ_REPORT_DEEP_SEED),
        ),
        _ => unreachable!("target validated above"),
    }
}

pub(crate) const PROJECT_SEED_CASES: &[&str] = &[
    "appliance",
    "image-project",
    "multi-module-accept",
    "import-cycle-accept",
];

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

pub(crate) fn mutate_seed_input(rng: &mut Rng, seed_inputs: &[String]) -> Vec<u8> {
    mutate_seed_input_from(rng, seed_inputs, seed_inputs)
}

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

pub(crate) const FUZZ_PARSER_DEEP_ITERS: u64 = 100_000;
pub(crate) const FUZZ_PARSER_DEEP_SEED: u64 = 1;
pub(crate) const FUZZ_PARSER_SMOKE_SEEDS: &[u64] = &[1, 2];
pub(crate) const FUZZ_PARSER_SMOKE_ITERS_PER_SEED: u64 = 1_000;

pub(crate) enum PipelineOutcome {
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

#[derive(Debug, Clone, Default)]
pub(crate) struct ParserReach {
    parsed: bool,
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

pub(crate) const FUZZ_SEMA_DEEP_ITERS: u64 = 2_000_000;
pub(crate) const FUZZ_SEMA_DEEP_SEED: u64 = 1;
pub(crate) const FUZZ_SEMA_SMOKE_SEEDS: &[u64] = &[1, 2];
pub(crate) const FUZZ_SEMA_SMOKE_ITERS_PER_SEED: u64 = 1_000;

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
    "build",
    "actor",
    "sema",
    "intrinsic",
];

pub(crate) enum SemaPipelineOutcome {
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

#[derive(Debug, Clone, Default)]
pub(crate) struct SemaReach {
    checked: bool,
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

pub(crate) enum SemaOutcomeSummary {
    Ok(String),
    Err {
        category: &'static str,
        message: String,
        extra_lines: Vec<String>,
        omit_location: bool,
    },
}

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

pub(crate) fn rotate_first_item_to_end(module: &Module) -> Option<Module> {
    if module.items.len() < 2 {
        return None;
    }
    let mut rotated = module.clone();
    rotated.items.rotate_left(1);
    Some(rotated)
}

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

pub(crate) const FUZZ_EVAL_DEEP_ITERS: u64 = 2_000_000;
pub(crate) const FUZZ_EVAL_DEEP_SEED: u64 = 1;
pub(crate) const FUZZ_EVAL_SMOKE_SEEDS: &[u64] = &[1, 2];
pub(crate) const FUZZ_EVAL_SMOKE_ITERS_PER_SEED: u64 = 1_000;

pub(crate) enum EvalPipelineOutcome {
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

pub(crate) fn summary_line_well_formed(line: &str) -> bool {
    let Some((n, rest)) = line.split_once(" passed, ") else {
        return false;
    };
    let Some(m) = rest.strip_suffix(" failed") else {
        return false;
    };
    n.parse::<u64>().is_ok() && m.parse::<u64>().is_ok()
}

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

#[derive(Debug, Clone, Default)]
pub(crate) struct EvalReach {
    check_typed: bool,
    died_lex: bool,
    died_parse: bool,
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

pub(crate) const FUZZ_LOWER_DEEP_ITERS: u64 = 2_000_000;
pub(crate) const FUZZ_LOWER_DEEP_SEED: u64 = 1;
pub(crate) const FUZZ_LOWER_SMOKE_SEEDS: &[u64] = &[1, 2];
pub(crate) const FUZZ_LOWER_SMOKE_ITERS_PER_SEED: u64 = 1_000;

#[derive(Debug, Clone, PartialEq)]
pub(crate) enum LayoutOutcome {
    Skipped,
    Rejected(String),
    Built {
        blob: Vec<u8>,
        entry: u64,
        sections: Vec<(&'static str, u64, u64)>,
    },
}

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
    LowerRejected {
        message: String,
    },
    CodegenRejected {
        message: String,
    },
    Ok {
        mwir_dump: String,
        code_words: Vec<u32>,
        layout: LayoutOutcome,
    },
    Bug(String),
}

pub(crate) fn runtime_test_names(program: &sema::typed::TypedProgram) -> Vec<String> {
    program
        .tests
        .iter()
        .filter(|t| t.kind == TestKind::Runtime)
        .map(|t| t.name.clone())
        .collect()
}

pub(crate) fn concat_code_words(program: &codegen::CodegenProgram) -> Vec<u32> {
    let mut words = Vec::new();
    for f in program.fns.values() {
        for ew in &f.code {
            words.push(ew.word);
        }
    }
    words
}

pub(crate) fn attempt_layout(
    program: &sema::typed::TypedProgram,
    codegen_program: &codegen::CodegenProgram,
) -> Result<LayoutOutcome, String> {
    let runtime_tests = runtime_test_names(program);
    if runtime_tests.is_empty() {
        return Ok(LayoutOutcome::Skipped);
    }
    if program
        .tests
        .iter()
        .any(|t| t.kind == TestKind::Runtime && program.fns.get(&t.name).is_none_or(|f| f.is_async))
    {
        return Ok(LayoutOutcome::Skipped);
    }
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

#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct LowerReach {
    check_typed: bool,
    lower_ok: bool,
    lower_rejected: bool,
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

pub(crate) fn fuzz_lower_smoke() -> Result<(), String> {
    let seed_inputs = corpus_seed_inputs()?;
    with_silenced_panic_hook(|| {
        for &seed in FUZZ_LOWER_SMOKE_SEEDS {
            run_lower_fuzz(FUZZ_LOWER_SMOKE_ITERS_PER_SEED, seed, &seed_inputs)?;
        }
        Ok(())
    })
}

pub(crate) const FUZZ_ASYNC_DEEP_ITERS: u64 = 400_000;
pub(crate) const FUZZ_ASYNC_DEEP_SEED: u64 = 1;
pub(crate) const FUZZ_ASYNC_SMOKE_SEEDS: &[u64] = &[1, 2];
pub(crate) const FUZZ_ASYNC_SMOKE_ITERS_PER_SEED: u64 = 1_000;

pub(crate) const ASYNC_SEED_CASES: &[&str] = &[
    "flowwir-basic",
    "flowwir-branch-await",
    "flowwir-chain",
    "flowwir-defer",
    "flowwir-loop-await",
    "asm-async-basic",
    "asm-async-loop-checkpoint",
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

#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct AsyncReach {
    typechecked: bool,
    flow_lowered: bool,
    async_fns: usize,
    codegen_ok: bool,
    image_built: bool,
    async_image: bool,
}

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
    SemaErr {
        category: &'static str,
        message: String,
        line: u32,
        col: u32,
        extra_lines: Vec<String>,
        omit_location: bool,
    },
    Rejected {
        stage: &'static str,
        category: &'static str,
        message: String,
    },
    Ok {
        flow_dump: String,
        code_words: Vec<u32>,
        layout: LayoutOutcome,
    },
    Bug(String),
}

impl AsyncFuzzOutcome {
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
    let mwir_program = match lower::lower_program(&program) {
        Err(e) => {
            return (
                async_stage_err("lower::lower_program", "unimplemented", e.message),
                reach,
            );
        }
        Ok(p) => p,
    };
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
        let compiled = match layout::lower_and_codegen_image(
            &modules,
            &programs,
            &layout_ctx,
            &graph,
            &runtime_tests,
            &async_tests,
            false,
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

pub(crate) const FUZZ_IMPORTS_DEEP_ITERS: u64 = 200_000;
pub(crate) const FUZZ_IMPORTS_DEEP_SEED: u64 = 1;
pub(crate) const FUZZ_IMPORTS_SMOKE_SEEDS: &[u64] = &[1, 2];
pub(crate) const FUZZ_IMPORTS_SMOKE_ITERS_PER_SEED: u64 = 1_000;

pub(crate) struct ImportClosure {
    modules: Vec<(Vec<String>, String)>,
    root: Vec<String>,
}

pub(crate) fn import_test_fn(expect: u32, msg: &str) -> String {
    format!("@test\npub fn t():\n    assert D == {expect}, \"{msg}\"\n")
}

pub(crate) fn import_shape_comptime_construct(n: u32, k: u32) -> ImportClosure {
    let expect = n.wrapping_add(k);
    let app = format!(
        "module app.main\n\nfrom lib.g import Cell\n\nconst D: u32 = Cell(n={n}).n + {k}\n\n{}",
        import_test_fn(expect, "imported comptime construct")
    );
    ImportClosure {
        modules: vec![
            (
                vec!["lib".into(), "g".into()],
                "module lib.g\n\npub struct Cell:\n    pub n: u32\n".into(),
            ),
            (vec!["app".into(), "main".into()], app),
        ],
        root: vec!["app".into(), "main".into()],
    }
}

pub(crate) fn import_shape_fields_and_method(a: u32, b: u32) -> ImportClosure {
    let expect = a.wrapping_add(b);
    let app = format!(
        "module app.main\n\nfrom lib.g import Pair\n\nfn drive() -> u32:\n    return Pair(a={a}, b={b}).sum()\n\nconst D: u32 = drive()\n\n{}",
        import_test_fn(expect, "imported fields and method")
    );
    ImportClosure {
        modules: vec![
            (
                vec!["lib".into(), "g".into()],
                "module lib.g\n\npub struct Pair:\n    pub a: u32\n    pub b: u32\n\n    pub fn sum(read self) -> u32:\n        return self.a + self.b\n"
                    .into(),
            ),
            (vec!["app".into(), "main".into()], app),
        ],
        root: vec!["app".into(), "main".into()],
    }
}

pub(crate) fn import_shape_reachable_unimported(seed: u32, add: u32) -> ImportClosure {
    let expect = seed.wrapping_add(1).wrapping_add(add);
    let app = format!(
        "module app.main\n\nfrom lib.g import Maker\n\nfn drive() -> u32:\n    m = Maker(seed={seed})\n    b = m.build()\n    return b.n + {add}\n\nconst D: u32 = drive()\n\n{}",
        import_test_fn(expect, "reachable unimported Box")
    );
    ImportClosure {
        modules: vec![
            (
                vec!["lib".into(), "g".into()],
                "module lib.g\n\npub struct Box:\n    pub n: u32\n\npub struct Maker:\n    pub seed: u32\n\n    pub fn build(read self) -> Box:\n        return Box(n=self.seed + 1)\n"
                    .into(),
            ),
            (vec!["app".into(), "main".into()], app),
        ],
        root: vec!["app".into(), "main".into()],
    }
}

pub(crate) fn import_shape_alias_peer_generic(n: u32, add: u32) -> ImportClosure {
    let expect = n.wrapping_add(add);
    let app = format!(
        "module app.main\n\nfrom lib.g import Src as Item\nfrom lib.g import wrap_box\nfrom lib.g import peel_box\n\nfn drive() -> u32:\n    s = Item(n={n})\n    b = wrap_box(take s)\n    i: Item = peel_box(take b)\n    return i.n + {add}\n\nconst D: u32 = drive()\n\n{}",
        import_test_fn(expect, "aliased peer + reachable generic")
    );
    ImportClosure {
        modules: vec![
            (
                vec!["lib".into(), "g".into()],
                "module lib.g\n\npub struct Src:\n    pub n: u32\n\npub struct Box[T]:\n    pub v: T\n\npub fn peel_box(take b: Box[Src]) -> Src:\n    return b.v\n\npub fn wrap_box(take s: Src) -> Box[Src]:\n    return Box[Src](v=s)\n"
                    .into(),
            ),
            (vec!["app".into(), "main".into()], app),
        ],
        root: vec!["app".into(), "main".into()],
    }
}

pub(crate) fn import_shape_chain(seed: u32, add: u32) -> ImportClosure {
    let expect = seed.wrapping_add(1).wrapping_add(add);
    let app = format!(
        "module app.main\n\nfrom lib.a import A\n\nfn drive() -> u32:\n    a = A(seed={seed})\n    b = a.make()\n    c = b.get()\n    return c.n + {add}\n\nconst D: u32 = drive()\n\n{}",
        import_test_fn(expect, "two-deep reachable chain")
    );
    ImportClosure {
        modules: vec![
            (
                vec!["lib".into(), "c".into()],
                "module lib.c\n\npub struct C:\n    pub n: u32\n".into(),
            ),
            (
                vec!["lib".into(), "b".into()],
                "module lib.b\n\nfrom lib.c import C\n\npub struct B:\n    pub inner: C\n\n    pub fn get(read self) -> C:\n        return self.inner\n"
                    .into(),
            ),
            (
                vec!["lib".into(), "a".into()],
                "module lib.a\n\nfrom lib.b import B\nfrom lib.c import C\n\npub struct A:\n    pub seed: u32\n\n    pub fn make(read self) -> B:\n        return B(inner=C(n=self.seed + 1))\n"
                    .into(),
            ),
            (vec!["app".into(), "main".into()], app),
        ],
        root: vec!["app".into(), "main".into()],
    }
}

pub(crate) fn import_shape_alias_owner(seed: u32, add: u32) -> ImportClosure {
    let expect = seed.wrapping_add(1).wrapping_add(add);
    let app = format!(
        "module app.main\n\nfrom lib.g import Maker as Builder\n\nfn drive() -> u32:\n    m = Builder(seed={seed})\n    b = m.build()\n    return b.n + {add}\n\nconst D: u32 = drive()\n\n{}",
        import_test_fn(expect, "reachable under aliased owner")
    );
    ImportClosure {
        modules: vec![
            (
                vec!["lib".into(), "g".into()],
                "module lib.g\n\npub struct Box:\n    pub n: u32\n\npub struct Maker:\n    pub seed: u32\n\n    pub fn build(read self) -> Box:\n        return Box(n=self.seed + 1)\n"
                    .into(),
            ),
            (vec!["app".into(), "main".into()], app),
        ],
        root: vec!["app".into(), "main".into()],
    }
}

pub(crate) fn import_shape_enum_payload(n: u32, add: u32) -> ImportClosure {
    let expect = n.wrapping_add(add);
    let app = format!(
        "module app.main\n\nfrom lib.g import Res as R\nfrom lib.g import make\n\nfn drive() -> u32:\n    match make(n={n}):\n        case .Good(p):\n            return p.n + {add}\n        case .Bad:\n            return 0\n\nconst D: u32 = drive()\n\n{}",
        import_test_fn(expect, "aliased enum unimported payload")
    );
    ImportClosure {
        modules: vec![
            (
                vec!["lib".into(), "g".into()],
                "module lib.g\n\npub struct Payload:\n    pub n: u32\n\npub enum Res:\n    Good(Payload)\n    Bad\n\npub fn make(n: u32) -> Res:\n    return Res.Good(Payload(n=n))\n"
                    .into(),
            ),
            (vec!["app".into(), "main".into()], app),
        ],
        root: vec!["app".into(), "main".into()],
    }
}

pub(crate) fn import_shape_enum_payload_generic(n: u32, add: u32) -> ImportClosure {
    let expect = n.wrapping_add(add);
    let app = format!(
        "module app.main\n\nfrom lib.g import Res as R\nfrom lib.g import make\n\nfn drive() -> u32:\n    match make(n={n}):\n        case .Good(b):\n            return b.v.n + {add}\n        case .Bad:\n            return 0\n\nconst D: u32 = drive()\n\n{}",
        import_test_fn(expect, "enum payload generic Box")
    );
    ImportClosure {
        modules: vec![
            (
                vec!["lib".into(), "g".into()],
                "module lib.g\n\npub struct Payload:\n    pub n: u32\n\npub struct Box[T]:\n    pub v: T\n\npub enum Res:\n    Good(Box[Payload])\n    Bad\n\npub fn make(n: u32) -> Res:\n    return Res.Good(Box[Payload](v=Payload(n=n)))\n"
                    .into(),
            ),
            (vec!["app".into(), "main".into()], app),
        ],
        root: vec!["app".into(), "main".into()],
    }
}

pub(crate) fn generate_import_closure(rng: &mut Rng) -> ImportClosure {
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
    if totals.check_accepted == 0 || totals.run_tests == 0 || totals.lower_ok == 0 {
        return Err(format!(
            "fuzz imports: reach collapsed (accepted={}, run_tests={}, lower_ok={}); \
             a lane that reaches none of its subject fails closed",
            totals.check_accepted, totals.run_tests, totals.lower_ok
        ));
    }
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

pub(crate) const FUZZ_REPORT_DEEP_ITERS: u64 = 200_000;
pub(crate) const FUZZ_REPORT_DEEP_SEED: u64 = 1;
pub(crate) const FUZZ_REPORT_SMOKE_SEEDS: &[u64] = &[1, 2];
pub(crate) const FUZZ_REPORT_SMOKE_ITERS_PER_SEED: u64 = 1_000;

fn report_fuzz_baseline() -> String {
    use wrela_machine::layout as ml;
    use wrela_machine::report::EMPTY_SHA256;

    let cores = 2usize;
    let rtdata_base = ml::RTDATA_BASE;
    let rtdata_size = 0x1000u64;
    let entry = ml::IMAGE_BASE;
    let mut s = String::new();
    s.push_str(&format!(
        "Machine revision={}\n",
        wrela_machine::MACHINE_REVISION_STR
    ));
    s.push_str(&format!("Input path=<fuzz> sha256={EMPTY_SHA256}\n"));
    s.push_str(&format!("Image sha256={EMPTY_SHA256}\n"));
    s.push_str(&format!(
        "Section name=rtcode base={:#x} size={}\n",
        entry, 0x1000
    ));
    s.push_str(&format!(
        "Section name=rtdata base={rtdata_base:#x} size={rtdata_size}\n"
    ));
    s.push_str(&format!("Entry base={entry:#x}\n"));
    s.push_str(&format!("Cores count={cores}\n"));
    s.push_str(&format!("CoreEntry core=1 base={:#x}\n", entry + 0x40));
    for c in 0..cores {
        s.push_str(&format!(
            "CoreStack core={c} base={:#x} size={}\n",
            ml::core_stack_base_n(c, cores),
            ml::CORE_STACK_SIZE
        ));
    }
    s.push_str("Actor name=Root\n");
    let cap = 4u64;
    let slot = 16u64;
    let bytes = cap * slot + 24;
    s.push_str(&format!(
        "Ring kind=request src=1 dst=0 target=Root cap={cap} slot={slot} bytes={bytes} \
         base={:#x}\n",
        rtdata_base
    ));
    s.push_str(&format!(
        "Ring kind=reply src=0 dst=1 target=- cap={cap} slot={slot} bytes={bytes} base={:#x}\n",
        rtdata_base + 0x200
    ));
    s
}

fn report_accepted_invariants(p: &wrela_machine::report::ParsedReport) -> Option<String> {
    use wrela_machine::layout as ml;
    let dram_end = ml::dram_end();

    if p.cores < 1 || p.cores > wrela_machine::CORE_SLOTS {
        return Some(format!(
            "accepted `Cores count={}` outside 1..=CORE_SLOTS",
            p.cores
        ));
    }
    if p.entry < ml::DRAM_BASE || p.entry >= dram_end || p.entry % 4 != 0 {
        return Some(format!(
            "accepted `Entry base={:#x}`: outside DRAM or misaligned",
            p.entry
        ));
    }
    for e in &p.core_entries {
        if e.core == 0 || e.core >= p.cores {
            return Some(format!(
                "accepted `CoreEntry core={}` outside 1..cores",
                e.core
            ));
        }
    }
    for r in &p.request_rings {
        if r.src >= p.cores || r.dst >= p.cores {
            return Some(format!(
                "accepted `Ring src={} dst={}` naming a core outside 0..{}",
                r.src, r.dst, p.cores
            ));
        }
        if r.capacity == 0 || r.capacity > ml::DRAM_SIZE {
            return Some(format!(
                "accepted `Ring cap={}`: not a capacity guest DRAM can hold (this is the value \
                 the admission witness uses as a loop bound)",
                r.capacity
            ));
        }
        if r.count_addr < ml::DRAM_BASE || r.count_addr.saturating_add(8) > dram_end {
            return Some(format!(
                "accepted ring `count_addr={:#x}` outside guest DRAM",
                r.count_addr
            ));
        }
    }
    for s in &p.exec_sections {
        if s.base < ml::DRAM_BASE || s.base.saturating_add(s.size) > dram_end {
            return Some(format!(
                "accepted exec `Section name={} base={:#x} size={}` outside guest DRAM (this \
                 range is handed to `hv_vm_protect`)",
                s.name, s.base, s.size
            ));
        }
    }
    None
}

fn mutate_report(rng: &mut Rng, base: &str) -> String {
    const NASTY: &[&str] = &[
        "0",
        "1",
        "18446744073709551615",
        "9223372036854775808",
        "0x0",
        "0xffffffffffffffff",
        "0x40500001",
        "-1",
        "",
        "0x0x40500000",
        "4294967296",
        "33",
    ];
    let mut lines: Vec<String> = base.lines().map(|l| l.to_string()).collect();
    if lines.is_empty() {
        return base.to_string();
    }
    match rng.gen_range(4) {
        0 => {
            let i = rng.gen_range(lines.len());
            let dup = lines[i].clone();
            lines.insert(i, dup);
        }
        1 => {
            let i = rng.gen_range(lines.len());
            lines.remove(i);
        }
        2 => {
            let i = rng.gen_range(lines.len());
            let line = lines[i].clone();
            let fields: Vec<usize> = line
                .char_indices()
                .filter(|(_, c)| *c == '=')
                .map(|(i, _)| i)
                .collect();
            if !fields.is_empty() {
                let eq = fields[rng.gen_range(fields.len())];
                let tail = &line[eq + 1..];
                let end = tail.find(' ').map(|j| eq + 1 + j).unwrap_or(line.len());
                let nasty = NASTY[rng.gen_range(NASTY.len())];
                lines[i] = format!("{}{}{}", &line[..=eq], nasty, &line[end..]);
            }
        }
        _ => {
            let i = rng.gen_range(lines.len());
            lines.push(lines[i].clone());
        }
    }
    let mut out = lines.join("\n");
    out.push('\n');
    out
}

fn run_report_fuzz(iters: u64, seed: u64) -> Result<(), String> {
    let base = report_fuzz_baseline();
    let parsed_base = wrela_machine::report::parse_report(&base).map_err(|e| {
        format!("fuzz report: the baseline report must parse, but it was rejected: {e}")
    })?;
    if let Some(bad) = report_accepted_invariants(&parsed_base) {
        return Err(format!(
            "fuzz report: the baseline violates its own oracle: {bad}"
        ));
    }

    let mut rng = Rng::new(seed);
    let (mut accepted, mut rejected) = (0u64, 0u64);
    for i in 0..iters {
        let input = mutate_report(&mut rng, &base);
        let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            wrela_machine::report::parse_report(&input)
        }));
        match outcome {
            Err(_) => {
                return report_fuzz_failure(
                    "report",
                    "report-crash-",
                    seed,
                    i,
                    &input,
                    "parse_report panicked (a panic on a forged report is a bug, not a rejection)",
                );
            }
            Ok(Ok(parsed)) => {
                accepted += 1;
                if let Some(bad) = report_accepted_invariants(&parsed) {
                    return report_fuzz_failure(
                        "report",
                        "report-accept-",
                        seed,
                        i,
                        &input,
                        &format!("parse_report ACCEPTED a report the VMM cannot trust: {bad}"),
                    );
                }
            }
            Ok(Err(msg)) => {
                rejected += 1;
                if msg.contains("internal error:") {
                    return report_fuzz_failure(
                        "report",
                        "report-internal-",
                        seed,
                        i,
                        &input,
                        &format!("parse_report reported an internal error: {msg}"),
                    );
                }
            }
        }
    }
    println!(
        "fuzz report: {iters} iteration(s), seed {seed}: no panic, no internal error, no \
         untrustworthy accept ({accepted} accepted, {rejected} rejected)"
    );
    if accepted == 0 {
        return Err(
            "fuzz report: every mutation was rejected — the lane is clean about nothing (the \
             generator or the mutator has drifted away from well-formed reports)"
                .to_string(),
        );
    }
    Ok(())
}

pub(crate) fn fuzz_report(iters: u64, seed: u64) -> Result<(), String> {
    with_silenced_panic_hook(|| run_report_fuzz(iters, seed))
}

pub(crate) fn fuzz_report_smoke() -> Result<(), String> {
    with_silenced_panic_hook(|| {
        for &seed in FUZZ_REPORT_SMOKE_SEEDS {
            run_report_fuzz(FUZZ_REPORT_SMOKE_ITERS_PER_SEED, seed)?;
        }
        Ok(())
    })
}

pub(crate) fn fuzz_smoke_all() -> Result<(), String> {
    fuzz_lexer_smoke()?;
    fuzz_parser_smoke()?;
    fuzz_sema_smoke()?;
    fuzz_eval_smoke()?;
    fuzz_lower_smoke()?;
    fuzz_async_smoke()?;
    fuzz_imports_smoke()?;
    fuzz_report_smoke()
}

pub(crate) fn fuzz_deep_all() -> Result<(), String> {
    let _mode = crate::CompileOptsGuard::mode(wrela_compiler::opts::CompileMode::Dev);
    fuzz_lexer(FUZZ_LEXER_DEEP_ITERS, FUZZ_LEXER_DEEP_SEED)?;
    fuzz_parser(FUZZ_PARSER_DEEP_ITERS, FUZZ_PARSER_DEEP_SEED)?;
    fuzz_sema(FUZZ_SEMA_DEEP_ITERS, FUZZ_SEMA_DEEP_SEED)?;
    fuzz_eval(FUZZ_EVAL_DEEP_ITERS, FUZZ_EVAL_DEEP_SEED)?;
    fuzz_lower(FUZZ_LOWER_DEEP_ITERS, FUZZ_LOWER_DEEP_SEED)?;
    fuzz_async(FUZZ_ASYNC_DEEP_ITERS, FUZZ_ASYNC_DEEP_SEED)?;
    fuzz_imports(FUZZ_IMPORTS_DEEP_ITERS, FUZZ_IMPORTS_DEEP_SEED)?;
    fuzz_report(FUZZ_REPORT_DEEP_ITERS, FUZZ_REPORT_DEEP_SEED)
}
