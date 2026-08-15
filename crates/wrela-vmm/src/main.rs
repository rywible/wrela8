use std::path::PathBuf;
use std::process::ExitCode;

use wrela_machine::vmm_process::{EXIT_REPLAY_DIVERGENCE, EXIT_VMM_FAILURE};

fn usage() -> ! {
    eprintln!(
        "usage: wrela-vmm <report> <img> [--display headless|native] [--record <path>] [--replay <path>] [--dump-lane2 <path>]"
    );
    std::process::exit(EXIT_VMM_FAILURE);
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut positional: Vec<String> = Vec::new();
    let mut record_path: Option<PathBuf> = None;
    let mut replay_path: Option<PathBuf> = None;
    let mut dump_lane2_path: Option<PathBuf> = None;
    let mut display = wrela_vmm::display::DisplayBackendSelection::Headless;
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
            "--display" => {
                i += 1;
                display = match args.get(i).map(String::as_str) {
                    Some("headless") => wrela_vmm::display::DisplayBackendSelection::Headless,
                    Some("native") => wrela_vmm::display::DisplayBackendSelection::Native,
                    _ => usage(),
                };
            }
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
    let report_path = PathBuf::from(report);
    let img_path = PathBuf::from(img);

    if let Some(replay_path) = replay_path {
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
        return match wrela_vmm::record::replay(&report_path, &img_path, &recorded) {
            Ok(divergences) if divergences.is_empty() => {
                if recorded.exit_code == 0 {
                    ExitCode::SUCCESS
                } else {
                    ExitCode::FAILURE
                }
            }
            Ok(divergences) => {
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

    match wrela_vmm::boot_image_with_display(&report_path, &img_path, display) {
        Ok(outcome) => {
            if let Some(record_path) = record_path {
                let recorded = wrela_vmm::record::RecordFile::from_outcome(&outcome);
                if let Err(e) = std::fs::write(&record_path, recorded.to_text()) {
                    eprintln!("wrela-vmm: cannot write {}: {e}", record_path.display());
                    return ExitCode::from(EXIT_VMM_FAILURE as u8);
                }
            }
            if let Some(dump_path) = dump_lane2_path {
                let body = format!(
                    "lane3 hits={}\n",
                    wrela_vmm::lane3::format_hits(&outcome.lane2_hits)
                );
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
