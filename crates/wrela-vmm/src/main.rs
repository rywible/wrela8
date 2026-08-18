use std::path::PathBuf;
use std::process::ExitCode;

use wrela_machine::vmm_process::{EXIT_REPLAY_DIVERGENCE, EXIT_VMM_FAILURE};

fn usage() -> ! {
    eprintln!(
        "usage: wrela-vmm <report> <img> [--display headless|native] [--input-events <path>] [--record <path>] [--replay <path>] [--metrics <path>] [--dump-lane2 <path>] [--diagnostic-mmu-off] [--guest-counters]"
    );
    std::process::exit(EXIT_VMM_FAILURE);
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.as_slice() == ["--validate-product-host"] {
        return match wrela_vmm::validate_product_host_profile() {
            Ok(()) => ExitCode::SUCCESS,
            Err(error) => {
                eprintln!("wrela-vmm: {error}");
                ExitCode::from(EXIT_VMM_FAILURE as u8)
            }
        };
    }
    let mut positional: Vec<String> = Vec::new();
    let mut record_path: Option<PathBuf> = None;
    let mut replay_path: Option<PathBuf> = None;
    let mut dump_lane2_path: Option<PathBuf> = None;
    let mut metrics_path: Option<PathBuf> = None;
    let mut input_events_path: Option<PathBuf> = None;
    let mut display = wrela_vmm::display::DisplayBackendSelection::Headless;
    let mut diagnostic_mmu_off = false;
    let mut guest_counters = false;
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--record" => {
                i += 1;
                match args.get(i) {
                    Some(p) => record_path = Some(PathBuf::from(p)),
                    None => usage(),
                }
            }
            "--replay" => {
                i += 1;
                match args.get(i) {
                    Some(p) => replay_path = Some(PathBuf::from(p)),
                    None => usage(),
                }
            }
            "--dump-lane2" => {
                i += 1;
                match args.get(i) {
                    Some(p) => dump_lane2_path = Some(PathBuf::from(p)),
                    None => usage(),
                }
            }
            "--metrics" => {
                i += 1;
                match args.get(i) {
                    Some(p) => metrics_path = Some(PathBuf::from(p)),
                    None => usage(),
                }
            }
            "--input-events" => {
                i += 1;
                match args.get(i) {
                    Some(p) => input_events_path = Some(PathBuf::from(p)),
                    None => usage(),
                }
            }
            "--display" => {
                i += 1;
                display = match args.get(i).map(String::as_str) {
                    Some("headless") => wrela_vmm::display::DisplayBackendSelection::Headless,
                    Some("native") => wrela_vmm::display::DisplayBackendSelection::Native,
                    _ => usage(),
                };
            }
            "--diagnostic-mmu-off" => diagnostic_mmu_off = true,
            "--guest-counters" => guest_counters = true,
            other => positional.push(other.to_string()),
        }
        i += 1;
    }
    let [report, img] = positional.as_slice() else {
        usage();
    };
    if record_path.is_some() && replay_path.is_some() {
        eprintln!("wrela-vmm: --record and --replay are mutually exclusive");
        return ExitCode::from(EXIT_VMM_FAILURE as u8);
    }
    if dump_lane2_path.is_some() && replay_path.is_some() {
        eprintln!("wrela-vmm: --dump-lane2 is not supported with --replay");
        return ExitCode::from(EXIT_VMM_FAILURE as u8);
    }
    if replay_path.is_some() && display == wrela_vmm::display::DisplayBackendSelection::Native {
        eprintln!("wrela-vmm: replay suppresses native presentation; use --display headless");
        return ExitCode::from(EXIT_VMM_FAILURE as u8);
    }
    if replay_path.is_some() && input_events_path.is_some() {
        eprintln!("wrela-vmm: replay suppresses live input; remove --input-events");
        return ExitCode::from(EXIT_VMM_FAILURE as u8);
    }
    if let Some(path) = input_events_path {
        unsafe { std::env::set_var("WRELA_INPUT_EVENTS", path) };
    }
    if replay_path.is_some() && diagnostic_mmu_off {
        eprintln!("wrela-vmm: --diagnostic-mmu-off is not supported with --replay");
        return ExitCode::from(EXIT_VMM_FAILURE as u8);
    }
    if replay_path.is_some() && guest_counters {
        eprintln!("wrela-vmm: --guest-counters is not supported with --replay");
        return ExitCode::from(EXIT_VMM_FAILURE as u8);
    }
    if diagnostic_mmu_off && guest_counters {
        eprintln!("wrela-vmm: --guest-counters requires sealed stage 1");
        return ExitCode::from(EXIT_VMM_FAILURE as u8);
    }
    let report_path = PathBuf::from(report);
    let img_path = PathBuf::from(img);

    if let Some(replay_path) = replay_path {
        if let Some(metrics_path) = metrics_path {
            let body = "format=wrela-vcpu-run-metrics-v1\nhost_profile=replay-suppressed\n";
            if let Err(e) = std::fs::write(&metrics_path, body) {
                eprintln!("wrela-vmm: cannot write {}: {e}", metrics_path.display());
                return ExitCode::from(EXIT_VMM_FAILURE as u8);
            }
        }
        let text = match std::fs::read_to_string(&replay_path) {
            Ok(t) => t,
            Err(e) => {
                eprintln!("wrela-vmm: cannot read {}: {e}", replay_path.display());
                return ExitCode::from(EXIT_VMM_FAILURE as u8);
            }
        };
        let recorded = match wrela_vmm::record::RecordFile::parse(&text) {
            Ok(r) => r,
            Err(e) => {
                eprintln!(
                    "wrela-vmm: malformed record file {}: {e}",
                    replay_path.display()
                );
                return ExitCode::from(EXIT_VMM_FAILURE as u8);
            }
        };
        return match wrela_vmm::record::replay_with_outcome(&report_path, &img_path, &recorded) {
            Ok((outcome, divergences)) if divergences.is_empty() => {
                use std::io::Write;
                if std::io::stdout().write_all(&outcome.transcript).is_err() {
                    return ExitCode::from(EXIT_VMM_FAILURE as u8);
                }
                if recorded.exit_code == 0 {
                    ExitCode::SUCCESS
                } else {
                    ExitCode::FAILURE
                }
            }
            Ok((_, divergences)) => {
                eprintln!("wrela-vmm: replay diverged from the recording:");
                for d in &divergences {
                    eprintln!("  {d}");
                }
                ExitCode::from(EXIT_REPLAY_DIVERGENCE as u8)
            }
            Err(e) => {
                eprintln!("wrela-vmm: {e}");
                match e {
                    wrela_vmm::VmmError::ReplayDivergence(_) => {
                        ExitCode::from(EXIT_REPLAY_DIVERGENCE as u8)
                    }
                    _ => ExitCode::from(EXIT_VMM_FAILURE as u8),
                }
            }
        };
    }

    let boot = if diagnostic_mmu_off {
        if display == wrela_vmm::display::DisplayBackendSelection::Native {
            eprintln!("wrela-vmm: --diagnostic-mmu-off requires --display headless");
            return ExitCode::from(EXIT_VMM_FAILURE as u8);
        }
        wrela_vmm::boot_image_diagnostic_mmu_off(&report_path, &img_path)
    } else if guest_counters {
        wrela_vmm::boot_image_with_guest_counters(&report_path, &img_path, display)
    } else {
        wrela_vmm::boot_image_with_display(&report_path, &img_path, display)
    };
    match boot {
        Ok(outcome) => {
            if let Some(metrics_path) = metrics_path {
                let mut body = String::from("format=wrela-vcpu-run-metrics-v1\n");
                for (core, run_ns) in outcome.vcpu_run_ns.iter().enumerate() {
                    body.push_str(&format!("core.{core:04}.run_ns={run_ns}\n"));
                }
                if guest_counters {
                    for (core, counters) in outcome.vcpu_guest_counters.iter().enumerate() {
                        for (name, value) in [
                            "br_mis_pred",
                            "cpu_cycles",
                            "inst_retired",
                            "l1d_cache_refill",
                            "l2d_cache_refill",
                            "stall_backend",
                            "stall_frontend",
                        ]
                        .into_iter()
                        .zip(counters)
                        {
                            body.push_str(&format!("core.{core:04}.{name}={value}\n"));
                        }
                    }
                }
                body.push_str(&format!("host_profile={}\n", outcome.host_profile));
                body.push_str(&format!(
                    "translation_profile={}\n",
                    outcome.translation_profile
                ));
                if let Err(e) = std::fs::write(&metrics_path, body) {
                    eprintln!("wrela-vmm: cannot write {}: {e}", metrics_path.display());
                    return ExitCode::from(EXIT_VMM_FAILURE as u8);
                }
            }
            if let Some(record_path) = record_path {
                let recorded = wrela_vmm::record::RecordFile::from_outcome(&outcome);
                if let Err(e) = std::fs::write(&record_path, recorded.to_text()) {
                    eprintln!("wrela-vmm: cannot write {}: {e}", record_path.display());
                    return ExitCode::from(EXIT_VMM_FAILURE as u8);
                }
            }
            if let Some(dump_path) = dump_lane2_path {
                let mut body = format!(
                    "lane3 hits={}\n",
                    wrela_vmm::lane3::format_hits(&outcome.lane2_hits)
                );
                for (core, hits) in outcome.lane2_hits_per_core.iter().enumerate() {
                    body.push_str(&format!(
                        "lane3 core={core} hits={}\n",
                        wrela_vmm::lane3::format_hits(hits)
                    ));
                }
                if let Err(e) = std::fs::write(&dump_path, body) {
                    eprintln!("wrela-vmm: cannot write {}: {e}", dump_path.display());
                    return ExitCode::from(EXIT_VMM_FAILURE as u8);
                }
            }
            use std::io::Write;
            let mut stdout = std::io::stdout();
            if stdout.write_all(&outcome.transcript).is_err() {
                return ExitCode::from(EXIT_VMM_FAILURE as u8);
            }
            if outcome.exit_code == 0 {
                ExitCode::SUCCESS
            } else {
                ExitCode::FAILURE
            }
        }
        Err(e) => {
            eprintln!("wrela-vmm: {e}");
            ExitCode::from(EXIT_VMM_FAILURE as u8)
        }
    }
}
