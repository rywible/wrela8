use std::collections::BTreeSet;

use super::rule::CostRule;

pub fn inventory_rows(rule: CostRule) -> &'static [u32] {
    match rule {
        CostRule::Alu | CostRule::MovWide | CostRule::Adrp => &[1],
        CostRule::Mul | CostRule::MulW | CostRule::MulHigh => &[3],
        CostRule::Sdiv | CostRule::Udiv => &[4],
        CostRule::Load => &[7, 9, 12, 29],
        CostRule::Store => &[8, 13, 30],
        CostRule::LoadAcquire => &[39, 7, 9],
        CostRule::StoreRelease => &[39, 8, 13],
        CostRule::Branch => &[21, 22, 23, 25],
        CostRule::Call => &[21],
        CostRule::Abort | CostRule::AbortVal => &[20],
        CostRule::Barrier => &[17],
        CostRule::System => &[19],
        CostRule::Neon => &[35],
    }
}

pub fn dimension_inventory_rows() -> Result<BTreeSet<u32>, String> {
    let census = crate::census::data();
    if census.cost_dimension_ids.is_empty() {
        return Err("tests/census.toml: cost_dimension.ids is empty".to_string());
    }
    let mut rows = BTreeSet::new();
    for (&id, name) in census
        .cost_dimension_ids
        .iter()
        .zip(&census.cost_dimension_names)
    {
        if name.trim().is_empty() {
            return Err(format!(
                "tests/census.toml: cost dimension row {id} has an empty name"
            ));
        }
        if !rows.insert(id) {
            return Err(format!(
                "tests/census.toml: cost dimension row {id} appears twice"
            ));
        }
    }
    Ok(rows)
}

fn check_dimension_inventory_rows(declared: &BTreeSet<u32>) -> Result<String, String> {
    let max = declared.iter().copied().max().unwrap_or(0);
    for n in 1..=max {
        if !declared.contains(&n) {
            return Err(format!(
                "tests/census.toml: cost dimension inventory is missing row {n} \
                 (rows must be dense 1..={max})"
            ));
        }
    }
    let mut claimed = BTreeSet::new();
    for &rule in CostRule::ALL {
        let rows = inventory_rows(rule);
        if rows.is_empty() {
            return Err(format!(
                "CostRule::{rule:?} (`{}`) names no cost-dimension row",
                rule.as_str()
            ));
        }
        for &r in rows {
            if !declared.contains(&r) {
                return Err(format!(
                    "CostRule::{rule:?} (`{}`) names cost-dimension row {r}, which does not \
                     exist in tests/census.toml",
                    rule.as_str()
                ));
            }
            claimed.insert(r);
        }
    }
    Ok(format!(
        "dimension inventory: {} row(s) in tests/census.toml, {} rule(s) accounted for across {} row(s)",
        declared.len(),
        CostRule::ALL.len(),
        claimed.len()
    ))
}

pub fn check_dimension_inventory() -> Result<String, String> {
    check_dimension_inventory_rows(&dimension_inventory_rows()?)
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::*;
    use crate::codegen::{BlockSpan, CodegenFn, CodegenProgram};
    use crate::cost::branch::{BlockCounts, BlockObs, BranchTerms};
    use crate::cost::bridge::BlockBridge;
    use crate::cost::compose::{block_grain_fxs, uncovered_charge};
    use crate::cost::footprint::{self, HotBlocks};
    use crate::cost::rule::{CostRule, EmittedWord, FlagEffect, MemRef};
    use crate::cost::score::{
        CostReport, basic_block_ranges, score_program_at, score_program_at_with_hot,
    };
    use crate::cost::sweep::{SweepPoint, endpoint_corners};
    use crate::cost::table::{CostTable, End, load_default};
    use crate::encode::enc_brk;
    use crate::eval::image::ImageDeclRef;
    use crate::placement::{PlacementEntry, PlacementSource, PlacementTable};

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

    fn load_stack(dst: u8, offset: u64) -> EmittedWord {
        word(CostRule::Load, Some(dst), &[31]).with_mem(MemRef::stack(offset))
    }

    fn load_stack_after(dst: u8, offset: u64, dep: u8) -> EmittedWord {
        word(CostRule::Load, Some(dst), &[31, dep]).with_mem(MemRef::stack(offset))
    }

    fn store_stack(offset: u64) -> EmittedWord {
        word(CostRule::Store, None, &[31, 0]).with_mem(MemRef::stack(offset))
    }

    fn serial_loads(offsets: &[u64], reload_first: bool) -> Vec<EmittedWord> {
        let mut code = Vec::new();
        for (i, &off) in offsets.iter().enumerate() {
            let dst = (i as u8) + 1;
            if i == 0 {
                code.push(load_stack(dst, off));
            } else {
                code.push(load_stack_after(dst, off, i as u8));
            }
            code.push(word(CostRule::Alu, Some(dst), &[dst, dst]));
        }
        if reload_first {
            code.push(load_stack_after(20, offsets[0], offsets.len() as u8));
        }
        code
    }

    fn cold_load(rule: CostRule, seq: u64) -> EmittedWord {
        word(rule, Some(1), &[0]).with_mem(MemRef::cold_unique(seq))
    }

    fn fn_of(code: Vec<EmittedWord>) -> CodegenFn {
        CodegenFn {
            frame_size: 0,
            code,
            relocs: Vec::new(),
        }
    }

    fn prog(key: &str, code: Vec<EmittedWord>) -> CodegenProgram {
        program(&[(key, code)])
    }

    fn program(fns: &[(&str, Vec<EmittedWord>)]) -> CodegenProgram {
        let mut map = BTreeMap::new();
        for (k, code) in fns {
            map.insert((*k).to_string(), fn_of(code.clone()));
        }
        CodegenProgram {
            fns: map,
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

    fn total_at(p: &CodegenProgram, place: &PlacementTable, point: &SweepPoint) -> u64 {
        score_program_at(p, &table(), place, point)
            .expect("score")
            .total_proxy_cycles
    }

    fn total(p: &CodegenProgram) -> u64 {
        total_at(p, &single_core(), &pinned())
    }

    fn cbz(byte_offset: i32) -> EmittedWord {
        let imm19 = ((byte_offset >> 2) as u32) & 0x7FFFF;
        word_enc(0xB400_0000 | (imm19 << 5), CostRule::Branch, None, &[0])
    }

    const REMOVAL_SENSITIVE: &[&str] = &[
        "call_overhead",
        "dmb_cost",
        "load_acquire_cost",
        "store_release_cost",
        "sysreg_flush_cost",
    ];

    #[test]
    fn every_swept_dimension_pins_its_declared_pessimistic_end() {
        let t = table();
        let dims = t.sweep_dimensions();
        assert!(!dims.is_empty(), "the profile declares no sweep box");
        let mut sensitive: Vec<&str> = Vec::new();
        for d in &dims {
            let row = t.sweep(d).expect("row");
            assert!(
                row.lo < row.hi,
                "`{d}`: degenerate bracket {}..{}",
                row.lo,
                row.hi
            );
            assert!(
                row.lo <= row.pinned && row.pinned <= row.hi,
                "`{d}`: pinned {} outside {}..{}",
                row.pinned,
                row.lo,
                row.hi
            );
            let want = match row.pessimistic {
                End::Lo => row.lo,
                End::Hi => row.hi,
            };
            assert_eq!(
                row.pinned, want,
                "`{d}` is not pinned at its declared pessimistic end"
            );
            if row.removal_sensitive {
                assert_eq!(
                    row.pessimistic,
                    End::Lo,
                    "`{d}`: removal_sensitive must pin low"
                );
                assert!(
                    row.ambiguity.is_some(),
                    "`{d}`: removal_sensitive must record its ambiguity (decision 1609)"
                );
                sensitive.push(d);
            }
        }
        assert_eq!(
            sensitive, REMOVAL_SENSITIVE,
            "the removal-sensitive set moved; item L's direction assertion exempts exactly these"
        );
    }

    fn witness(dim: &str, point: &SweepPoint) -> u64 {
        let t = table();
        let one = single_core();
        let footprint_lines = |lines: usize| {
            let words = lines * 16;
            let code = (0..words)
                .map(|_| word(CostRule::Alu, Some(1), &[0, 0]))
                .collect();
            footprint::compute(
                &program(&[("Foo.turn", code)]),
                &t,
                point,
                &one,
                HotBlocks::All,
            )
            .expect("footprint witness")
            .iter()
            .map(|budget| budget.charge)
            .sum()
        };
        match dim {
            "l2_latency" => footprint_lines(1025),
            "l3_latency" => footprint_lines(8193),
            "dram_latency" | "effective_l3_bytes" => footprint_lines(16_385),
            "store_to_load_forwarding" => total_at(
                &prog("f", vec![store_stack(0), load_stack(1, 0)]),
                &one,
                point,
            ),
            "mispredict_penalty" => {
                let code = vec![
                    cbz(8),
                    word(CostRule::Alu, Some(1), &[0, 0]),
                    word(CostRule::Alu, Some(2), &[0, 0]),
                ];
                let p = prog("f", code);
                let counts = |_k: &str, b: usize| Some(BlockObs::new(1, b as u64));
                score_program_at_with_hot(
                    &p,
                    &t,
                    &one,
                    point,
                    HotBlocks::All,
                    BlockCounts::Measured(&counts),
                )
                .expect("score")
                .total_proxy_cycles
            }
            "divide_x_latency" => total_at(
                &prog(
                    "f",
                    vec![
                        word(CostRule::Sdiv, Some(1), &[2, 3]),
                        word(CostRule::Alu, Some(4), &[1, 1]),
                    ],
                ),
                &one,
                point,
            ),
            "range_cross_penalty" => {
                let code: Vec<EmittedWord> = (0..5).map(|_| cbz(4)).collect();
                total_at(&prog("f", code), &one, point)
            }
            "tlb_walk_cost" => {
                let words = (49 * footprint::PAGE_BYTES / 4) as usize;
                let code: Vec<EmittedWord> = (0..words)
                    .map(|_| word(CostRule::Alu, Some(1), &[0, 0]))
                    .collect();
                let p = program(&[("Foo.turn", code)]);
                footprint::compute(&p, &t, point, &single_core(), HotBlocks::All)
                    .expect("footprint")
                    .iter()
                    .map(|b| b.charge)
                    .sum()
            }
            "dmb_cost" => total_at(
                &prog("f", vec![word(CostRule::Barrier, None, &[])]),
                &one,
                point,
            ),
            "sysreg_flush_cost" => total_at(
                &prog(
                    "f",
                    vec![word_enc(0xD518_2000, CostRule::System, None, &[0])],
                ),
                &one,
                point,
            ),
            "snoop_cost" => total_at(
                &prog("Foo.turn", vec![cold_load(CostRule::LoadAcquire, 0)]),
                &three_cores(),
                point,
            ),
            "load_acquire_cost" => total_at(
                &prog(
                    "f",
                    vec![word(CostRule::LoadAcquire, Some(1), &[31]).with_mem(MemRef::stack(8))],
                ),
                &one,
                point,
            ),
            "store_release_cost" => total_at(
                &prog(
                    "f",
                    vec![word(CostRule::StoreRelease, None, &[31, 0]).with_mem(MemRef::stack(8))],
                ),
                &one,
                point,
            ),
            "load_line_cross_penalty" => total_at(
                &prog(
                    "f",
                    vec![
                        EmittedWord::new(
                            0xF940_0000,
                            String::new(),
                            CostRule::Load,
                            Some(1),
                            &[31],
                        )
                        .with_mem(MemRef::stack(60)),
                    ],
                ),
                &one,
                point,
            ),
            "store_boundary_cross_penalty" => total_at(
                &prog(
                    "f",
                    vec![
                        EmittedWord::new(
                            0xF900_0000,
                            String::new(),
                            CostRule::Store,
                            None,
                            &[31, 0],
                        )
                        .with_mem(MemRef::stack(12)),
                    ],
                ),
                &one,
                point,
            ),
            "call_overhead" => total_at(
                &prog("f", vec![word(CostRule::Call, None, &[])]),
                &one,
                point,
            ),
            other => panic!(
                "no over-cost witness for swept dimension `{other}` — freeze 1623 requires one per \
                 dimension, so a new dimension must bring its witness"
            ),
        }
    }

    #[test]
    fn every_swept_dimension_is_live_and_moves_the_score_the_way_it_declares() {
        let t = table();
        let base = SweepPoint::pinned(&t);
        let mut lines = Vec::new();
        for d in t.sweep_dimensions() {
            let row = t.sweep(d).expect("row");
            let other_end = match row.pessimistic {
                End::Lo => row.hi,
                End::Hi => row.lo,
            };
            let at_pinned = witness(d, &base);
            let at_other = witness(d, &base.with(d, other_end));
            lines.push(format!(
                "  {d:<30} pinned={:<8} -> {at_pinned:<8} other_end={:<8} -> {at_other}",
                row.pinned, other_end
            ));
            assert_ne!(
                at_pinned, at_other,
                "`{d}`: the model scores the same at both bracket ends — the dimension is \
                 decorative, and its bracket protects nothing"
            );
            if row.removal_sensitive {
                assert!(
                    at_pinned < at_other,
                    "`{d}` is removal_sensitive and must pin the **cheap** end \
                     (pinned {at_pinned} >= other {at_other}); freeze 1633 forbids a charge that \
                     makes removing the construct profitable"
                );
            } else {
                assert!(
                    at_pinned > at_other,
                    "`{d}`: the pinned end scores {at_pinned}, cheaper than the other end \
                     {at_other} — the row declares `pessimistic = {}` but the model reads the \
                     dimension the other way. This is decision 1609 violated at the pinned point.",
                    format!("{:?}", row.pessimistic).to_lowercase()
                );
            }
        }
        eprintln!(
            "over-cost witnesses (pinned vs other bracket end):\n{}",
            lines.join("\n")
        );
    }

    #[test]
    fn removal_sensitive_dimensions_are_safe_by_refusal_not_by_coefficient() {
        use crate::cost::crosscore::{ordering_removals, ordering_rules};
        let t = table();
        for d in REMOVAL_SENSITIVE {
            let row = t.sweep(d).expect("row");
            assert!(row.removal_sensitive);
            assert_eq!(row.pinned, row.lo);
            let note = row.ambiguity.as_deref().unwrap_or("");
            assert!(!note.is_empty(), "`{d}` must state its ambiguity");
        }
        let refused: Vec<&str> = ordering_rules().iter().map(|r| r.as_str()).collect();
        for term in ["barrier", "load_acquire", "store_release", "system"] {
            assert!(
                refused.contains(&term),
                "freeze 1633's refusal set lost `{term}`; its low pin is then unprotected"
            );
        }
        assert!(
            !refused.contains(&"call"),
            "`call` is not an ordering word: refusing to remove one would refuse inlining outright"
        );
        let side = |barrier: u64| {
            BTreeMap::from([
                (("f".to_string(), "barrier"), barrier),
                (("f".to_string(), "load_acquire"), 0u64),
                (("f".to_string(), "store_release"), 0),
                (("f".to_string(), "system"), 0),
            ])
        };
        let base = side(1);
        let gone = side(0);
        assert!(
            !ordering_removals(&base, &gone).is_empty(),
            "freeze 1633: dropping a barrier must be refused structurally"
        );
        assert!(
            ordering_removals(&base, &base).is_empty(),
            "keeping every ordering word is not a removal"
        );
    }

    #[test]
    fn every_declared_dependence_channel_serializes() {
        let gpr_dep = vec![
            word(CostRule::Alu, Some(1), &[0, 0]),
            word(CostRule::Alu, Some(2), &[1, 1]),
        ];
        let gpr_ind = vec![
            word(CostRule::Alu, Some(1), &[0, 0]),
            word(CostRule::Alu, Some(2), &[3, 3]),
        ];

        let flags_dep = vec![
            EmittedWord::new(0, String::new(), CostRule::Alu, None, &[5, 6])
                .with_flags(FlagEffect::Write),
            EmittedWord::new(0, String::new(), CostRule::Alu, Some(7), &[8, 9])
                .with_flags(FlagEffect::Read),
        ];
        let flags_ind = vec![
            word(CostRule::Alu, None, &[5, 6]),
            word(CostRule::Alu, Some(7), &[8, 9]),
        ];

        let sp_write = EmittedWord::new(0xD100_43FF, String::new(), CostRule::Alu, Some(31), &[31]);
        let sp_dep = vec![sp_write.clone(), load_stack(1, 8)];
        let sp_ind = vec![
            EmittedWord::new(0xD100_4000, String::new(), CostRule::Alu, Some(0), &[0]),
            load_stack(1, 8),
        ];

        let cases: [(&str, Vec<EmittedWord>, Vec<EmittedWord>); 3] = [
            ("gpr", gpr_dep, gpr_ind),
            ("nzcv", flags_dep, flags_ind),
            ("sp", sp_dep, sp_ind),
        ];
        let mut lines = Vec::new();
        for (channel, dependent, independent) in cases {
            let d = total(&prog("f", dependent));
            let i = total(&prog("f", independent));
            lines.push(format!("  {channel:<10} dependent={d:<6} independent={i}"));
            assert!(
                d > i,
                "channel `{channel}`: a dependent pair costs {d}, no more than the independent \
                 pair's {i}. The model has stopped charging this dependence. If a change deleted \
                 the edge on a renaming argument, read decision 1616: renaming removes WAR and \
                 WAW, never RAW."
            );
        }
        eprintln!("dependence channels:\n{}", lines.join("\n"));
    }

    #[test]
    fn register_thirty_one_as_xzr_carries_no_dependence() {
        let sp_write = EmittedWord::new(0xD100_43FF, String::new(), CostRule::Alu, Some(31), &[31]);
        let xzr_store =
            EmittedWord::new(0xF900_0000, String::new(), CostRule::Store, None, &[0, 31])
                .with_mem(MemRef::cold_stable(0, 0));
        let with_producer = total(&prog("f", vec![sp_write, xzr_store.clone()]));
        let alone = total(&prog(
            "f",
            vec![
                EmittedWord::new(0xD100_40A5, String::new(), CostRule::Alu, Some(5), &[5]),
                xzr_store,
            ],
        ));
        assert_eq!(
            with_producer, alone,
            "an XZR read must not wait on an SP write: {with_producer} vs {alone}"
        );
    }

    fn monotonicity_shapes() -> Vec<(&'static str, Vec<EmittedWord>)> {
        vec![
            ("empty", Vec::new()),
            (
                "straight",
                vec![
                    word(CostRule::Alu, Some(1), &[0, 0]),
                    word(CostRule::Alu, Some(2), &[1, 1]),
                    load_stack(3, 8),
                ],
            ),
            (
                "mem_heavy",
                vec![
                    load_stack(1, 0),
                    load_stack(2, 64),
                    load_stack(3, 128),
                    load_stack(4, 192),
                ],
            ),
            (
                "mpipe",
                vec![
                    word(CostRule::MulHigh, Some(1), &[2, 3]),
                    word(CostRule::Mul, Some(4), &[5, 6]),
                    word(CostRule::Sdiv, Some(7), &[8, 9]),
                ],
            ),
            (
                "dense_branches",
                vec![
                    cbz(4),
                    word(CostRule::Alu, Some(1), &[0, 0]),
                    cbz(4),
                    word(CostRule::Alu, Some(2), &[0, 0]),
                    cbz(4),
                    word(CostRule::Alu, Some(3), &[0, 0]),
                    cbz(4),
                    cbz(4),
                    word(CostRule::Alu, Some(4), &[0, 0]),
                ],
            ),
            (
                "crosscore",
                vec![
                    word(CostRule::Barrier, None, &[]),
                    cold_load(CostRule::LoadAcquire, 0),
                    word(CostRule::StoreRelease, None, &[31, 0]).with_mem(MemRef::stack(8)),
                ],
            ),
        ]
    }

    fn dead_word() -> EmittedWord {
        word(CostRule::Alu, Some(20), &[21, 21])
    }

    #[test]
    fn a_dead_word_never_lowers_the_schedule_net_of_the_decidable_frontend_terms() {
        let t = table();
        let p = pinned();
        let place = single_core();
        let mut drops = Vec::new();
        let mut lines = Vec::new();
        for (name, code) in monotonicity_shapes() {
            let measure = |c: &[EmittedWord]| -> (u64, u64) {
                let gross = total_at(&prog("f", c.to_vec()), &place, &p);
                let fe = BranchTerms::compute("f", c, &t, &p, &BlockCounts::Flat)
                    .expect("terms")
                    .total_frontend_charge();
                (gross, fe)
            };
            let (base_total, base_fe) = measure(&code);
            let mut worst_gross = base_total;
            for at in 0..=code.len() {
                let mut grown = code.clone();
                grown.insert(at, dead_word());
                let (gross, fe) = measure(&grown);
                worst_gross = worst_gross.min(gross);
                assert!(
                    gross - fe >= base_total - base_fe,
                    "{name}@{at}: a dead word lowered the schedule net of §4.8: \
                     {base_total}-{base_fe} -> {gross}-{fe}"
                );
                if gross < base_total {
                    drops.push(format!(
                        "{name}@{at} {base_total}->{gross} (fe {base_fe}->{fe})"
                    ));
                    assert!(
                        fe < base_fe,
                        "{name}@{at}: the total fell {base_total} -> {gross} without the §4.8 \
                         front-end charge falling. Only rows 23 and 25 may be non-monotone \
                         (decision 1618); a drop anywhere else is a model that rewards adding work."
                    );
                    assert_eq!(
                        base_total - gross,
                        base_fe - fe,
                        "{name}@{at}: the drop is not exactly the §4.8 term's — something else \
                         moved"
                    );
                }
            }
            lines.push(format!(
                "  {name:<16} base {base_total:>4} (fe {base_fe}) worst over {} insertions: \
                 {worst_gross}",
                code.len() + 1
            ));
        }
        eprintln!(
            "monotonicity (dead word inserted at every position):\n{}\n  decision-1618 drops: {}",
            lines.join("\n"),
            if drops.is_empty() {
                "none".to_string()
            } else {
                drops.join(", ")
            }
        );
        assert!(
            !drops.is_empty(),
            "no insertion witnessed the decision-1618 drop — the oracle would pass vacuously and \
             would not notice if row 23 stopped being live"
        );
    }

    fn rename(key: &str) -> String {
        format!("{key}$fused")
    }

    fn box_points(t: &CostTable) -> Vec<SweepPoint> {
        let base = SweepPoint::pinned(t);
        let mut points = vec![base.clone()];
        for d in t.sweep_dimensions() {
            let row = t.sweep(d).expect("row");
            points.push(base.with(d, row.lo));
            points.push(base.with(d, row.hi));
        }
        points.extend(endpoint_corners(
            t,
            &["l3_latency", "snoop_cost", "dmb_cost", "call_overhead"],
        ));
        points
    }

    #[test]
    fn null_opt_renaming_every_fn_key_is_never_cheaper_at_any_box_point() {
        let t = table();
        let place = three_cores();
        let fns: Vec<(&str, Vec<EmittedWord>)> = vec![
            (
                "Foo.turn",
                vec![
                    cold_load(CostRule::LoadAcquire, 0),
                    word(CostRule::Barrier, None, &[]),
                    word(CostRule::Alu, Some(1), &[0, 0]),
                ],
            ),
            (
                "Bar.turn",
                vec![
                    load_stack(1, 0),
                    load_stack(2, 8),
                    word(CostRule::Call, None, &[]),
                    cbz(4),
                ],
            ),
            (
                "__wrela_abort",
                vec![
                    word(CostRule::Abort, None, &[]),
                    word(CostRule::Alu, Some(1), &[0, 0]),
                ],
            ),
        ];
        let renamed: Vec<(String, Vec<EmittedWord>)> =
            fns.iter().map(|(k, c)| (rename(k), c.clone())).collect();
        let base = program(&fns);
        let after = program(
            &renamed
                .iter()
                .map(|(k, c)| (k.as_str(), c.clone()))
                .collect::<Vec<_>>(),
        );

        let mut worst: Option<(String, u64, u64)> = None;
        for point in box_points(&t) {
            let b = total_at(&base, &place, &point);
            let a = total_at(&after, &place, &point);
            if a < b {
                worst = Some((point.label(), b, a));
                break;
            }
        }
        assert!(
            worst.is_none(),
            "a pure rename ranked cheaper at {:?} — the ruler rewards a semantically neutral \
             change",
            worst
        );
    }

    #[test]
    fn null_opt_rename_is_never_cheaper_with_block_grain_f_attached() {
        let t = table();
        let place = single_core();
        let fns: Vec<(&str, Vec<EmittedWord>)> = vec![
            (
                "Ledger.mark",
                vec![
                    load_stack(1, 0),
                    cbz(8),
                    word(CostRule::Alu, Some(2), &[1, 1]),
                    word(CostRule::Alu, Some(3), &[0, 0]),
                ],
            ),
            (
                "Worker.slow",
                vec![
                    word(CostRule::Sdiv, Some(1), &[2, 3]),
                    word(CostRule::Alu, Some(4), &[1, 1]),
                    cbz(4),
                ],
            ),
        ];

        let build = |renamed: bool| -> (CostReport, CodegenProgram, Vec<BlockSpan>) {
            let keyed: Vec<(String, Vec<EmittedWord>)> = fns
                .iter()
                .map(|(k, c)| {
                    (
                        if renamed { rename(k) } else { (*k).to_string() },
                        c.clone(),
                    )
                })
                .collect();
            let p = program(
                &keyed
                    .iter()
                    .map(|(k, c)| (k.as_str(), c.clone()))
                    .collect::<Vec<_>>(),
            );
            let mut spans = Vec::new();
            for (key, f) in &p.fns {
                for (i, (start, end)) in basic_block_ranges(&f.code).into_iter().enumerate() {
                    spans.push(BlockSpan {
                        fn_key: key.clone(),
                        block_index: i as u32,
                        id: spans.len() as u32,
                        word_start: start,
                        word_end: end,
                    });
                }
            }
            let r = score_program_at(&p, &t, &place, &SweepPoint::pinned(&t)).expect("score");
            (r, p, spans)
        };

        let (base_report, base_prog, base_spans) = build(false);
        let mut freq: BTreeMap<String, u64> = BTreeMap::new();
        for s in &base_spans {
            freq.insert(format!("{}#{}", s.fn_key, s.block_index), 100);
        }

        let base_bridge =
            BlockBridge::build(&base_prog, &base_spans, &t, &place).expect("baseline bridge");
        let base_m = block_grain_fxs(&base_report.fns, &base_bridge, &freq).expect("baseline fxs");

        let (ren_report, ren_prog, ren_spans) = build(true);
        let ren_bridge =
            BlockBridge::build(&ren_prog, &ren_spans, &t, &place).expect("renamed bridge");
        let ren_m = block_grain_fxs(&ren_report.fns, &ren_bridge, &freq).expect("renamed fxs");

        eprintln!(
            "null-opt block grain: baseline cycles={} matched={}/{} uncovered={} | \
             renamed cycles={} matched={}/{} uncovered={} (uncovered_charge={})",
            base_m.cycles,
            base_m.matched,
            base_m.total,
            base_m.uncovered_cycles,
            ren_m.cycles,
            ren_m.matched,
            ren_m.total,
            ren_m.uncovered_cycles,
            uncovered_charge(&base_report.fns)
        );
        assert_eq!(
            ren_m.matched, 0,
            "a full rename must resolve nothing — otherwise this fixture is not testing the \
             coverage rule"
        );
        assert!(
            ren_m.cycles >= base_m.cycles,
            "renaming every key lowered the block-grain measured row {} -> {}: the ruler is \
             rewarding measuring less (04 §5)",
            base_m.cycles,
            ren_m.cycles
        );
        assert_eq!(
            base_report.total_proxy_cycles, ren_report.total_proxy_cycles,
            "the flat row must be identical under a rename"
        );
    }

    #[test]
    fn cliff_witnesses_rank_in_the_expected_direction() {
        let t = table();
        let p = pinned();
        let one = single_core();
        let mut lines = Vec::new();

        let text_words = |bytes: u64| -> Vec<EmittedWord> {
            (0..(bytes / 4) as usize)
                .map(|_| word(CostRule::Alu, Some(1), &[0, 0]))
                .collect()
        };
        let budget = |bytes: u64| -> (u64, u64) {
            let prog = program(&[("Foo.turn", text_words(bytes))]);
            let b = footprint::compute(&prog, &t, &p, &one, HotBlocks::All).expect("footprint");
            (b[0].charge, b[0].fetched_text_bytes)
        };
        let (under, under_bytes) = budget(60 * 1024);
        let (over, over_bytes) = budget(68 * 1024);
        lines.push(format!(
            "  l1i-capacity      {under_bytes} B -> charge {under} | {over_bytes} B -> charge {over}"
        ));
        assert!(
            over > under,
            "L1I capacity cliff is not live: {under} -> {over}"
        );

        let set_stride = 64u64 * 256;
        let conflict: Vec<u64> = (0..5).map(|k| k * set_stride).collect();
        let spread: Vec<u64> = (0..5).map(|k| k * 64).collect();
        let c = total_at(&prog("f", serial_loads(&conflict, true)), &one, &p);
        let n = total_at(&prog("f", serial_loads(&spread, true)), &one, &p);
        lines.push(format!(
            "  l1d-static-order  conflicting={c} vs spread={n} (rank-neutral)"
        ));
        assert_eq!(
            c, n,
            "a static block order must not be mistaken for a D-cache execution trace"
        );

        let itlb = |pages: u64| -> (u64, u64) {
            let prog = program(&[("Foo.turn", text_words(pages * footprint::PAGE_BYTES))]);
            let b = footprint::compute(&prog, &t, &p, &one, HotBlocks::All).expect("footprint");
            (b[0].charge, b[0].over_itlb_pages)
        };
        let (p48, over48) = itlb(48);
        let (p49, over49) = itlb(49);
        lines.push(format!(
            "  itlb-span         48 pages -> charge {p48} (over {over48}) | 49 -> {p49} (over {over49})"
        ));
        assert!(p49 > p48, "I-TLB span cliff is not live: {p48} -> {p49}");
        assert_eq!(over48, 0);
        assert_eq!(over49, 1);

        let dense: Vec<EmittedWord> = (0..5).map(|_| cbz(4)).collect();
        let mut spread: Vec<EmittedWord> = Vec::new();
        for _ in 0..5 {
            spread.push(cbz(4));
            for _ in 0..8 {
                spread.push(word(CostRule::Alu, Some(1), &[0, 0]));
            }
        }
        let d_fe = BranchTerms::compute("f", &dense, &t, &p, &BlockCounts::Flat)
            .expect("terms")
            .total_frontend_charge();
        let s_fe = BranchTerms::compute("f", &spread, &t, &p, &BlockCounts::Flat)
            .expect("terms")
            .total_frontend_charge();
        lines.push(format!(
            "  branch-density    5-in-32B fe={d_fe} vs spread fe={s_fe}"
        ));
        assert!(d_fe > s_fe, "row 23 is not live: {d_fe} vs {s_fe}");

        let code = || vec![cold_load(CostRule::LoadAcquire, 0)];
        let local = total_at(&prog("Foo.turn", code()), &one, &p);
        let remote = total_at(&prog("Foo.turn", code()), &three_cores(), &p);
        lines.push(format!("  local-vs-remote   local={local} remote={remote}"));
        assert!(remote > local, "row 18 is not live: {local} vs {remote}");

        eprintln!("cliff witnesses:\n{}", lines.join("\n"));
    }

    #[test]
    fn a_candidate_that_wins_only_at_the_most_generous_corner_is_vetoed() {
        let t = table();
        let one = single_core();
        let baseline = prog(
            "f",
            vec![
                word(CostRule::Barrier, None, &[]),
                word(CostRule::Alu, Some(1), &[0, 0]),
            ],
        );
        let candidate = prog(
            "f",
            vec![
                word(CostRule::Alu, Some(1), &[0, 0]),
                word(CostRule::Alu, Some(2), &[1, 1]),
                word(CostRule::Alu, Some(3), &[2, 2]),
                word(CostRule::Alu, Some(4), &[3, 3]),
                word(CostRule::Alu, Some(5), &[4, 4]),
            ],
        );

        let generous = pinned().with("dmb_cost", t.sweep("dmb_cost").expect("row").hi);
        let b_gen = total_at(&baseline, &one, &generous);
        let c_gen = total_at(&candidate, &one, &generous);
        assert!(
            c_gen < b_gen,
            "the fixture must actually win somewhere, or it is not an ∃-cheat: {c_gen} vs {b_gen}"
        );

        let mut flip: Option<(String, u64, u64)> = None;
        for point in box_points(&t) {
            let b = total_at(&baseline, &one, &point);
            let c = total_at(&candidate, &one, &point);
            if c >= b {
                flip = Some((point.label_over(&["dmb_cost"]), b, c));
                break;
            }
        }
        let (label, b, c) = flip.expect(
            "the ∃-cheat was not caught: no box point ranked the candidate at or above baseline",
        );
        eprintln!("∃-cheat vetoed at {label}: baseline={b} candidate={c}");
        assert!(c >= b);
    }

    fn body(n: usize, seed: u8) -> Vec<EmittedWord> {
        (0..n)
            .map(|i| {
                let d = seed.wrapping_add((i % 12) as u8).wrapping_add(1);
                word(CostRule::Alu, Some(d), &[d, d])
            })
            .collect()
    }

    #[test]
    fn synthetic_partition_adversary_fusion_versus_small_fns() {
        let one = single_core();
        let p = pinned();
        const TOTAL_WORDS: usize = 128;

        let score_split = |n: usize, with_calls: bool| -> (u64, u64) {
            let per = TOTAL_WORDS / n;
            let mut fns: Vec<(String, Vec<EmittedWord>)> = Vec::new();
            let mut caller = Vec::new();
            for k in 0..n {
                fns.push((format!("leaf{k:02}"), body(per, k as u8)));
                caller.push(word(CostRule::Call, None, &[]));
            }
            if with_calls && n > 1 {
                fns.push(("caller".to_string(), caller));
            }
            let prog = program(
                &fns.iter()
                    .map(|(k, c)| (k.as_str(), c.clone()))
                    .collect::<Vec<_>>(),
            );
            let r = score_program_at(&prog, &table(), &one, &p).expect("score");
            (r.total_proxy_cycles, r.total_words)
        };

        let (fused, fused_words) = score_split(1, false);
        let mut lines = Vec::new();
        let mut pure_gaps = Vec::new();
        let mut call_gaps = Vec::new();
        for n in [1usize, 2, 4, 8, 16, 32] {
            let (pure, pure_words) = score_split(n, false);
            let (calls, call_words) = score_split(n, true);
            lines.push(format!(
                "  N={n:<3} pure-repartition {pure_words:>4} words -> {pure:>4} cycles \
                 (vs fused {fused}: {:+}) | with calls {call_words:>4} words -> {calls:>4} cycles \
                 (vs fused: {:+})",
                pure as i64 - fused as i64,
                calls as i64 - fused as i64,
            ));
            pure_gaps.push((n, pure as i64 - fused as i64));
            call_gaps.push((n, calls as i64 - fused as i64));
        }
        eprintln!(
            "partition adversary (fusion), {TOTAL_WORDS} identical ALU words, fused = 1 fn / \
             {fused_words} words -> {fused} cycles:\n{}",
            lines.join("\n")
        );

        assert!(
            pure_gaps.iter().all(|&(_, g)| g >= 0),
            "the bias reversed direction: splitting now costs *less* than fusing, which would be \
             an even worse null-opt hole. Gaps: {pure_gaps:?}"
        );
        assert!(
            pure_gaps.last().expect("gaps").1 > 0,
            "FUSION NO LONGER READS AS A WIN on the pure repartition. That is the hole item L \
             recorded closing — good news, but it must be recorded deliberately rather than \
             discovered by a passing test: update plans/M20.md item L's finding."
        );
        for w in pure_gaps.windows(2) {
            assert!(
                w[1].1 >= w[0].1,
                "the fusion bias is not monotone in the partition count: {:?} then {:?}",
                w[0],
                w[1]
            );
        }
        for (n, pure) in &pure_gaps {
            let calls = call_gaps.iter().find(|(m, _)| m == n).expect("paired").1;
            assert!(
                calls >= *pure,
                "N={n}: adding call words did not raise the split's total ({calls} vs {pure})"
            );
        }
    }

    #[test]
    fn synthetic_partition_adversary_loop_body_times_one_versus_times_k() {
        let one = single_core();
        let p = pinned();
        let per = 8usize;
        let mut lines = Vec::new();
        let mut totals = Vec::new();
        for k in [1usize, 2, 4, 8] {
            let mut code = Vec::new();
            for i in 0..k {
                code.extend(body(per, i as u8));
            }
            code.push(cbz(-((code.len() as i32) * 4)));
            let fe = BranchTerms::compute("loop", &code, &table(), &p, &BlockCounts::Flat)
                .expect("terms")
                .total_frontend_charge();
            let r = score_program_at(&prog("loop", code), &table(), &one, &p).expect("score");
            lines.push(format!(
                "  x{k:<2} {:>4} words -> {:>5} cycles (§4.8 {fe}) \
                 ({:>6.2} cycles/word, {:>6.2} cycles/trip-body)",
                r.total_words,
                r.total_proxy_cycles,
                r.total_proxy_cycles as f64 / r.total_words as f64,
                r.total_proxy_cycles as f64 / k as f64,
            ));
            totals.push((k, r.total_proxy_cycles));
        }
        eprintln!("partition adversary (unroll):\n{}", lines.join("\n"));
        let one_copy = totals[0].1;
        for (k, tot) in &totals {
            assert!(
                *tot >= one_copy,
                "unrolling x{k} scored {tot} < x1 {one_copy}: the ruler pays nothing for growth"
            );
        }
    }

    #[test]
    fn the_provenance_digest_moves_when_any_single_row_changes_tier() {
        let text = std::fs::read_to_string(crate::cost::table::default_table_path())
            .expect("committed profile");
        let base = crate::cost::table::parse(&text).expect("parse");
        let mut checked = 0usize;
        for (i, line) in text.lines().enumerate() {
            let trimmed = line.trim();
            if !trimmed.starts_with("tier = ") {
                continue;
            }
            let replacement = match trimmed {
                "tier = \"T1\"" => "tier = \"T2\"",
                "tier = \"T2\"" => "tier = \"T3\"",
                "tier = \"T3\"" => "tier = \"T4\"",
                "tier = \"T4\"" => "tier = \"T1\"",
                _ => continue,
            };
            let edited: String = text
                .lines()
                .enumerate()
                .map(|(j, l)| {
                    if j == i {
                        replacement.to_string()
                    } else {
                        l.to_string()
                    }
                })
                .collect::<Vec<_>>()
                .join("\n");
            let Ok(after) = crate::cost::table::parse(&edited) else {
                continue;
            };
            assert_ne!(
                base.provenance_digest(),
                after.provenance_digest(),
                "line {}: changing a tier left the provenance digest unmoved",
                i + 1
            );
            checked += 1;
        }
        assert!(
            checked > 20,
            "only {checked} tier rows were exercised — the profile should carry dozens"
        );
    }

    #[test]
    fn every_cost_rule_names_an_inventory_row_that_exists_in_the_census() {
        let summary = check_dimension_inventory().expect("cost dimension census");
        eprintln!("{summary}");
        let rows = dimension_inventory_rows().expect("rows");
        assert!(
            rows.contains(&39),
            "row 39 (the ordered accesses) must exist"
        );
        assert!(
            !rows.contains(&40),
            "the census grew a row this check does not know about"
        );
    }

    #[test]
    fn the_inventory_check_fails_closed_on_a_missing_row() {
        let mut pruned = dimension_inventory_rows().expect("rows");
        pruned.remove(&17);
        let e = check_dimension_inventory_rows(&pruned).expect_err("a deleted row must be refused");
        assert!(
            e.contains("17"),
            "the refusal must name the missing row, got: {e}"
        );
        let e = check_dimension_inventory_rows(&BTreeSet::new()).expect_err("empty inventory");
        assert!(e.contains("row"), "got: {e}");
    }

    #[test]
    fn the_inventory_accounts_for_every_priced_rule() {
        let t = table();
        for &rule in CostRule::ALL {
            assert!(
                !inventory_rows(rule).is_empty(),
                "CostRule::{rule:?} has no inventory row"
            );
            assert!(
                t.latency(rule) > 0 || rule.is_crosscore(),
                "CostRule::{rule:?} prices at zero"
            );
        }
        assert!(!crate::cost::crosscore::system_word_flushes(enc_brk(1)));
    }
}
