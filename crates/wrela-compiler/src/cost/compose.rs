//! Multi-W cost compose (integrity Item J): flat row + method-grain `Σ f×s`.
//!
//! - `flat` = today's program total (`f≡1`, sum of per-fn schedules).
//! - Named W with a measured frequency vector: `Σ_fn f(fn) × s(fn)`.
//! - Pure cost-* cases without an `f` vector emit the flat row alone.

use std::collections::BTreeMap;
use std::path::Path;

use super::freq::{self, MethodFreq};
use super::score::{CostReport, FnCost};
use super::workload::{self, FLAT_NAME, WorkloadSet};

/// Workloads.toml set plus optional measured frequency vectors.
#[derive(Debug, Clone)]
pub struct WorkloadAttach {
    pub set: WorkloadSet,
    /// Workload name → method-grain counts.
    pub frequencies: BTreeMap<String, BTreeMap<String, u64>>,
}

impl WorkloadAttach {
    /// Load `bench/workloads.toml` and, when `source` has a sibling
    /// `lane1-freq.txt`, attach that measured vector.
    pub fn load_default_for(source: Option<&Path>) -> Result<Self, String> {
        let set = workload::load_default()?;
        let mut frequencies = BTreeMap::new();
        if let Some(path) = source {
            if let Some(freq_path) = freq::sibling_freq_path(path) {
                let f = freq::load_from_path(&freq_path)?;
                if set.weight(&f.workload).is_none() {
                    return Err(format!(
                        "freq: workload `{}` not in workloads.toml",
                        f.workload
                    ));
                }
                frequencies.insert(f.workload, f.counts);
            }
        }
        Ok(Self { set, frequencies })
    }

    /// Test helper: set + one measured vector.
    pub fn from_parts(set: WorkloadSet, freq: MethodFreq) -> Self {
        let mut frequencies = BTreeMap::new();
        frequencies.insert(freq.workload, freq.counts);
        Self { set, frequencies }
    }
}

/// Attach multi-W rows onto a scored report (mutates in place).
///
/// Always writes `flat = total_proxy_cycles`. For each measured frequency
/// whose name is in `attach.set`, writes `Σ f×s` and coverage
/// `(matched_hits, total_hits)`.
pub fn attach_workloads(report: &mut CostReport, attach: &WorkloadAttach) {
    report.workloads_digest = Some(attach.set.digest());
    report.workload_totals.clear();
    report.workload_coverage.clear();

    report
        .workload_totals
        .insert(FLAT_NAME.to_string(), report.total_proxy_cycles);

    for (name, counts) in &attach.frequencies {
        if attach.set.weight(name).is_none() {
            continue;
        }
        let (cycles, matched, total) = method_grain_fxs(&report.fns, counts);
        report.workload_totals.insert(name.clone(), cycles);
        report
            .workload_coverage
            .insert(name.clone(), (matched, total));
    }
}

/// `Σ f(fn)×s(fn)` over method keys present in both `freq` and scored fns.
/// Coverage: matched hit sum / total hit sum in `freq`.
pub fn method_grain_fxs(
    fns: &[FnCost],
    freq: &BTreeMap<String, u64>,
) -> (
    u64, /* cycles */
    u64, /* matched */
    u64, /* total */
) {
    let mut by_key: BTreeMap<&str, u64> = BTreeMap::new();
    for f in fns {
        by_key.insert(f.key.as_str(), f.proxy_cycles);
    }
    let mut cycles = 0u64;
    let mut matched = 0u64;
    let mut total = 0u64;
    for (key, &f) in freq {
        total = total.saturating_add(f);
        if let Some(&s) = by_key.get(key.as_str()) {
            cycles = cycles.saturating_add(f.saturating_mul(s));
            matched = matched.saturating_add(f);
        }
    }
    (cycles, matched, total)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cost::freq::parse as parse_freq;
    use crate::cost::score::FnCost;
    use crate::cost::workload::parse as parse_workloads;

    fn fn_cost(key: &str, cycles: u64) -> FnCost {
        FnCost {
            key: key.to_string(),
            owner: "app".to_string(),
            proxy_cycles: cycles,
            terms: BTreeMap::new(),
        }
    }

    fn bare_report(fns: Vec<FnCost>) -> CostReport {
        let total: u64 = fns.iter().map(|f| f.proxy_cycles).sum();
        CostReport {
            version: 2,
            digest: "t".to_string(),
            alu_ports: 2,
            mem_ports: 2,
            max_issue_per_cycle: 2,
            branch_penalty: 3,
            mem_reuse_window: 8,
            mem_working_set_cap: 4,
            total_proxy_cycles: total,
            owner_totals: BTreeMap::new(),
            fns,
            workloads_digest: None,
            workload_totals: BTreeMap::new(),
            workload_coverage: BTreeMap::new(),
        }
    }

    #[test]
    fn flat_row_equals_total_and_boot_actors_fxs() {
        let fns = vec![
            fn_cost("Ledger.mark", 88),
            fn_cost("Ledger.read_marks", 41),
            fn_cost("Worker.slow", 833),
            fn_cost("Worker.quick", 475),
            fn_cost("Worker.report", 463),
            fn_cost("__wrela_abort", 142),
        ];
        let mut report = bare_report(fns);
        let set = parse_workloads(
            r#"
[flat]
weight = 1
[boot-actors]
weight = 10
"#,
        )
        .expect("workloads");
        let freq = parse_freq(
            r#"
workload=boot-actors
Ledger.mark=3
Ledger.read_marks=1
Worker.slow=3
Worker.quick=2
Worker.report=2
"#,
        )
        .expect("freq");
        let attach = WorkloadAttach::from_parts(set, freq);
        attach_workloads(&mut report, &attach);

        assert_eq!(report.workload_totals["flat"], report.total_proxy_cycles);
        // 3*88 + 1*41 + 3*833 + 2*475 + 2*463 = 264+41+2499+950+926 = 4680
        assert_eq!(report.workload_totals["boot-actors"], 4680);
        assert_eq!(report.workload_coverage["boot-actors"], (11, 11));
        assert!(report.workloads_digest.is_some());
    }

    #[test]
    fn uncovered_hits_reduce_coverage() {
        let mut report = bare_report(vec![fn_cost("Foo.bar", 10)]);
        let set = parse_workloads("[flat]\nweight = 1\n[w]\nweight = 1\n").unwrap();
        let freq = parse_freq("workload=w\nFoo.bar=2\nMissing.m=3\n").unwrap();
        attach_workloads(&mut report, &WorkloadAttach::from_parts(set, freq));
        assert_eq!(report.workload_totals["w"], 20);
        assert_eq!(report.workload_coverage["w"], (2, 5));
    }
}
