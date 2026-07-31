//! Oracles on **the ruler itself** (plans/M20.md item L; 04 §5 "Ruler
//! oracles"). Nothing here tests an emission; everything here tests
//! whether the model that ranks emissions can be gamed, and whether the
//! numbers it is built from are the ends of their brackets they claim to
//! be.
//!
//! This module carries the load hardware validation used to carry. Freeze
//! 1631 puts physical measurement permanently out of scope, so **nothing
//! downstream catches an optimistic table** — item L's over-cost oracle is
//! the last line, and decision 1616 is the proof that the failure class is
//! real rather than hypothetical (a shipped under-cost, caught by a human
//! reading and by no test).
//!
//! ## What is asserted, and what is deliberately not
//!
//! **Monotonicity is reworded here, because the plan's wording is false.**
//! Item L's text says "a dead instruction never lowers a schedule,
//! footprint cost, or mispredict charge". Decision 1618 records why the
//! *schedule* half cannot hold once inventory row 23 is live: a padding
//! word genuinely reduces branch density inside an aligned 32-byte fetch
//! region, and that is genuinely cheaper on A76 — it is the whole content
//! of SOG §4.8's rule, not a modelling artefact
//! (`branch::tests::a_padding_word_can_lower_the_density_charge_and_that_is_real`
//! pins the counter-example). The claim asserted instead, in three
//! separately-checkable halves:
//!
//! > Appending a dead, independent word never lowers the **mispredict**
//! > charge, never lowers the per-core **footprint** charge, and never
//! > lowers the schedule **net of the two decidable §4.8 front-end terms**
//! > (rows 23 and 25). The gross schedule *can* fall, and when it does the
//! > entire drop is accounted for by those two terms — never by the
//! > scoreboard, the memory model, or the cross-core model.
//!
//! The third clause is the one that keeps the reword honest: it is not
//! "monotonicity except where it fails", it is a bound on where the
//! failure may live, which fails closed if the non-monotonicity ever
//! spreads to a term that has no §4.8 rule behind it.
//!
//! ## The over-cost rule, per dimension (decision 1609 / freeze 1623)
//!
//! `sweep::tests::pinned_point_is_the_pessimistic_corner` already asserts
//! `pinned == the declared pessimistic end` over the whole box, and
//! `table::parse` asserts it at load. Both check the profile against
//! **its own declaration**. Neither checks whether that declaration is
//! *true of the model* — a row could declare `pessimistic = "hi"` while
//! the model reads the dimension in a direction that makes `hi` the cheap
//! end, and every existing assertion would still pass.
//!
//! [`tests::every_swept_dimension_is_live_and_moves_the_score_the_way_it_declares`]
//! closes that: for each dimension it scores a witness that exercises the
//! term at both bracket ends, and requires
//!
//! - the two scores to **differ** — a dimension the model never reads is
//!   decorative, and its bracket protects nothing; and
//! - the **pinned** end to be the more expensive one.
//!
//! …except for the five `removal_sensitive` dimensions (`dmb_cost`,
//! `sysreg_flush_cost`, `load_acquire_cost`, `store_release_cost`,
//! `call_overhead`), which pin their bracket's **low** end deliberately
//! (decision 1609's second clause, freeze 1633: for a penalty whose
//! *removal* is the win, a larger charge over-credits deleting the
//! construct). Asserting "pinned is the expensive end" over those would be
//! asserting the opposite of what the profile says and what the freeze
//! requires, so they are asserted the other way — pinned is the *cheap*
//! end — and their safety comes from the structural refusal in
//! [`super::crosscore::ordering_removals`], not from a coefficient.
//!
//! ## Freeze 1632: the inventory is machine-checked
//!
//! [`inventory_rows`] maps every `CostRule` to the dimension-inventory
//! rows in `plans/M20.md` that account for it. The match is exhaustive
//! and carries no wildcard, so **adding a `CostRule` variant without
//! giving it an inventory row does not compile**; `xtask check` then
//! checks that every row number named here actually exists in the plan's
//! inventory table, so deleting a row from the table is caught too.

use std::collections::BTreeSet;

use super::rule::CostRule;

/// Dimension-inventory rows in `plans/M20.md` that account for `rule`.
///
/// Freeze 1632: "a dimension may be omitted only with a reason in this
/// table — never by having been forgotten." A `CostRule` is a cost the
/// emitted stream incurs, so every variant must name at least one row.
/// The match is exhaustive on purpose: a new variant is a compile error
/// until it is accounted for.
pub fn inventory_rows(rule: CostRule) -> &'static [u32] {
    match rule {
        // Row 1: integer ALU latency / throughput / port. `MOVZ`/`MOVK`
        // (move immed) and `ADR`/`ADRP` (address generation) are rows of
        // the same SOG §3.4 group at latency 1, throughput 3, port I, so
        // they are the same inventory dimension.
        CostRule::Alu | CostRule::MovWide | CostRule::Adrp => &[1],
        // Row 3: multiply / MAC / multiply-high plus the M-pipe stalls.
        CostRule::Mul | CostRule::MulHigh => &[3],
        // Row 4: divide range + pipe blocking.
        CostRule::Sdiv | CostRule::Udiv => &[4],
        // Rows 7 (L1D-hit latency) and 9 (the miss hierarchy); a load also
        // carries row 12's reuse distance and row 29's §4.5 crossing.
        CostRule::Load => &[7, 9, 12, 29],
        // Row 8 (address+data split, V-pipe contention), row 13 (store
        // buffer + forwarding), row 30 (§4.5 16 B crossing).
        CostRule::Store => &[8, 13, 30],
        // Row 39: the ordered accesses, added by item D's census. They
        // take their plain twin's memory path, so they carry those rows
        // too.
        CostRule::LoadAcquire => &[39, 7, 9],
        CostRule::StoreRelease => &[39, 8, 13],
        // Rows 21 (branch latency/port), 22 (bias-scaled mispredict), 23
        // (density per aligned 32 B), 25 (loop fitting one region).
        CostRule::Branch => &[21, 22, 23, 25],
        // `BL` is the branch table's "branch and link, immed" row; its
        // callee-side residual is the swept `call_overhead`.
        CostRule::Call => &[21],
        // Row 20: exception / abort path entry.
        CostRule::Abort | CostRule::AbortVal => &[20],
        // Row 17: `DMB(ishst/ishld)`.
        CostRule::Barrier => &[17],
        // Row 19: system-register / trap words.
        CostRule::System => &[19],
        // Row 35: FP/ASIMD data-processing, one coarse row, not expanded
        // (freeze 1630).
        CostRule::Neon => &[35],
    }
}

/// Row numbers of the dimension-inventory table in `plans/M20.md`.
///
/// The table is the "## Cost dimension inventory" section's markdown rows
/// whose first cell is a number. Parsed rather than restated so the plan
/// stays the single source of the list.
pub fn plan_inventory_rows(plan_text: &str) -> Result<BTreeSet<u32>, String> {
    let mut rows = BTreeSet::new();
    let mut inside = false;
    for line in plan_text.lines() {
        if line.starts_with("## ") {
            inside = line.trim() == "## Cost dimension inventory";
            continue;
        }
        if !inside || !line.starts_with('|') {
            continue;
        }
        let first = line
            .trim_start_matches('|')
            .split('|')
            .next()
            .unwrap_or("")
            .trim();
        if let Ok(n) = first.parse::<u32>()
            && !rows.insert(n)
        {
            return Err(format!(
                "plans/M20.md: dimension inventory row {n} appears twice"
            ));
        }
    }
    if rows.is_empty() {
        return Err(
            "plans/M20.md: no `## Cost dimension inventory` table found (freeze 1632's list)"
                .to_string(),
        );
    }
    Ok(rows)
}

/// Freeze 1632, as a check `xtask check` runs: every `CostRule` names at
/// least one inventory row, and every row it names exists in the plan's
/// table. Returns a one-line summary on success.
///
/// Fails closed in both directions — a rule with no row, and a row number
/// that has been deleted from (or never added to) the plan.
pub fn check_dimension_inventory(plan_text: &str) -> Result<String, String> {
    let declared = plan_inventory_rows(plan_text)?;
    // The inventory is numbered from 1 and dense: a hole means a row was
    // deleted rather than superseded, which is exactly the silent omission
    // freeze 1632 forbids.
    let max = declared.iter().copied().max().unwrap_or(0);
    for n in 1..=max {
        if !declared.contains(&n) {
            return Err(format!(
                "plans/M20.md: dimension inventory is missing row {n} (rows must be dense 1..={max}; \
                 a dimension is removed by superseding its row, never by deleting it)"
            ));
        }
    }
    let mut claimed = BTreeSet::new();
    for &rule in CostRule::ALL {
        let rows = inventory_rows(rule);
        if rows.is_empty() {
            return Err(format!(
                "freeze 1632: CostRule::{rule:?} (`{}`) names no dimension-inventory row",
                rule.as_str()
            ));
        }
        for &r in rows {
            if !declared.contains(&r) {
                return Err(format!(
                    "freeze 1632: CostRule::{rule:?} (`{}`) names inventory row {r}, which does not \
                     exist in plans/M20.md's table",
                    rule.as_str()
                ));
            }
            claimed.insert(r);
        }
    }
    Ok(format!(
        "dimension inventory: {} row(s) in plans/M20.md, {} rule(s) accounted for across {} row(s)",
        declared.len(),
        CostRule::ALL.len(),
        claimed.len()
    ))
}

/// `plans/M20.md`'s text, read from the repo. Used by the pinned rules check and
/// by this module's units.
pub fn plan_text() -> Result<String, String> {
    let path = super::repo_root().join("plans/M20.md");
    std::fs::read_to_string(&path).map_err(|e| format!("read {}: {e}", path.display()))
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

    // -----------------------------------------------------------------------
    // Fixtures — the `prog()` / `word()` shape item L is told to reuse
    // (cost/ab.rs), plus the placement shapes cost/crosscore.rs uses.
    // -----------------------------------------------------------------------

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

    /// A serial load chain over `offsets`, each load waiting on the
    /// previous one's consumer — the shape
    /// `score::tests::five_way_conflict_inside_capacity_costs_more_than_a_reuse`
    /// uses, so a memory verdict shows up in the schedule instead of
    /// hiding under a parallel issue.
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

    /// `CBZ x0, #off` — a conditional branch whose target the CFG resolves.
    fn cbz(byte_offset: i32) -> EmittedWord {
        let imm19 = ((byte_offset >> 2) as u32) & 0x7FFFF;
        word_enc(0xB400_0000 | (imm19 << 5), CostRule::Branch, None, &[0])
    }

    // -----------------------------------------------------------------------
    // 1. The over-cost rule, per dimension (decision 1609 / freeze 1623).
    //    "This is the oracle that replaces hardware validation."
    // -----------------------------------------------------------------------

    /// The five dimensions that pin their bracket's **low** end on purpose:
    /// a larger charge for a construct whose *removal* is the candidate win
    /// over-credits the removal (decision 1609's second clause, freeze
    /// 1633). Listed here so the direction assertion below cannot silently
    /// grow or shrink the exempt set.
    const REMOVAL_SENSITIVE: &[&str] = &[
        "call_overhead",
        "dmb_cost",
        "load_acquire_cost",
        "store_release_cost",
        "sysreg_flush_cost",
    ];

    /// Per dimension: the bracket is non-degenerate, the pinned value sits
    /// inside it at the declared end, and `removal_sensitive` is exactly
    /// the set above (each with the `ambiguity` note decision 1609 demands).
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

    /// One witness per swept dimension, exercising the term the dimension
    /// prices. Returns the score at an arbitrary point of the box.
    fn witness(dim: &str, point: &SweepPoint) -> u64 {
        let t = table();
        let one = single_core();
        match dim {
            // A frame slot's compulsory reference is charged at its class
            // home level, L2.
            "l2_latency" => total_at(&prog("f", vec![load_stack(1, 0)]), &one, point),
            // A `Cold` base's compulsory reference is charged at L3.
            "l3_latency" => total_at(&prog("f", vec![cold_load(CostRule::Load, 0)]), &one, point),
            // 17 lines in one L3 set (1 MiB / 64 B / 16 ways = 1024 sets, so
            // the stride is 1024 lines) evict the first from every level;
            // its reload takes the DRAM leaf. The same shape witnesses the
            // effective L3 **size**: at 2 MiB there are 2048 sets, the 17
            // lines split across two of them, and the reload is an L3 hit.
            "dram_latency" | "effective_l3_bytes" => {
                let mut code = Vec::new();
                for k in 0..17u64 {
                    code.push(load_stack(1, k * 65536));
                }
                code.push(load_stack(2, 0));
                total_at(&prog("f", code), &one, point)
            }
            // A store buffer hit: the reload of a slot a recent store wrote
            // is satisfied by forwarding.
            "store_to_load_forwarding" => total_at(
                &prog("f", vec![store_stack(0), load_stack(1, 0)]),
                &one,
                point,
            ),
            // A measured 50/50 branch is the full penalty.
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
            // Five branches inside the first aligned 32 B region: row 23's
            // density excess, charged at the shared front-end magnitude.
            "range_cross_penalty" => {
                let code: Vec<EmittedWord> = (0..5).map(|_| cbz(4)).collect();
                total_at(&prog("f", code), &one, point)
            }
            // 49 pages of hot text against a 48-entry I-TLB: the walk cost
            // is the footprint term's, not the scoreboard's, so this reads
            // `footprint::compute` directly.
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
            // `MSR TTBR0_EL1, x0` — the only shape that carries the flush.
            "sysreg_flush_cost" => total_at(
                &prog(
                    "f",
                    vec![word_enc(0xD518_2000, CostRule::System, None, &[0])],
                ),
                &one,
                point,
            ),
            // Placement makes the line remote; the snoop rides above the
            // memory model's leaf.
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
            // §4.5, witnessed synthetically: item I measured that the fixed
            // frame can never straddle either boundary, so the witness sits
            // at a frame offset `codegen.rs` does not emit.
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

    /// **The oracle that replaces hardware validation.** For every swept
    /// dimension: the model actually reads it (the two bracket ends score
    /// differently — a decorative bracket protects nothing), and the
    /// **pinned** end is the more expensive one.
    ///
    /// The five `removal_sensitive` dimensions are asserted the other way
    /// round, because they pin their bracket's low end on purpose (freeze
    /// 1633: a large charge for a barrier over-credits deleting it). Their
    /// safety is structural, not numeric — see
    /// `crosscore::tests::ordering_removals_fire_only_on_a_drop`.
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

    /// The removal-sensitive five, stated as the trade they are: the pinned
    /// point is knowingly the *optimistic* end, and what makes that sound is
    /// the structural refusal, not the number.
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
        // Four of the five price a `CostRule` whose removal is refused
        // outright by item G (freeze 1633, `crosscore::ordering_removals`,
        // wired into the win gate). `call_overhead` is the fifth and is
        // deliberately **not** in that set — deleting a call is inlining,
        // a legitimate candidate — so its only protection is decision
        // 1604's ∀ sweep, which is why it is listed here rather than left
        // to look like an oversight.
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
        // Counts are keyed per fn: freeze 1633 is about where an ordering
        // word is, not just how many the program has.
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

    // -----------------------------------------------------------------------
    // 2. Decision 1616's failure class: a deleted dependence edge.
    //    The single most valuable thing this item can add — 1616 was a
    //    shipped under-cost caught by a human reading, not by a test.
    // -----------------------------------------------------------------------

    /// **The oracle decision 1616 needed and did not have.**
    ///
    /// Item E deleted the NZCV and SP RAW edges on the premise that
    /// renaming removes them. It does not: renaming kills WAR and WAW, and
    /// a consumer still cannot read a value before its producer produced
    /// it. The deletion shipped as an under-cost — the one direction 04 §5
    /// forbids — and nothing failed.
    ///
    /// Item I's repair added two units *about NZCV*. This one is about the
    /// **class**: for every architectural channel this model carries a
    /// dependence through, a two-word dependent pair must cost strictly
    /// more than the same two words made independent. Deleting any
    /// channel's edge — flags, SP, GPR, or the memory slot — fails here,
    /// whichever channel a future item decides renaming has abolished.
    #[test]
    fn every_declared_dependence_channel_serializes() {
        // (channel, dependent pair, independent pair)
        let gpr_dep = vec![
            word(CostRule::Alu, Some(1), &[0, 0]),
            word(CostRule::Alu, Some(2), &[1, 1]),
        ];
        let gpr_ind = vec![
            word(CostRule::Alu, Some(1), &[0, 0]),
            word(CostRule::Alu, Some(2), &[3, 3]),
        ];

        // NZCV: a flag writer then a flag reader, against the same two
        // words with the flag effects removed.
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

        // SP: `sub sp, sp, #N` (add/sub immediate naming register 31 as Rn
        // and Rd) then a frame access that needs the new SP.
        let sp_write = EmittedWord::new(0xD100_43FF, String::new(), CostRule::Alu, Some(31), &[31]);
        let sp_dep = vec![sp_write.clone(), load_stack(1, 8)];
        // The same two words with the producer not naming SP: `sub x0, x0, #16`.
        let sp_ind = vec![
            EmittedWord::new(0xD100_4000, String::new(), CostRule::Alu, Some(0), &[0]),
            load_stack(1, 8),
        ];

        // Memory: the reuse channel. Same two words, same GPR dependence,
        // same everything except which line the second load names — a
        // fresh line (compulsory, L2) against the line the first load
        // already installed (L1D hit). Deleting the reuse state collapses
        // the two into one number.
        let mem_miss = vec![load_stack(1, 0), load_stack_after(2, 4096, 1)];
        let mem_hit = vec![load_stack(1, 0), load_stack_after(2, 8, 1)];

        let cases: [(&str, Vec<EmittedWord>, Vec<EmittedWord>); 4] = [
            ("gpr", gpr_dep, gpr_ind),
            ("nzcv", flags_dep, flags_ind),
            ("sp", sp_dep, sp_ind),
            ("mem-reuse", mem_miss, mem_hit),
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

    /// The other half of 1616, kept cheap: register 31 is **XZR** outside
    /// add/sub-immediate, so an `str xzr` must carry no SP edge at all. The
    /// repair that restored the SP edge must not restore the false one.
    #[test]
    fn register_thirty_one_as_xzr_carries_no_dependence() {
        let sp_write = EmittedWord::new(0xD100_43FF, String::new(), CostRule::Alu, Some(31), &[31]);
        // `str xzr, [x0]` — reads register 31 as the zero register.
        let xzr_store =
            EmittedWord::new(0xF900_0000, String::new(), CostRule::Store, None, &[0, 31])
                .with_mem(MemRef::cold_stable(0, 0));
        let with_producer = total(&prog("f", vec![sp_write, xzr_store.clone()]));
        // The control writes a register the store reads *neither* as a base
        // nor as data: `sub x5, x5, #16`.
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

    // -----------------------------------------------------------------------
    // 3. Monotonicity, reworded (decision 1618).
    // -----------------------------------------------------------------------

    /// Shapes a dead word is appended to. `dense` is the shape decision
    /// 1618 names: five branches clustered inside one aligned 32 B region,
    /// where a padding word genuinely *lowers* the §4.8 density charge.
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
            // Decision 1618's shape, copied from
            // `branch::tests::a_padding_word_can_lower_the_density_charge_and_that_is_real`:
            // five branches inside eight words, dense at some permitted
            // placement. A padding word inserted between the last two
            // spreads them over nine and the density charge falls.
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

    /// A dead, independent word. Writes a register nothing reads; reads a
    /// register nothing wrote.
    fn dead_word() -> EmittedWord {
        word(CostRule::Alu, Some(20), &[21, 21])
    }

    /// **Monotonicity, as item L rewords it (decision 1618).** The plan's
    /// wording — "a dead instruction never lowers a schedule, footprint
    /// cost, or mispredict charge" — is false for the schedule half, and
    /// the counter-example is real rather than an artefact. Asserted here:
    ///
    /// > Inserting a dead word **at any position** never lowers the
    /// > schedule *net of the two decidable §4.8 front-end terms* (rows 23
    /// > and 25), and whenever the gross schedule does fall, the whole drop
    /// > is exactly those two terms' — never the scoreboard's, the memory
    /// > model's, or the cross-core model's.
    ///
    /// Every insertion position is tried, not only the append. Appending is
    /// the weak form: decision 1618's counter-example needs the padding
    /// word *between* two clustered branches, so an append-only oracle
    /// would have passed while asserting something false.
    ///
    /// The mispredict half and the footprint half hold outright and are
    /// asserted separately —
    /// `branch::tests::a_dead_word_never_lowers_the_mispredict_charge` and
    /// `footprint::tests::a_dead_word_never_lowers_a_footprint_or_a_tlb_charge`.
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

    // -----------------------------------------------------------------------
    // 4. Null-opt, at every box endpoint, with block-grain `f` attached.
    // -----------------------------------------------------------------------

    fn rename(key: &str) -> String {
        format!("{key}$fused")
    }

    /// Endpoint excursions: the pinned point, plus each dimension moved
    /// alone to each of its bracket ends, plus the `2^4` corners over the
    /// four dimensions this fixture is sensitive to.
    ///
    /// Not the full `2^17` product — that is 131072 scorings per side and
    /// the plan's own reduction rule (item J: "reduce by bracket endpoints
    /// only, never by dropping a dimension") is honoured, since every
    /// dimension is visited at both of its ends.
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

    /// **Null-opt.** Renaming every scored fn key is semantically neutral,
    /// and is exactly the shape fusion and outlining take. It must not rank
    /// as a win at any point of the residual box.
    ///
    /// Scored on a program that carries every key-sensitive term the model
    /// has: `classify_owner` reads the key, and so does the cross-core
    /// locality classifier (`accessing_core` resolves a method key through
    /// its type's placement entry) — so a rename that made a remote load
    /// unclassifiable would *lower* the total, and that is the failure this
    /// oracle exists to catch.
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

    /// Null-opt **with block-grain `f` attached**, which is where the rename
    /// actually bites: the sidecar's `<fn_key>#<block>` keys stop resolving,
    /// and the coverage rule charges every unresolved key at
    /// `uncovered_charge` (the program maximum) rather than dropping it.
    ///
    /// Decision 1617 is why this is asserted as "never cheaper" and nothing
    /// finer: on `boot-actors` the measured row is 13.4%-covered and the
    /// uncovered term dominates it, so the measured row is a blunt
    /// instrument. What is asserted is exactly the direction 04 §5 requires
    /// — measuring less must never be cheaper — and the magnitude is
    /// reported rather than pinned.
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

        // The sidecar is measured on the *baseline* names, exactly as a
        // real `lane2-freq.txt` would be.
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

    // -----------------------------------------------------------------------
    // 5. Cliff witnesses: each new term is live, not decorative.
    // -----------------------------------------------------------------------

    /// The five cliffs item L names, each ranked in the expected direction
    /// with its magnitude reported. A term that ranks nothing is a term the
    /// model may as well not have.
    #[test]
    fn cliff_witnesses_rank_in_the_expected_direction() {
        let t = table();
        let p = pinned();
        let one = single_core();
        let mut lines = Vec::new();

        // (a) L1I capacity: hot text over 64 KiB against hot text under it.
        let text_words = |bytes: u64| -> Vec<EmittedWord> {
            (0..(bytes / 4) as usize)
                .map(|_| word(CostRule::Alu, Some(1), &[0, 0]))
                .collect()
        };
        let budget = |bytes: u64| -> (u64, u64) {
            let prog = program(&[("Foo.turn", text_words(bytes))]);
            let b = footprint::compute(&prog, &t, &p, &one, HotBlocks::All).expect("footprint");
            (b[0].charge, b[0].hot_text_bytes)
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

        // (b) L1D 4-way conflict inside capacity: 5 lines in one set, then
        // a reload of the first. 320 B against a 64 KiB cache, so no
        // capacity term can explain the difference — only associativity.
        let set_stride = 64u64 * 256; // 64 KiB / 64 B / 4 ways = 256 sets.
        let conflict: Vec<u64> = (0..5).map(|k| k * set_stride).collect();
        let spread: Vec<u64> = (0..5).map(|k| k * 64).collect();
        let c = total_at(&prog("f", serial_loads(&conflict, true)), &one, &p);
        let n = total_at(&prog("f", serial_loads(&spread, true)), &one, &p);
        lines.push(format!("  l1d-4way-conflict conflicting={c} vs spread={n}"));
        assert!(c > n, "L1D associativity cliff is not live: {c} vs {n}");

        // (c) I-TLB span: 49 pages against 48.
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

        // (d) 32 B branch density: 5 branches in one region vs 5 spread out.
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

        // (e) local vs remote load: same word, same reuse distance.
        let code = || vec![cold_load(CostRule::LoadAcquire, 0)];
        let local = total_at(&prog("Foo.turn", code()), &one, &p);
        let remote = total_at(&prog("Foo.turn", code()), &three_cores(), &p);
        lines.push(format!("  local-vs-remote   local={local} remote={remote}"));
        assert!(remote > local, "row 18 is not live: {local} vs {remote}");

        eprintln!("cliff witnesses:\n{}", lines.join("\n"));
    }

    // -----------------------------------------------------------------------
    // 6. The ∃-cheat regression (freeze 1624).
    // -----------------------------------------------------------------------

    /// A candidate built to win **only** at the box's most generous point
    /// must not pass. Construction: the candidate trades one `DMB` away for
    /// a three-deep dependent ALU chain. At `dmb_cost = hi` the barrier is
    /// expensive and the trade looks like a win; at the pinned `dmb_cost`
    /// (its bracket's low end) the barrier is nearly free and the trade
    /// loses. The ∀ quantifier over the box's endpoints is what rejects it,
    /// and no `∃`-form predicate exists to accept it (freeze 1624).
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

        // ∀ over the box's endpoints: a single point where the candidate
        // fails to win is the veto, and the point is named.
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

    // -----------------------------------------------------------------------
    // 7. The synthetic partition adversary (the pre-registered experiment).
    // -----------------------------------------------------------------------

    /// Straight-line body of `n` independent ALU words plus a dependent
    /// tail, so a fused fn is not trivially issue-bound.
    fn body(n: usize, seed: u8) -> Vec<EmittedWord> {
        (0..n)
            .map(|i| {
                let d = seed.wrapping_add((i % 12) as u8).wrapping_add(1);
                word(CostRule::Alu, Some(d), &[d, d])
            })
            .collect()
    }

    /// **The mega-function question, asked on a ruler that can see ports,
    /// fetch, trip counts, and cross-core traffic — and answered against
    /// the ruler rather than for it.**
    ///
    /// 128 identical ALU words are partitioned into `N` fns and scored two
    /// ways: as a **pure repartition** (the same 128 words, no call words
    /// added) and as the realistic split (`N` leaves plus a caller emitting
    /// one `BL` per leaf).
    ///
    /// **Finding, recorded rather than fixed (plans/M20.md item L): fusion
    /// reads as a win, and it does so even in the pure repartition, where
    /// no instruction has been removed.** The cause is compositionality:
    /// `total = Σ` per-fn schedule lengths, and each per-fn schedule pays
    /// its own issue-width rounding and drain, so `N` schedules over the
    /// same words always sum to at least the one schedule over their
    /// concatenation. The ruler therefore has a standing preference for
    /// mega-functions, proportional to `N`.
    ///
    /// This is a hole in the null-opt oracle and it is left open on
    /// purpose — item L's instruction is "do not tune the table to hide
    /// it", and freeze 1628 lands no transformation. What is pinned here
    /// is the *shape* of the bias (monotone in `N`, and the pure-repartition
    /// gap is strictly smaller than the gap with calls, so the model is not
    /// merely counting `BL` words), so the day it changes is loud.
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

        // The finding, pinned in the direction it was measured. This is
        // **not** 04 §5's requirement being satisfied — it is 04 §5's
        // requirement being recorded as violated by the compositional
        // definition of the total.
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
        // Monotone in N: more partitions, more per-fn rounding.
        for w in pure_gaps.windows(2) {
            assert!(
                w[1].1 >= w[0].1,
                "the fusion bias is not monotone in the partition count: {:?} then {:?}",
                w[0],
                w[1]
            );
        }
        // And the call words are a *separate* cost on top, not the whole
        // story — which is what makes this a null-opt hole rather than an
        // ordinary "inlining removes a `BL`" observation.
        for (n, pure) in &pure_gaps {
            let calls = call_gaps.iter().find(|(m, _)| m == n).expect("paired").1;
            assert!(
                calls >= *pure,
                "N={n}: adding call words did not raise the split's total ({calls} vs {pure})"
            );
        }
    }

    /// The unrolling half of the same experiment: one loop body against
    /// `K` copies of it. Same multiset per trip; the question is whether
    /// the ruler pays for the growth.
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
            // The back edge: one conditional branch closing the loop.
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
        // The only assertion 04 §5 supports: `K` copies of a body never
        // cost less than one copy. Per-trip amortization is reported, not
        // asserted — with no measured trip count the flat row cannot argue
        // it, and freeze 1628 lands no unroll.
        let one_copy = totals[0].1;
        for (k, tot) in &totals {
            assert!(
                *tot >= one_copy,
                "unrolling x{k} scored {tot} < x1 {one_copy}: the ruler pays nothing for growth"
            );
        }
    }

    // -----------------------------------------------------------------------
    // 8. Provenance (freeze 1629) — extending, not duplicating.
    // -----------------------------------------------------------------------

    /// `table::tests::reject_pinned_t5_row` covers the load refusal and
    /// `provenance_digest_moves_when_only_a_tier_moves` covers one row.
    /// This extends it to **every** tiered row in the committed profile: a
    /// digest that only hashes some sections would pass the single-row test
    /// and silently stop witnessing the rest.
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
            // Move the tier one step within T1..T4 (never into T5, which a
            // pinned row cannot carry at all).
            let replacement = match trimmed {
                "tier = \"T1\"" => "tier = \"T2\"",
                "tier = \"T2\"" => "tier = \"T3\"",
                "tier = \"T3\"" => "tier = \"T4\"",
                "tier = \"T4\"" => "tier = \"T1\"",
                // T5 rows are sweep dimensions and cross-core terms; moving
                // them off T5 is refused by the parser for `[crosscore]`,
                // so they are witnessed by `reject_pinned_t5_row` instead.
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

    // -----------------------------------------------------------------------
    // 9. Inventory completeness (freeze 1632).
    // -----------------------------------------------------------------------

    /// Every `CostRule` names at least one dimension-inventory row, and
    /// every row it names exists in `plans/M20.md`'s table. The match in
    /// [`inventory_rows`] is exhaustive, so a new variant is a compile
    /// error until it is accounted for; this checks the other side — that
    /// the rows it names are really in the plan.
    #[test]
    fn every_cost_rule_names_an_inventory_row_that_exists_in_the_plan() {
        let text = plan_text().expect("plans/M20.md");
        let summary = check_dimension_inventory(&text).expect("freeze 1632");
        eprintln!("{summary}");
        let rows = plan_inventory_rows(&text).expect("rows");
        assert!(
            rows.contains(&39),
            "row 39 (the ordered accesses) must exist"
        );
        assert!(
            !rows.contains(&40),
            "the plan grew a row this check does not know about"
        );
    }

    /// The check fails closed in both directions: a rule naming a row the
    /// plan does not carry, and an inventory table with a hole in it.
    #[test]
    fn the_inventory_check_fails_closed_on_a_missing_row() {
        let text = plan_text().expect("plans/M20.md");
        // Delete row 17 (the `DMB` row `CostRule::Barrier` names).
        let pruned: String = text
            .lines()
            .filter(|l| !l.starts_with("| 17 |"))
            .collect::<Vec<_>>()
            .join("\n");
        let e = check_dimension_inventory(&pruned).expect_err("a deleted row must be refused");
        assert!(
            e.contains("17"),
            "the refusal must name the missing row, got: {e}"
        );
        // And a table that is not there at all.
        let e = check_dimension_inventory("# no inventory here\n").expect_err("no table");
        assert!(e.contains("Cost dimension inventory"), "got: {e}");
    }

    /// A `CostRule` whose emitted words carry a cost the model charges must
    /// score: this is the standing "every rule scores" invariant restated
    /// against the inventory, so a rule that is priced but accounted for by
    /// no dimension cannot slip through.
    #[test]
    fn the_inventory_accounts_for_every_priced_rule() {
        let t = table();
        for &rule in CostRule::ALL {
            assert!(
                !inventory_rows(rule).is_empty(),
                "CostRule::{rule:?} has no inventory row"
            );
            // Every rule is priced by exactly one of `[latency]` /
            // `[crosscore]` — the parser enforces it; this asserts the
            // model reads a number for each.
            assert!(
                t.latency(rule) > 0 || rule.is_crosscore(),
                "CostRule::{rule:?} prices at zero"
            );
        }
        // The trap word is the one emitted `System` shape, and it is not a
        // system-register write, so it takes no flush charge — recorded
        // here because it is what makes row 19 inert on today's stream.
        assert!(!crate::cost::crosscore::system_word_flushes(enc_brk(1)));
    }
}
