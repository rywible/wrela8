//! Hot/cold basic-block layout (plans/codegen-pareto.md item D).
//!
//! **PARKED — present, compiling, tested, and deliberately not wired**
//! (CLAUDE.md's "a refused opt is parked, not deleted";
//! plans/codegen-pareto-2.md decisions 1910 and 1940). The three things the
//! parking rule requires, stated here so they cannot drift from the code:
//!
//! - **The measurement that refused it.** Under the order-sensitive
//!   footprint term (item K, decision 1955) this pass moves `boot-actors`'
//!   *measured* density charge, but the column the ∀ gate reads is
//!   `HotBlocks::All`, where the density charge is **identically zero by
//!   construction** — every block hot means every fn's hot bytes are all
//!   its bytes, so the fetched line count equals the packing floor and the
//!   slack is zero. The pass therefore scores 0 in the gate under every
//!   wiring available in this tree (item K, decision 1956).
//! - **The mechanism.** Sinking cold blocks only ever shrinks a *fetched
//!   line set*, and a line set can only shrink where some blocks are cold.
//!   The gate's column has no cold blocks. Separately, only one program in
//!   the tree carries a block-grain sidecar
//!   (`tests/golden/boot-actors/lane2-freq.txt`), so even a gate reading
//!   the measured column could see this pass move at most one case.
//! - **What would make it worth re-asking — and it has already half
//!   happened.** Item M's `boot-tile-compositor` is the first program here
//!   with L1I headroom (47 744 B of flat hot text in `dev`, 28 480 B in
//!   `release`, against a 65 536 B L1I). Item O ran `cargo xtask
//!   gen-lane2-freq boot-tile-compositor` once, off-tree, and measured this
//!   pass against that sidecar: **measured hot text 28 736 → 26 688 B and
//!   density charge 63 → 0**, 16 of 32 fns moved, 88 cold blocks sunk, for
//!   +244 words. On the workload that did not exist when it was refused,
//!   the pass does exactly what it claims — the opposite of its result on
//!   `boot-actors`. Two things still block ranking it, and both are the
//!   ruler's, not the pass's: the ∀ gate reads `HotBlocks::All` (zero slack
//!   by construction), and that sidecar does **not resolve** against a
//!   `RELEASE_OPTS` closure at all, because `gen-lane2-freq` measures a
//!   `dev` image while `--stage=cost` scores a `release` one and item J's
//!   `Dce` now makes those two partitions disagree (decision 1947). The
//!   numbers above are therefore taken with `ConstProp`/`Gvn`/`Dce` off,
//!   which is the only configuration where the bridge resolves.
//!   `plans/codegen-pareto-2-O.md` has the full run.
//! - **The other rung, untouched.** `plans/codegen-pareto-D.md` §8.4
//!   measured 36 % of hot blocks in **async** fns, which this pass does not
//!   reach at all (decision 1756).
//!
//! Wiring it also still re-keys the Lane 2 bridge — see "Why this pass is
//! not installed on the emission path" below, which is unchanged.
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
        },
        summary,
    ))
}

/// How many times [`relayout_program`] has run on this thread.
///
/// The parked pass's own tripwire (decision 1940): "it is not on the
/// compile path" is a claim about what *runs*, so it is counted rather
/// than argued — see `unit:the_parked_pass_is_not_on_the_compile_path`.
/// Test-only; the shipped compiler carries no counter.
#[cfg(test)]
thread_local! {
    static RELAYOUT_CALLS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

#[cfg(test)]
fn relayout_calls() -> usize {
    RELAYOUT_CALLS.with(|c| c.get())
}

// ---------------------------------------------------------------------------
// The 2 MiB same-region property (decision 1705 / decision 1754) lives in
// `layout.rs`, not here — item K moved it to its only consumer when it
// deleted this module (decision 1956), and decision 1943 keeps it there.
// The property is independent of this pass and outlives it:
// `layout::verify_branch_region` still fails any image build whose
// branchable text straddles a 2 MiB region, and its two units
// (`same_region_is_the_span_property_not_the_base_property`,
// `the_region_constant_agrees_with_the_cost_table`) live beside it.
// Restoring the pass must not un-do that consolidation.
// ---------------------------------------------------------------------------

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
        const BEFORE_HOT_TEXT_BYTES: u64 = 6080;

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
        let frameless = |p: &crate::codegen::CodegenProgram| -> u64 {
            p.fns.values().filter(|f| f.frame_size == 0).count() as u64
        };
        let regained = frameless(&before).saturating_sub(frameless(&after));

        // Which fns changed, and by how much — printed before anything is
        // asserted, because the interesting failures of this test are
        // *interactions* with other opts and a bare `left != right` hides
        // them (decision 1944).
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

        // Decision 1753's oracle on a real build: the partition this pass
        // reorders is exactly the partition Lane 2 keyed its ids over.
        // Collected rather than asserted inline so the whole measurement
        // prints before anything fails (decision 1944).
        let mut partition_mismatch: Vec<(String, usize, usize)> = Vec::new();
        for (key, plan) in &summary.plans {
            let recorded = spans_before.iter().filter(|s| &s.fn_key == key).count();
            if recorded == 0 {
                continue; // the counter helper, which carries no spans
            }
            if plan.order.len() != recorded {
                partition_mismatch.push((key.clone(), plan.order.len(), recorded));
            }
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
             against it. It has already moved four times: 7744 at item A, 7680 once \
             item B's one-word `ADR` addressing merged, 7616 once item C's arithmetic \
             opts did, and {BEFORE_HOT_TEXT_BYTES} once codegen-pareto-2's items I \
             (argument/return hinting) and J (ConstProp/Gvn/Dce) landed.\n\
             \n\
             What to do: re-run this test with `-- --nocapture`, re-pin \
             `BEFORE_HOT_TEXT_BYTES` to what it prints, and **re-measure every number \
             in `plans/codegen-pareto-D.md`** — the delta, the density figure and the \
             packing headroom all move with the baseline and none of them may be \
             rescaled arithmetically. Do not update this constant without doing that; \
             a green test over stale prose is worse than a red one."
        );
        // **Decision 1941 — an invariant that is genuinely gone, recorded
        // rather than relaxed.** This assertion used to read
        //
        //     assert!(after.hot_text_bytes <= before.hot_text_bytes,
        //             "packing the hot blocks must never grow the hot line set");
        //
        // and it is now **false**: 6080 → 6528 B, 95 → 102 fetched lines.
        // The pass did not stop packing; the allocator started reacting to
        // the packing. `regalloc::allocate` builds each temp's live
        // interval as `[first point, last point]` in *emission* order and
        // item I's `hint_admissible` (decision 1902) walks that whole
        // interval to decide whether an argument/return register is free
        // over it. Sinking a cold block that used a value stretches that
        // value's interval across the entire function, the hint stops
        // being admissible, the temp loses its home and goes back to the
        // frame — and every word that costs is a *hot* word, because it is
        // spill traffic in the hot blocks. Leave-one-out over
        // `RELEASE_OPTS` attributes the whole effect to `OptId::RegAlloc`
        // (decision 1942): with it off the reordered program is exactly
        // `+repairs` words, with it on it is `+repairs + 207` under every
        // other combination.
        //
        // So the pinned pair below is a *measurement of the interaction*,
        // not a claim that the pass wins. The pass's honest verdict on
        // `boot-actors` today is: it lowers the modelled density charge and
        // raises the real hot-text footprint.
        assert_eq!(
            (
                budget_before[0].hot_text_bytes,
                budget_after[0].hot_text_bytes
            ),
            (6080, 6528),
            "the parked pass's measured hot text. The `after` side is *larger*, which \
             is decision 1941: item I's argument/return hinting makes residency \
             order-dependent, so reordering blocks costs spill words. Do not restore \
             the old `<=` — re-measure and re-argue."
        );
        // **plans/codegen-pareto-2.md item K3 (decision 1955).** This
        // assertion used to read `charge == charge`, both zero: both sides
        // are far inside the L1I, and the only thing the footprint term
        // charged for was overflow — so the model could not see block order
        // at all, which is decision 1750's "the forall gate scores this at
        // zero, and here is why". The density term charges the slack
        // between the fetched line set and the per-fn packing floor, and it
        // does see the order:
        assert!(
            budget_after[0].charge < budget_before[0].charge,
            "packing the hot blocks must now be visible to the model: {} -> {}",
            budget_before[0].charge,
            budget_after[0].charge
        );
        assert_eq!(
            (budget_before[0].charge, budget_after[0].charge),
            (84, 49),
            "item D's measured win under the order-sensitive footprint term (91 -> 49 \
             when item K measured it; the baseline moved with items I and J). Pinned, \
             not tracked, for decision 1757's reason: when this moves, re-measure \
             plans/codegen-pareto-D.md rather than rescaling it.\n\
             \n\
             **Decision 1945 — read this number next to the one above.** The charge \
             falls (84 -> 49) while the hot text it is supposed to describe *rises* \
             (6080 -> 6528 B). Both are correct: the density term charges \
             `fetched_lines - per_fn_packing_floor`, and the floor is computed from \
             the program being scored, so a pass that makes each fn bigger raises its \
             own floor and books the difference as less slack. The term ranks \
             orderings of a *fixed* program, which is what item K built it for; it is \
             not a footprint metric and must not be read as one."
        );

        // **The word budget, decomposed rather than asserted away.** This
        // used to be `words_after == words_before + repairs + regained*4`
        // and it no longer closes: the same interaction above adds spill
        // words that are neither repairs nor prologue words. Pin all four
        // quantities so the *shape* of the excess is visible when it moves.
        assert_eq!(
            (words_before, words_after, summary.repairs, regained),
            (1982, 2204, 15, 1),
            "the parked pass's word budget on boot-actors. 222 extra words for 15 \
             repair jumps: 15 repairs + 4 prologue words (`__wrela_line_commit` \
             regains a frame, 0 -> 160 bytes) + 203 words of spill traffic the \
             allocator emits because the reordering lengthened live intervals \
             (decision 1942)."
        );
        assert_eq!(
            words_after - words_before - summary.repairs as u64,
            207,
            "unattributed word growth. Leave-one-out over RELEASE_OPTS puts all of it \
             on `OptId::RegAlloc`: this figure is 0 with the allocator off (dev, and \
             release-minus-RegAlloc) and 207 with every other single opt removed."
        );

        // **Decision 1948 — decision 1753's identity no longer holds for
        // every fn, and that is a finding, not a test to loosen.** The
        // reorder unit is the MWIR block; the Lane 2 id space is keyed over
        // the *emitted* partition. Item J's `mwir_opt` runs **inside**
        // `codegen`, after this pass has already planned, and its `Dce`
        // deletes whole blocks from app methods — so for `Ledger.mark` and
        // `Ledger.read_marks` this pass plans over a 2-block partition that
        // is emitted as 1 block. Attribution: the list below is empty with
        // `OptId::Dce` off and is exactly these two fns with it on.
        //
        // It is a *second*, independent reason not to wire the pass (the
        // first is decision 1755's positional bridge): a class looked up at
        // ordinal `k` no longer necessarily describes the run being moved.
        // Pinned as an exact list so a third fn joining it is a failure.
        assert_eq!(
            partition_mismatch
                .iter()
                .map(|(k, a, b)| (k.as_str(), *a, *b))
                .collect::<Vec<_>>(),
            vec![("Ledger.mark", 2, 1), ("Ledger.read_marks", 2, 1)],
            "the MWIR partition this pass reorders and the emitted partition Lane 2 \
             keys its ids over have diverged (decision 1948). See the comment above \
             before changing this list."
        );
    }

    /// **Decision 1942's attribution, as the control half of a
    /// leave-one-out** the measurement unit above is the treatment half of.
    ///
    /// With `OptId::RegAlloc` removed and everything else in `RELEASE_OPTS`
    /// left on, the reordered program is `words_before + repairs` words —
    /// **exactly**, no excess and no frame regained. The 207-word excess
    /// and the lost frame in the release measurement are therefore the
    /// allocator's reaction to the reordering and nothing else's. The full
    /// thirteen-way sweep was run once and is written up in
    /// `plans/codegen-pareto-2-O.md`; only the one leave-one-out that
    /// carries the whole effect is committed here, because the other twelve
    /// cost seconds to re-prove a null.
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
        assert!(summary.fns_moved > 0, "the fixture must move something");
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

    /// **The park's own oracle: the pass is present and is not wired.**
    ///
    /// Two independent checks, because either alone is weak.
    ///
    /// 1. **Dynamic.** A normal release build of a real program never
    ///    reaches [`relayout_program`] — counted, not argued. Then the
    ///    parked entry point is driven with the *measured* classification
    ///    that moves seven fns, and the normal build is repeated: it is
    ///    byte-identical, word for word, to the one taken before. A parked
    ///    pass that leaked into the compile path through a global would
    ///    fail here.
    /// 2. **Static.** No file in the crate calls the parked entry points
    ///    except the entry point's own definition and this test module. A
    ///    future session that wires it has to delete this assertion, which
    ///    is the point: wiring re-keys the Lane 2 bridge (decision 1755)
    ///    and diverges from the emitted partition (decision 1948), and both
    ///    are decisions, not edits.
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
        assert!(
            summary.fns_moved > 0,
            "the fixture must actually move fns, or this proves nothing"
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

        // (2) the static half.
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

    /// **The compositor measurement — decision 1946**, and the reason this
    /// module is back in the tree.
    ///
    /// Item K deleted this pass partly on the argument that no image in the
    /// tree has L1I headroom, so density can never matter. Item M's
    /// `boot-tile-compositor` landed hours later and is the first program
    /// here with real headroom. This unit asks the question that could not
    /// be asked then, on the workload that did not exist then, and pins
    /// the answer.
    ///
    /// Print the numbers with
    /// `cargo test -p wrela-compiler --lib
    /// blocklayout::tests::the_compositor_is_the_workload_that_could_re_ask
    /// -- --nocapture`.
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
                "O-COMPOSITOR {label}: words={words} hot_text={} hot_code={} \
                 slack_lines={} l1i={} charge={} pages={}",
                budget[0].hot_text_bytes,
                budget[0].hot_code_bytes,
                budget[0].slack_lines,
                budget[0].l1i_bytes,
                budget[0].charge,
                budget[0].text_pages
            );
            flat.push((words, budget[0].clone()));
        }

        // **The answer, in the tree, is a null with a named cause.** The
        // pass is the identity on this program, because it reorders on
        // *measured* coldness and no block-grain sidecar is committed here.
        //
        // Item O ran `cargo xtask gen-lane2-freq boot-tile-compositor` once
        // and measured the pass against the result (decision 1946): hot
        // text 28 736 → 26 688 B, density charge 63 → 0, 16/32 fns moved,
        // 88 cold blocks sunk, +244 words. That sidecar is **not**
        // committed, because it does not resolve against a `RELEASE_OPTS`
        // closure at all (decision 1947) and because committing it moves
        // `cost-product-compositor`'s golden. So the tree's answer is the
        // null below and the real number lives in the findings file — which
        // is what the assertion's message is for.
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

        // And the second half of the null, which no sidecar can fix: the
        // column the ∀ gate reads is `HotBlocks::All`, where `slack_lines`
        // is zero **by construction** (item K, decision 1955) — every block
        // hot means each fn's fetched line count *is* its packing floor.
        // So even a compositor sidecar would move the measured column only,
        // and ranking this pass would still need a gate that reads it.
        let after_flat = cost::footprint::compute(
            &after,
            &table,
            &SweepPoint::pinned(&table),
            &placement,
            HotBlocks::All,
        )
        .expect("footprint after");
        assert_eq!(flat[1].1.slack_lines, 0);
        assert_eq!(flat[1].1.charge, after_flat[0].charge);

        // The headroom that makes the question worth re-asking at all.
        // Pinned rather than tracked, for the same reason
        // `BEFORE_HOT_TEXT_BYTES` is: when it moves, decision 1946 is
        // re-argued from the new number, not rescaled from the old one.
        assert_eq!(
            (
                flat[0].1.hot_text_bytes,
                flat[1].1.hot_text_bytes,
                flat[1].1.l1i_bytes
            ),
            (47_744, 28_480, 65_536),
            "the compositor's flat hot text, dev and release, against the L1I. Item M's \
             ~17 KB-of-headroom figure is the **dev** column; release has 37 KB of \
             headroom, so the L1I overflow term is zero on both sides and the only \
             footprint term that could ever rank this pass here is the density one \
             (decision 1946). Re-measure before touching."
        );
        assert!(
            flat[1].1.hot_text_bytes < flat[1].1.l1i_bytes,
            "the compositor's release hot text must fit the L1I with room, or the \
             density argument changes shape: {} vs {}",
            flat[1].1.hot_text_bytes,
            flat[1].1.l1i_bytes
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
}
