//! Stable text dump for `--stage=cost` (plans/M18.md items D+E;
//! integrity Item J multi-W rows).

use std::path::Path;

use crate::codegen::CodegenProgram;
use crate::placement::PlacementTable;

use super::attr::attribute_cores;
use super::compose::{WorkloadAttach, attach_workloads};
use super::ghz::{self, fmt_compact};
use super::score::{CostReport, score_program};
use super::table::CostTable;
use super::workload::FLAT_NAME;

/// Score `program`, attach multi-W rows from `attach`, then format.
pub fn dump(
    program: &CodegenProgram,
    table: &CostTable,
    placement: &PlacementTable,
    ghz: f64,
    attach: &WorkloadAttach,
) -> Result<String, String> {
    // Item E threaded `placement` into scoring (items F and G read it for
    // the per-core footprint and the local/remote verdict); item C made
    // `attach_workloads` fallible (the bridge fails closed) and gave
    // `format_report` the attach so it can print the block-grain coverage
    // row. Both are needed.
    let mut report = score_program(program, table, placement)?;
    attach_workloads(&mut report, attach)?;
    format_report(&report, placement, ghz, Some(attach))
}

/// Convenience: load default workloads (+ sibling `lane1-freq.txt` when
/// `source` is set) then dump.
pub fn dump_for_source(
    program: &CodegenProgram,
    table: &CostTable,
    placement: &PlacementTable,
    ghz: f64,
    source: Option<&Path>,
) -> Result<String, String> {
    let attach = WorkloadAttach::load_default_for(source, program, table, placement)?;
    dump(program, table, placement, ghz, &attach)
}

fn format_report(
    report: &CostReport,
    placement: &PlacementTable,
    ghz: f64,
    attach: Option<&WorkloadAttach>,
) -> Result<String, String> {
    let mut out = String::new();
    // plans/M20.md item D: the v2 `ports.* / max_issue_per_cycle /
    // branch_penalty / mem_reuse_window / mem_working_set_cap` fields are
    // gone — they were v2 concepts, and their replacements land in items
    // E / F / H. What replaces them is the profile's identity: the pipeline
    // set, the dispatch constraints, the bounded reorder window, and the
    // **provenance digest** over the tier mix (freeze 1629).
    let mut header = format!(
        "Cost version={} profile={} pipelines={} dispatch_mops={} dispatch_uops={} reorder_window={} digest={} provenance={} ghz={}",
        report.version,
        report.profile,
        report.pipelines,
        report.dispatch_mops,
        report.dispatch_uops,
        report.reorder_window,
        report.digest,
        report.provenance,
        fmt_compact(ghz)
    );
    if let Some(wd) = &report.workloads_digest {
        header.push_str(&format!(" workloads_digest={wd}"));
    }
    push_line(&mut out, 0, &header);
    // The digest alone is opaque; the tier mix is the thing a reviewer
    // actually reads to see whether the model rests on vendor-normative
    // rows or on brackets, so it gets its own indented line.
    push_line(
        &mut out,
        1,
        &format!("Provenance {}", report.provenance_summary),
    );
    push_line(
        &mut out,
        1,
        // plans/M20.md item F appends `pseudo_lru=modelled_as_lru` without
        // reordering item A's fields: A76's L1I/L1D replacement policy is
        // pseudo-LRU (Core TRM) and `cost::mem`'s levels are true LRU, so
        // the approximation is named where a reader of a pinned dump sees
        // it rather than only in a module comment.
        "Assumptions ignore_cache=0 ignore_mispredict=0 target=a76_pi5 ghz_model=1 turn_path=max_entry_method valid_for=static_shape_opts workload=flat pseudo_lru=modelled_as_lru",
    );
    push_line(&mut out, 1, "Composition sum_of_fn_schedules=1");
    push_line(
        &mut out,
        1,
        &format!("Total proxy_cycles={}", report.total_proxy_cycles),
    );
    append_workload_rows(&mut out, 1, report, attach);
    for name in ["app", "runtime", "driver"] {
        let cycles = report.owner_totals.get(name).copied().unwrap_or(0);
        push_line(
            &mut out,
            1,
            &format!("Owner name={name} proxy_cycles={cycles}"),
        );
    }
    append_core_block(&mut out, 1, report, placement, ghz, true, attach)?;
    for f in &report.fns {
        push_line(
            &mut out,
            1,
            &format!(
                "Fn key={} owner={} proxy_cycles={}",
                f.key, f.owner, f.proxy_cycles
            ),
        );
        for (rule, count) in &f.terms {
            push_line(&mut out, 2, &format!("Term rule={rule} count={count}"));
        }
    }
    Ok(out)
}

/// Emit `Workload name=…` rows. Flat first; other names sorted.
/// Measured W get a nested `coverage=matched/total grain=<method|block>`
/// line. The grain is on the review surface because a case may commit both
/// sidecars and `attach_workloads` resolves that in favour of block grain —
/// which must never be a silent choice (plans/M20.md item C).
pub(crate) fn append_workload_rows(
    out: &mut String,
    depth: usize,
    report: &CostReport,
    attach: Option<&WorkloadAttach>,
) {
    if report.workload_totals.is_empty() {
        push_line(
            out,
            depth,
            &format!(
                "Workload name={FLAT_NAME} proxy_cycles={}",
                report.total_proxy_cycles
            ),
        );
        return;
    }
    if let Some(cycles) = report.workload_totals.get(FLAT_NAME) {
        push_line(
            out,
            depth,
            &format!("Workload name={FLAT_NAME} proxy_cycles={cycles}"),
        );
    }
    for (name, cycles) in &report.workload_totals {
        if name == FLAT_NAME {
            continue;
        }
        push_line(
            out,
            depth,
            &format!("Workload name={name} proxy_cycles={cycles}"),
        );
        if let Some(&(matched, total)) = report.workload_coverage.get(name) {
            let grain = attach.and_then(|a| a.grain_of(name)).unwrap_or("method");
            push_line(
                out,
                depth + 1,
                &format!("coverage={matched}/{total} grain={grain}"),
            );
        }
    }
}

/// Append Core / Budget / Shared / optional Placeable lines after owners.
///
/// Each `Core n=…` line is followed by that core's **per-core text and
/// translation budget** line (04 §6, plans/M20.md item F): its hot text
/// against its 64 KiB L1I, and its page span against the 48-entry I-TLB and
/// the 1280-entry L2 TLB. 04 §5 makes that budget the hard constraint code
/// growth is argued against, which is why it is per core and why it prints
/// beside the core it belongs to rather than as a program-wide aggregate.
///
/// A case that commits a block-grain `lane2-freq.txt` additionally gets a
/// `MeasuredBudget workload=<name> …` line per core per measured workload
/// (plans/M20.md items C+F): the same budget with hot text restricted to
/// blocks whose measured `f` is non-zero. Both lines print, because the two
/// answer different questions — the flat one is the static-footprint row
/// 04 §5's veto is argued against, the measured one says how much of that
/// text the measurement actually reached (decision 1617).
///
/// Legacy single-file dumps with no image (`cores == 0` and no entries)
/// omit this block entirely.
pub(crate) fn append_core_block(
    out: &mut String,
    depth: usize,
    report: &CostReport,
    placement: &PlacementTable,
    ghz: f64,
    include_placeables: bool,
    attach: Option<&WorkloadAttach>,
) -> Result<(), String> {
    if placement.cores == 0 && placement.entries.is_empty() {
        return Ok(());
    }
    let attr = attribute_cores(report, placement)?;
    for c in &attr.cores {
        let tps = match ghz::turns_per_sec(c.max_turn_proxy, ghz) {
            Some(v) => fmt_compact(v),
            None => "n/a".to_string(),
        };
        let mpt = match ghz::ms_per_turn(c.max_turn_proxy, ghz) {
            Some(v) => fmt_compact(v),
            None => "n/a".to_string(),
        };
        push_line(
            out,
            depth,
            &format!(
                "Core n={} proxy_cycles={} max_turn_proxy={} turns_per_sec={} ms_per_turn_model={}",
                c.n, c.proxy_cycles, c.max_turn_proxy, tps, mpt
            ),
        );
        if let Some(b) = report.footprint.iter().find(|b| b.n == c.n) {
            push_line(out, depth, &b.render());
        }
        if let Some(a) = attach {
            for (name, budgets) in &a.measured_footprint {
                if let Some(b) = budgets.iter().find(|b| b.n == c.n) {
                    push_line(out, depth, &b.render_measured(name));
                }
            }
        }
    }
    push_line(
        out,
        depth,
        &format!("Shared proxy_cycles={}", attr.shared_proxy_cycles),
    );
    if include_placeables {
        for p in &attr.placeables {
            let method = p.method.as_deref().unwrap_or("-");
            push_line(
                out,
                depth,
                &format!(
                    "Placeable id={} type={} core={} turn_proxy={} method={}",
                    p.id, p.type_name, p.core, p.turn_proxy, method
                ),
            );
        }
    }
    Ok(())
}

fn push_line(out: &mut String, depth: usize, line: &str) {
    out.push_str(&"  ".repeat(depth));
    out.push_str(line);
    out.push('\n');
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cost::compose::WorkloadAttach;
    use crate::cost::ghz::DEFAULT_GHZ;
    use crate::cost::score::FnCost;
    use crate::cost::workload::parse as parse_workloads;
    use crate::eval::image::ImageDeclRef;
    use crate::placement::{PlacementEntry, PlacementSource};
    use std::collections::BTreeMap;

    fn fn_cost(key: &str, cycles: u64) -> FnCost {
        FnCost {
            key: key.to_string(),
            owner: "app".to_string(),
            proxy_cycles: cycles,
            words: cycles,
            terms: BTreeMap::new(),
        }
    }

    fn report(fns: Vec<FnCost>) -> CostReport {
        let total: u64 = fns.iter().map(|f| f.proxy_cycles).sum();
        CostReport {
            version: 3,
            digest: "test".to_string(),
            provenance: "test-prov".to_string(),
            provenance_summary: "T1=1 T2=0 T3=0 T4=0 T5=0 rows=1".to_string(),
            profile: "a76-pi5".to_string(),
            pipelines: 8,
            dispatch_mops: 4,
            dispatch_uops: 8,
            reorder_window: 128,
            total_proxy_cycles: total,
            total_words: total,
            owner_totals: BTreeMap::from([("app".to_string(), total)]),
            fns,
            workloads_digest: None,
            workload_totals: BTreeMap::new(),
            workload_coverage: BTreeMap::new(),
            footprint: Vec::new(),
        }
    }

    fn entry(id: ImageDeclRef, type_name: &str, core: usize) -> PlacementEntry {
        PlacementEntry {
            id,
            type_name: type_name.to_string(),
            core,
            source: PlacementSource::Explicit,
            work: 0,
            work_source: "unproved",
            bytes: 0,
            bytes_state: 0,
            bytes_mailbox: 0,
            bytes_pool: 0,
        }
    }

    #[test]
    fn legacy_empty_placement_omits_core_block() {
        let mut report = report(vec![fn_cost("add", 10)]);
        let set = parse_workloads("[flat]\nweight = 1\n").unwrap();
        attach_workloads(
            &mut report,
            &WorkloadAttach {
                set,
                frequencies: BTreeMap::new(),
                block_frequencies: BTreeMap::new(),
                bridge: None,
                measured_footprint: BTreeMap::new(),
            },
        )
        .expect("attach");
        let placement = PlacementTable {
            entries: Vec::new(),
            cores: 0,
        };
        let text = format_report(&report, &placement, DEFAULT_GHZ, None).expect("ok");
        assert!(text.contains("workloads_digest="), "got:\n{text}");
        assert!(
            text.contains("Workload name=flat proxy_cycles=10"),
            "got:\n{text}"
        );
        assert!(!text.contains("Workload name=boot-actors"), "got:\n{text}");
        assert!(!text.contains("issue_width"), "got:\n{text}");
        assert!(text.contains("ghz=2.4"), "got:\n{text}");
        // plans/M20.md item A: the Assumptions line is the clearest single
        // signal of what the model claims, so the whole prefix is pinned
        // here and not only its tail — `target=a76_pi5` with the cache and
        // mispredict terms live is the reversal this milestone lands.
        assert!(text.contains(
            "Assumptions ignore_cache=0 ignore_mispredict=0 target=a76_pi5 ghz_model=1 turn_path=max_entry_method valid_for=static_shape_opts workload=flat"
        ));
        assert!(!text.contains("Core n="), "got:\n{text}");
        assert!(!text.contains("Shared proxy_cycles="), "got:\n{text}");
        assert!(!text.contains("Placeable "), "got:\n{text}");
        assert!(text.contains("Fn key=add"));
    }

    #[test]
    fn measured_workload_row_and_coverage() {
        let mut report = report(vec![
            fn_cost("Ledger.mark", 88),
            fn_cost("Worker.slow", 833),
        ]);
        let set = parse_workloads("[flat]\nweight = 1\n[boot-actors]\nweight = 10\n").unwrap();
        let mut frequencies = BTreeMap::new();
        frequencies.insert(
            "boot-actors".to_string(),
            BTreeMap::from([
                ("Ledger.mark".to_string(), 3u64),
                ("Worker.slow".to_string(), 1u64),
            ]),
        );
        let attach = WorkloadAttach {
            set,
            frequencies,
            block_frequencies: BTreeMap::new(),
            bridge: None,
            measured_footprint: BTreeMap::new(),
        };
        attach_workloads(&mut report, &attach).expect("attach");
        let placement = PlacementTable {
            entries: Vec::new(),
            cores: 0,
        };
        let text = format_report(&report, &placement, DEFAULT_GHZ, None).expect("ok");
        assert!(
            text.contains("Workload name=flat proxy_cycles=921"),
            "got:\n{text}"
        );
        assert!(
            text.contains("Workload name=boot-actors proxy_cycles=1097"),
            "got:\n{text}"
        );
        assert!(text.contains("coverage=4/4"), "got:\n{text}");
    }

    #[test]
    fn core_lines_include_scaled_rates() {
        let report = report(vec![fn_cost("Foo.hot", 2400)]);
        let placement = PlacementTable {
            cores: 1,
            entries: vec![entry(ImageDeclRef::Actor(0), "Foo", 0)],
        };
        let at_24 = format_report(&report, &placement, 2.4, None).expect("ok");
        let at_1 = format_report(&report, &placement, 1.0, None).expect("ok");
        assert!(at_24.contains("ghz=2.4"), "got:\n{at_24}");
        assert!(at_1.contains("ghz=1"), "got:\n{at_1}");
        assert!(at_24.contains("turns_per_sec=1000000"), "got:\n{at_24}");
        assert!(
            at_1.contains("turns_per_sec=416666.6666666667"),
            "got:\n{at_1}"
        );
        assert!(at_24.contains("ms_per_turn_model=0.001"), "got:\n{at_24}");
        assert!(at_1.contains("Placeable id=actor#0"));
        assert!(at_1.contains("method=Foo.hot"));
        assert!(at_24.contains("Shared proxy_cycles=0"));
    }

    #[test]
    fn zero_turn_prints_na_rates() {
        let report = report(vec![fn_cost("__wrela_abort", 5)]);
        let placement = PlacementTable {
            cores: 1,
            entries: vec![entry(ImageDeclRef::Actor(0), "Empty", 0)],
        };
        let text = format_report(&report, &placement, DEFAULT_GHZ, None).expect("ok");
        assert!(
            text.contains("turns_per_sec=n/a ms_per_turn_model=n/a"),
            "got:\n{text}"
        );
        assert!(text.contains("method=-"), "got:\n{text}");
    }
}
