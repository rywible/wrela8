use std::collections::{BTreeMap, BTreeSet};

use crate::bench::golden_test_image;
use crate::golden::build_and_sign_vmm;
use crate::{CompileOptsGuard, root, stage_repro_dir};

fn parse_lane3_dump(text: &str) -> Result<Vec<(u32, u64)>, String> {
    let line = text
        .lines()
        .find(|l| l.starts_with("lane3 hits="))
        .ok_or_else(|| "gen-lane2-freq: --dump-lane2 file has no `lane3 hits=` line".to_string())?;
    let body = line.trim_start_matches("lane3 hits=").trim();
    if body.is_empty() {
        return Err(
            "gen-lane2-freq: Lane 3 snapshot is empty — the control case must \
                    exercise block counters (fail closed)"
                .to_string(),
        );
    }
    let mut out = Vec::new();
    for part in body.split(',') {
        let (id, n) = part
            .split_once(':')
            .ok_or_else(|| format!("gen-lane2-freq: malformed pair `{part}` (want id:count)"))?;
        let id: u32 = id
            .parse()
            .map_err(|e| format!("gen-lane2-freq: bad id `{id}`: {e}"))?;
        let n: u64 = n
            .parse()
            .map_err(|e| format!("gen-lane2-freq: bad count `{n}`: {e}"))?;
        if n == 0 {
            return Err(format!("gen-lane2-freq: zero count for id {id}"));
        }
        out.push((id, n));
    }
    Ok(out)
}

pub(crate) fn gen_lane2_freq(case: &str) -> Result<(), String> {
    let _mode = CompileOptsGuard::mode(wrela_compiler::opts::CompileMode::Release);
    if !(cfg!(target_os = "macos") && cfg!(target_arch = "aarch64")) {
        return Err(
            "gen-lane2-freq: needs Hypervisor.framework (macOS/aarch64) to boot the case; \
             refuse to fabricate a sidecar without a real measurement"
                .to_string(),
        );
    }

    wrela_compiler::codegen::set_block_count(true);
    wrela_compiler::codegen::set_block_bridge(true);
    let built = golden_test_image(case);
    let spans = wrela_compiler::codegen::block_spans();
    let ids_assigned = wrela_compiler::codegen::block_ids_assigned();
    wrela_compiler::codegen::set_block_count(false);
    wrela_compiler::codegen::set_block_bridge(false);
    let (img_bytes, report_text) = built?;

    let mut key_of: BTreeMap<u32, String> = BTreeMap::new();
    for s in &spans {
        let key = wrela_compiler::cost::make_key(&s.fn_key, s.block_index);
        if let Some(prev) = key_of.insert(s.id, key.clone()) {
            return Err(format!(
                "gen-lane2-freq: id {} maps to both `{prev}` and `{key}` — the assignment map \
                 is not injective (fail closed)",
                s.id
            ));
        }
    }
    if key_of.len() != ids_assigned as usize {
        return Err(format!(
            "gen-lane2-freq: {} span(s) recorded but {ids_assigned} id(s) assigned — the bridge \
             did not observe every Lane 2 block (fail closed)",
            key_of.len()
        ));
    }

    let vmm = build_and_sign_vmm()?;
    let (_, img_path, report_path, _) = stage_repro_dir(
        &format!("target/gen-lane2-freq-{case}"),
        &img_bytes,
        &report_text,
    )?;
    let dump_path = img_path.with_file_name("lane2-snapshot.txt");
    let out = std::process::Command::new(&vmm)
        .arg(&report_path)
        .arg(&img_path)
        .arg("--dump-lane2")
        .arg(&dump_path)
        .output()
        .map_err(|e| format!("gen-lane2-freq: run wrela-vmm: {e}"))?;
    let transcript = String::from_utf8_lossy(&out.stdout).into_owned();
    let code = out.status.code().unwrap_or(-1);
    if code != 0 {
        return Err(format!(
            "gen-lane2-freq: `{case}` did not boot cleanly (exit {code}); a sidecar generated \
             from a failed boot would be a fiction.\nstdout:\n{transcript}\nstderr:\n{}",
            String::from_utf8_lossy(&out.stderr)
        ));
    }
    let snapshot_text = std::fs::read_to_string(&dump_path)
        .map_err(|e| format!("gen-lane2-freq: read {}: {e}", dump_path.display()))?;
    let hits = parse_lane3_dump(&snapshot_text)?;

    // Rank only the function universe emitted by the shipped image.  Test
    // runner, assertion formatting, and other harness-only blocks are
    // measurement artifacts, not unresolved production work.
    let case_dir = root().join(format!("tests/golden/{case}"));
    let source = match std::fs::read_to_string(case_dir.join("root")) {
        Ok(relative) => case_dir.join(relative.trim()),
        Err(_) => case_dir.join("input.wr"),
    };
    let production: BTreeSet<String> = wrela_compiler::cost::linked_shipped_program(&source)?
        .0
        .fns
        .keys()
        .cloned()
        .collect();

    let mut counts: BTreeMap<String, u64> = BTreeMap::new();
    for (id, n) in &hits {
        let key = key_of.get(id).ok_or_else(|| {
            format!(
                "gen-lane2-freq: Lane 2 id {id} has no block key in the test-image assignment \
                 map ({ids_assigned} id(s) assigned) — never attribute by nearest offset \
                 (decision 1608)"
            )
        })?;
        let (fn_key, _) = wrela_compiler::cost::split_key(key)?;
        if !production.contains(fn_key) {
            continue;
        }
        if counts.insert(key.clone(), *n).is_some() {
            return Err(format!("gen-lane2-freq: duplicate key `{key}`"));
        }
    }
    if counts.is_empty() {
        return Err(
            "gen-lane2-freq: the measured window reached no shipped-image blocks".to_string(),
        );
    }

    let transcript_line = transcript
        .lines()
        .find(|l| l.starts_with("lane2 hits="))
        .unwrap_or("<none>");
    let printed_pairs = transcript_line
        .trim_start_matches("lane2 hits=")
        .split(" truncated=")
        .next()
        .map(|b| {
            if b.is_empty() {
                0
            } else {
                b.split(',').count()
            }
        })
        .unwrap_or(0);
    let mut text = String::new();
    text.push_str(&format!(
        "# Lane 2 block-grain frequencies for workload `{case}`.\n\
         # Generated by `cargo xtask gen-lane2-freq {case}`.\n\
         #\n\
         # Source: the **host DRAM snapshot** `wrela-vmm --dump-lane2`, which is Lane 2's\n\
         # normative sink (decision 1610). The guest transcript `lane2 hits=` line carried\n\
         # only {printed_pairs} of these {} pair(s) and said so with its own `truncated=`\n\
         # marker, so it is a bounded diagnostic and not the vector.\n\
         #\n\
         # Keys are `<fn_key>#<block_index>`, not Lane 2 ids: the `@test(runtime)` image\n\
         # this was measured on assigns {ids_assigned} block ids. Translation uses that\n\
         # image's exact origin map, then removes test-harness-only keys by intersecting\n\
         # with the separately compiled shipped-image function universe. Every committed\n\
         # key must therefore resolve exactly in linked-image scoring; there is no\n\
         # maximum-function fallback in the profitability gate.\n\
         workload={case}\n",
        counts.len()
    ));
    for (key, n) in &counts {
        text.push_str(&format!("{key}={n}\n"));
    }
    let out_path = root().join(format!("tests/golden/{case}/lane2-freq.txt"));
    std::fs::write(&out_path, &text)
        .map_err(|e| format!("gen-lane2-freq: write {}: {e}", out_path.display()))?;
    println!(
        "gen-lane2-freq: wrote {} ({} block(s), {} id(s) assigned in the test image, \
         {printed_pairs} pair(s) in the bounded transcript line)",
        out_path.display(),
        counts.len(),
        ids_assigned
    );
    Ok(())
}
