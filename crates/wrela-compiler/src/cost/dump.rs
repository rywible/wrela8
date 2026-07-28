//! Stable text dump for `--stage=cost` (plans/M18.md items D+E).

use crate::codegen::CodegenProgram;
use crate::placement::PlacementTable;

use super::attr::attribute_cores;
use super::ghz::{self, fmt_compact};
use super::score::{CostReport, score_program};
use super::table::CostTable;

/// Score `program` then format the cost dump (owners, optional per-core
/// attribution, then Fn/Term lines).
pub fn dump(
    program: &CodegenProgram,
    table: &CostTable,
    placement: &PlacementTable,
    ghz: f64,
) -> Result<String, String> {
    let report = score_program(program, table)?;
    format_report(&report, placement, ghz)
}

fn format_report(
    report: &CostReport,
    placement: &PlacementTable,
    ghz: f64,
) -> Result<String, String> {
    let mut out = String::new();
    push_line(
        &mut out,
        0,
        &format!(
            "Cost version={} digest={} issue_width={} ghz={}",
            report.version,
            report.digest,
            report.issue_width,
            fmt_compact(ghz)
        ),
    );
    push_line(
        &mut out,
        1,
        "Assumptions ignore_cache=1 ignore_mispredict=1 target=isa_baseline ghz_model=1 turn_path=max_entry_fn",
    );
    push_line(&mut out, 1, "Composition sum_of_fn_schedules=1");
    push_line(
        &mut out,
        1,
        &format!("Total proxy_cycles={}", report.total_proxy_cycles),
    );
    for name in ["app", "runtime", "driver"] {
        let cycles = report.owner_totals.get(name).copied().unwrap_or(0);
        push_line(
            &mut out,
            1,
            &format!("Owner name={name} proxy_cycles={cycles}"),
        );
    }
    append_core_block(&mut out, 1, report, placement, ghz, true)?;
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

/// Append Core / Shared / optional Placeable lines after owners.
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
                "Core n={} proxy_cycles={} max_turn_proxy={} turns_per_sec={} ms_per_turn={}",
                c.n, c.proxy_cycles, c.max_turn_proxy, tps, mpt
            ),
        );
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
    use crate::cost::ghz::DEFAULT_GHZ;
    use crate::cost::score::FnCost;
    use crate::eval::image::ImageDeclRef;
    use crate::placement::{PlacementEntry, PlacementSource};
    use std::collections::BTreeMap;

    fn fn_cost(key: &str, cycles: u64) -> FnCost {
        FnCost {
            key: key.to_string(),
            owner: "app".to_string(),
            proxy_cycles: cycles,
            terms: BTreeMap::new(),
        }
    }

    fn report(fns: Vec<FnCost>) -> CostReport {
        let total: u64 = fns.iter().map(|f| f.proxy_cycles).sum();
        CostReport {
            version: 1,
            digest: "test".to_string(),
            issue_width: 4,
            total_proxy_cycles: total,
            owner_totals: BTreeMap::from([("app".to_string(), total)]),
            fns,
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
        let report = report(vec![fn_cost("add", 10)]);
        let placement = PlacementTable {
            entries: Vec::new(),
            cores: 0,
        };
        let text = format_report(&report, &placement, DEFAULT_GHZ).expect("ok");
        assert!(text.contains("ghz=2.4"), "got:\n{text}");
        assert!(text.contains("ghz_model=1 turn_path=max_entry_fn"));
        assert!(!text.contains("Core n="), "got:\n{text}");
        assert!(!text.contains("Shared proxy_cycles="), "got:\n{text}");
        assert!(!text.contains("Placeable "), "got:\n{text}");
        assert!(text.contains("Fn key=add"));
    }

    #[test]
    fn core_lines_include_scaled_rates() {
        let report = report(vec![fn_cost("Foo.hot", 2400)]);
        let placement = PlacementTable {
            cores: 1,
            entries: vec![entry(ImageDeclRef::Actor(0), "Foo", 0)],
        };
        let at_24 = format_report(&report, &placement, 2.4).expect("ok");
        let at_1 = format_report(&report, &placement, 1.0).expect("ok");
        assert!(at_24.contains("ghz=2.4"), "got:\n{at_24}");
        assert!(at_1.contains("ghz=1"), "got:\n{at_1}");
        assert!(at_24.contains("turns_per_sec=1000000"), "got:\n{at_24}");
        assert!(
            at_1.contains("turns_per_sec=416666.6666666667"),
            "got:\n{at_1}"
        );
        assert!(at_24.contains("ms_per_turn=0.001"), "got:\n{at_24}");
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
        let text = format_report(&report, &placement, DEFAULT_GHZ).expect("ok");
        assert!(
            text.contains("turns_per_sec=n/a ms_per_turn=n/a"),
            "got:\n{text}"
        );
        assert!(text.contains("method=-"), "got:\n{text}");
    }
}
