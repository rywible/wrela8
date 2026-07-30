//! `cargo xtask gen-lane2-freq <case>` — the Lane 2 sidecar generator
//! (plans/M20.md item C).
//!
//! ## Why this is offline
//!
//! `wrela dump --stage=cost` cannot boot a guest, so a measured `f` vector
//! has to be a committed fixture. Lane 1 already works this way
//! (`lane1-freq.txt`: flat index → method key resolved offline). Lane 2's
//! version has one extra wrinkle item C measured: there are **two disjoint
//! Lane 2 id spaces**. `boot-actors` assigns 184 block ids in the cost-stage
//! closure `--stage=cost` scores and 2527 in the `@test(runtime)` image the
//! guest boots, so an id read out of a boot **does not index the scored
//! program at all**. Keying the sidecar by id would attribute hits to the
//! wrong blocks — exactly the nearest-offset attribution decision 1608
//! forbids.
//!
//! So the sidecar is keyed by `<fn_key>#<block_index>`, an identity that
//! survives both closures, and the id → key translation happens **here**,
//! against the *test-image* closure's own assignment map. This generator
//! runs the identical `build_runtime_test_image` chain the boot goldens and
//! `bench guest` run, with `--block-count` emission **on**, so the map it
//! reads is literally the one the booted image used.
//!
//! ## The chain
//!
//! 1. build the test image under `set_block_count(true)` +
//!    `set_block_bridge(true)`, and take `codegen::block_spans()` — the
//!    `id → fn_key#block_index` map of *that* build;
//! 2. boot it on the codesigned `wrela-vmm` with `--dump-lane2`, which
//!    writes the **host DRAM snapshot** (`lane3 hits=…`). The snapshot, not
//!    the transcript line, is Lane 2's normative sink (decision 1610): the
//!    transcript is capped at `BLOCK_BOUND_PRINT_PAIRS` pairs and says so,
//!    while the snapshot has no console bound;
//! 3. translate every snapshot id through the map — an id with no entry is
//!    a **fail-closed error**, never a dropped or nearest-offset hit;
//! 4. write `tests/golden/<case>/lane2-freq.txt`.
//!
//! The scored closure then resolves `fn_key#idx` to a word-block range in
//! **its own** partition (`cost::bridge`). Two closures, one stable key, no
//! cross-closure id arithmetic.

use std::collections::BTreeMap;

use crate::bench::golden_test_image;
use crate::golden::build_and_sign_vmm;
use crate::{root, stage_repro_dir};

/// Parse `wrela-vmm --dump-lane2`'s file body: one `lane3 hits=<id>:<n>,…`
/// line. Fail closed on anything else — a malformed snapshot must not
/// produce a plausible-looking sidecar.
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
    if !(cfg!(target_os = "macos") && cfg!(target_arch = "aarch64")) {
        return Err(
            "gen-lane2-freq: needs Hypervisor.framework (macOS/aarch64) to boot the case; \
             refuse to fabricate a sidecar without a real measurement"
                .to_string(),
        );
    }

    // (1) the id -> key map of the instrumented test-image build.
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

    // (2) boot it and take the host DRAM snapshot.
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

    // (3) translate — fail closed on any id the map does not know.
    let mut counts: BTreeMap<String, u64> = BTreeMap::new();
    for (id, n) in &hits {
        let key = key_of.get(id).ok_or_else(|| {
            format!(
                "gen-lane2-freq: Lane 2 id {id} has no block key in the test-image assignment \
                 map ({ids_assigned} id(s) assigned) — never attribute by nearest offset \
                 (decision 1608)"
            )
        })?;
        if counts.insert(key.clone(), *n).is_some() {
            return Err(format!("gen-lane2-freq: duplicate key `{key}`"));
        }
    }

    // (4) write the sidecar.
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
         # Generated by `cargo xtask gen-lane2-freq {case}` (plans/M20.md item C).\n\
         #\n\
         # Source: the **host DRAM snapshot** `wrela-vmm --dump-lane2`, which is Lane 2's\n\
         # normative sink (decision 1610). The guest transcript `lane2 hits=` line carried\n\
         # only {printed_pairs} of these {} pair(s) and said so with its own `truncated=`\n\
         # marker, so it is a bounded diagnostic and not the vector.\n\
         #\n\
         # Keys are `<fn_key>#<block_index>`, not Lane 2 ids: the `@test(runtime)` image\n\
         # this was measured on assigns {ids_assigned} block ids, while the cost-stage\n\
         # closure `wrela dump --stage=cost` scores assigns far fewer, and the two id\n\
         # spaces are disjoint. The id -> key translation happened offline here, against\n\
         # this image's own assignment map; `cost::bridge` resolves the key to a word-block\n\
         # range in the scored program's own partition and fails closed on disagreement.\n\
         #\n\
         # Most of these keys name fns the scored closure does not contain (test harness,\n\
         # boot init, unreached stdlib). Those are **uncovered** hits, charged at the\n\
         # program maximum by `cost::compose` and never dropped (04 Section 5 coverage\n\
         # honesty; freeze 1627 keeps the denominator the whole scored set).\n\
         workload={case}\n",
        hits.len()
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
