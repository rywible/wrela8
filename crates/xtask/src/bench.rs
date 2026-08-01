use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

use wrela_compiler::codegen;
use wrela_compiler::eval;
use wrela_compiler::layout;
use wrela_compiler::lower;
use wrela_compiler::report;
use wrela_compiler::sema;
use wrela_compiler::sema::typed::TestKind;
use wrela_compiler::syntax::ast::Module;
use wrela_compiler::syntax::lexer::{self};
use wrela_compiler::syntax::parser::{self};

use crate::corpus::{extract_doc_blocks, extract_example_files};
use crate::golden::{build_and_sign_vmm, golden_case_target};
use crate::{
    CompileOptsGuard, GuestRecord, build_runtime_test_image, fail_closed, golden_case_dirs,
    parse_guest_record, produce_report_and_image, root, typecheck_for_diff_eval,
};

pub(crate) const BENCH_GUEST_WARMUP_ITERS: usize = 2;
pub(crate) const BENCH_GUEST_TIMED_ITERS: usize = 5;

pub(crate) fn guest_bench_threshold_us() -> Result<u128, String> {
    bench_threshold_us("guest", "boot_actors_median_us")
}

pub(crate) fn boot_hello_test_image() -> Result<(Vec<u8>, String), String> {
    golden_test_image("boot-hello")
}

pub(crate) fn golden_test_image(case_name: &str) -> Result<(Vec<u8>, String), String> {
    let _mode = CompileOptsGuard::mode(wrela_compiler::opts::CompileMode::Release);
    let case = root().join("tests/golden").join(case_name);
    let target = golden_case_target(&case)?
        .ok_or_else(|| format!("tests/golden/{case_name} has no input.wr"))?;
    let source =
        std::fs::read_to_string(&target).map_err(|e| format!("read {}: {e}", target.display()))?;
    let path_display = target.display().to_string();
    let checked = typecheck_for_diff_eval(&target)
        .ok_or_else(|| format!("tests/golden/{case_name} failed to typecheck"))?;
    let program = &checked.root_program;
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
    build_runtime_test_image(
        program,
        &checked.modules,
        &checked.programs,
        &source,
        &path_display,
        &runtime_names,
    )
}

pub(crate) fn bench_guest_lane() -> Result<(), String> {
    let _mode = CompileOptsGuard::mode(wrela_compiler::opts::CompileMode::Release);
    let vmm = build_and_sign_vmm()?;
    let (img_bytes, report_text) = golden_test_image("boot-actors")?;

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
         of tests/golden/boot-actors"
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

pub(crate) fn profile() -> Result<(), String> {
    let _mode = CompileOptsGuard::mode(wrela_compiler::opts::CompileMode::Release);
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
    let image_digest = report::sha256_hex(&image_layout.blob);
    let mut report_text = format!(
        "Machine revision={}\nInput path={path_display} sha256={source_digest}\nImage sha256={image_digest}\n",
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

pub(crate) const BENCH_WARMUP_ITERS: usize = 3;
pub(crate) const BENCH_TIMED_ITERS: usize = 5;

pub(crate) struct BenchEntry {
    name: String,
    body: String,
    has_dots: bool,
}

pub(crate) fn bench_corpus_entries() -> Result<Vec<BenchEntry>, String> {
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

pub(crate) fn run_bench_workload(
    entries: &[BenchEntry],
    track_index: usize,
) -> (Duration, Duration) {
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

pub(crate) struct CheckBenchEntry {
    body: String,
}

pub(crate) fn bench_check_entries() -> Result<Vec<CheckBenchEntry>, String> {
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

pub(crate) fn run_check_bench_workload(entries: &[CheckBenchEntry]) -> Duration {
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

pub(crate) struct EvalBenchEntry {
    body: String,
}

pub(crate) fn bench_eval_entries() -> Result<Vec<EvalBenchEntry>, String> {
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

pub(crate) fn run_eval_bench_workload(entries: &[EvalBenchEntry]) -> Duration {
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

pub(crate) fn bench_threshold_us(section: &str, key: &str) -> Result<u128, String> {
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

pub(crate) fn compiler_bench_threshold_us() -> Result<u128, String> {
    bench_threshold_us("compiler", "full_corpus_per_entry_us")
}

pub(crate) fn check_bench_threshold_us() -> Result<u128, String> {
    bench_threshold_us("compiler", "check_golden_per_entry_us")
}

pub(crate) fn eval_bench_threshold_us() -> Result<u128, String> {
    bench_threshold_us("compiler", "eval_tests_per_entry_us")
}

pub(crate) fn enforce_per_entry_lock(
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

pub(crate) fn build_bench_threshold_us() -> Result<u128, String> {
    bench_threshold_us("build", "build_appliance_median_us")
}

pub(crate) fn median(sorted: &[Duration]) -> Duration {
    sorted[sorted.len() / 2]
}

pub(crate) fn bench_compiler() -> Result<(), String> {
    let _mode = CompileOptsGuard::mode(wrela_compiler::opts::CompileMode::Release);
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

pub(crate) fn bench_check_lane() -> Result<(), String> {
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

pub(crate) fn bench_eval_lane() -> Result<(), String> {
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

pub(crate) fn bench_build_target() -> Result<PathBuf, String> {
    let case = root().join("tests/golden/appliance");
    golden_case_target(&case)?
        .ok_or_else(|| "bench build: tests/golden/appliance has no `root`-named target".to_string())
}

pub(crate) fn run_build_bench_workload(target: &Path) -> Duration {
    let start = Instant::now();
    let _ = produce_report_and_image(target);
    start.elapsed()
}

pub(crate) fn bench_build_lane() -> Result<(), String> {
    let _mode = CompileOptsGuard::mode(wrela_compiler::opts::CompileMode::Release);
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

pub(crate) fn bench(args: &[String]) -> Result<(), String> {
    match args {
        [lane] if lane == "compiler" => bench_compiler(),
        [lane] if lane == "build" => bench_build_lane(),
        [lane] if lane == "guest" => bench_guest_lane(),
        [] => fail_closed(
            "bench",
            "bare `bench` fails closed; run `bench compiler`, `bench build`, or `bench guest` \
             (all live)",
        ),
        [other] => Err(format!(
            "bench: unknown lane `{other}` (expected `compiler`, `build`, or `guest`)"
        )),
        _ => Err(format!(
            "bench: unexpected argument(s): {}",
            args[1..].join(" ")
        )),
    }
}
