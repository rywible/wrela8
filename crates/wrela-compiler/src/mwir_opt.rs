//! plans/codegen-pareto-2.md item J — three plain passes over MWIR.
//!
//! The optimization ladder's 2b (GVN + SCCP + DCE), written the way
//! CLAUDE.md says to write them: three ordinary functions over the
//! existing IR, in the existing single-threaded batch pipeline. **No pass
//! manager, no trait over "a pass", no framework.** Each pass is one `fn`
//! taking `&mut MwirFn`, and `optimize` below calls whichever of them
//! their TLS knobs enable, in a fixed order.
//!
//! **The ladder's 2a — the shrinking inliner — was built, measured and
//! refused (decision 1935).** It worked: it spliced leaf callees, aliased
//! parameters rather than copying them, deleted every callee it emptied,
//! and `diff-eval` and three boot transcripts agreed with it. It also
//! **cost** words and cycles everywhere it was asked. Over the whole
//! `cost-*` corpus, added on top of the rest of the shipped list, it was
//! `+308` words and `+221` proxy cycles; asked alone against `dev`,
//! `+664` cycles; restricted to the ladder's rule (i) alone — a single
//! call site, where the body moves rather than duplicates and the callee
//! is deleted — still `+307`. On the two shipped images it was `+0` words
//! on the appliance (it has no customers there at all) and `+326` words
//! on the compositor, `7 197` against `6 871`. Losers are deleted, not
//! kept disabled, so it is gone; plans/codegen-pareto-2-J.md keeps the
//! numbers and the rule it was built on.
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
use crate::mwir::{Inst, MwirFn, MwirProgram, Temp};
use crate::sema::types::Type;
use crate::syntax::ast::BinOp;

// --- knobs ---------------------------------------------------------------

thread_local! {
    static CONST_PROP: Cell<bool> = const { Cell::new(false) };
    static GVN: Cell<bool> = const { Cell::new(false) };
    static DCE: Cell<bool> = const { Cell::new(false) };
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
pub fn optimize(mwir: &MwirProgram) -> Option<MwirProgram> {
    if !(const_prop() || gvn() || dce()) {
        return None;
    }
    if !runtime_closure_is_known() {
        return None;
    }
    let mut prog = mwir.clone();
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
        let (mwir, _layout) = lower(FOLDS);
        let opt = optimize(&mwir).expect("passes ran");
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
        let (mwir, _layout) = lower(OVERFLOWS);
        let opt = optimize(&mwir).expect("passes ran");
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
        let (mwir, _layout) = lower(REDUNDANT);
        let base = mwir.fns["twice"]
            .body
            .iter()
            .filter(|i| matches!(i, Inst::Bitwise { .. }))
            .count();
        assert_eq!(base, 2, "the baseline must really compute it twice");

        apply_opts(&[OptId::Gvn]);
        let opt = optimize(&mwir).expect("passes ran");
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
        let (mwir, _layout) = lower(REDUNDANT);
        let opt = optimize(&mwir).expect("passes ran");
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
        let (mwir, _layout) = lower(DEAD);
        let opt = optimize(&mwir).expect("passes ran");
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
        let (mwir, _layout) = lower(REDUNDANT);
        assert!(
            optimize(&mwir).is_none(),
            "dev must not clone or rewrite the program at all"
        );
        apply_mode(CompileMode::Release);
    }

    /// Determinism: the same input twice gives the identical program.
    /// Every table in this file is a `BTreeMap`/`BTreeSet` or a `Vec`
    /// walked in index order, so this holds by construction (CLAUDE.md);
    /// this is the assertion that says so.
    #[test]
    fn the_passes_are_deterministic() {
        apply_mode(CompileMode::Release);
        let (mwir, _layout) = lower(REDUNDANT);
        let a = optimize(&mwir).expect("ran");
        let b = optimize(&mwir).expect("ran");
        assert_eq!(crate::mwir::dump(&a), crate::mwir::dump(&b));
    }
}
