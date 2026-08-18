use std::path::Path;

use crate::codegen::CodegenProgram;
use crate::placement::PlacementTable;

use super::attr::attribute_cores;
use super::compose::{WorkloadAttach, attach_workloads};
use super::ghz::fmt_compact;
use super::score::{CostReport, score_linked_program, score_program};
use super::table::CostTable;
use super::workload::FLAT_NAME;

pub fn dump(
    program: &CodegenProgram,
    table: &CostTable,
    placement: &PlacementTable,
    ghz: f64,
    attach: &WorkloadAttach,
) -> Result<String, String> {
    let mut report = score_program(program, table, placement)?;
    attach_workloads(&mut report, attach)?;
    format_report(&report, placement, ghz, Some(attach), true)
}

pub fn dump_linked(
    linked: &crate::linked::LinkedProgram,
    table: &CostTable,
    placement: &PlacementTable,
    ghz: f64,
) -> Result<String, String> {
    dump_linked_for_source(linked, table, placement, ghz, None)
}

pub fn dump_linked_for_source(
    linked: &crate::linked::LinkedProgram,
    table: &CostTable,
    placement: &PlacementTable,
    ghz: f64,
    source: Option<&Path>,
) -> Result<String, String> {
    let mut report = score_linked_program(linked, table, placement)?;
    let attach = WorkloadAttach::load_default_for_linked(source, linked, table, placement)?;
    attach_workloads(&mut report, &attach)?;
    let mut out = format_report(&report, placement, ghz, Some(&attach), false)?;
    push_line(
        &mut out,
        1,
        &format!(
            "Scope scope=linked-image executable_words={} executable_code_bytes={} fetched_text_bytes={} image_bytes={} rodata_bytes={} sync_frame_max_bytes={} async_frame_total_bytes={}",
            linked.executable_words(),
            linked.executable_code_bytes(),
            report
                .footprint
                .iter()
                .map(|b| b.fetched_text_bytes)
                .sum::<u64>(),
            linked.image_bytes,
            linked.rodata_bytes(),
            linked.sync_frame_max_bytes,
            linked.async_frame_total_bytes,
        ),
    );
    Ok(out)
}

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
    emit_closure_scope: bool,
) -> Result<String, String> {
    let mut out = String::new();
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
    push_line(
        &mut out,
        1,
        &format!("Provenance {}", report.provenance_summary),
    );
    push_line(
        &mut out,
        1,
        "Assumptions ignore_cache=0 ignore_mispredict=0 target=a76_pi5 ghz_model=1 turn_path=max_entry_method_proxy_cycles valid_for=static_shape_opts workload=flat pseudo_lru=modelled_as_lru",
    );
    push_line(&mut out, 1, "Composition sum_of_fn_schedules=1");
    push_line(
        &mut out,
        1,
        &format!(
            "Rank rank_cycles={} schedule_cycles={} footprint_cycles={} total_proxy_cycles={}",
            report.rank_cycles,
            report.schedule_cycles,
            report.footprint_cycles,
            report.total_proxy_cycles
        ),
    );
    if emit_closure_scope {
        let fetched_text_bytes: u64 = report.footprint.iter().map(|b| b.fetched_text_bytes).sum();
        let sync_frame_max_bytes = report.sync_frame_max_bytes;
        push_line(
            &mut out,
            1,
            &format!(
                "Scope scope=closure executable_words={} executable_code_bytes={} fetched_text_bytes={} rodata_bytes=0 image_bytes=n/a sync_frame_max_bytes={} async_frame_total_bytes=0",
                report.total_words,
                report.total_words.saturating_mul(4),
                fetched_text_bytes,
                sync_frame_max_bytes,
            ),
        );
    }
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
                "Fn key={} owner={} frame_bytes={} proxy_cycles={}",
                f.key, f.owner, f.frame_bytes, f.proxy_cycles
            ),
        );
        for (rule, count) in &f.terms {
            push_line(&mut out, 2, &format!("Term rule={rule} count={count}"));
        }
    }
    Ok(out)
}

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
        if let Some(bound) = report.workload_validation_bounds.get(name) {
            push_line(
                out,
                depth + 1,
                &format!("validation_bound_cycles={bound} basis=published-upper-serial-v1"),
            );
        }
    }
}

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
        let _ = ghz;
        push_line(
            out,
            depth,
            &format!(
                "Core n={} proxy_cycles={} max_entry_method_proxy_cycles={}",
                c.n, c.proxy_cycles, c.max_turn_proxy
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
            frame_bytes: 0,
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
            schedule_cycles: total,
            footprint_cycles: 0,
            rank_cycles: total,
            total_words: total,
            sync_frame_max_bytes: 0,
            async_frame_total_bytes: 0,
            owner_totals: BTreeMap::from([("app".to_string(), total)]),
            fns,
            workloads_digest: None,
            workload_totals: BTreeMap::new(),
            workload_coverage: BTreeMap::new(),
            workload_validation_bounds: BTreeMap::new(),
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
        let text = format_report(&report, &placement, DEFAULT_GHZ, None, false).expect("ok");
        assert!(text.contains("workloads_digest="), "got:\n{text}");
        assert!(
            text.contains("Workload name=flat proxy_cycles=10"),
            "got:\n{text}"
        );
        assert!(!text.contains("Workload name=boot-actors"), "got:\n{text}");
        assert!(!text.contains("issue_width"), "got:\n{text}");
        assert!(text.contains("ghz=2.4"), "got:\n{text}");
        assert!(text.contains(
            "Assumptions ignore_cache=0 ignore_mispredict=0 target=a76_pi5 ghz_model=1 turn_path=max_entry_method_proxy_cycles valid_for=static_shape_opts workload=flat"
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
        let text = format_report(&report, &placement, DEFAULT_GHZ, None, false).expect("ok");
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
        let at_24 = format_report(&report, &placement, 2.4, None, false).expect("ok");
        let at_1 = format_report(&report, &placement, 1.0, None, false).expect("ok");
        assert!(at_24.contains("ghz=2.4"), "got:\n{at_24}");
        assert!(at_1.contains("ghz=1"), "got:\n{at_1}");
        assert!(
            at_24.contains("max_entry_method_proxy_cycles=2400"),
            "got:\n{at_24}"
        );
        assert!(
            at_1.contains("max_entry_method_proxy_cycles=2400"),
            "got:\n{at_1}"
        );
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
        let text = format_report(&report, &placement, DEFAULT_GHZ, None, false).expect("ok");
        assert!(
            text.contains("max_entry_method_proxy_cycles=0"),
            "got:\n{text}"
        );
        assert!(text.contains("method=-"), "got:\n{text}");
    }
}
