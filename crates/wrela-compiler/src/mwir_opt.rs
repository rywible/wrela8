//! plans/codegen-pareto-2.md item J — three plain passes over MWIR.
//!
//! The optimization ladder's 2b (GVN + SCCP + DCE), written the way
//! CLAUDE.md says to write them: three ordinary functions over the
//! existing IR, in the existing single-threaded batch pipeline. **No pass
//! manager, no trait over "a pass", no framework.** Each pass is one `fn`
//! taking `&mut MwirFn`, and `optimize` below calls whichever of them
//! their TLS knobs enable, in a fixed order.
//!
//! **The ladder's 2a — the shrinking inliner — is here, parked**
//! (plans/codegen-pareto-2-P.md, decisions 1980–1989). Item J built it,
//! measured it, refused it (decision 1935) and then *deleted* it before
//! ever committing it, so the numbers that re-ranked the ladder's #1
//! candidate could not be reproduced from this repository at all. CLAUDE.md's
//! rule changed on the strength of exactly that (2026-07-31): **a refused
//! opt is parked, not deleted.** Item P rebuilds it from item J's stated
//! rule, keeps it out of `RELEASE_OPTS`, and re-derives the refusal with a
//! measurement anyone can now re-run.
//!
//! It also settles the question item J's measurement could not answer.
//! An inliner is an *enabling* pass — its value is what redundancy
//! elimination can do to the merged body afterwards — and item J's opt
//! list does not record where the inliner sat relative to `ConstProp`/
//! `Gvn`/`Dce` **inside** this one `optimize` call. [`set_inline_after_redundancy`]
//! is the knob that asks both orders; see [`optimize`] and
//! `opts::win`'s `the_inliner_measured_in_both_pipeline_positions`.
//!
//! **Where these run (decision 1920).** At the top of
//! `codegen::codegen_program` / `codegen::codegen_program_with_async`,
//! against the *merged* `MwirProgram`. That is the single choke point
//! every path shares — `wrela build`, `--stage=asm`, `--stage=report`,
//! the cost stage the ∀ gate scores, `diff-eval` and both fuzz lanes —
//! so the program the gate ranks is byte-for-byte the program that
//! ships. It is also the point at which the program is *whole*:
//! `lower.rs` emits an imported fn into every importing module's own
//! MWIR and `merge_mwir_programs` resolves the duplicates last-wins, so
//! a per-module pass would be looking at a different program from the
//! one that ships.
//!
//! **The async path is excluded (decision 1927)**, the analogue of item
//! E's decision 1762. A FlowWir fn's temps live in the persistent turn
//! area precisely because they must survive a `ret`-to-scheduler
//! suspension; a state's `ops` are one straight-line list but a temp
//! defined in one state is read in another, so a per-state pass has no
//! whole-body liveness to reason from and DCE would delete a definition
//! whose only reader is three states away. FlowWir is never rewritten
//! here and never read.

use std::cell::Cell;
use std::collections::{BTreeMap, BTreeSet};

use crate::eval::value::{self, Value};
use crate::flowwir::{AwaitKind, FlowInst, FlowWirProgram, Transition};
use crate::mwir::{Inst, LayoutCtx, MwirFn, MwirProgram, Temp};
use crate::sema::types::Type;
use crate::syntax::ast::{AccessMode, BinOp};

// --- knobs ---------------------------------------------------------------

thread_local! {
    static INLINE: Cell<bool> = const { Cell::new(false) };
    static INLINE_AFTER_REDUNDANCY: Cell<bool> = const { Cell::new(false) };
    static CONST_PROP: Cell<bool> = const { Cell::new(false) };
    static GVN: Cell<bool> = const { Cell::new(false) };
    static DCE: Cell<bool> = const { Cell::new(false) };
}

/// Item P / decision 1980: the shrinking inliner, **parked**. Present,
/// wired, proved by `diff-eval`, and deliberately absent from
/// `RELEASE_OPTS` — see [`inline_program`] for the rule and
/// plans/codegen-pareto-2-P.md for the measurement that refuses it.
pub fn set_inline(enabled: bool) {
    INLINE.with(|c| c.set(enabled));
}
pub fn inlining() -> bool {
    INLINE.with(|c| c.get())
}

/// **Item P / decision 1981 — the pipeline position, as a knob rather
/// than as an argument.**
///
/// `false` (the default) puts the inliner *ahead* of `ConstProp`/`Gvn`/
/// `Dce`: the splice happens first and redundancy elimination then runs
/// over the merged body. That is the only order in which an enabling
/// pass can enable anything, and it is what an inliner has to be
/// measured in to be measured at all.
///
/// `true` puts it *after* all three, where by construction it can enable
/// nothing: the callee's instructions arrive in the caller after the last
/// pass that could have folded them together.
///
/// Item J's numbers were taken with the inliner somewhere in this one
/// `optimize` call and the position was never committed. Item P measures
/// both and reports the pair; the knob exists for that measurement and
/// for nothing else, so it is **not** an [`crate::opts::OptId`] — an id
/// is a product decision and this is a question.
pub fn set_inline_after_redundancy(enabled: bool) {
    INLINE_AFTER_REDUNDANCY.with(|c| c.set(enabled));
}
pub fn inline_after_redundancy() -> bool {
    INLINE_AFTER_REDUNDANCY.with(|c| c.get())
}

/// Item J / decision 1924: extended-basic-block constant propagation and
/// folding, plus constant-condition branch resolution.
pub fn set_const_prop(enabled: bool) {
    CONST_PROP.with(|c| c.set(enabled));
}
pub fn const_prop() -> bool {
    CONST_PROP.with(|c| c.get())
}

/// Item J / decision 1925: value numbering of pure scalar computations
/// over an extended basic block.
pub fn set_gvn(enabled: bool) {
    GVN.with(|c| c.set(enabled));
}
pub fn gvn() -> bool {
    GVN.with(|c| c.get())
}

/// Item J / decision 1926: dead-code elimination — non-trapping pure
/// instructions whose result is read nowhere, and unreachable
/// instructions.
pub fn set_dce(enabled: bool) {
    DCE.with(|c| c.set(enabled));
}
pub fn dce() -> bool {
    DCE.with(|c| c.get())
}

/// Run whichever of item J's three passes are enabled, in pipeline order,
/// over a whole merged MWIR program. Returns `None` when every knob is
/// off, so `dev` and every pre-item-J opt list take the identical code
/// path they always did.
///
/// **Decision 1932 — all three passes rewrite only the fns the user's own
/// source declares.** [`is_late_bound`] names the rest, and they are
/// skipped as *callers* here just as decision 1929 skips them as
/// callees. There are three independent reasons and each one alone is
/// sufficient:
///
/// 1. **Their bodies are placeholders.** `layout.rs` replaces
///    `__test_call_{i}`, `__test_prefix_{i}`, `__method_{n}`,
///    `__enqueue_{i}`, `rt_enqueue <actor>` and `__wrela_abort_tail`
///    with hand-assembled code after codegen. Optimizing a placeholder
///    optimizes nothing, and *inlining* one was the miscompile decision
///    1929 records.
/// 2. **Their block partition is a committed measurement.**
///    `tests/golden/boot-actors/lane2-freq.txt` is a Lane 2 block-grain
///    frequency vector keyed `<fn_key>#<block_index>`, and it is
///    overwhelmingly runtime keys. Decision 1608's bridge fails closed
///    when a key names a block the scored program no longer has — which
///    is exactly what happens when a pass merges or deletes a runtime
///    block. Re-partitioning the runtime without re-measuring would make
///    the measured tier explain a program that does not exist. The
///    honest options were "re-measure on HVF" or "do not repartition";
///    this item takes the second and says so with the number it costs
///    (plans/codegen-pareto-2-J.md).
/// 3. **It puts the win where it can be attributed.** Item F measured
///    all four product cases moving by the identical −47 and −108,
///    because what moved lived in the shared runtime closure every one
///    of them borrows. Confining item J to app code means its number is
///    a statement about the *application*, which is the thing item M's
///    compositor was added to make visible.
/// **`flow` is how rule (i) stays honest (decision 1982).** The inliner's
/// first rule deletes a callee whose splice consumed its *only* reference
/// in the whole sealed program, and an async state machine references a
/// sync fn every bit as much as another MWIR body does. `codegen_program_with_async`
/// hands the `FlowWirProgram` in; `codegen_program` — the sync-only dump
/// entry — passes `None`, which is not an approximation there because no
/// FlowWir exists on that path. FlowWir itself is never rewritten
/// (decision 1927 stands); it is only *read*, as a reference source.
pub fn optimize(
    mwir: &MwirProgram,
    flow: Option<&FlowWirProgram>,
    layout: &LayoutCtx,
) -> Option<MwirProgram> {
    if !(inlining() || const_prop() || gvn() || dce()) {
        return None;
    }
    if !runtime_closure_is_known() {
        return None;
    }
    let mut prog = mwir.clone();
    // Decision 1981: the inliner runs on one side of the other three or
    // the other, and which side is the whole question item P exists to
    // answer. Ahead of them is the *enabling* order.
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
    Some(prog)
}

// --- instruction shape helpers ------------------------------------------
//
// MWIR has 53 instruction variants and no walker. These three functions
// are that walker, written as exhaustive matches so a new variant is a
// compile error rather than a silently unvisited temp. They are long and
// boring on purpose (CLAUDE.md: "prefer long obvious files").

/// Every `Temp` field of `inst`, in a fixed order, mutably.
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
        Inst::MakeEnum { dst, payload, .. } => {
            f(dst);
            for p in payload {
                f(p);
            }
        }
        Inst::StringConcat { dst, lhs, rhs, .. }
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
        // `Project`'s and `EnumPayload`'s `index` is a literal slot
        // number, not a temp; `MemLoad`/`PtrOffset`/`MmioRead` carry a
        // `u64` offset. `BytesIndexGet`'s `index` **is** a temp and
        // belongs with `IndexGet` below, not here — see decision 1930.
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
        | Inst::BytesIndexGet { dst, base, index }
        | Inst::PlacedIndexGet {
            dst, base, index, ..
        } => {
            f(dst);
            f(base);
            f(index);
        }
        Inst::IndexSet {
            base, index, value, ..
        }
        | Inst::PlacedIndexSet {
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

/// The temp `inst` writes a fresh value into, if it writes one at all.
/// **Not** the same as "every temp `inst` changes" — see [`clobbers`],
/// which also covers the in-place forms (`SetField`, `IndexSet`, a
/// `Call`'s write-backs).
fn def_of(inst: &Inst) -> Option<Temp> {
    match inst {
        Inst::ConstInt { dst, .. }
        | Inst::ConstBool { dst, .. }
        | Inst::ConstFloat { dst, .. }
        | Inst::ConstChar { dst, .. }
        | Inst::ConstUnit { dst }
        | Inst::ConstText { dst, .. }
        | Inst::Copy { dst, .. }
        | Inst::MakeAggregate { dst, .. }
        | Inst::FormatScalar { dst, .. }
        | Inst::StringConcat { dst, .. }
        | Inst::Project { dst, .. }
        | Inst::IndexGet { dst, .. }
        | Inst::PlacedIndexGet { dst, .. }
        | Inst::BytesIndexGet { dst, .. }
        | Inst::MakeEnum { dst, .. }
        | Inst::EnumTag { dst, .. }
        | Inst::EnumPayload { dst, .. }
        | Inst::ArithChecked { dst, .. }
        | Inst::ArithWrapping { dst, .. }
        | Inst::DivRem { dst, .. }
        | Inst::Shift { dst, .. }
        | Inst::Bitwise { dst, .. }
        | Inst::Compare { dst, .. }
        | Inst::Neg { dst, .. }
        | Inst::BitNot { dst, .. }
        | Inst::Convert { dst, .. }
        | Inst::Not { dst, .. }
        | Inst::BoolAnd { dst, .. }
        | Inst::Call { dst, .. }
        | Inst::MmioRead { dst, .. }
        | Inst::LoadIrqVector { dst, .. }
        | Inst::InterruptCellLoadAcquire { dst, .. }
        | Inst::InterruptCellSwapAcquire { dst, .. }
        | Inst::InterruptCellFetchOrRelease { dst, .. }
        | Inst::Now { dst }
        | Inst::Entropy { dst, .. }
        | Inst::MemLoad { dst, .. }
        | Inst::PtrOffset { dst, .. }
        | Inst::TurnAddrFromId { dst, .. } => Some(*dst),
        Inst::SetField { .. }
        | Inst::IndexSet { .. }
        | Inst::PlacedIndexSet { .. }
        | Inst::MemStore { .. }
        | Inst::MmioWrite { .. }
        | Inst::InterruptCellStoreRelease { .. }
        | Inst::Dmb { .. }
        | Inst::Wake { .. }
        | Inst::SlotMapMint { .. }
        | Inst::Jump { .. }
        | Inst::JumpIfFalse { .. }
        | Inst::Return { .. }
        | Inst::Abort { .. }
        | Inst::AssertFail { .. } => None,
    }
}

/// Every temp whose *value* this instruction may change: its `def_of`
/// plus the in-place mutation forms. Every table this file keeps is
/// invalidated on each of these.
fn clobbers(inst: &Inst, out: &mut Vec<Temp>) {
    out.clear();
    if let Some(d) = def_of(inst) {
        out.push(d);
    }
    match inst {
        Inst::SetField { base, .. }
        | Inst::IndexSet { base, .. }
        | Inst::PlacedIndexSet { base, .. }
        | Inst::MemStore { base, .. } => out.push(*base),
        Inst::SlotMapMint { map } => out.push(*map),
        Inst::Call { write_backs, .. } => {
            for (_, t) in write_backs {
                out.push(*t);
            }
        }
        _ => {}
    }
}

/// The temps this instruction *reads*. Used only for liveness, so an
/// in-place base counts as a read too (its old contents survive into the
/// new value).
fn reads_of(inst: &Inst, out: &mut Vec<Temp>) {
    out.clear();
    let def = def_of(inst);
    let mut first = true;
    let mut i = inst.clone();
    visit_temps_mut(&mut i, &mut |t| {
        if first && def.is_some() {
            // `visit_temps_mut` always emits the `dst` first for every
            // variant that has one.
            first = false;
            return;
        }
        first = false;
        out.push(*t);
    });
    // The in-place forms read their base as well as writing it.
    match inst {
        Inst::SetField { base, .. }
        | Inst::IndexSet { base, .. }
        | Inst::PlacedIndexSet { base, .. }
        | Inst::MemStore { base, .. } => out.push(*base),
        Inst::SlotMapMint { map } => out.push(*map),
        _ => {}
    }
}

/// A control-transfer instruction's own target, if it has one.
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

/// Does control fall through from this instruction to the next?
fn falls_through(inst: &Inst) -> bool {
    !matches!(
        inst,
        Inst::Jump { .. } | Inst::Return { .. } | Inst::Abort { .. } | Inst::AssertFail { .. }
    )
}

/// **The purity whitelist for GVN.** A pure *scalar* computation: it
/// reads and writes only scalar temps, touches no memory and no
/// aggregate, and depends on nothing an in-place write could change.
///
/// Trapping arithmetic (`ArithChecked`, `DivRem`, `Shift`, `Neg`,
/// `Convert`) **is** in this set, and that is sound in this direction:
/// reaching the second of two identical trapping computations proves the
/// first one did not trap, so the second cannot either, and both carry
/// the byte-identical abort wording. It is emphatically *not* sound in
/// DCE's direction, which is why [`dce_removable`] is a strictly smaller
/// set.
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

/// **The removability whitelist for DCE**, and it is deliberately
/// narrower than [`gvn_pure`]: nothing here can abandon.
///
/// `ArithChecked`, `DivRem`, `Shift`, `Neg`, `Convert`, `IndexGet` and
/// every memory read are excluded on purpose. A dead `let _ = a + b`
/// still abandons in the evaluator when it overflows, so deleting it
/// because its result is unread would make the backend disagree with the
/// reference implementation — exactly the divergence `diff-eval` exists
/// to catch. Fail closed (decision 1926).
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

// --- rewriting a body while keeping its jump targets honest --------------

/// Rebuild `body` keeping only the instructions `keep[i]` marks, and
/// rewrite every `Jump`/`JumpIfFalse` target through the resulting index
/// map. A target that pointed at a dropped instruction lands on the next
/// surviving one, which is why every caller of this may only drop
/// instructions that are semantically no-ops at that point. A target of
/// `body.len()` (fall off the end) maps to the new length.
fn compact(body: &mut Vec<Inst>, keep: &[bool]) {
    debug_assert_eq!(body.len(), keep.len());
    // `map[i]` = index of the first surviving instruction at or after i.
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

/// Indices that are the first instruction of a new value-table scope: a
/// jump target, index 0, or an instruction whose predecessor does not
/// fall through. Everything between two of these is an *extended basic
/// block* — a single-predecessor chain, so a value computed earlier in
/// it dominates every later point of it and no dominator tree is needed
/// (decision 1925).
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

/// **Decision 1929 — item J touches only keys the user's own source
/// declares.**
///
/// Every compiler-generated runtime/harness key is *late-bound*:
/// `layout.rs` **replaces** the compiled body of `__test_call_{i}`,
/// `__test_prefix_{i}`, `__method_{n}`, `__enqueue_{i}`,
/// `rt_enqueue <actor>`, `rt_boot_init 0` and `__wrela_abort_tail` with a
/// hand-assembled one *after* codegen has run. What this pass can see at
/// those keys is `rtconfig`'s placeholder — `return` / `return 0` — not
/// the program. Others (`__wrela_deadline_poll`, `__wrela_runtime_probe`)
/// are reached only by hand-written glue (`bl_call_key`) or by checkpoint
/// and ISR relocations, neither of which is an `Inst::Call` this pass
/// could count.
///
/// This is not a hypothetical. Before this rule existed, the (since
/// refused) inliner spliced `__test_prefix_0`'s one-instruction `return`
/// stub into `__wrela_test_append_prefix` and deleted the key, and the
/// guest printed a bare `ok` for every test with the test's name gone.
/// Units were green, both ∀ tiers were green; `diff-eval` is what caught
/// it.
fn is_late_bound(key: &str) -> bool {
    key.starts_with("__") || key.starts_with("rt_") || runtime_closure_keys().contains(key)
}

/// Every fn `stdlib/core/runtime.wr` declares, by the key `lower.rs`
/// gives it — the shared runtime closure decision 1932 keeps off limits.
///
/// The prefix test above catches most of it, but not all: the runtime
/// declares plain-named private helpers (`ascii_digit`,
/// `copy_bytes_range`, `copy_line_buf_range`, `turns`) that look exactly
/// like application code from inside `MwirProgram`, which loses module
/// provenance at `merge_mwir_programs`. `ascii_digit#21` is how that gap
/// was found: the committed Lane 2 vector names it, and DCE deleted one
/// of its blocks.
///
/// Read once per process and cached. **Fails closed**: if the toolchain's
/// runtime module cannot be read, [`optimize`] does nothing at all rather
/// than optimize a closure it cannot identify.
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

/// True when the runtime closure could be identified. Anything else is a
/// toolchain this pass will not guess about.
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

// --- pass 1: the inliner, parked --------------------------------------------

/// **Rule (ii)'s bound, counted rather than tuned** (item J's decision
/// 1921, rebuilt here). A call site deletes 8 emitted words: a
/// frame-carrying callee's prologue/epilogue/`ret` is 5 (`sub sp`,
/// `str lr`, `ldr lr`, `add sp`, `ret`), the `BL` is 1, and a
/// two-argument site's argument and result moves are 2. One MWIR
/// instruction of a scalar body is one emitted word under `NarrowImm`,
/// so a body of 8 is the break-even point on words. Nothing here
/// consults a score, a frequency or a profile.
const INLINE_MAX_BODY: usize = 8;

/// Leaf-only inlining, re-applied (decision 1983). A caller that has just
/// had its last call spliced away is itself a leaf, so the next round can
/// inline *it*. Four rounds, and it terminates without a recursion check
/// because a cycle is never a leaf: a self- or mutually recursive fn
/// contains an `Inst::Call` and is refused on that alone.
const INLINE_MAX_ROUNDS: usize = 4;

/// **Decision 1984 — the frame bound, computed conservatively.**
///
/// `codegen::build_frame` fails closed above 4 095 bytes (the `ADD`/`SUB`
/// `imm12` window). A splice adds one caller frame slot per callee temp
/// it moves, and this pass runs *before* `RegAlloc`, so the frame it must
/// respect is the spill-everything one. The estimate below is the
/// pessimistic version of `build_frame`'s own arithmetic — every temp
/// spilled, a self pointer, a pointer per parameter, a return pointer,
/// the link register, and a slack word for the entropy/reply scratch this
/// pass does not model. Over-estimating loses an inline; under-estimating
/// stops the program compiling, which is what happened to
/// `cost-icache-cliff` before item J had this bound at all.
const INLINE_FRAME_CEILING: usize = 4095;
const INLINE_FRAME_SLACK: usize = 64;

/// **The rule, and every refusal on it, in one place.**
///
/// `Some(reason)` means this callee is not inlinable. The reasons are
/// item J's (decision 1922), rebuilt: a receiver or a `mut`/`take`
/// parameter would need a write-back the splice does not model; an
/// `InterruptCell` op is an ordering-load-bearing access this pass will
/// not move; a non-leaf callee would make the pass need a recursion
/// check; and an assignment to the callee's *own parameter* is the one
/// case where binding parameters by **aliasing** rather than copying
/// would be observable in the caller.
///
/// `is_late_bound` leads the list, and that is decision 1932's
/// runtime-closure exclusion doing double duty. Those bodies are
/// `rtconfig` placeholders that `layout.rs` replaces after codegen with
/// hand-assembled code; splicing one inlines a stub *instead of* the
/// program. Item J did exactly that to `__test_prefix_0` and every guest
/// test line lost its name — units green, both ∀ tiers green, caught only
/// by `diff-eval` (item J §6, decision 1929).
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

/// How many times each fn key is referenced by the **whole sealed
/// program** — every MWIR body, including the late-bound ones, plus every
/// FlowWir state (decision 1982). A key referenced exactly once is rule
/// (i)'s case: the body *moves* into its one caller and the callee is
/// deleted.
///
/// FlowWir contributes four shapes: an embedded `Inst::Call`, a `Send`'s
/// and an awaited `ActorCall`'s `method_key`, and a `GroupStart`'s
/// `callee_key`. Counting a key this pass could never inline costs
/// nothing; *missing* one would delete a body something still calls.
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

/// **The shrinking inliner (decision 1980), stated as item J stated it.**
///
/// > A call site whose callee is *inlinable* is inlined when either
/// > **(i)** it is that callee's only reference in the whole sealed
/// > program — MWIR bodies and FlowWir states both — in which case the
/// > body *moves* rather than duplicates and the callee is deleted; or
/// > **(ii)** the callee's body is at most [`INLINE_MAX_BODY`] MWIR
/// > instructions.
/// >
/// > There are no other heuristics.
///
/// Parameters are bound by **aliasing**, not copying: the callee's
/// parameter temp is rewritten to the caller's argument temp, so a splice
/// is a strict deletion of the call sequence rather than a trade of a `BL`
/// for a run of `mov`s. That is only sound because [`inline_refusal`]
/// rejects every callee that writes to a parameter, directly or through
/// an in-place base; it is also what the ABI already does for an
/// aggregate argument, which `codegen` passes as a pointer to the
/// caller's own slot.
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
        }
        if !changed {
            return;
        }
    }
}

/// Splice every inlinable call site of one caller, one at a time,
/// rescanning after each because a splice renumbers the body. Returns
/// whether anything moved; `moved` collects the rule-(i) callees this
/// caller consumed, which [`inline_program`] then deletes.
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
            // A self-call is never a leaf, so this can only fire for a
            // caller that has already had a body spliced into it; refuse
            // it anyway rather than reason about the case.
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
            if !(single || callee.body.len() <= INLINE_MAX_BODY) {
                continue;
            }
            site = Some((i, key.clone(), single));
            break;
        }
        let Some((at, key, single)) = site else {
            return any;
        };
        if !splice(caller, at, &prog.fns[&key], layout) {
            // The only refusal `splice` makes that this loop cannot
            // simply skip past is the frame bound, and the frame only
            // grows — so stop inlining into this caller entirely rather
            // than keep a set of refused sites whose indices the next
            // splice would renumber. Dumb and obviously terminating.
            return any;
        }
        any = true;
        if single {
            moved.push(key);
        }
    }
}

/// The caller's frame under this pass's spill-everything assumption, or
/// `None` when a temp's size cannot be computed at all. See
/// [`INLINE_FRAME_CEILING`].
fn frame_estimate(temp_types: &[Type], f: &MwirFn, layout: &LayoutCtx) -> Option<usize> {
    let mut off = 0usize;
    for ty in temp_types {
        off += crate::mwir::size_of(ty, layout).ok()?;
    }
    off += 8; // self pointer
    off += 8 * f.params.len(); // a `mut` parameter's pointer slot
    off += 8; // aggregate-return pointer
    off += 8; // the link register
    off += INLINE_FRAME_SLACK;
    Some((off + 15) & !15)
}

/// Replace `caller.body[at]` — a `Call` — with `callee`'s body, renamed
/// into the caller's temp space and with every jump target remapped.
/// Returns `false` when the splice is refused, in which case `caller` is
/// untouched.
///
/// **Two index spaces and a two-phase map.** A callee `Return` expands to
/// up to two instructions (a copy into the call's `dst`, then a jump to
/// the join), so callee index `j` does not survive as expansion index
/// `j`. `start[j]` is the phase-one map from callee index to expansion
/// offset, `start[n]` is the join, and phase two adds `at` to turn that
/// into a caller index. Every *other* instruction of the caller shifts by
/// `expansion.len() - 1`.
fn splice(caller: &mut MwirFn, at: usize, callee: &MwirFn, layout: &LayoutCtx) -> bool {
    // Policy — [`inline_refusal`] and the two rules — lives in
    // [`inline_into`]. This is the mechanism, and it is kept callable
    // on any callee shape so the walker test below can splice one
    // instance of all 53 `Inst` variants at it.
    let Inst::Call { dst, args, .. } = caller.body[at].clone() else {
        return false;
    };
    if args.len() != callee.params.len() {
        return false;
    }

    // Phase zero: the temp map. A parameter *aliases* its argument; every
    // other temp the body actually mentions gets a fresh caller temp.
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

    // Phase one: the expansion, in callee target space.
    let n = callee.body.len();
    let mut start = vec![0usize; n + 1];
    let mut out: Vec<Inst> = Vec::with_capacity(n + 1);
    for (j, inst) in callee.body.iter().enumerate() {
        start[j] = out.len();
        let mut inst = inst.clone();
        visit_temps_mut(&mut inst, &mut |t| {
            // Every temp of every shape, through the one walker
            // `visit_temps_mut_visits_exactly_the_temps_the_dump_prints`
            // pins. A field the walker forgets is a silent miscompile
            // here and nowhere else (decision 1930).
            if let Some(r) = map.get(&t.0) {
                *t = *r;
            }
        });
        match inst {
            Inst::Return { value } => {
                if let Some(v) = value {
                    out.push(Inst::Copy { dst, src: v });
                }
                // A `Return` that is already the callee's last
                // instruction falls straight through to the join.
                if j + 1 != n {
                    out.push(Inst::Jump { target: n });
                }
            }
            other => out.push(other),
        }
    }
    start[n] = out.len();

    // Phase two: callee target space -> caller index space.
    for inst in &mut out {
        if let Some(t) = target_of(inst) {
            set_target(inst, at + start[t.min(n)]);
        }
    }
    // And the caller's own targets, which shift past the splice. A target
    // *at* the call site still points at the splice's first instruction.
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
    true
}

// --- pass 2: constant propagation and folding ----------------------------

/// Item J's SCCP slot. **Decision 1924 names it for what it is:**
/// MWIR is not SSA — `lower.rs` re-defines a loop's induction temp with
/// a `Copy` on every back edge — so the sparse SSA-lattice algorithm has
/// nothing to be sparse over. What lands is dense constant propagation
/// over an *extended basic block* (a single-predecessor chain, so every
/// fact established earlier in it holds at every later point of it),
/// with the table dropped at every join, plus resolution of a
/// `JumpIfFalse` whose condition is a known constant.
///
/// **Folding goes through the evaluator** (`eval::value`), which is the
/// reference implementation of these semantics (CLAUDE.md: "evaluator
/// before backend"). When the evaluator returns `Err` — an overflow, a
/// division by zero, an out-of-range shift or conversion — the
/// instruction is **left exactly as it was**, so the abandonment the
/// program is entitled to still happens at run time. Fail closed.
fn const_prop_fn(f: &mut MwirFn) {
    let leader = ebb_leaders(&f.body);
    let mut known: BTreeMap<Temp, Value> = BTreeMap::new();
    let mut clob = Vec::new();
    for i in 0..f.body.len() {
        if leader[i] {
            known.clear();
        }
        // Fold first, then record what the (possibly rewritten)
        // instruction now defines.
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
    // A `JumpIfFalse` whose condition folded to a constant becomes a
    // `Jump` or nothing at all; DCE then drops whichever arm went dead.
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

/// The reverse direction: a `Value` back into the `Inst` that
/// materializes it, or `None` when no constant instruction can.
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

/// Fold one instruction against the facts in `known`, or `None` to leave
/// it alone. Every arithmetic answer comes from `eval::value`; an `Err`
/// from it means the instruction may abandon and is therefore kept.
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
            // `eval_ordinary`'s own `checked_op` is `unreachable!` for
            // anything but these three.
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
            // Floats are excluded outright: NaN makes both the ordering
            // and the equality answers depend on a bit pattern this
            // table does not carry.
            if matches!(l, Value::F32(_) | Value::F64(_))
                || matches!(r, Value::F32(_) | Value::F64(_))
            {
                return None;
            }
            let value = match op {
                // `eval_compare` covers exactly the four orderings and
                // panics on anything else; equality is answered here on
                // the integer/bool/char shapes it is defined for.
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
            // `eval_neg` is `unreachable!` on an unsigned operand.
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

// --- pass 3: value numbering --------------------------------------------

/// The key a pure scalar computation is numbered by: the instruction
/// itself with its `dst` normalized away, so two instructions match iff
/// they compute the same value from the same operand temps.
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

/// **Decision 1925 — GVN, scoped to an extended basic block.**
///
/// A redundant pure scalar computation is replaced by a `Copy` from the
/// earlier result, and every *later* read of its destination inside the
/// same EBB is rewritten to read the earlier temp directly. The `Copy`
/// is what keeps a destination that is read after the EBB's end correct;
/// where it is not, [`collect_gvn_copies`] deletes it outright and
/// `RegAlloc`'s coalescing (item I) makes whatever survives free.
///
/// The scope is an EBB and not a dominator tree because that is the
/// dumb version that is obviously right: an EBB is a single-predecessor
/// chain, so every fact in it dominates every later point of it, and no
/// dominator computation exists to be wrong. Redundancy across a join is
/// left on the table, and the finding says so with a number.
///
/// Aggregates and every memory read are outside the whitelist, so no
/// alias question is ever asked: `SetField`/`IndexSet`/`MemStore` write
/// through a base temp, and two `MakeAggregate`s with equal elements are
/// two distinct mutable objects, not one value.
fn gvn_fn(f: &mut MwirFn) {
    let leader = ebb_leaders(&f.body);
    // Indices of the `Copy`s this pass introduces, so it can collect the
    // ones nothing reads rather than leaving them for `Dce` — see
    // `gvn_collects_its_own_copies` below for why that matters.
    let mut introduced: Vec<usize> = Vec::new();
    // (key, the temp holding that value).
    let mut table: Vec<(Inst, Temp)> = Vec::new();
    // Reads of `k` in this EBB are reads of `v`.
    let mut rewrite: BTreeMap<Temp, Temp> = BTreeMap::new();
    let mut clob = Vec::new();
    for i in 0..f.body.len() {
        if leader[i] {
            table.clear();
            rewrite.clear();
        }
        // Apply the EBB's rewrites to this instruction's *reads*.
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
                // The rewrite/table invalidation below still applies:
                // `dst` was redefined by the `Copy`.
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

/// **Decision 1936 — GVN collects its own leftovers, and that is what
/// makes it rankable alone.**
///
/// The `Copy` this pass leaves where a redundant computation stood is an
/// artifact of the implementation, not of the transform: the value is
/// already in `prev`, and every read inside the EBB was rewritten to say
/// so. Where nothing outside the EBB reads the old destination either,
/// the `Copy` is pure overhead.
///
/// Leaving it for `Dce` was measured and it is not free. Asked alone over
/// the shipped list, GVN-with-leftovers **raised** `cost-branch-bias` and
/// `cost-mem-locality` by a cycle each while falling by 20 833 overall —
/// and `CaseRose` is an absolute veto, so a 20 833-cycle transform was
/// refused for two microbenchmark cycles it had itself created. Ten lines
/// here is the answer; relaxing the veto would have been tuning the
/// ruler.
///
/// The test is whole-body and deliberately conservative — "read nowhere
/// in this function" — for the same reason [`dce_fn`]'s is: MWIR is not
/// SSA, so a temp can be defined at several points and only the question
/// that cannot be got wrong is worth asking.
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

/// Drop every table entry and every rewrite that mentions a clobbered
/// temp — in its key, in its value, or as the rewrite's own source.
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

// --- pass 4: dead code elimination --------------------------------------

/// **Decision 1926 — DCE, and what it refuses to delete.**
///
/// Two things go: an instruction from [`dce_removable`] whose
/// destination is read nowhere in the function, and an instruction no
/// path from entry reaches. "Read nowhere in the function" is
/// deliberately whole-body and not a liveness analysis — MWIR is not
/// SSA, a temp can be defined at several points, and the conservative
/// question is the one that cannot be got wrong.
///
/// Nothing that can abandon is ever removed, however dead its result:
/// the evaluator abandons on a dead overflowing add and so must the
/// backend, or `diff-eval` is measuring two different languages.
fn dce_fn(f: &mut MwirFn) {
    loop {
        let mut keep = vec![true; f.body.len()];
        // Unreachable first: a Jump whose target became a no-op leaves
        // whole runs of body behind.
        let reach = reachable(&f.body);
        let mut changed = false;
        for i in 0..f.body.len() {
            if !reach[i] {
                keep[i] = false;
                changed = true;
            }
        }
        // Then dead definitions, over what survives.
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

/// Which instructions a path from index 0 can reach.
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

    /// One instance of **every** `Inst` variant, with a distinct temp
    /// number in every temp-shaped field. The list is the input to
    /// `visit_temps_mut_visits_exactly_the_temps_the_dump_prints`, which
    /// is the oracle for the walker every pass in this file is built on.
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

    /// **The oracle for the walker, and it is not a formality.**
    ///
    /// Every pass in this file renames temps through
    /// [`visit_temps_mut`]. A temp-shaped field the walker forgets is
    /// silently *not* renamed when the inliner splices a callee, so the
    /// inlined body reads whatever the caller happens to hold at that
    /// number — a miscompile with no diagnostic anywhere.
    ///
    /// That is not hypothetical: `BytesIndexGet`'s `index` was grouped
    /// with `Project`'s literal slot number and went unvisited, and the
    /// guest printed 22 copies of the letter `t` instead of a test name
    /// (decision 1930). Units were green and both ∀ tiers were green.
    ///
    /// The expected answer is taken from `mwir::fmt_inst` — the
    /// `--stage=mwir` dump, an independently written, golden-pinned
    /// formatter that prints every temp field. Two independent
    /// enumerations of the same 53 variants have to agree.
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

    /// Every variant appears in the list above. A `mwir::Inst` gains a
    /// variant only rarely, and when it does the walker has to learn it —
    /// this is what says so.
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

    /// **Constant propagation fires**: the whole expression is one
    /// constant, and both the multiply and the shift (with its
    /// out-of-range abort path) are gone.
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
        // 6 * 7 = 42, >> 1 = 21 — the value, not merely "a constant".
        assert!(
            f.body
                .iter()
                .any(|i| matches!(i, Inst::ConstInt { value: 21, .. })),
            "the folded value must be 21:\n{f:?}"
        );
    }

    /// **Fail closed.** An overflowing constant expression is *not*
    /// folded: the evaluator abandons on it, so the backend must too.
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

    /// **GVN fires**: the second `x & y` becomes a copy of the first.
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

    /// **DCE fires**, and `Gvn` + `Dce` together delete rather than
    /// merely rename: the copy GVN left behind has no reader.
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

    /// **Fail closed, DCE's direction.** A dead checked add still
    /// abandons in the evaluator, so it may not be deleted for being
    /// unread — this is the asymmetry between [`gvn_pure`] and
    /// [`dce_removable`], asserted rather than argued.
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

    /// Every one of the three is off under `dev`, and `optimize` takes
    /// the identity path there — `dev` stays the correctness reference
    /// (M19 freeze 1407).
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

    // --- item P: the inliner, parked (decisions 1980-1989) --------------

    /// A body of `n` `U64` temps and nothing else, for the hand-built
    /// programs below. Every one of them is deliberately synthetic: the
    /// two miscompiles item J hit were both about *shapes* the source
    /// language does not conveniently produce on demand.
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

    /// **Rule (ii) fires**: an 8-instruction-or-smaller leaf is spliced
    /// at every call site, the callee stays (it has two references), and
    /// the arithmetic it contained is now the caller's.
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

    /// **Rule (i) fires**: a callee with exactly one reference in the
    /// whole program has its body *moved*, and the callee is deleted.
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

    /// **Item J §6's second miscompile, pinned** (decisions 1929/1932).
    ///
    /// `rtconfig` emits `__test_prefix_{i}` as a bare `return` stub and
    /// `layout.rs` injects the real `"test <name>: "` body *after*
    /// codegen. Item J's inliner saw a one-instruction single-call-site
    /// leaf, spliced the stub, and deleted the key before layout could
    /// inject anything — every guest test line became a bare `ok` with
    /// the name gone. Units were green and both ∀ tiers were green;
    /// `diff-eval` is what caught it.
    ///
    /// All three shapes of late-bound key are asked, including the
    /// plain-named `stdlib/core/runtime.wr` half that no prefix test
    /// catches.
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

    /// **Item J §6's first miscompile, pinned** (decision 1930).
    ///
    /// The splice renames every callee temp into the caller's space
    /// through [`visit_temps_mut`]. `BytesIndexGet`'s `index` was grouped
    /// with `Project`'s literal slot number and went unvisited, so a
    /// spliced `copy_bytes_range` kept reading the *callee's* loop
    /// counter — which in the caller held `bump`. The guest printed 22
    /// copies of the letter `t` where a test name should have been. Units
    /// were green; both ∀ tiers were green.
    ///
    /// `visit_temps_mut_visits_exactly_the_temps_the_dump_prints` pins
    /// the walker against the dump. **This pins the consequence**: splice
    /// a callee whose body is one instance of all 53 variants and assert
    /// that no callee temp number survives into the caller. The caller is
    /// given a temp space large enough that every callee number is below
    /// every fresh one, so an unrenamed field cannot hide as a plausible
    /// fresh temp.
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

    /// The refusals, stated as a table because that is what they are
    /// (decision 1922). Each row is a callee this pass may not touch.
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
        // A receiver, which needs a self pointer the splice does not bind.
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
        // And the one that is allowed, so the table above is not vacuous.
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

    /// A callee referenced from a **FlowWir** state is not a rule-(i)
    /// callee, however few MWIR call sites it has (decision 1982). This
    /// is the counting half of the rule; getting it wrong deletes a body
    /// an async state machine still calls.
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
        // The body is one instruction, so rule (ii) still splices the
        // sync site: what the FlowWir reference changes is the *deletion*.
        let mut without_flow = prog.clone();
        inline_program(&mut without_flow, None, &layout);
        assert!(
            !without_flow.fns.contains_key("p_shared_leaf"),
            "with no other reference at all, rule (i) moves the body and \
             deletes the key"
        );
    }

    /// A splice renumbers the caller's body, and every jump target on
    /// both sides of it has to follow. Asserted on a caller whose loop
    /// back edge straddles the call site.
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
        // The loop still exists: some target points backwards.
        assert!(
            f.body
                .iter()
                .enumerate()
                .any(|(i, inst)| target_of(inst).is_some_and(|t| t <= i)),
            "the back edge must survive the renumbering:\n{f:?}"
        );
    }

    /// The parked opt must not rot: both pipeline positions produce a
    /// program, deterministically, and `dev` still takes the identity
    /// path with the inliner off.
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

    /// Determinism: the same input twice gives the identical program.
    /// Every table in this file is a `BTreeMap`/`BTreeSet` or a `Vec`
    /// walked in index order, so this holds by construction (CLAUDE.md);
    /// this is the assertion that says so.
    #[test]
    fn the_passes_are_deterministic() {
        apply_mode(CompileMode::Release);
        let (mwir, layout) = lower(REDUNDANT);
        let a = optimize(&mwir, None, &layout).expect("ran");
        let b = optimize(&mwir, None, &layout).expect("ran");
        assert_eq!(crate::mwir::dump(&a), crate::mwir::dump(&b));
    }
}
