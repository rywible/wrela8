use std::collections::BTreeMap;

use crate::placement::PlacementTable;

use super::attr::{AttrTarget, classify_target};
use super::rule::{CostRule, EmittedWord, MemClass};
use super::score::{CostReport, CrossExtra};
use super::sweep::SweepPoint;
use super::table::CostTable;

const TERM_DMB: &str = "dmb";
const TERM_SNOOP: &str = "snoop";
const TERM_SYSREG_FLUSH: &str = "sysreg_flush";

fn sweep_dim<'t>(table: &'t CostTable, term: &str) -> &'t str {
    &table
        .crosscore(term)
        .unwrap_or_else(|| {
            panic!(
                "cost table: [crosscore.{term}] is required by the cross-core cost model \
                 (plans/M20.md item G); a missing term would price it at 0"
            )
        })
        .sweep
}

fn swept(table: &CostTable, term: &str, point: &SweepPoint) -> u64 {
    point.get(sweep_dim(table, term))
}

fn swept_above_pinned(table: &CostTable, term: &str, point: &SweepPoint) -> u64 {
    let dim = sweep_dim(table, term);
    let pinned = table
        .sweep(dim)
        .unwrap_or_else(|| panic!("cost table: [sweep.{dim}] is required by [crosscore.{term}]"))
        .pinned;
    point.get(dim).saturating_sub(pinned)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Locality {
    Local,
    Remote,
    Unclassified,
}

impl Locality {
    pub fn is_remote(self) -> bool {
        matches!(self, Locality::Remote)
    }
}

pub fn accessing_core(fn_key: &str, placement: &PlacementTable) -> Option<usize> {
    match classify_target(fn_key, placement) {
        Ok(AttrTarget::Core(n)) => Some(n),
        Ok(AttrTarget::Shared) | Err(_) => None,
    }
}

pub fn classify_line(fn_key: &str, ew: &EmittedWord, placement: &PlacementTable) -> Locality {
    if !ew.rule.is_load() && !ew.rule.is_store() {
        return Locality::Local;
    }
    if matches!(ew.mem.map(|m| m.class), Some(MemClass::Stack)) {
        return Locality::Local;
    }
    let accessing = accessing_core(fn_key, placement);
    let peer_exists = match accessing {
        Some(n) => (0..placement.cores).any(|c| c != n),
        None => placement.cores > 1,
    };
    if !peer_exists {
        return Locality::Local;
    }
    if ew.rule == CostRule::LoadAcquire || ew.rule == CostRule::StoreRelease {
        return Locality::Remote;
    }
    Locality::Unclassified
}

const MSR_REGISTER_MASK: u32 = 0xFFF0_0000;
const MSR_REGISTER: u32 = 0xD510_0000;

pub fn system_word_flushes(word: u32) -> bool {
    word & MSR_REGISTER_MASK == MSR_REGISTER
}

pub fn charge(
    fn_key: &str,
    ew: &EmittedWord,
    table: &CostTable,
    point: &SweepPoint,
    placement: &PlacementTable,
) -> CrossExtra {
    let mut extra_cycles = 0u64;
    let mut serializes_window = false;

    match ew.rule {
        CostRule::Barrier => {
            extra_cycles = swept_above_pinned(table, TERM_DMB, point);
            serializes_window = true;
        }
        CostRule::System => {
            serializes_window = true;
            if system_word_flushes(ew.word) {
                extra_cycles = swept_above_pinned(table, TERM_SYSREG_FLUSH, point);
            }
        }
        CostRule::LoadAcquire | CostRule::StoreRelease => {
            if let Some(dim) = table.crosscore_extra_dim(ew.rule) {
                extra_cycles = point.get(dim);
            }
        }
        CostRule::Abort | CostRule::AbortVal => {}
        _ => {}
    }

    if classify_line(fn_key, ew, placement).is_remote() {
        extra_cycles = extra_cycles.saturating_add(swept(table, TERM_SNOOP, point));
    }

    CrossExtra {
        extra_cycles,
        serializes_window,
    }
}

pub fn ordering_rules() -> Vec<CostRule> {
    CostRule::ALL
        .iter()
        .copied()
        .filter(|r| r.is_crosscore())
        .collect()
}

pub type OrderingCounts = BTreeMap<(String, &'static str), u64>;

pub fn ordering_word_counts(report: &CostReport) -> OrderingCounts {
    ordering_word_counts_of(&report.fns)
}

pub fn ordering_word_counts_of(fns: &[crate::cost::score::FnCost]) -> OrderingCounts {
    let mut out: OrderingCounts = BTreeMap::new();
    for f in fns {
        for r in ordering_rules() {
            out.insert((f.key.clone(), r.as_str()), 0);
        }
        for (rule, n) in &f.terms {
            let Some(r) = CostRule::from_str(rule) else {
                continue;
            };
            if !r.is_crosscore() {
                continue;
            }
            *out.get_mut(&(f.key.clone(), r.as_str()))
                .expect("ordering rule slot") += n;
        }
    }
    out
}

pub fn ordering_word_totals(counts: &OrderingCounts) -> BTreeMap<&'static str, u64> {
    let mut out: BTreeMap<&'static str, u64> = ordering_rules()
        .iter()
        .map(|r| (r.as_str(), 0u64))
        .collect();
    for ((_, rule), n) in counts {
        *out.entry(rule).or_insert(0) += n;
    }
    out
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OrderingRemoval {
    pub fn_key: String,
    pub rule: &'static str,
    pub baseline: u64,
    pub candidate: u64,
}

impl OrderingRemoval {
    pub fn label(&self) -> String {
        format!(
            "ordering_words_removed:{}:{}:{}->{}",
            self.fn_key, self.rule, self.baseline, self.candidate
        )
    }
}

pub fn ordering_removals(
    baseline: &OrderingCounts,
    candidate: &OrderingCounts,
) -> Vec<OrderingRemoval> {
    let mut out = Vec::new();
    for ((fn_key, rule), &b) in baseline {
        let c = candidate
            .get(&(fn_key.clone(), *rule))
            .copied()
            .unwrap_or(0);
        if c < b {
            out.push(OrderingRemoval {
                fn_key: fn_key.clone(),
                rule,
                baseline: b,
                candidate: c,
            });
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::codegen::{CodegenFn, CodegenProgram};
    use crate::cost::rule::{FlagEffect, MemRef};
    use crate::cost::score::{FnCost, score_program_at};
    use crate::cost::table::load_default;
    use crate::eval::image::ImageDeclRef;
    use crate::placement::{PlacementEntry, PlacementSource};

    fn table() -> CostTable {
        load_default().expect("bench/a76-pi5.toml")
    }

    fn pinned() -> SweepPoint {
        SweepPoint::pinned(&table())
    }

    fn word(rule: CostRule, dst: Option<u8>, srcs: &[u8]) -> EmittedWord {
        EmittedWord::new(0, String::new(), rule, dst, srcs)
    }

    fn word_enc(enc: u32, rule: CostRule, dst: Option<u8>, srcs: &[u8]) -> EmittedWord {
        EmittedWord::new(enc, String::new(), rule, dst, srcs)
    }

    fn cold_load(rule: CostRule, seq: u64) -> EmittedWord {
        word(rule, Some(1), &[0]).with_mem(MemRef::cold_unique(seq))
    }

    fn prog(key: &str, code: Vec<EmittedWord>) -> CodegenProgram {
        let mut fns = BTreeMap::new();
        fns.insert(
            key.to_string(),
            CodegenFn {
                frame_size: 0,
                code,
                relocs: Vec::new(),
            },
        );
        CodegenProgram {
            fns,
            rodata: Vec::new(),
            ..Default::default()
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

    fn single_core() -> PlacementTable {
        PlacementTable {
            entries: vec![entry(ImageDeclRef::Actor(0), "Foo", 0)],
            cores: 1,
        }
    }

    fn three_cores() -> PlacementTable {
        PlacementTable {
            entries: vec![
                entry(ImageDeclRef::Actor(0), "Foo", 0),
                entry(ImageDeclRef::Actor(1), "Bar", 1),
            ],
            cores: 3,
        }
    }

    fn total(key: &str, code: Vec<EmittedWord>, placement: &PlacementTable) -> u64 {
        total_at(key, code, placement, &pinned())
    }

    fn total_at(
        key: &str,
        code: Vec<EmittedWord>,
        placement: &PlacementTable,
        point: &SweepPoint,
    ) -> u64 {
        score_program_at(&prog(key, code), &table(), placement, point)
            .expect("score")
            .total_proxy_cycles
    }

    #[test]
    fn a_remote_load_costs_more_than_the_identical_local_load() {
        let code = || vec![cold_load(CostRule::LoadAcquire, 0)];
        let local = total("Foo.turn", code(), &single_core());
        let remote = total("Foo.turn", code(), &three_cores());
        assert_eq!(local, 4, "one core: resolved loads use the L1 rank floor");
        assert_eq!(remote, 4 + 312, "peer core adds the swept snoop cost");
        assert!(
            remote > local,
            "remote {remote} must exceed local {local} at equal reuse distance"
        );
        let lo = pinned().with("snoop_cost", 0);
        assert_eq!(
            total_at("Foo.turn", code(), &three_cores(), &lo),
            4,
            "snoop_cost must reach the schedule through the sweep point"
        );
    }

    #[test]
    fn locality_classifies_exactly_what_placement_decides() {
        let three = three_cores();
        let one = single_core();

        assert_eq!(
            classify_line("Foo.turn", &cold_load(CostRule::LoadAcquire, 0), &three),
            Locality::Remote
        );
        assert_eq!(
            classify_line("Foo.turn", &cold_load(CostRule::LoadAcquire, 0), &one),
            Locality::Local
        );
        assert_eq!(
            classify_line("Foo.turn", &cold_load(CostRule::Load, 0), &three),
            Locality::Unclassified
        );
        let stack = word(CostRule::LoadAcquire, Some(1), &[31]).with_mem(MemRef::stack(8));
        assert_eq!(classify_line("Foo.turn", &stack, &three), Locality::Local);
        let rel = word(CostRule::StoreRelease, None, &[0]).with_mem(MemRef::cold_unique(1));
        assert_eq!(classify_line("Foo.turn", &rel, &three), Locality::Remote);
        assert_eq!(classify_line("Foo.turn", &rel, &one), Locality::Local);
        let str_cold = word(CostRule::Store, None, &[0]).with_mem(MemRef::cold_unique(2));
        assert_eq!(
            classify_line("Foo.turn", &str_cold, &three),
            Locality::Unclassified
        );
        let str_stack = word(CostRule::Store, None, &[31]).with_mem(MemRef::stack(8));
        assert_eq!(
            classify_line("Foo.turn", &str_stack, &three),
            Locality::Local
        );
        assert_eq!(
            classify_line("Foo.turn", &word(CostRule::Alu, Some(1), &[0]), &three),
            Locality::Local
        );
        assert!(Locality::Remote.is_remote());
        assert!(!Locality::Unclassified.is_remote());
        assert!(!Locality::Local.is_remote());
    }

    #[test]
    fn accessing_core_comes_from_sealed_placement() {
        let three = three_cores();
        assert_eq!(accessing_core("Foo.turn", &three), Some(0));
        assert_eq!(accessing_core("Bar.turn", &three), Some(1));
        assert_eq!(accessing_core("rt_enqueue Bar", &three), Some(1));
        assert_eq!(accessing_core("rt_secondary_core_entry 2", &three), Some(2));
        assert_eq!(accessing_core("__wrela_abort", &three), None);
        assert_eq!(accessing_core("free_helper", &three), None);
        let split = PlacementTable {
            entries: vec![
                entry(ImageDeclRef::Actor(0), "Foo", 0),
                entry(ImageDeclRef::Actor(1), "Foo", 1),
            ],
            cores: 2,
        };
        assert_eq!(accessing_core("Foo.turn", &split), None);
        for key in [
            "Foo.turn",
            "Bar.turn",
            "rt_secondary_core_entry 2",
            "__wrela_abort",
        ] {
            assert_eq!(
                classify_line(key, &cold_load(CostRule::LoadAcquire, 0), &three),
                Locality::Remote,
                "{key}"
            );
        }
    }

    #[test]
    fn a_dmb_costs_more_than_an_add() {
        let one = single_core();
        let load = || cold_load(CostRule::Load, 0);
        let with_add = total(
            "f",
            vec![load(), word(CostRule::Alu, Some(9), &[8, 8])],
            &one,
        );
        let with_dmb = total("f", vec![load(), word(CostRule::Barrier, None, &[])], &one);
        assert_eq!(with_add, 4, "an independent ADD hides under the load");
        assert_eq!(with_dmb, 5, "the barrier waits for the load to retire");
        assert!(with_dmb > with_add);
        let after = total(
            "f",
            vec![
                word(CostRule::Barrier, None, &[]),
                load(),
                word(CostRule::Alu, Some(9), &[8, 8]),
            ],
            &one,
        );
        assert_eq!(
            after,
            1 + 4,
            "the load cannot issue before the barrier retires"
        );
        let hi = pinned().with("dmb_cost", 64);
        assert_eq!(
            total_at("f", vec![word(CostRule::Barrier, None, &[])], &one, &hi),
            64,
            "dmb_cost must be read through the point, not pinned"
        );
        assert_eq!(
            total("f", vec![word(CostRule::Barrier, None, &[])], &one),
            1,
            "and the pinned point stays at the bracket's low end (freeze 1633)"
        );
    }

    #[test]
    fn ishst_and_ishld_share_one_swept_row() {
        let t = table();
        let p = pinned();
        let one = single_core();
        let ishst = word_enc(crate::encode::enc_dmb_ishst(), CostRule::Barrier, None, &[]);
        let ishld = word_enc(crate::encode::enc_dmb_ishld(), CostRule::Barrier, None, &[]);
        assert_eq!(
            charge("f", &ishst, &t, &p, &one),
            charge("f", &ishld, &t, &p, &one)
        );
        assert_eq!(sweep_dim(&t, TERM_DMB), "dmb_cost");
    }

    #[test]
    fn a_flushing_register_write_serializes_but_an_nzcv_write_does_not() {
        let one = single_core();
        let load = || cold_load(CostRule::Load, 0);
        let msr = word_enc(0xD518_2000, CostRule::System, None, &[0]);
        let nzcv = EmittedWord::new(0, String::new(), CostRule::Alu, None, &[5, 6])
            .with_flags(FlagEffect::Write);

        assert_eq!(
            total("f", vec![load(), nzcv], &one),
            4,
            "a renamed-flag write does not fence: it hides under the load"
        );
        assert_eq!(
            total("f", vec![load(), msr.clone()], &one),
            5,
            "an in-order system access waits for the window to drain"
        );

        let t = table();
        let hi = pinned().with("sysreg_flush_cost", 64);
        let brk = word_enc(crate::encode::enc_brk(1), CostRule::System, None, &[]);
        assert!(system_word_flushes(msr.word));
        assert!(
            !system_word_flushes(brk.word),
            "the async-dispatch BRK trap is not a system-register access"
        );
        assert_eq!(charge("f", &msr, &t, &hi, &one).extra_cycles, 63);
        assert_eq!(charge("f", &brk, &t, &hi, &one).extra_cycles, 0);
        assert!(charge("f", &msr, &t, &pinned(), &one).serializes_window);
        assert!(charge("f", &brk, &t, &pinned(), &one).serializes_window);
        assert!(!system_word_flushes(0xD530_0000));
    }

    #[test]
    fn no_emitted_system_word_carries_a_flush_side_effect() {
        assert!(
            !system_word_flushes(crate::encode::enc_brk(0xACD4)),
            "the only emitted CostRule::System word is the async-dispatch BRK trap"
        );
    }

    #[test]
    fn ordered_accesses_never_cost_less_than_their_plain_twin() {
        let one = single_core();
        let t = table();
        for (ordered, plain, dim) in [
            (CostRule::LoadAcquire, CostRule::Load, "load_acquire_cost"),
            (
                CostRule::StoreRelease,
                CostRule::Store,
                "store_release_cost",
            ),
        ] {
            let row = t.sweep(dim).expect("bracket");
            for value in [row.lo, row.pinned, row.hi] {
                let p = pinned().with(dim, value);
                let mk = |rule: CostRule| {
                    if rule.is_store() {
                        word(rule, None, &[31, 0]).with_mem(MemRef::stack(8))
                    } else {
                        word(rule, Some(1), &[31]).with_mem(MemRef::stack(8))
                    }
                };
                let a = total_at("f", vec![mk(ordered)], &one, &p);
                let b = total_at("f", vec![mk(plain)], &one, &p);
                assert!(
                    a >= b,
                    "{} at {dim}={value}: {a} < plain {} {b}",
                    ordered.as_str(),
                    plain.as_str()
                );
                assert_eq!(
                    a,
                    b + value,
                    "{} must be its twin plus the swept increment",
                    ordered.as_str()
                );
            }
            assert_eq!(row.pinned, row.lo);
        }
    }

    #[test]
    fn ordered_accesses_do_not_serialize_the_window() {
        let t = table();
        let p = pinned();
        let one = single_core();
        for rule in [CostRule::LoadAcquire, CostRule::StoreRelease] {
            let ew = word(rule, Some(1), &[31]).with_mem(MemRef::stack(8));
            assert!(
                !charge("f", &ew, &t, &p, &one).serializes_window,
                "{} must not fence",
                rule.as_str()
            );
        }
    }

    #[test]
    fn the_abort_branch_is_charged_but_no_handler_body_is() {
        let t = table();
        let p = pinned();
        let one = single_core();
        for rule in [CostRule::Abort, CostRule::AbortVal] {
            assert_eq!(
                charge("f", &word(rule, None, &[0]), &t, &p, &one),
                CrossExtra::default(),
                "{} must carry no cross-core charge",
                rule.as_str()
            );
        }

        let handler: Vec<EmittedWord> = (0..20)
            .map(|i| word(CostRule::Alu, Some((i % 8) + 1), &[9, 9]))
            .collect();
        let check_site = || {
            vec![
                word(CostRule::Alu, None, &[1, 2]),
                word(CostRule::Branch, None, &[]),
                word(CostRule::Abort, None, &[0]),
            ]
        };
        let build = |sites: usize| {
            let mut code = Vec::new();
            for _ in 0..sites {
                code.extend(check_site());
            }
            let mut fns = BTreeMap::new();
            fns.insert(
                "checker".to_string(),
                CodegenFn {
                    frame_size: 0,
                    code,
                    relocs: Vec::new(),
                },
            );
            fns.insert(
                "__wrela_abort".to_string(),
                CodegenFn {
                    frame_size: 0,
                    code: handler.clone(),
                    relocs: Vec::new(),
                },
            );
            score_program_at(
                &CodegenProgram {
                    fns,
                    rodata: Vec::new(),
                    ..Default::default()
                },
                &t,
                &one,
                &p,
            )
            .expect("score")
        };
        let one_site = build(1);
        let four_sites = build(4);
        let handler_cost = one_site
            .fns
            .iter()
            .find(|f| f.key == "__wrela_abort")
            .expect("handler")
            .proxy_cycles;
        assert!(handler_cost > 0, "the handler body is scored once");
        let growth = four_sites.total_proxy_cycles - one_site.total_proxy_cycles;
        assert!(
            growth < handler_cost,
            "3 more checks cost {growth}, which must stay below one handler body \
             {handler_cost} — the handler never returns, so it is not per-site"
        );
        assert_eq!(
            four_sites
                .fns
                .iter()
                .find(|f| f.key == "__wrela_abort")
                .expect("handler")
                .proxy_cycles,
            handler_cost
        );
    }

    #[test]
    fn ordering_rules_are_the_crosscore_priced_ones() {
        let names: Vec<&str> = ordering_rules().iter().map(|r| r.as_str()).collect();
        assert_eq!(
            names,
            vec!["load_acquire", "store_release", "barrier", "system"]
        );
    }

    #[test]
    fn ordering_word_counts_sum_over_fns() {
        let mk = |key: &str, pairs: &[(&str, u64)]| FnCost {
            key: key.to_string(),
            owner: "runtime".to_string(),
            frame_bytes: 0,
            proxy_cycles: 0,
            words: 0,
            terms: pairs.iter().map(|(k, v)| ((*k).to_string(), *v)).collect(),
        };
        let report = CostReport {
            version: 3,
            digest: "d".to_string(),
            provenance: "p".to_string(),
            provenance_summary: "s".to_string(),
            profile: "a76-pi5".to_string(),
            pipelines: 8,
            dispatch_mops: 4,
            dispatch_uops: 8,
            reorder_window: 128,
            total_proxy_cycles: 0,
            schedule_cycles: 0,
            footprint_cycles: 0,
            rank_cycles: 0,
            total_words: 0,
            sync_frame_max_bytes: 0,
            async_frame_total_bytes: 0,
            owner_totals: BTreeMap::new(),
            footprint: Vec::new(),
            fns: vec![
                mk("a", &[("barrier", 2), ("alu", 99), ("load_acquire", 1)]),
                mk("b", &[("barrier", 4), ("store_release", 3)]),
            ],
            workloads_digest: None,
            workload_totals: BTreeMap::new(),
            workload_coverage: BTreeMap::new(),
        };
        let counts = ordering_word_counts(&report);
        assert_eq!(counts[&("a".to_string(), "barrier")], 2);
        assert_eq!(counts[&("b".to_string(), "barrier")], 4);
        assert_eq!(counts[&("a".to_string(), "load_acquire")], 1);
        assert_eq!(counts[&("b".to_string(), "store_release")], 3);
        assert_eq!(
            counts[&("a".to_string(), "system")],
            0,
            "an absent rule reads 0, not missing"
        );
        assert_eq!(counts.len(), 8, "no non-ordering rule leaks in: 2 fns x 4");

        let totals = ordering_word_totals(&counts);
        assert_eq!(totals["barrier"], 6);
        assert_eq!(totals["load_acquire"], 1);
        assert_eq!(totals["store_release"], 3);
        assert_eq!(totals["system"], 0);
        assert_eq!(totals.len(), 4);
    }

    #[test]
    fn ordering_removals_fire_only_on_a_drop() {
        let counts = |b: u64, l: u64, s: u64, y: u64| -> OrderingCounts {
            BTreeMap::from([
                (("f".to_string(), "barrier"), b),
                (("f".to_string(), "load_acquire"), l),
                (("f".to_string(), "store_release"), s),
                (("f".to_string(), "system"), y),
            ])
        };
        let base = counts(6, 4, 6, 1);
        assert!(ordering_removals(&base, &base).is_empty(), "identity");
        assert!(
            ordering_removals(&base, &counts(7, 4, 6, 1)).is_empty(),
            "adding a barrier is not a removal"
        );
        let dropped = ordering_removals(&base, &counts(5, 4, 6, 1));
        assert_eq!(
            dropped,
            vec![OrderingRemoval {
                fn_key: "f".to_string(),
                rule: "barrier",
                baseline: 6,
                candidate: 5
            }]
        );
        assert_eq!(dropped[0].label(), "ordering_words_removed:f:barrier:6->5");
        assert_eq!(ordering_removals(&base, &counts(0, 0, 0, 0)).len(), 4);
        let mut missing = base.clone();
        missing.remove(&("f".to_string(), "barrier"));
        assert_eq!(
            ordering_removals(&base, &missing),
            vec![OrderingRemoval {
                fn_key: "f".to_string(),
                rule: "barrier",
                baseline: 6,
                candidate: 0
            }]
        );
    }

    #[test]
    fn moving_a_barrier_between_fns_is_a_removal_even_at_equal_totals() {
        let side = |hot: u64, cold: u64| -> OrderingCounts {
            BTreeMap::from([
                (("hot".to_string(), "barrier"), hot),
                (("cold".to_string(), "barrier"), cold),
            ])
        };
        let base = side(2, 0);
        let moved = side(1, 1);
        assert_eq!(
            ordering_word_totals(&base),
            ordering_word_totals(&moved),
            "the evasion's premise: the program-wide totals are identical"
        );
        let removals = ordering_removals(&base, &moved);
        assert_eq!(
            removals,
            vec![OrderingRemoval {
                fn_key: "hot".to_string(),
                rule: "barrier",
                baseline: 2,
                candidate: 1
            }]
        );

        let recreated = BTreeMap::from([(("new".to_string(), "barrier"), 2u64)]);
        assert_eq!(
            ordering_word_totals(&base),
            ordering_word_totals(&recreated)
        );
        assert_eq!(
            ordering_removals(&base, &recreated),
            vec![OrderingRemoval {
                fn_key: "hot".to_string(),
                rule: "barrier",
                baseline: 2,
                candidate: 0
            }]
        );
    }

    #[test]
    fn removal_sensitive_terms_pin_the_low_end() {
        let t = table();
        for dim in [
            "dmb_cost",
            "sysreg_flush_cost",
            "load_acquire_cost",
            "store_release_cost",
        ] {
            let row = t.sweep(dim).expect("bracket");
            assert!(row.removal_sensitive, "{dim} must be removal_sensitive");
            assert_eq!(row.pinned, row.lo, "{dim} must pin its low end");
            assert!(row.ambiguity.is_some(), "{dim} must record its ambiguity");
        }
        for term in [TERM_DMB, TERM_SNOOP, TERM_SYSREG_FLUSH] {
            assert!(t.crosscore(term).is_some(), "[crosscore.{term}] required");
        }
    }

    #[should_panic(expected = "[crosscore.snoop] is required")]
    #[test]
    fn a_missing_crosscore_term_fails_closed() {
        let text = std::fs::read_to_string(crate::cost::table::default_table_path())
            .expect("committed profile");
        let mut out = String::new();
        let mut skipping = false;
        for line in text.lines() {
            if line.starts_with('[') {
                skipping = line.starts_with("[crosscore.snoop]");
            }
            if !skipping {
                out.push_str(line);
                out.push('\n');
            }
        }
        let t = crate::cost::table::parse(&out).expect("parses without the snoop term");
        let _ = swept(&t, TERM_SNOOP, &SweepPoint::pinned(&t));
    }
}
