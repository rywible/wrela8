//! Stable text dump for `--stage=cost` (plans/M18.md items D+E).

use crate::codegen::CodegenProgram;

use super::score::{CostReport, score_program};
use super::table::CostTable;

/// Score `program` then format the cost dump.
pub fn dump(program: &CodegenProgram, table: &CostTable) -> Result<String, String> {
    let report = score_program(program, table)?;
    Ok(format_report(&report))
}

fn format_report(report: &CostReport) -> String {
    let mut out = String::new();
    push_line(
        &mut out,
        0,
        &format!(
            "Cost version={} digest={} issue_width={}",
            report.version, report.digest, report.issue_width
        ),
    );
    push_line(
        &mut out,
        1,
        "Assumptions ignore_cache=1 ignore_mispredict=1 target=isa_baseline",
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
    out
}

fn push_line(out: &mut String, depth: usize, line: &str) {
    out.push_str(&"  ".repeat(depth));
    out.push_str(line);
    out.push('\n');
}
