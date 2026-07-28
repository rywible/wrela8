//! `corpus` subcommand and helpers (extracted from main.rs).

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

use crate::corpus_sema_census;
use crate::corpus_sema_context;
use crate::{golden_case_dirs, root};

// --- corpus ---------------------------------------------------------------
//
// The docs are test inputs: every ```wrela fenced block in docs/language/
// must lex cleanly (from M1 on, also parse — fragments containing `...`
// stay lex-only). Blocks are materialized under target/corpus/ for
// debugging; the check itself runs in-process.

/// One ```wrela fenced block extracted from a docs/language/*.md file.
pub(crate) struct DocBlock {
    pub(crate) doc: PathBuf,
    pub(crate) start_line: usize,
    pub(crate) name: String,
    pub(crate) body: String,
}

/// Walks every docs/language/*.md file and pulls out its ```wrela blocks,
/// in deterministic (sorted-by-filename, source-order-within-file) order.
/// Shared by `corpus` (which lexes every block) and `fuzz` (which mutates
/// them) — the walk exists exactly once.
pub(crate) fn extract_doc_blocks() -> Result<(Vec<DocBlock>, Vec<String>), String> {
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
pub(crate) fn extract_example_files() -> Result<Vec<DocBlock>, String> {
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
///
/// Always then sema-classifies the parseable non-aspirational set against
/// the pinned census (plans/M9.md item J3). `--sema` only prints the
/// verbose per-block report; the gate is identical with or without it.
pub(crate) fn corpus(args: &[String]) -> Result<(), String> {
    let mut verbose_sema = false;
    for a in args {
        match a.as_str() {
            "--sema" => verbose_sema = true,
            other => {
                return Err(format!(
                    "corpus: unknown argument `{other}` (want `corpus` or `corpus --sema`)"
                ));
            }
        }
    }
    let out_dir = root().join("target/corpus");
    std::fs::create_dir_all(&out_dir).map_err(|e| format!("create {}: {e}", out_dir.display()))?;
    let (blocks, mut failures) = extract_doc_blocks()?;
    let examples = extract_example_files()?;
    let mut lexed = 0usize;
    let mut parsed = 0usize;
    let mut fragments = 0usize;
    let mut sema_rows: Vec<CorpusSemaRow> = Vec::new();
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
            Ok(parsed_ast) => {
                parsed += 1;
                // Aspirational whole-file examples (header marks ASPIRATIONAL)
                // stay lex/parse-only: they import modules the stdlib does
                // not ship. Sema would only restate the missing-module fact
                // (plans/M9.md item J2a / decision 516).
                if !example_is_aspirational(&b) {
                    sema_rows.push(corpus_sema_one(&b, parsed_ast)?);
                }
            }
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
    if !failures.is_empty() {
        for f in &failures {
            eprintln!("{f}");
        }
        return Err(format!("corpus: {} failure(s)", failures.len()));
    }
    println!("corpus: lexed {lexed}, parsed {parsed}, fragments skipped {fragments}");
    if verbose_sema {
        print_corpus_sema_report(&sema_rows);
    }
    // Keys first: a duplicate/malformed key makes the other two ambiguous.
    // The stub and census checks then both run, because one edited block
    // fails both and a human wants to see both rows to repin, not to
    // rediscover the second failure after fixing the first.
    verify_corpus_sema_keys(&sema_rows)?;
    let mut errs = Vec::new();
    if let Err(e) = verify_corpus_sema_contexts(&sema_rows) {
        errs.push(e);
    }
    if let Err(e) = verify_corpus_sema_census(&sema_rows) {
        errs.push(e);
    }
    if !errs.is_empty() {
        return Err(errs.join("; "));
    }
    Ok(())
}

/// Whole-file `docs/language/examples/*.wr` whose header marks
/// `ASPIRATIONAL` — lex/parse corpus only until the imported modules ship
/// (plans/M9.md decision 516). Markdown ```wrela fences are never
/// aspirational under this predicate.
pub(crate) fn example_is_aspirational(b: &DocBlock) -> bool {
    let rel = display_repo_path(&b.doc);
    if !rel.starts_with("docs/language/examples/") || !rel.ends_with(".wr") {
        return false;
    }
    // Only the file header (comments before `module`) — not a later
    // occurrence inside the body.
    for line in b.body.lines() {
        let t = line.trim();
        if t.is_empty() {
            continue;
        }
        if t.starts_with("module ") {
            return false;
        }
        if t.starts_with('#') && t.contains("ASPIRATIONAL") {
            return true;
        }
    }
    false
}

/// One `--sema` outcome (plans/M9.md item J1b). `kind` is ok or
/// disagreement — the J1 "noise" keyhole is retired by per-block context.
#[derive(Debug)]
pub(crate) struct CorpusSemaRow {
    /// Content key of the block body — the identity used by both pinned
    /// tables (plans/M10.md item A3, decision 710).
    key: String,
    /// Live `path:line`, for humans and diagnostics only.
    loc: String,
    section: String,
    kind: CorpusSemaKind,
    detail: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CorpusSemaKind {
    Ok,
    /// Genuine doc/compiler disagreement (or residual incomplete context
    /// that could not be given honestly — counted as disagreement so the
    /// keyhole cannot hide it).
    Disagreement,
}

impl CorpusSemaKind {
    fn as_str(self) -> &'static str {
        match self {
            CorpusSemaKind::Ok => "ok",
            CorpusSemaKind::Disagreement => "disagreement",
        }
    }
}

/// Sema-check one parseable corpus block. Fragments are wrapped so
/// declarations stay at module scope (or nest into a preamble type —
/// decision J1c) and statements sit inside a synthetic
/// `fn _corpus_snippet()` (decision 501), with an optional per-block
/// preamble from `corpus_sema_context` (decision J1b). Import-bearing
/// modules — and import-bearing fragment wraps — load through
/// `loader::load_closure` so path-agreement and missing-module errors
/// surface honestly (decision 503).
pub(crate) fn corpus_sema_one(b: &DocBlock, parsed_ast: Parsed) -> Result<CorpusSemaRow, String> {
    let loc = format!("{}:{}", display_repo_path(&b.doc), b.start_line);
    let section = nearest_doc_section(&b.doc, b.start_line);
    let key = corpus_sema_context::block_key(&b.body);
    let ctx = corpus_sema_context::lookup(&key);
    let result = match parsed_ast {
        Parsed::Module(module) => {
            if module.imports.is_empty() {
                let path = synthetic_module_path(&module);
                sema::check(&module, &path).map_err(|e| format!("[{}] {}", e.category, e.message))
            } else {
                corpus_sema_load_file(&b.doc)
            }
        }
        Parsed::Fragment(entries) => {
            // Wrap / preservation / re-lex / re-parse failures are harness
            // bugs (propagate via `?`). Only the subsequent sema/loader
            // diagnostic becomes a disagreement row.
            let wrapped = wrap_corpus_fragment(&entries, ctx)
                .map_err(|e| format!("{}:{}: {e}", b.doc.display(), b.start_line))?;
            assert_fragment_items_preserved(&entries, &wrapped, &loc)?;
            corpus_sema_check_wrapped_sema(&wrapped)
                .map_err(|e| format!("{}:{}: {e}", b.doc.display(), b.start_line))?
        }
    };
    match result {
        Ok(()) => Ok(CorpusSemaRow {
            key,
            loc,
            section,
            kind: CorpusSemaKind::Ok,
            detail: "ok".into(),
        }),
        Err(detail) => Ok(CorpusSemaRow {
            key,
            loc,
            section,
            kind: CorpusSemaKind::Disagreement,
            detail,
        }),
    }
}

pub(crate) fn corpus_sema_load_file(path: &Path) -> Result<(), String> {
    // Prefer a repo-relative path so the loader's path-agreement
    // diagnostic names the tree path a human greps, not `/private/tmp/...`.
    let rel = path.strip_prefix(root()).unwrap_or(path);
    let loaded = loader::load_closure(rel).map_err(|e| match e {
        loader::LoadError::Lex(e) => format!("[lex] {}", e.message),
        loader::LoadError::Parse(e) => format!("[parse] {}", e.message),
        loader::LoadError::Build(e) => format!("[{}] {}", e.category, e.message),
    })?;
    let mut modules = BTreeMap::new();
    let mut paths = BTreeMap::new();
    for (key, loaded_mod) in &loaded.modules {
        modules.insert(key.clone(), loaded_mod.module.clone());
        paths.insert(key.clone(), loaded_mod.file.display().to_string());
    }
    sema::check_program(&modules, &paths).map_err(|e| format!("[{}] {}", e.category, e.message))
}

/// Wrap a ```wrela fragment into a checkable module: optional preamble,
/// item declarations at module scope or nested into a preamble type
/// (`nest_items_into`), optional postamble, statement entries inside
/// `fn _corpus_snippet()` (shape configurable via context).
///
/// Fence items are never discarded. Nesting is the only alternative to
/// module scope; a context that cannot nest honestly must not ship a
/// hand-copy (plans/M9.md item J1c).
pub(crate) fn wrap_corpus_fragment(
    entries: &[parser::FragmentEntry],
    ctx: Option<&corpus_sema_context::CorpusSemaContext>,
) -> Result<String, String> {
    use parser::FragmentEntry;
    let nest_into = ctx.map(|c| c.nest_items_into).unwrap_or("");
    let mut items = Vec::new();
    let mut stmts = Vec::new();
    for e in entries {
        match e {
            FragmentEntry::Item(i) => items.push(i.clone()),
            FragmentEntry::Stmt(s) => stmts.push(s.clone()),
        }
    }
    if !nest_into.is_empty() {
        let preamble = ctx.map(|c| c.preamble).unwrap_or("");
        let as_struct = format!("struct {nest_into}");
        let as_enum = format!("enum {nest_into}");
        if !preamble.contains(&as_struct) && !preamble.contains(&as_enum) {
            return Err(format!(
                "corpus sema: nest_items_into=`{nest_into}` but preamble has no `struct`/`enum` of that name — refuse rather than check a copy"
            ));
        }
        if items.is_empty() {
            return Err(format!(
                "corpus sema: nest_items_into=`{nest_into}` but the fence has no items to nest — refuse rather than check a copy"
            ));
        }
    }
    let mut out = String::from("module corpus.snippet\n\n");
    if let Some(c) = ctx {
        let preamble = c.preamble.trim();
        if !preamble.is_empty() {
            out.push_str(preamble);
            out.push_str("\n\n");
        }
    }
    if !items.is_empty() {
        let frag: Vec<_> = items.into_iter().map(FragmentEntry::Item).collect();
        let pretty = printer::pretty_fragment(&frag);
        if nest_into.is_empty() {
            out.push_str(&pretty);
        } else {
            // Indent every line one level so the fence items continue the
            // open type body left by the preamble.
            for line in pretty.lines() {
                if line.is_empty() {
                    out.push('\n');
                } else {
                    out.push_str("    ");
                    out.push_str(line);
                    out.push('\n');
                }
            }
        }
        if !out.ends_with('\n') {
            out.push('\n');
        }
        out.push('\n');
    }
    if let Some(c) = ctx {
        let postamble = c.postamble.trim_end_matches('\n');
        let postamble = postamble.strip_prefix('\n').unwrap_or(postamble);
        if !postamble.is_empty() {
            out.push_str(postamble);
            out.push_str("\n\n");
        }
    }
    if !stmts.is_empty()
        || ctx
            .map(|c| !c.params.is_empty() || !c.ret.is_empty())
            .unwrap_or(false)
    {
        let async_kw = if ctx.map(|c| c.async_wrapper).unwrap_or(false) {
            "async "
        } else {
            ""
        };
        let params = ctx.map(|c| c.params).unwrap_or("");
        let ret = ctx.map(|c| c.ret).unwrap_or("");
        out.push_str(async_kw);
        out.push_str("fn _corpus_snippet(");
        out.push_str(params);
        out.push(')');
        if !ret.is_empty() {
            out.push_str(" -> ");
            out.push_str(ret);
        }
        out.push_str(":\n");
        if !stmts.is_empty() {
            let frag: Vec<_> = stmts.into_iter().map(FragmentEntry::Stmt).collect();
            let pretty = printer::pretty_fragment(&frag);
            for line in pretty.lines() {
                if line.is_empty() {
                    out.push('\n');
                } else {
                    out.push_str("    ");
                    out.push_str(line);
                    out.push('\n');
                }
            }
        }
        if let Some(c) = ctx {
            if !c.ret_ok.is_empty() {
                out.push_str("    return ");
                out.push_str(c.ret_ok);
                out.push('\n');
            }
        }
    }
    Ok(out)
}

/// Standing guard (J1c): every fence `Item` must appear in the wrap.
/// Trim-equal line match so nesting indent does not count as a rewrite.
/// Failure is a harness error (fails `check`), not a disagreement row —
/// silent decoupling is exactly what the corpus exists to catch.
pub(crate) fn assert_fragment_items_preserved(
    entries: &[parser::FragmentEntry],
    wrapped: &str,
    loc: &str,
) -> Result<(), String> {
    use parser::FragmentEntry;
    for e in entries {
        let FragmentEntry::Item(item) = e else {
            continue;
        };
        let pretty = printer::pretty_fragment(&[FragmentEntry::Item(item.clone())]);
        for line in pretty.lines() {
            let needle = line.trim();
            if needle.is_empty() {
                continue;
            }
            let found = wrapped.lines().any(|w| w.trim() == needle);
            if !found {
                return Err(format!(
                    "corpus sema: fence item content missing from wrap at {loc} \
                     (silent decoupling): `{needle}`"
                ));
            }
        }
    }
    Ok(())
}

/// Lex/parse a wrapped fragment, then sema-check it. Outer `Err` = harness
/// failure (re-lex / re-parse / temp I/O). Inner `Err` = disagreement
/// diagnostic from sema or the loader.
pub(crate) fn corpus_sema_check_wrapped_sema(wrapped: &str) -> Result<Result<(), String>, String> {
    let tokens =
        lexer::lex(wrapped).map_err(|e| format!("--sema wrap re-lex failed: {}", e.message))?;
    let module =
        parser::parse(tokens).map_err(|e| format!("--sema wrap re-parse failed: {}", e.message))?;
    if module.imports.is_empty() {
        return Ok(sema::check(&module, "corpus/snippet.wr")
            .map_err(|e| format!("[{}] {}", e.category, e.message)));
    }
    let dir = std::env::temp_dir().join(format!(
        "wrela-corpus-sema-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    ));
    let corpus_dir = dir.join("corpus");
    std::fs::create_dir_all(&corpus_dir).map_err(|e| format!("--sema wrap temp dir: {e}"))?;
    let path = corpus_dir.join("snippet.wr");
    std::fs::write(&path, wrapped).map_err(|e| format!("--sema wrap temp write: {e}"))?;
    let result = corpus_sema_load_file(&path);
    let _ = std::fs::remove_dir_all(&dir);
    Ok(result)
}

pub(crate) fn synthetic_module_path(module: &Module) -> String {
    let last = module
        .path
        .last()
        .cloned()
        .unwrap_or_else(|| "snippet".into());
    if module.path.len() > 1 {
        format!(
            "{}/{last}.wr",
            module.path[..module.path.len() - 1].join("/")
        )
    } else {
        format!("{last}.wr")
    }
}

pub(crate) fn display_repo_path(path: &Path) -> String {
    path.strip_prefix(root())
        .unwrap_or(path)
        .display()
        .to_string()
}

/// Nearest ATX heading above `start_line` (1-based body line), for the
/// disagreement enumeration. Falls back to the file stem when the block
/// is a whole `.wr` example with no markdown headings. Headings inside
/// fenced code blocks are ignored (a `# comment` in a ```text diagram
/// is not a section title).
pub(crate) fn nearest_doc_section(doc: &Path, start_line: usize) -> String {
    let Ok(text) = std::fs::read_to_string(doc) else {
        return doc
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("?")
            .to_string();
    };
    if doc.extension().is_some_and(|x| x == "wr") {
        return doc
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("example.wr")
            .to_string();
    }
    let mut section = String::from("(top)");
    let mut in_fence = false;
    for (i, line) in text.lines().enumerate() {
        let line_no = i + 1;
        if line_no >= start_line {
            break;
        }
        let trimmed = line.trim_start();
        if trimmed.starts_with("```") {
            in_fence = !in_fence;
            continue;
        }
        if in_fence {
            continue;
        }
        if trimmed.starts_with('#') {
            let hashes = trimmed.chars().take_while(|c| *c == '#').count();
            if (1..=6).contains(&hashes) {
                let title = trimmed[hashes..].trim();
                // Require the markdown space after the hashes so a
                // mid-prose `#comment` is not treated as a heading.
                if !title.is_empty() && trimmed.as_bytes().get(hashes).is_some_and(|b| *b == b' ') {
                    section = title.to_string();
                }
            }
        }
    }
    section
}

pub(crate) fn print_corpus_sema_report(rows: &[CorpusSemaRow]) {
    let ok = rows.iter().filter(|r| r.kind == CorpusSemaKind::Ok).count();
    let disagreements = rows
        .iter()
        .filter(|r| r.kind == CorpusSemaKind::Disagreement)
        .count();
    println!(
        "corpus sema: checked {}, ok {ok}, disagreements {disagreements}",
        rows.len()
    );
    // Every row, with its content key: the pinned tables are keyed by that
    // key and humans hand-write them, so the report has to print it — a
    // key you cannot read off the report is a key you cannot pin
    // (plans/M10.md item A3, decision 712).
    println!("corpus sema blocks (key = content hash; pin tables match on this, not on the line):");
    for r in rows {
        println!(
            "  {:<13} {:<12} {} (§ {})",
            r.kind.as_str(),
            r.key,
            r.loc,
            r.section
        );
    }
    if disagreements > 0 {
        println!("corpus sema disagreements:");
        for r in rows
            .iter()
            .filter(|r| r.kind == CorpusSemaKind::Disagreement)
        {
            println!("  {} [{}] (§ {})", r.loc, r.key, r.section);
            println!("    {}", r.detail);
        }
    }
}

/// Content keys must be unique and well-formed, in the live set and in both
/// pinned tables (plans/M10.md item A3). Uniqueness is what makes a 12-hex
/// content key safe to use as an identity: a collision, or two doc fences
/// with byte-identical bodies, would make a pin ambiguous — so it fails
/// closed here instead of picking one.
pub(crate) fn verify_corpus_sema_keys(rows: &[CorpusSemaRow]) -> Result<(), String> {
    use std::collections::BTreeMap;
    let mut problems = Vec::new();
    let mut seen: BTreeMap<&str, &str> = BTreeMap::new();
    for r in rows {
        if r.key.len() != 12 || !r.key.bytes().all(|b| b.is_ascii_hexdigit()) {
            problems.push(format!(
                "block `{}` has malformed content key `{}` (want 12 hex chars)",
                r.loc, r.key
            ));
        }
        if let Some(prev) = seen.insert(&r.key, &r.loc) {
            problems.push(format!(
                "two corpus blocks share content key `{}`: `{prev}` and `{}` — \
                 byte-identical fence bodies (or a hash collision) make a pin \
                 ambiguous; make one block's text differ",
                r.key, r.loc
            ));
        }
    }
    let mut pinned_keys: BTreeMap<&str, &str> = BTreeMap::new();
    for p in corpus_sema_census::pins() {
        if let Some(prev) = pinned_keys.insert(p.key.as_str(), p.loc.as_str()) {
            problems.push(format!(
                "ledger/census.toml [[corpus_sema.pins]] key `{}` twice (`{prev}` and `{}`)",
                p.key, p.loc
            ));
        }
    }
    let mut ctx_keys: BTreeMap<&str, usize> = BTreeMap::new();
    for c in corpus_sema_context::CORPUS_SEMA_CONTEXTS {
        if let Some(prev) = ctx_keys.insert(c.key, c.line) {
            problems.push(format!(
                "corpus_sema_context.rs declares key `{}` twice (lines {prev} and {})",
                c.key, c.line
            ));
        }
    }
    if !problems.is_empty() {
        for p in &problems {
            eprintln!("corpus sema keys: {p}");
        }
        return Err(format!("corpus sema keys: {} problem(s)", problems.len()));
    }
    Ok(())
}

/// Every per-block declaration stub must attach to a live block
/// (plans/M10.md item A3). Before the content-key change a stub whose line
/// number had shifted simply stopped matching and the block was checked
/// bare — the silent detach that produced three phantom disagreements in
/// item A. Now a stub that matches nothing is a loud failure.
pub(crate) fn verify_corpus_sema_contexts(rows: &[CorpusSemaRow]) -> Result<(), String> {
    let mut problems = Vec::new();
    for c in corpus_sema_context::CORPUS_SEMA_CONTEXTS {
        if !rows.iter().any(|r| r.key == c.key) {
            problems.push(format!(
                "stub for `{}:{}` (§ {}) has key `{}`, which matches no live corpus \
                 block — that fence's body was edited, or the fence was removed. \
                 Stubs are keyed by block content, so this is not a line-number \
                 drift you can ignore: re-review the block, then update its `key` \
                 in crates/xtask/src/corpus_sema_context.rs to the key printed for \
                 it by `cargo xtask corpus --sema` (and delete the stub if the \
                 block is gone).",
                c.doc, c.line, c.section, c.key
            ));
        }
    }
    if !problems.is_empty() {
        for p in &problems {
            eprintln!("corpus sema context: {p}");
        }
        return Err(format!(
            "corpus sema context: {} stale stub(s); update crates/xtask/src/corpus_sema_context.rs after review",
            problems.len()
        ));
    }
    Ok(())
}

/// Compare live corpus-sema rows against the pinned census (plans/M9.md
/// item J3). Fails in both directions: an `ok` block that starts
/// disagreeing (`ok-decay`), and an accepted disagreement that starts
/// typechecking (`accepted-disagreement-cleared`, naming the cited ledger
/// gap). Also fails if a pin's shape is wrong, a block is unpinned /
/// missing, or an accepted row's ledger gap is missing / no longer
/// `status = "gap"`.
///
/// Matching is on the block's **content key**, never on `path:line`
/// (plans/M10.md item A3, decision 710) — an insertion above a fence must
/// not disturb its pin. The flip side is that editing a fence's body loses
/// its pin; that is reported as a re-review requirement, not as decay
/// (decision 711).
pub(crate) fn verify_corpus_sema_census(rows: &[CorpusSemaRow]) -> Result<(), String> {
    use std::collections::BTreeMap;
    let live: BTreeMap<&str, &CorpusSemaRow> = rows.iter().map(|r| (r.key.as_str(), r)).collect();
    let pinned: BTreeMap<&str, &corpus_sema_census::CorpusSemaPin> = corpus_sema_census::pins()
        .iter()
        .map(|p| (p.key.as_str(), p))
        .collect();

    let mut problems = Vec::new();
    for (key, pin) in &pinned {
        let loc = pin.loc.as_str();
        let kind = pin.kind.as_str();
        // Structural: ok rows have no gap; disagreements must cite one.
        match (kind, pin.gap.as_deref()) {
            ("ok", None) => {}
            ("disagreement", Some(_)) => {}
            ("ok", Some(gap)) => {
                problems.push(format!("pin `{loc}`: kind=ok must not cite gap `{gap}`"))
            }
            ("disagreement", None) => problems.push(format!(
                "pin `{loc}`: accepted disagreement must cite a ledger gap id"
            )),
            (other, _) => problems.push(format!(
                "pin `{loc}`: unknown kind `{other}` (want ok or disagreement)"
            )),
        }
        match live.get(key) {
            None => problems.push(format!(
                "pin-lost: no live corpus block has content key `{key}` (pinned \
                 {kind} for `{loc}`) — that fence's body was edited, or the fence was \
                 removed. Pins are keyed by block content, so this is not pin \
                 decay and not a line-number shift: the edited block's \
                 classification must be re-reviewed. Run `cargo xtask corpus \
                 --sema`, read the block's new classification, then update this \
                 row's `key` (and `loc`) in ledger/census.toml \
                 — or delete the row if the block is gone."
            )),
            Some(row) if row.kind.as_str() != kind => {
                let live_kind = row.kind.as_str();
                let at = &row.loc;
                if kind == "ok" {
                    problems.push(format!(
                        "ok-decay: `{at}` [{key}] was pinned ok, now {live_kind}"
                    ));
                } else if kind == "disagreement" && live_kind == "ok" {
                    let gap = pin.gap.as_deref().unwrap_or("<missing gap>");
                    problems.push(format!(
                        "accepted-disagreement-cleared: `{at}` [{key}] was pinned as \
                         disagreement citing gap `{gap}`, but now typechecks — \
                         update ledger/census.toml and flip that ledger gap \
                         to `test` (or restore the disagreement if the clearance \
                         is wrong)"
                    ));
                } else {
                    problems.push(format!(
                        "census drift: `{at}` [{key}] pinned {kind}, live {live_kind}"
                    ));
                }
            }
            Some(_) => {}
        }
    }
    for (key, row) in &live {
        if !pinned.contains_key(key) {
            problems.push(format!(
                "unpinned block `{}` (currently at {}, live classification {}) — \
                 either a new fence, or an existing fence whose body was edited \
                 (editing a body changes its key by design). An edited block's \
                 pinned classification must be re-reviewed and updated: after \
                 reviewing `cargo xtask corpus --sema`, add or repin the row in \
                 ledger/census.toml with key `{}`.",
                key,
                row.loc,
                row.kind.as_str(),
                key
            ));
        }
    }

    // Coupling: every accepted disagreement's gap must exist and still be
    // a gap. Flipping the ledger without un-accepting the row (or the
    // reverse) fails here.
    let gap_status = ledger_clause_statuses()?;
    for pin in corpus_sema_census::pins() {
        let Some(gap) = pin.gap.as_deref() else {
            continue;
        };
        match gap_status.get(gap).map(String::as_str) {
            Some("gap") => {}
            Some(status) => problems.push(format!(
                "accepted-disagreement gap `{gap}` (cited by `{}`) is \
                 status=`{status}` in ledger/ledger.toml — un-accept the \
                 census row or reopen the gap; they must move together",
                pin.loc
            )),
            None => problems.push(format!(
                "accepted-disagreement cites unknown ledger gap `{gap}` \
                 (cited by `{}`) — add the clause or fix the pin",
                pin.loc
            )),
        }
    }

    if !problems.is_empty() {
        for p in &problems {
            eprintln!("corpus sema census: {p}");
        }
        return Err(format!(
            "corpus sema census: {} mismatch(es); update ledger/census.toml after review",
            problems.len()
        ));
    }
    Ok(())
}

/// Map every `[[clause]]` id in `ledger/ledger.toml` to its `status`
/// string. Used by the corpus-sema pin so accepted-disagreement gap cites
/// cannot drift from the gap list (plans/M9.md item J3).
pub(crate) fn ledger_clause_statuses() -> Result<std::collections::BTreeMap<String, String>, String>
{
    let path = root().join("ledger/ledger.toml");
    let text =
        std::fs::read_to_string(&path).map_err(|e| format!("read {}: {e}", path.display()))?;
    let value: toml::Value = text.parse().map_err(|e| format!("parse ledger: {e}"))?;
    let clauses = value
        .get("clause")
        .and_then(|c| c.as_array())
        .ok_or("ledger has no [[clause]] entries")?;
    let mut out = std::collections::BTreeMap::new();
    for (i, clause) in clauses.iter().enumerate() {
        let id = clause
            .get("id")
            .and_then(|v| v.as_str())
            .ok_or(format!("clause {i}: missing id"))?;
        let status = clause
            .get("status")
            .and_then(|v| v.as_str())
            .ok_or(format!("clause `{id}`: missing status"))?;
        out.insert(id.to_string(), status.to_string());
    }
    Ok(out)
}
