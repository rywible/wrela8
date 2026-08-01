use std::collections::BTreeMap;

use crate::placement::PlacementTable;

use super::score::CostReport;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CoreCostReport {
    pub cores: Vec<CoreBucket>,
    pub shared_proxy_cycles: u64,
    pub placeables: Vec<PlaceableTurn>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CoreBucket {
    pub n: usize,
    pub proxy_cycles: u64,
    pub max_turn_proxy: u64,
    pub max_turn_method: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlaceableTurn {
    pub id: String,
    pub type_name: String,
    pub core: usize,
    pub turn_proxy: u64,
    pub method: Option<String>,
}

pub fn attribute_cores(
    report: &CostReport,
    placement: &PlacementTable,
) -> Result<CoreCostReport, String> {
    let skip_enqueue_clones = report.fns.iter().any(|f| f.key.starts_with("rt_enqueue "));

    let mut core_mass = vec![0u64; placement.cores];
    let mut shared_proxy_cycles = 0u64;
    let mut turn_by_type: BTreeMap<String, (u64, String)> = BTreeMap::new();

    for f in &report.fns {
        if skip_enqueue_clones && f.key.starts_with("__enqueue_") {
            continue;
        }

        match classify_target(&f.key, placement)? {
            AttrTarget::Core(n) => {
                core_mass[n] = core_mass[n].saturating_add(f.proxy_cycles);
            }
            AttrTarget::Shared => {
                shared_proxy_cycles = shared_proxy_cycles.saturating_add(f.proxy_cycles);
            }
        }

        if let Some((ty, method)) = parse_owner_method(&f.key) {
            if method != "init" {
                let cur = turn_by_type.entry(ty).or_insert((0, String::new()));
                if f.proxy_cycles > cur.0 {
                    *cur = (f.proxy_cycles, f.key.clone());
                }
            }
        }
    }

    let mut placeables = Vec::with_capacity(placement.entries.len());
    for e in &placement.entries {
        let (turn_proxy, method) = match turn_by_type.get(&e.type_name) {
            Some((c, m)) => (*c, Some(m.clone())),
            None => (0, None),
        };
        placeables.push(PlaceableTurn {
            id: e.id.render(),
            type_name: e.type_name.clone(),
            core: e.core,
            turn_proxy,
            method,
        });
    }

    let mut cores = Vec::with_capacity(placement.cores);
    for n in 0..placement.cores {
        let mut max_turn_proxy = 0u64;
        let mut max_turn_method = None;
        for p in &placeables {
            if p.core != n {
                continue;
            }
            if p.turn_proxy > max_turn_proxy {
                max_turn_proxy = p.turn_proxy;
                max_turn_method = p.method.clone();
            }
        }
        cores.push(CoreBucket {
            n,
            proxy_cycles: core_mass[n],
            max_turn_proxy,
            max_turn_method,
        });
    }

    Ok(CoreCostReport {
        cores,
        shared_proxy_cycles,
        placeables,
    })
}

pub(crate) enum AttrTarget {
    Core(usize),
    Shared,
}

pub(crate) fn classify_target(key: &str, placement: &PlacementTable) -> Result<AttrTarget, String> {
    if let Some(n) = parse_secondary_core(key) {
        if n >= placement.cores {
            return Err(format!(
                "cost attr: secondary entry key `{key}` names core {n}, but placement.cores={}",
                placement.cores
            ));
        }
        return Ok(AttrTarget::Core(n));
    }

    if let Some(ty) = key.strip_prefix("rt_enqueue ") {
        return core_or_shared(ty, placement);
    }

    if let Some((ty, _)) = parse_owner_method(key) {
        return core_or_shared(&ty, placement);
    }

    Ok(AttrTarget::Shared)
}

fn core_or_shared(type_name: &str, placement: &PlacementTable) -> Result<AttrTarget, String> {
    match core_of_type(placement, type_name)? {
        Some(n) => Ok(AttrTarget::Core(n)),
        None => Ok(AttrTarget::Shared),
    }
}

fn core_of_type(placement: &PlacementTable, type_name: &str) -> Result<Option<usize>, String> {
    let mut found: Option<usize> = None;
    for e in placement
        .entries
        .iter()
        .filter(|e| e.type_name == type_name)
    {
        match found {
            None => found = Some(e.core),
            Some(c) if c == e.core => {}
            Some(other) => {
                return Err(format!(
                    "cost attr: type `{type_name}` is placed on multiple cores \
                     ({other} and {}); cannot attribute",
                    e.core
                ));
            }
        }
    }
    Ok(found)
}

fn parse_secondary_core(key: &str) -> Option<usize> {
    if let Some(rest) = key.strip_prefix("rt_secondary_core_entry ") {
        return rest.parse().ok();
    }
    if let Some(rest) = key.strip_prefix("__wrela_secondary_entry_") {
        return rest.parse().ok();
    }
    None
}

fn parse_owner_method(key: &str) -> Option<(String, String)> {
    if key.contains(' ') || key.starts_with("__") {
        return None;
    }
    if let Some(rest) = key.strip_prefix("struct:") {
        let (ty, method) = rest.rsplit_once('.')?;
        if ty.is_empty() || method.is_empty() {
            return None;
        }
        return Some((ty.to_string(), method.to_string()));
    }
    if key.starts_with("fn:") || key.starts_with("method:") {
        return None;
    }
    let (ty, method) = key.split_once('.')?;
    if ty.is_empty() || method.is_empty() || method.contains('.') {
        return None;
    }
    Some((ty.to_string(), method.to_string()))
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::*;
    use crate::cost::score::FnCost;
    use crate::eval::image::ImageDeclRef;
    use crate::placement::{PlacementEntry, PlacementSource};

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
            owner_totals: BTreeMap::new(),
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

    fn placement(cores: usize, entries: Vec<PlacementEntry>) -> PlacementTable {
        PlacementTable { entries, cores }
    }

    #[test]
    fn attributes_methods_and_enqueue_to_placed_cores() {
        let placement = placement(
            2,
            vec![
                entry(ImageDeclRef::Actor(0), "Foo", 1),
                entry(ImageDeclRef::Driver(0), "Drv", 0),
            ],
        );
        let report = report(vec![
            fn_cost("Foo.hot", 100),
            fn_cost("Foo.cold", 10),
            fn_cost("Drv.on_irq", 50),
            fn_cost("rt_enqueue Foo", 20),
            fn_cost("__wrela_abort", 7),
            fn_cost("free_helper", 3),
            fn_cost("rt_run_one 0", 9),
        ]);
        let out = attribute_cores(&report, &placement).expect("attr");
        assert_eq!(out.cores.len(), 2);
        assert_eq!(out.cores[0].n, 0);
        assert_eq!(out.cores[0].proxy_cycles, 50);
        assert_eq!(out.cores[1].n, 1);
        assert_eq!(out.cores[1].proxy_cycles, 130);
        assert_eq!(out.shared_proxy_cycles, 7 + 3 + 9);
        assert_eq!(out.placeables.len(), 2);
        assert_eq!(out.placeables[0].id, "actor#0");
        assert_eq!(out.placeables[0].turn_proxy, 100);
        assert_eq!(out.placeables[0].method.as_deref(), Some("Foo.hot"));
        assert_eq!(out.placeables[1].id, "driver#0");
        assert_eq!(out.placeables[1].turn_proxy, 50);
        assert_eq!(out.placeables[1].method.as_deref(), Some("Drv.on_irq"));
        assert_eq!(out.cores[0].max_turn_proxy, 50);
        assert_eq!(out.cores[1].max_turn_proxy, 100);
    }

    #[test]
    fn dedups_enqueue_clones_when_rt_enqueue_present() {
        let placement = placement(1, vec![entry(ImageDeclRef::Actor(0), "Foo", 0)]);
        let report = report(vec![
            fn_cost("rt_enqueue Foo", 40),
            fn_cost("__enqueue_0", 40),
            fn_cost("Foo.turn", 5),
        ]);
        let out = attribute_cores(&report, &placement).expect("attr");
        assert_eq!(out.cores[0].proxy_cycles, 45);
        assert_eq!(out.shared_proxy_cycles, 0);
    }

    #[test]
    fn keeps_enqueue_clones_when_no_rt_enqueue_keys() {
        let placement = placement(1, vec![entry(ImageDeclRef::Actor(0), "Foo", 0)]);
        let report = report(vec![fn_cost("__enqueue_0", 40), fn_cost("Foo.turn", 5)]);
        let out = attribute_cores(&report, &placement).expect("attr");
        assert_eq!(out.cores[0].proxy_cycles, 5);
        assert_eq!(out.shared_proxy_cycles, 40);
    }

    #[test]
    fn secondary_entry_goes_to_named_core() {
        let placement = placement(3, vec![entry(ImageDeclRef::Actor(0), "Foo", 0)]);
        let report = report(vec![
            fn_cost("rt_secondary_core_entry 2", 33),
            fn_cost("__wrela_secondary_entry_1", 11),
            fn_cost("__wrela_rt_secondary_entry", 99),
        ]);
        let out = attribute_cores(&report, &placement).expect("attr");
        assert_eq!(out.cores[0].proxy_cycles, 0);
        assert_eq!(out.cores[1].proxy_cycles, 11);
        assert_eq!(out.cores[2].proxy_cycles, 33);
        assert_eq!(out.shared_proxy_cycles, 99);
    }

    #[test]
    fn split_type_across_cores_fails_closed() {
        let placement = placement(
            2,
            vec![
                entry(ImageDeclRef::Actor(0), "Foo", 0),
                entry(ImageDeclRef::Actor(1), "Foo", 1),
            ],
        );
        let report = report(vec![fn_cost("Foo.hot", 10)]);
        let err = attribute_cores(&report, &placement).expect_err("split");
        assert!(err.contains("multiple cores"), "got: {err}");
        assert!(err.contains("Foo"), "got: {err}");
    }

    #[test]
    fn turn_excludes_init_includes_driver_on() {
        let placement = placement(
            1,
            vec![
                entry(ImageDeclRef::Actor(0), "Store", 0),
                entry(ImageDeclRef::Driver(0), "Blk", 0),
            ],
        );
        let report = report(vec![
            fn_cost("Store.init", 1000),
            fn_cost("Store.get", 12),
            fn_cost("Store.put", 30),
            fn_cost("Blk.on_queue_irq", 44),
            fn_cost("Blk.init", 900),
        ]);
        let out = attribute_cores(&report, &placement).expect("attr");
        let store = out.placeables.iter().find(|p| p.id == "actor#0").unwrap();
        assert_eq!(store.turn_proxy, 30);
        assert_eq!(store.method.as_deref(), Some("Store.put"));
        let drv = out.placeables.iter().find(|p| p.id == "driver#0").unwrap();
        assert_eq!(drv.turn_proxy, 44);
        assert_eq!(drv.method.as_deref(), Some("Blk.on_queue_irq"));
        assert_eq!(out.cores[0].proxy_cycles, 1000 + 12 + 30 + 44 + 900);
        assert_eq!(out.cores[0].max_turn_proxy, 44);
    }

    #[test]
    fn struct_instance_keys_join_rendered_type_name() {
        let ty = "BlkDriver[DriverMode.Irq]";
        let placement = placement(1, vec![entry(ImageDeclRef::Driver(0), ty, 0)]);
        let hot = format!("struct:{ty}.on_queue_irq");
        let init = format!("struct:{ty}.init");
        let report = report(vec![fn_cost(&hot, 77), fn_cost(&init, 5)]);
        let out = attribute_cores(&report, &placement).expect("attr");
        assert_eq!(out.cores[0].proxy_cycles, 82);
        assert_eq!(out.placeables[0].turn_proxy, 77);
        assert_eq!(out.placeables[0].method.as_deref(), Some(hot.as_str()));
    }

    #[test]
    fn empty_core_buckets_still_emitted() {
        let placement = placement(3, vec![entry(ImageDeclRef::Actor(0), "Foo", 1)]);
        let report = report(vec![fn_cost("Foo.turn", 8)]);
        let out = attribute_cores(&report, &placement).expect("attr");
        assert_eq!(out.cores.len(), 3);
        assert_eq!(out.cores[0].proxy_cycles, 0);
        assert_eq!(out.cores[0].max_turn_proxy, 0);
        assert_eq!(out.cores[0].max_turn_method, None);
        assert_eq!(out.cores[1].proxy_cycles, 8);
        assert_eq!(out.cores[2].proxy_cycles, 0);
    }

    #[test]
    fn placeable_without_methods_has_zero_turn() {
        let placement = placement(1, vec![entry(ImageDeclRef::Actor(0), "Empty", 0)]);
        let report = report(vec![fn_cost("__wrela_abort", 1)]);
        let out = attribute_cores(&report, &placement).expect("attr");
        assert_eq!(out.placeables[0].turn_proxy, 0);
        assert_eq!(out.placeables[0].method, None);
        assert_eq!(out.cores[0].max_turn_proxy, 0);
        assert_eq!(out.shared_proxy_cycles, 1);
    }

    #[test]
    fn does_not_write_placement_work() {
        let placement = placement(1, vec![entry(ImageDeclRef::Actor(0), "Foo", 0)]);
        assert_eq!(placement.entries[0].work, 0);
        let report = report(vec![fn_cost("Foo.hot", 99)]);
        let _ = attribute_cores(&report, &placement).expect("attr");
        assert_eq!(placement.entries[0].work, 0);
        assert_eq!(placement.entries[0].work_source, "unproved");
    }
}
