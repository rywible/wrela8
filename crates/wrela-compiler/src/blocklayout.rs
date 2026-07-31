//! Hot/cold basic-block layout (plans/codegen-pareto.md item D).
//!
//! Pack the measured-hot blocks of a fn contiguously and sink the measured
//! **cold** ones — abort paths, error handling, rare branches — below them,
//! so a core's L1I holds hot text instead of hot text interleaved with
//! never-executed text.
//!
//! The whole input is item A's classifier: [`crate::cost::layout_classes`]
//! returns a [`LayoutClasses`] and [`LayoutClasses::class_of`] answers
//! [`BlockClass`] per `(fn_key, block_index)`. This module adds no
//! measurement, no heuristic and no second source of truth — if the
//! classifier says `Unmeasured`, the block stays exactly where it was.
//!
//! ## What a block is here
//!
//! The reorder unit is the **MWIR block**: a contiguous run of
//! `MwirFn::body` indices starting at a leader, where the leader set is
//! `codegen::mwir_block_leaders` — the *same* function that assigns
//! Lane 2 its block ids. That identity is not a convenience, it is the whole
//! correctness argument: the sidecar's `<fn_key>#<block_index>` keys are
//! ordinals over exactly this partition, so a class looked up at index `k`
//! describes exactly the run this module is about to move. Decision 1753;
//! checked against a real bridge-mode build rather than argued, inside
//! `unit:the_measured_hot_text_footprint_before_and_after`.
//!
//! ## The algorithm (decision 1751)
//!
//! A **stable two-way partition**. Walk the blocks in original order and
//! emit, first, every block that is `Hot` **or** `Unmeasured`, then every
//! block that is `Cold` — each run keeping its original relative order.
//! Nothing else moves: no chain layout, no trace formation, no
//! frequency sort. Two properties follow from stability and they are the
//! reason it is the algorithm:
//!
//! - a fn with no cold block, or no measurement at all, permutes to the
//!   **identity** and its emitted words are byte-identical;
//! - every fallthrough edge whose two ends land in the same run survives
//!   untouched, so the repair cost is one word per hot/cold *boundary*
//!   rather than one word per block.
//!
//! ## Fallthrough repair
//!
//! MWIR control flow is index-relative: `Jump`/`JumpIfFalse` name a body
//! index and every other instruction falls through to `i + 1`.
//! [`apply_fn`] therefore does two things beyond permuting: it rewrites
//! every target through the old→new index map, and it appends an explicit
//! `Inst::Jump` to any block whose successor is no longer the block
//! physically after it. A block ending in `Jump` or `Return` has no
//! fallthrough and never gets one (`Inst::Return` is itself a jump to
//! `body.len()`, which the remap carries).
//!
//! A repair costs **one word**, and it costs one extra *block* only when
//! the block it repairs ends in a conditional branch: `mwir_block_leaders`
//! marks the instruction after any branch as a leader, so a `Jump` appended
//! after a `JumpIfFalse` becomes a one-instruction block of its own, while a
//! `Jump` appended after an ordinary instruction stays inside the block it
//! repairs. The pass does **not** claim to preserve the block count and the
//! unit measures the growth instead of asserting it away
//! (`unit:a_repair_after_a_conditional_costs_one_block`). This is a second,
//! smaller reason a post-pass partition cannot be re-keyed against a
//! pre-pass sidecar — see "Why this pass is not installed" below.
//!
//! ## Unmeasured is laid out hot, never sunk (decision 1752)
//!
//! Item A's §6 is explicit and this module obeys it literally:
//! `BlockClass::Unmeasured` means *no evidence*, and sinking code on no
//! evidence is a guess. It is grouped with `Hot`, not with `Cold`.
//! `cost::bridge::MeasuredBlocks::is_hot` answers `false` for unmeasured —
//! correct for the footprint term, wrong here — and is deliberately not
//! used.
//!
//! ## Why this pass is not installed on the emission path (decision 1755)
//!
//! `cost::bridge::BlockBridge` requires a fn's recorded spans to satisfy
//! `block_index == word order`: it rejects "block ordinals out of order"
//! and any `word_start` that does not continue the previous span. So a
//! sidecar key `fn#k` is resolved to the **k-th emitted block**, by
//! position. Reordering blocks re-keys that correspondence against a
//! sidecar recorded under the old order, and the one program in the tree
//! with a sidecar (`tests/golden/boot-actors`) would then print a
//! `MeasuredBudget` line describing the wrong blocks — silently.
//!
//! Fail closed: the pass is built, measured and pinned here, and it is not
//! installed. Installing it needs a bridge that carries a block's identity
//! ([`FnLayout::new_block_span`] is exactly that datum) instead of
//! inferring it from position — ruler plumbing, and not this item's to
//! change. `plans/codegen-pareto-D.md` names the change.
//!
//! One consequence to be honest about: `cargo xtask diff-eval` compares the
//! evaluator against the **default** compile path, so it cannot see this
//! pass at all. `verify_successors` is what stands in its place — the
//! pass proves CFG equivalence for every fn it moves, on real programs, and
//! refuses to emit a body it cannot prove.

use std::collections::BTreeMap;

use crate::codegen;
use crate::cost::{BlockClass, LayoutClasses};
use crate::mwir::{Inst, MwirFn, MwirProgram};

/// The MWIR-block partition of one body: `(start, end)` per block, in
/// original order. `end` is exclusive; the last block ends at `body.len()`.
///
/// Empty for an empty body, which is the one case with no blocks at all.
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

/// Whether a block ending at `end` (exclusive) falls through to `end`.
///
/// Exactly the complement of `codegen::mwir_block_leaders`'s two
/// unconditional terminators. `JumpIfFalse` **does** fall through (it
/// branches only when the condition is false), and so does every
/// non-control instruction.
fn falls_through(body: &[Inst], end: usize) -> bool {
    match body.get(end.wrapping_sub(1)) {
        Some(Inst::Jump { .. } | Inst::Return { .. }) => false,
        Some(_) => true,
        // An empty block cannot exist (`block_ranges` never produces one),
        // but if it somehow did, treating it as falling through is the
        // safe direction: an unnecessary `b` is correct, a missing one is
        // not.
        None => true,
    }
}

/// One fn's layout plan: the new order of its blocks, by original block
/// index.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FnLayout {
    /// A permutation of `0..block_count`.
    pub order: Vec<usize>,
    /// Blocks classified `Hot`.
    pub hot: usize,
    /// Blocks classified `Cold` — the ones this plan sinks.
    pub cold: usize,
    /// Blocks the sidecar has no evidence about. Laid out **hot**
    /// (decision 1752).
    pub unmeasured: usize,
    /// Fallthrough edges the permutation broke, each costing one extra
    /// `Inst::Jump` word.
    pub repairs: usize,
    /// **Where original block `b` went**, as the half-open range of block
    /// ordinals it occupies in the *post-pass* partition:
    /// `new_block_span[b] == (lo, hi)`.
    ///
    /// Usually one block wide. It is two exactly when block `b` needed a
    /// repair and ended in a conditional branch, because the appended
    /// `Jump` is then a leader of its own (see the module doc). This vector
    /// is the block **identity** a post-pass partition needs and that
    /// `cost::bridge` infers from position instead — decision 1755's whole
    /// subject, and the reason it is carried rather than recomputed.
    pub new_block_span: Vec<(usize, usize)>,
}

impl FnLayout {
    /// True when the plan moves nothing — the degrade path's whole
    /// contract (decision 1752).
    pub fn is_identity(&self) -> bool {
        self.order.iter().enumerate().all(|(i, &b)| i == b)
    }
}

/// Plan one fn's block order from its per-block classes.
///
/// `classes[k]` is the class of block `k` of `body`'s partition; the caller
/// gets it from [`LayoutClasses::class_of`] at `(fn_key, k)`. A `classes`
/// shorter than the partition is a caller bug and errors rather than
/// defaulting — a block with no class must never be laid out as if it had
/// one (fail closed).
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
            // Decision 1752: no evidence is not evidence of coldness.
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

    // One repair per block whose fallthrough successor is no longer the
    // block physically after it.
    let mut position_of = vec![0usize; blocks.len()];
    for (p, &b) in order.iter().enumerate() {
        position_of[b] = p;
    }
    let mut repairs = 0usize;
    let mut new_block_span = vec![(0usize, 0usize); blocks.len()];
    let mut next_ordinal = 0usize;
    for (p, &b) in order.iter().enumerate() {
        let (_, end) = blocks[b];
        // The successor block is the one starting at `end`; `end ==
        // body.len()` means the fn's epilogue, which only the physically
        // last block reaches by falling through.
        let repaired = falls_through(body, end)
            && !match blocks.iter().position(|&(s, _)| s == end) {
                Some(succ) => position_of[succ] == p + 1,
                None => p + 1 == order.len(),
            };
        if repaired {
            repairs += 1;
        }
        // A repair appended after a conditional branch is a leader of its
        // own; after an ordinary instruction it joins the block it repairs.
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

/// Where every old body index lands under `plan`, plus one final entry:
/// `map[body.len()]` is the new body length, which is the epilogue position
/// `Inst::Return` branches to (`codegen::emit_fn` resolves it through
/// `word_offsets[body.len()]`).
///
/// Also the relabelling a *wired* version of this pass would hand the Lane 2
/// span recorder, which is why it is public rather than a local of
/// [`apply_fn`].
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

/// Apply a plan to one fn, producing the reordered body with every target
/// remapped and every broken fallthrough repaired.
///
/// The identity plan returns a body equal to the input, instruction for
/// instruction (`unit:an_identity_plan_is_byte_identical`).
pub fn apply_fn(f: &MwirFn, plan: &FnLayout) -> Result<MwirFn, String> {
    let body = &f.body;
    let blocks = block_ranges(body);
    let new_index = new_index_map(body, plan)?;
    let at = new_index[body.len()];
    let mut position_of = vec![0usize; blocks.len()];
    for (p, &b) in plan.order.iter().enumerate() {
        position_of[b] = p;
    }

    // Pass 2: emit.
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

/// The successors of body index `i`, in that body's own index space, where
/// `body.len()` means the fn epilogue (`Inst::Return`'s destination, which
/// `codegen::emit_fn` resolves through `word_offsets[body.len()]`).
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

/// **The pass's own correctness invariant, checked on every fn it moves.**
///
/// A permutation is correct exactly when it preserves the successor
/// relation: for every original index `i`, the successors of `i` in the new
/// body — resolved through any inserted repair jump, which is pure
/// forwarding — must be the image under `new_index` of `i`'s original
/// successors.
///
/// This runs on real programs rather than only on the synthetic bodies in
/// `unit:the_permuted_body_has_the_same_successor_relation`, which matters
/// because `diff-eval` cannot reach this pass at all: it exercises the
/// default compile path, and item D is not on it (decision 1755). So the
/// evaluator-vs-backend oracle says nothing about a reordered body, and
/// this check is what stands in its place. It fails closed — a permutation
/// it cannot prove equivalent does not get emitted.
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

/// What [`relayout_program`] did, for the findings file and for the units.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LayoutSummary {
    /// Fns whose plan was not the identity.
    pub fns_moved: usize,
    /// Fns considered (every fn of the program).
    pub fns_total: usize,
    pub hot: usize,
    pub cold: usize,
    pub unmeasured: usize,
    /// Extra `Inst::Jump` words the repairs cost, program-wide.
    pub repairs: usize,
    /// The per-fn plan, by fn key. Keyed by the same `fn_key` the sidecar
    /// and `codegen::block_spans` use, so a caller can follow an original
    /// block index to where it landed ([`FnLayout::new_block_span`]).
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

/// Lay out every sync MWIR fn of `program` against `classes`.
///
/// **Sync fns only** (decision 1756): async fns are emitted from FlowWir
/// through `codegen`'s state-machine path, whose flattened stream is
/// indexed by `state_flat_base` from the dispatch header, and permuting it
/// is a different job with a different correctness argument. Async fns keep
/// their emission order and this is reported, not hidden — `MwirProgram`
/// simply does not contain them.
///
/// [`LayoutClasses::Unmeasured`] — no sidecar beside the source, or no
/// source — makes every class `Unmeasured`, every plan the identity, and
/// this function a deep clone. That is the degrade path and it is the one
/// this repo will take for every program but one.
pub fn relayout_program(
    program: &MwirProgram,
    classes: &LayoutClasses,
) -> Result<(MwirProgram, LayoutSummary), String> {
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
        },
        summary,
    ))
}

// ---------------------------------------------------------------------------
// The 2 MiB same-region property (decision 1705 / decision 1754)
// ---------------------------------------------------------------------------

/// SOG §4.8's region size, in bytes: "keep a branch and its target within
/// the same 2 MiB region". The cost table carries the same number in
/// `[branch.region_bytes]`; this constant is the *layout* side of it and
/// [`same_region_holds`] is checked against the table row by
/// `unit:the_region_constant_agrees_with_the_cost_table`.
pub const REGION_BYTES: u64 = 2 * 1024 * 1024;

/// Whether every branch in `[lo, hi)` necessarily shares a 2 MiB region
/// with its target — true exactly when the whole code span sits inside one
/// aligned region.
///
/// This is the property decision 1705 asks for, stated directly. It is
/// **stronger** than "the base is 2 MiB-aligned" (a 2 MiB-aligned base
/// followed by more than 2 MiB of code still straddles) and it is what
/// SOG §4.8 actually says.
pub fn same_region_holds(lo: u64, hi: u64) -> bool {
    if hi <= lo {
        return true;
    }
    lo / REGION_BYTES == (hi - 1) / REGION_BYTES
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
        // 0: b0  const                 (block 0, falls through)
        // 1:     jumpiffalse -> 4      (block 0 tail)
        // 2: b1  const                 (block 1, the cold arm)
        // 3:     jump -> 6             (block 1 tail)
        // 4: b2  const                 (block 2, the hot arm)
        // 5:     jump -> 6
        // 6: b3  return                (block 3)
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

    /// The named oracle: a synthetic hot/cold program orders as expected —
    /// hot blocks contiguous, cold blocks out of the hot region.
    #[test]
    fn a_synthetic_hot_cold_program_packs_its_hot_blocks() {
        let b = body_hot_cold();
        let classes = vec![
            BlockClass::Hot,  // 0 entry
            BlockClass::Cold, // 1 the rare arm
            BlockClass::Hot,  // 2 the taken arm
            BlockClass::Hot,  // 3 the exit
        ];
        let plan = plan_fn(&b, &classes).expect("plan");
        assert_eq!(
            plan.order,
            vec![0, 2, 3, 1],
            "cold block 1 sinks to the end"
        );
        assert_eq!((plan.hot, plan.cold, plan.unmeasured), (3, 1, 0));

        let out = apply_fn(&fnof(b), &plan).expect("apply");
        // The hot run is now contiguous: block 0, then block 2, then block
        // 3, and only then the sunk cold block.
        assert_eq!(
            out.body,
            vec![
                cbool(0),
                // block 0's conditional now branches to block 2, which
                // starts at index 3 — index 2 is block 0's own repair.
                Inst::JumpIfFalse {
                    cond: Temp(0),
                    target: 3,
                },
                // Block 0's fallthrough to block 1 is broken, so it gains
                // one repair jump to block 1's new home (index 6).
                Inst::Jump { target: 6 },
                cbool(2),
                Inst::Jump { target: 5 }, // block 2 -> block 3, remapped
                Inst::Return { value: None },
                cbool(1),
                Inst::Jump { target: 5 }, // block 1 -> block 3, remapped
            ]
        );
        assert_eq!(plan.repairs, 1);
        // The hot text is now the prefix `0..6` and the cold block is the
        // suffix `6..8` — which is the whole point of the item.
        let ranges = block_ranges(&out.body);
        assert_eq!(ranges, vec![(0, 2), (2, 3), (3, 5), (5, 6), (6, 8)]);
    }

    /// The honest cost of a repair, measured rather than assumed: a repair
    /// appended after a **conditional** branch is a leader of its own, so
    /// the block count grows by one; after an ordinary instruction it is
    /// not, and the count does not move.
    #[test]
    fn a_repair_after_a_conditional_costs_one_block() {
        // Case 1: block 0 ends in `JumpIfFalse`, so its repair splits off.
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

        // Case 2: the repaired block ends in an ordinary instruction, so
        // the repair joins it and the count does not move.
        //  0: const           (block 0)
        //  1: jumpiffalse -> 4
        //  2: const           (block 1 — ordinary tail, falls through)
        //  3: const
        //  4: return          (block 2)
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
        // Two repairs: block 0's broken fallthrough (after a conditional,
        // +1 block) and block 1's (after an ordinary instruction, +0).
        assert_eq!(plan2.repairs, 2);
        assert_eq!(block_ranges(&out2.body).len(), block_ranges(&b2).len() + 1);
    }

    /// The degrade path, and the one most likely to silently misbehave:
    /// no measurement at all must be byte-identical to today.
    #[test]
    fn no_sidecar_degrades_to_a_byte_identical_layout() {
        let f = fnof(body_hot_cold());
        let program = MwirProgram {
            fns: BTreeMap::from([("F.m".to_string(), f.clone())]),
            rodata: vec![],
        };
        let (out, summary) =
            relayout_program(&program, &LayoutClasses::Unmeasured).expect("relayout");
        assert_eq!(out.fns["F.m"].body, f.body, "not one instruction moved");
        assert_eq!(summary.fns_moved, 0);
        assert_eq!(summary.repairs, 0);
        assert_eq!(summary.unmeasured, 4, "all four blocks, and none sunk");
        assert_eq!(summary.cold, 0);
    }

    /// Decision 1752, stated as a test rather than as a comment:
    /// `Unmeasured` is laid out with the hot run and is never sunk, even
    /// when real cold blocks exist beside it.
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
        // And the reverse reading — treating Unmeasured as Cold — would
        // have produced this instead, which is the bug this test exists to
        // catch.
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

    /// The correctness oracle: the reordered body has **the same successor
    /// relation**, instruction for instruction.
    ///
    /// For every old index `i` the new body's successor set at
    /// `new_index[i]` — resolved through any inserted repair jump, which is
    /// a pure forwarding instruction — must be exactly the image of `i`'s
    /// old successor set under `new_index`. Checked over every
    /// hot/cold assignment of a four-block body, so the permutation is
    /// exercised in every direction rather than in one.
    #[test]
    fn the_permuted_body_has_the_same_successor_relation() {
        let b = body_hot_cold();
        let n = b.len();

        // Successors of old index i, in old index space (n == epilogue).
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

            // A repair jump forwards; resolve through it (never a cycle —
            // an inserted jump always targets a real instruction).
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
    fn same_region_is_the_span_property_not_the_base_property() {
        // The current image: code 0x40500050..0x40515610.
        assert!(same_region_holds(0x4050_0050, 0x4051_5610));
        // A 2 MiB-aligned base does not by itself buy the property.
        assert!(!same_region_holds(
            0x4060_0000,
            0x4060_0000 + REGION_BYTES + 4
        ));
        // …and an unaligned base that stays inside one region does.
        assert!(same_region_holds(0x4050_0050, 0x405F_FFFC));
        // Straddling the boundary is exactly what §4.8 warns about.
        assert!(!same_region_holds(0x405F_FFF0, 0x4060_0010));
    }

    /// [`FnLayout::new_block_span`] must say exactly where the applied body
    /// put each original block. Checked by re-deriving the post-pass
    /// partition and matching widths, over every hot/cold assignment.
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

            // Spans must tile `0..after.len()` in physical order …
            let mut covered = 0usize;
            for &p in &plan.order {
                let (lo, hi) = plan.new_block_span[p];
                assert_eq!(lo, covered, "bits={bits:b} block {p}");
                covered = hi;
            }
            assert_eq!(covered, after.len(), "bits={bits:b}");

            // … and each one must start at the new index of that block's
            // own first instruction.
            for (p, &(s, _)) in before.iter().enumerate() {
                let (lo, _) = plan.new_block_span[p];
                assert_eq!(after[lo].0, map[s], "bits={bits:b} block {p}");
            }
        }
    }

    /// **The number that justifies the item** (plans/codegen-pareto.md item
    /// D's cheap oracle), measured end to end on the one program in the
    /// tree that has a block-grain sidecar.
    ///
    /// Emits the real `boot-actors` cost-stage closure twice — once as the
    /// compiler emits it today, once with item D's layout applied — and
    /// asks the **unmodified** `cost::footprint::compute` for each core's
    /// measured hot text. No model was changed and no term was made
    /// order-sensitive (decision 1750); the measured term already is, and
    /// this is what it says.
    ///
    /// The `after` hot predicate is built from [`FnLayout::new_block_span`]
    /// rather than from `MeasuredBlocks::resolve`, because after the pass
    /// the sidecar's `fn#k` no longer names the k-th *emitted* block —
    /// which is decision 1755 in one sentence, and the reason this pass is
    /// not on the compile path.
    ///
    /// Numbers land in `plans/codegen-pareto-D.md`. Print them with:
    /// `cargo test -p wrela-compiler --lib
    /// blocklayout::tests::the_measured_hot_text_footprint_before_and_after
    /// -- --nocapture`
    #[test]
    fn the_measured_hot_text_footprint_before_and_after() {
        /// Measured hot text of the `boot-actors` cost-stage closure as the
        /// compiler emits it today — item D's baseline, not a property of
        /// item D. Every word-shrinking opt moves it; see the assertion
        /// below for what to do when it does.
        const BEFORE_HOT_TEXT_BYTES: u64 = 7616;

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

        // --- before: exactly what the compiler emits today ---------------
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

        // The per-fn packing floor: what the footprint term would say if
        // every fn's hot blocks were perfectly contiguous from its own
        // 64 B-aligned base. `footprint::compute` gives each fn such a base,
        // so this is the hard lower bound on what *any* intra-fn block
        // ordering can reach — the headroom the item is spending.
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

        // --- after: the same closure under item D's layout ---------------
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

        // Hot word blocks of the *after* program, followed through the
        // block identity the pass carries.
        let bridged: BTreeMap<&String, &cost::BridgedBlock> = bridge_after.blocks().collect();
        let mut hot_words: BTreeMap<String, BTreeSet<usize>> = BTreeMap::new();
        for (key, count) in &counts {
            if *count == 0 {
                continue;
            }
            let (fn_key, orig) = cost::split_key(key).expect("a committed sidecar key");
            let ords = match summary.plans.get(fn_key) {
                // A fn the pass did not touch (async, or absent from this
                // closure) keeps its ordinals.
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

        // The same set of blocks must still be hot — the pass moved code,
        // it did not reclassify it.
        let words_before: u64 = before.fns.values().map(|f| f.code.len() as u64).sum();
        let words_after: u64 = after.fns.values().map(|f| f.code.len() as u64).sum();
        // **plans/codegen-pareto-F.md, decision 1778.** This held with a
        // caveat for one revision of item F3 and holds unqualified again.
        // F3's first landing relaxed the allocator's two-read rule to
        // reach framelessness, and reordering blocks could then cost a
        // function its residency and hand it back a frame — four words
        // (`sub sp`, `str x30`, `ldr x30`, `add sp`) that were neither a
        // repair nor accounted anywhere. The ∀ gate refused that
        // relaxation for unrelated reasons and the interaction went with
        // it: F3 now deletes the `x30` save from every leaf, which no
        // block ordering can undo, so `frameless(before) == frameless(after)`
        // and this reads as it always did. The term stays because it is
        // what makes the equality a *measurement* rather than an
        // assumption about a pass that is not on the compile path.
        let frameless = |p: &crate::codegen::CodegenProgram| -> u64 {
            p.fns.values().filter(|f| f.frame_size == 0).count() as u64
        };
        const FRAME_WORDS: u64 = 4;
        let regained = frameless(&before).saturating_sub(frameless(&after));
        assert_eq!(regained, 0, "item F3's frames must survive a reordering");
        assert_eq!(
            words_after,
            words_before + summary.repairs as u64 + regained * FRAME_WORDS,
            "every extra word must be an accounted repair jump"
        );

        // Decision 1753's oracle on a real build: the partition this pass
        // reorders is exactly the partition Lane 2 keyed its ids over.
        for (key, plan) in &summary.plans {
            let recorded = spans_before.iter().filter(|s| &s.fn_key == key).count();
            if recorded == 0 {
                continue; // the counter helper, which carries no spans
            }
            assert_eq!(
                plan.order.len(),
                recorded,
                "fn `{key}`: blocklayout partitions it into {} block(s), bridge mode recorded {}",
                plan.order.len(),
                recorded
            );
        }

        // What it cost: the flat (all-hot) footprint is the static-text row
        // decision 1617's veto is argued against, and the repair jumps are
        // real words in it. Reported beside the win, never netted against
        // it.
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

        eprintln!("D-MEASURE {}", summary.render());
        eprintln!(
            "D-MEASURE fns sync={} total={}",
            summary.fns_total,
            before.fns.len()
        );
        eprintln!("D-MEASURE words before={words_before} after={words_after}");
        eprintln!(
            "D-MEASURE flat_hot_text before={} after={}",
            flat_before[0].hot_text_bytes, flat_after[0].hot_text_bytes
        );
        eprintln!(
            "D-MEASURE hot_bytes={hot_bytes} per_fn_packing_floor={floor} \
             headroom={} captured={}",
            budget_before[0].hot_text_bytes.saturating_sub(floor),
            budget_before[0]
                .hot_text_bytes
                .saturating_sub(budget_after[0].hot_text_bytes)
        );
        for (b, a) in budget_before.iter().zip(budget_after.iter()) {
            eprintln!(
                "D-MEASURE core={} measured_hot_text before={} after={} \
                 lines {}->{} pages {}->{} charge {}->{}",
                b.n,
                b.hot_text_bytes,
                a.hot_text_bytes,
                b.hot_text_bytes / 64,
                a.hot_text_bytes / 64,
                b.text_pages,
                a.text_pages,
                b.charge,
                a.charge
            );
        }

        // The pinned facts. These are the item's claim; if a future change
        // moves them the claim is re-argued, not silently updated.
        assert_eq!(budget_before.len(), budget_after.len());
        assert!(!budget_before.is_empty(), "boot-actors places one core");
        assert_eq!(
            budget_before[0].hot_text_bytes, BEFORE_HOT_TEXT_BYTES,
            "item D's baseline moved.\n\
             \n\
             This is the assertion doing its job, not a bug in it. \
             `{BEFORE_HOT_TEXT_BYTES}` is the measured hot text of the `boot-actors` \
             cost-stage closure **as the compiler emits it today**, so *any* opt that \
             deletes or adds words moves it — and item D's whole claim is a delta \
             against it. It has already moved twice on this plan: 7744 at item A, 7680 \
             once item B's one-word `ADR` addressing merged, {BEFORE_HOT_TEXT_BYTES} \
             once item C's arithmetic opts did.\n\
             \n\
             What to do: re-run this test with `-- --nocapture`, re-pin \
             `BEFORE_HOT_TEXT_BYTES` to what it prints, and **re-measure every number \
             in `plans/codegen-pareto-D.md`** — the delta, the density figure and the \
             packing headroom all move with the baseline and none of them may be \
             rescaled arithmetically. Do not update this constant without doing that; \
             a green test over stale prose is worse than a red one."
        );
        assert!(
            budget_after[0].hot_text_bytes <= budget_before[0].hot_text_bytes,
            "packing the hot blocks must never grow the hot line set: {} -> {}",
            budget_before[0].hot_text_bytes,
            budget_after[0].hot_text_bytes
        );
        assert_eq!(
            budget_before[0].charge, budget_after[0].charge,
            "both sides are inside the L1I budget, so the model charges neither"
        );
    }

    /// A stale sidecar must **fail the build**, not lay out an image.
    ///
    /// Item A owns the three staleness directions; this is item D's own
    /// obligation: the pass's pipeline entry point must never be reached
    /// with a classification the checker rejected. Driven through the real
    /// entry point with a real (deliberately shrunken) partition rather
    /// than a fixture, so a future refactor that swallows the error fails
    /// here.
    #[test]
    fn a_stale_sidecar_fails_the_build_rather_than_laying_out() {
        use crate::cost;

        let input = cost::repo_root().join("tests/golden/boot-actors/input.wr");
        crate::opts::apply_mode(crate::opts::CompileMode::Release);
        crate::codegen::set_block_bridge(true);
        let _ = cost::codegen_cost_stage(&input).expect("cost-stage codegen");
        let spans = crate::codegen::block_spans();
        crate::codegen::set_block_bridge(false);

        // A fresh partition classifies.
        assert!(
            cost::layout_classes(Some(&input), &spans)
                .expect("fresh")
                .is_measured()
        );

        // Now pretend a measured fn was recompiled into fewer blocks —
        // exactly what a stale profile looks like. `copy_line_buf_range` is
        // in both the sidecar and this closure (item A's table 2).
        let shrunk: Vec<_> = spans
            .iter()
            .filter(|s| !(s.fn_key == "copy_line_buf_range" && s.block_index >= 2))
            .cloned()
            .collect();
        assert!(shrunk.len() < spans.len(), "the fixture must remove blocks");
        let err = cost::layout_classes(Some(&input), &shrunk).expect_err("stale must fail closed");
        assert!(err.contains("is stale"), "{err}");

        // And an empty partition — the caller forgot bridge mode — is a
        // failure too, never a silent "nothing to lay out".
        assert!(cost::layout_classes(Some(&input), &[]).is_err());
    }

    #[test]
    fn the_region_constant_agrees_with_the_cost_table() {
        let table = crate::cost::load_default().expect("cost table");
        let row = table
            .branch_row("region_bytes")
            .expect("[branch.region_bytes] is SOG §4.8's region size");
        assert_eq!(
            row.value, REGION_BYTES,
            "the layout constant and the cost table must mean the same region"
        );
    }
}
