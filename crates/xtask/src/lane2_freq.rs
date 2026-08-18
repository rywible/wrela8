use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use crate::bench::test_image_details_from_case_dir;
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

fn parse_lane3_core_dumps(text: &str, cores: usize) -> Result<Vec<Vec<(u32, u64)>>, String> {
    let mut out = vec![None; cores];
    for line in text.lines().filter(|line| line.starts_with("lane3 core=")) {
        let rest = line.trim_start_matches("lane3 core=");
        let (core, hits) = rest
            .split_once(" hits=")
            .ok_or_else(|| format!("gen-lane2-freq: malformed per-core dump `{line}`"))?;
        let core = core
            .parse::<usize>()
            .map_err(|_| format!("gen-lane2-freq: bad core in `{line}`"))?;
        if core >= cores || out[core].is_some() {
            return Err(format!(
                "gen-lane2-freq: duplicate or out-of-range core in `{line}`"
            ));
        }
        out[core] = Some(crate::lane2_freq::parse_lane3_dump(&format!(
            "lane3 hits={hits}"
        ))?);
    }
    if out.iter().any(Option::is_none) {
        return Err(format!(
            "gen-lane2-freq: per-core dump does not contain exactly cores 0..{cores}"
        ));
    }
    Ok(out.into_iter().map(Option::unwrap).collect())
}

fn load_core_frequency(
    path: &Path,
    workload: &str,
    cores: usize,
) -> Result<Vec<BTreeMap<String, u64>>, String> {
    let text = std::fs::read_to_string(path)
        .map_err(|error| format!("proxy validation: read {}: {error}", path.display()))?;
    if !text.ends_with('\n') {
        return Err(format!(
            "proxy validation: {} lacks final LF",
            path.display()
        ));
    }
    let mut out = vec![BTreeMap::new(); cores];
    let mut seen_workload = false;
    let mut seen_cores = false;
    for line in text
        .lines()
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
    {
        if let Some(value) = line.strip_prefix("workload=") {
            if seen_workload || value != workload {
                return Err(format!(
                    "proxy validation: {} has wrong workload",
                    path.display()
                ));
            }
            seen_workload = true;
            continue;
        }
        if let Some(value) = line.strip_prefix("cores=") {
            if seen_cores || value.parse::<usize>().ok() != Some(cores) {
                return Err(format!(
                    "proxy validation: {} has wrong core count",
                    path.display()
                ));
            }
            seen_cores = true;
            continue;
        }
        let (prefix, value) = line
            .rsplit_once('=')
            .ok_or_else(|| format!("proxy validation: malformed core frequency `{line}`"))?;
        let rest = prefix
            .strip_prefix("core.")
            .ok_or_else(|| format!("proxy validation: malformed core frequency `{line}`"))?;
        let (core, key) = rest
            .split_once('.')
            .ok_or_else(|| format!("proxy validation: malformed core frequency `{line}`"))?;
        let core = core
            .parse::<usize>()
            .map_err(|_| format!("proxy validation: malformed core frequency `{line}`"))?;
        let count = value
            .parse::<u64>()
            .map_err(|_| format!("proxy validation: malformed core frequency `{line}`"))?;
        if core >= cores || count == 0 || out[core].insert(key.to_string(), count).is_some() {
            return Err(format!("proxy validation: invalid core frequency `{line}`"));
        }
    }
    if !seen_workload || !seen_cores || out.iter().any(BTreeMap::is_empty) {
        return Err(format!(
            "proxy validation: {} is incomplete",
            path.display()
        ));
    }
    Ok(out)
}

pub(crate) struct ValidationPrediction {
    pub(crate) cycles_per_core: Vec<u64>,
    pub(crate) modeled_branch_paths: u64,
    pub(crate) modeled_memory_accesses: u64,
    pub(crate) modeled_memory_transitions: u64,
    pub(crate) workload_sha256: String,
}

pub(crate) fn validation_prediction(case: &str) -> Result<ValidationPrediction, String> {
    let _mode = CompileOptsGuard::mode(wrela_compiler::opts::CompileMode::Release);
    let case_dir = crate::proxy_validation::proxy_fixture_dir(&root(), case)?;
    let source = match std::fs::read_to_string(case_dir.join("root")) {
        Ok(relative) => case_dir.join(relative.trim()),
        Err(_) => case_dir.join("input.wr"),
    };
    let sidecar = case_dir.join("lane2-freq.txt");
    let frequency = wrela_compiler::cost::freq::load_block_from_path(&sidecar)?;
    if frequency.workload != case {
        return Err(format!(
            "proxy validation: {} names workload `{}` instead of `{case}`",
            sidecar.display(),
            frequency.workload
        ));
    }
    let production = wrela_compiler::cost::linked_shipped_program(&source)?
        .0
        .fns
        .keys()
        .cloned()
        .collect::<BTreeSet<_>>();
    wrela_compiler::codegen::set_block_filter(Some(production));
    wrela_compiler::codegen::set_block_count(false);
    wrela_compiler::codegen::set_block_bridge(true);
    let built = test_image_details_from_case_dir(&case_dir);
    wrela_compiler::codegen::set_block_bridge(false);
    wrela_compiler::codegen::set_block_filter(None);
    let built = built?;
    let linked = built.linked;
    let placement = built.placement;
    let needed = frequency
        .counts
        .keys()
        .map(|key| wrela_compiler::cost::split_key(key).map(|(name, _)| name.to_string()))
        .collect::<Result<BTreeSet<_>, _>>()?;
    let mut sections = Vec::new();
    let mut fns = BTreeMap::new();
    for key in needed {
        let mut function = linked
            .fns
            .get(&key)
            .cloned()
            .ok_or_else(|| format!("proxy validation: measured function `{key}` is absent"))?;
        let id = sections.len();
        function.section = id;
        sections.push(wrela_compiler::linked::LinkedSection {
            id,
            name: format!("measured:{key}"),
            byte_address: function.byte_address,
            executable: true,
            code: function.code.clone(),
            raw_bytes: Vec::new(),
            padding_before: 0,
        });
        fns.insert(key, function);
    }
    let image_base = sections
        .iter()
        .map(|section| section.byte_address)
        .min()
        .unwrap_or(wrela_machine::layout::IMAGE_BASE);
    let linked = wrela_compiler::linked::LinkedProgram::from_parts(sections, fns, image_base)?;
    let table = wrela_compiler::cost::load_default()?;
    let flat = wrela_compiler::cost::BlockBridge::from_linked(&linked, &table, &placement)?;
    let measured = wrela_compiler::cost::MeasuredBlocks::resolve(&flat, &frequency.counts)?;
    if measured.unresolved_keys != 0 {
        return Err(format!(
            "proxy validation: `{case}` sidecar has {} unresolved block key(s)",
            measured.unresolved_keys
        ));
    }
    let obs = |key: &str, block: usize| measured.obs(key, block);
    let counts = wrela_compiler::cost::BlockCounts::Measured(&obs);
    let bridge = wrela_compiler::cost::BlockBridge::from_linked_with_counts(
        &linked, &table, &placement, &counts,
    )?;
    let report = wrela_compiler::cost::score_linked_program(&linked, &table, &placement)?;
    let measure = wrela_compiler::cost::block_grain_fxs(&report.fns, &bridge, &frequency.counts)?;
    if measure.matched != measure.total || measure.unresolved_keys != 0 {
        return Err(format!(
            "proxy validation: `{case}` whole-run coverage is {}/{} with {} unresolved key(s)",
            measure.matched, measure.total, measure.unresolved_keys
        ));
    }
    let digest = |bytes: &[u8]| wrela_machine::sha256::sha256_hex(bytes);
    let mut modeled_branch_paths = 0_u64;
    let mut modeled_memory_accesses = 0_u64;
    let mut modeled_memory_transitions = 0_u64;
    for (key, count) in &frequency.counts {
        let wrela_compiler::cost::Resolved::Block(block) = bridge.lookup(key)? else {
            return Err(format!(
                "proxy validation: `{case}` cannot attribute modeled terms for `{key}`"
            ));
        };
        modeled_branch_paths =
            modeled_branch_paths.saturating_add(count.saturating_mul(block.modeled_branch_paths));
        modeled_memory_accesses = modeled_memory_accesses
            .saturating_add(count.saturating_mul(block.modeled_memory_accesses));
        modeled_memory_transitions = modeled_memory_transitions
            .saturating_add(count.saturating_mul(block.modeled_memory_transitions));
    }
    if modeled_branch_paths == 0 || modeled_memory_accesses == 0 || modeled_memory_transitions == 0
    {
        return Err(format!(
            "proxy validation: `{case}` lacks modeled branch or memory-transition evidence"
        ));
    }
    let core_sidecar = case_dir.join("lane2-core-freq.txt");
    let core_frequencies = if placement.cores == 1 {
        vec![frequency.counts.clone()]
    } else {
        load_core_frequency(&core_sidecar, case, placement.cores)?
    };
    let mut cycles_per_core = Vec::with_capacity(placement.cores);
    for (core, core_counts) in core_frequencies.iter().enumerate() {
        let core_measured = wrela_compiler::cost::MeasuredBlocks::resolve(&flat, core_counts)?;
        if core_measured.unresolved_keys != 0 {
            return Err(format!(
                "proxy validation: `{case}` core {core} has {} unresolved block key(s)",
                core_measured.unresolved_keys
            ));
        }
        let core_obs = |key: &str, block: usize| core_measured.obs(key, block);
        let core_hot = |key: &str, block: usize| core_measured.is_hot(key, block);
        let core_bridge = wrela_compiler::cost::BlockBridge::from_linked_with_counts(
            &linked,
            &table,
            &placement,
            &wrela_compiler::cost::BlockCounts::Measured(&core_obs),
        )?;
        let core_measure =
            wrela_compiler::cost::block_grain_fxs(&report.fns, &core_bridge, core_counts)?;
        if core_measure.matched != core_measure.total || core_measure.unresolved_keys != 0 {
            return Err(format!(
                "proxy validation: `{case}` core {core} whole-run coverage is {}/{} with {} unresolved key(s)",
                core_measure.matched, core_measure.total, core_measure.unresolved_keys
            ));
        }
        let footprints = wrela_compiler::cost::compute_linked(
            &linked,
            &table,
            &wrela_compiler::cost::SweepPoint::pinned(&table),
            &placement,
            wrela_compiler::cost::HotBlocks::Measured(&core_hot),
        )?;
        cycles_per_core.push(core_measure.cycles.saturating_add(footprints[core].charge));
    }
    Ok(ValidationPrediction {
        cycles_per_core,
        modeled_branch_paths,
        modeled_memory_accesses,
        modeled_memory_transitions,
        workload_sha256: {
            let mut bytes = b"wrela-proxy-workload-v1\0".to_vec();
            let mut paths = vec![source.as_path(), sidecar.as_path()];
            if placement.cores > 1 {
                paths.push(core_sidecar.as_path());
            }
            for path in paths {
                bytes.extend_from_slice(
                    path.strip_prefix(root())
                        .unwrap_or(path)
                        .to_string_lossy()
                        .as_bytes(),
                );
                bytes.push(0);
                bytes.extend_from_slice(&std::fs::read(path).map_err(|error| {
                    format!("proxy validation: read {}: {error}", path.display())
                })?);
                bytes.push(0xff);
            }
            digest(&bytes)
        },
    })
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

    let case_dir = crate::proxy_validation::proxy_fixture_dir(&root(), case)?;
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

    wrela_compiler::codegen::set_block_filter(Some(production.clone()));
    wrela_compiler::codegen::set_block_count(true);
    wrela_compiler::codegen::set_block_bridge(true);
    let built = test_image_details_from_case_dir(&case_dir);
    let spans = wrela_compiler::codegen::block_spans();
    let ids_assigned = wrela_compiler::codegen::block_ids_assigned();
    wrela_compiler::codegen::set_block_count(false);
    wrela_compiler::codegen::set_block_bridge(false);
    wrela_compiler::codegen::set_block_filter(None);
    let built = built?;
    let cores = built.placement.cores;

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
        &built.image,
        &built.report,
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
    let per_core_hits = parse_lane3_core_dumps(&snapshot_text, cores)?;

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
    let mut core_counts = vec![BTreeMap::new(); cores];
    for (core, hits) in per_core_hits.iter().enumerate() {
        for (id, count) in hits {
            let key = key_of.get(id).ok_or_else(|| {
                format!("gen-lane2-freq: per-core Lane 2 id {id} has no exact block key")
            })?;
            let (fn_key, _) = wrela_compiler::cost::split_key(key)?;
            if production.contains(fn_key)
                && core_counts[core].insert(key.clone(), *count).is_some()
            {
                return Err(format!("gen-lane2-freq: duplicate core {core} key `{key}`"));
            }
        }
        if core_counts[core].is_empty() {
            return Err(format!(
                "gen-lane2-freq: core {core} reached no shipped-image blocks"
            ));
        }
    }
    for (key, aggregate) in &counts {
        let sum = core_counts
            .iter()
            .map(|row| row.get(key).copied().unwrap_or(0))
            .sum::<u64>();
        if sum != *aggregate {
            return Err(format!(
                "gen-lane2-freq: per-core sum for `{key}` is {sum}, aggregate is {aggregate}"
            ));
        }
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
    if cores > 1 {
        let mut core_text = format!(
            "# Per-core Lane 2 frequencies generated with the aggregate sidecar.\nworkload={case}\ncores={cores}\n"
        );
        for (core, row) in core_counts.iter().enumerate() {
            for (key, count) in row {
                core_text.push_str(&format!("core.{core}.{key}={count}\n"));
            }
        }
        let core_path = case_dir.join("lane2-core-freq.txt");
        std::fs::write(&core_path, core_text)
            .map_err(|error| format!("gen-lane2-freq: write {}: {error}", core_path.display()))?;
    }
    println!(
        "gen-lane2-freq: wrote {} ({} block(s), {} id(s) assigned in the test image, \
         {printed_pairs} pair(s) in the bounded transcript line)",
        out_path.display(),
        counts.len(),
        ids_assigned
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const PROXY_CASES: &[&str] = &[
        "boot-actors",
        "boot-pixels-numeric-holdout",
        "boot-pixels-numeric-holdout-v2",
        "boot-pixels-plane-one-core",
        "boot-pixels-walking-skeleton",
        "boot-pixels-partial-mode-three-core",
        "boot-pixels-program-view",
        "boot-pixels-frame-input",
        "boot-pixels-plane-three-core",
    ];

    #[test]
    fn checked_in_frequency_vectors_are_closed_inputs() {
        for case in PROXY_CASES {
            let sidecar = crate::proxy_validation::proxy_fixture_dir(&root(), case)
                .unwrap()
                .join("lane2-freq.txt");
            let frequency = wrela_compiler::cost::freq::load_block_from_path(&sidecar)
                .unwrap_or_else(|error| panic!("`{case}` frequency vector failed: {error}"));
            assert_eq!(frequency.workload, *case);
            assert!(!frequency.counts.is_empty());
            assert!(frequency.counts.values().all(|count| *count > 0));
        }
    }

    #[test]
    #[ignore = "whole proxy corpus prediction belongs in cargo xtask verify-deep"]
    fn checked_in_frequency_vector_produces_a_nonzero_closed_prediction() {
        for case in PROXY_CASES {
            let prediction = validation_prediction(case)
                .unwrap_or_else(|error| panic!("`{case}` prediction failed: {error}"));
            assert!(!prediction.cycles_per_core.is_empty());
            assert!(prediction.cycles_per_core.iter().all(|cycles| *cycles > 0));
            assert!(prediction.modeled_branch_paths > 0);
            assert!(prediction.modeled_memory_accesses > 0);
            assert!(prediction.modeled_memory_transitions > 0);
            assert!(prediction.modeled_memory_transitions <= prediction.modeled_memory_accesses);
            assert_eq!(prediction.workload_sha256.len(), 64);
        }
    }
}
