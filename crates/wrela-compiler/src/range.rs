//! Conservative integer range analysis for MWIR.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;

use crate::cfg::BlockId;
use crate::flowwir::{FlowInst, FlowWirProgram};
use crate::mwir::{Inst, MwirFn, Temp};
use crate::sema::types::Type;
use crate::syntax::ast::BinOp;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Range {
    Bottom,
    Interval { lo: i128, hi: i128 },
    Top,
}

impl Range {
    pub fn interval(lo: i128, hi: i128) -> Range {
        if lo > hi {
            Range::Bottom
        } else {
            Range::Interval { lo, hi }
        }
    }

    pub fn join(&self, other: &Range) -> Range {
        match (self, other) {
            (Range::Bottom, x) | (x, Range::Bottom) => x.clone(),
            (Range::Top, _) | (_, Range::Top) => Range::Top,
            (Range::Interval { lo: a, hi: b }, Range::Interval { lo: c, hi: d }) => {
                Range::interval((*a).min(*c), (*b).max(*d))
            }
        }
    }

    pub fn proves(&self, lo: i128, hi: i128) -> bool {
        matches!(self, Range::Interval { lo: a, hi: b } if *a >= lo && *b <= hi)
    }

    pub fn render(&self) -> String {
        match self {
            Range::Bottom => "bottom".to_string(),
            Range::Top => "top".to_string(),
            Range::Interval { lo, hi } => format!("[{lo},{hi}]"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstructionRanges {
    pub before: Vec<Range>,
    pub after: Vec<Range>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Analysis {
    pub entry: Vec<Vec<Range>>,
    pub exit: Vec<Vec<Range>>,
    pub instructions: Vec<InstructionRanges>,
    pub widened: BTreeSet<BlockId>,
}

fn type_bounds(ty: &Type) -> Option<(i128, i128)> {
    match ty {
        Type::U8 => Some((0, u8::MAX as i128)),
        Type::U16 => Some((0, u16::MAX as i128)),
        Type::U32 => Some((0, u32::MAX as i128)),
        Type::U64 | Type::Usize => Some((0, u64::MAX as i128)),
        Type::I8 => Some((i8::MIN as i128, i8::MAX as i128)),
        Type::I16 => Some((i16::MIN as i128, i16::MAX as i128)),
        Type::I32 => Some((i32::MIN as i128, i32::MAX as i128)),
        Type::I64 | Type::Isize => Some((i64::MIN as i128, i64::MAX as i128)),
        _ => None,
    }
}

fn clamp(r: Range, ty: &Type) -> Range {
    let Some((lo, hi)) = type_bounds(ty) else {
        return r;
    };
    match r {
        Range::Interval { lo: a, hi: b } => Range::interval(a.max(lo), b.min(hi)),
        other => other,
    }
}

fn unknown_for_type(ty: &Type) -> Range {
    type_bounds(ty)
        .map(|(lo, hi)| Range::interval(lo, hi))
        .unwrap_or(Range::Top)
}

fn const_range(inst: &Inst) -> Option<(Temp, Range)> {
    match inst {
        Inst::ConstInt { dst, ty, value } => {
            Some((*dst, clamp(Range::interval(*value, *value), ty)))
        }
        Inst::ConstBool { dst, value } => {
            Some((*dst, Range::interval(*value as i128, *value as i128)))
        }
        _ => None,
    }
}

fn constant(r: &Range) -> Option<i128> {
    match r {
        Range::Interval { lo, hi } if lo == hi => Some(*lo),
        _ => None,
    }
}

/// Apply one instruction's transfer function to `state` in place.
///
/// An instruction can only change its own definitions, so rebuilding the whole
/// per-temp vector per instruction is pure copying: the fixpoint below visits
/// every instruction up to `MAX_ITERATIONS` times, which made the copy, not the
/// analysis, the dominant cost of compiling a large generated function.
///
/// `defs` is the instruction's definition list from `mwir_facts`, hoisted out
/// of the iteration loop by the caller. Every source is read before the first
/// write, so an instruction whose destination is also one of its sources still
/// observes the pre-state exactly as the copying formulation did.
fn transfer_into(
    inst: &Inst,
    defs: &[Temp],
    state: &mut [Range],
    types: &[Type],
    tracked: Option<&[bool]>,
) {
    if tracked.is_some_and(|tracked| {
        !defs
            .iter()
            .any(|temp| tracked.get(temp.0).copied().unwrap_or(false))
    }) {
        return;
    }
    let read = |t: Temp| state.get(t.0).unwrap_or(&Range::Top).clone();
    let update = match inst {
        Inst::ConstInt { .. } | Inst::ConstBool { .. } => const_range(inst),
        Inst::Copy { dst, src } => Some((*dst, read(*src))),
        Inst::ArithChecked {
            dst,
            op,
            ty,
            lhs,
            rhs,
            ..
        }
        | Inst::ArithWrapping {
            dst,
            op,
            ty,
            lhs,
            rhs,
            ..
        } => {
            let l = read(*lhs);
            let r = read(*rhs);
            let result = if let Some(c) = constant(&r) {
                add_const(l, *op, c, ty)
            } else if let Some(c) = constant(&l) {
                // Constant-minus-variable is not the same transfer as
                // variable-minus-constant.
                if matches!(op, BinOp::Add | BinOp::AddW) {
                    add_const(r, *op, c, ty)
                } else {
                    Range::Top
                }
            } else {
                Range::Top
            };
            Some((*dst, result))
        }
        Inst::Compare { dst, .. } | Inst::Not { dst, .. } | Inst::BoolAnd { dst, .. } => {
            Some((*dst, Range::interval(0, 1)))
        }
        Inst::Convert { dst, src, ty, .. } => {
            let source = read(*src);
            let converted = match (source, type_bounds(ty)) {
                (Range::Bottom, _) => Range::Bottom,
                (Range::Interval { lo, hi }, Some((dst_lo, dst_hi)))
                    if lo >= dst_lo && hi <= dst_hi =>
                {
                    Range::interval(lo, hi)
                }
                (_, Some((dst_lo, dst_hi))) => Range::interval(dst_lo, dst_hi),
                _ => Range::Top,
            };
            Some((*dst, converted))
        }
        _ => None,
    };
    for d in defs {
        if d.0 < state.len() {
            state[d.0] = types.get(d.0).map(unknown_for_type).unwrap_or(Range::Top);
        }
    }
    if let Some((t, r)) = update
        && t.0 < state.len()
    {
        state[t.0] = clamp(r, &types[t.0]);
    }
}

/// Definition lists for every instruction, computed once so the fixpoint does
/// not re-derive them on each visit.
fn instruction_defs(f: &MwirFn) -> Vec<Vec<Temp>> {
    f.body
        .iter()
        .map(|inst| crate::mwir_facts::inst_facts(inst).defs)
        .collect()
}

fn add_const(r: Range, op: BinOp, c: i128, ty: &Type) -> Range {
    let (lo, hi) = match r {
        Range::Interval { lo, hi } => (lo, hi),
        Range::Bottom => return Range::Bottom,
        Range::Top => return Range::Top,
    };
    let result = match op {
        BinOp::Add | BinOp::AddW => (lo.checked_add(c), hi.checked_add(c)),
        BinOp::Sub | BinOp::SubW => (lo.checked_sub(c), hi.checked_sub(c)),
        _ => return Range::Top,
    };
    match result {
        (Some(a), Some(b)) => {
            let result = Range::interval(a.min(b), a.max(b));
            match (&result, type_bounds(ty)) {
                (Range::Interval { lo, hi }, Some((ty_lo, ty_hi)))
                    if *lo < ty_lo || *hi > ty_hi =>
                {
                    // Both checked and wrapping arithmetic need a more precise
                    // transfer to say anything useful once the declared type
                    // boundary is crossed.  Top is safe; intersecting with the
                    // type bounds would be unsound for wrapping operations.
                    Range::Top
                }
                _ => result,
            }
        }
        _ => Range::Top,
    }
}

fn reaching_compare_in_block(
    f: &MwirFn,
    start: usize,
    branch_at: usize,
    cond: Temp,
) -> Option<(BinOp, Temp, Temp)> {
    let mut candidate = None;
    for inst in &f.body[start..branch_at.min(f.body.len())] {
        let facts = crate::mwir_facts::inst_facts(inst);
        if let Some((_, lhs, rhs)) = candidate {
            if facts.defs.contains(&lhs) || facts.defs.contains(&rhs) {
                candidate = None;
            }
        }
        if facts.defs.contains(&cond) {
            candidate = match inst {
                Inst::Compare {
                    dst, op, lhs, rhs, ..
                } if *dst == cond => Some((*op, *lhs, *rhs)),
                _ => None,
            };
        }
    }
    candidate
}

fn refine(ranges: &mut [Range], cmp: (BinOp, Temp, Temp), truth: bool) {
    let (mut op, mut lhs, mut rhs) = cmp;
    if constant(&ranges[lhs.0]).is_some() && constant(&ranges[rhs.0]).is_none() {
        std::mem::swap(&mut lhs, &mut rhs);
        op = match op {
            BinOp::Lt => BinOp::Gt,
            BinOp::Le => BinOp::Ge,
            BinOp::Gt => BinOp::Lt,
            BinOp::Ge => BinOp::Le,
            other => other,
        };
    }
    let Some(bound) = constant(&ranges[rhs.0]) else {
        return;
    };
    let Some(current) = ranges.get(lhs.0).cloned() else {
        return;
    };
    let refined = match (op, truth) {
        (BinOp::Lt, true) => upper(current, bound - 1),
        (BinOp::Lt, false) => lower(current, bound),
        (BinOp::Le, true) => upper(current, bound),
        (BinOp::Le, false) => lower(current, bound + 1),
        (BinOp::Gt, true) => lower(current, bound + 1),
        (BinOp::Gt, false) => upper(current, bound),
        (BinOp::Ge, true) => lower(current, bound),
        (BinOp::Ge, false) => upper(current, bound - 1),
        (BinOp::Eq, true) => Range::interval(bound, bound),
        _ => current,
    };
    ranges[lhs.0] = refined;
}

fn lower(r: Range, lo: i128) -> Range {
    match r {
        Range::Interval { lo: a, hi } => Range::interval(a.max(lo), hi),
        other => other,
    }
}

fn upper(r: Range, hi: i128) -> Range {
    match r {
        Range::Interval { lo, hi: b } => Range::interval(lo, b.min(hi)),
        other => other,
    }
}

/// One converged block-level fixpoint, plus the per-instruction definition
/// lists it hoisted. Callers that walk the body again reuse those lists rather
/// than re-deriving them.
struct BlockAnalysis {
    cfg: crate::cfg::Cfg,
    entry: Vec<Vec<Range>>,
    exit: Vec<Vec<Range>>,
    widened: BTreeSet<BlockId>,
    inst_defs: Vec<Vec<Temp>>,
}

fn analyze_blocks_tracked(f: &MwirFn, tracked_temps: &[usize]) -> Result<BlockAnalysis, String> {
    let cfg = crate::cfg::build_cfg(f)?;
    let n = f.temp_types.len();
    let mut tracked = vec![false; n];
    for &temp in tracked_temps {
        if let Some(slot) = tracked.get_mut(temp) {
            *slot = true;
        }
    }
    let inst_defs = instruction_defs(f);
    let mut entry = vec![vec![Range::Bottom; n]; cfg.blocks.len()];
    let mut exit = vec![vec![Range::Bottom; n]; cfg.blocks.len()];
    let mut widened = BTreeSet::new();
    let mut expansions = vec![vec![0u8; n]; cfg.blocks.len()];

    // Branch refinement is deliberately non-relational and can oscillate at
    // joins in the presence of a loop.  A finite cap is part of the
    // conservative lattice implementation: an unproven function becomes Top
    // rather than making compilation wait forever.
    const MAX_ITERATIONS: usize = 64;
    // Scratch buffers reused across every block and sweep. These are the only
    // per-block temporaries, and reallocating them was itself a visible share
    // of a large function's analysis.
    let mut incoming = vec![Range::Bottom; n];
    let mut refined_edge = vec![Range::Bottom; n];
    let mut current = vec![Range::Bottom; n];
    for iteration in 0..MAX_ITERATIONS {
        let mut changed = false;
        for b in 0..cfg.blocks.len() {
            for &temp in tracked_temps {
                incoming[temp] = Range::Bottom;
            }
            for &pred in &cfg.blocks[b].predecessors {
                let last = cfg.blocks[pred].range.end - 1;
                let mut refinement = None;
                if let Inst::JumpIfFalse { cond, target } = &f.body[last] {
                    if let Some(cmp) =
                        reaching_compare_in_block(f, cfg.blocks[pred].range.start, last, *cond)
                    {
                        let target_block =
                            (*target < f.body.len()).then(|| cfg.block_of_inst[*target]);
                        let fallthrough_block =
                            (last + 1 < f.body.len()).then(|| cfg.block_of_inst[last + 1]);
                        let truth = if target_block == Some(b) {
                            Some(false)
                        } else if fallthrough_block == Some(b) {
                            Some(true)
                        } else {
                            None
                        };
                        refinement = truth.map(|truth| (cmp, truth));
                    }
                }
                // Only a refined edge needs its own copy of the predecessor's
                // exit state; an unrefined edge joins straight out of `exit`.
                let edge = match refinement {
                    Some((cmp, truth)) => {
                        for &temp in tracked_temps {
                            refined_edge[temp] = exit[pred][temp].clone();
                        }
                        refine(&mut refined_edge, cmp, truth);
                        &refined_edge
                    }
                    None => &exit[pred],
                };
                for &temp in tracked_temps {
                    incoming[temp] = incoming[temp].join(&edge[temp]);
                }
            }
            if b == 0 {
                // Only the receiver and parameters are definitions at entry.
                // Other temps begin unknown until their defining instruction.
                if let Some((t, _)) = f.receiver {
                    if tracked[t.0]
                        && let Some((lo, hi)) = type_bounds(&f.temp_types[t.0])
                    {
                        incoming[t.0] = Range::interval(lo, hi);
                    }
                }
                for (t, _) in &f.params {
                    if tracked[t.0]
                        && let Some((lo, hi)) = type_bounds(&f.temp_types[t.0])
                    {
                        incoming[t.0] = Range::interval(lo, hi);
                    }
                }
            }
            for &temp in tracked_temps {
                current[temp] = incoming[temp].clone();
            }
            for at in cfg.blocks[b].range.clone() {
                transfer_into(
                    &f.body[at],
                    &inst_defs[at],
                    &mut current,
                    &f.temp_types,
                    Some(&tracked),
                );
            }
            // Widen before deciding whether this block changed. A widened temp
            // is restored to its type envelope, which is normally the value
            // already recorded in `exit[b]`, so comparing the pre-widening
            // value against that record reported a change on every sweep: the
            // block could never converge, the analysis ran all `MAX_ITERATIONS`
            // times, and the fallback below then discarded the *whole function*
            // as `Top`. That cost every affected function all of its bounds
            // proofs — measured over a Pixels renderer compile, 104 of 218
            // functions were reaching this state and losing their proofs.
            //
            // Widening still runs under exactly its original condition, since
            // `exit[b] != current` already implies the change test below. The
            // envelope is a sound over-approximation and is itself a fixed
            // point (type bounds cannot expand further), so recognising the
            // stable state is a precision gain, not a weakened bound.
            let exit_expanded = tracked_temps
                .iter()
                .any(|&temp| exit[b][temp] != current[temp]);
            if exit_expanded && cfg.blocks[b].predecessors.iter().any(|pred| *pred >= b) {
                for &t in tracked_temps {
                    let expands = match (&exit[b][t], &current[t]) {
                        (
                            Range::Interval {
                                lo: old_lo,
                                hi: old_hi,
                            },
                            Range::Interval {
                                lo: new_lo,
                                hi: new_hi,
                            },
                        ) => new_lo < old_lo || new_hi > old_hi,
                        (Range::Bottom, Range::Interval { .. }) => true,
                        (Range::Top, _) => false,
                        (_, Range::Top) => true,
                        _ => false,
                    };
                    if expands {
                        expansions[b][t] = expansions[b][t].saturating_add(1);
                    }
                    if expansions[b][t] > 2 {
                        current[t] = f
                            .temp_types
                            .get(t)
                            .map(unknown_for_type)
                            .unwrap_or(Range::Top);
                        widened.insert(b);
                    }
                }
            }
            let changed_entry = tracked_temps
                .iter()
                .any(|&temp| entry[b][temp] != incoming[temp]);
            let changed_exit = tracked_temps
                .iter()
                .any(|&temp| exit[b][temp] != current[temp]);
            if changed_entry || changed_exit {
                for &temp in tracked_temps {
                    entry[b][temp] = incoming[temp].clone();
                    exit[b][temp] = current[temp].clone();
                }
                changed = true;
            }
        }
        if !changed {
            break;
        }
        if iteration + 1 == MAX_ITERATIONS {
            for b in 0..cfg.blocks.len() {
                for &temp in tracked_temps {
                    entry[b][temp] = Range::Top;
                    exit[b][temp] = Range::Top;
                }
                widened.insert(b);
            }
        }
    }

    Ok(BlockAnalysis {
        cfg,
        entry,
        exit,
        widened,
        inst_defs,
    })
}

fn analyze_blocks(f: &MwirFn) -> Result<BlockAnalysis, String> {
    analyze_blocks_tracked(f, &(0..f.temp_types.len()).collect::<Vec<_>>())
}

/// Temps whose values can affect an indexed operand or a branch refinement of
/// one of its dependencies. Bounds proving observes no other lattice entries,
/// so scanning them at every CFG edge was pure dense-state work.
fn bounds_relevant_temps(f: &MwirFn) -> Vec<usize> {
    let n = f.temp_types.len();
    let mut dependencies = vec![Vec::<Temp>::new(); n];
    let mut comparison_peers = vec![Vec::<Temp>::new(); n];
    let mut seeds = Vec::new();
    for inst in &f.body {
        match inst {
            Inst::Copy { dst, src } => dependencies[dst.0].push(*src),
            Inst::ArithChecked { dst, lhs, rhs, .. }
            | Inst::ArithWrapping { dst, lhs, rhs, .. } => {
                dependencies[dst.0].extend([*lhs, *rhs]);
            }
            Inst::Convert { dst, src, .. } => dependencies[dst.0].push(*src),
            Inst::Compare { lhs, rhs, .. } => {
                comparison_peers[lhs.0].push(*rhs);
                comparison_peers[rhs.0].push(*lhs);
            }
            Inst::IndexGet { index, .. }
            | Inst::IndexSet { index, .. }
            | Inst::PlacedIndexGet { index, .. }
            | Inst::PlacedIndexSet { index, .. } => seeds.push(*index),
            _ => {}
        }
    }
    let mut relevant = vec![false; n];
    let mut pending = seeds;
    while let Some(temp) = pending.pop() {
        if temp.0 >= n || std::mem::replace(&mut relevant[temp.0], true) {
            continue;
        }
        pending.extend(dependencies[temp.0].iter().copied());
        pending.extend(comparison_peers[temp.0].iter().copied());
    }
    relevant
        .into_iter()
        .enumerate()
        .filter_map(|(temp, relevant)| relevant.then_some(temp))
        .collect()
}

pub fn analyze(f: &MwirFn) -> Result<Analysis, String> {
    let BlockAnalysis {
        cfg,
        entry,
        exit,
        widened,
        inst_defs,
    } = analyze_blocks(f)?;
    let n = f.temp_types.len();
    let mut instructions = vec![
        InstructionRanges {
            before: vec![Range::Top; n],
            after: vec![Range::Top; n],
        };
        f.body.len()
    ];
    for block in &cfg.blocks {
        let mut current = entry[block.id].clone();
        for i in block.range.clone() {
            instructions[i].before = current.clone();
            transfer_into(&f.body[i], &inst_defs[i], &mut current, &f.temp_types, None);
            instructions[i].after = current.clone();
        }
    }
    Ok(Analysis {
        entry,
        exit,
        instructions,
        widened,
    })
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProofResult {
    Proven { len: usize },
    Unknown { reason: &'static str },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BoundsProof {
    pub at: usize,
    pub base: Temp,
    pub index: Temp,
    pub len: usize,
    pub result: ProofResult,
}

fn proof_for(r: &Range, len: usize, unsigned: bool) -> ProofResult {
    if len == 0 {
        return ProofResult::Unknown {
            reason: "zero-length",
        };
    }
    let lo = if unsigned { 0 } else { 0 };
    if r.proves(lo, len as i128 - 1) {
        ProofResult::Proven { len }
    } else if !matches!(r, Range::Interval { .. }) {
        ProofResult::Unknown {
            reason: "unknown-range",
        }
    } else {
        ProofResult::Unknown {
            reason: "range-not-contained",
        }
    }
}

fn is_unsigned(ty: &Type) -> bool {
    matches!(
        ty,
        Type::U8 | Type::U16 | Type::U32 | Type::U64 | Type::Usize
    )
}

pub fn bounds_proofs(f: &MwirFn, analysis: &Analysis) -> Vec<BoundsProof> {
    let mut out = Vec::new();
    for (at, inst) in f.body.iter().enumerate() {
        let (base, index, len) = match inst {
            Inst::IndexGet {
                base, index, len, ..
            }
            | Inst::IndexSet {
                base, index, len, ..
            } => (*base, *index, *len),
            Inst::PlacedIndexGet {
                base, index, len, ..
            }
            | Inst::PlacedIndexSet {
                base, index, len, ..
            } => (*base, *index, *len),
            _ => continue,
        };
        let ty = f.temp_types.get(index.0).unwrap_or(&Type::Usize);
        let unsigned = is_unsigned(ty);
        let r = analysis
            .instructions
            .get(at)
            .and_then(|i| i.before.get(index.0))
            .unwrap_or(&Range::Top);
        out.push(BoundsProof {
            at,
            base,
            index,
            len,
            result: proof_for(r, len, unsigned),
        });
    }
    out
}

fn bounds_proofs_sparse(f: &MwirFn) -> Result<Vec<BoundsProof>, String> {
    let tracked_temps = bounds_relevant_temps(f);
    let mut tracked = vec![false; f.temp_types.len()];
    for &temp in &tracked_temps {
        tracked[temp] = true;
    }
    let BlockAnalysis {
        cfg,
        entry,
        inst_defs,
        ..
    } = analyze_blocks_tracked(f, &tracked_temps)?;
    let mut out = Vec::new();
    for block in &cfg.blocks {
        let mut current = entry[block.id].clone();
        for at in block.range.clone() {
            let inst = &f.body[at];
            let selected = match inst {
                Inst::IndexGet {
                    base, index, len, ..
                }
                | Inst::IndexSet {
                    base, index, len, ..
                }
                | Inst::PlacedIndexGet {
                    base, index, len, ..
                }
                | Inst::PlacedIndexSet {
                    base, index, len, ..
                } => Some((*base, *index, *len)),
                _ => None,
            };
            if let Some((base, index, len)) = selected {
                let ty = f.temp_types.get(index.0).unwrap_or(&Type::Usize);
                let range = current.get(index.0).unwrap_or(&Range::Top);
                out.push(BoundsProof {
                    at,
                    base,
                    index,
                    len,
                    result: proof_for(range, len, is_unsigned(ty)),
                });
            }
            transfer_into(
                inst,
                &inst_defs[at],
                &mut current,
                &f.temp_types,
                Some(&tracked),
            );
        }
    }
    Ok(out)
}

/// Attach proof-carrying indexed variants to a cloned MWIR function.  The
/// ordinary variants are never changed in place, so a later rewrite cannot
/// accidentally retain a proof for different operands.
pub fn apply_proofs(f: &MwirFn, analysis: &Analysis) -> Result<MwirFn, String> {
    apply_proofs_from(f, bounds_proofs(f, analysis))
}

fn apply_proofs_sparse(f: &MwirFn) -> Result<MwirFn, String> {
    apply_proofs_from(f, bounds_proofs_sparse(f)?)
}

fn apply_proofs_from(f: &MwirFn, proofs: Vec<BoundsProof>) -> Result<MwirFn, String> {
    let mut by_at = BTreeMap::new();
    for proof in proofs {
        if let ProofResult::Proven { len } = proof.result {
            by_at.insert(proof.at, (proof.base, proof.index, len));
        }
    }
    let mut out = f.clone();
    for (at, inst) in out.body.iter_mut().enumerate() {
        let Some(&(base, index, len)) = by_at.get(&at) else {
            continue;
        };
        let replacement = match inst {
            Inst::IndexGet {
                dst,
                base: b,
                index: i,
                len: n,
            } if *b == base && *i == index && *n == len => Some(Inst::IndexGetProven {
                dst: *dst,
                base,
                index,
                len,
            }),
            Inst::IndexSet {
                base: b,
                index: i,
                value,
                len: n,
            } if *b == base && *i == index && *n == len => Some(Inst::IndexSetProven {
                base,
                index,
                value: *value,
                len,
            }),
            Inst::PlacedIndexGet {
                dst,
                base: b,
                field_offset,
                index: i,
                len: n,
                elem_stride,
                ty,
            } if *b == base && *i == index && *n == len => Some(Inst::PlacedIndexGetProven {
                dst: *dst,
                base,
                field_offset: *field_offset,
                index,
                len,
                elem_stride: *elem_stride,
                ty: ty.clone(),
            }),
            Inst::PlacedIndexSet {
                base: b,
                field_offset,
                index: i,
                value,
                len: n,
                elem_stride,
                ty,
            } if *b == base && *i == index && *n == len => Some(Inst::PlacedIndexSetProven {
                base,
                field_offset: *field_offset,
                index,
                value: *value,
                len,
                elem_stride: *elem_stride,
                ty: ty.clone(),
            }),
            _ => None,
        };
        if let Some(replacement) = replacement {
            *inst = replacement;
        } else {
            return Err(format!(
                "bounds proof at instruction {at} no longer matches its operands"
            ));
        }
    }
    Ok(out)
}

fn clear_proofs(f: &MwirFn) -> MwirFn {
    let mut out = f.clone();
    for inst in &mut out.body {
        let original = inst.clone();
        *inst = match &original {
            Inst::IndexGetProven {
                dst,
                base,
                index,
                len,
            } => Inst::IndexGet {
                dst: *dst,
                base: *base,
                index: *index,
                len: *len,
            },
            Inst::IndexSetProven {
                base,
                index,
                value,
                len,
            } => Inst::IndexSet {
                base: *base,
                index: *index,
                value: *value,
                len: *len,
            },
            Inst::PlacedIndexGetProven {
                dst,
                base,
                field_offset,
                index,
                len,
                elem_stride,
                ty,
            } => Inst::PlacedIndexGet {
                dst: *dst,
                base: *base,
                field_offset: *field_offset,
                index: *index,
                len: *len,
                elem_stride: *elem_stride,
                ty: ty.clone(),
            },
            Inst::PlacedIndexSetProven {
                base,
                field_offset,
                index,
                value,
                len,
                elem_stride,
                ty,
            } => Inst::PlacedIndexSet {
                base: *base,
                field_offset: *field_offset,
                index: *index,
                value: *value,
                len: *len,
                elem_stride: *elem_stride,
                ty: ty.clone(),
            },
            other => other.clone(),
        };
    }
    out
}

/// Re-run the proof after all MWIR rewrites.  A proven variant is accepted by
/// codegen only if the current operands and current control-flow facts prove
/// its recorded length again.
pub fn validate_proven_sites(f: &MwirFn) -> Result<(), String> {
    let required = clear_proofs(f);
    if required == *f {
        return Ok(());
    }
    let proofs: BTreeMap<usize, BoundsProof> = bounds_proofs_sparse(&required)?
        .into_iter()
        .map(|proof| (proof.at, proof))
        .collect();
    for (at, inst) in f.body.iter().enumerate() {
        if !matches!(
            inst,
            Inst::IndexGetProven { .. }
                | Inst::IndexSetProven { .. }
                | Inst::PlacedIndexGetProven { .. }
                | Inst::PlacedIndexSetProven { .. }
        ) {
            continue;
        }
        let proof = proofs
            .get(&at)
            .ok_or_else(|| format!("proven index at instruction {at} has no proof site"))?;
        if !matches!(proof.result, ProofResult::Proven { len } if len == proof.len) {
            return Err(format!(
                "proven index at instruction {at} no longer proves length {}",
                proof.len
            ));
        }
    }
    Ok(())
}

fn has_index_sites(f: &MwirFn) -> bool {
    f.body.iter().any(|inst| {
        matches!(
            inst,
            Inst::IndexGet { .. }
                | Inst::IndexSet { .. }
                | Inst::PlacedIndexGet { .. }
                | Inst::PlacedIndexSet { .. }
        )
    })
}

pub fn apply_program_proofs(
    program: &crate::mwir::MwirProgram,
) -> Result<crate::mwir::MwirProgram, String> {
    Ok(apply_program_proofs_certified(program)?.into_program())
}

/// An MWIR program whose proven index sites were produced by the range
/// analysis and have not passed through another rewrite.
///
/// The inner program is deliberately private. Codegen may use this witness to
/// avoid immediately repeating the same fixed-point analysis, while callers
/// that can mutate MWIR receive an ordinary `MwirProgram` and must validate it
/// again.
pub(crate) struct CertifiedProgram(crate::mwir::MwirProgram);

impl CertifiedProgram {
    pub(crate) fn as_program(&self) -> &crate::mwir::MwirProgram {
        &self.0
    }

    pub(crate) fn into_program(self) -> crate::mwir::MwirProgram {
        self.0
    }
}

pub(crate) fn apply_program_proofs_certified(
    program: &crate::mwir::MwirProgram,
) -> Result<CertifiedProgram, String> {
    apply_program_proofs_owned_certified(program.clone())
}

pub(crate) fn apply_program_proofs_owned_certified(
    mut program: crate::mwir::MwirProgram,
) -> Result<CertifiedProgram, String> {
    let keys = program.fns.keys().cloned().collect::<Vec<_>>();
    for key in keys {
        let f = &program.fns[&key];
        // Range analysis is intentionally demand-driven.  Besides avoiding
        // needless work on runtime helpers, this keeps a parked proof option
        // from turning every bounds-free function into a fixed-point problem.
        if !has_index_sites(f) {
            continue;
        }
        let rewritten = apply_proofs_sparse(f)?;
        program.fns.insert(key, rewritten);
    }
    Ok(CertifiedProgram(program))
}

/// Apply the same proof-carrying rewrite to FlowWir states whose local body
/// is entirely MWIR.  Mixed states are left unchanged: omitting a non-MWIR
/// definition from the local lattice would be less safe than declining the
/// proof, and cross-state facts are intentionally not guessed here.
pub fn apply_flow_program_proofs(program: &FlowWirProgram) -> Result<FlowWirProgram, String> {
    let mut out = program.clone();
    for (key, function) in &program.fns {
        let mut rewritten = function.clone();
        for (state_id, state) in function.states.iter().enumerate() {
            if state.ops.iter().any(|op| !matches!(op, FlowInst::Mwir(_))) {
                continue;
            }
            let body: Vec<Inst> = state
                .ops
                .iter()
                .map(|op| match op {
                    FlowInst::Mwir(inst) => inst.clone(),
                    _ => unreachable!("mixed FlowWir state was filtered above"),
                })
                .collect();
            let local = MwirFn {
                receiver: function.receiver,
                params: function.params.clone(),
                ret: function.ret.clone(),
                temp_types: function.frame.temp_types.clone(),
                body,
            };
            let local = apply_proofs_sparse(&local)
                .map_err(|e| format!("FlowWir function `{key}` state {state_id}: {e}"))?;
            rewritten.states[state_id].ops = local.body.into_iter().map(FlowInst::Mwir).collect();
        }
        out.fns.insert(key.clone(), rewritten);
    }
    Ok(out)
}

pub fn validate_flow_proven_sites(f: &crate::flowwir::FlowWirFn) -> Result<(), String> {
    for (state_id, state) in f.states.iter().enumerate() {
        let has_proof = state.ops.iter().any(|op| {
            matches!(
                op,
                FlowInst::Mwir(
                    Inst::IndexGetProven { .. }
                        | Inst::IndexSetProven { .. }
                        | Inst::PlacedIndexGetProven { .. }
                        | Inst::PlacedIndexSetProven { .. }
                )
            )
        });
        if !has_proof {
            continue;
        }
        if state.ops.iter().any(|op| !matches!(op, FlowInst::Mwir(_))) {
            return Err(format!(
                "Flow state s{state_id} mixes a proven index with operations outside the local range lattice"
            ));
        }
        let local = MwirFn {
            receiver: f.receiver,
            params: f.params.clone(),
            ret: f.ret.clone(),
            temp_types: f.frame.temp_types.clone(),
            body: state
                .ops
                .iter()
                .map(|op| match op {
                    FlowInst::Mwir(inst) => inst.clone(),
                    _ => unreachable!(),
                })
                .collect(),
        };
        validate_proven_sites(&local)
            .map_err(|error| format!("Flow state s{state_id}: {error}"))?;
    }
    Ok(())
}

pub fn dump(f: &MwirFn, analysis: &Analysis) -> String {
    let mut out = String::new();
    for (at, ranges) in analysis.instructions.iter().enumerate() {
        let _ = writeln!(
            out,
            "at={at} ranges=[{}]",
            ranges
                .before
                .iter()
                .enumerate()
                .map(|(i, r)| format!("t{i}={}", r.render()))
                .collect::<Vec<_>>()
                .join(",")
        );
    }
    for proof in bounds_proofs(f, analysis) {
        match proof.result {
            ProofResult::Proven { len } => {
                let _ = writeln!(
                    out,
                    "bounds at={} base={} index={} len={} result=proven",
                    proof.at, proof.base, proof.index, len
                );
            }
            ProofResult::Unknown { reason } => {
                let _ = writeln!(
                    out,
                    "bounds at={} base={} index={} len={} result=unknown reason={reason}",
                    proof.at, proof.base, proof.index, proof.len
                );
            }
        }
    }
    out
}

pub fn dump_flow_program(program: &FlowWirProgram) -> Result<String, String> {
    let mut out = String::new();
    for (key, function) in &program.fns {
        for (state_id, state) in function.states.iter().enumerate() {
            if state.ops.iter().any(|op| !matches!(op, FlowInst::Mwir(_))) {
                let _ = writeln!(
                    out,
                    "  range flow function {key} state s{state_id} result=unknown reason=mixed-state"
                );
                continue;
            }
            let local = MwirFn {
                receiver: function.receiver,
                params: function.params.clone(),
                ret: function.ret.clone(),
                temp_types: function.frame.temp_types.clone(),
                body: state
                    .ops
                    .iter()
                    .map(|op| match op {
                        FlowInst::Mwir(inst) => inst.clone(),
                        _ => unreachable!("mixed state was filtered"),
                    })
                    .collect(),
            };
            let analysis = analyze(&local)
                .map_err(|error| format!("FlowWir function `{key}` state {state_id}: {error}"))?;
            let _ = writeln!(out, "  range flow function {key} state s{state_id}");
            out.push_str(&dump(&local, &analysis));
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mwir::Inst;

    fn fn_with_index(ty: Type, value: i128) -> MwirFn {
        MwirFn {
            receiver: None,
            params: Vec::new(),
            ret: Type::U64,
            temp_types: vec![
                Type::Array(
                    Box::new(Type::U64),
                    Box::new(crate::syntax::ast::Expr::Int(
                        crate::syntax::ast::Span::default(),
                        "16".into(),
                    )),
                ),
                ty,
                Type::U64,
            ],
            body: vec![
                Inst::ConstInt {
                    dst: Temp(1),
                    ty: Type::Usize,
                    value,
                },
                Inst::IndexGet {
                    dst: Temp(2),
                    base: Temp(0),
                    index: Temp(1),
                    len: 16,
                },
                Inst::Return {
                    value: Some(Temp(2)),
                },
            ],
        }
    }

    #[test]
    fn unsigned_constant_proves_an_index() {
        let f = fn_with_index(Type::Usize, 15);
        let a = analyze(&f).expect("range");
        assert!(matches!(
            bounds_proofs(&f, &a)[0].result,
            ProofResult::Proven { .. }
        ));
    }

    #[test]
    fn signed_negative_is_not_proven() {
        let f = fn_with_index(Type::I64, -1);
        let a = analyze(&f).expect("range");
        assert!(!matches!(
            bounds_proofs(&f, &a)[0].result,
            ProofResult::Proven { .. }
        ));
    }

    fn branched_index(index_ty: Type) -> MwirFn {
        MwirFn {
            receiver: None,
            params: vec![(Temp(1), crate::syntax::ast::AccessMode::Read)],
            ret: Type::U64,
            temp_types: vec![
                Type::Array(
                    Box::new(Type::U64),
                    Box::new(crate::syntax::ast::Expr::Int(
                        crate::syntax::ast::Span::default(),
                        "16".into(),
                    )),
                ),
                index_ty.clone(),
                Type::Bool,
                Type::U64,
                Type::Usize,
                Type::U64,
            ],
            body: vec![
                Inst::ConstInt {
                    dst: Temp(4),
                    ty: Type::Usize,
                    value: 16,
                },
                Inst::Compare {
                    dst: Temp(2),
                    op: BinOp::Lt,
                    ty: index_ty,
                    lhs: Temp(1),
                    rhs: Temp(4),
                },
                Inst::JumpIfFalse {
                    cond: Temp(2),
                    target: 5,
                },
                Inst::IndexGet {
                    dst: Temp(3),
                    base: Temp(0),
                    index: Temp(1),
                    len: 16,
                },
                Inst::Return {
                    value: Some(Temp(3)),
                },
                Inst::ConstInt {
                    dst: Temp(5),
                    ty: Type::U64,
                    value: 0,
                },
                Inst::Return {
                    value: Some(Temp(5)),
                },
            ],
        }
    }

    #[test]
    fn join_uses_the_convex_hull_and_does_not_invent_a_bounds_proof() {
        let mut f = fn_with_index(Type::U64, 0);
        f.temp_types.push(Type::Bool);
        let cond = Temp(4);
        f.body = vec![
            Inst::ConstBool {
                dst: cond,
                value: true,
            },
            Inst::JumpIfFalse { cond, target: 4 },
            Inst::ConstInt {
                dst: Temp(0),
                ty: Type::U64,
                value: 0,
            },
            Inst::Jump { target: 5 },
            Inst::ConstInt {
                dst: Temp(0),
                ty: Type::U64,
                value: 100,
            },
            Inst::IndexGet {
                dst: Temp(2),
                base: Temp(1),
                index: Temp(0),
                len: 16,
            },
            Inst::Return {
                value: Some(Temp(2)),
            },
        ];
        let analysis = analyze(&f).expect("analysis");
        assert_eq!(
            analysis.instructions[5].before[0],
            Range::Interval { lo: 0, hi: 100 }
        );
        assert!(matches!(
            bounds_proofs(&f, &analysis)[0].result,
            ProofResult::Unknown { .. }
        ));
    }

    #[test]
    fn expanding_loop_widens_and_wrapping_overflow_becomes_top() {
        let f = MwirFn {
            receiver: None,
            params: Vec::new(),
            ret: Type::U64,
            temp_types: vec![Type::U64, Type::U64],
            body: vec![
                Inst::ConstInt {
                    dst: Temp(0),
                    ty: Type::U64,
                    value: 0,
                },
                Inst::ConstInt {
                    dst: Temp(1),
                    ty: Type::U64,
                    value: 1,
                },
                Inst::ArithWrapping {
                    dst: Temp(0),
                    op: BinOp::AddW,
                    ty: Type::U64,
                    lhs: Temp(0),
                    rhs: Temp(1),
                },
                Inst::Jump { target: 2 },
                Inst::Return {
                    value: Some(Temp(0)),
                },
            ],
        };
        let analysis = analyze(&f).expect("analysis");
        assert!(!analysis.widened.is_empty());
        // An unbounded wrapping accumulator widens to its declared envelope.
        // It used to read back as `Top` only because the widened block relatched
        // `changed` forever and the exhausted sweep discarded the function; the
        // guarantee under test is that no useful bound survives, which the
        // envelope satisfies for every real array length.
        let widened = &analysis.instructions[2].before[0];
        assert_eq!(*widened, Range::interval(0, i128::from(u64::MAX)));
        for len in [1_usize, 16, 4096] {
            assert!(
                matches!(proof_for(widened, len, true), ProofResult::Unknown { .. }),
                "a widened accumulator must not prove an index below {len}"
            );
        }
    }

    #[test]
    fn a_widened_loop_converges_without_erasing_unrelated_proofs() {
        // Regression lock for the non-convergent fixpoint: a widened block used
        // to relatch `changed` on every sweep, so the analysis ran all
        // `MAX_ITERATIONS` times and then filled *every* block with `Top`. That
        // silently cost each affected function all of its bounds proofs, so the
        // property to hold is that widening one loop does not erase a proof an
        // unrelated refined index still supports.
        let mut f = branched_index(Type::Usize);
        let counter = Temp(f.temp_types.len());
        let step = Temp(f.temp_types.len() + 1);
        f.temp_types.push(Type::U64);
        f.temp_types.push(Type::U64);
        let resume = f.body.len();
        f.body.extend([
            Inst::ConstInt {
                dst: counter,
                ty: Type::U64,
                value: 0,
            },
            Inst::ConstInt {
                dst: step,
                ty: Type::U64,
                value: 1,
            },
            Inst::ArithWrapping {
                dst: counter,
                op: BinOp::AddW,
                ty: Type::U64,
                lhs: counter,
                rhs: step,
            },
            Inst::Jump { target: resume + 2 },
        ]);
        let analysis = analyze(&f).expect("range");
        assert!(
            !analysis.widened.is_empty(),
            "the wrapping accumulator must still widen"
        );
        assert!(
            analysis
                .exit
                .iter()
                .any(|block| block.iter().any(|range| *range != Range::Top)),
            "a converged sweep must not discard the whole function as Top"
        );
        assert!(
            matches!(
                bounds_proofs(&f, &analysis)[0].result,
                ProofResult::Proven { len: 16 }
            ),
            "widening one loop must not erase an unrelated refined index proof"
        );
    }

    #[test]
    fn unsigned_upper_branch_refines_the_true_edge() {
        let f = branched_index(Type::Usize);
        let a = analyze(&f).expect("range");
        let proofs = bounds_proofs(&f, &a);
        assert!(matches!(proofs[0].result, ProofResult::Proven { len: 16 }));
        let rewritten = apply_proofs(&f, &a).expect("proof-carrying rewrite");
        assert!(matches!(rewritten.body[3], Inst::IndexGetProven { .. }));
    }

    #[test]
    fn sparse_proof_application_matches_full_instruction_history() {
        for ty in [Type::Usize, Type::I64] {
            let mut f = branched_index(ty);
            let copied_index = Temp(f.temp_types.len());
            let irrelevant = Temp(f.temp_types.len() + 1);
            f.temp_types.extend([Type::Usize, Type::U64]);
            f.body.splice(
                3..3,
                [
                    Inst::ConstInt {
                        dst: irrelevant,
                        ty: Type::U64,
                        value: 99,
                    },
                    Inst::Copy {
                        dst: copied_index,
                        src: Temp(1),
                    },
                ],
            );
            if let Inst::JumpIfFalse { target, .. } = &mut f.body[2] {
                *target += 2;
            }
            if let Inst::IndexGet { index, .. } = &mut f.body[5] {
                *index = copied_index;
            } else {
                panic!("fixture lost its indexed access");
            }
            let relevant = bounds_relevant_temps(&f);
            assert!(relevant.contains(&copied_index.0));
            assert!(relevant.contains(&1));
            assert!(relevant.contains(&4), "comparison peer must be tracked");
            assert!(!relevant.contains(&irrelevant.0));
            let full = apply_proofs(&f, &analyze(&f).expect("full range analysis"))
                .expect("full proof rewrite");
            let sparse = apply_proofs_sparse(&f).expect("sparse proof rewrite");
            assert_eq!(sparse, full);
        }
    }

    #[test]
    fn signed_upper_branch_does_not_prove_nonnegative() {
        let f = branched_index(Type::I64);
        let a = analyze(&f).expect("range");
        assert!(!matches!(
            bounds_proofs(&f, &a)[0].result,
            ProofResult::Proven { .. }
        ));
    }

    #[test]
    fn redefining_a_comparison_operand_invalidates_edge_refinement() {
        let mut f = branched_index(Type::Usize);
        // Compare i < 16, then replace the bound with 1 before branching.
        // Using the redefined value as though it had participated in the
        // comparison could incorrectly prove an index into a one-element
        // array.
        f.body.insert(
            2,
            Inst::ConstInt {
                dst: Temp(4),
                ty: Type::Usize,
                value: 1,
            },
        );
        if let Inst::JumpIfFalse { target, .. } = &mut f.body[3] {
            *target += 1;
        }
        if let Inst::IndexGet { len, .. } = &mut f.body[4] {
            *len = 1;
        }
        let a = analyze(&f).expect("range");
        assert!(!matches!(
            bounds_proofs(&f, &a)[0].result,
            ProofResult::Proven { .. }
        ));
    }

    #[test]
    fn arithmetic_crossing_the_declared_type_bound_becomes_top() {
        let r = add_const(Range::interval(250, 255), BinOp::AddW, 1, &Type::U8);
        assert_eq!(r, Range::Top);
        let r = add_const(
            Range::interval(i128::MAX, i128::MAX),
            BinOp::Add,
            1,
            &Type::I64,
        );
        assert_eq!(r, Range::Top);
    }

    #[test]
    fn flow_state_proofs_use_the_same_proven_variant() {
        let f = branched_index(Type::Usize);
        let flow = FlowWirProgram {
            fns: BTreeMap::from([(
                "flow".into(),
                crate::flowwir::FlowWirFn {
                    receiver: f.receiver,
                    params: f.params.clone(),
                    ret: f.ret.clone(),
                    frame: crate::flowwir::FrameLayout {
                        temp_types: f.temp_types.clone(),
                        lineage_group_slot: Temp(0),
                        lineage_deadline_slot: Temp(0),
                    },
                    states: vec![crate::flowwir::State {
                        ops: f.body.iter().cloned().map(FlowInst::Mwir).collect(),
                        transition: crate::flowwir::Transition::Return(None),
                    }],
                },
            )]),
        };
        let rewritten = apply_flow_program_proofs(&flow).expect("flow proofs");
        assert!(matches!(
            rewritten.fns["flow"].states[0].ops[3],
            FlowInst::Mwir(Inst::IndexGetProven { .. })
        ));
    }
}
