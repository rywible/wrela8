use std::cell::Cell;
use std::collections::{BTreeMap, BTreeSet};

use crate::eval::value::{self, Value};
use crate::flowwir::{AwaitKind, FlowInst, FlowWirProgram, Transition};
use crate::mwir::{Inst, LayoutCtx, MwirFn, MwirProgram, Temp};
use crate::sema::types::Type;
use crate::syntax::ast::{AccessMode, BinOp};

thread_local! {
    static INLINE: Cell<bool> = const { Cell::new(false) };
    static INLINE_AFTER_REDUNDANCY: Cell<bool> = const { Cell::new(false) };
    static CONST_PROP: Cell<bool> = const { Cell::new(false) };
    static GVN: Cell<bool> = const { Cell::new(false) };
    static DCE: Cell<bool> = const { Cell::new(false) };
    static SROA: Cell<bool> = const { Cell::new(false) };
    static INLINE_RULE_ONE_ONLY: Cell<bool> = const { Cell::new(false) };
    static INLINED_SITES: Cell<usize> = const { Cell::new(0) };
    static INLINED_CALLEES_DELETED: Cell<usize> = const { Cell::new(0) };
}

pub fn take_inline_reach() -> (usize, usize) {
    (
        INLINED_SITES.with(|c| c.replace(0)),
        INLINED_CALLEES_DELETED.with(|c| c.replace(0)),
    )
}

pub fn set_inline(enabled: bool) {
    INLINE.with(|c| c.set(enabled));
}
pub fn inlining() -> bool {
    INLINE.with(|c| c.get())
}

pub fn set_inline_after_redundancy(enabled: bool) {
    INLINE_AFTER_REDUNDANCY.with(|c| c.set(enabled));
}
pub fn inline_after_redundancy() -> bool {
    INLINE_AFTER_REDUNDANCY.with(|c| c.get())
}

pub fn set_inline_rule_one_only(enabled: bool) {
    INLINE_RULE_ONE_ONLY.with(|c| c.set(enabled));
}
pub fn inline_rule_one_only() -> bool {
    INLINE_RULE_ONE_ONLY.with(|c| c.get())
}

pub fn set_const_prop(enabled: bool) {
    CONST_PROP.with(|c| c.set(enabled));
}
pub fn const_prop() -> bool {
    CONST_PROP.with(|c| c.get())
}

pub fn set_gvn(enabled: bool) {
    GVN.with(|c| c.set(enabled));
}
pub fn gvn() -> bool {
    GVN.with(|c| c.get())
}

pub fn set_dce(enabled: bool) {
    DCE.with(|c| c.set(enabled));
}
pub fn dce() -> bool {
    DCE.with(|c| c.get())
}

pub fn set_sroa(enabled: bool) {
    SROA.with(|c| c.set(enabled));
}

pub fn sroa() -> bool {
    SROA.with(|c| c.get())
}

pub fn optimize_checked(
    mwir: &MwirProgram,
    flow: Option<&FlowWirProgram>,
    layout: &LayoutCtx,
) -> Result<Option<MwirProgram>, String> {
    Ok(optimize_for_codegen_checked(mwir, flow, layout)?.map(CodegenOptimized::into_program))
}

pub(crate) enum CodegenOptimized {
    Ordinary(MwirProgram),
    ProofsCurrent(crate::range::CertifiedProgram),
}

impl CodegenOptimized {
    pub(crate) fn as_program(&self) -> &MwirProgram {
        match self {
            Self::Ordinary(program) => program,
            Self::ProofsCurrent(program) => program.as_program(),
        }
    }

    pub(crate) fn proofs_current(&self) -> bool {
        matches!(self, Self::ProofsCurrent(_))
    }

    fn into_program(self) -> MwirProgram {
        match self {
            Self::Ordinary(program) => program,
            Self::ProofsCurrent(program) => program.into_program(),
        }
    }
}

pub(crate) fn optimize_for_codegen_checked(
    mwir: &MwirProgram,
    flow: Option<&FlowWirProgram>,
    layout: &LayoutCtx,
) -> Result<Option<CodegenOptimized>, String> {
    if !(inlining() || const_prop() || gvn() || dce() || sroa() || crate::lower::bounds_elide()) {
        return Ok(None);
    }
    if !runtime_closure_is_known() {
        return Ok(None);
    }
    let mut prog = mwir.clone();
    if sroa() {
        prog = crate::sroa::rewrite_program(&prog, layout)?.0;
    }
    if inlining() && !inline_after_redundancy() {
        inline_program(&mut prog, flow, layout);
    }
    for (key, f) in prog.fns.iter_mut() {
        if is_late_bound(key) {
            continue;
        }
        if const_prop() {
            const_prop_fn(f);
        }
        if gvn() {
            gvn_fn(f);
        }
        if dce() {
            dce_fn(f);
        }
    }
    if inlining() && inline_after_redundancy() {
        inline_program(&mut prog, flow, layout);
    }
    if crate::lower::bounds_elide() {
        return Ok(Some(CodegenOptimized::ProofsCurrent(
            crate::range::apply_program_proofs_owned_certified(prog)?,
        )));
    }
    Ok(Some(CodegenOptimized::Ordinary(prog)))
}

/// Compatibility wrapper for analysis tests.  Production codegen uses
/// `optimize_checked` so a malformed SROA rewrite or bounds proof fails closed
/// rather than silently compiling the unoptimized program.
pub fn optimize(
    mwir: &MwirProgram,
    flow: Option<&FlowWirProgram>,
    layout: &LayoutCtx,
) -> Option<MwirProgram> {
    optimize_checked(mwir, flow, layout).ok().flatten()
}

/// Explicit maintainer-only dataflow transform.  The normal optimizer keeps
/// this off until its linked cost oracle proves a product win.
pub fn optimize_with_proofs(
    program: &MwirProgram,
    layout: &crate::mwir::LayoutCtx,
    scalar_replace: bool,
    bounds_prove: bool,
) -> Result<MwirProgram, String> {
    let mut out = program.clone();
    if scalar_replace {
        out = crate::sroa::rewrite_program(&out, layout)?.0;
    }
    if bounds_prove {
        out = crate::range::apply_program_proofs(&out)?;
    }
    Ok(out)
}

pub fn visit_temps_mut(inst: &mut Inst, f: &mut impl FnMut(&mut Temp)) {
    match inst {
        Inst::ConstInt { dst, .. }
        | Inst::ConstBool { dst, .. }
        | Inst::ConstFloat { dst, .. }
        | Inst::ConstChar { dst, .. }
        | Inst::ConstUnit { dst }
        | Inst::ConstText { dst, .. }
        | Inst::LoadIrqVector { dst, .. }
        | Inst::Now { dst }
        | Inst::Entropy { dst, .. }
        | Inst::InterruptCellLoadAcquire { dst, .. } => f(dst),
        Inst::Copy { dst, src }
        | Inst::EnumTag { dst, src }
        | Inst::Not { dst, src }
        | Inst::EnumPayload { dst, src, .. }
        | Inst::FormatScalar { dst, src, .. }
        | Inst::Neg { dst, src, .. }
        | Inst::BitNot { dst, src, .. }
        | Inst::Convert { dst, src, .. } => {
            f(dst);
            f(src);
        }
        Inst::MakeAggregate { dst, elems } => {
            f(dst);
            for e in elems {
                f(e);
            }
        }
        Inst::I32x4FromLanes { dst, lanes } | Inst::PacketFromLanes { dst, lanes, .. } => {
            f(dst);
            f(lanes);
        }
        Inst::PacketSplat { dst, scalar, .. } => {
            f(dst);
            f(scalar);
        }
        Inst::PacketShiftRightArithmetic { dst, src, .. }
        | Inst::PacketConvert { dst, src, .. } => {
            f(dst);
            f(src);
        }
        Inst::PacketSelect {
            dst,
            lhs,
            rhs,
            if_true,
            if_false,
            ..
        } => {
            f(dst);
            f(lhs);
            f(rhs);
            f(if_true);
            f(if_false);
        }
        Inst::PacketFma {
            dst,
            lhs,
            rhs,
            addend,
        } => {
            f(dst);
            f(lhs);
            f(rhs);
            f(addend);
        }
        // Deliberately visits nothing: a region marker names no temp, so
        // every temp-renaming pass leaves it alone by construction.
        Inst::RegionMarker { .. } => {}
        Inst::MakeEnum { dst, payload, .. } => {
            f(dst);
            for p in payload {
                f(p);
            }
        }
        Inst::StringConcat { dst, lhs, rhs, .. }
        | Inst::I32x4Add { dst, lhs, rhs }
        | Inst::PacketBinary { dst, lhs, rhs, .. }
        | Inst::ArithChecked { dst, lhs, rhs, .. }
        | Inst::ArithWrapping { dst, lhs, rhs, .. }
        | Inst::DivRem { dst, lhs, rhs, .. }
        | Inst::Shift { dst, lhs, rhs, .. }
        | Inst::Bitwise { dst, lhs, rhs, .. }
        | Inst::Compare { dst, lhs, rhs, .. }
        | Inst::BoolAnd { dst, lhs, rhs } => {
            f(dst);
            f(lhs);
            f(rhs);
        }
        Inst::Project { dst, base, .. }
        | Inst::MemLoad { dst, base, .. }
        | Inst::PtrOffset { dst, base, .. }
        | Inst::MmioRead { dst, base, .. } => {
            f(dst);
            f(base);
        }
        Inst::SetField { base, value, .. } | Inst::MemStore { base, value, .. } => {
            f(base);
            f(value);
        }
        Inst::MmioWrite { base, value, .. } => {
            f(base);
            f(value);
        }
        Inst::IndexGet {
            dst, base, index, ..
        }
        | Inst::IndexGetProven {
            dst, base, index, ..
        }
        | Inst::BytesIndexGet { dst, base, index }
        | Inst::PlacedIndexGet {
            dst, base, index, ..
        }
        | Inst::PlacedIndexGetProven {
            dst, base, index, ..
        } => {
            f(dst);
            f(base);
            f(index);
        }
        Inst::IndexSet {
            base, index, value, ..
        }
        | Inst::IndexSetProven {
            base, index, value, ..
        }
        | Inst::PlacedIndexSet {
            base, index, value, ..
        }
        | Inst::PlacedIndexSetProven {
            base, index, value, ..
        } => {
            f(base);
            f(index);
            f(value);
        }
        Inst::InterruptCellStoreRelease { value, .. } => f(value),
        Inst::InterruptCellSwapAcquire { dst, value, .. }
        | Inst::InterruptCellFetchOrRelease { dst, value, .. } => {
            f(dst);
            f(value);
        }
        Inst::SlotMapMint { map } => f(map),
        Inst::TurnAddrFromId { dst, id } => {
            f(dst);
            f(id);
        }
        Inst::JumpIfFalse { cond, .. } => f(cond),
        Inst::Call {
            dst,
            write_backs,
            args,
            ..
        } => {
            f(dst);
            for a in args {
                f(a);
            }
            for (_, t) in write_backs {
                f(t);
            }
        }
        Inst::Return { value } => {
            if let Some(v) = value {
                f(v);
            }
        }
        Inst::Jump { .. } | Inst::Dmb { .. } | Inst::Wake { .. } => {}
        Inst::Abort { .. } | Inst::AssertFail { .. } => {}
    }
}

fn def_of(inst: &Inst) -> Option<Temp> {
    crate::mwir_facts::inst_facts(inst).defs.first().copied()
}

fn clobbers(inst: &Inst, out: &mut Vec<Temp>) {
    out.clear();
    out.extend(crate::mwir_facts::inst_facts(inst).defs);
}

fn reads_of(inst: &Inst, out: &mut Vec<Temp>) {
    out.clear();
    out.extend(crate::mwir_facts::inst_facts(inst).uses);
}

fn target_of(inst: &Inst) -> Option<usize> {
    match inst {
        Inst::Jump { target } | Inst::JumpIfFalse { target, .. } => Some(*target),
        _ => None,
    }
}

fn set_target(inst: &mut Inst, new: usize) {
    match inst {
        Inst::Jump { target } | Inst::JumpIfFalse { target, .. } => *target = new,
        _ => {}
    }
}

fn falls_through(inst: &Inst) -> bool {
    !matches!(
        inst,
        Inst::Jump { .. } | Inst::Return { .. } | Inst::Abort { .. } | Inst::AssertFail { .. }
    )
}

fn gvn_pure(inst: &Inst) -> bool {
    matches!(
        inst,
        Inst::ConstInt { .. }
            | Inst::ConstBool { .. }
            | Inst::ConstFloat { .. }
            | Inst::ConstChar { .. }
            | Inst::ConstUnit { .. }
            | Inst::ConstText { .. }
            | Inst::ArithChecked { .. }
            | Inst::ArithWrapping { .. }
            | Inst::DivRem { .. }
            | Inst::Shift { .. }
            | Inst::Bitwise { .. }
            | Inst::Compare { .. }
            | Inst::Neg { .. }
            | Inst::BitNot { .. }
            | Inst::Convert { .. }
            | Inst::Not { .. }
            | Inst::BoolAnd { .. }
            | Inst::PtrOffset { .. }
    )
}

fn dce_removable(inst: &Inst) -> bool {
    matches!(
        inst,
        Inst::ConstInt { .. }
            | Inst::ConstBool { .. }
            | Inst::ConstFloat { .. }
            | Inst::ConstChar { .. }
            | Inst::ConstUnit { .. }
            | Inst::ConstText { .. }
            | Inst::Copy { .. }
            | Inst::MakeAggregate { .. }
            | Inst::MakeEnum { .. }
            | Inst::Project { .. }
            | Inst::EnumTag { .. }
            | Inst::EnumPayload { .. }
            | Inst::ArithWrapping { .. }
            | Inst::Bitwise { .. }
            | Inst::Compare { .. }
            | Inst::BitNot { .. }
            | Inst::Not { .. }
            | Inst::BoolAnd { .. }
            | Inst::PtrOffset { .. }
    )
}

fn compact(body: &mut Vec<Inst>, keep: &[bool]) {
    debug_assert_eq!(body.len(), keep.len());
    let mut map = vec![0usize; body.len() + 1];
    let mut n = 0usize;
    for i in 0..body.len() {
        map[i] = n;
        if keep[i] {
            n += 1;
        }
    }
    map[body.len()] = n;
    let mut out = Vec::with_capacity(n);
    for (i, inst) in body.drain(..).enumerate() {
        if keep[i] {
            out.push(inst);
        }
    }
    for inst in &mut out {
        if let Some(t) = target_of(inst) {
            set_target(inst, map[t.min(map.len() - 1)]);
        }
    }
    *body = out;
}

fn ebb_leaders(body: &[Inst]) -> Vec<bool> {
    let mut leader = vec![false; body.len()];
    if body.is_empty() {
        return leader;
    }
    leader[0] = true;
    for (i, inst) in body.iter().enumerate() {
        if let Some(t) = target_of(inst) {
            if t < leader.len() {
                leader[t] = true;
            }
        }
        if !falls_through(inst) && i + 1 < leader.len() {
            leader[i + 1] = true;
        }
    }
    leader
}

fn is_late_bound(key: &str) -> bool {
    key.starts_with("__") || key.starts_with("rt_") || runtime_closure_keys().contains(key)
}

fn runtime_closure_keys() -> &'static BTreeSet<String> {
    static KEYS: std::sync::OnceLock<BTreeSet<String>> = std::sync::OnceLock::new();
    KEYS.get_or_init(|| {
        let mut out = BTreeSet::new();
        let Ok((_, loaded)) = crate::loader::load_runtime_module() else {
            return out;
        };
        collect_module_fn_keys(&loaded.module, &mut out);
        out
    })
}

fn runtime_closure_is_known() -> bool {
    !runtime_closure_keys().is_empty()
}

fn collect_module_fn_keys(m: &crate::syntax::ast::Module, out: &mut BTreeSet<String>) {
    use crate::syntax::ast::{Item, Member};
    for item in &m.items {
        match item {
            Item::Fn(f) => {
                out.insert(f.name.clone());
            }
            Item::Struct(s) => {
                for member in &s.members {
                    if let Member::Fn(f) = member {
                        out.insert(format!("{}.{}", s.name, f.name));
                    }
                }
            }
            _ => {}
        }
    }
}

const INLINE_MAX_BODY: usize = 8;

const INLINE_MAX_ROUNDS: usize = 4;

const INLINE_FRAME_CEILING: usize = 4095;
const INLINE_FRAME_SLACK: usize = 64;

fn inline_refusal(key: &str, f: &MwirFn) -> Option<&'static str> {
    if is_late_bound(key) {
        return Some("late-bound: layout.rs replaces this body after codegen");
    }
    if f.body.is_empty() {
        return Some("empty body");
    }
    if f.receiver.is_some() {
        return Some("has a receiver");
    }
    if f.params.iter().any(|(_, m)| *m != AccessMode::Read) {
        return Some("has a `mut`/`take` parameter");
    }
    let params: BTreeSet<Temp> = f.params.iter().map(|(t, _)| *t).collect();
    let mut clob = Vec::new();
    for inst in &f.body {
        match inst {
            Inst::Call { .. } => return Some("not a leaf"),
            Inst::InterruptCellLoadAcquire { .. }
            | Inst::InterruptCellStoreRelease { .. }
            | Inst::InterruptCellSwapAcquire { .. }
            | Inst::InterruptCellFetchOrRelease { .. } => return Some("touches an InterruptCell"),
            _ => {}
        }
        clobbers(inst, &mut clob);
        if clob.iter().any(|t| params.contains(t)) {
            return Some("assigns its own parameter");
        }
    }
    None
}

fn reference_counts(prog: &MwirProgram, flow: Option<&FlowWirProgram>) -> BTreeMap<String, usize> {
    let mut refs: BTreeMap<String, usize> = BTreeMap::new();
    let bump = |k: &str, refs: &mut BTreeMap<String, usize>| {
        *refs.entry(k.to_string()).or_insert(0) += 1;
    };
    for f in prog.fns.values() {
        for inst in &f.body {
            if let Inst::Call { key, .. } = inst {
                bump(key, &mut refs);
            }
        }
    }
    if let Some(flow) = flow {
        for f in flow.fns.values() {
            for state in &f.states {
                for op in &state.ops {
                    match op {
                        FlowInst::Mwir(Inst::Call { key, .. }) => bump(key, &mut refs),
                        FlowInst::Send { method_key, .. } => bump(method_key, &mut refs),
                        FlowInst::GroupStart { callee_key, .. } => bump(callee_key, &mut refs),
                        _ => {}
                    }
                }
                if let Transition::Await {
                    what: AwaitKind::ActorCall { method_key, .. },
                    ..
                } = &state.transition
                {
                    bump(method_key, &mut refs);
                }
            }
        }
    }
    refs
}

fn inline_program(prog: &mut MwirProgram, flow: Option<&FlowWirProgram>, layout: &LayoutCtx) {
    for _ in 0..INLINE_MAX_ROUNDS {
        let refs = reference_counts(prog, flow);
        let keys: Vec<String> = prog.fns.keys().cloned().collect();
        let mut consumed: BTreeSet<String> = BTreeSet::new();
        let mut changed = false;
        for caller_key in &keys {
            if is_late_bound(caller_key) {
                continue;
            }
            let mut caller = prog.fns[caller_key].clone();
            let mut moved: Vec<String> = Vec::new();
            if inline_into(&mut caller, caller_key, prog, &refs, layout, &mut moved) {
                changed = true;
                prog.fns.insert(caller_key.clone(), caller);
                consumed.extend(moved);
            }
        }
        for k in &consumed {
            prog.fns.remove(k);
            INLINED_CALLEES_DELETED.with(|c| c.set(c.get() + 1));
        }
        if !changed {
            return;
        }
    }
}

fn inline_into(
    caller: &mut MwirFn,
    caller_key: &str,
    prog: &MwirProgram,
    refs: &BTreeMap<String, usize>,
    layout: &LayoutCtx,
    moved: &mut Vec<String>,
) -> bool {
    let mut any = false;
    loop {
        let mut site: Option<(usize, String, bool)> = None;
        for (i, inst) in caller.body.iter().enumerate() {
            let Inst::Call {
                key,
                write_backs,
                args,
                ..
            } = inst
            else {
                continue;
            };
            if key == caller_key || !write_backs.is_empty() {
                continue;
            }
            let Some(callee) = prog.fns.get(key) else {
                continue;
            };
            if inline_refusal(key, callee).is_some() || args.len() != callee.params.len() {
                continue;
            }
            let single = refs.get(key).copied() == Some(1);
            let small = !inline_rule_one_only() && callee.body.len() <= INLINE_MAX_BODY;
            if !(single || small) {
                continue;
            }
            site = Some((i, key.clone(), single));
            break;
        }
        let Some((at, key, single)) = site else {
            return any;
        };
        if !splice(caller, at, &prog.fns[&key], layout) {
            return any;
        }
        any = true;
        if single {
            moved.push(key);
        }
    }
}

fn frame_estimate(temp_types: &[Type], f: &MwirFn, layout: &LayoutCtx) -> Option<usize> {
    let mut off = 0usize;
    for ty in temp_types {
        off += crate::mwir::size_of(ty, layout).ok()?;
    }
    off += 8;
    off += 8 * f.params.len();
    off += 8;
    off += 8;
    off += INLINE_FRAME_SLACK;
    Some((off + 15) & !15)
}

fn splice(caller: &mut MwirFn, at: usize, callee: &MwirFn, layout: &LayoutCtx) -> bool {
    let Inst::Call { dst, args, .. } = caller.body[at].clone() else {
        return false;
    };
    if args.len() != callee.params.len() {
        return false;
    }

    let mut map: BTreeMap<usize, Temp> = BTreeMap::new();
    for ((p, _), a) in callee.params.iter().zip(args.iter()) {
        map.insert(p.0, *a);
    }
    let mut new_types = caller.temp_types.clone();
    let mut mentioned: BTreeSet<usize> = BTreeSet::new();
    for inst in &callee.body {
        let mut c = inst.clone();
        visit_temps_mut(&mut c, &mut |t| {
            mentioned.insert(t.0);
        });
    }
    for t in mentioned {
        if map.contains_key(&t) {
            continue;
        }
        let Some(ty) = callee.temp_types.get(t) else {
            return false;
        };
        map.insert(t, Temp(new_types.len()));
        new_types.push(ty.clone());
    }
    match frame_estimate(&new_types, caller, layout) {
        Some(bytes) if bytes <= INLINE_FRAME_CEILING => {}
        _ => return false,
    }

    let n = callee.body.len();
    let mut start = vec![0usize; n + 1];
    let mut out: Vec<Inst> = Vec::with_capacity(n + 1);
    for (j, inst) in callee.body.iter().enumerate() {
        start[j] = out.len();
        let mut inst = inst.clone();
        visit_temps_mut(&mut inst, &mut |t| {
            if let Some(r) = map.get(&t.0) {
                *t = *r;
            }
        });
        match inst {
            Inst::Return { value } => {
                if let Some(v) = value {
                    out.push(Inst::Copy { dst, src: v });
                }
                if j + 1 != n {
                    out.push(Inst::Jump { target: n });
                }
            }
            other => out.push(other),
        }
    }
    start[n] = out.len();

    for inst in &mut out {
        if let Some(t) = target_of(inst) {
            set_target(inst, at + start[t.min(n)]);
        }
    }
    let delta = out.len() as isize - 1;
    let shift = |u: usize| -> usize {
        if u <= at {
            u
        } else {
            (u as isize + delta) as usize
        }
    };
    for (i, inst) in caller.body.iter_mut().enumerate() {
        if i == at {
            continue;
        }
        if let Some(t) = target_of(inst) {
            set_target(inst, shift(t));
        }
    }

    caller.body.splice(at..at + 1, out);
    caller.temp_types = new_types;
    INLINED_SITES.with(|c| c.set(c.get() + 1));
    true
}

fn const_prop_fn(f: &mut MwirFn) {
    let leader = ebb_leaders(&f.body);
    let mut known: BTreeMap<Temp, Value> = BTreeMap::new();
    let mut clob = Vec::new();
    for i in 0..f.body.len() {
        if leader[i] {
            known.clear();
        }
        if let Some(folded) = fold(&f.body[i], &known) {
            f.body[i] = folded;
        }
        let inst = f.body[i].clone();
        clobbers(&inst, &mut clob);
        for t in &clob {
            known.remove(t);
        }
        match &inst {
            Inst::ConstInt { dst, ty, value } => {
                if let Some(v) = int_value(ty, *value) {
                    known.insert(*dst, v);
                }
            }
            Inst::ConstBool { dst, value } => {
                known.insert(*dst, Value::Bool(*value));
            }
            Inst::ConstChar { dst, value } => {
                known.insert(*dst, Value::Char(*value));
            }
            Inst::Copy { dst, src } => {
                if let Some(v) = known.get(src).cloned() {
                    known.insert(*dst, v);
                }
            }
            _ => {}
        }
    }
    let mut keep = vec![true; f.body.len()];
    let leader = ebb_leaders(&f.body);
    let mut known: BTreeMap<Temp, Value> = BTreeMap::new();
    for i in 0..f.body.len() {
        if leader[i] {
            known.clear();
        }
        if let Inst::JumpIfFalse { cond, target } = &f.body[i] {
            match known.get(cond) {
                Some(Value::Bool(true)) => {
                    keep[i] = false;
                }
                Some(Value::Bool(false)) => {
                    let t = *target;
                    f.body[i] = Inst::Jump { target: t };
                }
                _ => {}
            }
        }
        let inst = f.body[i].clone();
        clobbers(&inst, &mut clob);
        for t in &clob {
            known.remove(t);
        }
        match &inst {
            Inst::ConstBool { dst, value } => {
                known.insert(*dst, Value::Bool(*value));
            }
            Inst::Copy { dst, src } => {
                if let Some(v) = known.get(src).cloned() {
                    known.insert(*dst, v);
                }
            }
            _ => {}
        }
    }
    if keep.iter().any(|k| !k) {
        compact(&mut f.body, &keep);
    }
}

fn int_value(ty: &Type, v: i128) -> Option<Value> {
    match ty {
        Type::U8
        | Type::U16
        | Type::U32
        | Type::U64
        | Type::Usize
        | Type::I8
        | Type::I16
        | Type::I32
        | Type::I64
        | Type::Isize => Some(value::make_int(ty, v)),
        _ => None,
    }
}

fn const_inst(dst: Temp, ty: &Type, v: &Value) -> Option<Inst> {
    match v {
        Value::Bool(b) => Some(Inst::ConstBool { dst, value: *b }),
        Value::Char(c) => Some(Inst::ConstChar { dst, value: *c }),
        _ => {
            let i = value::as_i128(v)?;
            int_value(ty, i)?;
            Some(Inst::ConstInt {
                dst,
                ty: ty.clone(),
                value: i,
            })
        }
    }
}

fn fold(inst: &Inst, known: &BTreeMap<Temp, Value>) -> Option<Inst> {
    let g = |t: &Temp| known.get(t);
    match inst {
        Inst::ArithChecked {
            dst,
            op,
            ty,
            lhs,
            rhs,
            ..
        } => {
            if !matches!(op, BinOp::Add | BinOp::Sub | BinOp::Mul) {
                return None;
            }
            let v = value::eval_ordinary(*op, ty, g(lhs)?, g(rhs)?).ok()?;
            const_inst(*dst, ty, &v)
        }
        Inst::ArithWrapping {
            dst,
            op,
            ty,
            lhs,
            rhs,
        } => {
            let v = value::eval_wrapping(*op, ty, g(lhs)?, g(rhs)?).ok()?;
            const_inst(*dst, ty, &v)
        }
        Inst::DivRem {
            dst,
            op,
            ty,
            lhs,
            rhs,
            ..
        } => {
            let v = value::eval_div_rem(*op, ty, g(lhs)?, g(rhs)?).ok()?;
            const_inst(*dst, ty, &v)
        }
        Inst::Shift {
            dst,
            op,
            ty,
            lhs,
            rhs,
            ..
        } => {
            let v = value::eval_shift(*op, ty, g(lhs)?, g(rhs)?).ok()?;
            const_inst(*dst, ty, &v)
        }
        Inst::Bitwise {
            dst,
            op,
            ty,
            lhs,
            rhs,
        } => {
            let v = value::eval_bitwise(*op, ty, g(lhs)?, g(rhs)?).ok()?;
            const_inst(*dst, ty, &v)
        }
        Inst::Compare {
            dst, op, lhs, rhs, ..
        } => {
            let (l, r) = (g(lhs)?, g(rhs)?);
            if matches!(l, Value::F32(_) | Value::F64(_))
                || matches!(r, Value::F32(_) | Value::F64(_))
            {
                return None;
            }
            let value = match op {
                BinOp::Lt | BinOp::Le | BinOp::Gt | BinOp::Ge => value::eval_compare(*op, l, r),
                BinOp::Eq | BinOp::Ne => {
                    let eq = match (l, r) {
                        (Value::Bool(a), Value::Bool(b)) => a == b,
                        (Value::Char(a), Value::Char(b)) => a == b,
                        _ => value::as_i128(l)? == value::as_i128(r)?,
                    };
                    if *op == BinOp::Eq { eq } else { !eq }
                }
                _ => return None,
            };
            Some(Inst::ConstBool { dst: *dst, value })
        }
        Inst::Neg { dst, ty, src, .. } => {
            let s = g(src)?;
            if !matches!(
                s,
                Value::I8(_)
                    | Value::I16(_)
                    | Value::I32(_)
                    | Value::I64(_)
                    | Value::Isize(_)
                    | Value::F32(_)
                    | Value::F64(_)
            ) {
                return None;
            }
            let v = value::eval_neg(s).ok()?;
            const_inst(*dst, ty, &v)
        }
        Inst::BitNot { dst, ty, src } => {
            let v = value::eval_bitnot(ty, g(src)?).ok()?;
            const_inst(*dst, ty, &v)
        }
        Inst::Not { dst, src } => match g(src)? {
            Value::Bool(b) => Some(Inst::ConstBool {
                dst: *dst,
                value: !*b,
            }),
            _ => None,
        },
        Inst::BoolAnd { dst, lhs, rhs } => match (g(lhs)?, g(rhs)?) {
            (Value::Bool(a), Value::Bool(b)) => Some(Inst::ConstBool {
                dst: *dst,
                value: *a && *b,
            }),
            _ => None,
        },
        Inst::Convert { dst, ty, src, .. } => {
            let v = value::eval_to_scalar(ty, g(src)?).ok()?;
            const_inst(*dst, ty, &v)
        }
        _ => None,
    }
}

fn gvn_key(inst: &Inst) -> Inst {
    let mut k = inst.clone();
    let mut first = true;
    visit_temps_mut(&mut k, &mut |t| {
        if first {
            *t = Temp(usize::MAX);
            first = false;
        }
    });
    k
}

fn gvn_fn(f: &mut MwirFn) {
    let leader = ebb_leaders(&f.body);
    let mut introduced: Vec<usize> = Vec::new();
    let mut table: Vec<(Inst, Temp)> = Vec::new();
    let mut rewrite: BTreeMap<Temp, Temp> = BTreeMap::new();
    let mut clob = Vec::new();
    for i in 0..f.body.len() {
        if leader[i] {
            table.clear();
            rewrite.clear();
        }
        if !rewrite.is_empty() {
            let def = def_of(&f.body[i]);
            let mut first = true;
            visit_temps_mut(&mut f.body[i], &mut |t| {
                if first && def.is_some() {
                    first = false;
                    return;
                }
                first = false;
                if let Some(r) = rewrite.get(t) {
                    *t = *r;
                }
            });
        }
        let inst = f.body[i].clone();
        if gvn_pure(&inst) {
            let key = gvn_key(&inst);
            let dst = def_of(&inst).expect("a pure computation defines a temp");
            if let Some((_, prev)) = table.iter().find(|(k, _)| *k == key) {
                let prev = *prev;
                if prev != dst {
                    f.body[i] = Inst::Copy { dst, src: prev };
                    introduced.push(i);
                    rewrite.insert(dst, prev);
                }
                clobbers(&f.body[i], &mut clob);
                invalidate(&mut table, &mut rewrite, &clob, dst);
                rewrite.insert(dst, prev);
                continue;
            }
            clobbers(&inst, &mut clob);
            invalidate(&mut table, &mut rewrite, &clob, dst);
            table.push((key, dst));
            continue;
        }
        clobbers(&inst, &mut clob);
        invalidate(&mut table, &mut rewrite, &clob, Temp(usize::MAX));
    }
    collect_gvn_copies(f, &introduced);
}

fn collect_gvn_copies(f: &mut MwirFn, introduced: &[usize]) {
    if introduced.is_empty() {
        return;
    }
    let mut read: BTreeSet<Temp> = BTreeSet::new();
    if let Some((t, _)) = &f.receiver {
        read.insert(*t);
    }
    for (t, _) in &f.params {
        read.insert(*t);
    }
    let mut buf = Vec::new();
    for inst in &f.body {
        reads_of(inst, &mut buf);
        for t in &buf {
            read.insert(*t);
        }
    }
    let mut keep = vec![true; f.body.len()];
    let mut any = false;
    for &i in introduced {
        let Inst::Copy { dst, .. } = &f.body[i] else {
            continue;
        };
        if !read.contains(dst) {
            keep[i] = false;
            any = true;
        }
    }
    if any {
        compact(&mut f.body, &keep);
    }
}

fn invalidate(
    table: &mut Vec<(Inst, Temp)>,
    rewrite: &mut BTreeMap<Temp, Temp>,
    clob: &[Temp],
    skip_self: Temp,
) {
    if clob.is_empty() {
        return;
    }
    let hits = |inst: &Inst| -> bool {
        let mut found = false;
        let mut c = inst.clone();
        visit_temps_mut(&mut c, &mut |t| {
            if clob.contains(t) {
                found = true;
            }
        });
        found
    };
    table.retain(|(k, v)| !clob.contains(v) && !hits(k));
    rewrite.retain(|k, v| !clob.contains(v) && (*k == skip_self || !clob.contains(k)));
    if skip_self != Temp(usize::MAX) {
        rewrite.remove(&skip_self);
    }
}

fn dce_fn(f: &mut MwirFn) {
    loop {
        let mut keep = vec![true; f.body.len()];
        let reach = reachable(&f.body);
        let mut changed = false;
        for i in 0..f.body.len() {
            if !reach[i] {
                keep[i] = false;
                changed = true;
            }
        }
        let mut read: BTreeSet<Temp> = BTreeSet::new();
        if let Some((t, _)) = &f.receiver {
            read.insert(*t);
        }
        for (t, _) in &f.params {
            read.insert(*t);
        }
        let mut buf = Vec::new();
        for (i, inst) in f.body.iter().enumerate() {
            if !keep[i] {
                continue;
            }
            reads_of(inst, &mut buf);
            for t in &buf {
                read.insert(*t);
            }
        }
        for (i, inst) in f.body.iter().enumerate() {
            if !keep[i] || !dce_removable(inst) {
                continue;
            }
            let Some(d) = def_of(inst) else { continue };
            if !read.contains(&d) {
                keep[i] = false;
                changed = true;
            }
        }
        if !changed {
            return;
        }
        compact(&mut f.body, &keep);
    }
}

fn reachable(body: &[Inst]) -> Vec<bool> {
    let mut seen = vec![false; body.len()];
    if body.is_empty() {
        return seen;
    }
    let mut stack = vec![0usize];
    while let Some(i) = stack.pop() {
        if i >= body.len() || seen[i] {
            continue;
        }
        seen[i] = true;
        if let Some(t) = target_of(&body[i]) {
            stack.push(t);
        }
        if falls_through(&body[i]) {
            stack.push(i + 1);
        }
    }
    seen
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::opts::{CompileMode, OptId, apply_mode, apply_opts};
    use crate::sema;
    use crate::syntax::{lexer, parser};

    fn one_of_every_variant() -> Vec<Inst> {
        let t = |n: usize| Temp(n);
        let u = || Type::U64;
        vec![
            Inst::ConstInt {
                dst: t(1),
                ty: u(),
                value: 7,
            },
            Inst::ConstBool {
                dst: t(2),
                value: true,
            },
            Inst::ConstFloat {
                dst: t(3),
                ty: Type::F64,
                bits: 0,
            },
            Inst::ConstChar {
                dst: t(4),
                value: 'x',
            },
            Inst::ConstUnit { dst: t(5) },
            Inst::ConstText { dst: t(6), data: 0 },
            Inst::Copy {
                dst: t(7),
                src: t(8),
            },
            Inst::MakeAggregate {
                dst: t(9),
                elems: vec![t(10), t(11)],
            },
            Inst::FormatScalar {
                dst: t(12),
                src: t(13),
                src_ty: u(),
                capacity: 4,
            },
            Inst::StringConcat {
                dst: t(14),
                lhs: t(15),
                rhs: t(16),
                lhs_cap: 1,
                rhs_cap: 1,
            },
            Inst::Project {
                dst: t(17),
                base: t(18),
                index: 3,
            },
            Inst::SetField {
                base: t(19),
                index: 3,
                value: t(20),
            },
            Inst::IndexGet {
                dst: t(21),
                base: t(22),
                index: t(23),
                len: 4,
            },
            Inst::IndexSet {
                base: t(24),
                index: t(25),
                value: t(26),
                len: 4,
            },
            Inst::PlacedIndexGet {
                dst: t(27),
                base: t(28),
                field_offset: 0,
                index: t(29),
                len: 4,
                elem_stride: 8,
                ty: u(),
            },
            Inst::PlacedIndexSet {
                base: t(30),
                field_offset: 0,
                index: t(31),
                value: t(32),
                len: 4,
                elem_stride: 8,
                ty: u(),
            },
            Inst::BytesIndexGet {
                dst: t(33),
                base: t(34),
                index: t(35),
            },
            Inst::MakeEnum {
                dst: t(36),
                tag: 0,
                payload: vec![t(37)],
            },
            Inst::EnumTag {
                dst: t(38),
                src: t(39),
            },
            Inst::EnumPayload {
                dst: t(40),
                src: t(41),
                index: 0,
            },
            Inst::ArithChecked {
                dst: t(42),
                op: BinOp::Add,
                ty: u(),
                lhs: t(43),
                rhs: t(44),
                abort: String::new(),
            },
            Inst::ArithWrapping {
                dst: t(45),
                op: BinOp::AddW,
                ty: u(),
                lhs: t(46),
                rhs: t(47),
            },
            Inst::DivRem {
                dst: t(48),
                op: BinOp::Div,
                ty: u(),
                lhs: t(49),
                rhs: t(50),
                abort_zero: String::new(),
                abort_overflow: String::new(),
            },
            Inst::Shift {
                dst: t(51),
                op: BinOp::Shr,
                ty: u(),
                lhs: t(52),
                rhs: t(53),
                bits: 64,
                lost: None,
            },
            Inst::Bitwise {
                dst: t(54),
                op: BinOp::BitAnd,
                ty: u(),
                lhs: t(55),
                rhs: t(56),
            },
            Inst::Compare {
                dst: t(57),
                op: BinOp::Lt,
                ty: u(),
                lhs: t(58),
                rhs: t(59),
            },
            Inst::Neg {
                dst: t(60),
                ty: Type::I64,
                src: t(61),
                abort: String::new(),
            },
            Inst::BitNot {
                dst: t(62),
                ty: u(),
                src: t(63),
            },
            Inst::Convert {
                dst: t(64),
                ty: u(),
                src: t(65),
                abort: String::new(),
            },
            Inst::Not {
                dst: t(66),
                src: t(67),
            },
            Inst::BoolAnd {
                dst: t(68),
                lhs: t(69),
                rhs: t(70),
            },
            Inst::Jump { target: 0 },
            Inst::JumpIfFalse {
                cond: t(71),
                target: 0,
            },
            Inst::Call {
                dst: t(72),
                write_backs: vec![(0, t(73))],
                key: "callee".into(),
                args: vec![t(73), t(74)],
            },
            Inst::Return { value: Some(t(75)) },
            Inst::MmioRead {
                dst: t(76),
                base: t(77),
                offset: 0,
                ty: u(),
            },
            Inst::MmioWrite {
                base: t(78),
                offset: 0,
                ty: u(),
                value: t(79),
            },
            Inst::LoadIrqVector {
                dst: t(80),
                driver: "D".into(),
            },
            Inst::InterruptCellLoadAcquire {
                dst: t(81),
                field_off: 0,
                width: 4,
            },
            Inst::InterruptCellStoreRelease {
                field_off: 0,
                width: 4,
                value: t(82),
            },
            Inst::InterruptCellSwapAcquire {
                dst: t(83),
                field_off: 0,
                width: 4,
                value: t(84),
            },
            Inst::InterruptCellFetchOrRelease {
                dst: t(85),
                field_off: 0,
                width: 4,
                value: t(86),
            },
            Inst::Dmb {
                option: "ishst".into(),
            },
            Inst::Wake { driver: "D".into() },
            Inst::Now { dst: t(87) },
            Inst::Entropy { dst: t(88), n: 4 },
            Inst::SlotMapMint { map: t(89) },
            Inst::MemLoad {
                dst: t(90),
                base: t(91),
                offset: 0,
                width: 8,
            },
            Inst::MemStore {
                base: t(92),
                offset: 0,
                value: t(93),
                width: 8,
            },
            Inst::PtrOffset {
                dst: t(94),
                base: t(95),
                offset: 0,
            },
            Inst::TurnAddrFromId {
                dst: t(96),
                id: t(97),
            },
            Inst::Abort {
                message: "m".into(),
            },
            Inst::AssertFail { message: None },
        ]
    }

    #[test]
    fn visit_temps_mut_visits_exactly_the_temps_the_dump_prints() {
        for inst in one_of_every_variant() {
            let printed: BTreeSet<usize> = crate::mwir::fmt_inst(&inst)
                .split(|c: char| !c.is_ascii_alphanumeric())
                .filter_map(|w| w.strip_prefix('t').and_then(|d| d.parse::<usize>().ok()))
                .collect();
            let mut seen: BTreeSet<usize> = BTreeSet::new();
            let mut c = inst.clone();
            visit_temps_mut(&mut c, &mut |t| {
                seen.insert(t.0);
            });
            assert_eq!(
                seen,
                printed,
                "visit_temps_mut disagrees with the dump for `{}`",
                crate::mwir::fmt_inst(&inst)
            );
        }
    }

    #[test]
    fn the_variant_list_covers_every_inst_shape() {
        let names: BTreeSet<String> = one_of_every_variant()
            .iter()
            .map(|i| {
                crate::mwir::fmt_inst(i)
                    .split([' ', '\n'])
                    .next()
                    .unwrap_or_default()
                    .to_string()
            })
            .collect();
        assert_eq!(
            names.len(),
            53,
            "one instance of every `mwir::Inst` variant, distinctly named: {names:?}"
        );
    }

    fn lower(src: &str) -> (MwirProgram, crate::mwir::LayoutCtx) {
        let tokens = lexer::lex(src).expect("lex");
        let module = parser::parse(tokens).expect("parse");
        let typed = sema::check_typed(&module, "<test>").expect("check");
        let layout = crate::mwir::build_layout_ctx(&module, &Default::default()).expect("layout");
        let mwir = crate::lower::lower_program(&typed).expect("lower");
        (mwir, layout)
    }

    const FOLDS: &str = r#"
module examples.j_fold

pub fn shifted() -> u64:
    a: u64 = 6
    b: u64 = 7
    return (a *% b) >> 1
"#;

    #[test]
    fn const_prop_folds_a_whole_expression() {
        apply_opts(&[OptId::ConstProp]);
        let (mwir, layout) = lower(FOLDS);
        let opt = optimize(&mwir, None, &layout).expect("passes ran");
        apply_mode(CompileMode::Release);
        let f = &opt.fns["shifted"];
        assert!(
            !f.body
                .iter()
                .any(|i| matches!(i, Inst::Shift { .. } | Inst::ArithWrapping { .. })),
            "the shift and the multiply must both fold:\n{f:?}"
        );
        assert!(
            f.body
                .iter()
                .any(|i| matches!(i, Inst::ConstInt { value: 21, .. })),
            "the folded value must be 21:\n{f:?}"
        );
    }

    #[test]
    fn const_prop_refuses_to_fold_what_would_abandon() {
        const OVERFLOWS: &str = r#"
module examples.j_fold_trap

pub fn boom() -> u8:
    a: u8 = 200
    b: u8 = 200
    return a + b
"#;
        apply_opts(&[OptId::ConstProp]);
        let (mwir, layout) = lower(OVERFLOWS);
        let opt = optimize(&mwir, None, &layout).expect("passes ran");
        apply_mode(CompileMode::Release);
        let f = &opt.fns["boom"];
        assert!(
            f.body
                .iter()
                .any(|i| matches!(i, Inst::ArithChecked { .. })),
            "a checked add that overflows must survive, abort path and all:\n{f:?}"
        );
    }

    const REDUNDANT: &str = r#"
module examples.j_gvn

pub fn twice(x: u64, y: u64) -> u64:
    return (x & y) +% (x & y)
"#;

    #[test]
    fn gvn_replaces_the_second_identical_computation() {
        apply_opts(&[]);
        let (mwir, layout) = lower(REDUNDANT);
        let base = mwir.fns["twice"]
            .body
            .iter()
            .filter(|i| matches!(i, Inst::Bitwise { .. }))
            .count();
        assert_eq!(base, 2, "the baseline must really compute it twice");

        apply_opts(&[OptId::Gvn]);
        let opt = optimize(&mwir, None, &layout).expect("passes ran");
        apply_mode(CompileMode::Release);
        let f = &opt.fns["twice"];
        assert_eq!(
            f.body
                .iter()
                .filter(|i| matches!(i, Inst::Bitwise { .. }))
                .count(),
            1,
            "the redundant computation must be numbered away:\n{f:?}"
        );
    }

    #[test]
    fn dce_deletes_the_copy_gvn_left_behind() {
        apply_opts(&[OptId::Gvn, OptId::Dce]);
        let (mwir, layout) = lower(REDUNDANT);
        let opt = optimize(&mwir, None, &layout).expect("passes ran");
        apply_mode(CompileMode::Release);
        let f = &opt.fns["twice"];
        let before = mwir.fns["twice"].body.len();
        assert!(
            f.body.len() < before,
            "GVN+DCE must shrink the body ({before} -> {}):\n{f:?}",
            f.body.len()
        );
    }

    #[test]
    fn dce_keeps_a_dead_computation_that_can_abandon() {
        const DEAD: &str = r#"
module examples.j_dce_trap

pub fn f(a: u8, b: u8) -> u64:
    _unused = a + b
    return 0
"#;
        apply_opts(&[OptId::Dce]);
        let (mwir, layout) = lower(DEAD);
        let opt = optimize(&mwir, None, &layout).expect("passes ran");
        apply_mode(CompileMode::Release);
        let f = &opt.fns["f"];
        assert!(
            f.body
                .iter()
                .any(|i| matches!(i, Inst::ArithChecked { .. })),
            "a dead trapping op must survive DCE:\n{f:?}"
        );
    }

    #[test]
    fn dev_runs_none_of_the_three() {
        apply_mode(CompileMode::Dev);
        let (mwir, layout) = lower(REDUNDANT);
        assert!(
            optimize(&mwir, None, &layout).is_none(),
            "dev must not clone or rewrite the program at all"
        );
        apply_mode(CompileMode::Release);
    }

    fn u64s(n: usize) -> Vec<Type> {
        vec![Type::U64; n]
    }

    fn call(dst: usize, key: &str, args: &[usize]) -> Inst {
        Inst::Call {
            dst: Temp(dst),
            write_backs: Vec::new(),
            key: key.to_string(),
            args: args.iter().map(|a| Temp(*a)).collect(),
        }
    }

    const INLINABLE: &str = r#"
module examples.p_inline

pub fn p_leaf(x: u64) -> u64:
    return x +% x

pub fn p_twice_over(a: u64, b: u64) -> u64:
    return p_leaf(a) +% p_leaf(b)
"#;

    #[test]
    fn inlining_splices_a_small_leaf_at_every_site() {
        apply_opts(&[OptId::Inline]);
        let (mwir, layout) = lower(INLINABLE);
        assert_eq!(
            mwir.fns["p_twice_over"]
                .body
                .iter()
                .filter(|i| matches!(i, Inst::Call { .. }))
                .count(),
            2,
            "the baseline must really call it twice"
        );
        let opt = optimize(&mwir, None, &layout).expect("the inliner ran");
        apply_mode(CompileMode::Release);
        let f = &opt.fns["p_twice_over"];
        assert!(
            !f.body.iter().any(|i| matches!(i, Inst::Call { .. })),
            "both call sites must be gone:\n{f:?}"
        );
        assert_eq!(
            f.body
                .iter()
                .filter(|i| matches!(i, Inst::ArithWrapping { .. }))
                .count(),
            3,
            "the caller's own add plus one per splice:\n{f:?}"
        );
        assert!(
            opt.fns.contains_key("p_leaf"),
            "rule (ii) duplicates; it does not consume the callee"
        );
    }

    #[test]
    fn rule_one_moves_the_body_and_deletes_the_callee() {
        const ONCE: &str = r#"
module examples.p_inline_once

pub fn p_only_leaf(x: u64) -> u64:
    return x +% 1

pub fn p_only_user(a: u64) -> u64:
    return p_only_leaf(a)
"#;
        apply_opts(&[OptId::Inline]);
        let (mwir, layout) = lower(ONCE);
        let opt = optimize(&mwir, None, &layout).expect("the inliner ran");
        apply_mode(CompileMode::Release);
        assert!(
            !opt.fns.contains_key("p_only_leaf"),
            "its one reference was consumed, so the key goes:\n{:?}",
            opt.fns.keys().collect::<Vec<_>>()
        );
        let f = &opt.fns["p_only_user"];
        assert!(
            !f.body.iter().any(|i| matches!(i, Inst::Call { .. })),
            "the body moved:\n{f:?}"
        );
    }

    #[test]
    fn inlining_refuses_a_placeholder_body_layout_will_replace() {
        let layout = crate::mwir::LayoutCtx::default();
        for placeholder in ["__test_prefix_0", "rt_boot_init 0", "ascii_digit"] {
            let mut prog = MwirProgram::default();
            prog.fns.insert(
                placeholder.to_string(),
                MwirFn {
                    receiver: None,
                    params: Vec::new(),
                    ret: Type::Unit,
                    temp_types: u64s(1),
                    body: vec![Inst::Return { value: None }],
                },
            );
            prog.fns.insert(
                "p_caller".to_string(),
                MwirFn {
                    receiver: None,
                    params: Vec::new(),
                    ret: Type::Unit,
                    temp_types: u64s(2),
                    body: vec![call(0, placeholder, &[]), Inst::Return { value: None }],
                },
            );
            inline_program(&mut prog, None, &layout);
            assert!(
                prog.fns.contains_key(placeholder),
                "`{placeholder}` is a placeholder `layout.rs` replaces; deleting it \
                 is a silent miscompile"
            );
            assert!(
                prog.fns["p_caller"]
                    .body
                    .iter()
                    .any(|i| matches!(i, Inst::Call { key, .. } if key == placeholder)),
                "`{placeholder}`'s stub must not be spliced: what this pass can see \
                 at that key is not the program"
            );
        }
    }

    #[test]
    fn splicing_renames_every_temp_of_every_inst_shape() {
        const CALLER_TEMPS: usize = 128;
        let layout = crate::mwir::LayoutCtx::default();
        let body = one_of_every_variant();
        let widest = {
            let mut w = 0usize;
            for inst in &body {
                let mut c = inst.clone();
                visit_temps_mut(&mut c, &mut |t| w = w.max(t.0));
            }
            w + 1
        };
        assert!(
            widest < CALLER_TEMPS,
            "every callee temp number must be below the caller's, or this \
             assertion cannot tell a forgotten field from a fresh temp"
        );
        let callee = MwirFn {
            receiver: None,
            params: vec![(Temp(0), AccessMode::Read)],
            ret: Type::U64,
            temp_types: u64s(widest),
            body,
        };
        let mut caller = MwirFn {
            receiver: None,
            params: Vec::new(),
            ret: Type::Unit,
            temp_types: u64s(CALLER_TEMPS),
            body: vec![call(3, "callee", &[2]), Inst::Return { value: None }],
        };
        assert!(
            splice(&mut caller, 0, &callee, &layout),
            "the splice must not be refused"
        );
        for inst in &caller.body {
            let mut c = inst.clone();
            visit_temps_mut(&mut c, &mut |t| {
                assert!(
                    t.0 == 2 || t.0 == 3 || t.0 >= CALLER_TEMPS,
                    "t{} survived the splice unrenamed in `{}` — `visit_temps_mut` \
                     forgot a temp-shaped field, which is decision 1930's silent \
                     miscompile exactly",
                    t.0,
                    crate::mwir::fmt_inst(inst)
                );
            });
        }
    }

    #[test]
    fn the_inliner_refuses_what_aliasing_cannot_model() {
        let scalar = |body: Vec<Inst>, params: Vec<(Temp, AccessMode)>| MwirFn {
            receiver: None,
            params,
            ret: Type::U64,
            temp_types: u64s(8),
            body,
        };
        let read = vec![(Temp(0), AccessMode::Read)];
        let cases: Vec<(&str, MwirFn, &str)> = vec![
            (
                "not a leaf",
                scalar(vec![call(1, "other", &[0])], read.clone()),
                "not a leaf",
            ),
            (
                "mut parameter",
                scalar(
                    vec![Inst::Return {
                        value: Some(Temp(0)),
                    }],
                    vec![(Temp(0), AccessMode::Mut)],
                ),
                "has a `mut`/`take` parameter",
            ),
            (
                "take parameter",
                scalar(
                    vec![Inst::Return {
                        value: Some(Temp(0)),
                    }],
                    vec![(Temp(0), AccessMode::Take)],
                ),
                "has a `mut`/`take` parameter",
            ),
            (
                "assigns its own parameter",
                scalar(
                    vec![Inst::Copy {
                        dst: Temp(0),
                        src: Temp(1),
                    }],
                    read.clone(),
                ),
                "assigns its own parameter",
            ),
            (
                "writes through its own parameter",
                scalar(
                    vec![Inst::SetField {
                        base: Temp(0),
                        index: 0,
                        value: Temp(1),
                    }],
                    read.clone(),
                ),
                "assigns its own parameter",
            ),
            (
                "InterruptCell",
                scalar(
                    vec![Inst::InterruptCellLoadAcquire {
                        dst: Temp(1),
                        field_off: 0,
                        width: 4,
                    }],
                    read.clone(),
                ),
                "touches an InterruptCell",
            ),
        ];
        for (name, f, reason) in cases {
            assert_eq!(
                inline_refusal("p_callee", &f),
                Some(reason),
                "{name} must be refused"
            );
        }
        let mut with_self = scalar(
            vec![Inst::Return {
                value: Some(Temp(1)),
            }],
            Vec::new(),
        );
        with_self.receiver = Some((Temp(0), AccessMode::Read));
        assert_eq!(
            inline_refusal("p_callee", &with_self),
            Some("has a receiver")
        );
        assert_eq!(
            inline_refusal(
                "p_callee",
                &scalar(
                    vec![Inst::Return {
                        value: Some(Temp(0))
                    }],
                    read
                )
            ),
            None
        );
    }

    #[test]
    fn flowwir_references_count_towards_the_single_reference_rule() {
        use crate::flowwir::{FlowWirFn, FrameLayout, State};
        let layout = crate::mwir::LayoutCtx::default();
        let leaf = MwirFn {
            receiver: None,
            params: vec![(Temp(0), AccessMode::Read)],
            ret: Type::U64,
            temp_types: u64s(2),
            body: vec![Inst::Return {
                value: Some(Temp(0)),
            }],
        };
        let mut prog = MwirProgram::default();
        prog.fns.insert("p_shared_leaf".into(), leaf);
        prog.fns.insert(
            "p_sync_user".into(),
            MwirFn {
                receiver: None,
                params: Vec::new(),
                ret: Type::U64,
                temp_types: u64s(3),
                body: vec![
                    call(1, "p_shared_leaf", &[0]),
                    Inst::Return {
                        value: Some(Temp(1)),
                    },
                ],
            },
        );
        let mut flow = FlowWirProgram::default();
        flow.fns.insert(
            "p_async_user".into(),
            FlowWirFn {
                receiver: None,
                params: Vec::new(),
                ret: Type::U64,
                frame: FrameLayout {
                    temp_types: u64s(3),
                    lineage_group_slot: Temp(0),
                    lineage_deadline_slot: Temp(1),
                },
                states: vec![State {
                    ops: vec![FlowInst::Mwir(call(2, "p_shared_leaf", &[0]))],
                    transition: Transition::Return(Some(Temp(2))),
                }],
            },
        );
        let mut with_flow = prog.clone();
        inline_program(&mut with_flow, Some(&flow), &layout);
        assert!(
            with_flow.fns.contains_key("p_shared_leaf"),
            "the async state machine still calls it — rule (i) may not consume it"
        );
        let mut without_flow = prog.clone();
        inline_program(&mut without_flow, None, &layout);
        assert!(
            !without_flow.fns.contains_key("p_shared_leaf"),
            "with no other reference at all, rule (i) moves the body and \
             deletes the key"
        );
    }

    #[test]
    fn a_splice_keeps_jump_targets_honest() {
        const LOOPY: &str = r#"
module examples.p_inline_loop

pub fn p_loop_leaf(x: u64) -> u64:
    return x +% 1

pub fn p_loop_user(n: u64) -> u64:
    acc: u64 = 0
    i: u64 = 0
    @budget(bound=64)
    while i < n:
        acc = acc +% p_loop_leaf(i)
        i = i +% 1
    return acc
"#;
        apply_opts(&[OptId::Inline]);
        let (mwir, layout) = lower(LOOPY);
        let opt = optimize(&mwir, None, &layout).expect("the inliner ran");
        apply_mode(CompileMode::Release);
        let f = &opt.fns["p_loop_user"];
        assert!(
            !f.body.iter().any(|i| matches!(i, Inst::Call { .. })),
            "the site inside the loop must be spliced:\n{f:?}"
        );
        for inst in &f.body {
            if let Some(t) = target_of(inst) {
                assert!(
                    t <= f.body.len(),
                    "target {t} is past the body ({}) in `{}`",
                    f.body.len(),
                    crate::mwir::fmt_inst(inst)
                );
            }
        }
        assert!(
            f.body
                .iter()
                .enumerate()
                .any(|(i, inst)| target_of(inst).is_some_and(|t| t <= i)),
            "the back edge must survive the renumbering:\n{f:?}"
        );
    }

    #[test]
    fn both_inline_positions_are_deterministic() {
        for after in [false, true] {
            apply_opts(&[OptId::Inline, OptId::ConstProp, OptId::Gvn, OptId::Dce]);
            set_inline_after_redundancy(after);
            let (mwir, layout) = lower(INLINABLE);
            let a = optimize(&mwir, None, &layout).expect("ran");
            let b = optimize(&mwir, None, &layout).expect("ran");
            assert_eq!(
                crate::mwir::dump(&a),
                crate::mwir::dump(&b),
                "after={after}"
            );
        }
        set_inline_after_redundancy(false);
        apply_mode(CompileMode::Dev);
        let (mwir, layout) = lower(INLINABLE);
        assert!(optimize(&mwir, None, &layout).is_none());
        apply_mode(CompileMode::Release);
    }

    #[test]
    fn the_passes_are_deterministic() {
        apply_mode(CompileMode::Release);
        let (mwir, layout) = lower(REDUNDANT);
        let a = optimize(&mwir, None, &layout).expect("ran");
        let b = optimize(&mwir, None, &layout).expect("ran");
        assert_eq!(crate::mwir::dump(&a), crate::mwir::dump(&b));
    }
}
