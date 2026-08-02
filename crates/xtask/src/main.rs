use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};
use std::time::Instant;

use wrela_compiler::eval;
use wrela_compiler::layout;
use wrela_compiler::loader;
use wrela_compiler::opts::{self, CompileMode};
use wrela_compiler::placement;
use wrela_compiler::report;
use wrela_compiler::sema;
use wrela_compiler::sema::typed::TestKind;
use wrela_compiler::syntax::ast::Module;
use wrela_compiler::syntax::lexer::{self};
use wrela_compiler::syntax::parser::{self, Parsed};
use wrela_compiler::syntax::printer;

mod corpus_sema_census;
mod corpus_sema_context;

mod agnostic_sweep;
mod bench;
mod corpus;
mod diff_blk;
mod fuzz;
mod golden;
mod lane2_freq;
mod stdlib_test;

use agnostic_sweep::*;
use bench::*;
use corpus::*;
use diff_blk::{blk_shape, diff_blk, fill_blk_ring, qemu_path};
use fuzz::*;
use golden::*;
use lane2_freq::*;
use stdlib_test::*;

pub(crate) fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("repo root")
        .to_path_buf()
}

pub(crate) fn golden_case_dirs(golden_dir: &Path) -> Result<Vec<PathBuf>, String> {
    let entries =
        std::fs::read_dir(golden_dir).map_err(|e| format!("read {}: {e}", golden_dir.display()))?;
    let mut dirs = Vec::new();
    for entry in entries {
        let path = entry
            .map_err(|e| format!("read {} entry: {e}", golden_dir.display()))?
            .path();
        if path.is_dir() {
            dirs.push(path);
        }
    }
    dirs.sort();
    Ok(dirs)
}

const USAGE: &str = "agent verification:\n  cargo xtask verify\n\nmaintainer commands:\n  cargo xtask verify-deep\n  cargo xtask golden [--update] [--filter <substr>] [--only-boot|--no-boot] [--jobs N] [--boot-jobs N]\n  cargo xtask corpus [--sema]\n  cargo xtask fuzz <smoke|all|lexer|parser|sema|eval|lower|async|imports|report> [--iters N] [--seed S]\n  cargo xtask roundtrip|report-determinism|agnostic-sweep|cost-inventory|stdlib-test|repro\n  cargo xtask diff-eval [--with-opt <OptId>]\n  cargo xtask diff-block-count|diff-blk|profile\n  cargo xtask gen-lane2-freq <case>\n  cargo xtask bench <compiler|build|guest>";

fn no_args(command: &str, args: &[String]) -> Result<(), String> {
    if args.len() == 1 {
        Ok(())
    } else {
        Err(format!(
            "{command}: unexpected argument(s): {}",
            args[1..].join(" ")
        ))
    }
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if matches!(args.first().map(String::as_str), Some("-h" | "--help")) {
        if args.len() != 1 {
            eprintln!("xtask: --help takes no arguments");
            return ExitCode::FAILURE;
        }
        println!("{USAGE}");
        return ExitCode::SUCCESS;
    }
    let result = match args.first().map(String::as_str) {
        Some("verify") => no_args("verify", &args).and_then(|()| verify()),
        Some("verify-deep") => no_args("verify-deep", &args).and_then(|()| verify_deep()),
        Some("verify-milestone") => Err(
            "`verify-milestone` was removed; `cargo xtask verify` is the sole required gate, and \
             `cargo xtask verify-deep` is an optional maintainer diagnostic"
                .to_string(),
        ),
        Some("check") => {
            Err("`check` was replaced by the sole required gate, `cargo xtask verify`".to_string())
        }
        Some("golden") => parse_golden_opts(&args[1..]).and_then(|opts| golden(&opts)),
        Some("corpus") => corpus(&args[1..]),
        Some("roundtrip") => no_args("roundtrip", &args).and_then(|()| roundtrip()),
        Some("report-determinism") => {
            no_args("report-determinism", &args).and_then(|()| report_determinism())
        }
        Some("agnostic-sweep") => no_args("agnostic-sweep", &args).and_then(|()| agnostic_sweep()),
        Some("cost-inventory") => no_args("cost-inventory", &args).and_then(|()| cost_inventory()),
        Some("stdlib-test") => no_args("stdlib-test", &args).and_then(|()| stdlib_test()),
        Some("repro") => no_args("repro", &args).and_then(|()| repro()),
        Some("diff-eval") => diff_eval(&args[1..]),
        Some("diff-block-count") => {
            no_args("diff-block-count", &args).and_then(|()| diff_block_count())
        }
        Some("diff-blk") => no_args("diff-blk", &args).and_then(|()| diff_blk()),
        Some("gen-lane2-freq") => match args.as_slice() {
            [_, case] => gen_lane2_freq(case),
            _ => Err("usage: cargo xtask gen-lane2-freq <golden-case>".to_string()),
        },
        Some("profile") => no_args("profile", &args).and_then(|()| profile()),
        Some("fuzz") => fuzz(&args[1..]),
        Some("bench") => bench(&args[1..]),
        Some(other) => Err(format!("unknown xtask command `{other}`\n\n{USAGE}")),
        None => Err(USAGE.to_string()),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(msg) => {
            eprintln!("xtask: {msg}");
            ExitCode::FAILURE
        }
    }
}

fn deep_lane() -> Result<(), String> {
    run(
        Command::new("cargo").args([
            "test",
            "--workspace",
            "--exclude",
            "wrela-vmm",
            "--quiet",
            "--",
            "--ignored",
            "--test-threads=1",
        ]),
        "cargo test -- --ignored --test-threads=1 (deep lane)",
    )
}

fn assert_unit_suite_within_budget(elapsed: std::time::Duration) -> Result<(), String> {
    let budget_us = bench_threshold_us("tests", "workspace_suite_max_us")?;
    let measured_us = elapsed.as_micros();
    if measured_us > budget_us {
        return Err(format!(
            "cargo test: the default unit lane took {:.1}s, over its locked budget of {:.1}s \
             (bench/thresholds.toml `[tests] workspace_suite_max_us`).\n\
             \n\
             This is a *placement* failure, not a speed one. Broad whole-corpus and whole-sweep \
             proofs belong behind `#[ignore]` in `verify-deep`; keep focused smoke coverage \
             in `verify`. Find the long pole with:\n\
             \n    cargo test -p wrela-compiler --lib -- --test-threads=1\n\n\
             — libtest prints each test as it finishes, making long gaps visible.\n\
             \n\
             Re-lock this number only deliberately, in its own commit, citing why — never to \
             make a regression quietly pass.",
            measured_us as f64 / 1e6,
            budget_us as f64 / 1e6,
        ));
    }
    println!(
        "cargo test: default unit lane {:.1}s, within locked budget {:.1}s (bench/thresholds.toml)",
        measured_us as f64 / 1e6,
        budget_us as f64 / 1e6,
    );
    Ok(())
}

fn parse_golden_opts(args: &[String]) -> Result<GoldenOpts, String> {
    fn positive(name: &str, value: &str) -> Result<usize, String> {
        let n = value
            .parse::<usize>()
            .map_err(|e| format!("golden: {name}: {e}"))?;
        if n == 0 {
            return Err(format!("golden: {name} must be at least 1"));
        }
        Ok(n)
    }

    let mut opts = GoldenOpts::default();
    let mut seen_update = false;
    let mut seen_boot = false;
    let mut seen_filter = false;
    let mut seen_jobs = false;
    let mut seen_boot_jobs = false;
    let mut i = 0usize;
    while i < args.len() {
        let arg = args[i].as_str();
        let next = |i: &mut usize, name: &str| -> Result<&str, String> {
            *i += 1;
            args.get(*i)
                .map(String::as_str)
                .ok_or_else(|| format!("golden: `{name}` needs a value"))
        };
        match arg {
            "--update" if !seen_update => {
                seen_update = true;
                opts.update = true;
            }
            "--no-boot" if !seen_boot => {
                seen_boot = true;
                opts.boot = BootSel::None;
            }
            "--only-boot" if !seen_boot => {
                seen_boot = true;
                opts.boot = BootSel::Only;
            }
            "--filter" if !seen_filter => {
                seen_filter = true;
                opts.filter = Some(next(&mut i, "--filter")?.to_string());
            }
            "--jobs" if !seen_jobs => {
                seen_jobs = true;
                opts.jobs = positive("--jobs", next(&mut i, "--jobs")?)?;
            }
            "--boot-jobs" if !seen_boot_jobs => {
                seen_boot_jobs = true;
                opts.boot_jobs = positive("--boot-jobs", next(&mut i, "--boot-jobs")?)?;
            }
            other if other.starts_with("--filter=") && !seen_filter => {
                seen_filter = true;
                opts.filter = Some(other["--filter=".len()..].to_string());
            }
            "--update" | "--no-boot" | "--only-boot" | "--filter" | "--jobs" | "--boot-jobs" => {
                return Err(format!("golden: duplicate or conflicting flag `{arg}`"));
            }
            other => return Err(format!("golden: unknown flag `{other}`")),
        }
        i += 1;
    }
    Ok(opts)
}

fn cost_inventory() -> Result<(), String> {
    println!("{}", wrela_compiler::cost::check_dimension_inventory()?);
    Ok(())
}

pub(crate) fn fail_closed<T>(name: &str, why: &str) -> Result<T, String> {
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

pub(crate) struct CompileOptsGuard {
    saved: Vec<opts::OptId>,
}

impl CompileOptsGuard {
    pub(crate) fn mode(mode: CompileMode) -> Self {
        let saved = opts::active_opts();
        opts::apply_mode(mode);
        Self { saved }
    }
}

impl Drop for CompileOptsGuard {
    fn drop(&mut self) {
        opts::apply_opts(&self.saved);
    }
}

fn verify_stage(
    lane: &str,
    name: &str,
    rerun: &str,
    f: impl FnOnce() -> Result<(), String>,
) -> Result<(), String> {
    println!("{lane}: START {name}");
    let started = Instant::now();
    match f() {
        Ok(()) => {
            println!(
                "{lane}: PASS {name} ({:.2}s)",
                started.elapsed().as_secs_f64()
            );
            Ok(())
        }
        Err(e) => Err(format!("{lane}: FAIL {name}\nrerun: {rerun}\n{e}")),
    }
}

fn test_wrela_vmm_portable() -> Result<(), String> {
    fn listed(args: &[&str]) -> Result<usize, String> {
        let out = Command::new("cargo")
            .current_dir(root())
            .args(args)
            .output()
            .map_err(|e| format!("list wrela-vmm tests: {e}"))?;
        if !out.status.success() {
            return Err(format!(
                "list wrela-vmm tests failed:\n{}",
                String::from_utf8_lossy(&out.stderr)
            ));
        }
        Ok(String::from_utf8_lossy(&out.stdout)
            .lines()
            .filter(|line| line.ends_with(": test"))
            .count())
    }

    const ALL: usize = 122;
    let all = listed(&["test", "-q", "-p", "wrela-vmm", "--lib", "--", "--list"])?;
    let hvf = listed(&[
        "test",
        "-q",
        "-p",
        "wrela-vmm",
        "--lib",
        "--",
        "--ignored",
        "--list",
    ])?;
    if all != ALL || hvf != VMM_HVF_TESTS {
        return Err(format!(
            "wrela-vmm test census changed: total={all} (expected {ALL}), HVF={hvf} \
             (expected {VMM_HVF_TESTS}); classify new tests deliberately"
        ));
    }
    run(
        Command::new("cargo").args(["test", "-p", "wrela-vmm", "--lib", "--quiet"]),
        "portable wrela-vmm tests",
    )
}

fn unit_lane() -> Result<(), String> {
    run(
        Command::new("cargo").args([
            "test",
            "--workspace",
            "--exclude",
            "wrela-vmm",
            "--no-run",
            "--quiet",
        ]),
        "cargo test --no-run",
    )?;
    let unit_start = Instant::now();
    run(
        Command::new("cargo").args(["test", "--workspace", "--exclude", "wrela-vmm", "--quiet"]),
        "cargo test",
    )?;
    assert_unit_suite_within_budget(unit_start.elapsed())
}

fn verify_preflight() -> Result<(), String> {
    if !(cfg!(target_os = "macos") && cfg!(target_arch = "aarch64")) {
        return fail_closed(
            "verify",
            "requires macOS/aarch64 for its focused Hypervisor.framework coverage",
        );
    }
    let entitlements = root().join("crates/wrela-vmm/entitlements.plist");
    for path in [Path::new("/usr/bin/codesign"), entitlements.as_path()] {
        if !path.is_file() {
            return fail_closed(
                "verify",
                &format!("required tool/input `{}` is missing", path.display()),
            );
        }
    }
    Ok(())
}

fn deep_preflight() -> Result<(), String> {
    verify_preflight()?;
    qemu_path()?;
    Ok(())
}

fn verify() -> Result<(), String> {
    const LANE: &str = "verify";
    verify_stage(
        LANE,
        "host preflight",
        "cargo xtask verify",
        verify_preflight,
    )?;
    verify_stage(LANE, "format", "cargo fmt --all --check", || {
        run(
            Command::new("cargo").args(["fmt", "--all", "--check"]),
            "cargo fmt --check",
        )
    })?;
    verify_stage(
        LANE,
        "cost inventory",
        "cargo xtask cost-inventory",
        cost_inventory,
    )?;
    verify_stage(
        LANE,
        "tracked-tree sweep",
        "cargo xtask agnostic-sweep",
        agnostic_sweep,
    )?;
    verify_stage(
        LANE,
        "workspace units",
        "cargo test --workspace --exclude wrela-vmm",
        unit_lane,
    )?;
    verify_stage(
        LANE,
        "portable VMM units",
        "cargo test -p wrela-vmm --lib",
        test_wrela_vmm_portable,
    )?;
    verify_stage(
        LANE,
        "signed HVF smoke",
        "cargo xtask verify",
        test_wrela_vmm_hvf_signed_smoke,
    )?;
    verify_stage(
        LANE,
        "boot goldens",
        "cargo xtask golden --only-boot",
        || {
            golden(&GoldenOpts {
                boot: BootSel::Only,
                ..GoldenOpts::default()
            })
        },
    )?;
    verify_stage(
        LANE,
        "static goldens",
        "cargo xtask golden --no-boot",
        || {
            golden(&GoldenOpts {
                boot: BootSel::None,
                ..GoldenOpts::default()
            })
        },
    )?;
    verify_stage(LANE, "documentation corpus", "cargo xtask corpus", || {
        corpus(&[])
    })?;
    verify_stage(LANE, "roundtrip", "cargo xtask roundtrip", roundtrip)?;
    verify_stage(LANE, "stdlib", "cargo xtask stdlib-test", stdlib_test)?;
    println!("verify: ok");
    Ok(())
}

fn verify_deep() -> Result<(), String> {
    const LANE: &str = "verify-deep";
    verify_stage(
        LANE,
        "host preflight",
        "cargo xtask verify-deep",
        deep_preflight,
    )?;
    verify_stage(
        LANE,
        "report determinism",
        "cargo xtask report-determinism",
        report_determinism,
    )?;
    verify_stage(
        LANE,
        "deep optimizer proofs",
        "cargo test --workspace --exclude wrela-vmm -- --ignored --test-threads=1",
        deep_lane,
    )?;
    verify_stage(
        LANE,
        "signed HVF VMM tests",
        "cargo xtask verify-deep",
        test_wrela_vmm_hvf_signed,
    )?;
    verify_stage(
        LANE,
        "boot goldens",
        "cargo xtask golden --only-boot",
        || {
            golden(&GoldenOpts {
                boot: BootSel::Only,
                ..GoldenOpts::default()
            })
        },
    )?;
    verify_stage(
        LANE,
        "full differential eval",
        "cargo xtask diff-eval",
        || diff_eval(&[]),
    )?;
    verify_stage(
        LANE,
        "hardware reproducibility",
        "cargo xtask repro",
        repro_hardware,
    )?;
    verify_stage(LANE, "QEMU block oracle", "cargo xtask diff-blk", diff_blk)?;
    verify_stage(
        LANE,
        "compiler benchmarks",
        "cargo xtask bench compiler",
        bench_compiler,
    )?;
    verify_stage(
        LANE,
        "build benchmark",
        "cargo xtask bench build",
        bench_build_lane,
    )?;
    verify_stage(
        LANE,
        "guest benchmark",
        "cargo xtask bench guest",
        bench_guest_lane,
    )?;
    println!("verify-deep: ok");
    Ok(())
}

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

fn report_determinism() -> Result<(), String> {
    let _mode = CompileOptsGuard::mode(CompileMode::Release);
    let golden_dir = root().join("tests/golden");
    let targets: Vec<PathBuf> = golden_case_dirs(&golden_dir)?
        .into_iter()
        .filter(|c| c.join("expected/report.txt").exists())
        .collect();

    let cursor = std::sync::atomic::AtomicUsize::new(0);
    let collected: std::sync::Mutex<Vec<(usize, String, Vec<String>)>> =
        std::sync::Mutex::new(Vec::new());
    let hard_error: std::sync::Mutex<Option<String>> = std::sync::Mutex::new(None);
    let jobs = golden::default_jobs().min(targets.len().max(1));

    std::thread::scope(|scope| {
        for _ in 0..jobs {
            scope.spawn(|| {
                loop {
                    let i = cursor.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    if i >= targets.len() {
                        return;
                    }
                    let case = &targets[i];
                    let mut failures = Vec::new();
                    let target = match golden_case_target(case) {
                        Ok(Some(t)) if t.exists() => t,
                        Ok(_) => {
                            failures.push(format!(
                                "{}: expected/report.txt exists but no input.wr/`root` target found",
                                case.display()
                            ));
                            collected.lock().expect("lock").push((i, String::new(), failures));
                            continue;
                        }
                        Err(e) => {
                            let mut slot = hard_error.lock().expect("lock");
                            if slot.is_none() {
                                *slot = Some(e);
                            }
                            continue;
                        }
                    };
                    let first = produce_report_and_image(&target);
                    let second = produce_report_and_image(&target);
                    match (first, second) {
                        (Ok(a), Ok(b)) => {
                            collected
                                .lock()
                                .expect("lock")
                                .push((i, case.display().to_string(), compare_two_runs(case, a, b)));
                        }
                        (Err(e), _) | (_, Err(e)) => {
                            let mut slot = hard_error.lock().expect("lock");
                            if slot.is_none() {
                                *slot = Some(e);
                            }
                        }
                    }
                }
            });
        }
    });

    if let Some(e) = hard_error.into_inner().expect("lock") {
        return Err(e);
    }
    let mut collected = collected.into_inner().expect("lock");
    collected.sort_by_key(|(i, _, _)| *i);
    let cases = collected.len();
    let failures: Vec<String> = collected.into_iter().flat_map(|(_, _, f)| f).collect();

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

fn compare_two_runs(
    case: &Path,
    (first_text, first_img): (String, Option<Vec<u8>>),
    (second_text, second_img): (String, Option<Vec<u8>>),
) -> Vec<String> {
    let mut failures = Vec::new();
    {
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
    failures
}

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

fn repro_choice_log_roundtrip(vmm: &Path) -> Result<(), String> {
    let (img_bytes, report_text) = boot_hello_test_image()?;
    let (tmp_dir, img_path, report_path, record_path) =
        stage_repro_dir("target/repro-choice-log-tmp", &img_bytes, &report_text)?;

    let (_, record_exit) = run_vmm_mode(
        vmm,
        &report_path,
        &img_path,
        "--record",
        &record_path,
        "--record",
    )?;
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

    let (replay_out, replay_exit) = run_vmm_mode(
        vmm,
        &report_path,
        &img_path,
        "--replay",
        &record_path,
        "--replay",
    )?;
    let _ = std::fs::remove_dir_all(&tmp_dir);
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

fn repro_replay_exit_code_contract(vmm: &Path) -> Result<(), String> {
    use wrela_machine::vmm_process::{EXIT_REPLAY_DIVERGENCE, EXIT_VMM_FAILURE};

    let (img_bytes, report_text) = boot_hello_test_image()?;
    let (tmp_dir, img_path, report_path, record_path) = stage_repro_dir(
        "target/repro-exit-code-contract-tmp",
        &img_bytes,
        &report_text,
    )?;

    let fail = |tmp_dir: &Path, msg: String| -> Result<(), String> {
        let _ = std::fs::remove_dir_all(tmp_dir);
        Err(msg)
    };

    let (_, record_exit) = run_vmm_mode(
        vmm,
        &report_path,
        &img_path,
        "--record",
        &record_path,
        "--record",
    )?;
    if record_exit != 0 && record_exit != 1 {
        return fail(
            &tmp_dir,
            format!(
                "repro: exit-code-contract record boot did not complete (exit {record_exit}, \
                 expected the guest-authored 0 or 1)"
            ),
        );
    }

    let (_, clean_exit) = run_vmm_mode(
        vmm,
        &report_path,
        &img_path,
        "--replay",
        &record_path,
        "--replay",
    )?;
    if clean_exit != record_exit {
        return fail(
            &tmp_dir,
            format!(
                "repro: exit-code-contract clean replay exit ({clean_exit}) does not match the \
                 recorded boot's own guest-authored exit ({record_exit})"
            ),
        );
    }

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
    let (diverged_replay, diverged_exit) = run_vmm_mode(
        vmm,
        &report_path,
        &img_path,
        "--replay",
        &tampered_path,
        "--replay (tampered)",
    )?;
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

    let malformed_path = tmp_dir.join("malformed.record.txt");
    std::fs::write(&malformed_path, b"not a choice log at all\n")
        .map_err(|e| format!("write malformed record: {e}"))?;
    let (_, malformed_exit) = run_vmm_mode(
        vmm,
        &report_path,
        &img_path,
        "--replay",
        &malformed_path,
        "--replay (malformed)",
    )?;
    if malformed_exit != EXIT_VMM_FAILURE {
        return fail(
            &tmp_dir,
            format!(
                "repro: a malformed --replay record file must exit {EXIT_VMM_FAILURE} \
                 (EXIT_VMM_FAILURE), got {malformed_exit}"
            ),
        );
    }

    let unwritable_path = tmp_dir.join("no-such-subdir").join("rec.txt");
    let (_, unwritable_exit) = run_vmm_mode(
        vmm,
        &report_path,
        &img_path,
        "--record",
        &unwritable_path,
        "--record (unwritable)",
    )?;
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

fn repro_deadline_cancel_replay_is_clock_log_driven(vmm: &Path) -> Result<(), String> {
    use wrela_machine::vmm_process::EXIT_REPLAY_DIVERGENCE;
    let (img_bytes, report_text) = golden_test_image("boot-deadline-cancel")?;
    let (tmp_dir, img_path, report_path, record_path) =
        stage_repro_dir("target/repro-deadline-cancel-tmp", &img_bytes, &report_text)?;

    let (record_out, record_exit) = run_vmm_mode(
        vmm,
        &report_path,
        &img_path,
        "--record",
        &record_path,
        "--record",
    )?;
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

    let (replay_out, replay_exit) = run_vmm_mode(
        vmm,
        &report_path,
        &img_path,
        "--replay",
        &record_path,
        "--replay",
    )?;
    if replay_exit != record_exit {
        let _ = std::fs::remove_dir_all(&tmp_dir);
        return Err(format!(
            "repro: boot-deadline-cancel replayed with exit {replay_exit}, expected \
             {record_exit}:\n{}",
            String::from_utf8_lossy(&replay_out.stderr)
        ));
    }

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
    let (tampered_out, tampered_exit) = run_vmm_mode(
        vmm,
        &report_path,
        &img_path,
        "--replay",
        &tampered_path,
        "--replay (tampered)",
    )?;
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

fn repro_entropy_replay_is_choice_log_driven(vmm: &Path) -> Result<(), String> {
    use wrela_machine::vmm_process::EXIT_REPLAY_DIVERGENCE;
    let (img_bytes, report_text) = golden_test_image("boot-entropy-hex")?;
    let (tmp_dir, img_path, report_path, record_path) =
        stage_repro_dir("target/repro-entropy-tmp", &img_bytes, &report_text)?;

    let (record_out, record_exit) = run_vmm_mode(
        vmm,
        &report_path,
        &img_path,
        "--record",
        &record_path,
        "--record",
    )?;
    if record_exit != 0 {
        let _ = std::fs::remove_dir_all(&tmp_dir);
        return Err(format!(
            "repro: boot-entropy-hex's own recording boot did not pass (exit {record_exit}):\n{}",
            String::from_utf8_lossy(&record_out.stdout)
        ));
    }
    let record_text = std::fs::read_to_string(&record_path)
        .map_err(|e| format!("read {}: {e}", record_path.display()))?;
    let entropy_reads = record_text.matches("=EntropyRead ").count();
    if entropy_reads < 1 {
        let _ = std::fs::remove_dir_all(&tmp_dir);
        return Err(format!(
            "repro: boot-entropy-hex recorded {entropy_reads} EntropyRead choice(s) — this boot \
             is supposed to issue entropy[8]() (one park-shaped fill → one EntropyRead)"
        ));
    }

    let (replay_out, replay_exit) = run_vmm_mode(
        vmm,
        &report_path,
        &img_path,
        "--replay",
        &record_path,
        "--replay",
    )?;
    if replay_exit != record_exit {
        let _ = std::fs::remove_dir_all(&tmp_dir);
        return Err(format!(
            "repro: boot-entropy-hex replayed with exit {replay_exit}, expected \
             {record_exit}:\n{}",
            String::from_utf8_lossy(&replay_out.stderr)
        ));
    }

    let mut tampered = String::new();
    let mut done = false;
    for line in record_text.lines() {
        if !done && line.contains("=EntropyRead ") && line.contains(" hex=") {
            let Some(hex_at) = line.find(" hex=") else {
                tampered.push_str(line);
                tampered.push('\n');
                continue;
            };
            let head = &line[..hex_at + " hex=".len()];
            let old_hex = &line[hex_at + " hex=".len()..];
            if old_hex.is_empty() || old_hex.len() % 2 != 0 {
                let _ = std::fs::remove_dir_all(&tmp_dir);
                return Err(format!(
                    "repro: EntropyRead hex field looks malformed: {old_hex:?}"
                ));
            }
            let mut new_hex = String::with_capacity(old_hex.len());
            for c in old_hex.chars() {
                let n = match c {
                    '0'..='9' => c as u8 - b'0',
                    'a'..='f' => c as u8 - b'a' + 10,
                    _ => {
                        let _ = std::fs::remove_dir_all(&tmp_dir);
                        return Err(format!(
                            "repro: non-lowercase-hex digit {c:?} in EntropyRead hex"
                        ));
                    }
                };
                let flipped = n ^ 1;
                new_hex.push(char::from(if flipped < 10 {
                    b'0' + flipped
                } else {
                    b'a' + (flipped - 10)
                }));
            }
            if new_hex == old_hex {
                let _ = std::fs::remove_dir_all(&tmp_dir);
                return Err("repro: EntropyRead hex tamper produced an identical value".into());
            }
            tampered.push_str(head);
            tampered.push_str(&new_hex);
            tampered.push('\n');
            done = true;
            continue;
        }
        tampered.push_str(line);
        tampered.push('\n');
    }
    if !done {
        let _ = std::fs::remove_dir_all(&tmp_dir);
        return Err("repro: no EntropyRead line to tamper in the recording".to_string());
    }
    let tampered_path = tmp_dir.join("boot.tampered.txt");
    std::fs::write(&tampered_path, &tampered).map_err(|e| format!("write tampered: {e}"))?;
    let (tampered_out, tampered_exit) = run_vmm_mode(
        vmm,
        &report_path,
        &img_path,
        "--replay",
        &tampered_path,
        "--replay (tampered)",
    )?;
    let _ = std::fs::remove_dir_all(&tmp_dir);
    if tampered_exit != EXIT_REPLAY_DIVERGENCE {
        return Err(format!(
            "repro: a replay whose first logged EntropyRead hex was flipped exited \
             {tampered_exit}, expected {EXIT_REPLAY_DIVERGENCE} — the replayed guest's own \
             entropy fill must come from the log, so this tamper has to change its \
             transcript and diverge\nstderr:\n{}",
            String::from_utf8_lossy(&tampered_out.stderr)
        ));
    }
    println!(
        "repro: tests/golden/boot-entropy-hex ({entropy_reads} recorded EntropyRead choice(s)) \
         replays clean with zero divergence, and a tampered entropy hex diverges \
         (exit {EXIT_REPLAY_DIVERGENCE}) — replay's entropy comes from the log"
    );
    Ok(())
}

fn repro() -> Result<(), String> {
    report_determinism()?;
    repro_hardware()
}

fn repro_hardware() -> Result<(), String> {
    let _mode = CompileOptsGuard::mode(CompileMode::Release);
    repro_test_image()?;
    let vmm = build_and_sign_vmm()?;
    repro_choice_log_roundtrip(&vmm)?;
    repro_deadline_cancel_replay_is_clock_log_driven(&vmm)?;
    repro_entropy_replay_is_choice_log_driven(&vmm)?;
    repro_blk_completion_replay(&vmm)?;
    repro_cross_core_admission_replay(&vmm)?;
    repro_cross_core_mailbox_depth_admissions(&vmm)?;
    repro_lane1_trailer_repeats(&vmm)?;
    repro_replay_exit_code_contract(&vmm)
}

fn repro_lane1_trailer_repeats(vmm: &Path) -> Result<(), String> {
    const REPEATS: usize = 5;
    const CASES: [&str; 2] = [
        "boot-cross-core-ring-full",
        "boot-cross-core-admission-order",
    ];
    for case in CASES {
        let (img_bytes, report_text) = golden_test_image(case)?;
        let (tmp_dir, img_path, report_path, _record) =
            stage_repro_dir("target/repro-lane1-tmp", &img_bytes, &report_text)?;
        let mut first: Option<String> = None;
        for run in 0..REPEATS {
            let boot = run_vmm(vmm, &report_path, &img_path)?;
            let trailer: String = boot
                .transcript
                .lines()
                .filter(|l| l.starts_with("lane1 "))
                .map(|l| format!("{l}\n"))
                .collect();
            if trailer.is_empty() {
                let _ = std::fs::remove_dir_all(&tmp_dir);
                return Err(format!(
                    "repro: {case} boot {run} printed no `lane1 …` trailer at all:\n{}",
                    boot.transcript
                ));
            }
            if trailer.contains("quiesce=timeout") {
                let _ = std::fs::remove_dir_all(&tmp_dir);
                return Err(format!(
                    "repro: {case} boot {run} timed out waiting for the released cores to park \
                     (`lane1 quiesce=timeout`), so its Lane 1 totals are a mid-flight sample — \
                     raise `QUIESCE_POLL_BOUND` in stdlib/core/runtime.wr or find out which core \
                     never parked"
                ));
            }
            match &first {
                None => first = Some(trailer),
                Some(want) if *want == trailer => {}
                Some(want) => {
                    let _ = std::fs::remove_dir_all(&tmp_dir);
                    return Err(format!(
                        "repro: {case}'s Lane 1 trailer is not reproducible — boot 0 printed\n\
                         {want}but boot {run} printed\n{trailer}"
                    ));
                }
            }
        }
        let _ = std::fs::remove_dir_all(&tmp_dir);
    }
    println!(
        "repro: the Lane 1 trailer of {} reproduced byte-for-byte across {REPEATS} boots each \
         (per-core counters summed at a quiesced halt)",
        CASES.join(" and ")
    );
    Ok(())
}

fn repro_cross_core_mailbox_depth_admissions(vmm: &Path) -> Result<(), String> {
    const CASE: &str = "boot-cross-core-mailbox-depth";
    let (img_bytes, report_text) = golden_test_image(CASE)?;
    let (tmp_dir, img_path, report_path, record_path) =
        stage_repro_dir("target/repro-mailbox-depth-tmp", &img_bytes, &report_text)?;

    let (record_out, record_exit) = run_vmm_mode(
        vmm,
        &report_path,
        &img_path,
        "--record",
        &record_path,
        "--record",
    )?;
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
    let has = |s: &str| admissions.iter().any(|a| *a == s);
    if !has("Admission mailbox=Far sender=core0") {
        let _ = std::fs::remove_dir_all(&tmp_dir);
        return Err(format!(
            "repro: {CASE} recorded {:?}, need Far←core0 (kick); guest total==11 \
             locks Far's +10 — cap-1 under-count may drop every Sink←* observe",
            admissions
        ));
    }
    let (replay_out, replay_exit) = run_vmm_mode(
        vmm,
        &report_path,
        &img_path,
        "--replay",
        &record_path,
        "--replay",
    )?;
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

fn repro_cross_core_admission_replay(vmm: &Path) -> Result<(), String> {
    use wrela_machine::vmm_process::EXIT_REPLAY_DIVERGENCE;
    const CASE: &str = "boot-cross-core-admission-order";
    let (img_bytes, report_text) = golden_test_image(CASE)?;
    let (tmp_dir, img_path, report_path, record_path) =
        stage_repro_dir("target/repro-admission-tmp", &img_bytes, &report_text)?;

    let (record_out, record_exit) = run_vmm_mode(
        vmm,
        &report_path,
        &img_path,
        "--record",
        &record_path,
        "--record",
    )?;
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

    let (replay_out, replay_exit) = run_vmm_mode(
        vmm,
        &report_path,
        &img_path,
        "--replay",
        &record_path,
        "--replay",
    )?;
    if replay_exit != record_exit {
        let _ = std::fs::remove_dir_all(&tmp_dir);
        return Err(format!(
            "repro: {CASE} replayed with exit {replay_exit}, expected {record_exit}:\n{}",
            String::from_utf8_lossy(&replay_out.stderr)
        ));
    }

    struct Tamper {
        name: &'static str,
        expect: &'static str,
        apply: fn(&str) -> String,
    }
    let tampers: &[Tamper] = &[
        Tamper {
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
            name: "drop unique Far admission",
            expect: "admission mismatch",
            apply: |text| {
                let mut choice_lines: Vec<String> = Vec::new();
                let mut trailer: Vec<String> = Vec::new();
                for line in text.lines() {
                    if line.starts_with("choice[") {
                        choice_lines.push(line.to_string());
                    } else if line.starts_with("ChoiceLog") || line.starts_with("choice_count=") {
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
                non_admission.push("choice[N]=Admission mailbox=Spurious sender=core0".to_string());
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

    let (record_out, record_exit) = run_vmm_mode(
        vmm,
        &report_path,
        &img_path,
        "--record",
        &record_path,
        "--record",
    )?;
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

    let (replay_out, replay_exit) = run_vmm_mode(
        vmm,
        &report_path,
        &img_path,
        "--replay",
        &record_path,
        "--replay",
    )?;
    if replay_exit != record_exit {
        let _ = std::fs::remove_dir_all(&tmp_dir);
        return Err(format!(
            "repro: the blk workload's replay diverged from its own recording (exit {replay_exit}, \
             expected {record_exit}):\n{}",
            String::from_utf8_lossy(&replay_out.stderr)
        ));
    }

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
    let (tampered_out, tampered_exit) = run_vmm_mode(
        vmm,
        &report_path,
        &img_path,
        "--replay",
        &tampered_path,
        "--replay (tampered)",
    )?;
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

fn blk_conformance_image() -> (Vec<u8>, String) {
    use wrela_compiler::encode;
    use wrela_machine::{layout as machine_layout, machine_info, mmio, pending};

    use blk_shape::*;
    const DEVICE_FEATURES: u64 = (1 << 32) | (1 << 9);
    const BLK_VECTOR: u64 = 1;
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
        w.push(encode::enc_add_imm(31, 9, 0, true));

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
            w.extend(load_imm(12, 20_000_000));
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

        w.push(encode::enc_movz(1, 0, 0, true));
        let check = |w: &mut Vec<u32>, actual: u8, expect: u64, bit: u8| {
            w.extend(load_imm(13, expect));
            w.push(encode::enc_cmp_reg(actual, 13, true));
            w.push(encode::enc_cset(14, encode::Cond::Ne, true));
            if bit > 0 {
                w.push(encode::enc_lsl_imm(14, 14, bit, true));
            }
            w.push(encode::enc_orr_reg(1, 1, 14, true));
        };
        check(&mut w, 19, 2u64 << 16, 0);
        check(&mut w, 20, 1 | (3u64 << 32), 1);
        check(&mut w, 21, 513, 2);
        check(&mut w, 22, 0, 3);
        check(&mut w, 23, 0, 4);
        check(&mut w, 24, expect_first, 5);
        check(&mut w, 25, expect_last, 6);
        check(&mut w, 26, 1u64 << BLK_VECTOR, 7);

        w.extend(load_imm(15, mmio::EXIT_MMIO_ADDR));
        w.push(encode::enc_str_x_imm(1, 15, 0));
        w.push(encode::enc_brk(0));
        w
    };

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

#[derive(Default)]
struct DiffEvalTally {
    agree: usize,
    cases_agreed: usize,
    lowering_skips: usize,
    exhaustive_skips: usize,
    quota_skips: usize,
    import_skips: usize,
}

struct DiffEvalChecked {
    root_program: sema::typed::TypedProgram,
    modules: BTreeMap<String, Module>,
    programs: BTreeMap<String, sema::typed::TypedProgram>,
}

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

pub(crate) fn build_runtime_test_image(
    program: &sema::typed::TypedProgram,
    modules: &BTreeMap<String, Module>,
    programs: &BTreeMap<String, sema::typed::TypedProgram>,
    source: &str,
    path: &str,
    test_names: &[String],
) -> Result<(Vec<u8>, String), String> {
    let mut layout_ctx = layout::merge_layout_ctx(modules).map_err(|e| e.message)?;
    layout::enrich_layout_ctx_with_instantiations(&mut layout_ctx, programs);
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
        true,
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
    layout::append_vmm_runtime_lines(&mut report_text, &image_layout);
    Ok((image_layout.blob, report_text))
}

struct VmmBoot {
    transcript: String,
    exit_code_class: i32,
}

fn stage_repro_dir(
    dir_name: &str,
    img_bytes: &[u8],
    report_text: &str,
) -> Result<(PathBuf, PathBuf, PathBuf, PathBuf), String> {
    let tmp_dir = root().join(dir_name);
    if tmp_dir.exists() {
        std::fs::remove_dir_all(&tmp_dir)
            .map_err(|e| format!("remove {}: {e}", tmp_dir.display()))?;
    }
    std::fs::create_dir_all(&tmp_dir).map_err(|e| format!("create {}: {e}", tmp_dir.display()))?;
    let img_path = tmp_dir.join("boot.img");
    let report_path = tmp_dir.join("boot.report.txt");
    let record_path = tmp_dir.join("boot.record.txt");
    std::fs::write(&img_path, img_bytes).map_err(|e| format!("write img: {e}"))?;
    std::fs::write(&report_path, report_text).map_err(|e| format!("write report: {e}"))?;
    Ok((tmp_dir, img_path, report_path, record_path))
}

fn run_vmm_mode(
    vmm: &Path,
    report_path: &Path,
    img_path: &Path,
    mode: &str,
    mode_path: &Path,
    what: &str,
) -> Result<(std::process::Output, i32), String> {
    let out = Command::new(vmm)
        .arg(report_path)
        .arg(img_path)
        .arg(mode)
        .arg(mode_path)
        .output()
        .map_err(|e| format!("run wrela-vmm {what}: {e}"))?;
    let code = out.status.code().unwrap_or(-1);
    Ok((out, code))
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

fn diff_eval_over_cases(
    vmm: &Path,
    filter: Option<&[&str]>,
    extra_opts: &[opts::OptId],
) -> Result<DiffEvalTally, String> {
    let saved = opts::active_opts();
    let _restore = CompileOptsGuard { saved };
    if extra_opts.is_empty() {
        opts::apply_mode(CompileMode::Release);
    } else {
        let mut list = opts::RELEASE_OPTS.to_vec();
        list.extend_from_slice(extra_opts);
        opts::apply_opts(&list);
        println!("diff-eval: opt list = RELEASE_OPTS + {extra_opts:?}");
    }
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
        if golden_case_is_borrowed(&case)? {
            println!(
                "diff-eval: case {name}: borrows its program — covered by the case that owns it"
            );
            continue;
        }
        let source = std::fs::read_to_string(&target)
            .map_err(|e| format!("read {}: {e}", target.display()))?;
        let path_display = target.display().to_string();
        let Some(checked) = typecheck_for_diff_eval(&target) else {
            continue;
        };
        let program = &checked.root_program;
        if program.tests.is_empty() {
            continue;
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

        let (eval_report, _) = eval::run_tests(program);
        let eval_line_for = |test_name: &str| -> Option<&str> {
            let prefix = format!("test {test_name}: ");
            eval_report.lines().find(|l| l.starts_with(&prefix))
        };

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
            continue;
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
        let n = backend_names.len();
        let well_formed = t_lines.len() > n
            && t_lines[n].ends_with(" failed")
            && t_lines[n + 1..]
                .iter()
                .all(|l| l.starts_with("lane1 ") || l.starts_with("lane2 "));
        if !well_formed {
            return Err(format!(
                "diff-eval: case {name}: guest transcript is not well-formed (expected {n} test \
                 line(s), a summary, then only `lane1 `/`lane2 ` counter lines; got {} line(s)):\n{}",
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

const DIFF_EVAL_MIN_AGREE: usize = 100;

fn diff_eval(args: &[String]) -> Result<(), String> {
    let mut extra: Vec<opts::OptId> = Vec::new();
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--with-opt" => {
                let name = args.get(i + 1).ok_or_else(|| {
                    "usage: cargo xtask diff-eval [--with-opt <OptId>]".to_string()
                })?;
                let id = opts::opt_by_name(name).ok_or_else(|| {
                    format!(
                        "diff-eval: --with-opt {name}: no such opt. Known ids: {}",
                        opts::all_opts()
                            .iter()
                            .map(|o| format!("{o:?}"))
                            .collect::<Vec<_>>()
                            .join(" ")
                    )
                })?;
                extra.push(id);
                i += 2;
            }
            other => return Err(format!("diff-eval: unknown argument {other:?}")),
        }
    }
    let vmm = build_and_sign_vmm()?;
    let tally = diff_eval_over_cases(&vmm, None, &extra)?;
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
    if tally.agree < DIFF_EVAL_MIN_AGREE {
        return Err(format!(
            "diff-eval: reach collapsed — {} test(s) agreed, floor is {DIFF_EVAL_MIN_AGREE} \
             (DIFF_EVAL_MIN_AGREE). Every comparison was skipped, so this lane proved nothing; \
             the skip lines above name why.",
            tally.agree
        ));
    }
    Ok(())
}

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
            (compare_dumps(name, &dump1, &dump2, &pretty), None)
        }
        Err(_) => (RoundtripResult::Skipped, None),
    }
}

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

const DIFF_BLOCK_COUNT_TEST: &str = "tests::block_count_lane2_agrees_with_host_dram_on_boot_actors";

fn diff_block_count() -> Result<(), String> {
    if !(cfg!(target_os = "macos") && cfg!(target_arch = "aarch64")) {
        return fail_closed(
            "diff-block-count",
            "needs Hypervisor.framework (macOS/aarch64); refuse to fake a pass on other hosts",
        );
    }

    run(
        Command::new("cargo").args([
            "test",
            "-q",
            "-p",
            "wrela-vmm",
            "--lib",
            "lane3::",
            "--",
            "--test-threads=1",
        ]),
        "diff-block-count: lane3 parse/agree units",
    )?;

    let output = Command::new("cargo")
        .current_dir(root())
        .args(["test", "-p", "wrela-vmm", "--lib", "--no-run"])
        .output()
        .map_err(|e| format!("cargo test -p wrela-vmm --no-run: {e}"))?;
    if !output.status.success() {
        return Err(format!(
            "diff-block-count: cargo test --no-run failed:\n{}",
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
            "diff-block-count: cargo test --no-run found no test executable(s) to sign".to_string(),
        );
    }
    let mut ran = 0usize;
    for exe in &executables {
        let mut cmd = Command::new("codesign");
        cmd.args(["--force", "--sign", "-", "--entitlements"]);
        cmd.arg(root().join("crates/wrela-vmm/entitlements.plist"));
        cmd.arg(exe);
        run(&mut cmd, "codesign wrela-vmm test binary")?;

        let out = Command::new(exe)
            .arg(DIFF_BLOCK_COUNT_TEST)
            .arg("--exact")
            .arg("--test-threads=1")
            .arg("--nocapture")
            .output()
            .map_err(|e| format!("run {}: {e}", exe.display()))?;
        let stdout = String::from_utf8_lossy(&out.stdout);
        let stderr = String::from_utf8_lossy(&out.stderr);
        print!("{stdout}");
        eprint!("{stderr}");
        let ran_here = stdout
            .lines()
            .find_map(|l| {
                l.strip_prefix("running ")?
                    .strip_suffix(" tests")?
                    .parse::<usize>()
                    .ok()
            })
            .or_else(|| {
                stdout.lines().find_map(|l| {
                    l.strip_prefix("running ")?
                        .strip_suffix(" test")?
                        .parse::<usize>()
                        .ok()
                })
            })
            .unwrap_or(0);
        if ran_here == 0 {
            return Err(format!(
                "diff-block-count: HVF oracle did not run (filter `{DIFF_BLOCK_COUNT_TEST}` \
                 matched 0 tests in {}); refuse to fake a pass",
                exe.display()
            ));
        }
        if !out.status.success() {
            return Err(format!(
                "diff-block-count: HVF oracle failed (exit {:?})",
                out.status.code()
            ));
        }
        ran += ran_here;
    }
    if ran == 0 {
        return fail_closed("diff-block-count", "no HVF oracle iterations ran");
    }
    println!(
        "diff-block-count: Lane 2 guest dump agrees with Lane 3 host DRAM hit map \
         on control case boot-actors (--block-count) ({ran} test(s))"
    );
    Ok(())
}
