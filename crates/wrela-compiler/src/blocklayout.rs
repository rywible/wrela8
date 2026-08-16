use std::collections::BTreeMap;
#[cfg(test)]
use std::collections::BTreeSet;

use crate::codegen;
use crate::cost::{BlockClass, LayoutClasses};
use crate::mwir::{Inst, MwirFn, MwirProgram};

pub fn block_ranges(body: &[Inst]) -> Vec<(usize, usize)> {
    let leaders = codegen::mwir_block_leaders(body);
    let starts: Vec<usize> = leaders
        .iter()
        .enumerate()
        .filter(|(_, l)| **l)
        .map(|(i, _)| i)
        .collect();
    let n = body.len();
    starts
        .iter()
        .enumerate()
        .map(|(k, &s)| (s, starts.get(k + 1).copied().unwrap_or(n)))
        .collect()
}

fn falls_through(body: &[Inst], end: usize) -> bool {
    match body.get(end.wrapping_sub(1)) {
        Some(Inst::Jump { .. } | Inst::Return { .. }) => false,
        Some(_) => true,
        None => true,
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FnLayout {
    pub order: Vec<usize>,
    pub hot: usize,
    pub cold: usize,
    pub unmeasured: usize,
    pub repairs: usize,
    pub new_block_span: Vec<(usize, usize)>,
}

impl FnLayout {
    pub fn is_identity(&self) -> bool {
        self.order.iter().enumerate().all(|(i, &b)| i == b)
    }
}

pub fn plan_fn(body: &[Inst], classes: &[BlockClass]) -> Result<FnLayout, String> {
    let blocks = block_ranges(body);
    if classes.len() != blocks.len() {
        return Err(format!(
            "blocklayout: {} class(es) for a {}-block partition — the classifier and the body \
             must describe the same fn (fail closed, never lay out an unclassified block)",
            classes.len(),
            blocks.len()
        ));
    }
    let mut warm: Vec<usize> = Vec::with_capacity(blocks.len());
    let mut cold: Vec<usize> = Vec::new();
    let (mut hot, mut unmeasured) = (0usize, 0usize);
    for (k, c) in classes.iter().enumerate() {
        match c {
            BlockClass::Hot => {
                hot += 1;
                warm.push(k);
            }
            BlockClass::Unmeasured => {
                unmeasured += 1;
                warm.push(k);
            }
            BlockClass::Cold => cold.push(k),
        }
    }
    let cold_count = cold.len();
    let mut order = warm;
    order.extend(cold);

    let mut position_of = vec![0usize; blocks.len()];
    for (p, &b) in order.iter().enumerate() {
        position_of[b] = p;
    }
    let mut repairs = 0usize;
    let mut new_block_span = vec![(0usize, 0usize); blocks.len()];
    let mut next_ordinal = 0usize;
    for (p, &b) in order.iter().enumerate() {
        let (_, end) = blocks[b];
        let repaired = falls_through(body, end)
            && !match blocks.iter().position(|&(s, _)| s == end) {
                Some(succ) => position_of[succ] == p + 1,
                None => p + 1 == order.len(),
            };
        if repaired {
            repairs += 1;
        }
        let split = repaired && matches!(body.get(end - 1), Some(Inst::JumpIfFalse { .. }));
        let width = 1 + usize::from(split);
        new_block_span[b] = (next_ordinal, next_ordinal + width);
        next_ordinal += width;
    }

    Ok(FnLayout {
        order,
        hot,
        cold: cold_count,
        unmeasured,
        repairs,
        new_block_span,
    })
}

pub fn new_index_map(body: &[Inst], plan: &FnLayout) -> Result<Vec<usize>, String> {
    let blocks = block_ranges(body);
    if plan.order.len() != blocks.len() {
        return Err(format!(
            "blocklayout: plan orders {} block(s) but the body partitions into {}",
            plan.order.len(),
            blocks.len()
        ));
    }
    let n = body.len();
    let mut position_of = vec![0usize; blocks.len()];
    for (p, &b) in plan.order.iter().enumerate() {
        position_of[b] = p;
    }
    let mut map = vec![usize::MAX; n + 1];
    let mut at = 0usize;
    for &b in &plan.order {
        let (s, e) = blocks[b];
        for slot in &mut map[s..e] {
            *slot = at;
            at += 1;
        }
        if repair_needed(body, &blocks, &position_of, plan, b) {
            at += 1;
        }
    }
    map[n] = at;
    Ok(map)
}

pub fn apply_fn(f: &MwirFn, plan: &FnLayout) -> Result<MwirFn, String> {
    let body = &f.body;
    let blocks = block_ranges(body);
    let new_index = new_index_map(body, plan)?;
    let at = new_index[body.len()];
    let mut position_of = vec![0usize; blocks.len()];
    for (p, &b) in plan.order.iter().enumerate() {
        position_of[b] = p;
    }

    let mut out: Vec<Inst> = Vec::with_capacity(at);
    for &b in &plan.order {
        let (s, e) = blocks[b];
        for inst in &body[s..e] {
            out.push(remap(inst, &new_index)?);
        }
        if repair_needed(body, &blocks, &position_of, plan, b) {
            out.push(Inst::Jump {
                target: new_index[e],
            });
        }
    }
    debug_assert_eq!(out.len(), at);
    verify_successors(body, &out, &new_index)?;

    Ok(MwirFn {
        receiver: f.receiver,
        params: f.params.clone(),
        ret: f.ret.clone(),
        temp_types: f.temp_types.clone(),
        body: out,
    })
}

fn successors(body: &[Inst], i: usize) -> Vec<usize> {
    let mut s = match &body[i] {
        Inst::Jump { target } => vec![*target],
        Inst::JumpIfFalse { target, .. } => vec![*target, i + 1],
        Inst::Return { .. } => vec![body.len()],
        _ => vec![i + 1],
    };
    s.sort_unstable();
    s.dedup();
    s
}

fn verify_successors(before: &[Inst], after: &[Inst], new_index: &[usize]) -> Result<(), String> {
    let n = before.len();
    let real: std::collections::BTreeSet<usize> = new_index[..n].iter().copied().collect();
    let resolve = |mut j: usize| -> Result<usize, String> {
        for _ in 0..=after.len() {
            if j >= after.len() || real.contains(&j) {
                return Ok(j);
            }
            let Inst::Jump { target } = after[j] else {
                return Err(format!(
                    "blocklayout: instruction {j} of the reordered body is neither an original \
                     instruction nor a repair jump (fail closed)"
                ));
            };
            j = target;
        }
        Err("blocklayout: repair jumps form a cycle (fail closed)".to_string())
    };
    for i in 0..n {
        let mut want: Vec<usize> = successors(before, i)
            .into_iter()
            .map(|s| new_index[s])
            .collect();
        want.sort_unstable();
        want.dedup();
        let mut got: Vec<usize> = successors(after, new_index[i])
            .into_iter()
            .map(resolve)
            .collect::<Result<_, _>>()?;
        got.sort_unstable();
        got.dedup();
        if got != want {
            return Err(format!(
                "blocklayout: the permutation changed the successors of instruction {i}: \
                 {got:?} instead of {want:?} (fail closed, never emit a body this pass cannot \
                 prove equivalent)"
            ));
        }
    }
    Ok(())
}

fn repair_needed(
    body: &[Inst],
    blocks: &[(usize, usize)],
    position_of: &[usize],
    plan: &FnLayout,
    b: usize,
) -> bool {
    let (_, end) = blocks[b];
    if !falls_through(body, end) {
        return false;
    }
    let p = position_of[b];
    match blocks.iter().position(|&(s, _)| s == end) {
        Some(succ) => position_of[succ] != p + 1,
        None => p + 1 != plan.order.len(),
    }
}

fn remap(inst: &Inst, new_index: &[usize]) -> Result<Inst, String> {
    let map = |t: usize| -> Result<usize, String> {
        match new_index.get(t) {
            Some(&v) if v != usize::MAX => Ok(v),
            _ => Err(format!(
                "blocklayout: branch target {t} is not an index of this body (fail closed)"
            )),
        }
    };
    Ok(match inst {
        Inst::Jump { target } => Inst::Jump {
            target: map(*target)?,
        },
        Inst::JumpIfFalse { cond, target } => Inst::JumpIfFalse {
            cond: *cond,
            target: map(*target)?,
        },
        other => other.clone(),
    })
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LayoutSummary {
    pub fns_moved: usize,
    pub fns_total: usize,
    pub hot: usize,
    pub cold: usize,
    pub unmeasured: usize,
    pub repairs: usize,
    pub plans: BTreeMap<String, FnLayout>,
}

impl LayoutSummary {
    pub fn render(&self) -> String {
        format!(
            "blocklayout fns_moved={}/{} hot={} cold={} unmeasured={} repairs={}",
            self.fns_moved, self.fns_total, self.hot, self.cold, self.unmeasured, self.repairs
        )
    }
}

pub fn relayout_program(
    program: &MwirProgram,
    classes: &LayoutClasses,
) -> Result<(MwirProgram, LayoutSummary), String> {
    #[cfg(test)]
    RELAYOUT_CALLS.with(|c| c.set(c.get() + 1));
    let mut fns = BTreeMap::new();
    let mut summary = LayoutSummary::default();
    for (key, f) in &program.fns {
        summary.fns_total += 1;
        let blocks = block_ranges(&f.body);
        let per_block: Vec<BlockClass> = (0..blocks.len())
            .map(|k| classes.class_of(key, k as u32))
            .collect();
        let plan = plan_fn(&f.body, &per_block)?;
        summary.hot += plan.hot;
        summary.cold += plan.cold;
        summary.unmeasured += plan.unmeasured;
        summary.repairs += plan.repairs;
        if !plan.is_identity() {
            summary.fns_moved += 1;
        }
        fns.insert(key.clone(), apply_fn(f, &plan)?);
        summary.plans.insert(key.clone(), plan);
    }
    Ok((
        MwirProgram {
            fns,
            rodata: program.rodata.clone(),
            direct_fp_fns: program.direct_fp_fns.clone(),
        },
        summary,
    ))
}

#[cfg(test)]
thread_local! {
    static RELAYOUT_CALLS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

#[cfg(test)]
fn relayout_calls() -> usize {
    RELAYOUT_CALLS.with(|c| c.get())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mwir::Temp;
    use crate::sema::types::Type;

    fn cbool(dst: usize) -> Inst {
        Inst::ConstBool {
            dst: Temp(dst),
            value: true,
        }
    }

    fn body_hot_cold() -> Vec<Inst> {
        vec![
            cbool(0),
            Inst::JumpIfFalse {
                cond: Temp(0),
                target: 4,
            },
            cbool(1),
            Inst::Jump { target: 6 },
            cbool(2),
            Inst::Jump { target: 6 },
            Inst::Return { value: None },
        ]
    }

    fn fnof(body: Vec<Inst>) -> MwirFn {
        MwirFn {
            receiver: None,
            params: vec![],
            ret: Type::Unit,
            temp_types: vec![Type::Bool, Type::Bool, Type::Bool],
            body,
        }
    }

    #[test]
    fn block_ranges_are_the_leader_partition() {
        let b = body_hot_cold();
        assert_eq!(block_ranges(&b), vec![(0, 2), (2, 4), (4, 6), (6, 7)]);
    }

    #[test]
    fn a_synthetic_hot_cold_program_packs_its_hot_blocks() {
        let b = body_hot_cold();
        let classes = vec![
            BlockClass::Hot,
            BlockClass::Cold,
            BlockClass::Hot,
            BlockClass::Hot,
        ];
        let plan = plan_fn(&b, &classes).expect("plan");
        assert_eq!(
            plan.order,
            vec![0, 2, 3, 1],
            "cold block 1 sinks to the end"
        );
        assert_eq!((plan.hot, plan.cold, plan.unmeasured), (3, 1, 0));

        let out = apply_fn(&fnof(b), &plan).expect("apply");
        assert_eq!(
            out.body,
            vec![
                cbool(0),
                Inst::JumpIfFalse {
                    cond: Temp(0),
                    target: 3,
                },
                Inst::Jump { target: 6 },
                cbool(2),
                Inst::Jump { target: 5 },
                Inst::Return { value: None },
                cbool(1),
                Inst::Jump { target: 5 },
            ]
        );
        assert_eq!(plan.repairs, 1);
        let ranges = block_ranges(&out.body);
        assert_eq!(ranges, vec![(0, 2), (2, 3), (3, 5), (5, 6), (6, 8)]);
    }

    #[test]
    fn a_repair_after_a_conditional_costs_one_block() {
        let b = body_hot_cold();
        let before = block_ranges(&b).len();
        let plan = plan_fn(
            &b,
            &[
                BlockClass::Hot,
                BlockClass::Cold,
                BlockClass::Hot,
                BlockClass::Hot,
            ],
        )
        .expect("plan");
        let out = apply_fn(&fnof(b), &plan).expect("apply");
        assert_eq!(plan.repairs, 1);
        assert_eq!(block_ranges(&out.body).len(), before + 1);

        let b2 = vec![
            cbool(0),
            Inst::JumpIfFalse {
                cond: Temp(0),
                target: 4,
            },
            cbool(1),
            cbool(2),
            Inst::Return { value: None },
        ];
        assert_eq!(block_ranges(&b2), vec![(0, 2), (2, 4), (4, 5)]);
        let plan2 =
            plan_fn(&b2, &[BlockClass::Hot, BlockClass::Cold, BlockClass::Hot]).expect("plan");
        assert_eq!(plan2.order, vec![0, 2, 1]);
        let out2 = apply_fn(&fnof(b2.clone()), &plan2).expect("apply");
        assert_eq!(plan2.repairs, 2);
        assert_eq!(block_ranges(&out2.body).len(), block_ranges(&b2).len() + 1);
    }

    #[test]
    fn no_sidecar_degrades_to_a_byte_identical_layout() {
        let f = fnof(body_hot_cold());
        let program = MwirProgram {
            fns: BTreeMap::from([("F.m".to_string(), f.clone())]),
            rodata: vec![],
            direct_fp_fns: BTreeSet::new(),
        };
        let (out, summary) =
            relayout_program(&program, &LayoutClasses::Unmeasured).expect("relayout");
        assert_eq!(out.fns["F.m"].body, f.body, "not one instruction moved");
        assert_eq!(summary.fns_moved, 0);
        assert_eq!(summary.repairs, 0);
        assert_eq!(summary.unmeasured, 4, "all four blocks, and none sunk");
        assert_eq!(summary.cold, 0);
    }

    #[test]
    fn unmeasured_blocks_are_not_sunk() {
        let b = body_hot_cold();
        let classes = vec![
            BlockClass::Hot,
            BlockClass::Unmeasured,
            BlockClass::Cold,
            BlockClass::Hot,
        ];
        let plan = plan_fn(&b, &classes).expect("plan");
        assert_eq!(
            plan.order,
            vec![0, 1, 3, 2],
            "only the Cold block moves; the Unmeasured one keeps its place"
        );
        assert_eq!((plan.hot, plan.cold, plan.unmeasured), (2, 1, 1));
        assert_ne!(plan.order, vec![0, 3, 1, 2]);
    }

    #[test]
    fn an_identity_plan_is_byte_identical() {
        let b = body_hot_cold();
        let classes = vec![BlockClass::Hot; 4];
        let plan = plan_fn(&b, &classes).expect("plan");
        assert!(plan.is_identity());
        assert_eq!(plan.repairs, 0);
        let f = fnof(b.clone());
        assert_eq!(apply_fn(&f, &plan).expect("apply").body, b);
    }

    #[test]
    fn the_permuted_body_has_the_same_successor_relation() {
        let b = body_hot_cold();
        let n = b.len();

        let succ = |body: &[Inst], i: usize| -> Vec<usize> {
            let mut s = Vec::new();
            match &body[i] {
                Inst::Jump { target } => s.push(*target),
                Inst::JumpIfFalse { target, .. } => {
                    s.push(*target);
                    s.push(i + 1);
                }
                Inst::Return { .. } => s.push(body.len()),
                _ => s.push(i + 1),
            }
            s.sort_unstable();
            s.dedup();
            s
        };

        for bits in 0u32..16 {
            let classes: Vec<BlockClass> = (0..4)
                .map(|k| {
                    if bits & (1 << k) != 0 {
                        BlockClass::Cold
                    } else {
                        BlockClass::Hot
                    }
                })
                .collect();
            let plan = plan_fn(&b, &classes).expect("plan");
            let map = new_index_map(&b, &plan).expect("map");
            let out = apply_fn(&fnof(b.clone()), &plan).expect("apply");
            let inserted: std::collections::BTreeSet<usize> =
                (0..out.body.len()).filter(|j| !map.contains(j)).collect();

            let resolve = |mut j: usize| -> usize {
                let mut guard = 0;
                while inserted.contains(&j) {
                    let Inst::Jump { target } = out.body[j] else {
                        panic!("inserted instruction at {j} is not a repair jump");
                    };
                    j = target;
                    guard += 1;
                    assert!(guard < 8, "repair jumps must not chain");
                }
                j
            };

            for i in 0..n {
                let want: Vec<usize> = {
                    let mut v: Vec<usize> = succ(&b, i).into_iter().map(|s| map[s]).collect();
                    v.sort_unstable();
                    v.dedup();
                    v
                };
                let got: Vec<usize> = {
                    let mut v: Vec<usize> = succ(&out.body, map[i])
                        .into_iter()
                        .map(|s| if s == out.body.len() { s } else { resolve(s) })
                        .collect();
                    v.sort_unstable();
                    v.dedup();
                    v
                };
                assert_eq!(got, want, "bits={bits:b} old index {i}");
            }
        }
    }

    #[test]
    fn a_class_vector_of_the_wrong_length_fails_closed() {
        let b = body_hot_cold();
        let err = plan_fn(&b, &[BlockClass::Hot; 3]).expect_err("must fail");
        assert!(err.contains("3 class(es) for a 4-block partition"), "{err}");
    }

    #[test]
    fn new_block_span_locates_every_original_block() {
        let b = body_hot_cold();
        for bits in 0u32..16 {
            let classes: Vec<BlockClass> = (0..4)
                .map(|k| {
                    if bits & (1 << k) != 0 {
                        BlockClass::Cold
                    } else {
                        BlockClass::Hot
                    }
                })
                .collect();
            let plan = plan_fn(&b, &classes).expect("plan");
            let out = apply_fn(&fnof(b.clone()), &plan).expect("apply");
            let after = block_ranges(&out.body);
            let map = new_index_map(&b, &plan).expect("map");
            let before = block_ranges(&b);

            let mut covered = 0usize;
            for &p in &plan.order {
                let (lo, hi) = plan.new_block_span[p];
                assert_eq!(lo, covered, "bits={bits:b} block {p}");
                covered = hi;
            }
            assert_eq!(covered, after.len(), "bits={bits:b}");

            for (p, &(s, _)) in before.iter().enumerate() {
                let (lo, _) = plan.new_block_span[p];
                assert_eq!(after[lo].0, map[s], "bits={bits:b} block {p}");
            }
        }
    }

    #[test]
    fn the_measured_hot_text_footprint_before_and_after() {
        // Re-measured 2026-08-07 with `AdrAddressing` parked (the P7
        // image-base move put DRAM pages beyond ADR reach).
        //
        // Re-measured again after large aggregate copies became counted loops:
        // a copy that was a straight run of load/store pairs is now its own
        // small block, so the production window fetches slightly more hot text
        // even though total emitted text fell by about a quarter.
        const BEFORE_HOT_TEXT_BYTES: u64 = 2432;

        use crate::cost::{
            self, BlockBridge, HotBlocks, MeasuredBlocks, SweepPoint, make_key,
            sibling_block_freq_path,
        };
        use std::collections::BTreeSet;

        let input = cost::repo_root().join("tests/golden/boot-actors/input.wr");
        let table = cost::load_default().expect("cost table");
        let sidecar = sibling_block_freq_path(&input).expect("boot-actors has a lane2 sidecar");
        let counts = cost::freq::load_block_from_path(&sidecar)
            .expect("sidecar")
            .counts;

        crate::opts::apply_mode(crate::opts::CompileMode::Release);

        crate::codegen::set_block_bridge(true);
        let (before, placement) =
            cost::codegen_cost_stage_with_placement(&input).expect("cost-stage codegen");
        let spans_before = crate::codegen::block_spans();
        crate::codegen::set_block_bridge(false);

        let bridge_before =
            BlockBridge::build(&before, &spans_before, &table, &placement).expect("bridge");
        let mb = MeasuredBlocks::resolve(&bridge_before, &counts).expect("resolve");
        let hot_before = |k: &str, w: usize| mb.is_hot(k, w);
        let budget_before = cost::footprint::compute(
            &before,
            &table,
            &SweepPoint::pinned(&table),
            &placement,
            HotBlocks::Measured(&hot_before),
        )
        .expect("footprint");

        let line = 64u64;
        let mut hot_bytes = 0u64;
        let mut floor = 0u64;
        for (key, f) in &before.fns {
            let mut hb = 0u64;
            for (bi, (s, e)) in cost::basic_block_ranges(&f.code).into_iter().enumerate() {
                if hot_before(key, bi) {
                    hb += (e - s) as u64 * 4;
                }
            }
            hot_bytes += hb;
            floor += hb.div_ceil(line) * line;
        }

        let classes = cost::layout_classes(Some(&input), &spans_before).expect("classify");
        assert!(classes.is_measured(), "the committed sidecar must classify");

        crate::codegen::set_block_bridge(true);
        let (after, placement_after, summary) =
            cost::codegen_cost_stage_with_block_layout(&input, &classes).expect("relaid codegen");
        let spans_after = crate::codegen::block_spans();
        crate::codegen::set_block_bridge(false);
        assert_eq!(placement_after.cores, placement.cores);

        let bridge_after =
            BlockBridge::build(&after, &spans_after, &table, &placement).expect("bridge after");

        let bridged: BTreeMap<&String, &cost::BridgedBlock> = bridge_after.blocks().collect();
        let mut hot_words: BTreeMap<String, BTreeSet<usize>> = BTreeMap::new();
        for (key, count) in &counts {
            if *count == 0 {
                continue;
            }
            let (fn_key, orig) = cost::split_key(key).expect("a committed sidecar key");
            let ords = match summary.plans.get(fn_key) {
                None => (orig as usize, orig as usize + 1),
                Some(p) => match p.new_block_span.get(orig as usize) {
                    Some(&(lo, hi)) => (lo, hi),
                    None => continue,
                },
            };
            for ord in ords.0..ords.1 {
                let Some(bb) = bridged.get(&make_key(fn_key, ord as u32)) else {
                    continue;
                };
                let set = hot_words.entry(fn_key.to_string()).or_default();
                for w in bb.first_word_block..bb.first_word_block + bb.word_blocks as usize {
                    set.insert(w);
                }
            }
        }
        let hot_after = |k: &str, w: usize| hot_words.get(k).is_some_and(|s| s.contains(&w));
        let budget_after = cost::footprint::compute(
            &after,
            &table,
            &SweepPoint::pinned(&table),
            &placement,
            HotBlocks::Measured(&hot_after),
        )
        .expect("footprint after");

        let words_before: u64 = before.fns.values().map(|f| f.code.len() as u64).sum();
        let words_after: u64 = after.fns.values().map(|f| f.code.len() as u64).sum();
        let frameless = |p: &crate::codegen::CodegenProgram| -> u64 {
            p.fns.values().filter(|f| f.frame_size == 0).count() as u64
        };
        let regained = frameless(&before).saturating_sub(frameless(&after));

        for (key, bf) in &before.fns {
            let Some(af) = after.fns.get(key) else {
                continue;
            };
            let d_words = af.code.len() as i64 - bf.code.len() as i64;
            let repairs = summary.plans.get(key).map_or(0, |p| p.repairs) as i64;
            if d_words != repairs || af.frame_size != bf.frame_size {
                eprintln!(
                    "D-MEASURE fn `{key}` words {}->{} (d={d_words}, repairs={repairs}) \
                     frame {}->{}",
                    bf.code.len(),
                    af.code.len(),
                    bf.frame_size,
                    af.frame_size
                );
            }
        }

        let mut partition_mismatch: Vec<(String, usize, usize)> = Vec::new();
        for (key, plan) in &summary.plans {
            let recorded = spans_before.iter().filter(|s| &s.fn_key == key).count();
            if recorded == 0 {
                continue;
            }
            if plan.order.len() != recorded {
                partition_mismatch.push((key.clone(), plan.order.len(), recorded));
            }
        }

        let flat = |p: &crate::codegen::CodegenProgram| {
            cost::footprint::compute(
                p,
                &table,
                &SweepPoint::pinned(&table),
                &placement,
                HotBlocks::All,
            )
            .expect("flat footprint")
        };
        let (flat_before, flat_after) = (flat(&before), flat(&after));

        for (key, planned, recorded) in &partition_mismatch {
            eprintln!(
                "D-MEASURE partition-mismatch fn `{key}` mwir_blocks={planned} \
                 emitted_blocks={recorded}"
            );
        }
        eprintln!("D-MEASURE {}", summary.render());
        eprintln!(
            "D-MEASURE fns sync={} total={}",
            summary.fns_total,
            before.fns.len()
        );
        eprintln!("D-MEASURE words before={words_before} after={words_after}");
        eprintln!(
            "D-MEASURE frameless before={} after={} regained={regained} \
             word_delta={} repairs={}",
            frameless(&before),
            frameless(&after),
            words_after as i64 - words_before as i64,
            summary.repairs
        );
        eprintln!(
            "D-MEASURE flat_hot_text before={} after={}",
            flat_before[0].fetched_text_bytes, flat_after[0].fetched_text_bytes
        );
        eprintln!(
            "D-MEASURE hot_bytes={hot_bytes} per_fn_packing_floor={floor} \
             headroom={} captured={}",
            budget_before[0].fetched_text_bytes.saturating_sub(floor),
            budget_before[0]
                .fetched_text_bytes
                .saturating_sub(budget_after[0].fetched_text_bytes)
        );
        for (b, a) in budget_before.iter().zip(budget_after.iter()) {
            eprintln!(
                "D-MEASURE core={} measured_hot_text before={} after={} \
                 lines {}->{} pages {}->{} charge {}->{}",
                b.n,
                b.fetched_text_bytes,
                a.fetched_text_bytes,
                b.fetched_text_bytes / 64,
                a.fetched_text_bytes / 64,
                b.text_pages,
                a.text_pages,
                b.charge,
                a.charge
            );
        }

        assert_eq!(budget_before.len(), budget_after.len());
        assert!(!budget_before.is_empty(), "boot-actors places one core");
        assert_eq!(
            budget_before[0].fetched_text_bytes, BEFORE_HOT_TEXT_BYTES,
            "the production-window hot set moved; regenerate and re-measure the sidecar"
        );
        assert_eq!(
            budget_after, budget_before,
            "the current production-window classification yields an identity layout"
        );
        assert_eq!(
            (words_before, words_after, summary.repairs, regained),
            // Re-measured 2026-08-07 with `AdrAddressing` parked, and again
            // once counted-loop aggregate copies replaced the unrolled
            // per-word pairs (1805 -> 1765 emitted words).
            (1765, 1765, 0, 0)
        );
        assert_eq!(summary.fns_moved, 0);
        assert_eq!(
            partition_mismatch
                .iter()
                .map(|(k, a, b)| (k.as_str(), *a, *b))
                .collect::<Vec<_>>(),
            vec![("Ledger.mark", 2, 1), ("Ledger.read_marks", 2, 1)]
        );
    }

    #[test]
    fn without_the_allocator_a_reordering_costs_exactly_its_repairs() {
        use crate::cost;
        use crate::opts::{OptId, RELEASE_OPTS, apply_opts};

        let input = cost::repo_root().join("tests/golden/boot-actors/input.wr");
        let without: Vec<OptId> = RELEASE_OPTS
            .iter()
            .copied()
            .filter(|o| *o != OptId::RegAlloc)
            .collect();
        apply_opts(&without);

        crate::codegen::set_block_bridge(true);
        let (before, _) = cost::codegen_cost_stage_with_placement(&input).expect("cost-stage");
        let spans = crate::codegen::block_spans();
        crate::codegen::set_block_bridge(false);
        let classes = cost::layout_classes(Some(&input), &spans).expect("classify");
        assert!(classes.is_measured(), "the committed sidecar must classify");
        let (after, _, summary) =
            cost::codegen_cost_stage_with_block_layout(&input, &classes).expect("relaid");
        crate::opts::apply_mode(crate::opts::CompileMode::Release);

        let words = |p: &crate::codegen::CodegenProgram| -> u64 {
            p.fns.values().map(|f| f.code.len() as u64).sum()
        };
        let frameless = |p: &crate::codegen::CodegenProgram| -> usize {
            p.fns.values().filter(|f| f.frame_size == 0).count()
        };
        assert_eq!(
            words(&after),
            words(&before) + summary.repairs as u64,
            "with the allocator off, every extra word is an accounted repair jump"
        );
        assert_eq!(
            frameless(&after),
            frameless(&before),
            "with the allocator off, no fn's residency depends on block order"
        );
    }

    #[test]
    fn the_parked_pass_is_not_on_the_compile_path() {
        use crate::cost;

        let input = cost::repo_root().join("tests/golden/boot-actors/input.wr");
        crate::opts::apply_mode(crate::opts::CompileMode::Release);

        let calls0 = relayout_calls();
        crate::codegen::set_block_bridge(true);
        let (plain, _) = cost::codegen_cost_stage_with_placement(&input).expect("cost-stage");
        let spans = crate::codegen::block_spans();
        crate::codegen::set_block_bridge(false);
        let _ = cost::codegen_cost_stage(&input).expect("cost-stage");
        assert_eq!(
            relayout_calls(),
            calls0,
            "a normal build must not reach the parked block-layout pass"
        );

        let classes = cost::layout_classes(Some(&input), &spans).expect("classify");
        let (_relaid, _, summary) =
            cost::codegen_cost_stage_with_block_layout(&input, &classes).expect("relaid");
        assert_eq!(
            summary.fns_moved, 0,
            "the production window currently plans identity"
        );
        assert_eq!(relayout_calls(), calls0 + 1, "the parked entry point ran");

        let (again, _) = cost::codegen_cost_stage_with_placement(&input).expect("cost-stage");
        assert_eq!(again.fns.len(), plain.fns.len());
        for (key, f) in &plain.fns {
            let g = again.fns.get(key).unwrap_or_else(|| panic!("fn `{key}`"));
            assert_eq!(&g.code, &f.code, "fn `{key}` is not byte-identical");
            assert_eq!(g.frame_size, f.frame_size, "fn `{key}` frame");
        }
        assert_eq!(
            relayout_calls(),
            calls0 + 1,
            "the normal build after it must still not reach the pass"
        );

        let src = cost::repo_root().join("crates/wrela-compiler/src");
        let mut callers: Vec<String> = Vec::new();
        let mut stack = vec![src.clone()];
        while let Some(dir) = stack.pop() {
            for e in std::fs::read_dir(&dir).expect("read_dir") {
                let p = e.expect("entry").path();
                if p.is_dir() {
                    stack.push(p);
                    continue;
                }
                if p.extension().and_then(|s| s.to_str()) != Some("rs") {
                    continue;
                }
                let text = std::fs::read_to_string(&p).expect("read");
                let hit = text.lines().any(|l| {
                    let l = l.trim_start();
                    !l.starts_with("//")
                        && (l.contains("relayout_program(")
                            || l.contains("codegen_cost_stage_with_block_layout("))
                });
                if hit {
                    callers.push(
                        p.strip_prefix(&src)
                            .expect("prefix")
                            .to_string_lossy()
                            .replace('\\', "/"),
                    );
                }
            }
        }
        callers.sort();
        assert_eq!(
            callers,
            vec!["blocklayout.rs".to_string(), "cost/stage.rs".to_string()],
            "the parked pass grew a caller. Wiring it is decision 1755 and decision \
             1948, not an edit — see the module doc."
        );
    }

    #[test]
    fn the_compositor_is_the_workload_that_could_re_ask() {
        use crate::cost::{self, HotBlocks, SweepPoint};

        let input = cost::repo_root().join("tests/golden/boot-tile-compositor/input.wr");
        let table = cost::load_default().expect("cost table");

        let mut flat = Vec::new();
        for (label, mode) in [
            ("dev", crate::opts::CompileMode::Dev),
            ("release", crate::opts::CompileMode::Release),
        ] {
            crate::opts::apply_mode(mode);
            let (prog, placement) =
                cost::codegen_cost_stage_with_placement(&input).expect("cost-stage codegen");
            let budget = cost::footprint::compute(
                &prog,
                &table,
                &SweepPoint::pinned(&table),
                &placement,
                HotBlocks::All,
            )
            .expect("footprint");
            let words: u64 = prog.fns.values().map(|f| f.code.len() as u64).sum();
            eprintln!(
                "O-COMPOSITOR {label}: words={words} fetched_text={} executable_code={} \
                 l1i={} charge={} pages={}",
                budget[0].fetched_text_bytes,
                budget[0].executable_code_bytes,
                budget[0].l1i_bytes,
                budget[0].charge,
                budget[0].text_pages
            );
            flat.push((words, budget[0].clone()));
        }

        assert!(
            cost::sibling_block_freq_path(&input).is_none(),
            "a `lane2-freq.txt` appeared next to the compositor. That is the named \
             re-ask condition in this module's doc: re-run this test, re-measure the \
             density charge under `HotBlocks::Measured`, and re-argue decision 1946 — \
             do not just update the assertion below."
        );
        crate::opts::apply_mode(crate::opts::CompileMode::Release);
        crate::codegen::set_block_bridge(true);
        let (before, placement) =
            cost::codegen_cost_stage_with_placement(&input).expect("cost-stage codegen");
        let spans = crate::codegen::block_spans();
        crate::codegen::set_block_bridge(false);
        let classes = cost::layout_classes(Some(&input), &spans).expect("classify");
        assert_eq!(
            classes,
            crate::cost::LayoutClasses::Unmeasured,
            "no sidecar means no classification, which is the whole finding"
        );
        crate::codegen::set_block_bridge(true);
        let (after, _, summary) =
            cost::codegen_cost_stage_with_block_layout(&input, &classes).expect("relaid");
        crate::codegen::set_block_bridge(false);
        eprintln!("O-COMPOSITOR pass: {}", summary.render());
        assert_eq!((summary.fns_moved, summary.repairs), (0, 0));
        for (key, f) in &before.fns {
            assert_eq!(&after.fns[key].code, &f.code, "fn `{key}`");
        }

        let after_flat = cost::footprint::compute(
            &after,
            &table,
            &SweepPoint::pinned(&table),
            &placement,
            HotBlocks::All,
        )
        .expect("footprint after");
        assert_eq!(flat[1].1.charge, after_flat[0].charge);

        assert_eq!(
            (
                flat[0].1.fetched_text_bytes,
                flat[1].1.fetched_text_bytes,
                flat[1].1.l1i_bytes
            ),
            // Release column re-measured 2026-08-07 with `AdrAddressing`
            // parked; headroom against the L1I stays positive on both sides.
            // Both columns re-measured again after counted-loop aggregate
            // copies replaced the unrolled per-word pairs: the compositor's hot
            // text fell on both sides, so the L1I headroom the note describes
            // only grew.
            (43_328, 23_616, 65_536),
            "the compositor's flat hot text, dev and release, against the L1I. Item M's \
             ~17 KB-of-headroom figure is the **dev** column; release has 37 KB of \
             headroom, so the L1I overflow term is zero on both sides and the only \
             footprint term that could ever rank this pass here is the density one \
             (decision 1946). Re-measure before touching."
        );
        assert!(
            flat[1].1.fetched_text_bytes < flat[1].1.l1i_bytes,
            "the compositor's release hot text must fit the L1I with room, or the \
             density argument changes shape: {} vs {}",
            flat[1].1.fetched_text_bytes,
            flat[1].1.l1i_bytes
        );
    }

    #[test]
    fn a_stale_sidecar_fails_the_build_rather_than_laying_out() {
        use crate::cost;

        let input = cost::repo_root().join("tests/golden/boot-actors/input.wr");
        crate::opts::apply_mode(crate::opts::CompileMode::Release);
        crate::codegen::set_block_bridge(true);
        let _ = cost::codegen_cost_stage(&input).expect("cost-stage codegen");
        let spans = crate::codegen::block_spans();
        crate::codegen::set_block_bridge(false);

        assert!(
            cost::layout_classes(Some(&input), &spans)
                .expect("fresh")
                .is_measured()
        );

        let measured = cost::derive::derive(
            &cost::freq::load_block_from_path(
                &cost::repo_root().join("tests/golden/boot-actors/lane2-freq.txt"),
            )
            .expect("sidecar"),
        )
        .expect("derive");
        let victim = measured
            .blocks
            .iter()
            .find(|row| {
                row.block_index > 0
                    && spans.iter().any(|span| {
                        span.fn_key == row.fn_key && span.block_index == row.block_index
                    })
            })
            .expect("a measured non-entry block in the built closure");
        let shrunk: Vec<_> = spans
            .iter()
            .filter(|span| span.fn_key != victim.fn_key || span.block_index == 0)
            .cloned()
            .collect();
        assert!(shrunk.len() < spans.len(), "the fixture must remove blocks");
        let err = cost::layout_classes(Some(&input), &shrunk).expect_err("stale must fail closed");
        assert!(err.contains("is stale"), "{err}");

        assert!(cost::layout_classes(Some(&input), &[]).is_err());
    }
}
